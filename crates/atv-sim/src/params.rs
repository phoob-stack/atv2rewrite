//! Parser for the game's `.params` vehicle tuning files ("Dyne" physics).
//!
//! The files are whitespace-separated `KEY value...` lists with `//`
//! comments. Curves are a key followed by N numbers; `WHEEL n` starts a
//! per-wheel block. Each quad's file only overrides `Cars/Generic/Generic.params`.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct RawParams {
    pub values: HashMap<String, Vec<f32>>,
    pub wheels: Vec<HashMap<String, Vec<f32>>>,
}

impl RawParams {
    pub fn parse(text: &str) -> Self {
        let mut out = RawParams::default();
        let mut tokens = Vec::new();
        for line in text.lines() {
            let line = line.split("//").next().unwrap_or("");
            tokens.extend(line.split_whitespace().map(str::to_owned));
        }
        let mut wheel: Option<usize> = None;
        let mut i = 0;
        while i < tokens.len() {
            let key = tokens[i].to_ascii_uppercase();
            i += 1;
            let mut nums = Vec::new();
            while i < tokens.len() {
                match tokens[i].parse::<f32>() {
                    Ok(v) => nums.push(v),
                    Err(_) => break,
                }
                i += 1;
            }
            if key.parse::<f32>().is_ok() {
                continue;
            }
            match key.as_str() {
                "WHEEL" => {
                    let n = nums.first().copied().unwrap_or(0.0) as usize;
                    if out.wheels.len() <= n {
                        out.wheels.resize(n + 1, HashMap::new());
                    }
                    wheel = Some(n);
                }
                k if k.starts_with("WHEEL") && wheel.is_some() && k != "WHEELSPINDIFF" && k != "WHEELSPINREVSCALE" && k != "WHEELLOCKSTEERSCALE" && k != "WHEELIETORQUE" && k != "WHEELIERETURNTORQUE" && k != "WHEELIEDAMPING" => {
                    out.wheels[wheel.unwrap()].insert(key, nums);
                }
                _ => {
                    out.values.insert(key, nums);
                }
            }
        }
        out
    }

    /// Applies `other` on top of `self` (quad-specific overrides).
    pub fn overlay(mut self, other: &RawParams) -> Self {
        for (k, v) in &other.values {
            self.values.insert(k.clone(), v.clone());
        }
        for (i, w) in other.wheels.iter().enumerate() {
            if self.wheels.len() <= i {
                self.wheels.resize(i + 1, HashMap::new());
            }
            for (k, v) in w {
                self.wheels[i].insert(k.clone(), v.clone());
            }
        }
        self
    }

    pub fn get(&self, key: &str, idx: usize, default: f32) -> f32 {
        self.values.get(key).and_then(|v| v.get(idx)).copied().unwrap_or(default)
    }

