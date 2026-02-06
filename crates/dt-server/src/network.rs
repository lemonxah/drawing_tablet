//! Networking for the drawing tablet server.
//!
//! Uses a split architecture:
//! - TCP for reliable input and control packets (from client)
//! - UDP for low-latency video streaming (to client)

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use thiserror::Error;
use tracing::{info, warn};

/// Network errors.
#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[allow(dead_code)]
    #[error("packet too large: {size} bytes")]
    PacketTooLarge { size: usize },

    #[error("connection closed")]
    ConnectionClosed,

    #[error("channel closed")]
    ChannelClosed,
}

/// UDP server for video streaming (server -> client).
pub struct UdpServer {
    socket: UdpSocket,
}

impl UdpServer {
    /// Bind to the specified address.
    pub async fn bind(addr: SocketAddr) -> Result<Self, NetworkError> {
        let socket = UdpSocket::bind(addr).await?;
        socket.set_broadcast(false)?;

        Ok(Self { socket })
    }

    /// Get the local address.
    #[allow(dead_code)]
    pub fn local_addr(&self) -> Result<SocketAddr, NetworkError> {
        Ok(self.socket.local_addr()?)
    }

    /// Send a packet to the specified address.
    #[allow(dead_code)]
    pub async fn send(&self, data: &[u8], addr: SocketAddr) -> Result<(), NetworkError> {
        self.socket.send_to(data, addr).await?;
        Ok(())
    }

    /// Send multiple packets to the same address efficiently.
    pub async fn send_batch(&self, packets: &[&[u8]], addr: SocketAddr) -> Result<(), NetworkError> {
        for packet in packets {
            self.socket.send_to(packet, addr).await?;
        }
        Ok(())
    }
}

/// A message received from a TCP client.
#[derive(Debug)]
pub struct TcpMessage {
    pub data: Vec<u8>,
    pub client_id: u64,
}

/// TCP server for input and control packets (client -> server).
pub struct TcpServer {
    listener: TcpListener,
    /// Sender for outgoing messages to clients.
    /// Maps client_id -> sender channel.
    client_senders: Arc<tokio::sync::RwLock<std::collections::HashMap<u64, mpsc::Sender<Vec<u8>>>>>,
    next_client_id: std::sync::atomic::AtomicU64,
}

impl TcpServer {
    /// Bind to the specified address.
    pub async fn bind(addr: SocketAddr) -> Result<Self, NetworkError> {
        let listener = TcpListener::bind(addr).await?;

        // Enable TCP_NODELAY on the listener for lower latency
        // (actual client sockets will also need this)

        Ok(Self {
            listener,
            client_senders: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            next_client_id: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// Get the local address.
    #[allow(dead_code)]
    pub fn local_addr(&self) -> Result<SocketAddr, NetworkError> {
        Ok(self.listener.local_addr()?)
    }

    /// Accept a new client connection and spawn a task to handle it.
    /// Returns the client_id and their remote address, plus a receiver for their messages.
    pub async fn accept(
        &self,
        msg_tx: mpsc::Sender<TcpMessage>,
    ) -> Result<(u64, SocketAddr), NetworkError> {
        let (stream, addr) = self.listener.accept().await?;

        // Enable TCP_NODELAY for lower latency
        stream.set_nodelay(true)?;

        let client_id = self
            .next_client_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Create a channel for sending messages to this client
        let (client_tx, client_rx) = mpsc::channel::<Vec<u8>>(64);

        // Store the sender
        {
            let mut senders = self.client_senders.write().await;
            senders.insert(client_id, client_tx);
        }

        // Spawn task to handle this client
        let client_senders = self.client_senders.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_tcp_client(stream, client_id, msg_tx, client_rx).await {
                match e {
                    NetworkError::ConnectionClosed => {
                        info!("TCP client {} disconnected", client_id);
                    }
                    _ => {
                        warn!("TCP client {} error: {}", client_id, e);
                    }
                }
            }

            // Remove the sender when the client disconnects
            let mut senders = client_senders.write().await;
            senders.remove(&client_id);
        });

        info!("TCP client {} connected from {}", client_id, addr);
        Ok((client_id, addr))
    }

    /// Send a message to a specific client.
    pub async fn send_to_client(&self, client_id: u64, data: &[u8]) -> Result<(), NetworkError> {
        let senders = self.client_senders.read().await;
        if let Some(sender) = senders.get(&client_id) {
            sender
                .send(data.to_vec())
                .await
                .map_err(|_| NetworkError::ChannelClosed)?;
            Ok(())
        } else {
            Err(NetworkError::ConnectionClosed)
        }
    }

    /// Check if a client is still connected.
    pub async fn is_client_connected(&self, client_id: u64) -> bool {
        let senders = self.client_senders.read().await;
        senders.contains_key(&client_id)
    }

    /// Disconnect a client.
    pub async fn disconnect_client(&self, client_id: u64) {
        let mut senders = self.client_senders.write().await;
        senders.remove(&client_id);
    }
}

/// Handle a single TCP client connection.
async fn handle_tcp_client(
    stream: TcpStream,
    client_id: u64,
    msg_tx: mpsc::Sender<TcpMessage>,
    mut outgoing_rx: mpsc::Receiver<Vec<u8>>,
) -> Result<(), NetworkError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Buffer for reading the packet header
    let mut header_buf = [0u8; 3];

    loop {
        tokio::select! {
            // Read incoming packets from client
            result = reader.read_exact(&mut header_buf) => {
                match result {
                    Ok(_) => {
                        // Parse header: [type: u8][length: u16 LE]
                        let len = u16::from_le_bytes([header_buf[1], header_buf[2]]) as usize;

                        // Read the payload
                        let mut payload = vec![0u8; len];
                        reader.read_exact(&mut payload).await?;

                        // Reconstruct the full packet (header + payload)
                        let mut full_packet = Vec::with_capacity(3 + len);
                        full_packet.extend_from_slice(&header_buf);
                        full_packet.extend_from_slice(&payload);

                        // Send to the message handler
                        if msg_tx.send(TcpMessage {
                            data: full_packet,
                            client_id,
                        }).await.is_err() {
                            return Err(NetworkError::ChannelClosed);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        return Err(NetworkError::ConnectionClosed);
                    }
                    Err(e) => {
                        return Err(NetworkError::Io(e));
                    }
                }
            }

            // Send outgoing packets to client
            Some(data) = outgoing_rx.recv() => {
                writer.write_all(&data).await?;
                writer.flush().await?;
            }

            // Channel closed - client handler is shutting down
            else => {
                return Ok(());
            }
        }
    }
}
