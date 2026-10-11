//! The Options menu's settings: music and sound effects volume, the race
//! camera, each controller's layout and vibration (see
//! [`crate::controller`]), and the player profile last loaded (see
//! [`crate::career::Profile`]). They are remembered between runs in the player's
//! config folder as a plain `key = value` file, beside the launcher's.
//!
//! The cameras are the original's three follow cameras from
//! `Setup/VehicleCameras.xml` (`FollowNear`, `FollowFar`, `FollowVeryClose`).

use std::path::PathBuf;

use bevy::prelude::*;

use atv_formats::{setup, GameSource};

use crate::controller::{self, PadSetup, PadStyle, PORTS};
use crate::xinput::PadReader;

/// Segments in the volume bars (`FrontEnd/SoundBar`).
pub const VOLUME_STEPS: u8 = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CameraView {
    Close,
    #[default]
    Far,
    VeryClose,
}

impl CameraView {
    pub const ALL: [CameraView; 3] = [CameraView::Close, CameraView::Far, CameraView::VeryClose];

    /// As the Options menu names it (`CAM_FOLLOW_*`).
    pub fn label(self) -> &'static str {
        match self {
            CameraView::Close => "FOLLOW CLOSE",
            CameraView::Far => "FOLLOW FAR",
            CameraView::VeryClose => "FOLLOW VERY CLOSE",
        }
    }

    /// Its entry in `VehicleCameras.xml`.
    fn entry(self) -> &'static str {
        match self {
            CameraView::Close => "FollowNear",
            CameraView::Far => "FollowFar",
            CameraView::VeryClose => "FollowVeryClose",
        }
    }

    fn key(self) -> &'static str {
        match self {
            CameraView::Close => "close",
            CameraView::Far => "far",
            CameraView::VeryClose => "very_close",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }
}

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct Settings {
    /// Volume bar levels, 0 to [`VOLUME_STEPS`].
    pub music: u8,
    pub effects: u8,
    pub camera: CameraView,
    /// Each controller port's layout and vibration.
    pub pads: [PadSetup; PORTS],
    /// The profile slot to load at start.
    pub profile: Option<usize>,
    /// Button pictures and names for this make of pad whatever pad is in
    /// use (`pad_pictures` in the file; for Steam Input, which shows every
    /// pad to the game as an Xbox one). None: the pad in use's own.
    pub pad_pictures: Option<PadStyle>,
    /// Where pads are read from (`pad_reader`): XInput for Steam Input,
    /// or natively. Read at start.
    pub pad_reader: PadReader,
}

impl Default for Settings {
    fn default() -> Self {
        Self { music: VOLUME_STEPS, effects: VOLUME_STEPS, camera: CameraView::default(), pads: controller::default_setups(), profile: None, pad_pictures: None, pad_reader: PadReader::Auto }
    }
}

/// Gain for a volume bar level: squared, so each step sounds about as big
/// as the last.
pub fn gain(level: u8) -> f32 {
    let x = f32::from(level.min(VOLUME_STEPS)) / f32::from(VOLUME_STEPS);
    x * x
}

impl Settings {
    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "atv2").map(|d| d.config_dir().join("options.cfg"))
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "music" => s.music = v.parse::<u8>().map_or(s.music, |n| n.min(VOLUME_STEPS)),
                "effects" => s.effects = v.parse::<u8>().map_or(s.effects, |n| n.min(VOLUME_STEPS)),
                "camera" => s.camera = CameraView::ALL.into_iter().find(|c| c.key() == v).unwrap_or(s.camera),
                // Before each controller had its own.
                "vibration" => s.pads.iter_mut().for_each(|p| p.vibration = v == "true"),
                "pad_pictures" => s.pad_pictures = PadStyle::parse(v),
                "pad_reader" => s.pad_reader = PadReader::parse(v).unwrap_or_default(),
                "profile" => s.profile = v.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|&n| n < crate::career::PROFILES),
                k => {
                    let pad = k.strip_prefix("pad").and_then(|r| r.split_once('_')).and_then(|(n, rest)| Some((n.parse::<usize>().ok()?.checked_sub(1)?, rest)));
                    if let Some((n, rest)) = pad.filter(|(n, _)| *n < PORTS) {
                        s.pads[n].parse_line(rest, v);
                    }
                }
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let mut s = format!("music = {}\neffects = {}\ncamera = {}\n", self.music, self.effects, self.camera.key());
        for (n, p) in self.pads.iter().enumerate() {
            s += &p.to_text(n);
        }
        if self.pad_reader != PadReader::Auto {
            s += &format!("pad_reader = {}\n", self.pad_reader.key());
        }
        if let Some(style) = self.pad_pictures {
            s += &format!("pad_pictures = {}\n", style.key());
        }
        if let Some(n) = self.profile {
            s += &format!("profile = {}\n", n + 1);
        }
        s
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, self.to_text())
        };
        if let Err(e) = write() {
            warn!("couldn't save the options to {}: {e}", path.display());
        }
    }
}

