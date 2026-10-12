//! The player's own tuning on top of the game's `.params` files, set in the
//! launcher's Config tab and kept in `tuning.cfg` in the config folder. The
//! game's files are never changed.
//!
//! Two kinds of setting: percentages that scale every quad ("Physics"), and
//! per-quad values that replace a `.params` key ("Bandit.MASS = 1.2").
//! Only settings that differ from the game are written.

use std::collections::BTreeMap;

use crate::params::{RawParams, VehicleParams};

/// The file name, in the same config folder as `options.cfg`.
pub const FILE: &str = "tuning.cfg";

/// A percentage that scales one thing on every quad.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    /// Slider range in percent.
    pub min: f32,
    pub max: f32,
    apply: fn(&mut VehicleParams, f32),
}

pub const SCALES: [Scale; 8] = [
    Scale {
        key: "gravity",
        label: "Gravity",
        hint: "The world's pull, on the ground and in the air (the engine's 25 m/s²). Less also means less grip.",
        min: 20.0,
        max: 200.0,
        apply: |p, s| p.gravity *= s,
    },
    Scale {
        key: "air_gravity",
        label: "Air gravity",
        hint: "Only while airborne. Lower is floatier jumps that go further.",
        min: 20.0,
        max: 200.0,
        apply: |p, s| p.in_air_grav_scale *= s,
    },
    Scale {
        key: "power",
        label: "Engine power",
        hint: "Every quad's engine torque.",
        min: 25.0,
        max: 300.0,
        apply: |p, s| p.torque_curve.scale *= s,
    },
    Scale {
        key: "grip",
        label: "Grip",
        hint: "Cornering and sideways grip.",
        min: 25.0,
        max: 300.0,
        apply: |p, s| {
            p.cornering_max[0] *= s;
            p.cornering_max[1] *= s;
            p.side_max[0] *= s;
            p.side_max[1] *= s;
        },
    },
    Scale {
        key: "brakes",
        label: "Brakes",
        hint: "Brake and handbrake force.",
        min: 25.0,
        max: 300.0,
        apply: |p, s| {
            p.brake_curve.scale *= s;
            p.handbrake_force_max *= s;
        },
    },
    Scale {
        key: "steering",
        label: "Steering lock",
        hint: "How far the bars turn.",
        min: 25.0,
        max: 200.0,
        apply: |p, s| p.max_steer_deg *= s,
    },
    Scale {
        key: "air_drag",
        label: "Air drag",
        hint: "Lower lets every quad reach a higher top speed.",
        min: 0.0,
        max: 300.0,
        apply: |p, s| p.air_resistance *= s,
    },
    Scale {
        key: "preload",
        label: "Pre-load jump",
        hint: "The hop from holding and releasing the pre-load.",
        min: 0.0,
        max: 300.0,
        apply: |p, s| {
            p.preload_0mph *= s;
            p.preload_100mph *= s;
        },
    },
];

/// A per-quad value that replaces one in the quad's `.params`.
#[derive(Clone, Copy, Debug)]
pub struct Setting {
    /// The `.params` key; `FRONT.`/`REAR.` before a wheel key sets it on
    /// those wheels.
    pub key: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    /// Drag step and fewest sensible digits.
    pub speed: f32,
    pub decimals: usize,
    /// The value the game uses, read back from its parsed tuning.
    get: fn(&VehicleParams) -> f32,
}

impl Setting {
    pub fn game_value(&self, p: &VehicleParams) -> f32 {
        (self.get)(p)
    }
}

