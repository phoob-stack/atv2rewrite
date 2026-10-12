//! Checking the player's game data and starting the game binary.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use atv_formats::{setup, GameSource};
use atv_sim::VehicleParams;

use crate::config::{Config, Start};

/// The Freestyle Challenges in the game.
pub const CHALLENGES: usize = 12;
/// Freestyle time limits as the game offers them (0 = unlimited).
pub const MINUTES: [u32; 7] = [1, 3, 5, 10, 15, 30, 0];
/// Where the Challenges and ATV Academy are held.
const STADIUM: &str = "Stadium2";

/// What the launcher found in the player's copy of the game.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub disc_image: bool,
    pub tracks: Vec<String>,
    /// Tracks with no laps to race (the stadiums): freestyle only.
    pub arenas: Vec<String>,
    pub quads: Vec<String>,
    /// (name, model folder) in the game's own order.
    pub riders: Vec<(String, String)>,
    /// Each quad's handling as the game's own files set it.
    pub params: Arc<BTreeMap<String, VehicleParams>>,
}

impl Catalog {
    /// The tracks a mode can start on.
    pub fn tracks_for(&self, start: Start) -> impl Iterator<Item = &String> {
        self.tracks.iter().filter(move |t| !start.circuits_only() || !self.arenas.contains(t))
    }
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

    let read = |p: &str| game.read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    // A track whose `Checkpoints` list is empty has no laps (as the game
    // reads Tracks.xml).
    let arenas = setup::entries(&read("/Setup/Tracks.xml"))
        .into_iter()
        .filter(|e| e.get("Checkpoints").is_some_and(|c| c.trim().is_empty()))
        .map(|e| e.name)
        .collect();
    let riders = setup::entries(&read("/Setup/Riders.xml"))
        .into_iter()
        .filter_map(|e| Some((e.name.replace('_', " "), e.get("mModelFilename")?.to_string())))
        .collect();
    let generic = read("/Cars/Generic/Generic.params");
    let params = quads
        .iter()
        .map(|q| (q.clone(), VehicleParams::from_files(&generic, Some(&read(&format!("/Cars/{q}/{q}.params"))))))
        .collect::<BTreeMap<_, _>>();
    let params = Arc::new(params);
    Ok(Catalog { disc_image: !path.is_dir(), tracks, arenas, quads, riders, params })
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

/// The game's arguments for the launcher's settings (after `--game`).
pub fn args(cfg: &Config) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut add = |v: &[&str]| a.extend(v.iter().map(|s| s.to_string()));
    let laps = cfg.laps.to_string();
    let minutes = cfg.minutes.to_string();
    let challenge = cfg.challenge.to_string();
    match cfg.start {
        // No track: the game opens on its title screen.
        Start::Menu => {}
        Start::SingleRace => add(&["--mode", "race", "--track", &cfg.track, "--laps", &laps]),
        Start::TimeTrial => add(&["--mode", "timetrial", "--track", &cfg.track, "--laps", &laps]),
        Start::Freestyle => add(&["--mode", "freestyle", "--track", &cfg.track, "--minutes", &minutes]),
        Start::Arcade => add(&["--mode", "arcade", "--round", "0"]),
        Start::Challenge => add(&["--challenge", &challenge]),
        Start::Academy => add(&["--mode", "training", "--track", STADIUM]),
        Start::Host => add(&["--host", "--track", &cfg.track, "--laps", &laps]),
        Start::Join => add(&["--join", cfg.address.trim()]),
    }
    if cfg.start != Start::Menu {
        add(&["--quad", &cfg.quad, "--rider", &cfg.rider]);
    }
    add(&["--resolution", &format!("{}x{}", cfg.width, cfg.height)]);
    if cfg.fullscreen {
        add(&["--fullscreen"]);
    }
    if !cfg.hd {
        add(&["--no-hd"]);
    } else if !cfg.hd_dir.as_os_str().is_empty() {
        a.push("--hd".into());
        a.push(cfg.hd_dir.display().to_string());
    }
    a
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
        cmd.arg("--game").arg(&cfg.game).args(args(cfg)).stderr(Stdio::piped());
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
    fn menu_start_opens_the_title_screen() {
        let a = args(&Config::default());
        assert!(!a.iter().any(|s| s == "--track" || s == "--mode"), "{a:?}");
    }

    #[test]
    fn mode_args() {
        let cfg = Config { start: Start::TimeTrial, track: "Swamp2".into(), laps: 5, ..Default::default() };
        let a = args(&cfg).join(" ");
        assert!(a.starts_with("--mode timetrial --track Swamp2 --laps 5 --quad Bandit --rider Male1"), "{a}");
        let cfg = Config { start: Start::Challenge, challenge: 7, hd_dir: "D:/hd".into(), ..Default::default() };
        let a = args(&cfg).join(" ");
        assert!(a.starts_with("--challenge 7 --quad"), "{a}");
        assert!(a.ends_with("--hd D:/hd"), "{a}");
    }

    #[test]
    fn missing_path() {
        assert!(scan(Path::new("")).is_err());
        assert!(scan(Path::new("/definitely/not/here.iso")).is_err());
    }
}
