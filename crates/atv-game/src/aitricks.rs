//! Computer riders' tricks, from the original AI's jump states (0x635c0)
//! and its trick picker (0x63330). More than 1.75 m off the ground, a
//! computer rider predicts how long it will be in the air and throws a
//! random trick of its skill level that fills 30-80% of that time (the
//! lower bound no more than half the longest trick), then another while
//! the air lasts. A jump with nothing that fits gets no more tricks. The
//! tricks score through the rider's own [`Session`], so they count for
//! the career's trick points.

use bevy::prelude::*;

use atv_sim::{TrackCollision, Vehicle};

use crate::freestyle::Session;

/// Height above the ground (m) before a computer rider tries a trick.
const TRICK_HEIGHT: f32 = 1.75;
/// A trick takes at most this share of the air left, and at least
/// `MIN_SHARE` of that.
const MAX_SHARE: f32 = 0.8;
const MIN_SHARE: f32 = 0.3;

#[derive(Clone, Debug)]
pub struct AiTricks {
    /// Nothing fitted this jump: wait for the next.
    done: bool,
    seed: u32,
}

impl AiTricks {
    pub fn new(seed: u32) -> Self {
        Self { done: false, seed: seed.max(1) }
    }

    fn random(&mut self, n: usize) -> usize {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as usize % n.max(1)
    }

    /// Starts a trick when the moment is right.
    pub fn step(&mut self, s: &mut Session, v: &Vehicle, world: &TrackCollision) {
        if v.grounded() {
            self.done = false;
            return;
        }
        if self.done || s.active.is_some() {
            return;
        }
        let height = world.raycast(v.position, Vec3::NEG_Y, 50.0).map_or(f32::MAX, |h| v.position.y - h.point.y);
        if height <= TRICK_HEIGHT {
            return;
        }
        let picks = candidates(s, time_to_land(v, world));
        if picks.is_empty() {
            self.done = true;
            return;
        }
        let def = picks[self.random(picks.len())];
        debug!("computer rider trick {} ({height:.1} m up at {:.1})", s.tricks[def].name, v.position);
        s.start_trick(def);
    }
}

/// The tricks a rider of the session's level can fit into `air` seconds.
fn candidates(s: &Session, air: f32) -> Vec<usize> {
    let shortest = s.tricks.iter().map(|t| t.total).fold(f32::MAX, f32::min);
    let longest = s.tricks.iter().map(|t| t.total).fold(0.0, f32::max);
    if air < shortest {
        return Vec::new();
    }
    let most = air * MAX_SHARE;
    let least = (most * MIN_SHARE).min(longest * 0.5);
    (0..s.tricks.len()).filter(|&i| s.tricks[i].level <= s.level && s.tricks[i].total < most && s.tricks[i].total > least).collect()
}

/// Seconds until the quad comes down, flying on under the air gravity.
fn time_to_land(v: &Vehicle, world: &TrackCollision) -> f32 {
    const STEP: f32 = 0.05;
    let p = &v.params;
    let (mut pos, mut vel) = (v.position, v.velocity);
    for i in 1..100 {
        let up = if vel.y > 0.0 { p.in_air_grav_up } else { p.in_air_grav_down };
        vel.y -= atv_sim::vehicle::GRAVITY * p.in_air_grav_scale * up * STEP;
        pos += vel * STEP;
        match world.raycast(pos + Vec3::Y * 2.0, Vec3::NEG_Y, 60.0) {
            Some(h) if pos.y <= h.point.y + 0.5 => return i as f32 * STEP,
            _ => {}
        }
    }
    100.0 * STEP
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freestyle::{Dir, TrickDef};

    fn trick(level: u8, total: f32) -> TrickDef {
        TrickDef { name: String::new(), level, buttons: 1, dir: Dir::Up, points: 100, hold_points: 20, total, end: 0.3, clip: String::new() }
    }

    #[test]
    fn picks_tricks_that_fit_the_air() {
        let mut s = Session::test_with(vec![trick(1, 0.56), trick(1, 1.0), trick(2, 1.0), trick(1, 2.4)]);
        s.level = 1;
        // Too short a jump for anything.
        assert!(candidates(&s, 0.5).is_empty());
        // 1.5 s: up to 1.2 s, at least 0.36 s; level 2 is out.
        assert_eq!(candidates(&s, 1.5), vec![0, 1]);
        // A long flight: the floor is half the longest trick.
        assert_eq!(candidates(&s, 10.0), vec![3]);
    }
}
