//! Drawing tablet streaming server.
//!
//! Architecture:
//! - TCP (port): Input and Control packets from client (reliable, ordered)
//! - UDP (port): Video stream to client (low latency, lossy OK)

mod config;
mod gui;
mod network;

use anyhow::Result;
use config::ServerConfig;
use dt_capture::{select_monitor, ScreenCapture};
use dt_encoder::{EncoderConfig, VideoEncoder};
use dt_input::{EventRetimer, VirtualTablet};
use dt_protocol::{
    decode_packet, encode_control, encode_video, ControlPacket, DecodedPacket, VideoPacket,
    MAX_FRAGMENT_DATA, PROTOCOL_VERSION,
};
use gui::{DtServerApp, ServerStats};
use network::{TcpMessage, TcpServer, UdpServer};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{debug, error, info, warn};

/// Keeps an `avahi-publish` child alive for the lifetime of the server.
/// The mDNS advertisement is removed automatically when the child is killed
/// on drop.
struct AvahiService {
    child: Child,
}

impl AvahiService {
    fn start(name: &str, port: u16) -> Option<Self> {
        Command::new("avahi-publish")
            .args(["-s", name, "_drawingtablet._udp", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
            .map(|child| Self { child })
    }
}

impl Drop for AvahiService {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn get_hostname() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "Unknown".to_string())
        .trim()
        .to_string()
}

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("dt_server=info".parse()?)
                .add_directive("dt_capture=info".parse()?)
                .add_directive("dt_encoder=info".parse()?)
                .add_directive("dt_input=info".parse()?),
        )
        .init();

    info!("Drawing Tablet Server v{}", env!("CARGO_PKG_VERSION"));

    let stats = Arc::new(Mutex::new(ServerStats::default()));
    {
        let mut guard = stats.lock().unwrap();
        guard.status = "Stopped".to_string();
    }
    let running_flag = Arc::new(AtomicBool::new(false));

    let app = DtServerApp::new(stats, running_flag);

    let mut native_options = eframe::NativeOptions::default();
    native_options.viewport.inner_size = Some(eframe::egui::vec2(1000.0, 700.0));

    // Load icon
    let icon_data = include_bytes!("../assets/icon.png");
    if let Ok(image) = image::load_from_memory(icon_data) {
        let image = image.into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        native_options.viewport.icon = Some(Arc::new(eframe::egui::IconData {
            rgba,
            width,
            height,
        }));
    }

    eframe::run_native(
        "Drawing Tablet Server",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
    .map_err(|e| anyhow::anyhow!("Eframe error: {}", e))
}

pub fn run_server_pipeline(
    config: ServerConfig,
    stats: Arc<Mutex<ServerStats>>,
    running: Arc<AtomicBool>,
) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async move {
        if let Err(e) = run_server_async(config, stats.clone(), running.clone()).await {
            error!("Server error: {}", e);
            let mut guard = stats.lock().unwrap();
            guard.status = format!("Error: {}", e);
            running.store(false, Ordering::SeqCst);
        }
    });
}

