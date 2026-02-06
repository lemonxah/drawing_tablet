package com.drawingtablet.network

import android.util.Log
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException

private const val TAG = "UdpClient"
private const val HEARTBEAT_TIMEOUT_MS = 5000L

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
 * UDP client for communicating with the drawing tablet server.
 */
class UdpClient(
    private val serverAddress: String,
    private val serverPort: Int = 9999
) {
    private var socket: DatagramSocket? = null
    private var receiveJob: Job? = null
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
                socket = DatagramSocket().apply {
                    soTimeout = 5000 // 5 second timeout for initial connection
                }

                val address = InetAddress.getByName(serverAddress)

                // Send connect packet
                val connectPacket = ProtocolCodec.encodeControl(
                    ControlPacket.Connect(PROTOCOL_VERSION)
                )
                send(connectPacket, address, serverPort)

                // Wait for response
                val response = receive()
                when (val decoded = ProtocolCodec.decode(response)) {
                    is ControlPacket.ConnectAck -> {
                        Log.i(TAG, "Connected: ${decoded.width}x${decoded.height}")
                        socket?.soTimeout = 100 // Lower timeout for normal operation
                        lastActivityTime = System.currentTimeMillis()
                        _connectionState.value = ConnectionState.Connected(decoded.width, decoded.height)
                        startReceiveLoop(address, serverPort)
                        startHeartbeat(address, serverPort)
                    }
                    is ControlPacket.ConnectNack -> {
                        Log.e(TAG, "Connection refused: ${decoded.reason}")
                        _connectionState.value = ConnectionState.Error(decoded.reason)
                        socket?.close()
                        socket = null
                    }
                    else -> {
                        Log.e(TAG, "Unexpected response: $decoded")
                        _connectionState.value = ConnectionState.Error("Unexpected server response")
                        socket?.close()
                        socket = null
                    }
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Connection failed", e)
            _connectionState.value = ConnectionState.Error(e.message ?: "Connection failed")
            socket?.close()
            socket = null
        }
    }

    /**
     * Disconnect from the server.
     */
    fun disconnect() {
        scope.launch {
            try {
                socket?.let { sock ->
                    val address = InetAddress.getByName(serverAddress)
                    val disconnectPacket = ProtocolCodec.encodeControl(ControlPacket.Disconnect)
                    send(disconnectPacket, address, serverPort)
                }
            } catch (e: Exception) {
                Log.w(TAG, "Error sending disconnect", e)
            }
        }

        receiveJob?.cancel()
        heartbeatJob?.cancel()
        socket?.close()
        socket = null
        _connectionState.value = ConnectionState.Disconnected
    }

    /**
     * Send an input event to the server.
     */
    fun sendInput(event: InputEvent) {
        if (_connectionState.value !is ConnectionState.Connected) return

        scope.launch {
            try {
                val packet = InputPacket(
                    timestamp = System.currentTimeMillis() * 1000,
                    event = event
                )
                val data = ProtocolCodec.encodeInput(packet)
                val address = InetAddress.getByName(serverAddress)
                send(data, address, serverPort)
            } catch (e: Exception) {
                Log.w(TAG, "Failed to send input", e)
            }
        }
    }

    /**
     * Request a keyframe from the server.
     */
    fun requestKeyframe() {
        if (_connectionState.value !is ConnectionState.Connected) return

        scope.launch {
            try {
                val data = ProtocolCodec.encodeControl(ControlPacket.RequestKeyframe)
                val address = InetAddress.getByName(serverAddress)
                send(data, address, serverPort)
            } catch (e: Exception) {
                Log.w(TAG, "Failed to request keyframe", e)
            }
        }
    }

    private fun startReceiveLoop(address: InetAddress, port: Int) {
        receiveJob = scope.launch {
            while (isActive && _connectionState.value is ConnectionState.Connected) {
                try {
                    val data = receive()
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
                        is ControlPacket.Heartbeat -> {
                            // Server is alive – activity already stamped above
                        }
                        else -> {
                            // Ignore other packets
                        }
                    }
                } catch (e: SocketTimeoutException) {
                    // Check for heartbeat timeout (no data at all for 5 s)
                    if (lastActivityTime > 0 &&
                        System.currentTimeMillis() - lastActivityTime > HEARTBEAT_TIMEOUT_MS
                    ) {
                        Log.w(TAG, "Connection timed out – no activity for ${HEARTBEAT_TIMEOUT_MS}ms")
                        _connectionState.value = ConnectionState.Error("Connection lost")
                        break
                    }
                } catch (e: Exception) {
                    if (isActive) {
                        Log.e(TAG, "Receive error", e)
                    }
                }
            }
        }
    }

    private fun startHeartbeat(address: InetAddress, port: Int) {
        heartbeatJob = scope.launch {
            while (isActive && _connectionState.value is ConnectionState.Connected) {
                try {
                    val data = ProtocolCodec.encodeControl(ControlPacket.Heartbeat)
                    send(data, address, port)
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

    private suspend fun send(data: ByteArray, address: InetAddress, port: Int) {
        withContext(Dispatchers.IO) {
            socket?.send(DatagramPacket(data, data.size, address, port))
        }
    }

    private suspend fun receive(): ByteArray {
        return withContext(Dispatchers.IO) {
            val buffer = ByteArray(65536)
            val packet = DatagramPacket(buffer, buffer.size)
            socket?.receive(packet)
            buffer.copyOf(packet.length)
        }
    }

    fun close() {
        disconnect()
        scope.cancel()
    }
}
