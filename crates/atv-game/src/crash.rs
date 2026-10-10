//! Crashes: when the player's quad lands on its back or side, or hits
//! something hard, the rider is thrown off, tumbles through a crash clip,
//! lies there a moment and gets up, and the quad is put back on the track.
//!
//! The rules follow the original (crash start at 0x28ce0, its checks at
//! 0x1a5f0, the after-crash clips at 0x2bbb0):
//! - A body contact whose approach speed, scaled by how far the quad is
//!   from upright against the surface, passes 5 (10 with three or more
//!   wheels down) is a crash.
//! - Tipped past 75 degrees (up.y < 0.25), the quad crashes at once if its
//!   body touches, or after a few seconds with a wheel or less down.
//! - The clip follows which way the quad went over: nose first, over the
//!   back, or onto a side (mirrored for the right side).

use std::sync::Arc;

use bevy::prelude::*;

use atv_formats::GameSource;
use atv_sim::{TrackCollision, Vehicle};

use crate::rider::{Clip, Rider, ONE_SHOT_FPS};

const DIR: &str = "/Animations/Crashes/Bac/";

/// The crash clips, loaded with the race (and right-side mirror images of
/// the side falls).
#[derive(Resource)]
pub struct CrashClips {
    clips: Vec<(&'static str, Arc<Clip>)>,
}

/// Every clip a crash can use.
const NAMES: [&str; 13] = [
    "head_over_heels",
    "crash_front_1",
    "fall_front_endo",
    "fall_wheelie",
    "fall_high_wheelie",
    "fall_side_A",
    "fall_onto_back",
    "movement_on_back_A",
    "move_on_back_B",
    "push_up",
    "push_up_B",
    "push_up_C",
    "reach_out",
];

impl CrashClips {
    pub fn load(game: &GameSource, mirror_map: &[usize]) -> Self {
        let mut clips = Vec::new();
        for name in NAMES {
            match Clip::load_from(game, &format!("{DIR}{name}.bac")) {
                Ok(c) => {
                    if name.starts_with("fall_side") || name == "fall_onto_back" {
                        let m: &'static str = if name == "fall_onto_back" { "fall_onto_back~" } else { "fall_side_A~" };
                        clips.push((m, Arc::new(c.mirrored(mirror_map))));
                    }
                    clips.push((name, Arc::new(c)));
                }
                Err(e) => warn!("crash clip {name}: {e:#}"),
            }
        }
        CrashClips { clips }
    }

    fn get(&self, name: &str) -> Option<Arc<Clip>> {
        self.clips.iter().find(|(n, _)| *n == name).map(|(_, c)| c.clone())
    }
}

/// How the quad went over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    Front,
    Back,
    /// Onto a side: true for the right.
    Side(bool),
}

/// Why it crashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// Lay over too far for too long.
    Rollover,
    /// Hit the ground or a wall with the body.
    Impact,
}

/// The crash clips to play, in order, for a crash.
pub fn clips_for(cause: Cause, way: Way, speed: f32, airborne: bool, pick: f32) -> Vec<&'static str> {
    let first = match (cause, way) {
        (Cause::Rollover, Way::Front) => "fall_front_endo",
        (Cause::Impact, Way::Front) if speed * speed <= 800.0 => "head_over_heels",
        (Cause::Impact, Way::Front) => "crash_front_1",
        (_, Way::Back) if airborne => "fall_high_wheelie",
        (_, Way::Back) => "fall_wheelie",
        (Cause::Rollover, Way::Side(right)) => if right { "fall_onto_back~" } else { "fall_onto_back" },
        (Cause::Impact, Way::Side(right)) => if right { "fall_side_A~" } else { "fall_side_A" },
    };
    // Getting up: rolling about on the back after going head over heels,
    // pushing up off the front after a face-first crash.
    let second = match first {
        "head_over_heels" => Some(if pick < 0.5 { "movement_on_back_A" } else { "move_on_back_B" }),
        "crash_front_1" | "fall_front_endo" => Some(match (pick * 4.0) as u32 {
            0 => "push_up",
            1 => "push_up_B",
            2 => "push_up_C",
            _ => "reach_out",
        }),
        _ => None,
    };
    std::iter::once(first).chain(second).collect()
}

