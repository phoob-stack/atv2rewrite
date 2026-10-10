//! Raycast-wheel quad bike model driven by the game's own tuning values.
//!
//! The original "Dyne" engine normalises mass to 1, so forces in the
//! `.params` files read directly as accelerations. Where the files don't
//! say how a value is applied (the original code isn't decoded yet), the
//! scale constants below were tuned by feel and are marked `TUNE`.

use glam::{Mat3, Quat, Vec3};

use crate::collision::TrackCollision;
use crate::params::{sample_curve, VehicleParams};

pub const GRAVITY: f32 = 9.81;
const MPS_TO_MPH: f32 = 2.237;
/// TUNE: engine revs per (m/s x gear ratio).
const REVS_PER_MPS: f32 = 120.0;
/// TUNE: engine torque x ratio x GEARMULTIPLIER -> m/s^2.
const DRIVE_SCALE: f32 = 0.7;
/// TUNE: AIRRESISTANCE -> quadratic drag coefficient.
const DRAG_SCALE: f32 = 0.0016;
/// TUNE: SIDEFORCEMAX -> lateral grip (m/s^2 per axle).
const SIDE_SCALE: f32 = 0.45;
/// TUNE: lateral slip stiffness (1/s).
const SLIP_STIFFNESS: f32 = 14.0;
/// TUNE: PRELOAD force -> vertical velocity (m/s).
const PRELOAD_SCALE: f32 = 0.012;

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
    /// Distance from full extension, metres (0 = hanging).
    pub compression: f32,
    pub contact: bool,
    pub contact_normal: Vec3,
    /// Rolling angle for rendering, radians.
    pub spin: f32,
    /// Current steering angle (front wheels), radians.
    pub steer: f32,
}

#[derive(Debug, Clone)]
pub struct Vehicle {
    pub params: VehicleParams,
    pub position: Vec3,
    pub rotation: Quat,
    pub velocity: Vec3,
    pub angular_velocity: Vec3,
    pub wheels: Vec<WheelState>,
    pub steer: f32,
    pub throttle: f32,
    /// Index into `params.gears` (0 = reverse).
    pub gear: usize,
    pub revs: f32,
    pub preload: f32,
    pub air_time: f32,
    inertia: Vec3,
}

impl Vehicle {
    pub fn new(params: VehicleParams, position: Vec3, rotation: Quat) -> Self {
        let [w, h, l] = params.dims;
        let m = params.mass;
        let inertia = Vec3::new(m * (h * h + l * l) / 12.0, m * (w * w + l * l) / 12.0, m * (w * w + h * h) / 12.0);
        let wheels = vec![WheelState::default(); params.wheels.len()];
        Self {
            position,
            rotation,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            wheels,
            steer: 0.0,
            throttle: 0.0,
            gear: 1,
            revs: params.min_revs,
            preload: 0.0,
            air_time: 0.0,
            inertia,
            params,
        }
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
        self.wheels.iter().any(|w| w.contact)
    }

    /// Puts the quad back at `position`, at rest.
    pub fn reset(&mut self, position: Vec3, rotation: Quat) {
        self.position = position;
        self.rotation = rotation;
        self.velocity = Vec3::ZERO;
        self.angular_velocity = Vec3::ZERO;
        self.gear = 1;
        self.preload = 0.0;
    }

    /// Advances the simulation by `dt` seconds (internally sub-stepped).
    pub fn step(&mut self, input: &Input, dt: f32, world: &TrackCollision) {
        let sub = (dt / (1.0 / 240.0)).ceil().max(1.0) as usize;
        let h = dt / sub as f32;
        self.update_controls(input, dt);
        for _ in 0..sub {
            self.substep(input, h, world);
        }
    }

