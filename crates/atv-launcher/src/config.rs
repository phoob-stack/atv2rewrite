//! Launcher settings, remembered between runs in the user's config folder
//! as a plain `key = value` file.

use std::path::PathBuf;

/// What Play starts: the game's own menus, or straight into one mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// The title screen ("Press Start"), as the original boots.
    Menu,
    SingleRace,
    TimeTrial,
    Freestyle,
    Arcade,
    Challenge,
    Academy,
    /// An online race others join (see the game's `net`).
    Host,
    /// Joins a friend's online race at an address.
    Join,
}

impl Start {
    pub const ALL: [Start; 9] =
        [Start::Menu, Start::SingleRace, Start::TimeTrial, Start::Freestyle, Start::Arcade, Start::Challenge, Start::Academy, Start::Host, Start::Join];

    pub fn label(self) -> &'static str {
        match self {
            Start::Menu => "Start menu",
            Start::SingleRace => "Single Race",
            Start::TimeTrial => "Time Trial",
            Start::Freestyle => "Open Freestyle",
            Start::Arcade => "Arcade",
            Start::Challenge => "Freestyle Challenge",
            Start::Academy => "ATV Academy",
            Start::Host => "Host online race",
            Start::Join => "Join online race",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Start::Menu => "Opens the game's title screen. Pick a mode, rider and quad in the game's own menus.",
            Start::SingleRace => "Straight into a race against computer riders.",
            Start::TimeTrial => "Straight into a race against the clock and your ghost.",
            Start::Freestyle => "Roam any track scoring tricks against the clock.",
            Start::Arcade => "Arcade from the first round: beat the clock at each checkpoint.",
            Start::Challenge => "One of the Freestyle Challenges in the stadium.",
            Start::Academy => "Practice tricks in the stadium with no time limit.",
            Start::Host => "A race friends join over the internet. Open UDP port 7777 on your router to this PC, give them your public IP address, and press Enter in the lobby once they're in.",
            Start::Join => "Joins a friend's race: type the address of the PC hosting it (their public IP, or name:port).",
        }
    }

    /// The word kept in the settings file.
    fn key(self) -> &'static str {
        match self {
            Start::Menu => "menu",
            Start::SingleRace => "race",
            Start::TimeTrial => "timetrial",
            Start::Freestyle => "freestyle",
            Start::Arcade => "arcade",
            Start::Challenge => "challenge",
            Start::Academy => "academy",
            Start::Host => "host",
            Start::Join => "join",
        }
    }

    fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == k)
    }

    /// Picks a track (races and freestyle; the rest have their own).
    pub fn picks_track(self) -> bool {
        matches!(self, Start::SingleRace | Start::TimeTrial | Start::Freestyle | Start::Host)
    }

    /// Races go round circuits; freestyle roams any track.
    pub fn circuits_only(self) -> bool {
        matches!(self, Start::SingleRace | Start::TimeTrial | Start::Host)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// The player's disc image or extracted folder.
    pub game: PathBuf,
    pub start: Start,
    pub track: String,
    pub quad: String,
    /// Rider model folder under `Characters`.
    pub rider: String,
    pub laps: u32,
    /// Open Freestyle's time limit in minutes (0 = unlimited).
    pub minutes: u32,
    /// The Freestyle Challenge (1-based).
    pub challenge: usize,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    /// Play with the HD texture pack when one has been built.
    pub hd: bool,
    /// Also upscale track, quad and rider textures when building the pack.
    pub hd_world: bool,
    /// The upscaler, when the player picked it by hand (empty = find it).
    pub upscaler: PathBuf,
    /// Where the HD pack is built and read (empty = the default folder).
    pub hd_dir: PathBuf,
    /// The host's address, for joining an online race.
    pub address: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            game: std::env::var_os("ATV2_GAME").map(PathBuf::from).unwrap_or_default(),
            start: Start::Menu,
            track: "Dunes1".into(),
            quad: "Bandit".into(),
            rider: "Male1".into(),
            laps: 3,
            minutes: 3,
            challenge: 1,
            width: 1280,
            height: 720,
            fullscreen: false,
            hd: true,
            hd_world: false,
            upscaler: PathBuf::new(),
            hd_dir: PathBuf::new(),
            address: String::new(),
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
                "start" => c.start = Start::from_key(v).unwrap_or(c.start),
                "track" => c.track = v.into(),
                "quad" => c.quad = v.into(),
                "rider" if !v.is_empty() => c.rider = v.into(),
                "laps" => c.laps = v.parse::<u32>().map_or(c.laps, |l| l.clamp(1, 9)),
                "minutes" => c.minutes = v.parse().unwrap_or(c.minutes),
                "challenge" => c.challenge = v.parse::<usize>().map_or(c.challenge, |n| n.max(1)),
                "width" => c.width = v.parse().unwrap_or(c.width),
                "height" => c.height = v.parse().unwrap_or(c.height),
                "fullscreen" => c.fullscreen = v == "true",
                "hd" => c.hd = v == "true",
                "hd_world" => c.hd_world = v == "true",
                "upscaler" => c.upscaler = PathBuf::from(v),
                "hd_dir" => c.hd_dir = PathBuf::from(v),
                "address" => c.address = v.into(),
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
        std::fs::write(path, self.to_text())
    }

    fn to_text(&self) -> String {
        format!(
            "game = {}\nstart = {}\ntrack = {}\nquad = {}\nrider = {}\nlaps = {}\nminutes = {}\nchallenge = {}\nwidth = {}\nheight = {}\nfullscreen = {}\nhd = {}\nhd_world = {}\nupscaler = {}\nhd_dir = {}\naddress = {}\n",
            self.game.display(),
            self.start.key(),
            self.track,
            self.quad,
            self.rider,
            self.laps,
            self.minutes,
            self.challenge,
            self.width,
            self.height,
            self.fullscreen,
            self.hd,
            self.hd_world,
            self.upscaler.display(),
            self.hd_dir.display(),
            self.address
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_keys_round_trip() {
        for s in Start::ALL {
            assert_eq!(Start::from_key(s.key()), Some(s));
        }
        assert_eq!(Start::from_key("nonsense"), None);
    }

    #[test]
    fn text_lists_every_setting() {
        let text = Config { start: Start::Challenge, hd_dir: PathBuf::from("packs"), ..Default::default() }.to_text();
        assert!(text.contains("start = challenge\n"));
        assert!(text.contains("hd_dir = packs\n"));
    }
}