pub async fn run_server_async(
    config: ServerConfig,
    stats: Arc<Mutex<ServerStats>>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    info!("Server configuration: {:?}", config);

    {
        let mut guard = stats.lock().unwrap();
        guard.status = "Selecting Screen...".to_string();
    }

    // Select monitor via portal
    info!("Opening screen selection dialog...");
    let monitor = select_monitor(None).await?;
    info!(
        "Selected monitor: {}x{} (node {})",
        monitor.width, monitor.height, monitor.node_id
    );

    {
        let mut guard = stats.lock().unwrap();
        guard.status = "Initializing...".to_string();
        guard.resolution = format!("{}x{}", monitor.width, monitor.height);
    }

    // Create virtual tablet
    info!("Creating virtual tablet device...");
    let tablet = VirtualTablet::new(monitor.width, monitor.height, &config.tablet)?;
    info!("Virtual tablet created");

    // Create encoder
    let encoder_config = EncoderConfig {
        width: monitor.width,
        height: monitor.height,
        fps: config.fps,
        bitrate: config.bitrate,
        keyframe_interval: config.keyframe_interval,
    };
    let encoder = VideoEncoder::new(encoder_config)?;

    // Start screen capture
    info!("Starting screen capture...");
    let capture = ScreenCapture::start(&monitor)?;

    // Start UDP server (for video streaming to client)
    let udp_bind_addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    info!("Starting UDP server on {} (video)", udp_bind_addr);
    let udp_server = UdpServer::bind(udp_bind_addr).await?;

    // Start TCP server (for input/control from client)
    let tcp_bind_addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    info!("Starting TCP server on {} (input/control)", tcp_bind_addr);
    let tcp_server = TcpServer::bind(tcp_bind_addr).await?;

    // Advertise via mDNS so clients can discover us without a manual IP
    let service_name = format!("Drawing Tablet ({})", get_hostname());
    let _avahi = match AvahiService::start(&service_name, config.port) {
        Some(svc) => {
            info!("Advertising as '{}' via mDNS", service_name);
            Some(svc)
        }
        None => {
            warn!("avahi-publish not available – mDNS discovery disabled");
            None
        }
    };

    {
        let mut guard = stats.lock().unwrap();
        guard.status = "Running".to_string();
    }

    // Run the main server loop
    run_server(
        udp_server,
        tcp_server,
        capture,
        encoder,
        tablet,
        monitor.width,
        monitor.height,
        config.fps,
        stats,
        running,
    )
    .await
}

