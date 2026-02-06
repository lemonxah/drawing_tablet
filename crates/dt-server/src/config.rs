//! Server configuration.

use dt_input::TabletDescriptor;
use std::env;

/// Server configuration options.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// UDP port to listen on.
    pub port: u16,
    /// Target frame rate.
    pub fps: u32,
    /// Target bitrate in kbps.
    pub bitrate: u32,
    /// Keyframe interval in frames.
    pub keyframe_interval: u32,
    /// Manually selected output screen for touch mapping (optional).
    pub output_name: Option<String>,
    /// Selected tablet descriptor for emulation.
    pub tablet: TabletDescriptor,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 9999,
            fps: 60,
            bitrate: 8000,
            keyframe_interval: 10,
            output_name: None,
            tablet: TabletDescriptor::default(),
        }
    }
}

impl ServerConfig {
    /// Parse configuration from command line arguments and environment.
    pub fn from_args() -> Self {
        let mut config = Self::default();
        let args: Vec<String> = env::args().collect();

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "-p" | "--port" => {
                    if i + 1 < args.len() {
                        config.port = args[i + 1].parse().unwrap_or(config.port);
                        i += 1;
                    }
                }
                "-f" | "--fps" => {
                    if i + 1 < args.len() {
                        config.fps = args[i + 1].parse().unwrap_or(config.fps);
                        i += 1;
                    }
                }
                "-b" | "--bitrate" => {
                    if i + 1 < args.len() {
                        config.bitrate = args[i + 1].parse().unwrap_or(config.bitrate);
                        i += 1;
                    }
                }
                "-k" | "--keyframe-interval" => {
                    if i + 1 < args.len() {
                        config.keyframe_interval =
                            args[i + 1].parse().unwrap_or(config.keyframe_interval);
                        i += 1;
                    }
                }
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                _ => {}
            }
            i += 1;
        }

        // Environment variable overrides
        if let Ok(port) = env::var("DT_PORT") {
            config.port = port.parse().unwrap_or(config.port);
        }
        if let Ok(fps) = env::var("DT_FPS") {
            config.fps = fps.parse().unwrap_or(config.fps);
        }
        if let Ok(bitrate) = env::var("DT_BITRATE") {
            config.bitrate = bitrate.parse().unwrap_or(config.bitrate);
        }

        config
    }
}

fn print_help() {
    println!("Drawing Tablet Server");
    println!();
    println!("USAGE:");
    println!("    dt-server [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    -p, --port <PORT>              UDP port to listen on [default: 9999]");
    println!("    -f, --fps <FPS>                Target frame rate [default: 100]");
    println!("    -b, --bitrate <KBPS>           Target bitrate in kbps [default: 8000]");
    println!("    -k, --keyframe-interval <N>    Keyframe interval in frames [default: 10]");
    println!("    -h, --help                     Print help information");
    println!();
    println!("ENVIRONMENT VARIABLES:");
    println!("    DT_PORT      Override the UDP port");
    println!("    DT_FPS       Override the frame rate");
    println!("    DT_BITRATE   Override the bitrate");
}