/// One follow camera: where it sits and looks relative to the quad (Bevy
/// space, forward is the quad's heading) and how quickly it catches up,
/// on the ground and in the air.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowCam {
    pub offset: Vec3,
    pub look: Vec3,
    pub pos_stiffness: [f32; 2],
    pub rot_stiffness: [f32; 2],
}

impl FollowCam {
    /// The disc's values, with the original's own as the fallback.
    pub fn load(game: &GameSource, view: CameraView) -> Self {
        let xml = game.read("/Setup/VehicleCameras.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        Self::parse(&xml, view)
    }

    fn parse(xml: &str, view: CameraView) -> Self {
        let mut c = Self::fallback(view);
        let num = |m: &std::collections::HashMap<String, String>, k: &str| m.get(k).and_then(|v| v.trim().parse::<f32>().ok());
        let vec = |child: &str| {
            let m = setup::child(xml, view.entry(), child)?;
            // Game frame to Bevy: Z is forward in the game, so it stays the
            // distance along the heading.
            Some(Vec3::new(num(&m, "X")?, num(&m, "Y")?, num(&m, "Z")?))
        };
        c.offset = vec("PosOffset").unwrap_or(c.offset);
        c.look = vec("LookOffset").unwrap_or(c.look);
        if let Some(e) = setup::entries(xml).into_iter().find(|e| e.name == view.entry()) {
            let pair = |on: &str, off: &str, d: [f32; 2]| [e.num(on).unwrap_or(d[0]), e.num(off).unwrap_or(d[1])];
            c.pos_stiffness = pair("OnGroundPosStiffness", "OffGroundPosStiffness", c.pos_stiffness);
            c.rot_stiffness = pair("OnGroundRotStiffness", "OffGroundRotStiffness", c.rot_stiffness);
        }
        c
    }

    fn fallback(view: CameraView) -> Self {
        let (offset, look) = match view {
            CameraView::Close => (Vec3::new(0.0, 1.5, -2.75), Vec3::new(0.0, 0.0, 1.25)),
            CameraView::Far => (Vec3::new(0.0, 2.7, -4.3), Vec3::new(0.0, 0.0, 1.0)),
            CameraView::VeryClose => (Vec3::new(0.0, 1.5, -0.75), Vec3::new(0.0, 0.0, 1.25)),
        };
        Self { offset, look, pos_stiffness: [6.0, 4.0], rot_stiffness: [4.0, 4.0] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = Settings { music: 3, effects: 16, camera: CameraView::VeryClose, profile: None, ..default() };
        s.pads[1].vibration = false;
        s.pads[2].layout = 2;
        s.pads[3].rebind(3, crate::controller::Action::Hop, 2, Some(crate::controller::Input::Key(KeyCode::KeyJ)));
        assert_eq!(Settings::parse(&s.to_text()), s);
        let s = Settings { profile: Some(9), pad_pictures: Some(PadStyle::Nintendo), pad_reader: PadReader::Native, ..s };
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(Settings::parse("music = 99\njunk\ncamera = sideways\npad9_layout = 2"), Settings { music: VOLUME_STEPS, ..default() });
    }

    #[test]
    fn old_vibration_switch_covers_every_controller() {
        assert!(Settings::parse("vibration = false").pads.iter().all(|p| !p.vibration));
    }

    #[test]
    fn gain_ends() {
        assert_eq!(gain(0), 0.0);
        assert_eq!(gain(VOLUME_STEPS), 1.0);
        assert!((gain(VOLUME_STEPS / 2) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn follow_camera_from_xml() {
        let xml = r#"<Root>
   <FollowFar OnGroundPosStiffness="7.0" OffGroundPosStiffness="3.0" OnGroundRotStiffness="4.5" OffGroundRotStiffness="2.0">
      <PosOffset X="0.0" Y="2.5" Z="-4.0"/>
      <LookOffset X="0.0" Y="0.5" Z="1.0"/>
   </FollowFar>
</Root>"#;
        let c = FollowCam::parse(xml, CameraView::Far);
        assert_eq!(c.offset, Vec3::new(0.0, 2.5, -4.0));
        assert_eq!(c.look, Vec3::new(0.0, 0.5, 1.0));
        assert_eq!((c.pos_stiffness, c.rot_stiffness), ([7.0, 3.0], [4.5, 2.0]));
        assert_eq!(FollowCam::parse(xml, CameraView::Close), FollowCam::fallback(CameraView::Close));
    }
}