async fn run_server(
    udp_server: UdpServer,
    tcp_server: TcpServer,
    capture: ScreenCapture,
    encoder: VideoEncoder,
    mut tablet: VirtualTablet,
    width: u32,
    height: u32,
    fps: u32,
    stats: Arc<Mutex<ServerStats>>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let (frame_tx, frame_rx) = mpsc::sync_channel(1);
    let (encoded_tx, encoded_rx) = mpsc::sync_channel(2);
    let encoder_running = Arc::new(AtomicBool::new(true));
    let encoder_running_clone = encoder_running.clone();
    let keyframe_flag = Arc::new(AtomicBool::new(false));
    let keyframe_flag_clone = keyframe_flag.clone();
    let pts_counter = Arc::new(AtomicU64::new(0));
    let pts_counter_clone = pts_counter.clone();

    // Spawn encoder thread
    let encoder_handle = std::thread::spawn(move || {
        let mut encoder = encoder;
        while encoder_running_clone.load(Ordering::SeqCst) {
            let frame = match frame_rx.recv() {
                Ok(frame) => frame,
                Err(_) => break,
            };

            if keyframe_flag_clone.swap(false, Ordering::SeqCst) {
                encoder.request_keyframe();
            }

            match encoder.encode(&frame) {
                Ok(Some(mut encoded)) => {
                    let pts =
                        pts_counter_clone.fetch_add(1_000_000 / (fps as u64), Ordering::SeqCst);
                    encoded.pts = pts;
                    if encoded_tx.send(encoded).is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("Encoding error: {}", e);
                }
            }
        }
    });

    // Channel for TCP messages from clients
    let (tcp_msg_tx, mut tcp_msg_rx) = tokio_mpsc::channel::<TcpMessage>(256);

    // Current connected client state
    struct ClientState {
        tcp_client_id: u64,
        udp_addr: Option<SocketAddr>, // Set when client sends UDP port info
    }
    let mut client: Option<ClientState> = None;
    let mut sequence: u32 = 0;
    let mut last_heartbeat = Instant::now();
    let mut keyframe_requested = false;

    // Event retimer for smooth input delivery
    let mut event_retimer = EventRetimer::new();

    // Per-second stats
    let mut stats_captured: u64 = 0;
    let mut stats_encoded: u64 = 0;
    let mut stats_sent: u64 = 0;
    let mut stats_keyframes: u64 = 0;
    let mut stats_fragments: u64 = 0;
    let mut stats_dropped_frames: u64 = 0;
    let mut stats_encode_errors: u64 = 0;
    let mut last_stats = Instant::now();

    info!("Waiting for client connection (TCP for input, UDP for video)...");

    while running.load(Ordering::SeqCst) {
        tokio::select! {
            // Accept new TCP connections
            result = tcp_server.accept(tcp_msg_tx.clone()) => {
                match result {
                    Ok((client_id, addr)) => {
                        // If there's already a client, disconnect them
                        if let Some(old_client) = client.take() {
                            info!("Disconnecting previous client {}", old_client.tcp_client_id);
                            tcp_server.disconnect_client(old_client.tcp_client_id).await;
                        }

                        // Reset retimer for new client
                        event_retimer.reset();

                        // For now, use the TCP address as the UDP address too
                        // (client will be on the same IP, just different port for UDP)
                        let udp_addr = SocketAddr::new(addr.ip(), addr.port());

                        client = Some(ClientState {
                            tcp_client_id: client_id,
                            udp_addr: Some(udp_addr),
                        });

                        info!("New client {} connected from {}", client_id, addr);
                    }
                    Err(e) => {
                        warn!("Failed to accept TCP connection: {}", e);
                    }
                }
            }

            // Process TCP messages from clients
            Some(msg) = tcp_msg_rx.recv() => {
                // Only process messages from the current client
                if let Some(ref mut c) = client {
                    if msg.client_id == c.tcp_client_id {
                        match decode_packet(&msg.data) {
                            Ok(DecodedPacket::Control(ctrl)) => {
                                match &ctrl {
                                    ControlPacket::RequestKeyframe => {
                                        keyframe_requested = true;
                                    }
                                    ControlPacket::Connect { version } => {
                                        info!("Client protocol version: {}", version);
                                        if *version != PROTOCOL_VERSION {
                                            let nack = encode_control(&ControlPacket::ConnectNack {
                                                reason: format!(
                                                    "Protocol version mismatch: expected {}, got {}",
                                                    PROTOCOL_VERSION, version
                                                ),
                                            })?;
                                            let _ = tcp_server.send_to_client(c.tcp_client_id, &nack).await;
                                            tcp_server.disconnect_client(c.tcp_client_id).await;
                                            client = None;
                                            continue;
                                        }
                                        // Send ConnectAck
                                        let ack = encode_control(&ControlPacket::ConnectAck { width, height })?;
                                        let _ = tcp_server.send_to_client(c.tcp_client_id, &ack).await;
                                        info!("Client handshake complete");
                                    }
                                    ControlPacket::Disconnect => {
                                        info!("Client {} requested disconnect", c.tcp_client_id);
                                        tcp_server.disconnect_client(c.tcp_client_id).await;
                                        event_retimer.reset();
                                        client = None;
                                    }
                                    ControlPacket::Heartbeat => {
                                        debug!("Heartbeat from client {}", c.tcp_client_id);
                                    }
                                    ControlPacket::SetUdpPort { port } => {
                                        // Client tells us which UDP port to send video to
                                        if let Some(ref mut state) = client {
                                            if let Some(ref mut udp) = state.udp_addr {
                                                udp.set_port(*port);
                                                info!("Client UDP port set to {}", port);
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Ok(DecodedPacket::Input(input)) => {
                                // Update stats for buttons
                                if let dt_protocol::InputEvent::StylusButton { button, pressed } = input.event {
                                    if button < 2 {
                                        let mut guard = stats.lock().unwrap();
                                        guard.stylus_buttons[button as usize] = pressed;
                                    }
                                }

                                // Push to retimer for smooth delivery
                                event_retimer.push(input);
                            }
                            Ok(DecodedPacket::Video(_)) => {
                                // Ignore video packets from clients
                            }
                            Err(e) => {
                                debug!("Failed to decode TCP packet: {}", e);
                            }
                        }
                    }
                }
            }

            // Periodic tasks (encoding, sending video, stats)
            _ = tokio::time::sleep(Duration::from_micros(100)) => {
                // Process retimed input events
                for packet in event_retimer.drain_ready() {
                    if let Err(e) = tablet.process_event(&packet.event) {
                        warn!("Failed to process input: {}", e);
                    }
                }

                // Check if client is still connected
                if let Some(ref c) = client {
                    if !tcp_server.is_client_connected(c.tcp_client_id).await {
                        info!("Client {} disconnected", c.tcp_client_id);
                        event_retimer.reset();
                        client = None;
                    }
                }
            }
        }

        // Update connected client in GUI stats
        if sequence.is_multiple_of(60) {
            let mut guard = stats.lock().unwrap();
            guard.connected_client = client
                .as_ref()
                .and_then(|c| c.udp_addr.map(|a| a.to_string()));
        }

        // Check for keyframe requests
        if keyframe_requested {
            keyframe_flag.store(true, Ordering::SeqCst);
            keyframe_requested = false;
        }

        // 1. Send captured frames to encoder
        if client.is_some() {
            while let Some(frame) = capture.try_recv_frame() {
                stats_captured += 1;
                match frame_tx.try_send(frame) {
                    Ok(_) => {}
                    Err(mpsc::TrySendError::Full(_)) => {
                        stats_dropped_frames += 1;
                    }
                    Err(_) => {
                        running.store(false, Ordering::SeqCst);
                    }
                }
            }
        }

        // 2. Process encoded packets from encoder and send via UDP
        while let Ok(encoded) = encoded_rx.try_recv() {
            stats_encoded += 1;
            if encoded.is_keyframe {
                stats_keyframes += 1;
            }

            if let Some(ref c) = client {
                if let Some(addr) = c.udp_addr {
                    let data = &encoded.data;
                    let fragment_count = data.len().div_ceil(MAX_FRAGMENT_DATA) as u16;

                    let mut fragment_packets = Vec::with_capacity(fragment_count as usize);
                    let mut all_sent = true;

                    for i in 0..fragment_count {
                        let start = i as usize * MAX_FRAGMENT_DATA;
                        let end = (start + MAX_FRAGMENT_DATA).min(data.len());

                        let packet = VideoPacket {
                            sequence,
                            timestamp: encoded.pts,
                            is_keyframe: encoded.is_keyframe,
                            fragment_index: i,
                            fragment_count,
                            data: data[start..end].to_vec(),
                        };

                        match encode_video(&packet) {
                            Ok(wire) => {
                                fragment_packets.push(wire);
                            }
                            Err(e) => {
                                warn!("Failed to encode fragment: {}", e);
                                all_sent = false;
                                break;
                            }
                        }
                    }

                    if all_sent && !fragment_packets.is_empty() {
                        const BATCH_SIZE: usize = 8;
                        const PACING_DELAY: Duration = Duration::from_micros(100);

                        let packet_refs: Vec<&[u8]> =
                            fragment_packets.iter().map(|p| p.as_slice()).collect();

                        for chunk in packet_refs.chunks(BATCH_SIZE) {
                            if let Err(e) = udp_server.send_batch(chunk, addr).await {
                                warn!("Failed to send video fragments: {}", e);
                                all_sent = false;
                                break;
                            }
                            tokio::time::sleep(PACING_DELAY).await;
                        }
                    }
                    if all_sent {
                        stats_sent += 1;
                        stats_fragments += fragment_count as u64;
                    }
                }
            }
            sequence = sequence.wrapping_add(1);
        }

        // Send heartbeat every second via TCP
        if client.is_some() && last_heartbeat.elapsed() > Duration::from_secs(1) {
            if let Some(ref c) = client {
                let heartbeat = encode_control(&ControlPacket::Heartbeat)?;
                let _ = tcp_server.send_to_client(c.tcp_client_id, &heartbeat).await;
            }
            last_heartbeat = Instant::now();
        }

        // Update stats once per second
        if last_stats.elapsed() > Duration::from_secs(1) {
            {
                let mut guard = stats.lock().unwrap();
                guard.captured = stats_captured;
                guard.encoded = stats_encoded;
                guard.dropped = stats_dropped_frames;
                guard.sent = stats_sent;
                guard.keyframes = stats_keyframes;
                guard.fragments = stats_fragments;
                guard.errors = stats_encode_errors;
            }

            stats_captured = 0;
            stats_dropped_frames = 0;
            stats_encoded = 0;
            stats_sent = 0;
            stats_keyframes = 0;
            stats_fragments = 0;
            stats_encode_errors = 0;
            last_stats = Instant::now();
        }
    }

    info!("Server stopped");

    // Close channel to stop encoder thread
    drop(frame_tx);
    encoder_running.store(false, Ordering::SeqCst);

    if let Err(e) = encoder_handle.join() {
        warn!("Failed to join encoder thread: {:?}", e);
    }

    Ok(())
}
