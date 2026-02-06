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
        
        val context = androidx.compose.ui.platform.LocalContext.current
        val prefs = remember { PreferencesManager(context) }
        
        // Safety: Track rapid disconnects to prevent loops
        var disconnectCount by remember { mutableIntStateOf(0) }

        fun connectTo(address: String, port: Int) {
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

        // Auto-connect logic (Triggers on app start or recovery)
        LaunchedEffect(discoveredServers, currentClient) {
            // Only auto-connect if no client exists AND auto-connect is enabled
            if (currentClient == null && prefs.shouldAutoConnect) {
                val savedHost = prefs.rememberedServerHost
                val savedPort = prefs.rememberedServerPort
                if (savedHost != null) {
                    val found = discoveredServers.find { it.host == savedHost && it.port == savedPort }
                    if (found != null) {
                        connectTo(found.host, found.port)
                    }
                }
            }
        }
        
        // Connection Monitoring: Reset count on stable connection, Disable auto-connect on loops
        LaunchedEffect(connectionState) {
            when (connectionState) {
                is ConnectionState.Connected -> {
                    // If we stay connected for 5 seconds, reset the error count
                    kotlinx.coroutines.delay(5000)
                    disconnectCount = 0
                }
                is ConnectionState.Disconnected, is ConnectionState.Error -> {
                    // If we were trying to connect (client exists) and failed
                    if (currentClient != null) {
                        disconnectCount++
                        
                        if (disconnectCount >= 3) {
                            prefs.shouldAutoConnect = false
                            disconnectCount = 0
                            android.widget.Toast.makeText(
                                context, 
                                "Auto-connect disabled due to unstable connection", 
                                android.widget.Toast.LENGTH_LONG
                            ).show()
                        }
                        
                        // Reset client to allow retry (if not disabled)
                        // This enables the "Auto-reconnect" behavior
                        currentClient = null
                    }
                }
                else -> {}
            }
        }

        when (val state = connectionState) {
            is ConnectionState.Disconnected, is ConnectionState.Connecting, is ConnectionState.Error -> {
                ConnectionScreen(
                    connectionState = connectionState,
                    discoveredServers = discoveredServers,
                    onConnect = { address, port, remember ->
                        if (remember) {
                            prefs.rememberedServerHost = address
                            prefs.rememberedServerPort = port
                            prefs.shouldAutoConnect = true
                        } else {
                            // If manual connection without remember, disable auto-connect for future
                            prefs.shouldAutoConnect = false
                        }
                        connectTo(address, port)
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
