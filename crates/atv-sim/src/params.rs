//! Parser for the game's `.params` vehicle tuning files ("Dyne" physics).
//!
//! The files are whitespace-separated `KEY value...` lists with `//`
//! comments. Curves are a key followed by N numbers; `WHEEL n` starts a
//! per-wheel block. Each quad's file only overrides `Cars/Generic/Generic.params`.

use std::collections::HashMap;

/// Keys that belong to the `WHEEL n` block they follow. Every other key,
/// even one starting with WHEEL (WHEELIETORQUE0MPH after the last wheel
/// in the retail files), is a vehicle value.
const WHEEL_KEYS: [&str; 10] = [
    "WHEELOFFSET",
    "WHEELAXIS",
    "WHEELMASS",
    "WHEELRADIUS",
    "WHEELWIDTH",
    "WHEELMINPOS",
    "WHEELMAXPOS",
    "WHEELMAXVEL",
    "WHEELSTIFFNESS",
    "WHEELDAMPING",
];

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
                k if wheel.is_some() && WHEEL_KEYS.contains(&k) => {
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
        self.values
            .get(key)
            .and_then(|v| v.get(idx))
            .copied()
            .unwrap_or(default)
    }

    pub fn curve(&self, key: &str) -> Vec<f32> {
        self.values.get(key).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct WheelParams {
    /// Attachment point in the body frame (sim frame: right-handed, -Z forward).
    pub offset: [f32; 3],
    /// Attachment point as written in the file (game frame: left-handed, +Z forward).
    pub game_offset: [f32; 3],
    /// Suspension axis in the game body frame.
    pub axis: [f32; 3],
    pub mass: f32,
    pub radius: f32,
    /// Suspension travel along `axis` (negative = hanging).
    pub min_pos: f32,
    pub max_pos: f32,
    pub max_vel: f32,
    pub stiffness: f32,
    pub damping: f32,
    pub front: bool,
}

/// A response curve: `n` evenly spaced bins over `0..max_x`, scaled by `scale`.
/// Sampling matches the original: bin width is `max_x / n` and the last
/// segment is clamped rather than extrapolated.
#[derive(Debug, Clone, Default)]
pub struct Curve {
    pub bins: Vec<f32>,
    pub max_x: f32,
    pub scale: f32,
}

impl Curve {
    pub fn sample(&self, x: f32) -> f32 {
        let n = self.bins.len();
        if n == 0 {
            return self.scale;
        }
        if n < 2 {
            return self.bins[0] * self.scale;
        }
        let w = self.max_x / n as f32;
        let i = ((x / w) as i32).clamp(0, n as i32 - 2) as usize;
        let t = (x - i as f32 * w).clamp(0.0, w) / w;
        (self.bins[i] + (self.bins[i + 1] - self.bins[i]) * t) * self.scale
    }
}

#[derive(Debug, Clone)]
pub struct Gear {
    pub ratio: f32,
    pub up_revs: f32,
    pub down_revs: f32,
}

/// Typed view of the tuning values the simulation uses. Names follow the
/// `.params` keys; defaults are the engine's own built-in values for keys
/// that `Generic.params` doesn't set. A `[f32; 2]` pair is lerped by
/// surface grip: the first value on full grip, the second on none.
#[derive(Debug, Clone)]
pub struct VehicleParams {
    /// Tuning from the retail disc, which goes with the retail executable's
    /// handling code (see `docs/physics.md`); the prototype's files lack
    /// its ENGINEREVRATEREVVING key.
    pub retail: bool,
    pub mass: f32,
    pub friction: f32,
    pub resistance: [f32; 3],
    pub ang_resistance: [f32; 3],
    pub air_resistance: f32,
    pub dims: [f32; 3],
    pub wheelbase: f32,
    pub force_point: f32,
    pub in_air_grav_scale: f32,

    pub torque_curve: Curve,
    pub min_revs: f32,
    pub max_revs: f32,
    pub engine_friction: f32,
    /// The prototype's lerp rate toward the target revs; retail's revs
    /// slew limit in RPM per second while racing.
    pub rev_rate: f32,
    /// Retail's revs slew limit on the starting grid, RPM per second.
    pub rev_rate_revving: f32,
    /// Retail's turbo start: released with the revs in this fraction of
    /// the rev range, drive is scaled for `max_turbo_time` seconds.
    pub max_turbo_time: f32,
    pub min_turbo_rev_scale: f32,
    pub max_turbo_rev_scale: f32,
    pub drive_force_turbo_scale: f32,
    pub brake_curve: Curve,
    pub brake_min_vel: f32,
    pub handbrake_force_max: f32,
    pub gears: Vec<Gear>,
    pub gear_multiplier: f32,
    pub gear_change_time: f32,
    pub gear_wait_time: f32,

    pub cornering_curve: Curve,
    pub cornering_max: [f32; 2],
    pub ang_damping_curve: Curve,
    pub ang_damping_max: [f32; 2],
    pub side_curve: Curve,
    pub side_max: [f32; 2],
    pub steer_drag_scale: f32,
    pub max_steer_deg: f32,

    pub slide_scale_curve: Curve,
    pub torque_slide_scale: [f32; 2],
    pub side_force_slide_scale: [f32; 2],
    pub drive_force_slide_scale: [f32; 2],
    pub drive_force_spin_scale: [f32; 2],
    pub brake_force_slide_scale: [f32; 2],
    pub slide_scale_acc_modifier: [f32; 2],
    pub slide_handbrake_rate_curve: Curve,
    pub slide_handbrake_rate: [f32; 2],
    pub slide_handbrake_return_rate: [f32; 2],
    pub slide_brake_rate_curve: Curve,
    pub slide_brake_rate: [f32; 2],
    pub slide_brake_return_rate: [f32; 2],
    pub slide_steer_rate_curve: Curve,
    pub slide_steer_rate: [f32; 2],
    pub slide_steer_return_rate: [f32; 2],

    pub steer_rate_curve: Curve,
    pub steer_rate: [f32; 2],
    pub steer_return_rate: [f32; 2],
    pub acc_rate: f32,
    pub acc_return_rate: f32,
    pub brake_rate: f32,
    pub brake_return_rate: f32,
    pub wheels: Vec<WheelParams>,

    // ATV_BEGIN .. ATV_END
    pub const_mass_torque: f32,
    pub landing_stab_in_air_rate: f32,
    pub landing_stab_max: f32,
    pub landing_stab_decay_rate: f32,
    pub power_slide_boost: f32,
    pub wheel_lock_steer_scale: f32,
    pub front_wheel_lock_point: f32,
    pub rear_wheel_lock_point: f32,
    pub in_air_grav_up: f32,
    pub in_air_grav_down: f32,
    pub in_air_steer_vel: f32,
    pub in_air_steer_lean: f32,
    pub in_air_steer_lean_torque: f32,
    pub in_air_steer_lean_damping: f32,
    pub in_air_lean: f32,
    pub lean_rate: f32,
    pub lean_force: f32,
    pub wheelie_torque: f32,
    /// Retail: the wheelie torque scale at 0 and 100 mph.
    pub wheelie_torque_0mph: f32,
    pub wheelie_torque_100mph: f32,
    pub wheelie_return_torque: f32,
    pub wheelie_damping: f32,
    pub endo_torque: f32,
    pub endo_damping: f32,
    pub preload_0mph: f32,
    pub preload_100mph: f32,
    pub preload_grav_scale_0mph: f32,
    pub preload_grav_scale_100mph: f32,
    pub preload_rate: f32,
    pub preload_hold_time: f32,
    pub preload_fade_rate: f32,
    /// Retail: spin from releasing the pre-load, about the right axis
    /// (lean and pedals) and the up axis (steering).
    pub preload_vert_spin: f32,
    pub preload_horiz_spin: f32,
    /// Retail: rolls the quad back onto all four wheels when only one or
    /// two touch the ground.
    pub bicycle_torque: f32,
    pub bicycle_damping: f32,
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
        let g = |k: &str, d: f32| r.get(k, 0, d);
        let pair = |k: &str, a: f32, b: f32| [r.get(k, 0, a), r.get(k, 1, b)];
        let curve = |k: &str, max_key: &str, max_d: f32, scale: f32, default: &[f32]| Curve {
            bins: r.values.get(k).cloned().unwrap_or_else(|| default.to_vec()),
            max_x: g(max_key, max_d),
            scale,
        };
        let gears = r
            .values
            .get("GEARRATIOS")
            .map(|v| {
                v.chunks_exact(3)
                    .map(|c| Gear {
                        ratio: c[0],
                        up_revs: c[1],
                        down_revs: c[2],
                    })
                    .collect()
            })
            .unwrap_or_default();
        let wheels = r
            .wheels
            .iter()
            .map(|w| {
                let g = |k: &str, i: usize, d: f32| {
                    w.get(k).and_then(|v| v.get(i)).copied().unwrap_or(d)
                };
                let game_offset = [
                    g("WHEELOFFSET", 0, 0.0),
                    g("WHEELOFFSET", 1, 0.0),
                    g("WHEELOFFSET", 2, 0.0),
                ];
                WheelParams {
                    // Game data is left-handed: negate Z for the sim frame.
                    offset: [game_offset[0], game_offset[1], -game_offset[2]],
                    game_offset,
                    axis: [
                        g("WHEELAXIS", 0, 0.0),
                        g("WHEELAXIS", 1, 1.0),
                        g("WHEELAXIS", 2, 0.0),
                    ],
                    mass: g("WHEELMASS", 0, 0.1),
                    radius: g("WHEELRADIUS", 0, 0.35),
                    min_pos: g("WHEELMINPOS", 0, -1.5),
                    max_pos: g("WHEELMAXPOS", 0, 1.5),
                    max_vel: g("WHEELMAXVEL", 0, 8.0),
                    stiffness: g("WHEELSTIFFNESS", 0, 150.0),
                    damping: g("WHEELDAMPING", 0, 6.0),
                    front: game_offset[2] > 0.0,
                }
            })
            .collect();
        VehicleParams {
            retail: r.values.contains_key("ENGINEREVRATEREVVING"),
            mass: g("MASS", 1.0),
            friction: g("FRICTION", 1.0),
            resistance: [
                r.get("RESISTANCE", 0, 0.0),
                r.get("RESISTANCE", 1, 0.0),
                r.get("RESISTANCE", 2, 0.0),
            ],
            ang_resistance: [
                r.get("ANGRESISTANCE", 0, 0.1),
                r.get("ANGRESISTANCE", 1, 0.0),
                r.get("ANGRESISTANCE", 2, 0.1),
            ],
            air_resistance: g("AIRRESISTANCE", 1.0),
            dims: [g("WIDTH", 2.5), g("HEIGHT", 1.5), g("LENGTH", 3.0)],
            wheelbase: g("WHEELBASE", 1.0),
            force_point: g("FORCEPOINT", -0.25),
            in_air_grav_scale: r
                .values
                .get("AIRGRAVITYSCALE")
                .or(r.values.get("INAIRGRAVSCALE"))
                .and_then(|v| v.first().copied())
                .unwrap_or(1.3),

            torque_curve: curve(
                "ENGINETORQUECURVE",
                "ENGINEMAXREVS",
                13500.0,
                g("ENGINETORQUEMAX", 0.75),
                &[
                    0.0, 0.5, 0.55, 0.65, 0.75, 0.85, 0.9, 0.95, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
                ],
            ),
            min_revs: g("ENGINEMINREVS", 2000.0),
            max_revs: g("ENGINEMAXREVS", 13500.0),
            engine_friction: g("ENGINEFRICTION", 3.0),
            rev_rate: g("ENGINEREVRATENORMAL", 50.0),
            rev_rate_revving: g("ENGINEREVRATEREVVING", 0.0),
            max_turbo_time: g("MAXTURBOTIME", 0.0),
            min_turbo_rev_scale: g("MINTURBOREVSCALE", 0.0),
            max_turbo_rev_scale: g("MAXTURBOREVSCALE", 0.0),
            drive_force_turbo_scale: g("DRIVEFORCETURBOSCALE", 1.0),
            brake_curve: Curve {
                bins: r
                    .values
                    .get("BRAKECURVE")
                    .cloned()
                    .unwrap_or_else(|| vec![1.0; 5]),
                max_x: g("BRAKEVELMAX", 50.0) - g("BRAKEVELMIN", 0.0),
                scale: g("BRAKEFORCEMAX", 20.0),
            },
            brake_min_vel: g("BRAKEVELMIN", 0.0),
            handbrake_force_max: g("HANDBRAKEFORCEMAX", 0.0),
            gears,
            gear_multiplier: g("GEARMULTIPLIER", 23.0),
            gear_change_time: g("GEARCHANGETIME", 0.5),
            gear_wait_time: g("GEARWAITTIME", 0.0),

            cornering_curve: curve(
                "CORNERINGFORCECURVE",
                "CORNERINGFORCEVELMAX",
                100.0,
                1.0,
                &[0.0, 0.6, 1.0, 0.9, 0.8, 0.7, 0.6, 0.6, 0.5, 0.5, 0.5],
            ),
            cornering_max: pair("CORNERINGFORCEMAX", 30.0, 0.0),
            ang_damping_curve: curve(
                "ANGDAMPINGCURVE",
                "ANGDAMPINGVELMAX",
                100.0,
                1.0,
                &[1.0, 0.96, 0.92, 0.88, 0.84, 0.8, 0.76, 0.72, 0.68, 0.64],
            ),
            ang_damping_max: pair("ANGDAMPINGMAX", 13.0, 2.0),
            side_curve: curve(
                "SIDEFORCECURVE",
                "SIDEFORCEVELMAX",
                10.0,
                1.0,
                &[1.0, 0.95, 0.92, 0.89, 0.87, 0.85, 0.84, 0.83, 0.83, 0.83],
            ),
            side_max: pair("SIDEFORCEMAX", 50.0, 0.0),
            steer_drag_scale: g("STEERDRAGSCALE", 0.1),
            max_steer_deg: g("MAXSTEERANGLE", 30.0),

            slide_scale_curve: curve(
                "SLIDESCALECURVE",
                "SLIDESCALEVELMAX",
                90.0,
                1.0,
                &[0.0, 0.1, 0.2, 0.3, 0.5, 0.7, 0.9, 0.95, 1.0, 1.0],
            ),
            torque_slide_scale: pair("TORQUESLIDESCALE", 0.8, 1.0),
            side_force_slide_scale: pair("SIDEFORCESLIDESCALE", 0.4, 0.8),
            drive_force_slide_scale: pair("DRIVEFORCESLIDESCALE", 0.6, 0.9),
            drive_force_spin_scale: pair("DRIVEFORCESPINSCALE", 0.6, 0.9),
            brake_force_slide_scale: pair("BRAKEFORCESLIDESCALE", 0.6, 0.9),
            slide_scale_acc_modifier: pair("SLIDESCALEACCMODIFIER", 0.5, 0.05),
            slide_handbrake_rate_curve: curve(
                "SLIDEHANDBRAKERATECURVE",
                "SLIDEHANDBRAKERATEVELMAX",
                100.0,
                1.0,
                &[1.0],
            ),
            slide_handbrake_rate: pair("SLIDEHANDBRAKERATE", 4.0, 8.0),
            slide_handbrake_return_rate: pair("SLIDEHANDBRAKERETURNRATE", 3.0, 0.5),
            slide_brake_rate_curve: curve(
                "SLIDEBRAKERATECURVE",
                "SLIDEBRAKERATEVELMAX",
                100.0,
                1.0,
                &[1.0],
            ),
            slide_brake_rate: pair("SLIDEBRAKERATE", 1.0, 3.0),
            slide_brake_return_rate: pair("SLIDEBRAKERETURNRATE", 2.0, 0.5),
            slide_steer_rate_curve: curve(
                "SLIDESTEERRATECURVE",
                "SLIDESTEERRATEVELMAX",
                100.0,
                1.0,
                &[1.0],
            ),
            slide_steer_rate: pair("SLIDESTEERRATE", 0.05, 1.0),
            slide_steer_return_rate: pair("SLIDESTEERRETURNRATE", 2.0, 0.5),

            steer_rate_curve: curve(
                "STEERRATECURVE",
                "STEERRATEVELMAX",
                100.0,
                1.0,
                &[1.0, 0.95, 0.9, 0.8, 0.7, 0.6, 0.5, 0.45, 0.4, 0.37],
            ),
            steer_rate: pair("STEERRATE", 3.0, 8.0),
            steer_return_rate: pair("STEERRETURNRATE", 8.0, 8.0),
            acc_rate: g("ACCRATE", 0.75),
            acc_return_rate: g("ACCRETURNRATE", 2.0),
            brake_rate: g("BRAKERATE", 2.0),
            brake_return_rate: g("BRAKERETURNRATE", 4.0),
            wheels,

            const_mass_torque: g("CONSTMASSTORQUE", 0.0),
            landing_stab_in_air_rate: g("LANDINGSTABINAIRRATE", 0.0),
            landing_stab_max: g("LANDINGSTABMAX", 0.0),
            landing_stab_decay_rate: g("LANDINGSTABDECAYRATE", 0.0),
            power_slide_boost: g("POWERSLIDEBOOST", 0.0),
            wheel_lock_steer_scale: g("WHEELLOCKSTEERSCALE", 0.0),
            front_wheel_lock_point: g("FRONTWHEELLOCKPOINT", 1.0),
            rear_wheel_lock_point: g("REARWHEELLOCKPOINT", 1.0),
            in_air_grav_up: g("INAIRGRAVUPSCALE", 1.0),
            in_air_grav_down: g("INAIRGRAVDOWNSCALE", 1.0),
            in_air_steer_vel: g("INAIRSTEERVELSCALE", 0.0),
            in_air_steer_lean: g("INAIRSTEERLEANSCALE", 0.0),
            in_air_steer_lean_torque: g("INAIRSTEERLEANTORQUE", 0.0),
            in_air_steer_lean_damping: g("INAIRSTEERLEANDAMPING", 0.0),
            in_air_lean: g("INAIRLEANSCALE", 0.0),
            lean_rate: g("LEANRATE", 10.0),
            lean_force: g("LEANFORCE", 0.0),
            wheelie_torque: g("WHEELIETORQUE", 0.0),
            wheelie_torque_0mph: g("WHEELIETORQUE0MPH", 0.0),
            wheelie_torque_100mph: g("WHEELIETORQUE100MPH", 0.0),
            wheelie_return_torque: g("WHEELIERETURNTORQUE", 0.0),
            wheelie_damping: g("WHEELIEDAMPING", 0.0),
            endo_torque: g("ENDOTORQUE", 0.0),
            endo_damping: g("ENDODAMPING", 0.0),
            preload_0mph: g("PRELOAD0MPH", 0.0),
            preload_100mph: g("PRELOAD100MPH", 0.0),
            preload_grav_scale_0mph: g("PRELOADGRAVSCALE0MPH", 1.0),
            preload_grav_scale_100mph: g("PRELOADGRAVSCALE100MPH", 1.0),
            preload_rate: g("PRELOADRATE", 1.0),
            preload_hold_time: g("PRELOADHOLDTIME", 1.0),
            preload_fade_rate: g("PRELOADFADERATE", 1.0),
            preload_vert_spin: g("PRELOADVERTSPINMAG", 0.0),
            preload_horiz_spin: g("PRELOADHORIZSPINMAG", 0.0),
            bicycle_torque: g("BICYCLETORQUE", 0.0),
            bicycle_damping: g("BICYCLEDAMPING", 0.0),
        }
    }
}

