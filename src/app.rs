use std::time::Instant;

use eframe::egui;

use crate::engine::EngineHandle;

pub struct SoundShareApp {
    engine: EngineHandle,
    last_packets: u64,
    last_check: Instant,
}

impl SoundShareApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, engine: EngineHandle) -> Self {
        Self {
            engine,
            last_packets: 0,
            last_check: Instant::now(),
        }
    }

    fn status_color(running: bool, has_error: bool) -> egui::Color32 {
        if has_error {
            egui::Color32::from_rgb(230, 120, 0)
        } else if running {
            egui::Color32::GREEN
        } else {
            egui::Color32::LIGHT_GRAY
        }
    }
}

impl eframe::App for SoundShareApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            let (running, has_error) = {
                let e = self.engine.0.lock().unwrap();
                (e.is_running(), e.snapshot().last_error.is_some())
            };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("SoundShare").heading().strong());
                ui.separator();
                let color = Self::status_color(running, has_error);
                let label = if has_error {
                    "error"
                } else if running {
                    "streaming"
                } else {
                    "stopped"
                };
                ui.colored_label(color, egui::RichText::new(label).italics().size(13.0));
                ui.add_space(16.0);
                if ui.button(if running { "Stop" } else { "Start" }).clicked() {
                    let mut e = self.engine.0.lock().unwrap();
                    if running {
                        e.stop();
                    } else {
                        e.start();
                    }
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let stats = {
                let e = self.engine.0.lock().unwrap();
                e.snapshot()
            };

            egui::Grid::new("stats_grid")
                .spacing(egui::vec2(16.0, 4.0))
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Packets sent");
                    ui.label(stats.packets_sent.to_string());
                    ui.end_row();
                    ui.label("Pkts/sec");
                    let now = Instant::now();
                    let delta = now.duration_since(self.last_check);
                    let rate = if delta.as_secs_f32() > 0.0 {
                        (stats.packets_sent.saturating_sub(self.last_packets) as f32
                            / delta.as_secs_f32())
                            .min(5000.0)
                    } else {
                        0.0
                    };
                    self.last_packets = stats.packets_sent;
                    self.last_check = now;
                    ui.label(format!("{rate:.0}"));
                    ui.end_row();
                    ui.label("Drop %");
                    ui.label(format!(
                        "{:.1}",
                        if stats.packets_sent > 0 {
                            stats.drops as f32 / (stats.drops + stats.packets_sent) as f32 * 100.0
                        } else {
                            0.0
                        }
                    ));
                    ui.end_row();
                    ui.label("Drops");
                    ui.label(stats.drops.to_string());
                    ui.end_row();
                    ui.label("Capture drops");
                    ui.label(stats.capture_drops.to_string());
                    ui.end_row();
                    ui.label("Encode CPU %");
                    ui.label(format!("{:.2}", stats.encode_cpu_pct * 100.0));
                    ui.end_row();
                    ui.label("Web clients");
                    ui.label(stats.web_clients.to_string());
                    ui.end_row();
                });

            ui.separator();

            if stats.running {
                let url = {
                    let e = self.engine.0.lock().unwrap();
                    e.web_url()
                };
                if let Some(url) = url {
                    ui.group(|ui| {
                        ui.label(egui::RichText::new("Share this URL").strong());
                        ui.label("Open this address on any device on the same network:");
                        ui.label(egui::RichText::new(&url).code());
                        if ui.button("Copy URL").clicked() {
                            ctx.output_mut(|o| o.copied_text = url.clone());
                        }
                    });
                } else {
                    ui.label("Web server starting...");
                }
            } else {
                ui.label("Click Start to begin streaming. Receivers can join via web browser.");
            }

            let last_err = {
                let e = self.engine.0.lock().unwrap();
                e.snapshot().last_error
            };
            if let Some(err) = &last_err {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("⚠ {err}"));
            }
        });

        ctx.request_repaint();
    }
}
