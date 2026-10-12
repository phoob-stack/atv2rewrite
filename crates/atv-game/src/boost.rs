//! Speed boost: points from tricks fill a meter, and holding the boost
//! button spends it for extra drive, as in the original. The numbers are
//! Tricks.xml's `SpeedBoost`: a landed combo pays its points times
//! `TrickScoreMod` in (0x62040, through 0x1b830, which keeps the meter in
//! 0..1); boosting multiplies the drive by `BoostTorqueMod` and drains
//! `BoostTimeMod` a second (0x1b750); a crash loses `CrashLoss`. Holding
//! the button also opens the throttle fully (0x1a9a0).

use bevy::prelude::*;

use atv_formats::setup;
use atv_formats::GameSource;

#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct BoostRules {
    /// Drive multiplier while boosting.
    pub torque: f32,
    /// Meter spent per second of boosting.
    pub drain: f32,
    /// Meter gained per point banked.
    pub per_point: f32,
    /// Meter lost in a crash.
    pub crash_loss: f32,
}

impl Default for BoostRules {
    fn default() -> Self {
        Self { torque: 2.5, drain: 0.1, per_point: 0.0004, crash_loss: 0.2 }
    }
}

impl BoostRules {
    pub fn load(game: &GameSource) -> Self {
        let xml = game.read("/Setup/Tricks.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let d = Self::default();
        let entries = setup::entries(&xml);
        let Some(e) = entries.iter().find(|e| e.name == "SpeedBoost") else { return d };
        let num = |key: &str, default: f32| e.num(key).unwrap_or(default);
        Self {
            torque: num("BoostTorqueMod", d.torque),
            drain: num("BoostTimeMod", d.drain),
            per_point: num("TrickScoreMod", d.per_point),
            crash_loss: num("CrashLoss", d.crash_loss),
        }
    }
}

/// One rider's boost: player 1's is a resource, every other rider's is
/// kept with their racer.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct Boost {
    /// 0..1.
    pub meter: f32,
    /// Boosting this frame.
    pub on: bool,
}

impl Boost {
    pub fn add(&mut self, amount: f32) {
        self.meter = (self.meter + amount).clamp(0.0, 1.0);
    }

    /// One frame: `held`, the boost button; `points`, banked since the
    /// last frame. Returns the drive multiplier.
    pub fn step(&mut self, rules: &BoostRules, held: bool, points: u32, dt: f32) -> f32 {
        self.add(points as f32 * rules.per_point);
        self.on = held && self.meter > 0.0;
        if !self.on {
            return 1.0;
        }
        self.meter = (self.meter - rules.drain * dt).max(0.0);
        rules.torque
    }

    pub fn crashed(&mut self, rules: &BoostRules) {
        self.add(-rules.crash_loss);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tricks_fill_boosting_drains_crashes_lose() {
        let rules = BoostRules::default();
        let mut b = Boost::default();
        // Nothing to spend yet.
        assert_eq!(b.step(&rules, true, 0, 0.1), 1.0);
        assert!(!b.on);
        // A 1500-point combo fills 60%; more can't overfill it.
        b.step(&rules, false, 1500, 0.1);
        assert!((b.meter - 0.6).abs() < 1e-5);
        b.step(&rules, false, 5000, 0.1);
        assert_eq!(b.meter, 1.0);
        // Ten seconds of boost empties it.
        assert_eq!(b.step(&rules, true, 0, 1.0), 2.5);
        assert!((b.meter - 0.9).abs() < 1e-5);
        b.crashed(&rules);
        assert!((b.meter - 0.7).abs() < 1e-5);
        for _ in 0..8 {
            b.step(&rules, true, 0, 1.0);
        }
        assert_eq!(b.meter, 0.0);
        assert_eq!(b.step(&rules, true, 0, 0.1), 1.0);
        b.crashed(&rules);
        assert_eq!(b.meter, 0.0);
    }
}