/// Lerps a grip-dependent pair: full grip gives `p[0]`, no grip `p[1]`.
pub fn by_grip(p: [f32; 2], grip: f32) -> f32 {
    p[0] + (1.0 - grip) * (p[1] - p[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_curves_wheels_and_overrides() {
        let generic = "MASS 1.0 // c\nENGINEMAXREVS 10000\nNENGINEBINS 3\nENGINETORQUECURVE\n 0.1\n 0.5\n 1.0\nENGINETORQUEMAX 0.30\n\
                       GEARRATIOS\n -2.2 7000 0\n 2.8 7000 0\nWHEEL 0\nWHEELOFFSET -0.5 -0.17 0.58\nWHEELRADIUS 0.25\n\
                       WHEEL 1\nWHEELOFFSET 0.5 -0.17 0.58\nEND\nATV_BEGIN\nCONSTMASSTORQUE -2.0\nATV_END";
        let quad = "ENGINETORQUEMAX 0.26\n";
        let p = VehicleParams::from_files(generic, Some(quad));
        assert_eq!(p.torque_curve.bins, vec![0.1, 0.5, 1.0]);
        assert_eq!(p.torque_curve.scale, 0.26);
        assert_eq!(p.gears.len(), 2);
        assert_eq!(p.wheels.len(), 2);
        assert_eq!(p.wheels[0].offset, [-0.5, -0.17, -0.58]);
        assert!(p.wheels[0].front);
        assert_eq!(p.const_mass_torque, -2.0);
        assert!(!p.retail);
        // Bins are max_x / n wide: 0.1 at 0, 0.5 at 3333, 1.0 at 6667 and beyond.
        assert!((p.torque_curve.sample(5000.0) / 0.26 - 0.75).abs() < 1e-5);
        assert!((p.torque_curve.sample(20000.0) / 0.26 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn atv_keys_after_the_last_wheel() {
        let text = "ENGINEREVRATEREVVING 9000\nWHEEL 0\nWHEELRADIUS 0.25\nEND\nATV_BEGIN\n\
                    WHEELIETORQUE0MPH 0.7\nWHEELIETORQUE100MPH 4.0\nWHEELIERETURNTORQUE 8.0\nATV_END";
        let p = VehicleParams::from_files(text, None);
        assert!(p.retail);
        assert_eq!(p.wheels[0].radius, 0.25);
        assert_eq!(p.wheelie_torque_0mph, 0.7);
        assert_eq!(p.wheelie_torque_100mph, 4.0);
        assert_eq!(p.wheelie_return_torque, 8.0);
    }
}
