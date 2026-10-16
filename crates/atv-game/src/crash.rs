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
//! - Coming down fast just over water, the rider braces (`Plunge`); with
//!   the quad's centre or the thrown rider a metre under the surface, the
//!   rider treads water until the quad is put back (0xd7f60, 0x2cca0,
//!   0x2ca40, 0x26e30).

use std::sync::Arc;

use bevy::prelude::*;

use atv_formats::GameSource;
use atv_sim::{TrackCollision, Vehicle};

use crate::rider::{Clip, Rider, ONE_SHOT_FPS};
use crate::water::{Water, UNDER};

const DIR: &str = "/Animations/Crashes/Bac/";

/// The crash clips, loaded with the race (and right-side mirror images of
/// the side falls).
#[derive(Resource)]
pub struct CrashClips {
    clips: Vec<(&'static str, Arc<Clip>)>,
}

/// Every clip a crash can use.
const NAMES: [&str; 15] = [
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
    "Plunge",
    "Tread_water",
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
    /// Went into deep water (the original's crash type 5).
    Water,
}

/// The crash clips to play, in order, for a crash.
pub fn clips_for(cause: Cause, way: Way, speed: f32, airborne: bool, pick: f32) -> Vec<&'static str> {
    let first = match (cause, way) {
        (Cause::Water, _) => "Tread_water",
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
    /// The hardest the chassis hit the world since the sound last looked
    /// (closing speed, m/s).
    pub knock: f32,
    /// The hardest a thrown rider's body hit the world since the sound
    /// last looked, and how fast it was going.
    pub thump: Option<(f32, f32)>,
    /// Seconds since the seated rider began bracing for the water.
    plunge: Option<f32>,
    /// What the rider did in the water since the effects last looked.
    pub water_fx: Vec<WaterFx>,
}

/// Something in the water for the effects to show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaterFx {
    /// Something came down into the water: where it went in (on the
    /// surface), how far it moved in a 60 Hz frame, and how big it is.
    Splash { at: Vec3, motion: Vec3, radius: f32 },
    /// The rider treading water stirs it up (0x2bbb0).
    Stir(Vec3),
}

pub struct Fall {
    /// Where the rider's pelvis is, and how it's moving.
    pub body: Vec3,
    vel: Vec3,
    /// Where the pelvis was before the last step, to draw it in between.
    prev: Vec3,
    pub heading: Quat,
    clips: Vec<&'static str>,
    clip: usize,
    /// Seconds into the current clip.
    t: f32,
    /// Seconds spent lying still after the last clip.
    rest: f32,
    age: f32,
    /// Treading water.
    floating: bool,
    /// Seconds since the quad or the rider went into the water.
    wet: Option<f32>,
}

impl Fall {
    /// The pelvis drawn `between` of the way from the last step to the
    /// next (the fixed clock's overstep fraction), so it moves on every
    /// frame.
    pub fn drawn(&self, between: f32) -> Vec3 {
        self.prev.lerp(self.body, between.clamp(0.0, 1.0))
    }
}

const BODY_RADIUS: f32 = 0.2;
const LIE_STILL: f32 = 1.0;
/// How hard the ground slows a sliding rider, m/s².
/// (More than gravity down any slope, so a rider comes to rest.)
const SLIDE_DECEL: f32 = 25.0;
/// Coming down faster than this (0.08 m in a 60 Hz frame) less than
/// [`PLUNGE_HEIGHT`] over water, the seated rider braces (0xd7f60).
const PLUNGE_SPEED: f32 = 4.8;
const PLUNGE_HEIGHT: f32 = 3.0;
/// In the water (0x2ca40) the original pulls the rider's body up by
/// `FLOAT_PULL × (FLOAT_LEVEL − height over the surface − 0.5 × rising
/// speed)` and damps its motion at [`WATER_DRAG`] a second. (The
/// original measures from the water box's top and only pulls a body a
/// metre under it; here the body floats by the water you can see, held
/// down by [`FLOAT_GRAVITY`] so it sits 0.2 m under the surface.)
const FLOAT_PULL: f32 = 50.0;
const FLOAT_LEVEL: f32 = 0.3;
const WATER_DRAG: f32 = 25.0;
/// Not the game's gravity (9.81): the pull down that floats the body at
/// the depth chosen when water crashes were ported.
const FLOAT_GRAVITY: f32 = 25.0;
/// Seconds in the water before the original starts its respot (0x26e30);
/// the quad is put back after the respot's half-second fade (0x25f20).
const WET_TIME: f32 = 1.5;
/// Treading water plays frames 2-46 of `Tread_water` once, then 46-68
/// over and over, stirring the water each time round (0x2bbb0, state 6).
const TREAD_START: f32 = 2.0;
const TREAD_LOOP: (f32, f32) = (46.0, 68.0);
/// How big a rider is to the splash (the original's rider radius isn't
/// worked out).
const RIDER_RADIUS: f32 = 0.5;

