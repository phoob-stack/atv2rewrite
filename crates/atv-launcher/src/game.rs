//! Checking the player's game data and starting the game binary.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use atv_formats::GameSource;

use crate::config::Config;

/// What the launcher found in the player's copy of the game.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub disc_image: bool,
    pub tracks: Vec<String>,
    pub quads: Vec<String>,
}

/// Environments in the order the original game lists them.
const ENVIRONMENTS: [&str; 6] = ["Dunes", "Forest", "Swamp", "Industrial", "Glacier", "Stadium"];

/// Opens the disc image or folder with the game's own loader and lists the
/// tracks and quads it contains.
pub fn scan(path: &Path) -> anyhow::Result<Catalog> {
    if path.as_os_str().is_empty() {
        anyhow::bail!("choose your ATV Quad Power Racing 2 disc image or extracted folder");
    }
    if !path.exists() {
        anyhow::bail!("{} does not exist", path.display());
    }
    let game = GameSource::open(path).map_err(|e| anyhow::anyhow!("not a readable ATV Quad Power Racing 2 copy: {e}"))?;

    let mut tracks = Vec::new();
    let mut quads = Vec::new();
    for p in game.archive_paths() {
        let parts: Vec<&str> = p.trim_start_matches('/').split('/').collect();
        match parts.as_slice() {
            ["Levels", name, "World", file] if file.eq_ignore_ascii_case(&format!("{name}.row")) => {
                tracks.push(name.to_string())
            }
            ["Cars", name, file] if *name != "Generic" && file.eq_ignore_ascii_case(&format!("{name}.params")) => {
                quads.push(name.to_string())
            }
            _ => {}
        }
    }
    if tracks.is_empty() {
        anyhow::bail!("no tracks found; is this ATV Quad Power Racing 2?");
    }
    tracks.sort_by_key(|t| {
        let env = t.trim_end_matches(|c: char| c.is_ascii_digit());
        (ENVIRONMENTS.iter().position(|e| *e == env).unwrap_or(ENVIRONMENTS.len()), t.clone())
    });
    quads.sort();
    quads.dedup();
    Ok(Catalog { disc_image: !path.is_dir(), tracks, quads })
}

/// "Industrial2" -> "Industrial 2".
pub fn track_label(track: &str) -> String {
    match track.find(|c: char| c.is_ascii_digit()) {
        Some(i) => format!("{} {}", &track[..i], &track[i..]),
        None => track.to_string(),
    }
}

/// The game binary: next to the launcher, or else on the PATH.
pub fn game_binary() -> PathBuf {
    let name = format!("atv2{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(&name)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// A running game, with the tail of its error output kept for the launcher
/// to show if it fails.
pub struct Running {
    child: Child,
    log: Arc<Mutex<Vec<String>>>,
}

pub enum Exit {
    Ok,
    Failed(String),
}

impl Running {
    pub fn start(cfg: &Config) -> anyhow::Result<Self> {
        let bin = game_binary();
        let mut cmd = Command::new(&bin);
        cmd.arg("--game")
            .arg(&cfg.game)
            .args(["--track", &cfg.track, "--quad", &cfg.quad])
            .args(["--resolution", &format!("{}x{}", cfg.width, cfg.height)])
            .stderr(Stdio::piped());
        if cfg.fullscreen {
            cmd.arg("--fullscreen");
        }
        if !cfg.hd {
            cmd.arg("--no-hd");
        }
        let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("could not start {}: {e}", bin.display()))?;

        let log = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr is piped");
        let sink = log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("{line}");
                let mut l = sink.lock().unwrap();
                l.push(line);
                if l.len() > 12 {
                    l.remove(0);
                }
            }
        });
        Ok(Self { child, log })
    }

    /// `None` while the game is still running.
    pub fn poll(&mut self) -> Option<Exit> {
        let status = self.child.try_wait().ok()??;
        if status.success() {
            return Some(Exit::Ok);
        }
        let tail = self.log.lock().unwrap().join("\n");
        Some(Exit::Failed(format!("The game stopped with {status}.\n{tail}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(track_label("Industrial2"), "Industrial 2");
        assert_eq!(track_label("Dunes1"), "Dunes 1");
        assert_eq!(track_label("Custom"), "Custom");
    }

    #[test]
    fn missing_path() {
        assert!(scan(Path::new("")).is_err());
        assert!(scan(Path::new("/definitely/not/here.iso")).is_err());
    }
}