/// Which way an orientation is going over (sim frame: forward is -Z).
pub fn way_over(v: &Vehicle, normal: Option<Vec3>) -> Way {
    let (a, b) = (v.forward().y, v.right().y);
    if a.abs().max(b.abs()) < 0.3
        && let Some(n) = normal
    {
        // Still upright: go by what was hit.
        let (f, r) = (v.forward().dot(n), v.right().dot(n));
        return if f < -0.5 {
            Way::Front
        } else if f > 0.5 {
            Way::Back
        } else {
            Way::Side(r < 0.0)
        };
    }
    if a.abs() >= b.abs() {
        if a < 0.0 { Way::Front } else { Way::Back }
    } else {
        // Right side up means lying on the left.
        Way::Side(b < 0.0)
    }
}

/// The player's crash state.
#[derive(Resource, Default)]
pub struct Wreck {
    rollover: f32,
    pub fall: Option<Fall>,
    /// Set on a hard landing, for the rider to pick up.
    pub landed: bool,
}

pub struct Fall {
    /// Where the rider's pelvis is, and how it's moving.
    pub body: Vec3,
    vel: Vec3,
    pub heading: Quat,
    clips: Vec<&'static str>,
    clip: usize,
    /// Seconds into the current clip.
    t: f32,
    /// Seconds spent lying still after the last clip.
    rest: f32,
    age: f32,
}

/// Chassis spheres (centre relative to the centre of mass, game frame,
/// radius), as the simulation's body collision has them.
const SPHERES: [([f32; 3], f32); 5] = [
    ([0.0, 0.05, 0.0], 0.22),
    ([0.0, 0.3, 0.75], 0.25),
    ([0.0, 0.7, 0.3], 0.25),
    ([0.0, 0.5, -0.35], 0.3),
    ([0.0, 0.4, -0.85], 0.25),
];
const BODY_RADIUS: f32 = 0.2;
const LIE_STILL: f32 = 1.0;
/// How hard the ground slows a sliding rider, m/s².
/// (More than gravity down any slope, so a rider comes to rest.)
const SLIDE_DECEL: f32 = 25.0;

impl Wreck {
    /// Checks the step just taken for a crash. `before` is the quad's
    /// velocity and spin going into it. Returns the cause and way over.
    pub fn check(&mut self, v: &Vehicle, before: (Vec3, Vec3), world: &TrackCollision, dt: f32) -> Option<(Cause, Way)> {
        let mut touching = false;
        let mut impact: Option<(f32, Vec3)> = None;
        for &(local, radius) in &SPHERES {
            let r = v.rotation * Vec3::new(local[0], local[1], -local[2]);
            let Some(hit) = world.sphere(v.position + r, radius + 0.03) else { continue };
            touching = true;
            let speed = -(before.0 + before.1.cross(r)).dot(hit.normal);
            if impact.is_none_or(|(s, _)| speed > s) {
                impact = Some((speed, hit.normal));
            }
        }
        let up = v.up();
        let wheels = v.wheels.iter().filter(|w| w.contact).count();
        if let Some((speed, n)) = impact {
            let limit = if wheels > 2 { 10.0 } else { 5.0 };
            let hard = speed.max(0.0) * (0.5 - 0.5 * up.dot(n)) * ((v.forward().dot(n) + 1.0) * 0.5 + 1.0);
            if hard > limit {
                self.rollover = 0.0;
                return Some((Cause::Impact, way_over(v, Some(n))));
            }
        }
        if up.y < 0.25 {
            if touching {
                self.rollover += 1.2;
            } else if wheels <= 1 {
                self.rollover += (0.5 - 0.5 * up.y) * dt;
            } else {
                self.rollover = 0.0;
            }
        } else {
            self.rollover = 0.0;
        }
        if self.rollover > 6.0 {
            self.rollover = 0.0;
            return Some((Cause::Rollover, way_over(v, None)));
        }
        None
    }

    /// Throws the rider off.
    pub fn start(&mut self, v: &Vehicle, cause: Cause, way: Way, pick: f32) {
        let speed = v.velocity.length();
        let clips = clips_for(cause, way, speed, !v.grounded(), pick);
        let fwd = v.forward().with_y(0.0).normalize_or(Vec3::NEG_Z);
        // The seated pelvis: above and a little behind the centre of mass.
        let body = v.position + v.rotation * Vec3::new(0.0, 1.02 - crate::convert::COM_HEIGHT, 0.29);
        self.fall = Some(Fall {
            body,
            vel: v.velocity + Vec3::Y * 2.0,
            heading: Quat::from_rotation_arc(Vec3::NEG_Z, fwd),
            clips,
            clip: 0,
            t: 0.0,
            rest: 0.0,
            age: 0.0,
        });
    }

