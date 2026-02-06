use crate::config::ServerConfig;
use eframe::egui;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

#[derive(Clone, Default)]
pub struct ServerStats {
    pub captured: u64,
    pub encoded: u64,
    pub sent: u64,
    pub dropped: u64,
    pub keyframes: u64,
    pub fragments: u64,
    pub errors: u64,
    pub connected_client: Option<String>,
    pub resolution: String,
    pub status: String,
    pub stylus_buttons: [bool; 2],
    pub logs: VecDeque<String>,
}

impl ServerStats {
    pub fn log(&mut self, msg: String) {
        self.logs.push_back(format!(
            "[{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            msg
        ));
        if self.logs.len() > 50 {
            self.logs.pop_front();
        }
    }
}

pub struct DtServerApp {
    // Shared state
    stats: Arc<Mutex<ServerStats>>,
    running_flag: Arc<AtomicBool>,

    // Config state (editable when stopped)
    port_input: String,
    fps_input: String,
    bitrate_input: String,
    keyframe_input: String,

    // Help Dialog
    show_help_dialog: bool,

    // Runtime and Task
    rt: Runtime,
    server_task: Option<JoinHandle<()>>,
}

impl DtServerApp {
    pub fn new(stats: Arc<Mutex<ServerStats>>, running_flag: Arc<AtomicBool>) -> Self {
        // Create a persistent Tokio runtime
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");

        // Apply custom style once
        Self {
            stats,
            running_flag,
            port_input: "9999".to_string(),
            fps_input: "60".to_string(),
            bitrate_input: "8000".to_string(),
            keyframe_input: "10".to_string(),
            show_help_dialog: false,
            rt,
            server_task: None,
        }
    }

    fn start_server(&mut self) {
        if self.running_flag.load(Ordering::SeqCst) || self.server_task.is_some() {
            return;
        }

        // Parse config
        let port = self.port_input.trim().parse::<u16>().unwrap_or(9999);
        let fps = self.fps_input.trim().parse::<u32>().unwrap_or(100).max(1);
        let bitrate = self.bitrate_input.trim().parse::<u32>().unwrap_or(50000).max(1000);
        let keyframe_interval = self.keyframe_input.trim().parse::<u32>().unwrap_or(2).max(1);

        let config = ServerConfig {
            port,
            fps,
            bitrate,
            keyframe_interval,
            output_name: None, // Auto-mapping removed
        };

        let stats = self.stats.clone();
        let running = self.running_flag.clone();
        running.store(true, Ordering::SeqCst);

        // Reset stats
        {
            let mut guard = stats.lock().unwrap();
            let logs = guard.logs.clone(); // Persist logs across restarts
            *guard = ServerStats::default();
            guard.logs = logs;
            guard.status = "Starting...".to_string();
            guard.log("Starting server thread...".to_string());
        }

        // Spawn server task on the existing runtime
        let task = self.rt.spawn(async move {
            // We call run_server_async directly
            if let Err(e) = crate::run_server_async(config, stats.clone(), running.clone()).await {
                let mut guard = stats.lock().unwrap();
                guard.status = format!("Error: {}", e);
                guard.log(format!("Error: {}", e));
            }

            // When pipeline finishes, ensure running is false
            running.store(false, Ordering::SeqCst);
            let mut guard = stats.lock().unwrap();
            guard.status = "Stopped".to_string();
            guard.connected_client = None;
            guard.log("Server stopped.".to_string());
        });

        self.server_task = Some(task);
    }

    fn stop_server(&mut self) {
        {
            let mut guard = self.stats.lock().unwrap();
            guard.log("Stopping server...".to_string());
        }
        self.running_flag.store(false, Ordering::SeqCst);
    }
}

