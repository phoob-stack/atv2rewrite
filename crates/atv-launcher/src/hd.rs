//! Building the HD texture pack from the launcher: the game's own textures,
//! upscaled on this machine by Real-ESRGAN into the player's local data
//! folder, where the game picks them up.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use atv_formats::GameSource;
use atv_hd::{Options, Progress, Summary, Upscaler};

use crate::config::Config;

/// A build running on a background thread.
pub struct Build {
    pub progress: Arc<Mutex<Progress>>,
    pub cancel: Arc<AtomicBool>,
    done: std::sync::mpsc::Receiver<anyhow::Result<Summary>>,
}

impl Build {
    pub fn start(cfg: &Config, upscaler: Upscaler, out: PathBuf) -> Self {
        let progress = Arc::new(Mutex::new(Progress::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, done) = std::sync::mpsc::channel();
        let opts = Options { world: cfg.hd_world, ..Default::default() };
        let game = cfg.game.clone();
        let (p, c) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let res = GameSource::open(&game)
                .map_err(|e| anyhow::anyhow!("opening the game: {e}"))
                .and_then(|g| atv_hd::build(&g, &out, &upscaler, &opts, &c, |pr| *p.lock().unwrap() = pr.clone()));
            _ = tx.send(res);
        });
        Self { progress, cancel, done }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> Option<anyhow::Result<Summary>> {
        self.done.try_recv().ok()
    }
}

/// The upscaler: the one the player picked, else one found next to the
/// launcher or on the PATH.
pub fn upscaler(cfg: &Config) -> Option<Upscaler> {
    if cfg.upscaler.is_file() {
        return Some(Upscaler { exe: cfg.upscaler.clone() });
    }
    Upscaler::find()
}

pub fn pack_dir() -> Option<PathBuf> {
    atv_hd::default_dir()
}

pub fn pack_size(dir: &Path) -> usize {
    atv_hd::count(dir)
}

pub fn summary_text(s: &Summary, cancelled: bool) -> String {
    let mut t = format!(
        "{} {} textures upscaled ({} were already done).",
        if cancelled { "Stopped:" } else { "HD pack ready:" },
        s.written,
        s.kept
    );
    if !s.failed.is_empty() {
        t += &format!(" {} could not be read:", s.failed.len());
        for (p, e) in s.failed.iter().take(4) {
            t += &format!("\n{p}: {e}");
        }
    }
    t
}