    fn update_controls(&mut self, input: &Input, dt: f32) {
        let p = &self.params;
        let approach = |cur: f32, target: f32, rate: f32| cur + (target - cur).clamp(-rate * dt, rate * dt);
        let steer_rate = if input.steer.abs() > self.steer.abs() { p.steer_rate } else { p.steer_return_rate };
        self.steer = approach(self.steer, input.steer.clamp(-1.0, 1.0), steer_rate);
        let acc_rate = if input.throttle > self.throttle { p.acc_rate } else { p.acc_return_rate };
        self.throttle = approach(self.throttle, input.throttle.clamp(0.0, 1.0), acc_rate);

        // Automatic gearbox, with reverse when braking from a standstill.
        let fwd = self.velocity.dot(self.forward());
        if input.brake > 0.5 && input.throttle < 0.1 && fwd.abs() < 1.0 {
            self.gear = 0;
        } else if self.gear == 0 && (input.throttle > 0.1 || fwd > 1.0) {
            self.gear = 1;
        }
        if self.gear >= 1 {
            let g = &p.gears;
            if self.gear + 1 < g.len() && self.revs > g[self.gear].up_revs {
                self.gear += 1;
            } else if self.gear > 1 && self.revs < g[self.gear].down_revs {
                self.gear -= 1;
            }
        }

        // Pre-load: charge while held, release to hop.
        if input.jump {
            self.preload = (self.preload + dt / p.preload_rate.max(0.05)).min(1.0);
        } else if self.preload > 0.0 {
            if self.grounded() {
                let mph = self.speed_mph().abs();
                let force = p.preload_0mph + (p.preload_100mph - p.preload_0mph) * (mph / 100.0).min(1.0);
                self.velocity += self.up() * force * PRELOAD_SCALE * self.preload;
            }
            self.preload = 0.0;
        }
    }

