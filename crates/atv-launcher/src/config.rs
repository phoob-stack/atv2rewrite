//! Launcher settings, remembered between runs in the user's config folder
//! as a plain `key = value` file.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// The player's disc image or extracted folder.
    pub game: PathBuf,
    pub track: String,
    pub quad: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            game: std::env::var_os("ATV2_GAME").map(PathBuf::from).unwrap_or_default(),
            track: "Dunes1".into(),
            quad: "Bandit".into(),
            width: 1280,
            height: 720,
            fullscreen: false,
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "atv2").map(|d| d.config_dir().join("launcher.cfg"))
    }

    pub fn load() -> Self {
        let mut c = Self::default();
        let Some(text) = Self::path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return c;
        };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "game" if !v.is_empty() => c.game = PathBuf::from(v),
                "track" => c.track = v.into(),
                "quad" => c.quad = v.into(),
                "width" => c.width = v.parse().unwrap_or(c.width),
                "height" => c.height = v.parse().unwrap_or(c.height),
                "fullscreen" => c.fullscreen = v == "true",
                _ => {}
            }
        }
        c
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = format!(
            "game = {}\ntrack = {}\nquad = {}\nwidth = {}\nheight = {}\nfullscreen = {}\n",
            self.game.display(),
            self.track,
            self.quad,
            self.width,
            self.height,
            self.fullscreen
        );
        std::fs::write(path, text)
    }
}