/// Per-quad settings in groups, as the Config tab lists them.
pub const GROUPS: [(&str, &[Setting]); 4] = [
    (
        "Engine",
        &[
            Setting { key: "ENGINETORQUEMAX", label: "Torque", unit: "", speed: 0.005, decimals: 3, get: |p| p.torque_curve.scale },
            Setting { key: "ENGINEMAXREVS", label: "Rev limit", unit: "rpm", speed: 50.0, decimals: 0, get: |p| p.max_revs },
            Setting { key: "ENGINEMINREVS", label: "Idle revs", unit: "rpm", speed: 50.0, decimals: 0, get: |p| p.min_revs },
            Setting { key: "ENGINEFRICTION", label: "Engine braking", unit: "", speed: 0.05, decimals: 2, get: |p| p.engine_friction },
            Setting { key: "GEARMULTIPLIER", label: "Final drive", unit: "", speed: 0.1, decimals: 1, get: |p| p.gear_multiplier },
            Setting { key: "GEARCHANGETIME", label: "Gear change time", unit: "s", speed: 0.01, decimals: 2, get: |p| p.gear_change_time },
        ],
    ),
    (
        "Handling",
        &[
            Setting { key: "MASS", label: "Mass", unit: "", speed: 0.01, decimals: 2, get: |p| p.mass },
            Setting { key: "MAXSTEERANGLE", label: "Steering lock", unit: "°", speed: 0.5, decimals: 1, get: |p| p.max_steer_deg },
            Setting { key: "CORNERINGFORCEMAX", label: "Cornering force", unit: "", speed: 0.5, decimals: 1, get: |p| p.cornering_max[0] },
            Setting { key: "SIDEFORCEMAX", label: "Side grip", unit: "", speed: 0.5, decimals: 1, get: |p| p.side_max[0] },
            Setting { key: "BRAKEFORCEMAX", label: "Brake force", unit: "", speed: 0.5, decimals: 1, get: |p| p.brake_curve.scale },
            Setting { key: "AIRRESISTANCE", label: "Air drag", unit: "", speed: 0.05, decimals: 2, get: |p| p.air_resistance },
        ],
    ),
    (
        "Jumps and air",
        &[
            Setting { key: "AIRGRAVITYSCALE", label: "Air gravity", unit: "×", speed: 0.01, decimals: 2, get: |p| p.in_air_grav_scale },
            Setting { key: "INAIRGRAVUPSCALE", label: "Rising gravity", unit: "×", speed: 0.05, decimals: 2, get: |p| p.in_air_grav_up },
            Setting { key: "INAIRGRAVDOWNSCALE", label: "Falling gravity", unit: "×", speed: 0.05, decimals: 2, get: |p| p.in_air_grav_down },
            Setting { key: "PRELOAD0MPH", label: "Pre-load at 0 mph", unit: "", speed: 5.0, decimals: 0, get: |p| p.preload_0mph },
            Setting { key: "PRELOAD100MPH", label: "Pre-load at 100 mph", unit: "", speed: 5.0, decimals: 0, get: |p| p.preload_100mph },
            Setting { key: "INAIRLEANSCALE", label: "Air lean", unit: "", speed: 0.1, decimals: 2, get: |p| p.in_air_lean },
        ],
    ),
    (
        "Suspension",
        &[
            Setting { key: "FRONT.WHEELSTIFFNESS", label: "Front stiffness", unit: "", speed: 1.0, decimals: 1, get: |p| wheel(p, true, |w| w.stiffness) },
            Setting { key: "FRONT.WHEELDAMPING", label: "Front damping", unit: "", speed: 0.1, decimals: 2, get: |p| wheel(p, true, |w| w.damping) },
            Setting { key: "REAR.WHEELSTIFFNESS", label: "Rear stiffness", unit: "", speed: 1.0, decimals: 1, get: |p| wheel(p, false, |w| w.stiffness) },
            Setting { key: "REAR.WHEELDAMPING", label: "Rear damping", unit: "", speed: 0.1, decimals: 2, get: |p| wheel(p, false, |w| w.damping) },
        ],
    ),
];

fn wheel(p: &VehicleParams, front: bool, f: fn(&crate::params::WheelParams) -> f32) -> f32 {
    p.wheels.iter().find(|w| w.front == front).map_or(0.0, f)
}

/// Every per-quad setting.
pub fn settings() -> impl Iterator<Item = &'static Setting> {
    GROUPS.iter().flat_map(|(_, s)| s.iter())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tuning {
    /// Percentages by `Scale::key`; a missing key is 100.
    pub scales: BTreeMap<String, f32>,
    /// Quad name -> `Setting::key` -> value.
    pub quads: BTreeMap<String, BTreeMap<String, f32>>,
    /// Computer riders get the tuning too (otherwise only the players).
    pub computer_riders: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Self { scales: BTreeMap::new(), quads: BTreeMap::new(), computer_riders: false }
    }
}

