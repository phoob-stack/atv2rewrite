//! Quad bike handling, ported from the original game's vehicle code.
//!
//! Everything here follows the Xbox executable's own "Dyne" car model and
//! its ATV subclass (see `docs/physics.md` for the map from functions and
//! struct offsets to this file). In short: the wheels are spring-damper
//! struts that only hold the quad up; grip, drive, braking and steering are
//! body-level forces worked out in a "ground frame" built from the wheel
//! contacts, shaped by the `.params` response curves. Mass is 1, so forces
//! are accelerations, and gravity is the game's 25 m/s^2.
//!
//! The model runs in the game's own left-handed frame (X right, Y up,
//! +Z forward) so the original formulas carry over unchanged; the public
//! pose fields are mirrored into the sim's right-handed frame (Z negated)
//! after every step.

use std::f32::consts::{PI, TAU};

use glam::{Mat3, Quat, Vec3};

use crate::collision::TrackCollision;
use crate::params::{VehicleParams, by_grip};

pub const GRAVITY: f32 = 25.0;
const MPS_TO_MPH: f32 = 2.237_41;
const MPH_TO_MPS: f32 = 0.446_944;
/// Radians per second -> revs per minute.
const RAD_TO_RPM: f32 = 9.549_3;
/// The original ticks its physics at the display rate, 1/60 s on NTSC
/// (0x84f20), whatever the frame time. Stepping finer changes how the
/// suspension throws the quad off ramps and through water, so the port runs
/// the same fixed tick and interpolates the pose it shows in between.
const TICK: f32 = 1.0 / 60.0;
/// Seconds without wheel contact before the quad counts as airborne.
const AIRBORNE_DELAY: f32 = 0.15;
/// How hard deep water holds back the quad's motion and spin (0x5ba70).
const WATER_DRAG: f32 = 10.0;
const WATER_SPIN_DRAG: f32 = 5.0;
const FLIP: Vec3 = Vec3::new(1.0, 1.0, -1.0);

#[derive(Debug, Clone, Copy, Default)]
pub struct Input {
    pub throttle: f32,
    pub brake: f32,
    /// -1 left .. 1 right.
    pub steer: f32,
    /// -1 back .. 1 forward (rider weight; pitch control in the air).
    pub lean: f32,
    /// Held to charge the suspension pre-load, released to hop.
    pub jump: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WheelState {
    /// Suspension travel measured from full extension (`min_pos`), metres.
    pub compression: f32,
    pub contact: bool,
    /// Contact normal in the sim frame.
    pub contact_normal: Vec3,
    /// Rolling angle for rendering, radians.
    pub spin: f32,
    /// Steering angle for rendering (sim frame, about +Y), radians.
    pub steer: f32,
    /// Surface id under the tyre (see [`crate::surface`]).
    pub surface: u8,
}

/// What another game needs to show a quad (see [`Vehicle::pose`] and
/// [`Vehicle::show`]): its pose and motion, its wheels, and the smoothed
/// controls the rider and the engine sound follow. Sim frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuadPose {
    pub position: Vec3,
    pub rotation: Quat,
    pub velocity: Vec3,
    pub angular_velocity: Vec3,
    pub wheels: Vec<WheelState>,
    pub steer: f32,
    pub throttle: f32,
    pub brake: f32,
    pub lean: f32,
    pub gear: usize,
    pub revs: f32,
    pub preload: f32,
    pub air_time: f32,
}

/// A wheel as drawn: its travel from full extension, roll and steering
/// angle (see [`WheelState`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WheelPose {
    pub compression: f32,
    pub spin: f32,
    pub steer: f32,
}

/// Per-wheel strut state, in the game frame.
#[derive(Debug, Clone, Copy, Default)]
struct Strut {
    /// Position along the suspension axis (`min_pos..=max_pos`).
    pos: f32,
    vel: f32,
    /// Force on the wheel this step (spring reaction).
    force: f32,
    /// Ground penetration to resolve by moving the wheel up its axis.
    correction: f32,
    normal: Vec3,
    /// Contact normal . suspension axis.
    normal_axis: f32,
    contact: bool,
    surface: u8,
    /// Grip and drag of the surface under the tyre.
    friction: f32,
    damping: f32,
}

/// The ground plane under the quad, rebuilt each step from the wheels.
#[derive(Debug, Clone, Copy, Default)]
struct Ground {
    side: Vec3,
    normal: Vec3,
    fwd: Vec3,
    /// Body velocity in (side, normal, fwd) components.
    vel: Vec3,
    speed: f32,
    ang_vel: Vec3,
    contacts: usize,
    /// Average surface grip under the wheels (1 = full grip).
    grip: f32,
    /// Average surface drag (mud, water).
    drag: f32,
}

/// Where the quad is in the race start (retail's state at +0x294).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    Racing,
    /// Held on the grid: free to rev, pinned in place.
    Grid,
    /// A turbo start's extra drive.
    Turbo,
}

/// Retail sets the engine's lerp toward the slew-limited target revs to
/// 50 per second when it builds an ATV (0x6e540).
const RETAIL_REV_LERP: f32 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Preload {
    Idle,
    Charging,
    Holding,
    Fading,
    Released,
}

#[derive(Debug, Clone)]
pub struct Vehicle {
    pub params: VehicleParams,
    /// Sim-frame pose and motion as the last tick left them (mirrors of
    /// the internal game-frame state). Draw with [`Vehicle::drawn`].
    pub position: Vec3,
    pub rotation: Quat,
    pub velocity: Vec3,
    pub angular_velocity: Vec3,
    pub wheels: Vec<WheelState>,
    /// Smoothed controls, as the original keeps them.
    pub steer: f32,
    pub throttle: f32,
    pub brake: f32,
    pub lean: f32,
    /// Index into `params.gears` (0 = reverse).
    pub gear: usize,
    pub revs: f32,
    /// Pre-load charge, 0..1.
    pub preload: f32,
    pub air_time: f32,
    /// Scales the drive force; the game's speed boost raises it (the
    /// original's torque modifier, 1.0 unless boosting).
    pub drive_scale: f32,
    /// Set by the game while the quad is in deep water, which drags it to
    /// a crawl (0x5ba70).
    pub in_water: bool,

    // Game-frame rigid body.
    pos: Vec3,
    rot: Mat3,
    momentum: Vec3,
    ang_momentum: Vec3,
    vel: Vec3,
    ang_vel: Vec3,
    prev_vel: Vec3,
    /// Body-frame acceleration over the last step (drives wheelies/endos).
    local_acc: Vec3,
    inertia: Vec3,
    force: Vec3,
    torque: Vec3,
    pos_fix: Vec3,
    /// Time not yet simulated, and the pose before the last tick, which the
    /// shown pose blends from.
    pending: f32,
    shown_pos: Vec3,
    shown_rot: Mat3,
    shown_wheels: Vec<WheelPose>,
    /// How long the shown pose takes to blend into the current one: a
    /// tick, or the time between the poses a puppet is shown (see
    /// [`Vehicle::show`]).
    span: f32,
    /// Where each body sphere was last step (the original keeps it to tell
    /// which side of a face a sphere came from).
    body_prev: Vec<Vec3>,
    /// Ticks run by the last [`Vehicle::step`].
    ticks: u32,

    struts: Vec<Strut>,
    ground: Ground,
    /// Last forward speed over the ground (kept while airborne).
    ground_fwd: f32,

    throttle_target: f32,
    brake_target: f32,
    reverse: bool,
    reverse_timer: f32,

    slide_handbrake: f32,
    slide_brake: f32,
    slide_steer: f32,
    slide: f32,
    wheelspin: f32,

    gear_target: usize,
    clutch: f32,
    shifting: bool,
    shift_timer: f32,
    since_shift: f32,

    preload_state: Preload,
    preload_hold: f32,
    gravity_scale: f32,
    gravity_timer: f32,
    landing_stab: f32,
    airborne: bool,

    start: Start,
    turbo_timer: f32,
    /// Grid spot the quad is pinned to (game frame).
    hold: Option<Vec3>,
    /// Stopped with no pedal pressed: held like on the grid (see
    /// [`Vehicle::park`]).
    parked: bool,
}

/// A body touching something, game frame.
#[derive(Debug, Clone, Copy)]
struct Contact {
    /// From the centre of mass to the contact point.
    at: Vec3,
    /// Out of the thing touched, towards the body.
    normal: Vec3,
    depth: f32,
    friction: f32,
    /// The bounce: how much of the speed into the contact comes back out.
    bounce: f32,
    /// How much of the contact's leverage turns the body (0x6fef0 sets it
    /// below 1 for track contacts).
    lever: f32,
}

