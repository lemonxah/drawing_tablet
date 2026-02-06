package com.drawingtablet

import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.drawingtablet.network.ConnectionState
import com.drawingtablet.network.ServiceDiscovery
import com.drawingtablet.network.UdpClient
import com.drawingtablet.ui.ConnectionScreen
import com.drawingtablet.ui.StreamScreen
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    private var client: UdpClient? = null
    private lateinit var serviceDiscovery: ServiceDiscovery

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        serviceDiscovery = ServiceDiscovery(this)
        serviceDiscovery.start()

        // Keep screen on
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        // Hide system bars for immersive mode
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).let { controller ->
            controller.hide(WindowInsetsCompat.Type.systemBars())
            controller.systemBarsBehavior =
                WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }

        setContent {
            MaterialTheme {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background,
                ) {
                    DrawingTabletApp()
                }
            }
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        serviceDiscovery.stop()
        client?.close()
    }

    @Composable
    fun DrawingTabletApp() {
        var currentClient by remember { mutableStateOf<UdpClient?>(null) }
        val connectionState by currentClient?.connectionState?.collectAsState()
            ?: remember { mutableStateOf(ConnectionState.Disconnected) }
        val discoveredServers by serviceDiscovery.servers.collectAsState()
        val scope = rememberCoroutineScope()

        when (val state = connectionState) {
            is ConnectionState.Disconnected, is ConnectionState.Connecting, is ConnectionState.Error -> {
                ConnectionScreen(
                    connectionState = connectionState,
                    discoveredServers = discoveredServers,
                    onConnect = { address, port ->
                        // Close existing client if any
                        currentClient?.close()

                        // Create new client and connect
                        val newClient = UdpClient(address, port)
                        currentClient = newClient
                        client = newClient

                        scope.launch {
                            newClient.connect()
                        }
                    }
                )
            }
            is ConnectionState.Connected -> {
                currentClient?.let { client ->
                    StreamScreen(
                        client = client,
                        videoPackets = client.videoPackets,
                        width = state.width,
                        height = state.height,
                        onDisconnect = {
                            client.disconnect()
                            currentClient = null
                        }
                    )
                }
            }
        }
    }
}
