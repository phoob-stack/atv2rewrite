//! Launcher for ATV Quad Power Racing 2 on PC: pick your own copy of the
//! game once, choose a track, quad and display settings, and press Play.
//!
//! Settings are remembered between runs. The game binary (`atv2`) is
//! expected next to the launcher, or on the PATH.

mod config;
mod game;
mod hd;

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use config::Config;
use game::{Catalog, Exit, Running};

const RESOLUTIONS: [(u32, u32); 6] = [(1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)];
const ACCENT: egui::Color32 = egui::Color32::from_rgb(232, 140, 32);

enum Scan {
    Idle,
    Busy(mpsc::Receiver<anyhow::Result<Catalog>>),
    Done(Catalog),
    Failed(String),
}

struct Launcher {
    cfg: Config,
    /// Path text being edited; scanned when it changes.
    path_text: String,
    scan: Scan,
    running: Option<Running>,
    message: Option<(bool, String)>,
    build: Option<hd::Build>,
    /// Textures in the HD pack folder.
    pack: usize,
}

impl Launcher {
    fn new() -> Self {
        let cfg = Config::load();
        let pack = hd::pack_dir().map(|d| hd::pack_size(&d)).unwrap_or(0);
        let mut l = Self {
            path_text: cfg.game.display().to_string(),
            cfg,
            scan: Scan::Idle,
            running: None,
            message: None,
            build: None,
            pack,
        };
        l.rescan();
        l
    }

    fn set_game(&mut self, path: PathBuf) {
        self.path_text = path.display().to_string();
        self.rescan();
    }

    fn rescan(&mut self) {
        self.cfg.game = PathBuf::from(self.path_text.trim());
        if self.cfg.game.as_os_str().is_empty() {
            self.scan = Scan::Idle;
            return;
        }
        let (tx, rx) = mpsc::channel();
        let path = self.cfg.game.clone();
        std::thread::spawn(move || _ = tx.send(game::scan(&path)));
        self.scan = Scan::Busy(rx);
    }

    fn play(&mut self) {
        if let Err(e) = self.cfg.save() {
            eprintln!("could not save launcher settings: {e}");
        }
        match Running::start(&self.cfg) {
            Ok(r) => {
                self.running = Some(r);
                self.message = None;
            }
            Err(e) => self.message = Some((false, format!("{e:#}"))),
        }
    }

    fn poll(&mut self) {
        if let Scan::Busy(rx) = &self.scan {
            if let Ok(result) = rx.try_recv() {
                self.scan = match result {
                    Ok(cat) => {
                        // Keep the remembered picks if this copy has them.
                        if !cat.tracks.contains(&self.cfg.track) {
                            self.cfg.track = cat.tracks[0].clone();
                        }
                        if !cat.quads.contains(&self.cfg.quad) {
                            self.cfg.quad = cat.quads.first().cloned().unwrap_or_default();
                        }
                        _ = self.cfg.save();
                        Scan::Done(cat)
                    }
                    Err(e) => Scan::Failed(format!("{e:#}")),
                };
            }
        }
        if let Some(result) = self.build.as_ref().and_then(|b| b.poll()) {
            let cancelled = self.build.take().is_some_and(|b| b.cancel.load(std::sync::atomic::Ordering::Relaxed));
            self.pack = hd::pack_dir().map(|d| hd::pack_size(&d)).unwrap_or(0);
            self.message = Some(match result {
                Ok(s) => (true, hd::summary_text(&s, cancelled)),
                Err(e) => (false, format!("Building the HD pack failed: {e:#}")),
            });
        }
        if let Some(r) = &mut self.running {
            match r.poll() {
                None => {}
                Some(Exit::Ok) => self.running = None,
                Some(Exit::Failed(msg)) => {
                    self.running = None;
                    self.message = Some((false, msg));
                }
            }
        }
    }
}

impl eframe::App for Launcher {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if matches!(self.scan, Scan::Busy(_)) || self.running.is_some() || self.build.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let frame = egui::Frame::central_panel(ui.style()).inner_margin(20);
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            ui.label(egui::RichText::new("ATV Quad Power Racing 2").size(28.0).strong().color(ACCENT));
            ui.label("PC rewrite. Plays from your own copy of the game.");
            ui.add_space(14.0);

            self.game_section(ui);
            ui.add_space(14.0);
            let catalog = match &self.scan {
                Scan::Done(c) => Some(c.clone()),
                _ => None,
            };
            ui.add_enabled_ui(catalog.is_some(), |ui| self.options_section(ui, catalog.as_ref()));

            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                let ready = catalog.is_some() && self.running.is_none() && self.build.is_none();
                let text = if self.running.is_some() { "Running…" } else { "Play" };
                let button = egui::Button::new(egui::RichText::new(text).size(22.0).strong().color(egui::Color32::BLACK))
                    .fill(ACCENT)
                    .min_size(egui::vec2(ui.available_width(), 48.0));
                if ui.add_enabled(ready, button).clicked() {
                    self.play();
                }
                if let Some((ok, msg)) = &self.message {
                    let color = if *ok { ui.visuals().text_color() } else { ui.visuals().error_fg_color };
                    egui::ScrollArea::vertical().max_height(90.0).show(ui, |ui| {
                        ui.label(egui::RichText::new(msg).color(color).monospace().size(11.0));
                    });
                }
            });
        });
    }
}