/// Two quads touching (0x6fa00), each with the other's pose before and
/// after their last ticks: their wheels as spheres against each other's
/// (0xd5dc0), and each one's body spheres against the other's hull
/// (0xd7680; the original only tries one way round). Solved as the
/// original solves contacts between bodies (0xd5090): the overlap shared
/// out by mass, the impulse that turns the speed between the two points
/// around by the bounce, and friction on `a`. Wheels bounce by the product of the quads' `HARDNESS` and grip by
/// the product of their `FRICTION`; a body against a hull bounces by the
/// harder of the two, without friction.
pub fn bump(a: &mut Vehicle, b: &mut Vehicle) {
    if a.ticks == 0 && b.ticks == 0 {
        return;
    }
    if a.pos.distance(b.pos) > a.params.body.reach + b.params.body.reach + 1.0 {
        return;
    }
    let mut contacts = wheel_contacts(a, b);
    contacts.extend(hull_contacts(a, b));
    // The rewrite's: `b`'s spheres against `a`'s hull too, so neither quad
    // sinks into the other. One way round, pack riders lock together and
    // three times as many get respotted in a six-rider race.
    for c in hull_contacts(b, a) {
        contacts.push(PairContact { at_a: c.at_b, at_b: c.at_a, normal: -c.normal, ..c });
    }
    if contacts.is_empty() {
        return;
    }
    let (ma, mb) = (a.params.mass, b.params.mass);
    let (mut fix_a, mut fix_b) = (Vec3::ZERO, Vec3::ZERO);
    let mut pushed = Vec::with_capacity(contacts.len());
    for c in &contacts {
        let share = 1.0 / (ma + mb);
        Vehicle::add_fix(&mut fix_a, c.normal * (c.depth * mb * share));
        Vehicle::add_fix(&mut fix_b, -c.normal * (c.depth * ma * share));
        let v = (a.vel + a.ang_vel.cross(c.at_a)) - (b.vel + b.ang_vel.cross(c.at_b));
        let vn = v.dot(c.normal);
        let k = a.inv_mass_at(c.at_a, c.normal) + b.inv_mass_at(c.at_b, c.normal);
        let j = if k.abs() > 1e-6 { -(1.0 + c.bounce) * vn / k } else { 0.0 };
        a.impulse(c.normal * j, c.at_a);
        b.impulse(-c.normal * j, c.at_b);
        pushed.push(j);
    }
    let slides: Vec<Vec3> = contacts
        .iter()
        .map(|c| {
            let v = (a.vel + a.ang_vel.cross(c.at_a)) - (b.vel + b.ang_vel.cross(c.at_b));
            v - c.normal * v.dot(c.normal)
        })
        .collect();
    for ((c, &j), &slide) in contacts.iter().zip(&pushed).zip(&slides) {
        if j > 0.0 && c.friction > 0.0 {
            let own = Contact { at: c.at_a, normal: c.normal, depth: c.depth, friction: c.friction, bounce: c.bounce, lever: 1.0 };
            a.friction(&own, slide, c.friction * j);
        }
    }
    for (v, fix) in [(a, fix_a), (b, fix_b)] {
        v.pos += fix;
        v.shown_pos += fix;
        for p in &mut v.body_prev {
            *p += fix;
        }
        v.sync_out();
    }
}

/// A contact between two bodies, game frame.
#[derive(Debug, Clone, Copy)]
struct PairContact {
    /// From each body's centre of mass to the contact point.
    at_a: Vec3,
    at_b: Vec3,
    /// From `b` towards `a`.
    normal: Vec3,
    depth: f32,
    friction: f32,
    bounce: f32,
}

/// A quad's wheels as spheres (its WHEELOFFSETs and WHEELRADIUSes, the
/// set built at 0x1d840), where they were before its last tick and now.
fn wheel_spheres(v: &Vehicle) -> impl Iterator<Item = (Vec3, Vec3, f32)> + '_ {
    v.params.wheels.iter().map(|w| {
        let at = Vec3::from(w.game_offset);
        (v.shown_pos + v.shown_rot * at, v.pos + v.rot * at, w.radius)
    })
}

/// `a`'s wheels against `b`'s (0xd5dc0). Overlapping spheres push apart
/// along the line between their centres; spheres that passed into or
/// through each other during the tick push apart sideways from where they
/// first met. Either way the contact acts at the bodies' own height, so it
/// shoves and turns a quad but doesn't tip it.
fn wheel_contacts(a: &Vehicle, b: &Vehicle) -> Vec<PairContact> {
    let mut out = Vec::new();
    for (pa, ca, ra) in wheel_spheres(a) {
        for (pb, cb, rb) in wheel_spheres(b) {
            let reach = ra + rb;
            let (d0, d1) = (pb - pa, cb - ca);
            let (normal, depth, meet) = if d1.length_squared() < reach * reach {
                // Overlapping now.
                let gap = d1.length();
                if gap < 1e-4 {
                    continue;
                }
                (-d1 / gap, reach - gap, d1 / gap)
            } else {
                // Met during the tick: when the gap first closed to `reach`.
                let m = d1 - d0;
                let mm = m.length_squared();
                let dm = d0.dot(m);
                let c0 = d0.length_squared() - reach * reach;
                if mm < 1e-6 || c0 < 0.0 || dm * dm < mm * c0 {
                    continue;
                }
                let t = (-dm - (dm * dm - mm * c0).sqrt()) / mm;
                if !(0.0..=1.0).contains(&t) {
                    continue;
                }
                let Some(u) = (d0 + m * t).with_y(0.0).try_normalize() else { continue };
                (-u, reach - d1.dot(u), u)
            };
            if depth <= 0.0 {
                continue;
            }
            let at_a = (ca + meet * ra - a.pos).with_y(0.0);
            let at_b = (cb - meet * rb - b.pos).with_y(0.0);
            out.push(PairContact {
                at_a,
                at_b,
                normal,
                depth,
                friction: a.params.friction * b.params.friction,
                bounce: a.params.hardness * b.params.hardness,
            });
        }
    }
    out
}

/// `a`'s body spheres inside `b`'s hull (0xd7190): pushed out through the
/// face they are nearest to leaving by, or, a sphere that moved in this
/// tick, back out through the face it came in by.
fn hull_contacts(a: &Vehicle, b: &Vehicle) -> Vec<PairContact> {
    let mut out = Vec::new();
    let hull = b.posed_hull();
    if hull.is_empty() {
        return out;
    }
    for (&(local, _), (c, r)) in a.params.body.spheres.iter().zip(a.posed_spheres()) {
        let dist = |p: Vec3, &(n, d): &(Vec3, f32)| n.dot(p) + d;
        let Some(mut best) = hull.iter().max_by(|x, y| dist(c, x).total_cmp(&dist(c, y))).copied() else { continue };
        if dist(c, &best) > r {
            continue;
        }
        // Where it was before the tick (nowhere else if `a` didn't tick).
        let prev = if a.ticks > 0 { a.shown_pos + a.shown_rot * local } else { c };
        let moved = c - prev;
        if prev.is_finite() && moved.length_squared() > 0.005_548_87 && hull.iter().any(|h| dist(prev, h) > -r) {
            // The face its surface crossed last on the way in.
            let mut latest = -1e-6;
            for h in &hull {
                if moved.dot(h.0) > 0.0 {
                    continue;
                }
                let (s1, s0) = (c - h.0 * r, c - h.0 * r - moved);
                let (d0, d1) = (dist(s0, h), dist(s1, h));
                if (d0 < 0.0) != (d1 < 0.0) && (d1 - d0).abs() >= 1e-6 {
                    let t = d0 / (d0 - d1);
                    if t > latest {
                        latest = t;
                        best = *h;
                    }
                }
            }
        }
        let n = best.0;
        let depth = r - dist(c, &best);
        let point = c - n * r;
        out.push(PairContact {
            at_a: point - a.pos,
            at_b: point - b.pos,
            normal: n,
            depth,
            friction: 0.0,
            bounce: a.params.hardness.max(b.params.hardness),
        });
    }
    out
}

/// Moves `cur` toward `target`. `rate` applies when heading toward a
/// non-zero target, `ret` when returning toward zero, and both when
/// crossing zero (the original's input-interpolation rule).
fn approach(cur: &mut f32, target: f32, rate: f32, ret: f32, dt: f32) {
    if target > *cur {
        let r = if target > 0.0 {
            if *cur < 0.0 { rate + ret } else { rate }
        } else {
            ret
        };
        *cur = (*cur + r * dt).min(target);
    } else if target < *cur {
        let r = if target < 0.0 {
            if *cur > 0.0 { rate + ret } else { rate }
        } else {
            ret
        };
        *cur = (*cur - r * dt).max(target);
    }
}

/// Mirror between the sim's right-handed frame and the game's left-handed one.
fn mirror(m: Mat3) -> Mat3 {
    let f = Mat3::from_diagonal(FLIP);
    f * m * f
}

