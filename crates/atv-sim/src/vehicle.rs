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

use glam::{Mat3, Quat, Vec3};

use crate::collision::TrackCollision;
use crate::params::{VehicleParams, by_grip};

pub const GRAVITY: f32 = 25.0;
const MPS_TO_MPH: f32 = 2.237_41;
const MPH_TO_MPS: f32 = 0.446_944;
/// Radians per second -> revs per minute.
const RAD_TO_RPM: f32 = 9.549_3;
/// The original steps once per frame at 60 Hz; sub-stepping finer keeps the
/// suspension stable at any render rate without changing the feel.
const STEP: f32 = 1.0 / 120.0;
/// Pre-load is applied by the original as a one-frame force at 60 Hz.
const ORIGINAL_FRAME: f32 = 1.0 / 60.0;
/// Seconds without wheel contact before the quad counts as airborne.
const AIRBORNE_DELAY: f32 = 0.15;
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

#[derive(Debug, Clone, Copy, Default)]
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
    /// Sim-frame pose and motion (mirrors of the internal game-frame state).
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
    /// Where each body sphere was last step, for catching tunnelling.
    body_prev: [Vec3; 5],

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
            body_prev: [Vec3::NAN; 5],
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
        self.body_prev = [Vec3::NAN; 5];
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
        self.sync_out();
    }

    /// Advances the simulation by `dt` seconds (internally sub-stepped).
    pub fn step(&mut self, input: &Input, dt: f32, world: &TrackCollision) {
        let n = (dt / STEP).ceil().clamp(1.0, 32.0) as usize;
        let h = dt.min(0.1) / n as f32;
        for _ in 0..n {
            self.substep(input, h, world);
        }
        self.sync_out();
    }

    fn substep(&mut self, input: &Input, h: f32, world: &TrackCollision) {
        self.controls(input, h);
        self.wheel_contacts(world);
        self.ground_frame();
        self.springs();
        if self.ground.contacts > 0 {
            let right = self.rot.x_axis;
            self.torque += right * self.lean * self.params.lean_force;
            self.wheelies();
            self.force += Vec3::NEG_Y * GRAVITY * self.params.mass;
            self.ground_forces(h);
        } else {
            self.air_forces();
        }
        self.preload_update(input.jump, h);
        self.landing_stabiliser(h);
        self.integrate(h, world);
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
    ) -> Option<(f32, Vec3, Vec3)> {
        world
            .raycast(origin * FLIP, dir * FLIP, max)
            .map(|h| (h.t, h.point * FLIP, h.normal * FLIP))
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
            let Some((t, _, n)) = Self::cast(
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
        let mut g = Ground {
            grip: 1.0,
            ..Default::default()
        };
        let mut best = -2.0;
        for s in self.struts.iter().filter(|s| s.contact) {
            g.contacts += 1;
            let d = s.normal.dot(up);
            if d > best {
                best = d;
                g.normal = s.normal;
            }
        }
        if g.contacts > 0 {
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
            let lift = p.wheelie_torque * self.lean * acc;
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
        let mut spin_target = ((self.steer.abs() + low_speed) * slide_scale)
            .clamp(0.0, self.throttle_target.max(0.0));
        if self.lean < 0.0 {
            spin_target = -(spin_target * self.lean);
        }
        if spin_target > self.wheelspin {
            self.wheelspin += (low_speed + self.throttle_target) * h;
        } else {
            self.wheelspin -= 2.0 * h;
        }
        self.wheelspin = self.wheelspin.clamp(0.0, self.throttle);
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
                * p.torque_curve.sample(self.revs)
                * self.throttle
                * self.clutch;
            if self.lean < 0.0 {
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
            yaw_damp *= 0.4;
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
        let front_contact = self.struts.iter().take(2).any(|s| s.contact);
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
            Vec3::NEG_Y * GRAVITY * p.in_air_grav_scale * scale * self.gravity_scale * p.mass;
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
                    self.momentum.y += push * self.preload * ORIGINAL_FRAME;
                    let g = t * (p.preload_grav_scale_100mph - p.preload_grav_scale_0mph)
                        + p.preload_grav_scale_0mph;
                    self.gravity_scale = 1.0 - (1.0 - g) * self.preload;
                    self.gravity_timer = 0.5;
                }
                self.preload = 0.0;
                if !held {
                    self.preload_state = Preload::Idle;
                }
            }
        }
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

    /// Spheres around the chassis keep it out of the ground and walls. The
    /// original collides body spheres with the world and answers each
    /// contact with a position fix plus an impulse at the contact point;
    /// this does the same for a hand-placed set covering the Bandit-sized
    /// chassis (positions from the model's hard points, relative to the
    /// centre of mass, game frame).
    fn body_collision(&mut self, world: &TrackCollision) {
        const SPHERES: [([f32; 3], f32); 5] = [
            ([0.0, 0.05, 0.0], 0.22),  // belly
            ([0.0, 0.3, 0.75], 0.25),  // front rack
            ([0.0, 0.7, 0.3], 0.25),   // handlebars
            ([0.0, 0.5, -0.35], 0.3),  // seat
            ([0.0, 0.4, -0.85], 0.25), // rear rack
        ];
        const BOUNCE: f32 = 0.1;
        const FRICTION: f32 = 0.6;
        let mass = self.params.mass;
        for (k, &(local, radius)) in SPHERES.iter().enumerate() {
            let r = self.rot * Vec3::from(local);
            let centre = self.pos + r;
            let prev = self.body_prev[k];
            self.body_prev[k] = centre;
            // Surfaces are thin shells, so a centre that crossed one (moving
            // fast, or shoved by another contact) would be pushed out the
            // wrong side: catch that with a ray from where it was.
            let travel = centre - prev;
            let hit = match (prev.is_finite() && travel.length() > 1e-4)
                .then(|| Self::cast(world, prev, travel.normalize(), travel.length()))
                .flatten()
            {
                Some((t, _, n)) => Some((n, travel.length() - t + radius)),
                None => world
                    .sphere(centre * FLIP, radius)
                    .map(|h| (h.normal * FLIP, h.t)),
            };
            let Some((n, depth)) = hit else { continue };
            let push = n * depth;
            self.pos += push;
            self.body_prev[k] += push;
            // Impulse that stops the point moving into the surface, with
            // a little bounce and Coulomb friction along it.
            let inv_i = self.inv_inertia_world();
            let v_point = self.vel + self.ang_vel.cross(r);
            let vn = v_point.dot(n);
            if vn >= 0.0 {
                continue;
            }
            let k_n = 1.0 / mass + n.dot((inv_i * r.cross(n)).cross(r));
            let jn = -(1.0 + BOUNCE) * vn / k_n;
            let vt = v_point - n * vn;
            let mut j = n * jn;
            if vt.length() > 1e-4 {
                let t = vt.normalize();
                let k_t = 1.0 / mass + t.dot((inv_i * r.cross(t)).cross(r));
                let jt = (vt.length() / k_t).min(FRICTION * jn);
                j -= t * jt;
            }
            self.momentum += j;
            self.ang_momentum += r.cross(j);
            self.vel = self.momentum / mass;
            self.ang_vel = inv_i * self.ang_momentum;
        }
    }

    /// Strut dynamics, engine revs and the gearbox.
    fn post_step(&mut self, h: f32) {
        if self.ground.contacts == 0 {
            self.air_time += h;
            self.airborne = self.air_time > AIRBORNE_DELAY;
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
        let w = if self.ground.contacts == 0 {
            free_w
        } else if self.gear_target == 1 {
            let s = (1.0 - self.clutch).max(0.0).sqrt();
            s * free_w + (1.0 - s) * wheel_w
        } else {
            wheel_w
        };
        let target = (w * RAD_TO_RPM).max(p.min_revs);
        self.revs += (target - self.revs) * (p.rev_rate * h).min(1.0);

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
    fn sync_out(&mut self) {
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
            ws.contact_normal = s.normal * FLIP;
            ws.steer = if wp.front {
                -self.steer * max_steer
            } else {
                0.0
            };
        }
        let roll = self.ground_fwd;
        for (ws, wp) in self.wheels.iter_mut().zip(&self.params.wheels) {
            ws.spin =
                (ws.spin + roll / wp.radius * ORIGINAL_FRAME).rem_euclid(std::f32::consts::TAU);
        }
    }

    /// Body orientation as a matrix (for rendering).
    pub fn basis(&self) -> Mat3 {
        Mat3::from_quat(self.rotation)
    }
}
