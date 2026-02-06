package com.drawingtablet.network

import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Protocol version - must match the Rust server.
 */
const val PROTOCOL_VERSION: Int = 1

/**
 * Packet type identifiers.
 */
object PacketType {
    const val VIDEO: Byte = 0x01
    const val INPUT: Byte = 0x02
    const val CONTROL: Byte = 0x03
}

/**
 * Video packet received from server.  Large frames are split into multiple
 * packets sharing the same [sequence]; the client reassembles them before
 * decoding.
 */
data class VideoPacket(
    val sequence: Int,
    val timestamp: Long,
    val isKeyframe: Boolean,
    val fragmentIndex: Int,
    val fragmentCount: Int,
    val data: ByteArray
) {
    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        if (javaClass != other?.javaClass) return false
        other as VideoPacket
        return sequence == other.sequence && fragmentIndex == other.fragmentIndex
    }

    override fun hashCode(): Int = sequence * 31 + fragmentIndex
}

/**
 * Input event types.
 */
sealed class InputEvent {
    data class StylusMove(
        val x: Float,
        val y: Float,
        val pressure: Float,
        val tiltX: Float,
        val tiltY: Float
    ) : InputEvent()

    data class StylusDown(
        val x: Float,
        val y: Float,
        val pressure: Float,
        val tiltX: Float,
        val tiltY: Float
    ) : InputEvent()

    object StylusUp : InputEvent()

    data class StylusButton(
        val button: Int,
        val pressed: Boolean
    ) : InputEvent()

    data class TouchDown(
        val id: Int,
        val x: Float,
        val y: Float
    ) : InputEvent()

    data class TouchMove(
        val id: Int,
        val x: Float,
        val y: Float
    ) : InputEvent()

    data class TouchUp(
        val id: Int
    ) : InputEvent()
}

/**
 * Input packet to send to server.
 */
data class InputPacket(
    val timestamp: Long,
    val event: InputEvent
)

/**
 * Control packet types.
 */
sealed class ControlPacket {
    data class Connect(val version: Int) : ControlPacket()
    data class ConnectAck(val width: Int, val height: Int) : ControlPacket()
    data class ConnectNack(val reason: String) : ControlPacket()
    object Heartbeat : ControlPacket()
    object RequestKeyframe : ControlPacket()
    object Disconnect : ControlPacket()
}

/**
 * Protocol codec for encoding/decoding packets.
 */
object ProtocolCodec {
    /**
     * Encode an input packet for transmission.
     */
    fun encodeInput(packet: InputPacket): ByteArray {
        val payload = encodeInputPayload(packet)
        return framePacket(PacketType.INPUT, payload)
    }

    /**
     * Encode a control packet for transmission.
     */
    fun encodeControl(packet: ControlPacket): ByteArray {
        val payload = encodeControlPayload(packet)
        return framePacket(PacketType.CONTROL, payload)
    }

    /**
     * Decode a received packet.
     */
    fun decode(data: ByteArray): Any? {
        if (data.size < 3) return null

        val type = data[0]
        val length = ByteBuffer.wrap(data, 1, 2)
            .order(ByteOrder.LITTLE_ENDIAN)
            .short.toInt() and 0xFFFF

        if (data.size < 3 + length) return null

        val payload = data.copyOfRange(3, 3 + length)

        return when (type) {
            PacketType.VIDEO -> decodeVideoPayload(payload)
            PacketType.CONTROL -> decodeControlPayload(payload)
            else -> null
        }
    }

    private fun framePacket(type: Byte, payload: ByteArray): ByteArray {
        val buffer = ByteBuffer.allocate(3 + payload.size)
            .order(ByteOrder.LITTLE_ENDIAN)
        buffer.put(type)
        buffer.putShort(payload.size.toShort())
        buffer.put(payload)
        return buffer.array()
    }