impl Vehicle {
    pub fn new(params: VehicleParams, position: Vec3, rotation: Quat) -> Self {
        let [w, h, l] = params.dims;
        let m = params.mass;
        let inertia = Vec3::new(
            m * (h * h + l * l),
            m * (w * w + l * l),
            m * (w * w + h * h),
        ) / 12.0;
        let n = params.wheels.len();
        let mut v = Self {
            position,
            rotation,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            wheels: vec![WheelState::default(); n],
            steer: 0.0,
            throttle: 0.0,
            brake: 0.0,
            lean: 0.0,
            gear: 1,
            revs: params.min_revs,
            preload: 0.0,
            air_time: 0.0,
            drive_scale: 1.0,
            in_water: false,
            pos: Vec3::ZERO,
            rot: Mat3::IDENTITY,
            momentum: Vec3::ZERO,
            ang_momentum: Vec3::ZERO,
            vel: Vec3::ZERO,
            ang_vel: Vec3::ZERO,
            prev_vel: Vec3::ZERO,
            local_acc: Vec3::ZERO,
            inertia,
            force: Vec3::ZERO,
            torque: Vec3::ZERO,
            pos_fix: Vec3::ZERO,
            pending: 0.0,
            shown_pos: Vec3::ZERO,
            shown_rot: Mat3::IDENTITY,
            shown_wheels: vec![WheelPose::default(); n],
            span: TICK,
            body_prev: Vec::new(),
            ticks: 0,
            struts: vec![Strut::default(); n],
            ground: Ground {
                grip: 1.0,
                ..Default::default()
            },
            ground_fwd: 0.0,
            throttle_target: 0.0,
            brake_target: 0.0,
            reverse: false,
            reverse_timer: 0.0,
            slide_handbrake: 0.0,
            slide_brake: 0.0,
            slide_steer: 0.0,
            slide: 0.0,
            wheelspin: 0.0,
            gear_target: 1,
            clutch: 1.0,
            shifting: false,
            shift_timer: 0.0,
            since_shift: 0.0,
            preload_state: Preload::Idle,
            preload_hold: 0.0,
            gravity_scale: 1.0,
            gravity_timer: 0.0,
            landing_stab: 0.0,
            airborne: false,
            start: Start::Racing,
            turbo_timer: 0.0,
            hold: None,
            parked: false,
            params,
        };
        v.reset(position, rotation);
        v
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }
    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }
    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }
    pub fn speed_mph(&self) -> f32 {
        self.velocity.dot(self.forward()) * MPS_TO_MPH
    }
    pub fn grounded(&self) -> bool {
        self.struts.iter().any(|s| s.contact)
    }
    /// Every wheel on the ground.
    pub fn all_wheels_down(&self) -> bool {
        !self.struts.is_empty() && self.struts.iter().all(|s| s.contact)
    }
    /// How far the driven wheels are spinning past the ground (0 gripping,
    /// up to the throttle when spinning freely).
    pub fn wheelspin(&self) -> f32 {
        self.wheelspin
    }
    /// The throttle pedal before smoothing.
    pub fn throttle_pedal(&self) -> f32 {
        self.throttle_target
    }
    /// How far the clutch is in: 0 slipping (a gear change, or rolling off
    /// the throttle slowly) to 1 driving.
    pub fn clutch(&self) -> f32 {
        self.clutch
    }
    /// The gearbox is in, or changing into, first gear.
    pub fn first_gear(&self) -> bool {
        self.gear_target == 1
    }

    /// Puts the quad back at `position`, at rest.
    pub fn reset(&mut self, position: Vec3, rotation: Quat) {
        self.pos = position * FLIP;
        self.rot = mirror(Mat3::from_quat(rotation));
        self.momentum = Vec3::ZERO;
        self.ang_momentum = Vec3::ZERO;
        self.vel = Vec3::ZERO;
        self.ang_vel = Vec3::ZERO;
        self.prev_vel = Vec3::ZERO;
        self.local_acc = Vec3::ZERO;
        self.pos_fix = Vec3::ZERO;
        self.body_prev.clear();
        for (i, s) in self.struts.iter_mut().enumerate() {
            *s = Strut {
                pos: self.params.wheels[i].max_pos.min(0.0),
                ..Default::default()
            };
        }
        self.ground = Ground {
            grip: 1.0,
            ..Default::default()
        };
        self.ground_fwd = 0.0;
        self.reverse = false;
        self.reverse_timer = 0.0;
        self.slide_handbrake = 0.0;
        self.slide_brake = 0.0;
        self.slide_steer = 0.0;
        self.slide = 0.0;
        self.wheelspin = 0.0;
        self.gear = 1;
        self.gear_target = 1;
        self.shifting = false;
        self.clutch = 1.0;
        self.preload_state = Preload::Idle;
        self.preload = 0.0;
        self.gravity_scale = 1.0;
        self.gravity_timer = 0.0;
        self.landing_stab = 0.0;
        self.airborne = false;
        self.air_time = 0.0;
        self.in_water = false;
        self.parked = false;
        // `pending` carries on, so the quad keeps ticking with the others.
        if self.hold.is_some() {
            self.hold = Some(self.pos);
        }
        self.shown_pos = self.pos;
        self.shown_rot = self.rot;
        self.shown_wheels = self.wheel_poses();
        self.sync_out();
    }

    /// Holds the quad on the starting grid until [`Vehicle::start_race`]:
    /// it can only settle on its springs, so the rider can rev the engine
    /// (0x6e470 with 1, and the Dyne's grid hold at 0xd9190).
    pub fn hold_on_grid(&mut self) {
        self.start = Start::Grid;
        self.hold = Some(self.pos);
    }

    pub fn on_grid(&self) -> bool {
        self.start == Start::Grid
    }

    /// Lets go of the grid. On retail tuning, revs held between
    /// MINTURBOREVSCALE and MAXTURBOREVSCALE of the rev range give a turbo
    /// start (0x6e470 with 0, 0x6d570).
    pub fn start_race(&mut self) {
        self.hold = None;
        if self.start != Start::Grid {
            return;
        }
        if self.turbo_window() {
            self.throttle = self.throttle_target;
            self.turbo_timer = 0.0;
            self.start = Start::Turbo;
        } else {
            self.start = Start::Racing;
        }
    }

    /// Holds the quad on the grid while `held`, and starts the race the
    /// first step it isn't.
    pub fn set_grid(&mut self, held: bool) {
        if held && self.hold.is_none() {
            self.hold_on_grid();
        } else if !held && self.hold.is_some() {
            self.start_race();
        }
    }

    /// Whether a turbo start is giving extra drive.
    pub fn turbo(&self) -> bool {
        self.start == Start::Turbo
    }

    fn turbo_window(&self) -> bool {
        let p = &self.params;
        let x = (self.revs - p.min_revs) / (p.max_revs - p.min_revs);
        p.min_turbo_rev_scale < x && x < p.max_turbo_rev_scale
    }

    /// The quad as the last tick left it, for another game to show (see
    /// [`Vehicle::show`]).
    pub fn pose(&self) -> QuadPose {
        QuadPose {
            position: self.pos * FLIP,
            rotation: Quat::from_mat3(&mirror(self.rot)).normalize(),
            velocity: self.velocity,
            angular_velocity: self.angular_velocity,
            wheels: self.wheels.clone(),
            steer: self.steer,
            throttle: self.throttle,
            brake: self.brake,
            lean: self.lean,
            gear: self.gear,
            revs: self.revs,
            preload: self.preload,
            air_time: self.air_time,
        }
    }

    /// Shows the quad at `pose` without simulating it, for a quad another
    /// game drives: drawn, it moves from where it was shown last to `pose`
    /// over the next `span` seconds.
    pub fn show(&mut self, pose: &QuadPose, span: f32) {
        self.shown_pos = self.pos;
        self.shown_rot = self.rot;
        self.shown_wheels = self.wheel_poses();
        self.pending = 0.0;
        self.span = span.max(1e-4);
        self.ticks = 0;
        self.pos = pose.position * FLIP;
        self.rot = mirror(Mat3::from_quat(pose.rotation.normalize()));
        self.vel = pose.velocity * FLIP;
        self.momentum = self.vel * self.params.mass;
        self.ang_vel = -(pose.angular_velocity * FLIP);
        self.ground_fwd = pose.velocity.dot(pose.rotation * Vec3::NEG_Z);
        (self.steer, self.throttle, self.brake, self.lean) = (pose.steer, pose.throttle, pose.brake, pose.lean);
        (self.gear, self.revs, self.preload, self.air_time) = (pose.gear, pose.revs, pose.preload, pose.air_time);
        for (i, wp) in self.params.wheels.iter().enumerate() {
            let Some(w) = pose.wheels.get(i) else { continue };
            let s = &mut self.struts[i];
            s.pos = wp.min_pos + w.compression;
            s.contact = w.contact;
            s.surface = w.surface;
            s.normal = w.contact_normal * FLIP;
            self.wheels[i].spin = w.spin;
        }
        self.airborne = !self.grounded();
        self.sync_out();
    }

    /// Moves the quad by `offset` and changes its velocity by `delta_v`
    /// (sim frame), as knocking a prop over does.
    pub fn nudge(&mut self, offset: Vec3, delta_v: Vec3) {
        self.pos += offset * FLIP;
        self.shown_pos += offset * FLIP;
        self.momentum += delta_v * FLIP * self.params.mass;
        self.vel = self.momentum / self.params.mass;
        self.position = self.pos * FLIP;
        self.velocity = self.vel * FLIP;
    }

    /// Advances the simulation by `dt` seconds in the original's fixed
    /// ticks; time short of a tick carries over to the next call.
    pub fn step(&mut self, input: &Input, dt: f32, world: &TrackCollision) {
        // As the original, at most 0.1 s per frame.
        self.pending += dt.clamp(0.0, 0.1);
        self.span = TICK;
        self.ticks = 0;
        while self.pending >= TICK - 1e-5 {
            self.ticks += 1;
            self.pending = (self.pending - TICK).max(0.0);
            self.shown_pos = self.pos;
            self.shown_rot = self.rot;
            self.shown_wheels = self.wheel_poses();
            self.substep(input, TICK, world);
            self.spin_wheels();
        }
        self.sync_out();
    }

    fn substep(&mut self, input: &Input, h: f32, world: &TrackCollision) {
        self.controls(input, h);
        self.wheel_contacts(world);
        self.ground_frame();
        self.springs();
        // Retail holds the rider's weight and the pre-load back on the
        // grid (0x6f750).
        let racing = !(self.params.retail && self.start == Start::Grid);
        if self.ground.contacts > 0 {
            if racing {
                let right = self.rot.x_axis;
                self.torque += right * self.lean * self.params.lean_force;
                self.wheelies();
            }
            self.force += Vec3::NEG_Y * self.params.gravity * self.params.mass;
            self.ground_forces(h);
        } else {
            self.air_forces();
        }
        if self.params.retail {
            self.stabiliser();
        }
        if self.in_water {
            self.force -= self.vel * WATER_DRAG;
            self.torque -= self.ang_vel * WATER_SPIN_DRAG;
        }
        if racing {
            self.preload_update(input.jump, h);
        }
        self.landing_stabiliser(h);
        if self.params.retail && self.start == Start::Turbo {
            self.turbo_timer += h;
            if self.turbo_timer > self.params.max_turbo_time {
                self.turbo_timer = 0.0;
                self.start = Start::Racing;
            }
        }
        self.pin_to_grid();
        self.park(input);
        self.integrate(h, world);
        if let Some(at) = self.hold {
            self.pos.x = at.x;
            self.pos.z = at.z;
        }
        self.post_step(h);
    }

    /// Pedal handling (forward/reverse switching, auto-brake at a standstill)
    /// and the original's smoothing of every control.
    fn controls(&mut self, input: &Input, h: f32) {
        let mph = self.ground_fwd * MPS_TO_MPH;
        let body_fwd = self.vel.dot(self.rot.z_axis);
        let (acc_in, brake_in) = (input.throttle.clamp(0.0, 1.0), input.brake.clamp(0.0, 1.0));
        let (mut throttle, mut brake);
        if !self.reverse {
            (throttle, brake) = (acc_in, brake_in);
            if brake_in < 0.2 {
                self.reverse_timer = 0.0;
                if body_fwd < -1.0 {
                    brake = 1.0;
                }
            } else if mph >= 1.0 {
                self.reverse_timer = (self.reverse_timer + h).min(1.0);
            } else {
                self.reverse_timer -= h;
                if self.reverse_timer <= 0.0 {
                    self.reverse_timer = 0.0;
                    self.set_reverse(true);
                    (throttle, brake) = (brake_in, acc_in);
                }
            }
        } else {
            (throttle, brake) = (brake_in, acc_in);
            if acc_in < 0.2 {
                self.reverse_timer = 0.0;
                if body_fwd >= 1.0 {
                    brake = 1.0;
                }
            } else if mph <= -1.0 {
                self.reverse_timer = (self.reverse_timer + h).min(1.0);
            } else {
                self.reverse_timer -= h;
                if self.reverse_timer <= 0.0 {
                    self.reverse_timer = 0.0;
                    self.set_reverse(false);
                    (throttle, brake) = (acc_in, brake_in);
                }
            }
        }
        if throttle < 0.2 && brake < 0.2 && mph.abs() < 0.1 {
            brake = 1.0;
        }
        if brake != 0.0 {
            throttle = (throttle - brake).max(0.0);
        }
        self.throttle_target = throttle;
        self.brake_target = brake;

        let p = &self.params;
        approach(
            &mut self.lean,
            input.lean.clamp(-1.0, 1.0),
            p.lean_rate,
            p.lean_rate,
            h,
        );
        self.lean = self.lean.clamp(-1.0, 1.0);
        let grip = self.ground.grip;
        let steer_rate = p.steer_rate_curve.sample(mph) * by_grip(p.steer_rate, grip);
        approach(
            &mut self.steer,
            input.steer.clamp(-1.0, 1.0),
            steer_rate,
            by_grip(p.steer_return_rate, grip),
            h,
        );
        approach(
            &mut self.throttle,
            self.throttle_target,
            p.acc_rate,
            p.acc_return_rate,
            h,
        );
        approach(
            &mut self.brake,
            self.brake_target,
            p.brake_rate,
            p.brake_return_rate,
            h,
        );
        self.steer = self.steer.clamp(-1.0, 1.0);
        self.throttle = self.throttle.clamp(0.0, 1.0);
        self.brake = self.brake.clamp(0.0, 1.0);
        // Leaning right back limits the steering.
        if self.lean < -0.2 {
            let lim = self.lean + 1.2;
            self.steer = self.steer.clamp(-lim, lim);
        }
        if p.retail {
            // Retail's ATV runs the Dyne's smoothing and then its own
            // copy of it (0x6e0b0), so the pedals and steering move twice
            // per tick.
            approach(&mut self.throttle, self.throttle_target, p.acc_rate, p.acc_return_rate, h);
            approach(&mut self.brake, self.brake_target, p.brake_rate, p.brake_return_rate, h);
            approach(
                &mut self.steer,
                input.steer.clamp(-1.0, 1.0),
                steer_rate,
                by_grip(p.steer_return_rate, grip),
                h,
            );
            self.throttle = self.throttle.clamp(0.0, 1.0);
            self.brake = self.brake.clamp(0.0, 1.0);
            self.steer = self.steer.clamp(-1.0, 1.0);
        }
    }

    fn set_reverse(&mut self, reverse: bool) {
        self.reverse = reverse;
        self.shift_to(if reverse { 0 } else { 1 });
    }

    fn shift_to(&mut self, gear: usize) {
        let gear = gear.min(self.params.gears.len().saturating_sub(1));
        if self.gear_target != gear {
            self.gear_target = gear;
            self.shifting = true;
            self.shift_timer = 0.0;
            self.since_shift = 0.0;
        }
    }

    /// Ray casts in the game frame against the (sim-frame) track.
    fn cast(
        world: &TrackCollision,
        origin: Vec3,
        dir: Vec3,
        max: f32,
    ) -> Option<(f32, Vec3, Vec3, u8)> {
        world
            .raycast_front(origin * FLIP, dir * FLIP, max)
            .map(|h| (h.t, h.point * FLIP, h.normal * FLIP, h.surface))
    }

    /// Finds where each tyre touches the ground and how far it sinks in.
    fn wheel_contacts(&mut self, world: &TrackCollision) {
        // Cast from a little above the wheel centre: enough to catch a
        // wheel that has sunk in, not so much that it finds the underside
        // of an overhang above the wheel and treats it as ground.
        const HEADROOM: f32 = 0.3;
        for (i, wp) in self.params.wheels.iter().enumerate() {
            let s = &mut self.struts[i];
            s.force = 0.0;
            s.correction = 0.0;
            s.contact = false;
            let attach = self.rot * Vec3::from(wp.game_offset);
            let axis = (self.rot * Vec3::from(wp.axis)).normalize_or(self.rot.y_axis);
            let centre = self.pos + attach + axis * s.pos;
            let Some((t, _, n, surface)) = Self::cast(
                world,
                centre + axis * HEADROOM,
                -axis,
                HEADROOM + wp.radius + 0.5,
            ) else {
                continue;
            };
            let na = n.dot(axis);
            // Faces steeper than ~85 degrees from the strut are walls.
            if na < 0.087_155_7 {
                continue;
            }
            let depth = wp.radius - (t - HEADROOM) * na;
            if depth < -0.03 {
                continue;
            }
            if depth > 0.0 {
                // The part of the push the strut can't take moves the body.
                // On steep faces the original leaves that to the body's
                // collision spheres; without them, push away horizontally
                // so the quad can't drive through walls or climb them.
                let push = if na >= std::f32::consts::FRAC_1_SQRT_2 {
                    n * (1.0 - na)
                } else {
                    n.with_y(0.0)
                };
                Self::add_fix(&mut self.pos_fix, push * depth);
                s.correction = na * depth;
            }
            let v_point = self.vel + self.ang_vel.cross(attach);
            s.vel = (-v_point.dot(n) * na).clamp(-wp.max_vel, wp.max_vel);
            s.normal = n;
            s.normal_axis = na;
            s.contact = s.pos + s.correction >= 0.0;
            // The surface's grip and drag; faces over 45 degrees from the
            // strut give a tenth of the grip.
            let surf = world.surface(surface);
            s.surface = surface;
            s.friction = surf.friction
                * if na < std::f32::consts::FRAC_1_SQRT_2 {
                    0.1
                } else {
                    1.0
                };
            s.damping = surf.damping;
        }
    }

    /// Position corrections keep the largest push per axis (summing pushes
    /// in opposite directions), as the original's collision response does.
    fn add_fix(fix: &mut Vec3, d: Vec3) {
        for k in 0..3 {
            let (a, b) = (fix[k], d[k]);
            fix[k] = if (a > 0.0) != (b > 0.0) || a == 0.0 {
                if a == 0.0 { b } else { a + b }
            } else if b.abs() > a.abs() {
                b
            } else {
                a
            };
        }
    }

    fn ground_frame(&mut self) {
        let up = self.rot.y_axis;
        // In the air the handling keeps the last grip on the ground.
        let mut g = Ground {
            grip: self.ground.grip,
            ..Default::default()
        };
        let (mut grip, mut drag) = (0.0, 0.0);
        let mut best = -2.0;
        for s in self.struts.iter().filter(|s| s.contact) {
            g.contacts += 1;
            grip += s.friction;
            drag += s.damping;
            let d = s.normal.dot(up);
            if d > best {
                best = d;
                g.normal = s.normal;
            }
        }
        if g.contacts > 0 {
            g.grip = grip / g.contacts as f32;
            g.drag = drag / g.contacts as f32;
            let n = g.normal;
            let mut f = self.rot.z_axis - n * self.rot.z_axis.dot(n);
            if f.length() < 1e-4 {
                f = up - n * up.dot(n);
            }
            g.fwd = f.normalize_or(Vec3::Z);
            g.side = n.cross(g.fwd);
            g.vel = Vec3::new(self.vel.dot(g.side), self.vel.dot(n), self.vel.dot(g.fwd));
            g.speed = g.vel.length();
            g.ang_vel = Vec3::new(
                self.ang_vel.dot(g.side),
                self.ang_vel.dot(n),
                self.ang_vel.dot(g.fwd),
            );
            self.ground_fwd = g.vel.z;
        }
        self.ground = g;
    }

    /// Strut springs push the body up along the contact normals.
    fn springs(&mut self) {
        for (i, wp) in self.params.wheels.iter().enumerate() {
            let s = &mut self.struts[i];
            if !s.contact {
                continue;
            }
            s.pos += s.correction;
            s.correction = 0.0;
            // Past the bump stops the body itself is moved.
            if s.pos > wp.max_pos {
                Self::add_fix(&mut self.pos_fix, s.normal * (s.pos - wp.max_pos));
                s.pos = wp.max_pos;
            } else if s.pos < wp.min_pos {
                Self::add_fix(&mut self.pos_fix, s.normal * (wp.min_pos - s.pos));
                s.pos = wp.min_pos;
            }
            let f = wp.stiffness * s.pos + wp.damping * s.vel;
            s.force = -f;
            if (f >= 0.0) == (s.normal_axis >= 0.0) {
                let f = if s.normal_axis < 0.0 { -f } else { f };
                let attach = self.rot * Vec3::from(wp.game_offset);
                let fv = s.normal * f;
                self.force += fv;
                self.torque += attach.cross(fv);
            }
        }
    }

    /// Rider weight shifts: wheelies when leaning back under power, endos
    /// when leaning forward under braking.
    fn wheelies(&mut self) {
        let p = &self.params;
        let c = |i: usize| self.struts.get(i).is_some_and(|s| s.contact);
        let right = self.rot.x_axis;
        let pitch_rate = self.ang_vel.dot(right);
        let acc = self.local_acc.z;
        let speed = self.ground.speed;
        if c(2) && c(3) && self.ground.vel.z > 0.0 {
            let lift = if p.retail {
                // Scaled by speed, then by the 0 mph value again (0x6cfa0).
                let mph = self.vel.length() * MPS_TO_MPH;
                ((p.wheelie_torque_100mph - p.wheelie_torque_0mph) * mph * 0.01
                    + p.wheelie_torque_0mph)
                    * acc
                    * self.lean
                    * p.wheelie_torque_0mph
            } else {
                p.wheelie_torque * self.lean * acc
            };
            let t = if !c(0) && !c(1) {
                (self.lean + 1.0) * p.wheelie_return_torque + lift - pitch_rate * p.wheelie_damping
            } else if self.lean <= 0.0 && acc >= 0.0 {
                if speed < 1.0 { lift * speed } else { lift }
            } else {
                0.0
            };
            self.torque += right * t;
        }
        if c(0) && c(1) && self.lean > 0.0 && acc < 0.0 && speed > 1.0 && self.brake > 0.5 {
            let mut t = -(p.endo_torque * self.lean * acc);
            if !c(2) && !c(3) {
                t -= pitch_rate * p.endo_damping;
            }
            self.torque += right * t;
        }
    }

    /// The original's handling model: slides, wheelspin, drive, brakes,
    /// cornering, side grip, yaw damping and drag, all in the ground frame.
    fn ground_forces(&mut self, h: f32) {
        let p = &self.params;
        let g = self.ground;
        let grip = g.grip;
        let (lx, lz) = (g.vel.x, g.vel.z);
        let speed = g.speed;
        let vh = (lx * lx + lz * lz).sqrt();
        let (dir_x, dir_z) = if vh > 1e-4 {
            (lx / vh, lz / vh)
        } else {
            (0.0, 0.0)
        };
        let mph = speed * MPS_TO_MPH;
        let slide_scale = p.slide_scale_curve.sample(mph);
        let acc_mod = by_grip(p.slide_scale_acc_modifier, grip);
        let ret = 1.0 - acc_mod * self.throttle;

        // Sliding: from the handbrake, the brakes and the steering.
        approach(
            &mut self.slide_handbrake,
            0.0,
            p.slide_handbrake_rate_curve.sample(mph) * by_grip(p.slide_handbrake_rate, grip),
            ret * by_grip(p.slide_handbrake_return_rate, grip),
            h,
        );
        approach(
            &mut self.slide_brake,
            slide_scale * self.brake,
            p.slide_brake_rate_curve.sample(mph) * by_grip(p.slide_brake_rate, grip),
            ret * by_grip(p.slide_brake_return_rate, grip),
            h,
        );
        approach(
            &mut self.slide_steer,
            self.steer.abs() * slide_scale,
            p.slide_steer_rate_curve.sample(mph) * by_grip(p.slide_steer_rate, grip),
            ret * by_grip(p.slide_steer_return_rate, grip),
            h,
        );
        self.slide = self
            .slide_steer
            .max(self.slide_brake)
            .max(self.slide_handbrake);
        if self.slide < 0.2 {
            self.slide = 0.0;
        }

        // Wheelspin builds when steering or pulling away hard.
        let low_speed = if speed <= 8.938_89 {
            (8.938_89 - speed) * 0.111_871
        } else {
            0.0
        };
        if p.retail && self.start == Start::Grid {
            // Revving on the grid spins the wheels in the turbo window.
            let target = if self.turbo_window() { 1.0 } else { 0.0 };
            if target > self.wheelspin {
                self.wheelspin += 2.0 * h;
            } else {
                self.wheelspin -= 2.0 * h;
            }
        } else {
            let mut spin_target = if !p.retail {
                ((self.steer.abs() + low_speed) * slide_scale)
                    .clamp(0.0, self.throttle_target.max(0.0))
            } else if self.throttle_target > 0.75 {
                // Retail: only near full throttle (0x6e6e0).
                (self.steer.abs() * slide_scale + low_speed).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if self.lean < 0.0 {
                spin_target = -(spin_target * self.lean);
            }
            if spin_target > self.wheelspin {
                self.wheelspin += (low_speed + self.throttle_target) * h;
            } else {
                self.wheelspin -= 2.0 * h;
            }
            self.wheelspin = self.wheelspin.clamp(0.0, self.throttle);
        }
        let slip = self.slide.max(self.wheelspin);
        let front_lock = self.brake > p.front_wheel_lock_point;

        // Drive, through the rear wheels.
        let gear = |i: usize| p.gears.get(i).map_or(0.0, |g| g.ratio);
        let mut drive = 0.0;
        let driven = self
            .struts
            .iter()
            .enumerate()
            .any(|(i, s)| i > 1 && s.contact);
        if driven {
            let mut boost = 0.0;
            if p.power_slide_boost > 0.0 {
                boost = lx.abs() / (p.power_slide_boost * MPH_TO_MPS);
                if boost > 1.0 {
                    boost = 1.0 - self.steer.abs();
                }
            }
            drive = (boost + 1.0)
                * (1.0 - by_grip(p.drive_force_slide_scale, grip) * self.slide)
                * (1.0 - by_grip(p.drive_force_spin_scale, grip) * self.wheelspin)
                * gear(self.gear)
                * p.gear_multiplier
                * self.drive_scale
                * p.torque_curve.sample(self.revs)
                * self.throttle
                * self.clutch;
            if self.start == Start::Turbo {
                drive *= p.drive_force_turbo_scale;
            }
            if self.lean < 0.0 && !(p.retail && self.start == Start::Grid) {
                drive *= 1.0 - 2.0 * low_speed * self.lean;
            }
            let engine_brake = (self.revs / p.max_revs - self.throttle)
                * p.engine_friction
                * gear(self.gear).abs();
            if engine_brake > 0.0 {
                drive += if lz <= 0.0 {
                    engine_brake
                } else {
                    -engine_brake
                };
            }
        }

        // Brakes.
        let brake_in = self.brake.max(p.handbrake_force_max * self.slide_handbrake);
        let mut brake = -(1.0 - by_grip(p.brake_force_slide_scale, grip) * self.slide)
            * p.brake_curve
                .sample(lz.abs() * MPS_TO_MPH - p.brake_min_vel)
            * brake_in;
        if speed < MPH_TO_MPS {
            brake = brake / MPH_TO_MPS * speed;
        }
        if lz < 0.0 {
            brake = -brake;
        }

        // Cornering: a yaw torque from the steered wheels' slip angle.
        let mut corner = 0.0;
        if self.steer.abs() > 0.1 {
            let a = p.max_steer_deg.to_radians() * self.steer;
            let slip_angle = dir_z * a.sin() + a.cos() * dir_x - dir_x;
            corner = p.cornering_curve.sample(mph) * by_grip(p.cornering_max, grip) * slip_angle;
            if front_lock {
                corner *= 1.0 - p.wheel_lock_steer_scale;
            }
            let body_speed = self.vel.length();
            if body_speed < MPH_TO_MPS {
                corner *= body_speed / MPH_TO_MPS;
            }
        }
        let steer_drag = if lz > 0.0 {
            -corner.abs() * p.steer_drag_scale
        } else {
            corner.abs() * p.steer_drag_scale
        };

        // Side grip against sliding sideways.
        let mut side = p.side_curve.sample(lx.abs() * MPS_TO_MPH)
            * by_grip(p.side_max, grip)
            * (1.0 - slip * by_grip(p.side_force_slide_scale, grip));
        if lx.abs() < 4.0 * MPH_TO_MPS {
            side = side / (4.0 * MPH_TO_MPS) * lx.abs();
        }
        if lx > 0.0 {
            side = -side;
        }
        if p.retail {
            // Less side grip the steeper the ground.
            side *= g.normal.y * g.normal.y;
        }

        // Yaw damping, relaxed while sliding.
        let low = if speed < 2.234_72 {
            speed * 0.447_483
        } else {
            1.0
        };
        let damp_in = -((1.0 - slip.min(low) * by_grip(p.torque_slide_scale, grip)) * g.ang_vel.y);
        let mut yaw_damp = p.ang_damping_curve.sample(vh * MPS_TO_MPH)
            * by_grip(p.ang_damping_max, grip)
            * damp_in;
        if lz < 0.0 {
            yaw_damp *= if p.retail { 0.8 } else { 0.4 };
        }

        // Air and surface drag.
        let drag_fwd =
            lz.abs() * p.air_resistance * lz * -0.000_625 - lz.abs() * g.drag * lz * 0.000_625;
        let drag_side = g.drag * lx * lx.abs() * -0.000_625;

        // Everything that needs tyres on the ground scales with load.
        let load: f32 = self
            .params
            .wheels
            .iter()
            .zip(&self.struts)
            .filter(|(_, s)| s.contact && s.pos > 0.0)
            .map(|(w, s)| w.stiffness * s.pos)
            .sum();
        let load = (load / GRAVITY).clamp(0.0, 1.0);
        let side = side * load;
        let drive = drive * load;
        let brake = brake * load;

        let frame = Mat3::from_cols(g.side, g.normal, g.fwd);
        let lat = (drag_side + side) * 0.5;
        let lon = (drag_fwd + brake) * 0.5;
        let fp = p.force_point;
        let wb = p.wheelbase;
        let front = (
            frame * Vec3::new(lat, 0.0, lon + steer_drag),
            frame * Vec3::new(0.0, fp, wb),
        );
        let rear = (
            frame * Vec3::new(lat, 0.0, lon + drive),
            frame * Vec3::new(0.0, fp, -wb),
        );
        for (f, at) in [front, rear] {
            self.force += f;
            self.torque += at.cross(f);
        }
        let front_contact = self.struts.iter().take(2).any(|s| s.contact)
            && (!p.retail || lz > 0.0);
        let pitch = (brake + drive) * -0.25
            + if front_contact {
                p.const_mass_torque
            } else {
                0.0
            };
        let t = frame * Vec3::new(pitch, corner * wb + yaw_damp, 0.0);
        let at = frame * Vec3::new(0.0, 0.0, if lz > 0.0 { wb } else { -wb });
        self.torque += t;
        self.force += at.cross(t);
    }

    /// Heavier-than-ground gravity plus steering and lean control in the air.
    fn air_forces(&mut self) {
        let p = &self.params;
        let scale = if self.vel.y > 0.0 {
            p.in_air_grav_up
        } else {
            p.in_air_grav_down
        };
        self.force +=
            Vec3::NEG_Y * p.gravity * p.in_air_grav_scale * scale * self.gravity_scale * p.mass;
        let (right, up, fwd) = (self.rot.x_axis, self.rot.y_axis, self.rot.z_axis);
        self.torque += up * (p.in_air_steer_vel * self.steer - self.ang_vel.dot(up));
        if up.y > -0.3 {
            let roll = -p.in_air_steer_lean * self.steer
                - right.y
                - self.ang_vel.dot(fwd) * p.in_air_steer_lean_damping;
            self.torque += fwd * roll * p.in_air_steer_lean_torque;
        }
        self.torque += right * p.in_air_lean * self.lean;
    }

    /// Hold to compress the suspension, release to hop. Charge builds over
    /// PRELOADRATE, holds for PRELOADHOLDTIME, then fades.
    fn preload_update(&mut self, held: bool, h: f32) {
        let p = &self.params;
        self.gravity_timer -= h;
        if self.gravity_timer <= 0.0 {
            self.gravity_timer = 0.0;
            if self.ground.contacts > 0 {
                self.gravity_scale = 1.0;
            }
        }
        match self.preload_state {
            Preload::Idle => {
                self.preload = 0.0;
                if held {
                    self.preload_state = Preload::Charging;
                }
            }
            Preload::Charging => {
                self.preload += h * p.preload_rate;
                if !held {
                    self.preload_state = Preload::Released;
                } else if self.preload >= 1.0 {
                    self.preload_hold = 0.0;
                    self.preload_state = Preload::Holding;
                }
            }
            Preload::Holding => {
                self.preload = 1.0;
                self.preload_hold += h;
                if !held {
                    self.preload_state = Preload::Released;
                } else if self.preload_hold > p.preload_hold_time {
                    self.preload_state = Preload::Fading;
                }
            }
            Preload::Fading => {
                self.preload -= h * p.preload_fade_rate;
                if !held {
                    self.preload_state = Preload::Released;
                } else if self.preload <= 0.0 {
                    self.preload = 0.0;
                    self.preload_state = Preload::Released;
                }
            }
            Preload::Released => {
                if self.ground.contacts > 0 && self.preload > 0.0 {
                    let t = self.vel.length() * MPS_TO_MPH * 0.01;
                    let push = t * (p.preload_100mph - p.preload_0mph) + p.preload_0mph;
                    // The original adds this as a force for one 60 Hz frame.
                    self.momentum.y += push * self.preload * TICK;
                    let g = t * (p.preload_grav_scale_100mph - p.preload_grav_scale_0mph)
                        + p.preload_grav_scale_0mph;
                    self.gravity_scale = 1.0 - (1.0 - g) * self.preload;
                    self.gravity_timer = 0.5;
                    if p.retail {
                        // Retail's hop also spins the quad: lean and the
                        // pedals pitch it, steering turns it (0x6db90).
                        let pitch = (self.lean - self.throttle + self.brake)
                            * p.preload_vert_spin
                            * self.preload;
                        let yaw = self.steer * p.preload_horiz_spin * self.preload;
                        self.torque += self.rot.x_axis * pitch + self.rot.y_axis * yaw;
                    }
                }
                self.preload = 0.0;
                if !held {
                    self.preload_state = Preload::Idle;
                }
            }
        }
    }

    /// Retail: with only one or two wheels down, a roll torque that grows
    /// with speed brings the quad back level with the ground. It is the
    /// resting state of the original's bicycle-trick code (0x6d1b0), applied
    /// at the front right wheel.
    fn stabiliser(&mut self) {
        if !matches!(self.ground.contacts, 1 | 2) {
            return;
        }
        let p = &self.params;
        let (right, fwd) = (self.rot.x_axis, self.rot.z_axis);
        let roll_rate = self.ang_vel.dot(fwd);
        let t = self.vel.length()
            * p.bicycle_torque
            * ((self.ground.side.y - right.y) - p.bicycle_damping * roll_rate);
        let t = fwd * t;
        let at = self
            .params
            .wheels
            .get(1)
            .map_or(Vec3::ZERO, |w| self.rot * Vec3::from(w.game_offset));
        self.torque += t;
        self.force += at.cross(t);
    }

    /// On the grid only the springs move the quad: no sliding, no turning
    /// (the Dyne's hold at 0xd9190; the position is put back after the step).
    fn pin_to_grid(&mut self) {
        if self.hold.is_none() {
            return;
        }
        self.force = Vec3::new(0.0, self.force.y, 0.0);
        self.momentum = Vec3::new(0.0, self.momentum.y, 0.0);
        self.torque.y = 0.0;
        self.ang_momentum.y = 0.0;
    }

    /// A quad stopped on all four wheels with neither pedal pressed stays
    /// where it is. The original's auto brake (0x1c150) only comes on
    /// below 0.1 mph and its brake force fades to nothing at a standstill,
    /// so on a slope (Forest 1's grid is about 8 degrees) the quad rolled
    /// back at the 1 m/s where the reversing brake catches it, and slid
    /// sideways the same way. Like the grid hold, this cancels the push
    /// along the ground; the springs still work, and a pedal, a jump, a
    /// knock from another quad or a steep slope lets go.
    fn park(&mut self, input: &Input) {
        const PARK_SPEED: f32 = 0.5 * MPH_TO_MPS;
        let idle = input.throttle < 0.2 && input.brake < 0.2 && !input.jump;
        let flat_enough = self.ground.normal.y > 0.85;
        let all_down = self.struts.iter().all(|s| s.contact);
        let slow = self.vel.with_y(0.0).length() < PARK_SPEED;
        self.parked = idle
            && all_down
            && flat_enough
            && self.hold.is_none()
            && !self.in_water
            && (self.parked || slow);
        if !self.parked {
            return;
        }
        // A knock speeds it up past the hold: let it roll.
        if self.vel.with_y(0.0).length() > 2.0 * PARK_SPEED {
            self.parked = false;
            return;
        }
        self.force = Vec3::new(0.0, self.force.y, 0.0);
        self.momentum = Vec3::new(0.0, self.momentum.y, 0.0);
        self.torque.y = 0.0;
        self.ang_momentum.y = 0.0;
    }

    /// Pitch damping that builds up in the air and fades after landing.
    fn landing_stabiliser(&mut self, h: f32) {
        let p = &self.params;
        if self.airborne {
            self.landing_stab = (self.landing_stab
                + h * p.landing_stab_in_air_rate * p.landing_stab_max)
                .min(p.landing_stab_max);
        } else if self.landing_stab > 0.0 {
            self.landing_stab -= p.landing_stab_max * p.landing_stab_decay_rate * h;
            let right = self.rot.x_axis;
            self.torque += right * (-self.ang_vel.dot(right) * self.landing_stab);
        } else {
            self.landing_stab = 0.0;
        }
    }

    fn inv_inertia_world(&self) -> Mat3 {
        self.rot * Mat3::from_diagonal(self.inertia.recip()) * self.rot.transpose()
    }

    fn integrate(&mut self, h: f32, world: &TrackCollision) {
        let p = &self.params;
        self.pos += self.vel * h;
        // Linear and angular resistance act in the body frame and never
        // reverse the motion.
        let resist = |m: &mut Vec3, rot: Mat3, k: Vec3| {
            let dec = rot * ((rot.transpose() * *m) * k * h);
            for i in 0..3 {
                let after = m[i] - dec[i];
                m[i] = if (after < 0.0) == (m[i] < 0.0) {
                    after
                } else {
                    0.0
                };
            }
        };
        resist(&mut self.momentum, self.rot, Vec3::from(p.resistance));
        resist(
            &mut self.ang_momentum,
            self.rot,
            Vec3::from(p.ang_resistance),
        );
        self.momentum += self.force * h;
        self.ang_momentum += self.torque * h;
        let q = Quat::from_mat3(&self.rot);
        let q = (Quat::from_scaled_axis(self.ang_vel * h) * q).normalize();
        self.rot = Mat3::from_quat(q);
        self.vel = self.momentum / p.mass;
        self.ang_vel = self.inv_inertia_world() * self.ang_momentum;
        self.pos += self.pos_fix;
        self.pos_fix = Vec3::ZERO;
        self.force = Vec3::ZERO;
        self.torque = Vec3::ZERO;

        self.body_collision(world);
    }

    /// The body's spheres against the track (0x6fef0), answered the way
    /// the original's contact solver answers them (0xd5090): one pass over
    /// the contacts in order, each pushing the body out of the face and
    /// giving it the impulse that turns its speed into the face around by
    /// the bounce, `-(1 + bounce) × speed in`, at the contact; then friction
    /// for each contact that pushed. A face within 45 degrees of the quad's
    /// up (the ground under its belly) is soft: half the push, a tenth of
    /// the friction and less leverage.
    fn body_collision(&mut self, world: &TrackCollision) {
        let body = self.params.body.clone();
        if self.body_prev.len() != body.spheres.len() {
            self.body_prev = vec![Vec3::NAN; body.spheres.len()];
        }
        let centres: Vec<Vec3> = body.spheres.iter().map(|&(c, _)| self.pos + self.rot * c).collect();
        let travel = self.body_prev.iter().zip(&centres).filter(|(p, _)| p.is_finite()).map(|(p, c)| p.distance(*c)).fold(0.0, f32::max);
        let near = world.near(self.pos * FLIP, body.reach + travel + 0.05);
        let up = self.rot.y_axis;
        let mut contacts = Vec::new();
        for (k, (&(_, radius), &centre)) in body.spheres.iter().zip(&centres).enumerate() {
            let prev = self.body_prev[k];
            for &i in &near {
                let Some(hit) = world.face_contact(i, centre * FLIP, prev * FLIP, radius) else { continue };
                let n = hit.normal * FLIP;
                let surface = world.surface(hit.surface);
                let mut c = Contact {
                    at: centre - n * radius - self.pos,
                    normal: n,
                    depth: hit.t,
                    friction: surface.friction * self.params.friction,
                    bounce: surface.hardness * self.params.hardness,
                    lever: (n.y.abs() + 1.0) * 0.5,
                };
                if n.dot(up) > std::f32::consts::FRAC_1_SQRT_2 {
                    c.lever = 0.4;
                    c.bounce = -0.5;
                    c.friction *= 0.1;
                }
                contacts.push(c);
            }
        }
        let fix = self.solve(&contacts);
        self.pos += fix;
        for (p, c) in self.body_prev.iter_mut().zip(&centres) {
            *p = *c + fix;
        }
    }

    /// The original's contact solver for the body's contacts with fixed
    /// things (0xd5090). Returns the position fix, each contact's push
    /// combined per axis as [`Self::add_fix`] does.
    fn solve(&mut self, contacts: &[Contact]) -> Vec3 {
        let mut fix = Vec3::ZERO;
        let mut pushed = Vec::with_capacity(contacts.len());
        for c in contacts {
            Self::add_fix(&mut fix, c.normal * c.depth);
            let vn = (self.vel + self.ang_vel.cross(c.at)).dot(c.normal);
            let k = self.inv_mass_at(c.at, c.normal);
            let j = if k.abs() > 1e-6 { -(1.0 + c.bounce) * vn / k } else { 0.0 };
            self.impulse(c.normal * j, c.at * c.lever);
            pushed.push(j);
        }
        // The slides are measured once every push is in.
        let slides: Vec<Vec3> = contacts
            .iter()
            .map(|c| {
                let v = self.vel + self.ang_vel.cross(c.at);
                v - c.normal * v.dot(c.normal)
            })
            .collect();
        for ((c, &j), &slide) in contacts.iter().zip(&pushed).zip(&slides) {
            if j > 0.0 && c.friction > 0.0 {
                self.friction(c, slide, c.friction * j);
            }
        }
        fix
    }

    /// Friction against the slide `slide` at a contact: up to `max`, as
    /// much as stops it (0xd5090's second pass).
    fn friction(&mut self, c: &Contact, slide: Vec3, max: f32) {
        let speed = slide.length();
        if speed < 0.01 {
            return;
        }
        let t = slide / speed;
        let k = self.inv_mass_at(c.at, t);
        let j = if speed < max * k { speed / k } else { max };
        self.impulse(-t * j, c.at * c.lever);
    }

    /// How easily the body moves along `n` at `at` (from the centre of
    /// mass): the inverse of its effective mass there (0xd2040).
    fn inv_mass_at(&self, at: Vec3, n: Vec3) -> f32 {
        1.0 / self.params.mass + n.dot((self.inv_inertia_world() * at.cross(n)).cross(at))
    }

    /// An impulse `j` at `at` (from the centre of mass), then the
    /// velocities that follow (0xd1410).
    fn impulse(&mut self, j: Vec3, at: Vec3) {
        self.momentum += j;
        self.ang_momentum += at.cross(j);
        self.vel = self.momentum / self.params.mass;
        self.ang_vel = self.inv_inertia_world() * self.ang_momentum;
    }

    /// The body's spheres placed where it is, game frame.
    fn posed_spheres(&self) -> Vec<(Vec3, f32)> {
        self.params.body.spheres.iter().map(|&(c, r)| (self.pos + self.rot * c, r)).collect()
    }

    /// The body's hull placed where it is, game frame.
    fn posed_hull(&self) -> Vec<(Vec3, f32)> {
        self.params
            .body
            .hull
            .iter()
            .map(|&(n, d)| {
                let n = self.rot * n;
                (n, d - n.dot(self.pos))
            })
            .collect()
    }

    /// The body's spheres where it is now, sim frame: centre and radius.
    pub fn body_spheres(&self) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        let (pos, rot) = (self.position * FLIP, mirror(Mat3::from_quat(self.rotation)));
        self.params.body.spheres.iter().map(move |&(c, r)| ((pos + rot * c) * FLIP, r))
    }

    /// Strut dynamics, engine revs and the gearbox.
    fn post_step(&mut self, h: f32) {
        if self.ground.contacts == 0 {
            self.air_time += h;
            // Retail stays in its turbo state through a hop (0x6d490).
            self.airborne = self.air_time > AIRBORNE_DELAY
                && !(self.params.retail && self.start == Start::Turbo);
        } else {
            self.air_time = 0.0;
            self.airborne = false;
        }
        self.local_acc = self.rot.transpose() * ((self.vel - self.prev_vel) / h);
        self.prev_vel = self.vel;

        for (i, wp) in self.params.wheels.iter().enumerate() {
            let s = &mut self.struts[i];
            if !s.contact {
                // A hanging wheel springs back toward its rest position.
                let mut damp = wp.damping * s.vel;
                if (s.vel < 0.0 && h * damp < wp.mass * s.vel)
                    || (s.vel > 0.0 && wp.mass * s.vel < h * damp)
                {
                    s.vel = 0.0;
                    damp = 0.0;
                }
                s.force -= damp + wp.stiffness * s.pos;
            }
            s.vel = (s.vel + s.force / wp.mass * h).clamp(-wp.max_vel, wp.max_vel);
            s.pos = (s.pos + h * s.vel).clamp(wp.min_pos, wp.max_pos);
        }

        let p = &self.params;
        let ratio = |i: usize| p.gears.get(i).map_or(0.0, |g| g.ratio);
        let max_w = p.max_revs * 0.104_72;
        let wheel_w = ratio(self.gear_target) * p.gear_multiplier * self.ground_fwd;
        let wheel_w = (max_w - wheel_w) * self.wheelspin + wheel_w;
        let free_w = self.throttle * max_w;
        let w = if self.ground.contacts == 0 || (p.retail && self.start == Start::Grid) {
            free_w
        } else if self.gear_target == 1 {
            let s = (1.0 - self.clutch).max(0.0).sqrt();
            s * free_w + (1.0 - s) * wheel_w
        } else {
            wheel_w
        };
        if p.retail {
            // Retail limits how fast the target moves, in RPM per second
            // (slower on the grid), then eases the revs toward it (0x6e2e0).
            let rate = if self.start == Start::Grid {
                p.rev_rate_revving
            } else {
                p.rev_rate
            };
            let lo = self.revs - rate * h;
            let hi = (self.revs + rate * h).min(p.max_revs);
            let target = w * RAD_TO_RPM;
            let target = if target < lo { lo } else { target.min(hi) }.max(p.min_revs);
            self.revs += (target - self.revs) * (RETAIL_REV_LERP * h).min(1.0);
        } else {
            let target = (w * RAD_TO_RPM).max(p.min_revs);
            self.revs += (target - self.revs) * (p.rev_rate * h).min(1.0);
        }

        // Automatic gearbox: picks the gear with the most usable torque.
        let mut shift = None;
        if !self.reverse && p.gears.len() > 1 {
            let mut best = (1, 0.0);
            for (i, g) in p.gears.iter().enumerate().skip(1) {
                let rpm = g.ratio * p.gear_multiplier * self.ground_fwd * RAD_TO_RPM;
                if rpm > 0.0 {
                    let x = rpm / p.max_revs;
                    let score = (1.0 - x * x * x) * p.torque_curve.sample(rpm) * g.ratio;
                    if score >= best.1 {
                        best = (i, score);
                    }
                }
            }
            if best.0 != self.gear_target
                && best.0 != self.gear
                && p.gear_wait_time < self.since_shift
            {
                shift = Some(best.0);
            }
        }
        let (change_time, gear_mult) = (p.gear_change_time, p.gear_multiplier);
        let ratio = ratio(self.gear);
        if let Some(g) = shift {
            self.shift_to(g);
        }
        if self.shifting {
            self.shift_timer += h;
            if self.shift_timer > change_time {
                self.gear = self.gear_target;
                self.shift_timer = 0.0;
                self.shifting = false;
            }
        }
        self.since_shift += h;
        let mut clutch_target = 0.0;
        if !self.shifting {
            clutch_target = if self.throttle != 0.0 {
                1.0
            } else {
                let x = ratio * gear_mult * self.ground_fwd * RAD_TO_RPM / self.revs;
                x.clamp(0.0, 1.0)
            };
        }
        let rate = h / change_time.max(1e-3);
        self.clutch = if clutch_target > self.clutch {
            (self.clutch + rate).min(clutch_target)
        } else {
            (self.clutch - rate).max(clutch_target)
        };
    }

    /// Copies the game-frame state out to the public sim-frame fields.
    /// How far between the last two ticks the pose `ahead` seconds after
    /// the last call to [`Vehicle::step`] is.
    fn blend(&self, ahead: f32) -> f32 {
        ((self.pending + ahead.max(0.0)) / self.span).clamp(0.0, 1.0)
    }

    /// The pose to draw `ahead` seconds after the last call to
    /// [`Vehicle::step`] (the time the caller has let pass since): between
    /// the last two ticks, as `position` and `rotation` are, but placed by
    /// the time of the frame drawn, so the quad moves smoothly at any frame
    /// rate rather than in the caller's steps.
    pub fn drawn(&self, ahead: f32) -> (Vec3, Quat) {
        let t = self.blend(ahead);
        let (from, to) = (Quat::from_mat3(&self.shown_rot), Quat::from_mat3(&self.rot));
        let rotation = Quat::from_mat3(&mirror(Mat3::from_quat(from.slerp(to, t).normalize()))).normalize();
        (self.shown_pos.lerp(self.pos, t) * FLIP, rotation)
    }

    /// Wheel `i` as drawn `ahead` seconds after the last step, as
    /// [`Vehicle::drawn`] places the body.
    pub fn drawn_wheel(&self, i: usize, ahead: f32) -> Option<WheelPose> {
        let (from, to) = (self.shown_wheels.get(i)?, self.wheel_pose(i)?);
        let t = self.blend(ahead);
        // The roll wraps at a full turn.
        let roll = (to.spin - from.spin + PI).rem_euclid(TAU) - PI;
        Some(WheelPose {
            compression: from.compression + (to.compression - from.compression) * t,
            spin: (from.spin + roll * t).rem_euclid(TAU),
            steer: from.steer + (to.steer - from.steer) * t,
        })
    }

    fn wheel_pose(&self, i: usize) -> Option<WheelPose> {
        let (wp, s, ws) = (self.params.wheels.get(i)?, self.struts.get(i)?, self.wheels.get(i)?);
        let steer = if wp.front { -self.steer * self.params.max_steer_deg.to_radians() } else { 0.0 };
        Some(WheelPose { compression: s.pos - wp.min_pos, spin: ws.spin, steer })
    }

    fn wheel_poses(&self) -> Vec<WheelPose> {
        (0..self.params.wheels.len()).filter_map(|i| self.wheel_pose(i)).collect()
    }

    fn sync_out(&mut self) {
        // The pose the last tick left, which the game plays by; drawing
        // blends towards it (see `drawn`).
        self.position = self.pos * FLIP;
        self.rotation = Quat::from_mat3(&mirror(self.rot)).normalize();
        self.velocity = self.vel * FLIP;
        // Angular velocity is an axial vector: mirroring flips its sign.
        self.angular_velocity = -(self.ang_vel * FLIP);
        let max_steer = self.params.max_steer_deg.to_radians();
        for (i, wp) in self.params.wheels.iter().enumerate() {
            let s = &self.struts[i];
            let ws = &mut self.wheels[i];
            ws.compression = s.pos - wp.min_pos;
            ws.contact = s.contact;
            ws.surface = s.surface;
            ws.contact_normal = s.normal * FLIP;
            ws.steer = if wp.front {
                -self.steer * max_steer
            } else {
                0.0
            };
        }
    }

    fn spin_wheels(&mut self) {
        let roll = self.ground_fwd;
        for (ws, wp) in self.wheels.iter_mut().zip(&self.params.wheels) {
            ws.spin = (ws.spin + roll / wp.radius * TICK).rem_euclid(std::f32::consts::TAU);
        }
    }

    /// Body orientation as a matrix (for rendering).
    pub fn basis(&self) -> Mat3 {
        Mat3::from_quat(self.rotation)
    }
}