impl eframe::App for DtServerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // --- Theme Configuration ---
        let mut visuals = egui::Visuals::dark();
        visuals.window_fill = egui::Color32::from_rgb(11, 14, 20); // Deep background
        visuals.panel_fill = egui::Color32::from_rgb(11, 14, 20);

        // Widget styles
        visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(27, 30, 40); // Card background
        visuals.widgets.noninteractive.fg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_gray(180));

        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(35, 38, 50); // Input fields
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(45, 48, 60);
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(55, 58, 70);

        visuals.selection.bg_fill = egui::Color32::from_rgb(58, 118, 240); // Accent Blue
        ctx.set_visuals(visuals);

        let is_running = self.running_flag.load(Ordering::SeqCst);

        // Clean up finished task
        if let Some(task) = &self.server_task {
            if task.is_finished() {
                // Task is done. We can drop the handle.
                self.server_task = None;

                // Ensure flag is false just in case
                self.running_flag.store(false, Ordering::SeqCst);
            }
        }

        let stats = {
            let guard = self.stats.lock().unwrap();
            guard.clone()
        };

        // --- Sidebar (Controls & Status) ---
        egui::SidePanel::left("sidebar_panel")
            .resizable(false)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.heading(
                        egui::RichText::new("Drawing Tablet")
                            .size(24.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                    ui.label(
                        egui::RichText::new("Server Control Center")
                            .size(12.0)
                            .color(egui::Color32::from_gray(140)),
                    );
                });

                ui.add_space(30.0);

                // Status Indicator
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        let (status_text, status_color) = if is_running {
                            ("ACTIVE", egui::Color32::from_rgb(100, 255, 100))
                        } else if self.server_task.is_some() {
                            ("STOPPING", egui::Color32::from_rgb(255, 200, 100))
                        } else {
                            ("OFFLINE", egui::Color32::from_rgb(255, 100, 100))
                        };

                        ui.label(
                            egui::RichText::new(status_text)
                                .size(28.0)
                                .strong()
                                .color(status_color),
                        );

                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new(&stats.status).color(egui::Color32::from_gray(180)),
                        );
                        ui.add_space(10.0);
                    });
                });

                ui.add_space(20.0);

                // Start/Stop Button
                let (btn_text, btn_color) = if is_running {
                    ("STOP SERVER", egui::Color32::from_rgb(180, 40, 40))
                } else if self.server_task.is_some() {
                    ("STOPPING...", egui::Color32::from_rgb(80, 80, 80))
                } else {
                    ("START SERVER", egui::Color32::from_rgb(58, 118, 240))
                };

                if ui
                    .add_sized(
                        egui::vec2(ui.available_width(), 50.0),
                        egui::Button::new(
                            egui::RichText::new(btn_text)
                                .size(16.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(btn_color)
                        .rounding(8.0),
                    )
                    .clicked()
                {
                    if is_running {
                        self.stop_server();
                    } else if self.server_task.is_none() {
                        self.start_server();
                    }
                }

                ui.add_space(30.0);

                // Configuration (Sidebar)
                ui.label(
                    egui::RichText::new("CONFIGURATION")
                        .size(12.0)
                        .strong()
                        .color(egui::Color32::from_gray(120)),
                );
                ui.add_space(5.0);

                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(20, 22, 30))
                    .rounding(8.0)
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!is_running, |ui| {
                            let grid = egui::Grid::new("config_grid")
                                .num_columns(2)
                                .spacing([10.0, 15.0])
                                .striped(false);

                            grid.show(ui, |ui| {
                                ui.label("Port");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.port_input)
                                        .desired_width(100.0),
                                );
                                ui.end_row();

                                ui.label("FPS Limit");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.fps_input)
                                        .desired_width(100.0),
                                );
                                ui.end_row();

                                ui.label("Bitrate (kbps)");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.bitrate_input)
                                        .desired_width(100.0),
                                );
                                ui.end_row();

                                ui.label("Keyframe Int");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.keyframe_input)
                                        .desired_width(100.0),
                                );
                                ui.end_row();
                            });
                        });
                    });
                
                ui.add_space(20.0);
                
                // Help Button
                 if ui
                    .add_sized(
                        egui::vec2(ui.available_width(), 30.0),
                        egui::Button::new(
                            egui::RichText::new("❓ Touch Mapping Help")
                                .size(14.0)
                                .color(egui::Color32::from_gray(200)),
                        )
                        .fill(egui::Color32::from_rgb(35, 38, 50))
                        .rounding(6.0),
                    )
                    .clicked()
                {
                    self.show_help_dialog = true;
                }

                ui.add_space(20.0);

                // Footer info
                if is_running {
                    if let Some(client) = &stats.connected_client {
                        ui.label(
                            egui::RichText::new(format!("Connected: {}", client))
                                .size(11.0)
                                .color(egui::Color32::GREEN),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("Waiting for client...")
                                .size(11.0)
                                .color(egui::Color32::YELLOW),
                        );
                    }
                    ui.label(
                        egui::RichText::new(format!("Source: {}", stats.resolution))
                            .size(11.0)
                            .color(egui::Color32::from_gray(150)),
                    );
                }
            });

        // --- Main Content (Stats & Logs) ---
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(10.0);

            // Statistics Cards
            ui.label(
                egui::RichText::new("PERFORMANCE METRICS")
                    .size(12.0)
                    .strong()
                    .color(egui::Color32::from_gray(120)),
            );
            ui.add_space(5.0);

            let card_width = (ui.available_width() - 20.0) / 3.0;

            ui.horizontal(|ui| {
                // FPS Card
                self.stat_card(ui, "Encoded FPS", &stats.encoded.to_string(), card_width);
                // Bandwidth/Data Card
                self.stat_card(ui, "Packets Sent", &stats.sent.to_string(), card_width);
                // Drops/Errors Card
                self.stat_card(ui, "Dropped Frames", &stats.dropped.to_string(), card_width);
            });

            ui.add_space(20.0);

            // Stylus Button Status
            ui.label(
                egui::RichText::new("STYLUS STATUS")
                    .size(12.0)
                    .strong()
                    .color(egui::Color32::from_gray(120)),
            );
            ui.add_space(5.0);

            ui.horizontal(|ui| {
                let btn_width = (ui.available_width() - 10.0) / 2.0;
                
                // Button 0
                let color0 = if stats.stylus_buttons[0] { egui::Color32::GREEN } else { egui::Color32::from_gray(50) };
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(27, 30, 40))
                    .rounding(6.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_width(btn_width);
                        ui.horizontal(|ui| {
                            ui.label("Button 1 (Side)");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.painter().circle_filled(ui.available_rect_before_wrap().center(), 8.0, color0);
                            });
                        });
                    });

                // Button 1
                let color1 = if stats.stylus_buttons[1] { egui::Color32::GREEN } else { egui::Color32::from_gray(50) };
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(27, 30, 40))
                    .rounding(6.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_width(btn_width);
                        ui.horizontal(|ui| {
                            ui.label("Button 2 (Eraser)");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.painter().circle_filled(ui.available_rect_before_wrap().center(), 8.0, color1);
                            });
                        });
                    });
            });

            ui.add_space(20.0);

            // Logs
            ui.label(
                egui::RichText::new("SYSTEM LOGS")
                    .size(12.0)
                    .strong()
                    .color(egui::Color32::from_gray(120)),
            );
            ui.add_space(5.0);

            egui::Frame::none()
                .fill(egui::Color32::from_rgb(15, 17, 22))
                .rounding(6.0)
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 32, 40)))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_min_height(ui.available_height());
                            for log in &stats.logs {
                                ui.label(
                                    egui::RichText::new(log)
                                        .family(egui::FontFamily::Monospace)
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(180)),
                                );
                            }
                        });
                });
        });

        // Periodic repaint
        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        // Help Window
        if self.show_help_dialog {
            let mut open = true;
            egui::Window::new("Touchscreen Mapping Help")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_max_width(400.0);
                    ui.heading("Touch Input on Wrong Screen?");
                    ui.add_space(10.0);
                    ui.label("If your touch input appears on the wrong monitor, you need to map the virtual tablet to the correct display in your OS settings.");
                    ui.add_space(10.0);
                    
                    ui.label(egui::RichText::new("KDE Plasma (Linux)").strong().color(egui::Color32::WHITE));
                    ui.label("1. Open System Settings.");
                    ui.label("2. Go to Input Devices > Touchscreen.");
                    ui.label("3. Select 'Drawing Tablet Touchscreen'.");
                    ui.label("4. Change 'Output' to match the monitor you are drawing on.");
                    ui.add_space(10.0);
                    
                    ui.label(egui::RichText::new("GNOME (Linux)").strong().color(egui::Color32::WHITE));
                    ui.label("1. Open Settings > Wacom Tablet.");
                    ui.label("2. Or use the command line: `xsetwacom` (if using X11).");
                    ui.label("   For Wayland, mapping is usually automatic or requires specific GNOME extensions.");
                });
            self.show_help_dialog = open;
        }
    }
}

impl DtServerApp {
    // Helper for rendering stat cards
    fn stat_card(&self, ui: &mut egui::Ui, title: &str, value: &str, width: f32) {
        egui::Frame::none()
            .fill(egui::Color32::from_rgb(27, 30, 40))
            .rounding(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(width);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .size(11.0)
                            .color(egui::Color32::from_gray(160)),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(value)
                            .size(20.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                });
            });
    }
}
