package com.drawingtablet.video

import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log
import android.view.Surface
import com.drawingtablet.network.VideoPacket
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.nio.ByteBuffer
import java.util.LinkedList
import java.util.Queue

private const val TAG = "VideoDecoder"
private const val MIME_TYPE = "video/avc"

/**
 * Hardware H.264 video decoder using MediaCodec.
 */
class VideoDecoder {
    private var codec: MediaCodec? = null
    private var surface: Surface? = null
    private var decodeJob: Job? = null
    private val scope = CoroutineScope(Dispatchers.Default + SupervisorJob())

    // Codec Specific Data (SPS and PPS)
    private var csd0: ByteBuffer? = null // SPS
    private var csd1: ByteBuffer? = null // PPS

    private val pendingConfigPackets: Queue<VideoPacket> = LinkedList()
    

    private var width: Int = 1920
    private var height: Int = 1080
    private var isConfigured = false
    private var waitingForKeyframe = true
    private var framesRendered = 0

    // FPS tracking – updated once per second inside decode()
    private var fpsFrameCount = 0
    private var lastFpsTime = System.nanoTime()
    private val _fps = MutableStateFlow(0)
    val fps: StateFlow<Int> = _fps

    /**
     * Configure the decoder with the given dimensions and output surface.
     */
    fun configure(width: Int, height: Int, surface: Surface, csd0: ByteBuffer? = null, csd1: ByteBuffer? = null) {
        this.width = width
        this.height = height
        this.surface = surface
        this.csd0 = csd0
        this.csd1 = csd1

        try {
            codec?.release()
            codec = MediaCodec.createDecoderByType(MIME_TYPE).apply {
                val format = MediaFormat.createVideoFormat(MIME_TYPE, width, height).apply {
                    setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 1024 * 1024)
                    setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                    csd0?.let { setByteBuffer("csd-0", it) }
                    csd1?.let { setByteBuffer("csd-1", it) }
                }
                configure(format, surface, null, 0)
                start()
            }
            isConfigured = true
            waitingForKeyframe = (csd0 == null || csd1 == null)
            Log.i(TAG, "Decoder configured: ${width}x${height}, csd0=${csd0 != null}, csd1=${csd1 != null}")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to configure decoder", e)
            isConfigured = false
            waitingForKeyframe = true
        }
    }

    /**
     * Start decoding video packets from the given channel.
     */
    fun startDecoding(packets: Channel<VideoPacket>) {
        if (!isConfigured) {
            Log.w(TAG, "Decoder not configured")
            return
        }

        decodeJob = scope.launch {
            for (packet in packets) {
                if (!isActive) break
                decode(packet)
            }
        }
    }

    /**
     * Decode a single video packet.
     */
    fun decode(packet: VideoPacket) {
        val currentSurface = surface ?: run {
            Log.w(TAG, "No surface available to decode to")
            return
        }

        if (waitingForKeyframe) {
            if (!packet.isKeyframe) {
                Log.d(TAG, "Dropping non-keyframe seq=${packet.sequence} while waiting for keyframe")
                pendingConfigPackets.add(packet) // Queue non-keyframes until configured
                return
            }
            Log.i(TAG, "Got keyframe seq=${packet.sequence} (${packet.data.size}B), attempting to extract CSD")

            // Try to extract SPS and PPS from the keyframe
            val (extractedSps, extractedPps) = extractH264Csd(packet.data)

            if (extractedSps != null && extractedPps != null) {
                if (csd0 != extractedSps || csd1 != extractedPps) {
                    Log.i(TAG, "Extracted new SPS/PPS. Reconfiguring decoder.")
                    // Reconfigure the decoder with the new CSD
                    // Stop and release existing codec first if it exists
                    try {
                        codec?.stop()
                        codec?.release()
                        codec = null
                    } catch (e: Exception) {
                        Log.w(TAG, "Failed to stop/release old codec during reconfig", e)
                    }
                    configure(width, height, currentSurface, extractedSps, extractedPps)
                } else {
                    Log.i(TAG, "SPS/PPS are the same. No reconfiguration needed.")
                    waitingForKeyframe = false
                }

            } else {
                Log.w(TAG, "Could not extract SPS/PPS from keyframe seq=${packet.sequence}. Will retry with next keyframe.")
                pendingConfigPackets.add(packet) // Keep the keyframe in queue if CSD extraction failed
                return
            }
        }

        // If we are configured (or just reconfigured), process pending packets first
        while (pendingConfigPackets.isNotEmpty()) {
            val pendingPacket = pendingConfigPackets.poll()
            if (pendingPacket != null) {
                Log.d(TAG, "Processing pending packet seq=${pendingPacket.sequence}")
                queueAndDrain(pendingPacket)
            }
        }

        // Process the current packet (if not already processed as part of pending)
        if (!waitingForKeyframe || packet.isKeyframe) {
            queueAndDrain(packet)
        }
    }

    private fun queueAndDrain(packet: VideoPacket) {
        val codec = codec ?: return

        try {
            // Get input buffer
            val inputIndex = codec.dequeueInputBuffer(10000) // 10ms timeout
            if (inputIndex < 0) {
                Log.w(TAG, "No input buffer available for seq=${packet.sequence}")
                return
            }

            val inputBuffer = codec.getInputBuffer(inputIndex) ?: return
            inputBuffer.clear()
            inputBuffer.put(packet.data)

            val flags = if (packet.isKeyframe) {
                MediaCodec.BUFFER_FLAG_KEY_FRAME
            } else {
                0
            }

            codec.queueInputBuffer(
                inputIndex,
                0,
                packet.data.size,
                packet.timestamp,
                flags
            )

            // Drain all available output buffers
            val bufferInfo = MediaCodec.BufferInfo()
            var outputIndex = codec.dequeueOutputBuffer(bufferInfo, 0)

            while (outputIndex != MediaCodec.INFO_TRY_AGAIN_LATER) {
                if (outputIndex >= 0) {
                    codec.releaseOutputBuffer(outputIndex, true) // Render to surface
                    framesRendered++
                    fpsFrameCount++

                    // Update FPS once per second
                    val now = System.nanoTime()
                    if (now - lastFpsTime >= 1_000_000_000L) {
                        _fps.value = fpsFrameCount
                        fpsFrameCount = 0
                        lastFpsTime = now
                    }

                    if (framesRendered <= 5 || framesRendered % 60 == 0) {
                        Log.i(TAG, "Rendered frame #$framesRendered pts=${bufferInfo.presentationTimeUs} fps=${_fps.value}")
                    }
                } else if (outputIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    Log.i(TAG, "Output format changed: ${codec.outputFormat}")
                    // Continue draining – do NOT exit the loop here
                }
                outputIndex = codec.dequeueOutputBuffer(bufferInfo, 0)
            }
        } catch (e: Exception) {
            Log.e(TAG, "Decode error at seq=${packet.sequence}", e)
            waitingForKeyframe = true
        }
    }

    /**
     * Reset decoder state (request keyframe).
     */
    fun reset() {
        waitingForKeyframe = true
        try {
            codec?.flush()
        } catch (e: Exception) {
            Log.w(TAG, "Flush failed", e)
        }
    }

    /**
     * Stop decoding and release resources.
     */
    fun stop() {
        decodeJob?.cancel()
        try {
            codec?.stop()
            codec?.release()
        } catch (e: Exception) {
            Log.w(TAG, "Stop failed", e)
        }
        codec = null
        isConfigured = false
    }

    fun close() {
        stop()
        scope.cancel()
    }
}

