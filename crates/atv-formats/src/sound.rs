//! `Setup/Sound.xml`: mix levels, the wheel sound for each surface, and
//! each environment's music and ambience.
//!
//! Unlike the other setup files it nests: `<Root>` holds `<Surfaces>`,
//! `<Music>`, `<Ambient>` and `<Crowd>`, each with one element per
//! environment (`DefaultTrack` is the fallback, and environments only list
//! what differs from it).

use std::collections::HashMap;

use crate::setup::parse_attrs;

#[derive(Debug, Clone)]
pub struct SurfaceSound {
    /// Vehicles bank sample name (`Rumble4`, `Twigs2`...).
    pub sample: String,
    /// `Rumble`, `Boarding`, `Gravel`, `Screech` or `Water`.
    pub kind: String,
    pub volume: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Music {
    /// Played once before the sections (`MUSIC\DUNES0`).
    pub intro: Option<String>,
    /// Played in order, then from the first again.
    pub sections: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SoundSetup {
    /// Root attributes (`mPlayerEngineVol`, `mMusicVol`, `mEnginePitchZero`...).
    pub mix: HashMap<String, String>,
    /// The retail file's mix levels instead, `<Platforms><XBox>`
    /// (`mEngineVol`, `mEngineIdleVol`, `mSurfaceVol`...).
    platform: HashMap<String, String>,
    surfaces: HashMap<String, HashMap<String, SurfaceSound>>,
    music: HashMap<String, Music>,
    /// Environment -> (sample bank path, background count, volume: retail
    /// only).
    ambient: HashMap<String, (String, u32, Option<f32>)>,
    crowd: HashMap<String, String>,
}

const DEFAULT: &str = "DefaultTrack";

impl SoundSetup {
    pub fn parse(xml: &str) -> Self {
        let mut s = SoundSetup::default();
        let mut path: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(start) = rest.find('<') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('>') else { break };
            let tag = &rest[..end];
            rest = &rest[end + 1..];
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if tag.starts_with('/') {
                path.pop();
                continue;
            }
            let self_closing = tag.ends_with('/');
            let tag = tag.trim_end_matches('/');
            let (name, attr_text) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
            let a = parse_attrs(attr_text);
            let num = |k: &str, d: f32| a.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(d);
            let parents: Vec<&str> = path.iter().map(String::as_str).collect();
            match parents.as_slice() {
                [] => s.mix = a.clone(),
                [_, "Surfaces", env] => {
                    let sound = SurfaceSound {
                        sample: a.get("mWheelSample").cloned().unwrap_or_default(),
                        kind: a.get("mWheelSFXType").cloned().unwrap_or_default(),
                        volume: num("mVolume", 1.0),
                        pitch: num("mPitch", 1.0),
                    };
                    s.surfaces.entry(env.to_string()).or_default().insert(name.to_string(), sound);
                }
                [_, "Music", env] => {
                    let m = s.music.entry(env.to_string()).or_default();
                    if let Some(n) = a.get("name") {
                        if name == "IntroSection" {
                            m.intro = Some(n.clone());
                        } else {
                            m.sections.push(n.clone());
                        }
                    }
                }
                [_, "Ambient"] => {
                    let bank = a.get("m_ambientSamplebank").cloned().unwrap_or_default();
                    let volume = a.get("m_ambientVol").and_then(|v| v.trim().parse().ok());
                    s.ambient.insert(name.to_string(), (bank, num("m_noOfBackgrounds", 0.0) as u32, volume));
                }
                [_, "Platforms"] if name == "XBox" => s.platform = a.clone(),
                [_, "Crowd"] => {
                    if let Some(bank) = a.get("m_crowdSamplebank") {
                        s.crowd.insert(name.to_string(), bank.clone());
                    }
                }
                _ => {}
            }
            if !self_closing {
                path.push(name.to_string());
            }
        }
        s
    }

    /// A root mix level, e.g. `mix_level("mMusicVol", 1.0)`.
    pub fn mix_level(&self, key: &str, default: f32) -> f32 {
        self.mix.get(key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
    }

    /// The retail disc's file: its mix levels are per platform, and its
    /// banks were rebuilt to go with them.
    pub fn retail(&self) -> bool {
        !self.platform.is_empty()
    }

    /// An Xbox mix level from the retail file, e.g.
    /// `platform_level("mEngineVol", 1.0)`.
    pub fn platform_level(&self, key: &str, default: f32) -> f32 {
        self.platform.get(key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
    }

    /// The wheel sound for a surface in an environment, falling back to
    /// `DefaultTrack`.
    pub fn surface(&self, env: &str, surface: &str) -> Option<&SurfaceSound> {
        self.surfaces.get(env).and_then(|m| m.get(surface)).or_else(|| self.surfaces.get(DEFAULT)?.get(surface))
    }

    pub fn music(&self, env: &str) -> Option<&Music> {
        self.music.get(env).or_else(|| self.music.get(DEFAULT))
    }

    /// The environment's ambient bank, when it has a background loop.
    pub fn ambient(&self, env: &str) -> Option<&str> {
        self.ambient.get(env).filter(|(_, n, _)| *n > 0).map(|(b, ..)| b.as_str())
    }

    /// The environment's own ambience level (retail's `m_ambientVol`).
    pub fn ambient_volume(&self, env: &str) -> Option<f32> {
        self.ambient.get(env).and_then(|&(_, _, v)| v)
    }

    /// The crowd bank (Stadium only).
    pub fn crowd(&self, env: &str) -> Option<&str> {
        self.crowd.get(env).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<Root mMusicVol="0.5">
   <Surfaces>
      <DefaultTrack>
         <Mud mWheelSample="Rumble2" mWheelSFXType="Rumble" mVolume="0.7" mPitch="1.0"/>
         <Sand mWheelSample="Rumble4" mWheelSFXType="Rumble" mVolume="0.8" mPitch="1.4"/>
      </DefaultTrack>
      <Swamp>
         <Mud mWheelSample="Twigs2" mWheelSFXType="Rumble" mVolume="1.0" mPitch="1.0"/>
      </Swamp>
   </Surfaces>
   <Music>
      <DefaultTrack>
         <TrackSection name="MUSIC\SWAMP1"/>
      </DefaultTrack>
      <Dunes>
         <IntroSection name="MUSIC\DUNES0"/>
         <TrackSection name="MUSIC\DUNES1"/>
      </Dunes>
   </Music>
   <Ambient>
      <DefaultTrack m_ambientSamplebank="SOUND\FOREST" m_noOfBackgrounds="0"/>
      <Dunes m_ambientSamplebank="SOUND\DUNES" m_noOfBackgrounds="1" m_ambientVol="0.3"/>
   </Ambient>
   <Crowd>
      <Stadium m_crowdSamplebank="SOUND\STADIUM"/>
   </Crowd>
</Root>"#;

    #[test]
    fn nested_sections() {
        let s = SoundSetup::parse(XML);
        assert_eq!(s.mix_level("mMusicVol", 1.0), 0.5);
        assert_eq!(s.surface("Swamp", "Mud").unwrap().sample, "Twigs2");
        assert_eq!(s.surface("Swamp", "Sand").unwrap().pitch, 1.4);
        assert_eq!(s.surface("Dunes", "Mud").unwrap().sample, "Rumble2");
        let m = s.music("Dunes").unwrap();
        assert_eq!(m.intro.as_deref(), Some("MUSIC\\DUNES0"));
        assert_eq!(m.sections, ["MUSIC\\DUNES1"]);
        assert_eq!(s.music("Glacier").unwrap().sections, ["MUSIC\\SWAMP1"]);
        assert_eq!(s.ambient("Dunes"), Some("SOUND\\DUNES"));
        assert_eq!(s.ambient("Stadium"), None);
        assert_eq!(s.crowd("Stadium"), Some("SOUND\\STADIUM"));
        assert_eq!((s.ambient_volume("Dunes"), s.ambient_volume("Swamp")), (Some(0.3), None));
        assert!(!s.retail());
    }

    #[test]
    fn retail_platform_levels() {
        let s = SoundSetup::parse(
            r#"<Root FileVersion="1.0">
   <Platforms>
      <XBox mEngineVol="0.8" mEngineIdleVol="0.35"/>
      <PS2 mEngineVol="0.55"/>
   </Platforms>
</Root>"#,
        );
        assert!(s.retail());
        assert_eq!(s.platform_level("mEngineVol", 1.0), 0.8);
        assert_eq!(s.platform_level("mSurfaceVol", 1.0), 1.0);
    }
}