impl Launcher {
    fn game_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Game data", |ui| {
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.path_text)
                        .hint_text("Disc image (.iso / .bin) or extracted folder")
                        .desired_width(ui.available_width() - 150.0),
                );
                if edit.lost_focus() && PathBuf::from(self.path_text.trim()) != self.cfg.game {
                    self.rescan();
                }
                if ui.button("Disc image…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .set_title("Choose your ATV Quad Power Racing 2 disc image")
                        .add_filter("Xbox disc image", &["iso", "bin", "xiso"])
                        .add_filter("All files", &["*"])
                        .pick_file()
                    {
                        self.set_game(p);
                    }
                }
                if ui.button("Folder…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .set_title("Choose the folder you extracted the disc to")
                        .pick_folder()
                    {
                        self.set_game(p);
                    }
                }
            });
            ui.add_space(4.0);
            match &self.scan {
                Scan::Idle => {
                    ui.label("Choose the disc image or the folder you extracted it to. Nothing is copied.");
                }
                Scan::Busy(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking your game data…");
                    });
                }
                Scan::Done(c) => {
                    let kind = if c.disc_image { "disc image" } else { "extracted folder" };
                    ui.colored_label(
                        egui::Color32::from_rgb(110, 200, 110),
                        format!("✔ Game found ({kind}): {} tracks, {} quads", c.tracks.len(), c.quads.len()),
                    );
                }
                Scan::Failed(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, format!("✖ {e}"));
                }
            }
        });
    }

    fn options_section(&mut self, ui: &mut egui::Ui, catalog: Option<&Catalog>) {
        let (tracks, quads) = catalog.map(|c| (c.tracks.as_slice(), c.quads.as_slice())).unwrap_or_default();
        section(ui, "Race", |ui| {
            egui::Grid::new("race").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                ui.label("Track");
                egui::ComboBox::from_id_salt("track")
                    .width(220.0)
                    .selected_text(game::track_label(&self.cfg.track))
                    .show_ui(ui, |ui| {
                        for t in tracks {
                            ui.selectable_value(&mut self.cfg.track, t.clone(), game::track_label(t));
                        }
                    });
                ui.end_row();
                ui.label("Quad");
                egui::ComboBox::from_id_salt("quad").width(220.0).selected_text(&self.cfg.quad).show_ui(ui, |ui| {
                    for q in quads {
                        ui.selectable_value(&mut self.cfg.quad, q.clone(), q);
                    }
                });
                ui.end_row();
            });
        });
        ui.add_space(14.0);
        section(ui, "Display", |ui| {
            egui::Grid::new("display").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                ui.label("Resolution");
                ui.add_enabled_ui(!self.cfg.fullscreen, |ui| {
                    egui::ComboBox::from_id_salt("res")
                        .width(220.0)
                        .selected_text(format!("{} × {}", self.cfg.width, self.cfg.height))
                        .show_ui(ui, |ui| {
                            for (w, h) in RESOLUTIONS {
                                let picked = (self.cfg.width, self.cfg.height) == (w, h);
                                if ui.selectable_label(picked, format!("{w} × {h}")).clicked() {
                                    (self.cfg.width, self.cfg.height) = (w, h);
                                }
                            }
                        });
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.cfg.fullscreen, "Fullscreen (uses your desktop resolution)");
                ui.end_row();
            });
        });
        ui.add_space(14.0);
        self.hd_section(ui);
    }

    fn hd_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "HD textures (AI upscaled)", |ui| {
            let upscaler = hd::upscaler(&self.cfg);
            if let Some(b) = &self.build {
                let p = b.progress.lock().unwrap().clone();
                let frac = if p.total == 0 { 0.0 } else { p.done as f32 / p.total as f32 };
                ui.add(egui::ProgressBar::new(frac).show_percentage().text(format!("{} / {}  {}", p.done, p.total, p.current)));
                if ui.button("Stop").clicked() {
                    b.cancel();
                }
                return;
            }
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.cfg.hd, "Use HD textures");
                ui.label(if self.pack > 0 {
                    format!("({} textures built)", self.pack)
                } else {
                    "(none built yet)".to_string()
                });
            });
            ui.checkbox(&mut self.cfg.hd_world, "Also upscale tracks, quads and riders (slower, a few hundred MB)");
            match &upscaler {
                Some(u) => {
                    ui.label(egui::RichText::new(format!("Upscaler: {}", u.exe.display())).size(11.0).weak());
                }
                None => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Needs Real-ESRGAN (ncnn-vulkan). Unzip it next to the launcher, or");
                        if ui.link("locate it…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().set_title("Choose realesrgan-ncnn-vulkan").pick_file() {
                                self.cfg.upscaler = p;
                            }
                        }
                    });
                    ui.hyperlink_to("Download Real-ESRGAN", atv_hd::UPSCALER_URL);
                }
            }
            ui.horizontal(|ui| {
                let dir = hd::pack_dir();
                if ui.add_enabled(upscaler.is_some() && dir.is_some(), egui::Button::new("Build HD pack")).clicked() {
                    _ = self.cfg.save();
                    self.message = None;
                    self.build = Some(hd::Build::start(&self.cfg, upscaler.unwrap(), dir.clone().unwrap()));
                }
                if self.pack > 0 && ui.button("Delete pack").clicked() {
                    if let Some(d) = &dir {
                        _ = std::fs::remove_dir_all(d);
                    }
                    self.pack = 0;
                }
            });
            ui.label(egui::RichText::new("Made on this PC from your own disc. No game art is downloaded or shared.").size(11.0).weak());
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.label(egui::RichText::new(title).size(15.0).strong());
    ui.add_space(4.0);
    egui::Frame::group(ui.style()).inner_margin(10).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ATV Quad Power Racing 2 Launcher")
            .with_inner_size([620.0, 700.0])
            .with_min_inner_size([520.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ATV Quad Power Racing 2 Launcher",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(Launcher::new()))
        }),
    )
}