    fn substep(&mut self, input: &Input, h: f32, world: &TrackCollision) {
        let p = self.params.clone();
        let up = self.up();
        let fwd_body = self.forward();
        let fwd_speed = self.velocity.dot(fwd_body);
        let mph = fwd_speed.abs() * MPS_TO_MPH;
        let mut force = Vec3::ZERO;
        let mut torque = Vec3::ZERO;

        // Engine.
        let gear = p.gears.get(self.gear).map_or(1.0, |g| g.ratio);
        self.revs = (fwd_speed.abs() * gear.abs() * REVS_PER_MPS).clamp(p.min_revs, p.max_revs);
        let torque_frac = sample_curve(&p.torque_curve, self.revs - p.min_revs, p.max_revs - p.min_revs);
        let rev_limit = if self.revs >= p.max_revs * 0.999 { 0.0 } else { 1.0 };
        let drive = torque_frac * p.torque_max * gear * p.gear_multiplier * DRIVE_SCALE * rev_limit;
        let drive = drive * if self.gear == 0 { input.brake } else { self.throttle };

        let mut lift: f32 = 0.0;
        let rear_count = p.wheels.iter().filter(|w| !w.front).count().max(1) as f32;
        let axle_count = |front: bool| p.wheels.iter().filter(|w| w.front == front).count().max(1) as f32;
        let side_curve_f = sample_curve(&p.side_curve, mph, p.side_vel_max);
        let max_steer = p.max_steer_deg.to_radians() * (1.0 - 0.6 * (mph / 85.0).min(1.0));

        for (i, wp) in p.wheels.iter().enumerate() {
            let anchor = self.position + self.rotation * Vec3::from(wp.offset);
            let travel = wp.max_pos - wp.min_pos;
            // Cast from well above the top of travel so a wheel can never
            // start its ray already inside a steep rise.
            const HEADROOM: f32 = 1.0;
            let origin = anchor + up * (wp.max_pos + HEADROOM);
            let reach = HEADROOM + travel + wp.radius;
            let ws = &mut self.wheels[i];
            ws.steer = if wp.front { -self.steer * max_steer } else { 0.0 };
            let hit = world.raycast(origin, -up, reach);
            let Some(hit) = hit else {
                ws.contact = false;
                ws.compression = 0.0;
                continue;
            };
            let prev = ws.compression;
            let comp = (reach - hit.t).clamp(0.0, travel + HEADROOM);
            if comp > travel {
                // Past the bump stop: lift the chassis out instead of sinking.
                lift = lift.max(comp - travel);
            }
            ws.compression = comp;
            ws.contact = true;
            ws.contact_normal = hit.normal;
            let comp_vel = (comp - prev) / h;

            // Suspension, with a stiff bump stop past full travel.
            let mut spring = wp.stiffness * comp + wp.damping * comp_vel;
            if comp > travel {
                spring += (comp - travel) * 200.0;
            }
            let spring = spring.max(0.0);
            let load = (spring / (GRAVITY / p.wheels.len() as f32)).clamp(0.0, 3.0);
            let contact = hit.point;
            let r = contact - self.position;
            let f = up * spring;

            // Tyre frame on the contact plane.
            let n = hit.normal;
            let wheel_fwd = Quat::from_axis_angle(up, ws.steer) * fwd_body;
            let wf = (wheel_fwd - n * wheel_fwd.dot(n)).normalize_or_zero();
            let side = n.cross(wf).normalize_or_zero();
            let v = self.velocity + self.angular_velocity.cross(r);
            let v_side = v.dot(side);
            let v_fwd = v.dot(wf);

            let axle = if wp.front { 0 } else { 1 };
            let lat_max = p.side_max[axle] * SIDE_SCALE * side_curve_f * load / axle_count(wp.front);
            let lat = (-v_side * SLIP_STIFFNESS / p.wheels.len() as f32).clamp(-lat_max, lat_max);

            let mut lon = 0.0;
            if !wp.front || self.gear == 0 {
                lon += drive / rear_count * if self.gear == 0 { -1.0 } else { 1.0 };
            }
            if self.gear != 0 && input.brake > 0.0 {
                lon -= v_fwd.signum() * (p.brake_force_max / p.wheels.len() as f32) * input.brake * (v_fwd.abs() * 2.0).min(1.0);
            }
            // Rolling resistance, plus an engine-braking hold when coasting
            // slowly so a parked quad doesn't creep down slopes.
            lon -= v_fwd * 0.08;
            if self.throttle < 0.05 && input.brake < 0.05 && v_fwd.abs() < 3.0 {
                lon -= v_fwd * 4.0;
            }
            let grip = p.friction * 2.2 * GRAVITY / p.wheels.len() as f32 * load;
            let lon = lon.clamp(-grip, grip);

            // Tyre forces act at FORCEPOINT height, not at the contact patch,
            // as in the original: this is what keeps the quad from rolling
            // over or wheelying under every bit of grip.
            let tyre = side * lat + wf * lon;
            let r_tyre = r - up * (r.dot(up) - p.force_point);
            force += f + tyre;
            torque += r.cross(f) + r_tyre.cross(tyre);
            ws.spin += v_fwd / wp.radius * h;
        }

        if lift > 0.0 {
            self.position += up * lift * 0.5;
            let vn = self.velocity.dot(up);
            if vn < 0.0 {
                self.velocity -= up * vn * 0.5;
            }
        }
        let grounded = self.grounded();
        let mut g = GRAVITY;
        if grounded {
            self.air_time = 0.0;
        } else {
            self.air_time += h;
            g *= if self.velocity.y > 0.0 { p.in_air_grav_up } else { p.in_air_grav_down } * 0.5 + 0.5;
        }
        force += Vec3::NEG_Y * g * p.mass;
        force -= self.velocity * self.velocity.length() * p.air_resistance * DRAG_SCALE;

        let right = self.right();
        if grounded {
            // Front/rear weight bias and rider lean (wheelies / endos).
            torque += right * (p.const_mass_torque * 0.05 - input.lean * 1.5);
            // Self-righting toward the ground normal (landing stabiliser),
            // plus roll damping so bumps don't tip the quad over.
            let avg_n = self.wheels.iter().filter(|w| w.contact).map(|w| w.contact_normal).sum::<Vec3>().normalize_or(Vec3::Y);
            torque += up.cross(avg_n) * p.landing_stab_max * 0.5;
            let fwd_axis = self.forward();
            torque -= fwd_axis * self.angular_velocity.dot(fwd_axis) * 3.0;
        } else {
            // Air control: steer to yaw, lean to pitch, gentle auto-level roll.
            torque += up * (-input.steer * p.in_air_steer_vel * 0.6 - self.angular_velocity.dot(up) * 2.0);
            torque += right * (-input.lean * p.in_air_lean * 0.6);
            torque += up.cross(Vec3::Y) * 4.0;
        }

        // Integrate (body-frame inertia).
        self.velocity += force / p.mass * h;
        let inv_rot = self.rotation.inverse();
        let t_body = inv_rot * torque;
        let mut w_body = inv_rot * self.angular_velocity;
        w_body += t_body / self.inertia * h;
        // Angular resistance (body x/z), always applied.
        let res = Vec3::from(p.ang_resistance);
        w_body *= Vec3::ONE - (res * h).min(Vec3::ONE);
        self.angular_velocity = self.rotation * w_body;
        self.position += self.velocity * h;
        let w = self.angular_velocity;
        if w.length_squared() > 0.0 {
            self.rotation = (Quat::from_scaled_axis(w * h) * self.rotation).normalize();
        }

        // Keep the chassis out of the ground if it lands on its belly.
        let probe = world.raycast(self.position + Vec3::Y * 4.0, Vec3::NEG_Y, 4.0 + 0.25);
        if let Some(hit) = probe {
            let floor = hit.point.y + 0.25;
            if self.position.y < floor {
                self.position.y = floor;
                if self.velocity.y < 0.0 {
                    self.velocity.y *= -0.1;
                }
                self.velocity *= 1.0 - (3.0 * h).min(1.0);
            }
        }
    }

    /// Body orientation as a matrix (for rendering).
    pub fn basis(&self) -> Mat3 {
        Mat3::from_quat(self.rotation)
    }
}