/// The `Tread_water` frame `t` seconds in, and how many times the loop
/// has started.
fn tread_frame(t: f32) -> (f32, u32) {
    let f = t.max(0.0) * ONE_SHOT_FPS;
    let into = TREAD_LOOP.0 - TREAD_START;
    if f < into {
        return (TREAD_START + f, 0);
    }
    let len = TREAD_LOOP.1 - TREAD_LOOP.0;
    (TREAD_LOOP.0 + (f - into) % len, ((f - into) / len) as u32 + 1)
}

impl Wreck {
    /// Checks the step just taken for a crash. `before` is the quad's
    /// velocity and spin going into it. Returns the cause and way over.
    pub fn check(&mut self, v: &Vehicle, before: (Vec3, Vec3), world: &TrackCollision, dt: f32) -> Option<(Cause, Way)> {
        let mut touching = false;
        let mut impact: Option<(f32, Vec3)> = None;
        for (centre, radius) in v.body_spheres() {
            let r = centre - v.position;
            let Some(hit) = world.sphere(centre, radius + 0.03) else { continue };
            touching = true;
            let speed = -(before.0 + before.1.cross(r)).dot(hit.normal);
            if impact.is_none_or(|(s, _)| speed > s) {
                impact = Some((speed, hit.normal));
            }
        }
        let up = v.up();
        let wheels = v.wheels.iter().filter(|w| w.contact).count();
        if let Some((speed, _)) = impact {
            self.knock = self.knock.max(speed);
        }
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

    /// Watches the quad over water (0xd7f60): coming down fast just over
    /// the surface, the seated rider braces (`Plunge`, 0x2cca0). Returns
    /// whether the quad is in the water, its centre [`UNDER`] the surface.
    pub fn over_water(&mut self, v: &Vehicle, surface: Option<f32>, dt: f32) -> bool {
        if let Some(t) = &mut self.plunge {
            *t += dt;
            // Once it's over, it can play again.
            if *t > 1.0 || self.fall.is_some() {
                self.plunge = None;
            }
        }
        let Some(height) = surface.map(|s| v.position.y - s) else { return false };
        if self.fall.is_none() && self.plunge.is_none() && height < PLUNGE_HEIGHT && -v.velocity.y > PLUNGE_SPEED {
            self.plunge = Some(0.0);
        }
        height <= -UNDER
    }

    /// The `Plunge` clip and how far through it, while the seated rider
    /// braces for the water.
    pub fn plunge(&self, clips: &CrashClips) -> Option<(Arc<Clip>, f32)> {
        let t = self.plunge?;
        let clip = clips.get("Plunge")?;
        let len = (clip.frames() - 1).max(1) as f32 / ONE_SHOT_FPS;
        (t <= len).then(|| (clip, t / len))
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
            prev: body,
            heading: Quat::from_rotation_arc(Vec3::NEG_Z, fwd),
            clips,
            clip: 0,
            t: 0.0,
            rest: 0.0,
            age: 0.0,
            floating: cause == Cause::Water,
            wet: (cause == Cause::Water).then_some(0.0),
        });
    }

    /// Moves the rider along. Returns true when the crash is over and the
    /// quad should be put back.
    pub fn advance(&mut self, clips: Option<&CrashClips>, world: &TrackCollision, dt: f32) -> bool {
        self.advance_in(clips, world, None, false, dt)
    }

    /// [`Wreck::advance`] by the track's water; `quad_wet` is whether the
    /// quad is in it.
    pub fn advance_in(&mut self, clips: Option<&CrashClips>, world: &TrackCollision, water: Option<&Water>, quad_wet: bool, dt: f32) -> bool {
        let Some(f) = &mut self.fall else { return false };
        f.prev = f.body;
        f.age += dt;
        let frames = |name: &str| clips.and_then(|c| c.get(name)).map_or(1, |c| c.frames());
        let surface = water.and_then(|w| w.surface(f.body));
        let level = |at: Vec3| water.and_then(|w| w.level(at)).map(|y| at.with_y(y));
        if !f.floating && surface.is_some_and(|s| s - f.body.y >= UNDER) {
            // A thrown rider landing in deep water treads water too, with
            // a splash if they came down fast.
            if -f.vel.y > PLUNGE_SPEED
                && let Some(at) = level(f.body)
            {
                self.water_fx.push(WaterFx::Splash { at, motion: f.vel / 60.0, radius: RIDER_RADIUS });
            }
            f.floating = true;
            f.clips = vec![clips_for(Cause::Water, Way::Front, 0.0, false, 0.0)[0]];
            f.clip = 0;
            f.t = 0.0;
        }
        if f.floating || quad_wet {
            *f.wet.get_or_insert(0.0) += dt;
        }
        if let (true, Some(s)) = (f.floating, water.and_then(|w| w.level(f.body))) {
            let height = f.body.y - s;
            f.vel.y += (FLOAT_PULL * (FLOAT_LEVEL - height - 0.5 * f.vel.y) - FLOAT_GRAVITY) * dt;
            f.vel *= (-WATER_DRAG * dt).exp();
            f.body += f.vel * dt;
            if let Some(hit) = world.sphere(f.body, BODY_RADIUS) {
                f.body += hit.normal * hit.t;
                f.vel -= hit.normal * f.vel.dot(hit.normal).min(0.0);
            }
            let loops = tread_frame(f.t).1;
            f.t += dt;
            if tread_frame(f.t).1 > loops
                && let Some(at) = level(f.body)
            {
                self.water_fx.push(WaterFx::Stir(at));
            }
            return f.wet.is_some_and(|w| w >= WET_TIME);
        }
        f.vel.y -= 9.81 * dt;
        f.body += f.vel * dt;
        // A little margin so a rider resting on the ground counts as on it.
        if let Some(hit) = world.sphere(f.body, BODY_RADIUS + 0.05) {
            f.body += hit.normal * (hit.t - 0.05).max(0.0);
            let vn = f.vel.dot(hit.normal);
            if vn < 0.0 {
                let speed = f.vel.length();
                if self.thump.is_none_or(|(i, _)| -vn > i) {
                    self.thump = Some((-vn, speed));
                }
                f.vel -= hit.normal * vn;
            }
            // Sliding to a stop.
            let vt = f.vel - hit.normal * f.vel.dot(hit.normal);
            let slow = (SLIDE_DECEL * dt).min(vt.length());
            f.vel -= vt.normalize_or_zero() * slow;
        }
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
        // However it lands, it's over after a while; sooner with the quad
        // in the water.
        done || f.age > 8.0 || f.wet.is_some_and(|w| w >= WET_TIME)
    }

    /// The crash clip and how far through it, for the rider.
    pub fn pose(&self, clips: &CrashClips) -> Option<(Arc<Clip>, f32)> {
        let f = self.fall.as_ref()?;
        let clip = clips.get(f.clips[f.clip])?;
        let last = (clip.frames() - 1).max(1) as f32;
        let t = if f.floating { tread_frame(f.t).0 / last } else { f.t * ONE_SHOT_FPS / last };
        Some((clip, t.clamp(0.0, 1.0)))
    }
}