    /// Moves the rider along. Returns true when the crash is over and the
    /// quad should be put back.
    pub fn advance(&mut self, clips: Option<&CrashClips>, world: &TrackCollision, dt: f32) -> bool {
        let Some(f) = &mut self.fall else { return false };
        f.age += dt;
        f.vel.y -= 9.81 * dt;
        f.body += f.vel * dt;
        // A little margin so a rider resting on the ground counts as on it.
        if let Some(hit) = world.sphere(f.body, BODY_RADIUS + 0.05) {
            f.body += hit.normal * (hit.t - 0.05).max(0.0);
            let vn = f.vel.dot(hit.normal);
            if vn < 0.0 {
                f.vel -= hit.normal * vn;
            }
            // Sliding to a stop.
            let vt = f.vel - hit.normal * f.vel.dot(hit.normal);
            let slow = (SLIDE_DECEL * dt).min(vt.length());
            f.vel -= vt.normalize_or_zero() * slow;
        }
        let frames = |name: &str| clips.and_then(|c| c.get(name)).map_or(1, |c| c.frames());
        f.t += dt;
        let len = (frames(f.clips[f.clip]) - 1) as f32 / ONE_SHOT_FPS;
        if f.t >= len {
            if f.clip + 1 < f.clips.len() && f.vel.length() < 2.0 {
                // Lie a moment, then get up.
                f.rest += dt;
                if f.rest > LIE_STILL * 0.5 {
                    f.clip += 1;
                    f.t = 0.0;
                    f.rest = 0.0;
                }
            } else if f.clip + 1 == f.clips.len() {
                f.t = len;
                f.rest += dt;
            }
        }
        let done = f.clip + 1 == f.clips.len() && f.rest > LIE_STILL;
        // However it lands, it's over after a while.
        done || f.age > 8.0
    }

    /// The crash clip and how far through it, for the rider.
    pub fn pose(&self, clips: &CrashClips) -> Option<(Arc<Clip>, f32)> {
        let f = self.fall.as_ref()?;
        let clip = clips.get(f.clips[f.clip])?;
        let len = (clip.frames() - 1).max(1) as f32 / ONE_SHOT_FPS;
        Some((clip, (f.t / len).clamp(0.0, 1.0)))
    }
}

/// The player's rider follows the crash, and plays a landing after a big
/// jump.
pub fn rider_follows(
    mut wreck: Option<ResMut<Wreck>>,
    clips: Option<Res<CrashClips>>,
    quad: Query<&Transform, (With<crate::QuadBody>, Without<Rider>)>,
    mut riders: Query<(&mut Rider, &mut Transform), Without<crate::OpponentRider>>,
) {
    let (Some(wreck), Some(clips)) = (wreck.as_mut(), clips) else { return };
    let landed = std::mem::take(&mut wreck.landed);
    let Ok(quad) = quad.single() else { return };
    for (mut rider, mut seat) in &mut riders {
        match (&wreck.fall, wreck.pose(&clips)) {
            (Some(f), Some(pose)) => {
                rider.set_crash(Some(pose));
                // The seat hangs off the quad: place it so the pelvis lands
                // on the body, facing the way the quad was going.
                let world = Transform::from_translation(f.body).with_rotation(f.heading);
                let local = quad.compute_affine().inverse() * world.compute_affine();
                *seat = Transform::from_matrix(Mat4::from(local));
            }
            _ => {
                if rider.crashed() {
                    rider.set_crash(None);
                    *seat = Transform::from_xyz(0.0, -crate::convert::COM_HEIGHT, 0.0);
                }
                if landed {
                    rider.land();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_choice() {
        assert_eq!(clips_for(Cause::Impact, Way::Front, 10.0, false, 0.1), ["head_over_heels", "movement_on_back_A"]);
        assert_eq!(clips_for(Cause::Impact, Way::Front, 40.0, false, 0.9), ["crash_front_1", "reach_out"]);
        assert_eq!(clips_for(Cause::Rollover, Way::Back, 0.0, true, 0.0), ["fall_high_wheelie"]);
        assert_eq!(clips_for(Cause::Impact, Way::Side(true), 5.0, false, 0.0), ["fall_side_A~"]);
        assert_eq!(clips_for(Cause::Rollover, Way::Side(false), 5.0, false, 0.0), ["fall_onto_back"]);
    }
}