    pub fn curve(&self, key: &str) -> Vec<f32> {
        self.values.get(key).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct WheelParams {
    /// Attachment point in the body frame (right-handed, metres).
    pub offset: [f32; 3],
    pub radius: f32,
    /// Suspension travel limits relative to the offset (down is negative).
    pub min_pos: f32,
    pub max_pos: f32,
    pub stiffness: f32,
    pub damping: f32,
    pub front: bool,
}

#[derive(Debug, Clone)]
pub struct Gear {
    pub ratio: f32,
    pub up_revs: f32,
    pub down_revs: f32,
}

/// Typed view of the parameters the simulation uses.
#[derive(Debug, Clone)]
pub struct VehicleParams {
    pub mass: f32,
    pub friction: f32,
    pub ang_resistance: [f32; 3],
    pub air_resistance: f32,
    pub dims: [f32; 3],
    pub torque_curve: Vec<f32>,
    pub torque_max: f32,
    pub min_revs: f32,
    pub max_revs: f32,
    pub brake_force_max: f32,
    pub gears: Vec<Gear>,
    pub gear_multiplier: f32,
    pub cornering_curve: Vec<f32>,
    pub cornering_max: [f32; 2],
    pub cornering_vel_max: f32,
    pub side_curve: Vec<f32>,
    pub side_max: [f32; 2],
    pub side_vel_max: f32,
    pub max_steer_deg: f32,
    pub steer_rate: f32,
    pub steer_return_rate: f32,
    pub acc_rate: f32,
    pub acc_return_rate: f32,
    pub wheels: Vec<WheelParams>,
    pub const_mass_torque: f32,
    pub in_air_grav_up: f32,
    pub in_air_grav_down: f32,
    pub in_air_steer_vel: f32,
    pub in_air_lean: f32,
    pub preload_0mph: f32,
    pub preload_100mph: f32,
    pub preload_rate: f32,
    pub landing_stab_max: f32,
    /// Height (relative to the centre of mass) where tyre forces act.
    pub force_point: f32,
}

impl VehicleParams {
    pub fn from_files(generic: &str, quad: Option<&str>) -> Self {
        let mut raw = RawParams::parse(generic);
        if let Some(q) = quad {
            raw = raw.overlay(&RawParams::parse(q));
        }
        Self::from_raw(&raw)
    }

    pub fn from_raw(r: &RawParams) -> Self {
        let gears = r
            .values
            .get("GEARRATIOS")
            .map(|v| v.chunks_exact(3).map(|c| Gear { ratio: c[0], up_revs: c[1], down_revs: c[2] }).collect())
            .unwrap_or_default();
        let wheels = r
            .wheels
            .iter()
            .map(|w| {
                let g = |k: &str, i: usize, d: f32| w.get(k).and_then(|v| v.get(i)).copied().unwrap_or(d);
                // Game data is left-handed: negate Z.
                let offset = [g("WHEELOFFSET", 0, 0.0), g("WHEELOFFSET", 1, 0.0), -g("WHEELOFFSET", 2, 0.0)];
                WheelParams {
                    offset,
                    radius: g("WHEELRADIUS", 0, 0.25),
                    min_pos: g("WHEELMINPOS", 0, -0.3),
                    max_pos: g("WHEELMAXPOS", 0, 0.2),
                    stiffness: g("WHEELSTIFFNESS", 0, 35.0),
                    damping: g("WHEELDAMPING", 0, 3.0),
                    front: offset[2] < 0.0,
                }
            })
            .collect();
        VehicleParams {
            mass: r.get("MASS", 0, 1.0),
            friction: r.get("FRICTION", 0, 0.7),
            ang_resistance: [r.get("ANGRESISTANCE", 0, 2.0), r.get("ANGRESISTANCE", 1, 0.0), r.get("ANGRESISTANCE", 2, 2.0)],
            air_resistance: r.get("AIRRESISTANCE", 0, 5.0),
            dims: [r.get("WIDTH", 0, 1.5), r.get("HEIGHT", 0, 1.0), r.get("LENGTH", 0, 3.0)],
            torque_curve: r.curve("ENGINETORQUECURVE"),
            torque_max: r.get("ENGINETORQUEMAX", 0, 0.3),
            min_revs: r.get("ENGINEMINREVS", 0, 700.0),
            max_revs: r.get("ENGINEMAXREVS", 0, 10000.0),
            brake_force_max: r.get("BRAKEFORCEMAX", 0, 20.0),
            gears,
            gear_multiplier: r.get("GEARMULTIPLIER", 0, 38.0),
            cornering_curve: r.curve("CORNERINGFORCECURVE"),
            cornering_max: [r.get("CORNERINGFORCEMAX", 0, 40.0), r.get("CORNERINGFORCEMAX", 1, 10.0)],
            cornering_vel_max: r.get("CORNERINGFORCEVELMAX", 0, 85.0),
            side_curve: r.curve("SIDEFORCECURVE"),
            side_max: [r.get("SIDEFORCEMAX", 0, 30.0), r.get("SIDEFORCEMAX", 1, 17.0)],
            side_vel_max: r.get("SIDEFORCEVELMAX", 0, 70.0),
            max_steer_deg: r.get("MAXSTEERANGLE", 0, 35.0),
            steer_rate: r.get("STEERRATE", 0, 5.0),
            steer_return_rate: r.get("STEERRETURNRATE", 0, 5.0),
            acc_rate: r.get("ACCRATE", 0, 2.5),
            acc_return_rate: r.get("ACCRETURNRATE", 0, 1.5),
            wheels,
            const_mass_torque: r.get("CONSTMASSTORQUE", 0, -2.0),
            in_air_grav_up: r.get("INAIRGRAVUPSCALE", 0, 2.8),
            in_air_grav_down: r.get("INAIRGRAVDOWNSCALE", 0, 1.5),
            in_air_steer_vel: r.get("INAIRSTEERVELSCALE", 0, 5.0),
            in_air_lean: r.get("INAIRLEANSCALE", 0, 6.5),
            preload_0mph: r.get("PRELOAD0MPH", 0, 250.0),
            preload_100mph: r.get("PRELOAD100MPH", 0, 600.0),
            preload_rate: r.get("PRELOADRATE", 0, 1.0),
            landing_stab_max: r.get("LANDINGSTABMAX", 0, 20.0),
            force_point: r.get("FORCEPOINT", 0, -0.07),
        }
    }
}

/// Samples a curve stored as evenly spaced bins over 0..=max.
pub fn sample_curve(curve: &[f32], x: f32, max: f32) -> f32 {
    match curve.len() {
        0 => 1.0,
        1 => curve[0],
        n => {
            let t = (x / max).clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (t as usize).min(n - 2);
            let f = t - i as f32;
            curve[i] * (1.0 - f) + curve[i + 1] * f
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_curves_wheels_and_overrides() {
        let generic = "MASS 1.0 // c\nNENGINEBINS 3\nENGINETORQUECURVE\n 0.1\n 0.5\n 1.0\nENGINETORQUEMAX 0.30\n\
                       GEARRATIOS\n -2.2 7000 0\n 2.8 7000 0\nWHEEL 0\nWHEELOFFSET -0.5 -0.17 0.58\nWHEELRADIUS 0.25\n\
                       WHEEL 1\nWHEELOFFSET 0.5 -0.17 0.58\nEND\nATV_BEGIN\nCONSTMASSTORQUE -2.0\nATV_END";
        let quad = "ENGINETORQUEMAX 0.26\n";
        let p = VehicleParams::from_files(generic, Some(quad));
        assert_eq!(p.torque_curve, vec![0.1, 0.5, 1.0]);
        assert_eq!(p.torque_max, 0.26);
        assert_eq!(p.gears.len(), 2);
        assert_eq!(p.wheels.len(), 2);
        assert_eq!(p.wheels[0].offset, [-0.5, -0.17, -0.58]);
        assert!(p.wheels[0].front);
        assert_eq!(p.const_mass_torque, -2.0);
        assert!((sample_curve(&p.torque_curve, 0.75, 1.0) - 0.75).abs() < 1e-6);
    }
}
