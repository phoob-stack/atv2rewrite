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

/// The upscaler: the one the player picked (none if it isn't there), else
/// one found next to the launcher or on the PATH.
pub fn upscaler(cfg: &Config) -> Option<Upscaler> {
    if !cfg.upscaler.as_os_str().is_empty() {
        return cfg.upscaler.is_file().then(|| Upscaler { exe: cfg.upscaler.clone() });
    }
    Upscaler::find()
}

/// Where the pack is built and read: the player's folder, else the
/// default one in their local data folder.
pub fn pack_dir(cfg: &Config) -> Option<PathBuf> {
    if cfg.hd_dir.as_os_str().is_empty() { atv_hd::default_dir() } else { Some(cfg.hd_dir.clone()) }
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

/// Deletes the pack's textures from `dir` and the folders they leave empty.
/// Only the pack's own files go, as the player may have picked a folder
/// with other things in it.
pub fn delete_pack(dir: &Path) {
    fn walk(d: &Path) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for p in rd.flatten().map(|e| e.path()) {
            if p.is_dir() {
                walk(&p);
                // Fails, as it should, unless the folder is now empty.
                _ = std::fs::remove_dir(&p);
            } else if p.extension().is_some_and(|x| x == "dds") {
                _ = std::fs::remove_file(&p);
            }
        }
    }
    _ = std::fs::remove_dir_all(dir.join(".work"));
    walk(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_keeps_other_files() {
        let dir = std::env::temp_dir().join(format!("atv-launcher-hd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("frontend/fonts")).unwrap();
        std::fs::write(dir.join("frontend/fonts/a.dds"), b"x").unwrap();
        std::fs::write(dir.join("frontend/b.dds"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"keep").unwrap();
        delete_pack(&dir);
        assert!(!dir.join("frontend").exists());
        assert!(dir.join("notes.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
