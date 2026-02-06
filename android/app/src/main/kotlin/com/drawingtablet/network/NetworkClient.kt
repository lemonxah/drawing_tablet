package com.drawingtablet.network

import android.util.Log
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.nio.ByteOrder

private const val TAG = "NetworkClient"
private const val HEARTBEAT_TIMEOUT_MS = 5000L
private const val TCP_CONNECT_TIMEOUT_MS = 5000
private const val TCP_READ_TIMEOUT_MS = 100

/** Accumulator for a fragmented video frame. */
private class FragmentBuffer(
    val sequence: Int,
    val timestamp: Long,
    val isKeyframe: Boolean,
    val fragmentCount: Int
) {
    val fragments = arrayOfNulls<ByteArray>(fragmentCount)
    var received = 0
}

/**
 * Connection state.
 */
sealed class ConnectionState {
    object Disconnected : ConnectionState()
    object Connecting : ConnectionState()
    data class Connected(val width: Int, val height: Int) : ConnectionState()
    data class Error(val message: String) : ConnectionState()
}

/**
 * Network client using TCP for input/control and UDP for video.
 * 
 * Protocol v2:
 * - TCP connection for reliable input events and control packets
 * - UDP socket for receiving low-latency video stream
 */
class NetworkClient(
    private val serverAddress: String,
    private val serverPort: Int = 9867
) {
    // TCP for input/control
    private var tcpSocket: Socket? = null
    private var tcpOutput: DataOutputStream? = null
    private var tcpInput: DataInputStream? = null
    private var tcpReceiveJob: Job? = null
    
    // UDP for video
    private var udpSocket: DatagramSocket? = null
    private var udpReceiveJob: Job? = null
    
    private var heartbeatJob: Job? = null
    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    private val _connectionState = MutableStateFlow<ConnectionState>(ConnectionState.Disconnected)
    val connectionState: StateFlow<ConnectionState> = _connectionState

    private val _videoPackets = Channel<VideoPacket>(
        capacity = 2,
        onBufferOverflow = BufferOverflow.DROP_OLDEST
    )
    val videoPackets: Channel<VideoPacket> = _videoPackets

    /** Pending fragment buffers keyed by sequence number. */
    private val fragmentBuffers = HashMap<Int, FragmentBuffer>()

    /** Timestamp of the last packet received from the server. */
    @Volatile
    private var lastActivityTime = 0L

    // Lock for TCP write operations
    private val tcpWriteLock = Any()

    /**
     * Connect to the server.
     */
    suspend fun connect() {
        if (_connectionState.value is ConnectionState.Connected) {
            return
        }

        _connectionState.value = ConnectionState.Connecting

        try {
            withContext(Dispatchers.IO) {
                val address = InetAddress.getByName(serverAddress)
                
                // 1. Create UDP socket first to know the local port
                udpSocket = DatagramSocket().apply {
                    soTimeout = 100 // Short timeout for non-blocking reads
                }
                val udpPort = udpSocket!!.localPort
                Log.i(TAG, "UDP socket bound to port $udpPort")
                
                // 2. Connect TCP socket
                tcpSocket = Socket().apply {
                    soTimeout = TCP_READ_TIMEOUT_MS
                    tcpNoDelay = true // Disable Nagle's algorithm for lower latency
                    connect(InetSocketAddress(address, serverPort), TCP_CONNECT_TIMEOUT_MS)
                }
                tcpOutput = DataOutputStream(tcpSocket!!.getOutputStream())
                tcpInput = DataInputStream(tcpSocket!!.getInputStream())
                Log.i(TAG, "TCP connected to $serverAddress:$serverPort")

                // 3. Send Connect packet over TCP
                val connectPacket = ProtocolCodec.encodeControl(
                    ControlPacket.Connect(PROTOCOL_VERSION)
                )
                sendTcp(connectPacket)

                // 4. Wait for ConnectAck
                val response = receiveTcpPacket()
                when (val decoded = ProtocolCodec.decode(response)) {
                    is ControlPacket.ConnectAck -> {
                        Log.i(TAG, "Connected: ${decoded.width}x${decoded.height}")
                        
                        // 5. Tell server our UDP port for video
                        val setPortPacket = ProtocolCodec.encodeControl(
                            ControlPacket.SetUdpPort(udpPort)
                        )
                        sendTcp(setPortPacket)
                        
                        lastActivityTime = System.currentTimeMillis()
                        _connectionState.value = ConnectionState.Connected(decoded.width, decoded.height)
                        
                        // Start receive loops
                        startTcpReceiveLoop()
                        startUdpReceiveLoop()
                        startHeartbeat()
                    }
                    is ControlPacket.ConnectNack -> {
                        Log.e(TAG, "Connection refused: ${decoded.reason}")
                        _connectionState.value = ConnectionState.Error(decoded.reason)
                        cleanup()
                    }
                    else -> {
                        Log.e(TAG, "Unexpected response: $decoded")
                        _connectionState.value = ConnectionState.Error("Unexpected server response")
                        cleanup()
                    }
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Connection failed", e)
            _connectionState.value = ConnectionState.Error(e.message ?: "Connection failed")
            cleanup()
        }
    }

    /**
     * Disconnect from the server.
     */
    fun disconnect() {
        scope.launch {
            try {
                val disconnectPacket = ProtocolCodec.encodeControl(ControlPacket.Disconnect)
                sendTcp(disconnectPacket)
            } catch (e: Exception) {
                Log.w(TAG, "Error sending disconnect", e)
            }
        }

        cleanup()
        _connectionState.value = ConnectionState.Disconnected
    }

    private fun cleanup() {
        tcpReceiveJob?.cancel()
        udpReceiveJob?.cancel()
        heartbeatJob?.cancel()
        
        try { tcpOutput?.close() } catch (e: Exception) {}
        try { tcpInput?.close() } catch (e: Exception) {}
        try { tcpSocket?.close() } catch (e: Exception) {}
        try { udpSocket?.close() } catch (e: Exception) {}
        
        tcpSocket = null
        tcpOutput = null
        tcpInput = null
        udpSocket = null
    }

    /**
     * Send an input event to the server (via TCP).
     */
    fun sendInput(event: TimestampedInputEvent) {
        if (_connectionState.value !is ConnectionState.Connected) return

        scope.launch {
            try {
                val packet = InputPacket(
                    timestamp = event.timestamp,
                    event = event.event
                )
                val data = ProtocolCodec.encodeInput(packet)
                sendTcp(data)
            } catch (e: Exception) {
                Log.w(TAG, "Failed to send input", e)
            }
        }
    }

    /**
     * Request a keyframe from the server (via TCP).
     */
    fun requestKeyframe() {
        if (_connectionState.value !is ConnectionState.Connected) return

        scope.launch {
            try {
                val data = ProtocolCodec.encodeControl(ControlPacket.RequestKeyframe)
                sendTcp(data)
            } catch (e: Exception) {
                Log.w(TAG, "Failed to request keyframe", e)
            }
        }
    }

    /**
     * Send data over TCP (thread-safe).
     */
    private fun sendTcp(data: ByteArray) {
        synchronized(tcpWriteLock) {
            tcpOutput?.let { out ->
                out.write(data)
                out.flush()
            }
        }
    }

    /**
     * Receive a single framed packet from TCP.
     * Blocking call - reads header then payload.
     */
    private fun receiveTcpPacket(): ByteArray {
        val input = tcpInput ?: throw IllegalStateException("TCP not connected")
        
        // Read header: [type: 1 byte][length: 2 bytes LE]
        val header = ByteArray(3)
        input.readFully(header)
        
        val length = ByteBuffer.wrap(header, 1, 2)
            .order(ByteOrder.LITTLE_ENDIAN)
            .short.toInt() and 0xFFFF
        
        // Read payload
        val payload = ByteArray(length)
        if (length > 0) {
            input.readFully(payload)
        }
        
        // Return full packet (header + payload)
        return header + payload
    }

    /**
     * Start TCP receive loop for control packets (heartbeats, etc.)
     */
    private fun startTcpReceiveLoop() {
        tcpReceiveJob = scope.launch {
            while (isActive && _connectionState.value is ConnectionState.Connected) {
                try {
                    val data = receiveTcpPacket()
                    lastActivityTime = System.currentTimeMillis()

                    when (val decoded = ProtocolCodec.decode(data)) {
                        is ControlPacket.Heartbeat -> {
                            // Server is alive
                            Log.d(TAG, "TCP heartbeat received")
                        }
                        else -> {
                            Log.d(TAG, "TCP received: $decoded")
                        }
                    }
                } catch (e: SocketTimeoutException) {
                    // Normal - check for overall timeout
                    if (lastActivityTime > 0 &&
                        System.currentTimeMillis() - lastActivityTime > HEARTBEAT_TIMEOUT_MS
                    ) {
                        Log.w(TAG, "Connection timed out – no activity for ${HEARTBEAT_TIMEOUT_MS}ms")
                        _connectionState.value = ConnectionState.Error("Connection lost")
                        break
                    }
                } catch (e: Exception) {
                    if (isActive) {
                        Log.e(TAG, "TCP receive error", e)
                        _connectionState.value = ConnectionState.Error("Connection lost: ${e.message}")
                    }
                    break
                }
            }
        }
    }

    /**
     * Start UDP receive loop for video packets.
     */
    private fun startUdpReceiveLoop() {
        udpReceiveJob = scope.launch {
            val buffer = ByteArray(65536)
            
            while (isActive && _connectionState.value is ConnectionState.Connected) {
                try {
                    val packet = DatagramPacket(buffer, buffer.size)
                    udpSocket?.receive(packet)
                    
                    val data = buffer.copyOf(packet.length)
                    lastActivityTime = System.currentTimeMillis()

                    when (val decoded = ProtocolCodec.decode(data)) {
                        is VideoPacket -> {
                            Log.d(TAG, "video frag seq=${decoded.sequence} " +
                                "${decoded.fragmentIndex}/${decoded.fragmentCount} " +
                                "${decoded.data.size}B keyframe=${decoded.isKeyframe}")
                            val complete = assembleFragment(decoded)
                            if (complete != null) {
                                Log.d(TAG, "assembled seq=${complete.sequence} " +
                                    "${complete.data.size}B keyframe=${complete.isKeyframe}")
                                _videoPackets.trySend(complete)
                            }
                        }
                        else -> {
                            // Ignore non-video packets on UDP
                        }
                    }
                } catch (e: SocketTimeoutException) {
                    // Normal timeout, continue
                } catch (e: Exception) {
                    if (isActive) {
                        Log.e(TAG, "UDP receive error", e)
                    }
                }
            }
        }
    }

    /**
     * Start heartbeat sender (via TCP).
     */
    private fun startHeartbeat() {
        heartbeatJob = scope.launch {
            while (isActive && _connectionState.value is ConnectionState.Connected) {
                try {
                    val data = ProtocolCodec.encodeControl(ControlPacket.Heartbeat)
                    sendTcp(data)
                } catch (e: Exception) {
                    Log.w(TAG, "Heartbeat failed", e)
                }
                delay(1000)
            }
        }
    }

    /**
     * Buffer a fragment and return a fully-reassembled VideoPacket once all
     * fragments for a given sequence number have arrived, or null if still
     * waiting.  Single-packet frames (fragmentCount == 1) pass through
     * immediately.
     */
    private fun assembleFragment(fragment: VideoPacket): VideoPacket? {
        if (fragment.fragmentCount == 1) {
            return fragment
        }

        val buf = fragmentBuffers.getOrPut(fragment.sequence) {
            FragmentBuffer(
                sequence = fragment.sequence,
                timestamp = fragment.timestamp,
                isKeyframe = fragment.isKeyframe,
                fragmentCount = fragment.fragmentCount
            )
        }

        // Guard: skip if index is out of range
        if (fragment.fragmentIndex < 0 || fragment.fragmentIndex >= buf.fragmentCount) {
            Log.w(TAG, "Bad fragment index ${fragment.fragmentIndex} for count ${buf.fragmentCount}")
            return null
        }

        // Duplicate fragment – ignore
        if (buf.fragments[fragment.fragmentIndex] != null) return null

        buf.fragments[fragment.fragmentIndex] = fragment.data
        buf.received++

        if (buf.received < buf.fragmentCount) {
            // Evict stale incomplete buffers (keep at most 3 pending frames)
            while (fragmentBuffers.size > 3) {
                val oldest = fragmentBuffers.keys.minOrNull() ?: break
                if (oldest != fragment.sequence) {
                    Log.d(TAG, "evicting incomplete seq=$oldest")
                    fragmentBuffers.remove(oldest)
                } else {
                    break
                }
            }
            return null
        }

        // All fragments present – concatenate in order
        fragmentBuffers.remove(fragment.sequence)
        val totalSize = buf.fragments.sumOf { it!!.size }
        val assembled = ByteArray(totalSize)
        var offset = 0
        for (piece in buf.fragments) {
            System.arraycopy(piece!!, 0, assembled, offset, piece.size)
            offset += piece.size
        }

        return VideoPacket(
            sequence = buf.sequence,
            timestamp = buf.timestamp,
            isKeyframe = buf.isKeyframe,
            fragmentIndex = 0,
            fragmentCount = 1,
            data = assembled
        )
    }

    fun close() {
        disconnect()
        scope.cancel()
    }
}
