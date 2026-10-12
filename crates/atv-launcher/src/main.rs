//! Launcher for ATV Quad Power Racing 2 on PC: pick your own copy of the
//! game once and press Play to boot into the game's start menu, or go
//! straight into a mode with a track, rider and quad picked here.
//!
//! The Config tab tunes the physics: every quad, or each quad's engine,
//! handling, jumps and suspension (see [`tuning`]).
//!
//! Settings are remembered between runs. The game binary (`atv2`) is
//! expected next to the launcher, or on the PATH.

mod config;
mod game;
mod hd;
mod tuning;

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use config::{Config, Start};
use game::{Catalog, Exit, Running};

const RESOLUTIONS: [(u32, u32); 6] = [(1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)];
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Play,
    Config,
}

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
    /// Upscaler and HD pack folder paths being edited.
    upscaler_text: String,
    hd_dir_text: String,
    tab: Tab,
    tune: tuning::Config,
}

impl Launcher {
    fn new() -> Self {
        let cfg = Config::load();
        let pack = hd::pack_dir(&cfg).map(|d| hd::pack_size(&d)).unwrap_or(0);
        let mut l = Self {
            path_text: cfg.game.display().to_string(),
            upscaler_text: cfg.upscaler.display().to_string(),
            hd_dir_text: cfg.hd_dir.display().to_string(),
            tune: tuning::Config::load(&cfg.quad),
            tab: Tab::Play,
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
        self.tune.save_if_changed();
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

    fn refresh_pack(&mut self) {
        self.pack = hd::pack_dir(&self.cfg).map(|d| hd::pack_size(&d)).unwrap_or(0);
    }

    fn set_upscaler(&mut self, path: PathBuf) {
        self.upscaler_text = path.display().to_string();
        self.cfg.upscaler = path;
        _ = self.cfg.save();
    }

    fn set_hd_dir(&mut self, path: PathBuf) {
        self.hd_dir_text = path.display().to_string();
        self.cfg.hd_dir = path;
        _ = self.cfg.save();
        self.refresh_pack();
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
                        if !cat.riders.iter().any(|(_, m)| *m == self.cfg.rider)
                            && let Some((_, m)) = cat.riders.first()
                        {
                            self.cfg.rider = m.clone();
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
            self.refresh_pack();
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
        let catalog = match &self.scan {
            Scan::Done(c) => Some(c.clone()),
            _ => None,
        };
        let ready = catalog.is_some() && self.running.is_none() && self.build.is_none();

        egui::Panel::top("header").frame(egui::Frame::new().inner_margin(egui::Margin { left: 20, right: 20, top: 16, bottom: 10 }).fill(ui.visuals().extreme_bg_color)).show(ui, |ui| {
            ui.label(egui::RichText::new("ATV Quad Power Racing 2").size(28.0).strong().color(ACCENT));
            ui.label("PC rewrite. Plays from your own copy of the game.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (tab, name) in [(Tab::Play, "Play"), (Tab::Config, "Config")] {
                    let text = egui::RichText::new(name).size(15.0);
                    let text = if self.tab == tab { text.strong().color(ACCENT) } else { text };
                    if ui.add(egui::Button::selectable(self.tab == tab, text).min_size(egui::vec2(80.0, 26.0))).clicked() {
                        self.tab = tab;
                    }
                }
                if !self.tune.tuning.is_empty() {
                    ui.label(egui::RichText::new("Tuning on").size(12.0).color(ACCENT)).on_hover_text("Some physics differ from the game's (Config tab).");
                }
            });
        });

        egui::Panel::bottom("play").frame(egui::Frame::new().inner_margin(egui::Margin { left: 20, right: 20, top: 10, bottom: 16 }).fill(ui.visuals().extreme_bg_color)).show(ui, |ui| {
            if let Some((ok, msg)) = &self.message {
                let color = if *ok { ui.visuals().text_color() } else { ui.visuals().error_fg_color };
                egui::ScrollArea::vertical().max_height(90.0).show(ui, |ui| {
                    ui.label(egui::RichText::new(msg).color(color).monospace().size(11.0));
                });
                ui.add_space(6.0);
            }
            let text = if self.running.is_some() {
                "Running…".to_string()
            } else if self.cfg.start == Start::Menu {
                "Play".to_string()
            } else {
                format!("Play {}", self.cfg.start.label())
            };
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                let button = egui::Button::new(egui::RichText::new(text).size(22.0).strong().color(egui::Color32::BLACK))
                    .fill(ACCENT)
                    .min_size(egui::vec2(ui.available_width(), 48.0));
                if ui.add_enabled(ready, button).clicked() {
                    self.play();
                }
            });
        });

