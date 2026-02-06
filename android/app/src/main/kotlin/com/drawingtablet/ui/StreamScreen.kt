package com.drawingtablet.ui

import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInteropFilter
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.drawingtablet.input.InputCapture
import com.drawingtablet.network.NetworkClient
import com.drawingtablet.network.VideoPacket
import com.drawingtablet.video.VideoDecoder
import kotlinx.coroutines.channels.Channel

@OptIn(ExperimentalComposeUiApi::class)
@Composable
fun StreamScreen(
    client: NetworkClient,
    videoPackets: Channel<VideoPacket>,
    width: Int,
    height: Int,
    onDisconnect: () -> Unit,
    modifier: Modifier = Modifier
) {
    var isDecoderReady by remember { mutableStateOf(false) }
    var viewWidth by remember { mutableIntStateOf(1) }
    var viewHeight by remember { mutableIntStateOf(1) }
    var showStats by remember { mutableStateOf(false) }

    val decoder = remember { VideoDecoder() }
    val fps by decoder.fps.collectAsState()

    DisposableEffect(Unit) {
        onDispose {
            decoder.close()
        }
    }

    // Black background behind the video so letterboxing margins are black
    Box(
        modifier = modifier.fillMaxSize().background(Color.Black),
        contentAlignment = Alignment.Center
    ) {
        // Video surface – constrained to the stream's aspect ratio so it
        // letterboxes/pillarboxes correctly on any screen shape
        AndroidView(
            factory = { context ->
                SurfaceView(context).apply {
                    holder.addCallback(object : SurfaceHolder.Callback {
                        override fun surfaceCreated(holder: SurfaceHolder) {
                            decoder.configure(width, height, holder.surface)
                            decoder.startDecoding(videoPackets)
                            isDecoderReady = true
                            client.requestKeyframe()
                        }

                        override fun surfaceChanged(
                            holder: SurfaceHolder,
                            format: Int,
                            w: Int,
                            h: Int
                        ) {
                            viewWidth = w
                            viewHeight = h
                        }

                        override fun surfaceDestroyed(holder: SurfaceHolder) {
                            isDecoderReady = false
                            decoder.stop()
                        }
                    })

                    // Handle generic motion events (Hover, Stylus Buttons)
                    setOnGenericMotionListener { _, event ->
                        if (!isDecoderReady) return@setOnGenericMotionListener false
                        val inputEvents = InputCapture.processMotionEvent(event, viewWidth, viewHeight)
                        for (inputEvent in inputEvents) {
                            client.sendInput(inputEvent)
                        }
                        true
                    }

                    // Handle touch events (Down, Move, Up)
                    setOnTouchListener { _, event ->
                        if (!isDecoderReady) return@setOnTouchListener false
                        val inputEvents = InputCapture.processMotionEvent(event, viewWidth, viewHeight)
                        for (inputEvent in inputEvents) {
                            client.sendInput(inputEvent)
                        }
                        true
                    }
                }
            },
            modifier = Modifier
                .fillMaxSize()
                .aspectRatio(width.toFloat() / height)
        )

        // ── Stats overlay (top-left) ─────────────────────────────────
        if (showStats) {
            Box(
                modifier = Modifier.fillMaxSize(),
                contentAlignment = Alignment.TopStart
            ) {
                Card(
                    colors = CardDefaults.cardColors(
                        containerColor = Color.Black.copy(alpha = 0.72f)
                    ),
                    modifier = Modifier.padding(12.dp)
                ) {
                    Column(modifier = Modifier.padding(12.dp)) {
                        Text(
                            text = "$fps FPS",
                            color = Color.White,
                            fontWeight = FontWeight.Bold,
                            fontSize = 22.sp
                        )
                        Text(
                            text = "${width}\u00d7${height}",
                            color = Color.White.copy(alpha = 0.7f),
                            fontSize = 14.sp
                        )
                    }
                }
            }
        }

        // ── Toggle buttons (bottom-right) ────────────────────────────
        Box(
            modifier = Modifier.fillMaxSize(),
            contentAlignment = Alignment.BottomEnd
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.padding(12.dp)
            ) {
                var isTouchEnabled by remember { mutableStateOf(true) }

                // Touch Toggle (Hand/Finger)
                IconButton(
                    onClick = {
                        isTouchEnabled = !isTouchEnabled
                        InputCapture.isTouchEnabled = isTouchEnabled
                    },
                    colors = IconButtonDefaults.iconButtonColors(
                        containerColor = if (isTouchEnabled) 
                            Color.Black.copy(alpha = 0.5f) // Unlocked/Default
                        else 
                            MaterialTheme.colorScheme.primary // Locked/Active Mode
                    ),
                    modifier = Modifier.size(44.dp)
                ) {
                    Icon(
                        imageVector = if (isTouchEnabled) Icons.Filled.Check else Icons.Filled.Lock,
                        contentDescription = if (isTouchEnabled) "Touch Enabled" else "Touch Disabled (Pen Only)",
                        tint = Color.White
                    )
                }

                Spacer(modifier = Modifier.width(12.dp))

                // Stats Toggle
                IconButton(
                    onClick = { showStats = !showStats },
                    colors = IconButtonDefaults.iconButtonColors(
                        containerColor = Color.Black.copy(alpha = 0.5f)
                    ),
                    modifier = Modifier.size(44.dp)
                ) {
                    Icon(
                        imageVector = Icons.Filled.Info,
                        contentDescription = "Toggle stats",
                        tint = Color.White
                    )
                }
            }
        }

        // ── Spinner while decoder initialises ────────────────────────
        if (!isDecoderReady) {
            CircularProgressIndicator()
        }
    }
}
