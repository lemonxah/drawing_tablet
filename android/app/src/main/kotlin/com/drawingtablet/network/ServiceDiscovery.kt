package com.drawingtablet.network

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.util.concurrent.Executors

private const val TAG = "ServiceDiscovery"
private const val SERVICE_TYPE = "_drawingtablet._udp."

/**
 * A discovered Drawing Tablet server on the local network.
 */
data class DiscoveredServer(
    val name: String,
    val host: String,
    val port: Int
)

/**
 * Discovers Drawing Tablet servers via mDNS (NSD).
 *
 * Call [start] when the app is ready to browse, and [stop] when it is no
 * longer needed.  Observed servers are exposed via the [servers] flow.
 */
class ServiceDiscovery(context: Context) {
    private val nsdManager = context.getSystemService(Context.NSD_SERVICE) as NsdManager

    private val _servers = MutableStateFlow<List<DiscoveredServer>>(emptyList())
    val servers: StateFlow<List<DiscoveredServer>> = _servers

    fun start() {
        try {
            nsdManager.discoverServices(SERVICE_TYPE, 1 /* PROTOCOL_NSD */, discoveryListener)
            Log.i(TAG, "Service discovery started")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start discovery", e)
        }
    }

    fun stop() {
        try {
            nsdManager.stopServiceDiscovery(discoveryListener)
        } catch (_: Exception) {}
    }

    private val discoveryListener = object : NsdManager.DiscoveryListener {
        override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
            Log.e(TAG, "Discovery start failed: $errorCode")
        }

        override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
            Log.e(TAG, "Discovery stop failed: $errorCode")
        }

        override fun onDiscoveryStarted(serviceType: String) {}
        override fun onDiscoveryStopped(serviceType: String) {}

        override fun onServiceFound(serviceInfo: NsdServiceInfo) {
            @Suppress("DEPRECATION")
            nsdManager.resolveService(serviceInfo, Executors.newSingleThreadExecutor(),
                object : NsdManager.ResolveListener {
                    override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) {
                        Log.w(TAG, "Resolve failed for '${info.serviceName}': $errorCode")
                    }

                    override fun onServiceResolved(info: NsdServiceInfo) {
                        val host = info.hostAddresses?.firstOrNull()?.hostAddress ?: return
                        val server = DiscoveredServer(
                            name = info.serviceName,
                            host = host,
                            port = info.port
                        )
                        _servers.value = _servers.value.filterNot { it.name == server.name } + server
                        Log.i(TAG, "Resolved server: $server")
                    }
                }
            )
        }

        override fun onServiceLost(serviceInfo: NsdServiceInfo) {
            _servers.value = _servers.value.filterNot { it.name == serviceInfo.serviceName }
            Log.i(TAG, "Server lost: ${serviceInfo.serviceName}")
        }
    }
}