/// The player's rider follows the crash, and plays a landing after a big
/// jump.
pub fn rider_follows(
    fixed: Res<Time<Fixed>>,
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
                let world = Transform::from_translation(f.drawn(fixed.overstep_fraction())).with_rotation(f.heading);
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
        assert_eq!(clips_for(Cause::Water, Way::Back, 20.0, true, 0.5), ["Tread_water"]);
    }

    /// A wide pond: its surface at height 0 over a flat bed 10 m down.
    fn pond() -> (Water, TrackCollision) {
        let s = 100.0;
        let tri = |y: f32| vec![[Vec3::new(-s, y, -s), Vec3::new(s, y, -s), Vec3::new(0.0, y, s)]];
        let lake = atv_formats::rom::Locator { name: "waterbox".into(), pos: [0.0, -5.0, 0.0], scale: [s, 10.0, s], rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] };
        (Water(TrackCollision::new(tri(0.0), 8.0), vec![crate::water::WaterBox::from_locator(&lake)]), TrackCollision::new(tri(-10.0), 8.0))
    }

    fn quad_at(y: f32) -> Vehicle {
        Vehicle::new(atv_sim::VehicleParams::from_files("", None), Vec3::new(0.0, y, 0.0), Quat::IDENTITY)
    }

    #[test]
    fn plunges_then_treads_water_until_put_back() {
        let (water, bed) = pond();
        let dt = 1.0 / 60.0;
        let mut w = Wreck::default();
        // Coming down fast 2 m over the surface: the rider braces.
        let mut v = quad_at(2.0);
        v.velocity = Vec3::new(0.0, -8.0, -10.0);
        assert!(!w.over_water(&v, water.surface(v.position), dt));
        assert!(w.plunge.is_some());
        // Half a metre under isn't in yet; a metre under is.
        let v = quad_at(-0.5);
        assert!(!w.over_water(&v, water.surface(v.position), dt));
        let v = quad_at(-1.0);
        assert!(w.over_water(&v, water.surface(v.position), dt));
        w.start(&v, Cause::Water, Way::Front, 0.0);
        let mut t = dt;
        while !w.advance_in(None, &bed, Some(&water), true, dt) {
            t += dt;
            assert!(t < 3.0);
        }
        assert!((t - WET_TIME).abs() < 0.05, "{t}");
        // Floating with the head and shoulders out.
        let body = w.fall.as_ref().unwrap().body;
        assert!((body.y + 0.2).abs() < 0.1, "{body}");
        assert_eq!(w.fall.as_ref().unwrap().clips, ["Tread_water"]);
    }

    #[test]
    fn treading_water_loops() {
        assert_eq!(tread_frame(0.0), (2.0, 0));
        let (f, n) = tread_frame(1.0);
        assert!((f - 32.0).abs() < 1e-3 && n == 0);
        // 44 frames in, the loop starts; every 22 frames it starts again.
        let (f, n) = tread_frame(45.0 / 30.0);
        assert!((f - 47.0).abs() < 1e-3 && n == 1, "{f} {n}");
        let (f, n) = tread_frame(67.0 / 30.0);
        assert!((f - 47.0).abs() < 1e-3 && n == 2, "{f} {n}");
    }

    #[test]
    fn thrown_into_the_water() {
        let (water, bed) = pond();
        let dt = 1.0 / 60.0;
        let mut w = Wreck::default();
        w.start(&quad_at(5.0), Cause::Impact, Way::Front, 0.1);
        let mut t = dt;
        while !w.advance_in(None, &bed, Some(&water), false, dt) {
            t += dt;
            assert!(t < 5.0);
        }
        // Falls about 6.5 m into the water with a splash, then treads water
        // and stirs it.
        assert!(matches!(w.water_fx[0], WaterFx::Splash { at, .. } if at.y == 0.0));
        assert!(w.water_fx[1..].iter().all(|fx| matches!(fx, WaterFx::Stir(_))) && w.water_fx.len() == 2);
        let f = w.fall.as_ref().unwrap();
        assert!(f.floating);
        assert_eq!(f.clips, ["Tread_water"]);
        assert!(t > WET_TIME + 1.0 && t < WET_TIME + 1.5, "{t}");
    }
}
