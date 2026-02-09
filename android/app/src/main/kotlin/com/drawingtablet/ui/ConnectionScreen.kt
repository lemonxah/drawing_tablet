package com.drawingtablet.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import com.drawingtablet.R
import com.drawingtablet.network.ConnectionState
import com.drawingtablet.network.DiscoveredServer

@Composable
fun ConnectionScreen(
    connectionState: ConnectionState,
    discoveredServers: List<DiscoveredServer>,
    onConnect: (String, Int, Boolean) -> Unit,
    modifier: Modifier = Modifier
) {
    val isConnecting = connectionState is ConnectionState.Connecting
    val errorMessage = (connectionState as? ConnectionState.Error)?.message
    var rememberServer by remember { mutableStateOf(false) }

    Box(
        modifier = modifier
            .fillMaxSize()
            .background(
                brush = Brush.verticalGradient(
                    colors = listOf(
                        Color(0xFF1A2E24), // Dark teal/black
                        Color(0xFF163E2E)  // Deep teal
                    )
                )
            )
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(32.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center
        ) {
            // ── Header ─────────────────────────────────────────────────
            Image(
                painter = painterResource(id = R.mipmap.ic_launcher),
                contentDescription = "Drawing Tablet",
                modifier = Modifier
                    .size(120.dp)
                    .clip(RoundedCornerShape(24.dp))
            )
            Spacer(modifier = Modifier.height(16.dp))
            Text(
                text = "Drawing Tablet",
                style = MaterialTheme.typography.headlineLarge,
                color = Color.White
            )

            Spacer(modifier = Modifier.height(48.dp))

            // ── Discovery / List ───────────────────────────────────────
            if (discoveredServers.isEmpty()) {
                CircularProgressIndicator(
                    color = Color(0xFF5BA88C),
                    modifier = Modifier.size(48.dp)
                )
                Spacer(modifier = Modifier.height(24.dp))
                Text(
                    text = "Looking for drawing tablet server...",
                    style = MaterialTheme.typography.titleMedium,
                    color = Color.White
                )
                Spacer(modifier = Modifier.height(8.dp))
                Text(
                    text = "Please start the server on your computer",
                    style = MaterialTheme.typography.bodyMedium,
                    color = Color.LightGray
                )
            } else {
                Text(
                    text = "Available Servers",
                    style = MaterialTheme.typography.titleMedium,
                    color = Color.LightGray,
                    modifier = Modifier.align(Alignment.Start)
                )
                Spacer(modifier = Modifier.height(16.dp))
                
                LazyColumn(
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                    modifier = Modifier.fillMaxWidth().weight(1f, fill = false)
                ) {
                    items(discoveredServers, key = { it.name }) { server ->
                        ServerCard(
                            server = server,
                            enabled = !isConnecting,
                            onClick = { onConnect(server.host, server.port, rememberServer) }
                        )
                    }
                }
                
                Spacer(modifier = Modifier.height(16.dp))

                // Remember Checkbox
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { rememberServer = !rememberServer },
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.Center
                ) {
                    Checkbox(
                        checked = rememberServer,
                        onCheckedChange = { rememberServer = it },
                        colors = CheckboxDefaults.colors(
                            checkedColor = Color(0xFF5BA88C),
                            uncheckedColor = Color.Gray,
                            checkmarkColor = Color.White
                        )
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text(
                        text = "Remember this server (Auto-connect)",
                        style = MaterialTheme.typography.bodyMedium,
                        color = Color.LightGray
                    )
                }
            }

            // ── Error Message ──────────────────────────────────────────
            if (errorMessage != null) {
                Spacer(modifier = Modifier.height(24.dp))
                Card(
                    colors = CardDefaults.cardColors(
                        containerColor = MaterialTheme.colorScheme.errorContainer
                    )
                ) {
                    Text(
                        text = errorMessage,
                        color = MaterialTheme.colorScheme.onErrorContainer,
                        modifier = Modifier.padding(16.dp)
                    )
                }
            }
        }
    }
}

/** A single discovered-server card with host:port. */
@Composable
private fun ServerCard(
    server: DiscoveredServer,
    enabled: Boolean,
    onClick: () -> Unit
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(enabled = enabled, onClick = onClick),
        colors = CardDefaults.cardColors(
            containerColor = Color(0xFF0F4634) // Teal card
        )
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween
        ) {
            Column {
                Text(
                    text = server.name,
                    style = MaterialTheme.typography.titleMedium,
                    color = Color.White
                )
                Text(
                    text = "${server.host}:${server.port}",
                    style = MaterialTheme.typography.bodySmall,
                    color = Color.Gray
                )
            }

            Icon(
                imageVector = Icons.AutoMirrored.Filled.ArrowForward,
                contentDescription = "Connect",
                tint = Color(0xFF5BA88C)
            )
        }
    }
}
