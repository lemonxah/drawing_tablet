//! Drawing tablet streaming server.

mod config;
mod gui;
mod network;

use anyhow::Result;
use config::ServerConfig;
use dt_capture::{select_monitor, ScreenCapture};
use dt_encoder::{EncoderConfig, VideoEncoder};
use dt_input::VirtualTablet;
use dt_protocol::{
    decode_packet, encode_control, encode_video, ControlPacket, DecodedPacket, VideoPacket,
    MAX_FRAGMENT_DATA, PROTOCOL_VERSION,
};
use gui::{DtServerApp, ServerStats};
use network::UdpServer;
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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

    let native_options = eframe::NativeOptions::default();
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
    let tablet = VirtualTablet::new(monitor.width, monitor.height)?;
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

    // Start UDP server
    let bind_addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    info!("Starting UDP server on {}", bind_addr);
    let server = UdpServer::bind(bind_addr).await?;

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
        server,
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
    server: UdpServer,
    capture: ScreenCapture,
    encoder: VideoEncoder,
    mut tablet: VirtualTablet,
    width: u32,
    height: u32,
    fps: u32,
    stats: Arc<Mutex<ServerStats>>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    // Note: We use the passed-in 'running' flag instead of creating a new one

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
                    let pts = pts_counter_clone.fetch_add(1_000_000 / (fps as u64), Ordering::SeqCst);
                    encoded.pts = pts;
                    if let Err(_) = encoded_tx.send(encoded) {
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

    // Current connected client
    let mut client_addr: Option<SocketAddr> = None;
    let mut sequence: u32 = 0;
    let mut last_heartbeat = Instant::now();
    let mut keyframe_requested = false;

    // Per-second stats
    let mut stats_captured: u64 = 0;
    let mut stats_encoded: u64 = 0;
    let mut stats_sent: u64 = 0;
    let mut stats_keyframes: u64 = 0;
    let mut stats_fragments: u64 = 0;
    let mut stats_dropped_frames: u64 = 0;
    let mut stats_encode_errors: u64 = 0;
    let mut last_stats = Instant::now();

    info!("Waiting for client connection...");

    while running.load(Ordering::SeqCst) {
        // Check for incoming packets
        match tokio::time::timeout(Duration::from_millis(5), server.recv()).await {
            Ok(Ok((data, addr))) => {
                match decode_packet(&data) {
                    Ok(DecodedPacket::Control(ctrl)) => {
                        if let ControlPacket::RequestKeyframe = &ctrl {
                            if Some(addr) == client_addr {
                                keyframe_requested = true;
                            }
                        }
                        handle_control_packet(&server, ctrl, addr, &mut client_addr, width, height)
                            .await?;
                    }
                    Ok(DecodedPacket::Input(input)) => {
                        if Some(addr) == client_addr {
                            // Debug logging for input events
                            // info!("Received input event: {:?}", input.event);

                            // Update stats for buttons
                            if let dt_protocol::InputEvent::StylusButton { button, pressed } = input.event {
                                if button < 2 {
                                    let mut guard = stats.lock().unwrap();
                                    guard.stylus_buttons[button as usize] = pressed;
                                }
                            }

                            // Process input on the tablet
                            if let Err(e) = tablet.process_event(&input.event) {
                                warn!("Failed to process input: {}", e);
                            }
                        }
                    }
                    Ok(DecodedPacket::Video(_)) => {
                        // Ignore video packets from clients
                    }
                    Err(e) => {
                        debug!("Failed to decode packet from {}: {}", addr, e);
                    }
                }
            }
            Ok(Err(e)) => {
                error!("Network error: {}", e);
            }
            Err(_) => {
                // Timeout - continue processing
            }
        }

        // Update connected client in GUI stats
        {
             // Only update occasionally to avoid lock contention
             if sequence % 60 == 0 {
                 let mut guard = stats.lock().unwrap();
                 guard.connected_client = client_addr.map(|a| a.to_string());
             }
        }

        // Check for keyframe requests
        if keyframe_requested {
            keyframe_flag.store(true, Ordering::SeqCst);
            keyframe_requested = false;
        }

        // 1. Send captured frames to encoder
        if client_addr.is_some() {
            while let Some(frame) = capture.try_recv_frame() {
                stats_captured += 1;
                match frame_tx.try_send(frame) {
                    Ok(_) => {
                        // Sent successfully
                    }
                    Err(mpsc::TrySendError::Full(_)) => {
                        // Encoder busy.
                        stats_dropped_frames += 1;
                    }
                    Err(_) => {
                        running.store(false, Ordering::SeqCst);
                    }
                }
            }
        }

        // 2. Process encoded packets from encoder
        while let Ok(encoded) = encoded_rx.try_recv() {
            stats_encoded += 1;
            if encoded.is_keyframe {
                stats_keyframes += 1;
            }

            if let Some(addr) = client_addr {
                let data = &encoded.data;
                let fragment_count =
                    ((data.len() + MAX_FRAGMENT_DATA - 1) / MAX_FRAGMENT_DATA) as u16;

                // Encode all fragments first, then send in batch
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
                    // Send fragments with pacing to avoid network congestion
                    const BATCH_SIZE: usize = 8;
                    const PACING_DELAY: Duration = Duration::from_micros(100);

                    let packet_refs: Vec<&[u8]> =
                        fragment_packets.iter().map(|p| p.as_slice()).collect();

                    for chunk in packet_refs.chunks(BATCH_SIZE) {
                        if let Err(e) = server.send_batch(chunk, addr).await {
                            warn!("Failed to send batch fragments: {}", e);
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
            sequence = sequence.wrapping_add(1);
        }

        // Send heartbeat every second
        if client_addr.is_some() && last_heartbeat.elapsed() > Duration::from_secs(1) {
            let heartbeat = encode_control(&ControlPacket::Heartbeat)?;
            let _ = server.send(&heartbeat, client_addr.unwrap()).await;
            last_heartbeat = Instant::now();
        }

        // Print stats once per second
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
            
            if client_addr.is_some() {
                info!(
                    "[stats] captured={} dropped={} encoded={} sent={} keyframes={} fragments={} errors={}",
                    stats_captured,
                    stats_dropped_frames,
                    stats_encoded,
                    stats_sent,
                    stats_keyframes,
                    stats_fragments,
                    stats_encode_errors
                );
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

        tokio::time::sleep(Duration::from_micros(100)).await;
    }

    info!("Server stopped");

    // Close channel to stop encoder thread
    drop(frame_tx);
    // Explicitly stop the encoder thread loop
    encoder_running.store(false, Ordering::SeqCst);

    // Wait for the encoder thread to finish
    if let Err(e) = encoder_handle.join() {
        warn!("Failed to join encoder thread: {:?}", e);
    }

    Ok(())
}

async fn handle_control_packet(
    server: &UdpServer,
    packet: ControlPacket,
    addr: SocketAddr,
    client_addr: &mut Option<SocketAddr>,
    width: u32,
    height: u32,
) -> Result<()> {
    match packet {
        ControlPacket::Connect { version } => {
            info!("Connection request from {} (protocol v{})", addr, version);

            if version != PROTOCOL_VERSION {
                let nack = encode_control(&ControlPacket::ConnectNack {
                    reason: format!(
                        "Protocol version mismatch: expected {}, got {}",
                        PROTOCOL_VERSION, version
                    ),
                })?;
                server.send(&nack, addr).await?;
                return Ok(());
            }

            // Accept connection
            *client_addr = Some(addr);
            let ack = encode_control(&ControlPacket::ConnectAck { width, height })?;
            server.send(&ack, addr).await?;
            info!("Client {} connected", addr);
        }
        ControlPacket::Disconnect => {
            if Some(addr) == *client_addr {
                info!("Client {} disconnected", addr);
                *client_addr = None;
            }
        }
        ControlPacket::Heartbeat => {
            // Client is alive
            debug!("Heartbeat from {}", addr);
        }
        ControlPacket::RequestKeyframe => {
            if Some(addr) == *client_addr {
                debug!("Keyframe requested by {}", addr);
                // Will be handled by the main loop
            }
        }
        _ => {}
    }

    Ok(())
}