    private fun encodeInputPayload(packet: InputPacket): ByteArray {
        // Bincode-compatible encoding
        val buffer = ByteBuffer.allocate(256).order(ByteOrder.LITTLE_ENDIAN)
        buffer.putLong(packet.timestamp)

        when (val event = packet.event) {
            is InputEvent.StylusMove -> {
                buffer.putInt(0) // Variant index
                buffer.putFloat(event.x)
                buffer.putFloat(event.y)
                buffer.putFloat(event.pressure)
                buffer.putFloat(event.tiltX)
                buffer.putFloat(event.tiltY)
            }
            is InputEvent.StylusDown -> {
                buffer.putInt(1)
                buffer.putFloat(event.x)
                buffer.putFloat(event.y)
                buffer.putFloat(event.pressure)
                buffer.putFloat(event.tiltX)
                buffer.putFloat(event.tiltY)
            }
            is InputEvent.StylusUp -> {
                buffer.putInt(2)
            }
            is InputEvent.StylusButton -> {
                buffer.putInt(3)
                buffer.put(event.button.toByte())
                buffer.put(if (event.pressed) 1 else 0)
            }
            is InputEvent.TouchDown -> {
                buffer.putInt(4)
                buffer.putInt(event.id)
                buffer.putFloat(event.x)
                buffer.putFloat(event.y)
            }
            is InputEvent.TouchMove -> {
                buffer.putInt(5)
                buffer.putInt(event.id)
                buffer.putFloat(event.x)
                buffer.putFloat(event.y)
            }
            is InputEvent.TouchUp -> {
                buffer.putInt(6)
                buffer.putInt(event.id)
            }
        }

        val result = ByteArray(buffer.position())
        buffer.rewind()
        buffer.get(result)
        return result
    }

    private fun encodeControlPayload(packet: ControlPacket): ByteArray {
        val buffer = ByteBuffer.allocate(256).order(ByteOrder.LITTLE_ENDIAN)

        when (packet) {
            is ControlPacket.Connect -> {
                buffer.putInt(0) // Variant index
                buffer.putInt(packet.version)
            }
            is ControlPacket.Heartbeat -> {
                buffer.putInt(3)
            }
            is ControlPacket.RequestKeyframe -> {
                buffer.putInt(4)
            }
            is ControlPacket.Disconnect -> {
                buffer.putInt(5)
            }
            else -> {}
        }

        val result = ByteArray(buffer.position())
        buffer.rewind()
        buffer.get(result)
        return result
    }

    private fun decodeVideoPayload(payload: ByteArray): VideoPacket? {
        // u32 sequence + u64 timestamp + u8 isKeyframe + u16 fragIdx + u16 fragCnt
        //   + u64 vecLen = 4+8+1+2+2+8 = 25 bytes minimum
        if (payload.size < 25) return null

        val buffer = ByteBuffer.wrap(payload).order(ByteOrder.LITTLE_ENDIAN)
        val sequence = buffer.int
        val timestamp = buffer.long
        val isKeyframe = buffer.get() != 0.toByte()
        val fragmentIndex = buffer.short.toInt() and 0xFFFF
        val fragmentCount = buffer.short.toInt() and 0xFFFF

        // Read Vec<u8> - bincode encodes length as u64
        val dataLen = buffer.long.toInt()
        if (buffer.remaining() < dataLen) return null

        val data = ByteArray(dataLen)
        buffer.get(data)

        return VideoPacket(sequence, timestamp, isKeyframe, fragmentIndex, fragmentCount, data)
    }

    private fun decodeControlPayload(payload: ByteArray): ControlPacket? {
        if (payload.size < 4) return null

        val buffer = ByteBuffer.wrap(payload).order(ByteOrder.LITTLE_ENDIAN)
        val variant = buffer.int

        return when (variant) {
            1 -> { // ConnectAck
                if (buffer.remaining() < 8) return null
                val width = buffer.int
                val height = buffer.int
                ControlPacket.ConnectAck(width, height)
            }
            2 -> { // ConnectNack
                val reasonLen = buffer.long.toInt()
                if (buffer.remaining() < reasonLen) return null
                val reasonBytes = ByteArray(reasonLen)
                buffer.get(reasonBytes)
                ControlPacket.ConnectNack(String(reasonBytes))
            }
            3 -> ControlPacket.Heartbeat
            else -> null
        }
    }
}