/**
 * Extracts H.264 SPS and PPS NAL units from a byte array containing H.264 byte-stream data.
 * Assumes NAL units are prefixed with start codes (0x00000001 or 0x000001).
 * Returns a Pair<ByteBuffer, ByteBuffer> where first is SPS (CSD-0) and second is PPS (CSD-1).
 */
fun extractH264Csd(data: ByteArray): Pair<ByteBuffer?, ByteBuffer?> {
    var sps: ByteBuffer? = null
    var pps: ByteBuffer? = null

    var i = 0
    while (i < data.size - 4) {
        // Look for NAL unit start code (0x00 0x00 0x00 0x01 or 0x00 0x00 0x01)
        if (data[i].toInt() == 0x00 && data[i + 1].toInt() == 0x00) {
            var nalStart = -1
            if (data[i + 2].toInt() == 0x01) {
                nalStart = i + 3
            } else if (data[i + 2].toInt() == 0x00 && data[i + 3].toInt() == 0x01) {
                nalStart = i + 4
            }

            if (nalStart != -1 && nalStart < data.size) {
                val nalType = data[nalStart].toInt() and 0x1F

                // Find the end of the current NAL unit (next start code or end of data)
                var nalEnd = data.size
                for (j in nalStart until data.size - 3) {
                    if (data[j].toInt() == 0x00 && data[j + 1].toInt() == 0x00 && data[j + 2].toInt() == 0x01) {
                        nalEnd = j
                        break
                    }
                    if (data[j].toInt() == 0x00 && data[j + 1].toInt() == 0x00 && data[j + 2].toInt() == 0x00 && data[j + 3].toInt() == 0x01) {
                        nalEnd = j
                        break
                    }
                }

                val nalUnit = ByteBuffer.wrap(data, nalStart, nalEnd - nalStart)

                when (nalType) {
                    7 -> sps = nalUnit // SPS
                    8 -> pps = nalUnit // PPS
                }

                if (sps != null && pps != null) {
                    return Pair(sps, pps)
                }
                i = nalEnd // Move to the next NAL unit
                continue
            }
        }
        i++
    }
    return Pair(sps, pps)
}