impl Tuning {
    pub fn parse(text: &str) -> Self {
        let mut t = Self::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("");
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            if k == "computer_riders" {
                t.computer_riders = v == "true";
                continue;
            }
            let Ok(n) = v.parse::<f32>() else { continue };
            if !n.is_finite() {
                continue;
            }
            if let Some(s) = SCALES.iter().find(|s| s.key == k) {
                t.set_scale(s.key, n.clamp(s.min, s.max));
            } else if let Some((quad, key)) = k.split_once('.')
                && settings().any(|s| s.key.eq_ignore_ascii_case(key))
            {
                t.quads.entry(quad.to_string()).or_default().insert(key.to_ascii_uppercase(), n);
            }
        }
        t
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from("# Tuning from the launcher's Config tab. Delete this file to play with the game's own.\n");
        out += &format!("computer_riders = {}\n", self.computer_riders);
        for (k, v) in &self.scales {
            out += &format!("{k} = {v}\n");
        }
        for (quad, keys) in &self.quads {
            for (k, v) in keys {
                out += &format!("{quad}.{k} = {v}\n");
            }
        }
        out
    }

    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_text())
    }

    /// The percentage for a `Scale::key` (100 = the game's).
    pub fn scale(&self, key: &str) -> f32 {
        self.scales.get(key).copied().unwrap_or(100.0)
    }

    pub fn set_scale(&mut self, key: &str, percent: f32) {
        if (percent - 100.0).abs() < 0.05 {
            self.scales.remove(key);
        } else {
            self.scales.insert(key.to_string(), percent);
        }
    }

    pub fn quad_value(&self, quad: &str, key: &str) -> Option<f32> {
        self.quad(quad).and_then(|q| q.get(key)).copied()
    }

    /// Sets a quad's value; `None` goes back to the game's.
    pub fn set_quad_value(&mut self, quad: &str, key: &str, value: Option<f32>) {
        let name = self.quads.keys().find(|q| q.eq_ignore_ascii_case(quad)).cloned().unwrap_or_else(|| quad.to_string());
        let keys = self.quads.entry(name.clone()).or_default();
        match value {
            Some(v) => _ = keys.insert(key.to_string(), v),
            None => _ = keys.remove(key),
        }
        if keys.is_empty() {
            self.quads.remove(&name);
        }
    }

    fn quad(&self, quad: &str) -> Option<&BTreeMap<String, f32>> {
        self.quads.iter().find(|(q, _)| q.eq_ignore_ascii_case(quad)).map(|(_, k)| k)
    }

    /// Nothing differs from the game.
    pub fn is_empty(&self) -> bool {
        self.scales.is_empty() && self.quads.is_empty()
    }

    /// A quad's handling from the game's files with this tuning on top.
    pub fn params(&self, quad: &str, generic: &str, own: Option<&str>) -> VehicleParams {
        let mut raw = RawParams::parse(generic);
        if let Some(q) = own {
            raw = raw.overlay(&RawParams::parse(q));
        }
        if let Some(keys) = self.quad(quad) {
            for (k, &v) in keys {
                set_raw(&mut raw, k, v);
            }
        }
        let mut p = VehicleParams::from_raw(&raw);
        for s in &SCALES {
            let pc = self.scale(s.key);
            if pc != 100.0 {
                (s.apply)(&mut p, pc / 100.0);
            }
        }
        p
    }
}

/// Puts a setting's value in the parsed file: the first number of a key, or
/// a wheel key on the front or rear wheels.
fn set_raw(raw: &mut RawParams, key: &str, v: f32) {
    let wheels = |raw: &mut RawParams, front: bool, k: &str| {
        for w in &mut raw.wheels {
            let z = w.get("WHEELOFFSET").and_then(|o| o.get(2)).copied().unwrap_or(0.0);
            if (z > 0.0) == front {
                set_first(w.entry(k.to_string()).or_default(), v);
            }
        }
    };
    match key.split_once('.') {
        Some(("FRONT", k)) => wheels(raw, true, k),
        Some(("REAR", k)) => wheels(raw, false, k),
        _ => {
            if key == "AIRGRAVITYSCALE" {
                // The game reads either name; this one wins.
                raw.values.remove("INAIRGRAVSCALE");
            }
            set_first(raw.values.entry(key.to_string()).or_default(), v);
        }
    }
}