        let frame = egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin { left: 20, right: 12, top: 14, bottom: 8 });
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.set_width(ui.available_width() - 8.0);
                match self.tab {
                    Tab::Play => {
                        self.game_section(ui);
                        ui.add_space(14.0);
                        ui.add_enabled_ui(catalog.is_some(), |ui| self.options_section(ui, catalog.as_ref()));
                        ui.add_space(8.0);
                    }
                    Tab::Config => self.tune.ui(ui, catalog.as_ref()),
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
        section(ui, "Game mode", |ui| {
            egui::Grid::new("mode").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                ui.label("Start in");
                egui::ComboBox::from_id_salt("start").width(220.0).selected_text(self.cfg.start.label()).show_ui(ui, |ui| {
                    for s in Start::ALL {
                        ui.selectable_value(&mut self.cfg.start, s, s.label());
                    }
                });
                ui.end_row();
            });
            ui.add_space(2.0);
            ui.label(egui::RichText::new(self.cfg.start.hint()).size(12.0).weak());
            if self.cfg.start == Start::Menu {
                return;
            }
            ui.add_space(6.0);
            self.mode_options(ui, catalog);
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

    /// Track, rider, quad and the mode's own settings for starting
    /// straight into a mode.
    fn mode_options(&mut self, ui: &mut egui::Ui, catalog: Option<&Catalog>) {
        let start = self.cfg.start;
        let empty = Catalog { disc_image: false, tracks: Vec::new(), arenas: Vec::new(), quads: Vec::new(), riders: Vec::new(), params: Default::default() };
        let cat = catalog.unwrap_or(&empty);
        // A race can't start on a stadium, so move off one.
        let tracks: Vec<&String> = cat.tracks_for(start).collect();
        if start.picks_track() && !tracks.contains(&&self.cfg.track) && let Some(t) = tracks.first() {
            self.cfg.track = (*t).clone();
        }
        egui::Grid::new("race").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            if start.picks_track() {
                ui.label("Track");
                egui::ComboBox::from_id_salt("track")
                    .width(220.0)
                    .selected_text(game::track_label(&self.cfg.track))
                    .show_ui(ui, |ui| {
                        for t in &tracks {
                            ui.selectable_value(&mut self.cfg.track, (*t).clone(), game::track_label(t));
                        }
                    });
                ui.end_row();
            }
            match start {
                Start::SingleRace | Start::TimeTrial => {
                    ui.label("Laps");
                    ui.add(egui::Slider::new(&mut self.cfg.laps, 1..=9));
                    ui.end_row();
                }
                Start::Freestyle => {
                    ui.label("Time limit");
                    egui::ComboBox::from_id_salt("minutes").width(220.0).selected_text(minutes_label(self.cfg.minutes)).show_ui(ui, |ui| {
                        for m in game::MINUTES {
                            ui.selectable_value(&mut self.cfg.minutes, m, minutes_label(m));
                        }
                    });
                    ui.end_row();
                }
                Start::Challenge => {
                    ui.label("Challenge");
                    egui::ComboBox::from_id_salt("challenge")
                        .width(220.0)
                        .selected_text(format!("Challenge {}", self.cfg.challenge))
                        .show_ui(ui, |ui| {
                            for n in 1..=game::CHALLENGES {
                                ui.selectable_value(&mut self.cfg.challenge, n, format!("Challenge {n}"));
                            }
                        });
                    ui.end_row();
                }
                _ => {}
            }
            ui.label("Rider");
            let rider_name = cat.riders.iter().find(|(_, m)| *m == self.cfg.rider).map_or(self.cfg.rider.as_str(), |(n, _)| n.as_str());
            egui::ComboBox::from_id_salt("rider").width(220.0).selected_text(rider_name).show_ui(ui, |ui| {
                for (name, model) in &cat.riders {
                    ui.selectable_value(&mut self.cfg.rider, model.clone(), name);
                }
            });
            ui.end_row();
            ui.label("Quad");
            egui::ComboBox::from_id_salt("quad").width(220.0).selected_text(&self.cfg.quad).show_ui(ui, |ui| {
                for q in &cat.quads {
                    ui.selectable_value(&mut self.cfg.quad, q.clone(), q);
                }
            });
            ui.end_row();
        });
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
            ui.add_space(4.0);
            egui::Grid::new("hd_paths").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Upscaler");
                ui.horizontal(|ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.upscaler_text)
                            .hint_text("Found automatically")
                            .desired_width(ui.available_width() - 130.0),
                    );
                    if edit.lost_focus() && PathBuf::from(self.upscaler_text.trim()) != self.cfg.upscaler {
                        self.set_upscaler(PathBuf::from(self.upscaler_text.trim()));
                    }
                    if ui.button("Browse…").clicked()
                        && let Some(p) = rfd::FileDialog::new().set_title("Choose realesrgan-ncnn-vulkan").pick_file()
                    {
                        self.set_upscaler(p);
                    }
                    if ui.add_enabled(!self.cfg.upscaler.as_os_str().is_empty(), egui::Button::new("Auto")).on_hover_text("Find it next to the launcher or on the PATH").clicked() {
                        self.set_upscaler(PathBuf::new());
                    }
                });
                ui.end_row();
                ui.label("");
                match &upscaler {
                    Some(u) if self.cfg.upscaler.as_os_str().is_empty() => {
                        ui.label(egui::RichText::new(format!("Found: {}", u.exe.display())).size(11.0).weak());
                    }
                    Some(_) => {
                        ui.colored_label(egui::Color32::from_rgb(110, 200, 110), egui::RichText::new("✔ Using this upscaler").size(11.0));
                    }
                    None if self.cfg.upscaler.as_os_str().is_empty() => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("Needs Real-ESRGAN (ncnn-vulkan). Unzip it next to the launcher or browse to it.").size(11.0));
                            ui.hyperlink_to(egui::RichText::new("Download").size(11.0), atv_hd::UPSCALER_URL);
                        });
                    }
                    None => {
                        ui.colored_label(ui.visuals().error_fg_color, egui::RichText::new("✖ No file at that path").size(11.0));
                    }
                }
                ui.end_row();

                ui.label("Pack folder");
                ui.horizontal(|ui| {
                    let hint = hd::pack_dir(&Config::default()).map(|d| d.display().to_string()).unwrap_or_default();
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.hd_dir_text)
                            .hint_text(hint)
                            .desired_width(ui.available_width() - 130.0),
                    );
                    if edit.lost_focus() && PathBuf::from(self.hd_dir_text.trim()) != self.cfg.hd_dir {
                        self.set_hd_dir(PathBuf::from(self.hd_dir_text.trim()));
                    }
                    if ui.button("Browse…").clicked()
                        && let Some(p) = rfd::FileDialog::new().set_title("Choose where to keep the HD textures").pick_folder()
                    {
                        self.set_hd_dir(p);
                    }
                    if ui.add_enabled(!self.cfg.hd_dir.as_os_str().is_empty(), egui::Button::new("Default")).clicked() {
                        self.set_hd_dir(PathBuf::new());
                    }
                });
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let dir = hd::pack_dir(&self.cfg);
                if ui.add_enabled(upscaler.is_some() && dir.is_some(), egui::Button::new("Build HD pack")).clicked() {
                    _ = self.cfg.save();
                    self.message = None;
                    self.build = Some(hd::Build::start(&self.cfg, upscaler.unwrap(), dir.clone().unwrap()));
                }
                if self.pack > 0 && ui.button("Delete pack").clicked() {
                    if let Some(d) = &dir {
                        hd::delete_pack(d);
                    }
                    self.refresh_pack();
                }
            });
            ui.label(egui::RichText::new("Made on this PC from your own disc. No game art is downloaded or shared.").size(11.0).weak());
        });
    }
}

fn minutes_label(m: u32) -> String {
    match m {
        0 => "Unlimited".into(),
        1 => "1 minute".into(),
        m => format!("{m} minutes"),
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
            .with_inner_size([640.0, 780.0])
            .with_min_inner_size([560.0, 520.0]),
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
