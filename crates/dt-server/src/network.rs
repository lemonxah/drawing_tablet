//! UDP networking for the drawing tablet server.

use std::net::SocketAddr;
use tokio::net::UdpSocket;
use thiserror::Error;

/// Network errors.
#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("packet too large: {size} bytes")]
    PacketTooLarge { size: usize },
}

/// UDP server for the drawing tablet protocol.
pub struct UdpServer {
    socket: UdpSocket,
    recv_buf: Vec<u8>,
}

impl UdpServer {
    /// Bind to the specified address.
    pub async fn bind(addr: SocketAddr) -> Result<Self, NetworkError> {
        let socket = UdpSocket::bind(addr).await?;

        // Set socket options for lower latency
        socket.set_broadcast(false)?;

        Ok(Self {
            socket,
            recv_buf: vec![0u8; 65536],
        })
    }

    /// Get the local address.
    pub fn local_addr(&self) -> Result<SocketAddr, NetworkError> {
        Ok(self.socket.local_addr()?)
    }

    /// Receive a packet.
    pub async fn recv(&self) -> Result<(Vec<u8>, SocketAddr), NetworkError> {
        let mut buf = vec![0u8; 65536];
        let (len, addr) = self.socket.recv_from(&mut buf).await?;
        buf.truncate(len);
        Ok((buf, addr))
    }

    /// Send a packet to the specified address.
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