fn set_first(values: &mut Vec<f32>, v: f32) {
    match values.first_mut() {
        Some(x) => *x = v,
        None => values.push(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERIC: &str = "MASS 1.0\nENGINETORQUEMAX 0.3\nINAIRGRAVSCALE 1.3\nCORNERINGFORCEMAX 30 5\n\
                           WHEEL 0\nWHEELOFFSET -0.5 0 0.6\nWHEELSTIFFNESS 150\nWHEEL 1\nWHEELOFFSET -0.5 0 -0.6\nWHEELSTIFFNESS 120\n";

    #[test]
    fn untuned_is_the_game() {
        let t = Tuning::default();
        let p = t.params("Bandit", GENERIC, Some("MASS 1.2"));
        assert_eq!(p.mass, 1.2);
        assert_eq!(p.gravity, crate::vehicle::GRAVITY);
        assert_eq!(p.in_air_grav_scale, 1.3);
    }

    #[test]
    fn scales_and_quad_values() {
        let mut t = Tuning::default();
        t.set_scale("gravity", 50.0);
        t.set_scale("grip", 200.0);
        t.set_quad_value("Bandit", "MASS", Some(2.0));
        t.set_quad_value("Bandit", "AIRGRAVITYSCALE", Some(1.0));
        t.set_quad_value("Bandit", "REAR.WHEELSTIFFNESS", Some(90.0));
        let p = t.params("bandit", GENERIC, None);
        assert_eq!(p.gravity, 12.5);
        assert_eq!(p.mass, 2.0);
        assert_eq!(p.in_air_grav_scale, 1.0);
        assert_eq!(p.cornering_max, [60.0, 10.0]);
        assert_eq!(p.wheels[0].stiffness, 150.0);
        assert_eq!(p.wheels[1].stiffness, 90.0);
        // Other quads only get the scales.
        assert_eq!(t.params("Storm", GENERIC, None).mass, 1.0);
    }

    #[test]
    fn text_round_trips() {
        let mut t = Tuning { computer_riders: true, ..Default::default() };
        t.set_scale("air_gravity", 60.0);
        t.set_quad_value("Nitro", "ENGINEMAXREVS", Some(12000.0));
        let back = Tuning::parse(&t.to_text());
        assert_eq!(back, t);
        // Going back to the game's value drops the line.
        t.set_scale("air_gravity", 100.0);
        t.set_quad_value("Nitro", "ENGINEMAXREVS", None);
        assert!(t.is_empty());
        assert!(!t.to_text().contains("Nitro"));
    }

    #[test]
    fn gravity_settings_change_the_fall() {
        use crate::{Input, TrackCollision, Vehicle};
        use glam::{Quat, Vec3};
        let world = TrackCollision::new(Vec::new(), 10.0);
        let drop = |t: &Tuning| {
            let mut v = Vehicle::new(t.params("Q", GENERIC, None), Vec3::new(0.0, 100.0, 0.0), Quat::IDENTITY);
            for _ in 0..30 {
                v.step(&Input::default(), 1.0 / 60.0, &world);
            }
            100.0 - v.position.y
        };
        let game = drop(&Tuning::default());
        let mut t = Tuning::default();
        t.set_scale("gravity", 50.0);
        assert!((drop(&t) / game - 0.5).abs() < 0.02, "{} vs {game}", drop(&t));
        let mut t = Tuning::default();
        t.set_scale("air_gravity", 60.0);
        assert!((drop(&t) / game - 0.6).abs() < 0.02, "{} vs {game}", drop(&t));
    }

    #[test]
    fn every_setting_reads_back_what_it_sets() {
        let mut t = Tuning::default();
        for s in settings() {
            t.set_quad_value("Q", s.key, Some(7.0));
        }
        let p = t.params("Q", GENERIC, None);
        for s in settings() {
            assert_eq!(s.game_value(&p), 7.0, "{}", s.key);
        }
    }
}
