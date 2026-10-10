//! Computer-controlled racers, ported from the original game's AI driver.
//!
//! The track's `.aid` racing line (see `atv_formats::aid`) is a graph of
//! nodes with a drivable corridor either side; each link between two nodes
//! is an [`Edge`]. Each frame the original driver (update at 0x69150, look
//! ahead at 0x68e60) walks forward along the edges from where the quad is:
//!
//! - **Steering target** (0x667b0, 0x68a90): the middle of the corridor a
//!   look-ahead distance down the line. The look-ahead is the corridor
//!   width, but at least 1.05 s of travel (0x66730). Other quads ahead in
//!   the way push the target sideways into the free part of the corridor
//!   (0x67050, 0x671e0).
//! - **Target speed** (0x68760, 0x67f70): every node within braking
//!   distance (v^2 / 2a, at most 200 m; 0x67e40) caps the speed to the one
//!   from which the quad can still slow to the node's limit,
//!   `sqrt(limit^2 + 2 a d)`, where `a` is 80% of the quad's maximum brake.
//! - **Controls** (0x68000, 0x68430, 0x67520): a PID on the sine of the
//!   angle to the target steers (gains 1, 0.2, 0.01); a PID on the speed
//!   error gives throttle when positive and brake when negative (gains
//!   0.04, 0.1, 0).
//! - **Recovery** (0x67b40): a quad stuck below 8 mph backs up with the
//!   steering reversed, then tries again; after repeated failures, or when
//!   it ends up far off the line, it asks to be respotted.
//!
//! Positions are in the sim frame (right-handed, -Z forward).

use glam::{Quat, Vec3};

use atv_formats::aid::AiPath;

use crate::track::to_sim;
use crate::{Input, Vehicle, VehicleParams};

/// Speeds below this count as stopped for stuck detection (8 mph).
const STUCK_SPEED: f32 = 3.575_56;
/// Original "no limit" node speed, m/s.
const NO_LIMIT: f32 = 1000.0;
const MAX_BRAKE_LOOKAHEAD: f32 = 200.0;
/// Rough quad half-width for passing, metres.
const QUAD_HALF_WIDTH: f32 = 1.0;
/// A driver that stays within this many metres for this many seconds is
/// respotted.
const PROGRESS_RADIUS: f32 = 10.0;
const PROGRESS_WINDOW: f32 = 8.0;
/// Jump nodes only get a hop from quads at least this fast (40 mph).
const HOP_SPEED: f32 = 17.877_8;

#[derive(Debug, Clone)]
pub struct LineNode {
    pub pos: Vec3,
    /// Corridor edges along the outgoing edge's side vector (`left` < 0).
    pub left: f32,
    pub right: f32,
    pub speed_limit: f32,
    pub respot: bool,
    /// Jump node: how hard (0-100) the AI hops off it.
    pub boost: u8,
    /// Outgoing and incoming edge indices.
    pub out: Vec<usize>,
    pub inc: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    /// Unit direction from `from` to `to`.
    pub dir: Vec3,
    /// `dir x up`: points to the right of the line, shorter on slopes like
    /// the original's.
    pub side: Vec3,
    pub len: f32,
}

/// A track's AI racing line.
#[derive(Debug, Clone)]
pub struct RacingLine {
    pub nodes: Vec<LineNode>,
    pub edges: Vec<Edge>,
}

impl RacingLine {
    pub fn from_path(path: &AiPath) -> Self {
        let mut nodes: Vec<LineNode> = path
            .nodes
            .iter()
            .map(|n| LineNode {
                pos: to_sim(n.pos),
                left: n.left,
                right: n.right,
                speed_limit: n.speed_limit.unwrap_or(NO_LIMIT),
                respot: n.respot(),
                boost: n.boost,
                out: Vec::new(),
                inc: Vec::new(),
            })
            .collect();
        let mut edges = Vec::new();
        for (from, n) in path.nodes.iter().enumerate() {
            for &to in &n.next {
                let d = nodes[to].pos - nodes[from].pos;
                let len = d.length();
                if len < 1e-3 {
                    continue;
                }
                let dir = d / len;
                nodes[from].out.push(edges.len());
                nodes[to].inc.push(edges.len());
                edges.push(Edge {
                    from,
                    to,
                    dir,
                    side: dir.cross(Vec3::Y),
                    len,
                });
            }
        }
        Self { nodes, edges }
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    pub fn point(&self, edge: usize, t: f32) -> Vec3 {
        let e = &self.edges[edge];
        self.nodes[e.from].pos.lerp(self.nodes[e.to].pos, t)
    }

    /// Corridor edges at `t` along `edge` (0x1eb0 / 0x1ed0 in the base class).
    pub fn corridor(&self, edge: usize, t: f32) -> (f32, f32) {
        let e = &self.edges[edge];
        let (a, b) = (&self.nodes[e.from], &self.nodes[e.to]);
        (
            a.left + (b.left - a.left) * t,
            a.right + (b.right - a.right) * t,
        )
    }

    /// Where `p` projects onto `edge`: (t unclamped, squared distance to
    /// the clamped point).
    fn project(&self, edge: usize, p: Vec3) -> (f32, f32) {
        let e = &self.edges[edge];
        let t = (p - self.nodes[e.from].pos).dot(e.dir) / e.len;
        (t, self.point(edge, t.clamp(0.0, 1.0)).distance_squared(p))
    }

    /// The edge nearest to `p`, and how far along it.
    pub fn locate(&self, p: Vec3) -> (usize, f32) {
        let mut best = (0, 0.0, f32::MAX);
        for i in 0..self.edges.len() {
            let (t, d) = self.project(i, p);
            if d < best.2 {
                best = (i, t.clamp(0.0, 1.0), d);
            }
        }
        (best.0, best.1)
    }

    /// The total length of the main loop, following first links.
    pub fn lap_length(&self) -> f32 {
        let Some(mut e) = self.nodes.first().and_then(|n| n.out.first().copied()) else {
            return 0.0;
        };
        let mut total = 0.0;
        for _ in 0..self.edges.len() {
            total += self.edges[e].len;
            match self.nodes[self.edges[e].to].out.first() {
                Some(&n) => e = n,
                None => break,
            }
            if self.edges[e].from == 0 {
                break;
            }
        }
        total
    }
}

/// The original's PID controller (0xac830 / 0xac880): each term is clamped
/// to -1..1, the integral only winds while the output is unsaturated.
#[derive(Debug, Clone, Default)]
struct Pid {
    gains: [f32; 3],
    integral: f32,
    last: Option<f32>,
    output: f32,
}

impl Pid {
    fn new(p: f32, i: f32, d: f32) -> Self {
        Self {
            gains: [p, i, d],
            ..Default::default()
        }
    }

    fn reset(&mut self) {
        self.integral = 0.0;
        self.last = None;
        self.output = 0.0;
    }

    fn update(&mut self, err: f32, dt: f32) -> f32 {
        let [kp, ki, kd] = self.gains;
        let p = (err * kp).clamp(-1.0, 1.0);
        if ki == 0.0 {
            self.integral = 0.0;
        } else if self.output > -1.0 && self.output < 1.0 {
            self.integral = (self.integral + dt * ki * err).clamp(-1.0, 1.0);
        }
        let d = match self.last {
            Some(last) if kd != 0.0 && dt > 0.0 => ((err - last) / dt * kd).clamp(-1.0, 1.0),
            _ => 0.0,
        };
        self.last = Some(err);
        self.output = (p + self.integral + d).clamp(-1.0, 1.0);
        self.output
    }
}

/// Where another racer is, for passing.
#[derive(Debug, Clone, Copy)]
pub struct Traffic {
    pub position: Vec3,
    pub velocity: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Race,
    /// Backing up out of a jam, with the steering flipped.
    Reverse,
}

/// One computer-controlled rider.
#[derive(Debug, Clone)]
pub struct AiDriver {
    /// Where on the line the quad is.
    pub edge: usize,
    pub t: f32,
    /// Scales every speed the driver aims for (1 = the original's pace).
    pub pace: f32,
    /// 0..1, from the rider's ability; shades how close to the limit
    /// corners are taken.
    pub ability: f32,
    /// Last steering target and target speed (for debugging views).
    pub target: Vec3,
    pub target_speed: f32,
    steer_pid: Pid,
    speed_pid: Pid,
    /// Sideways shift of the target from the corridor middle, metres.
    offset: f32,
    mode: Mode,
    mode_timer: f32,
    stuck_timer: f32,
    stuck_attempts: u32,
    stuck_at: Vec3,
    off_line: f32,
    /// Where the current no-progress window started, and its age.
    window: (Vec3, f32, f32),
    respot: bool,
    /// Edge chosen at each branch node, so a driver commits to one route.
    choice: Option<(usize, usize)>,
    /// Edge of the last respot and how many times in a row it was used.
    last_respot: Option<(usize, u32)>,
    rng: u32,
}

impl AiDriver {
    pub fn new(line: &RacingLine, position: Vec3, seed: u32) -> Self {
        let (edge, t) = if line.is_empty() {
            (0, 0.0)
        } else {
            line.locate(position)
        };
        Self {
            edge,
            t,
            pace: 1.0,
            ability: 1.0,
            target: position,
            target_speed: 0.0,
            steer_pid: Pid::new(1.0, 0.2, 0.01),
            speed_pid: Pid::new(0.04, 0.1, 0.0),
            offset: 0.0,
            mode: Mode::Race,
            mode_timer: 0.0,
            stuck_timer: 0.0,
            stuck_attempts: 0,
            stuck_at: position,
            off_line: 0.0,
            window: (position, 0.0, 0.0),
            respot: false,
            choice: None,
            last_respot: None,
            rng: seed.wrapping_mul(2_654_435_761).max(1),
        }
    }

    fn random(&mut self) -> u32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    /// The edge taken after `edge` (random at a split, like the original's
    /// 0x666c0, but remembered so the driver doesn't dither).
    fn next_edge(&mut self, line: &RacingLine, edge: usize) -> Option<usize> {
        let node = line.edges[edge].to;
        let out = &line.nodes[node].out;
        match out.len() {
            0 => None,
            1 => Some(out[0]),
            n => {
                if let Some((at, e)) = self.choice
                    && at == node
                {
                    return Some(e);
                }
                let e = out[self.random() as usize % n];
                self.choice = Some((node, e));
                Some(e)
            }
        }
    }

    /// Peek at the edge after `edge` without committing to a branch.
    fn peek_edge(&self, line: &RacingLine, edge: usize) -> Option<usize> {
        let node = line.edges[edge].to;
        let out = &line.nodes[node].out;
        if out.len() > 1
            && let Some((at, e)) = self.choice
            && at == node
        {
            return Some(e);
        }
        out.first().copied()
    }

    /// Follows the quad along the line.
    fn track(&mut self, line: &RacingLine, p: Vec3) {
        for _ in 0..8 {
            let (t, _) = line.project(self.edge, p);
            if t > 1.0 {
                match self.next_edge(line, self.edge) {
                    Some(e) => self.edge = e,
                    None => break,
                }
            } else if t < 0.0 {
                let from = line.edges[self.edge].from;
                let back = line.nodes[from]
                    .inc
                    .iter()
                    .copied()
                    .min_by(|&a, &b| line.project(a, p).1.total_cmp(&line.project(b, p).1));
                match back {
                    Some(e) if line.project(e, p).0 <= 1.0 => self.edge = e,
                    _ => break,
                }
            } else {
                break;
            }
        }
        let (t, d2) = line.project(self.edge, p);
        // Lost the line entirely (respot, big crash): find it again.
        if d2 > 40.0 * 40.0 {
            let (e, t) = line.locate(p);
            self.edge = e;
            self.t = t;
        } else {
            self.t = t.clamp(0.0, 1.0);
        }
    }

    /// Whether the driver wants putting back on the track.
    pub fn wants_respot(&self) -> bool {
        self.respot
    }

    /// A respot pose: the nearest respot-able node at or behind the quad,
    /// facing down the line, and the driver is reset there.
    pub fn respot(&mut self, line: &RacingLine) -> (Vec3, Quat) {
        let back_to_respot = |mut e: usize| {
            for _ in 0..line.edges.len() {
                if line.nodes[line.edges[e].from].respot {
                    break;
                }
                match line.nodes[line.edges[e].from].inc.first() {
                    Some(&p) => e = p,
                    None => break,
                }
            }
            e
        };
        let mut e = back_to_respot(self.edge);
        // Respotting at the same node again and again means the quad can't
        // get going from there (a spot it tips off, say), so each repeat
        // goes back one more respot node.
        let repeats = match self.last_respot {
            Some((last, n)) if last == e => n + 1,
            _ => 0,
        };
        self.last_respot = Some((e, repeats));
        for _ in 0..repeats.min(4) {
            if let Some(&p) = line.nodes[line.edges[e].from].inc.first() {
                e = back_to_respot(p);
            }
        }
        let edge = &line.edges[e];
        let (l, r) = line.corridor(e, 0.0);
        let pos = line.nodes[edge.from].pos + edge.side * ((l + r) * 0.5);
        let fwd = edge.dir.with_y(0.0).normalize_or(Vec3::NEG_Z);
        let rot = Quat::from_rotation_arc(Vec3::NEG_Z, fwd);
        self.edge = e;
        self.t = 0.0;
        self.respot = false;
        self.mode = Mode::Race;
        self.stuck_timer = 0.0;
        self.stuck_attempts = 0;
        self.off_line = 0.0;
        self.window = (pos, 0.0, 0.0);
        self.offset = 0.0;
        self.steer_pid.reset();
        self.speed_pid.reset();
        (pos, rot)
    }

    /// The quad's top speed in m/s, with the original's 10% margin.
    fn top_speed(v: &Vehicle) -> f32 {
        let p = &v.params;
        let ratio = p.gears.last().map_or(1.0, |g| g.ratio) * p.gear_multiplier;
        if ratio.abs() < 1e-4 {
            return 60.0;
        }
        p.max_revs / (ratio * 9.549_3) * 1.1
    }

    /// Works out this frame's controls.
    pub fn drive(&mut self, line: &RacingLine, v: &Vehicle, traffic: &[Traffic], dt: f32) -> Input {
        if line.is_empty() {
            return Input::default();
        }
        let pos = v.position;
        self.track(line, pos);
        let fwd = v.forward().with_y(0.0).normalize_or(Vec3::NEG_Z);
        let speed = v.velocity.dot(v.forward());
        let (left, right) = line.corridor(self.edge, self.t);
        let width = right - left;
        let edge = &line.edges[self.edge];

        // How far off the corridor the quad is.
        let lateral = (pos - line.point(self.edge, self.t)).dot(edge.side);
        let outside = (left - lateral).max(lateral - right).max(0.0);
        self.off_line = if outside > 15.0 {
            self.off_line + dt
        } else {
            0.0
        };
        if self.off_line > 2.0 || pos.y < line.point(self.edge, self.t).y - 100.0 {
            self.respot = true;
        }

        // Passing: the nearest quad ahead within the look-ahead that sits in
        // our lane moves the target to the roomier side of it.
        let gap = self.plan_passing(line, pos, fwd, lateral, left, right, traffic, dt);

        let look = width.min(gap).max(1.05 * speed.abs()).max(1.0);
        let decel = 0.8 * v.params.brake_curve.scale.abs();
        let brake_look = if decel > 1e-6 {
            (speed * speed / (2.0 * decel)).min(MAX_BRAKE_LOOKAHEAD)
        } else {
            0.0
        };

        // Walk forward along the line.
        let top = Self::top_speed(v);
        let mut target_speed = top;
        let mut target = None;
        let mut e = self.edge;
        let mut dist = -line.edges[e].len * self.t;
        for _ in 0..line.edges.len() {
            let len = line.edges[e].len;
            if target.is_none() && dist + len >= look {
                let f = ((look - dist) / len).clamp(0.0, 1.0);
                let (l, r) = line.corridor(e, f);
                let half = (r - l) * 0.5;
                let shift = self.offset.clamp(
                    -(half - QUAD_HALF_WIDTH).max(0.0),
                    (half - QUAD_HALF_WIDTH).max(0.0),
                );
                target = Some(line.point(e, f) + line.edges[e].side * ((l + r) * 0.5 + shift));
            }
            let reach = dist + len;
            if reach <= brake_look + len {
                let limit = line.nodes[line.edges[e].to].speed_limit;
                if limit < NO_LIMIT {
                    let allowed = (limit * limit + 2.0 * decel * reach.max(0.0)).sqrt();
                    // Abler riders brake later for corners.
                    let allowed = allowed * (0.9 + 0.1 * self.ability);
                    target_speed = target_speed.min(allowed);
                }
            }
            if target.is_some() && reach > brake_look {
                break;
            }
            dist = reach;
            match self.peek_edge(line, e) {
                Some(n) => e = n,
                None => break,
            }
        }
        let target = target.unwrap_or_else(|| line.point(e, 1.0));
        let target_speed = target_speed.min(top) * self.pace;
        self.target = target;
        self.target_speed = target_speed;

        // Steering: sine of the angle to the target, right positive.
        let to = (target - pos).with_y(0.0).normalize_or(fwd);
        let side = fwd.cross(Vec3::Y);
        let mut sine = to.dot(side);
        if to.dot(fwd) < 0.0 {
            // Target behind: full lock towards it.
            sine = if sine >= 0.0 { 1.0 } else { -1.0 };
        }

        // Stuck: stopped while trying to go.
        self.update_stuck(pos, speed, dt);
        let mut input = Input::default();
        match self.mode {
            Mode::Race => {
                self.steer_pid.gains[0] = 1.0;
                input.steer = self.steer_pid.update(sine, dt);
                let out = self.speed_pid.update(target_speed - speed, dt);
                if out >= 0.0 {
                    input.throttle = out;
                } else if speed > 0.894 {
                    input.brake = -out;
                }
                input.jump = self.hop(line, pos, fwd, speed);
            }
            Mode::Reverse => {
                self.steer_pid.gains[0] = 0.3;
                input.steer = -self.steer_pid.update(sine, dt).signum();
                input.brake = 1.0;
            }
        }
        input
    }

    /// Jump nodes (0x66380): coming up to one fast and lined up with the
    /// line, the rider loads the suspension and lets go at the lip, for a
    /// longer hold the stronger the node's boost.
    fn hop(&self, line: &RacingLine, pos: Vec3, fwd: Vec3, speed: f32) -> bool {
        let e = &line.edges[self.edge];
        let node = &line.nodes[e.to];
        if node.boost == 0
            || speed < HOP_SPEED
            || fwd.dot(e.dir.with_y(0.0).normalize_or_zero()) < 0.7
        {
            return false;
        }
        let d = (node.pos - pos).dot(e.dir);
        let hold = speed * 0.6 * f32::from(node.boost.min(100)) / 100.0;
        d > 0.5 && d < hold
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_passing(
        &mut self,
        line: &RacingLine,
        pos: Vec3,
        fwd: Vec3,
        lateral: f32,
        left: f32,
        right: f32,
        traffic: &[Traffic],
        dt: f32,
    ) -> f32 {
        let side = line.edges[self.edge].side.normalize_or(Vec3::X);
        let mid = (left + right) * 0.5;
        let mut nearest: Option<(f32, f32)> = None;
        for t in traffic {
            let d = t.position - pos;
            let ahead = d.dot(fwd);
            if !(0.5..40.0).contains(&ahead) {
                continue;
            }
            let lat = lateral + d.dot(side);
            if (lat - (mid + self.offset)).abs() > 2.0 * QUAD_HALF_WIDTH + 0.6 {
                continue;
            }
            if nearest.is_none_or(|n| ahead < n.0) {
                nearest = Some((ahead, lat));
            }
        }
        let want = match nearest {
            Some((_, lat)) => {
                let room_left = lat - left;
                let room_right = right - lat;
                let clear = 2.0 * QUAD_HALF_WIDTH + 0.5;
                if room_right >= room_left {
                    lat + clear - mid
                } else {
                    lat - clear - mid
                }
            }
            None => 0.0,
        };
        // Ease across rather than snapping.
        let k = 1.0 - (-1.5 * dt).exp();
        self.offset += (want - self.offset) * k;
        nearest.map_or(f32::MAX, |n| n.0)
    }

    fn update_stuck(&mut self, pos: Vec3, speed: f32, dt: f32) {
        // Making no headway at all (pinned on a wall, sliding back down a
        // bank) for long enough: give up and respot, as the original's
        // progress timers do.
        let (start, age, reach) = &mut self.window;
        *age += dt;
        *reach = reach.max(pos.distance(*start));
        if *age > PROGRESS_WINDOW {
            if *reach < PROGRESS_RADIUS {
                self.respot = true;
            }
            self.window = (pos, 0.0, 0.0);
        }
        match self.mode {
            Mode::Race => {
                if speed.abs() < STUCK_SPEED {
                    self.stuck_timer += dt;
                } else {
                    self.stuck_timer = 0.0;
                    if pos.distance(self.stuck_at) > 8.0 {
                        self.stuck_attempts = 0;
                    }
                }
                if self.stuck_timer > 1.5 {
                    if pos.distance(self.stuck_at) < 3.0 {
                        self.stuck_attempts += 1;
                    }
                    self.stuck_at = pos;
                    self.stuck_timer = 0.0;
                    if self.stuck_attempts >= 3 {
                        self.respot = true;
                    } else {
                        self.mode = Mode::Reverse;
                        self.mode_timer = 0.0;
                        self.steer_pid.reset();
                        self.speed_pid.reset();
                    }
                }
            }
            Mode::Reverse => {
                self.mode_timer += dt;
                if self.mode_timer > 1.5 {
                    self.mode = Mode::Race;
                    self.steer_pid.reset();
                }
            }
        }
    }
}

/// The retail game's tweaks to an AI quad's handling (0x66200, 0x681d0):
/// steering snaps straight to the driver's input, and the engine pulls
/// harder with the rider's ability (`ability` 0-1, the rider's Ability
/// characteristic out of 5).
pub fn tune_for_ai(params: &mut VehicleParams, ability: f32) {
    params.steer_rate = [1e6, 1e6];
    params.steer_return_rate = [1e6, 1e6];
    params.torque_curve.scale *= 1.15 * (1.0 + 0.1 * ability.clamp(0.0, 1.0));
}

/// Pushes two quads apart when they touch, trading the closing part of
/// their velocities (equal masses, a soft bump).
pub fn bump(a: &mut Vehicle, b: &mut Vehicle) {
    const RADIUS: f32 = 1.0;
    let d = (b.position - a.position).with_y(0.0);
    let dist = d.length();
    let dy = (b.position.y - a.position.y).abs();
    if dist >= 2.0 * RADIUS || dist < 1e-4 || dy > 1.5 {
        return;
    }
    let n = d / dist;
    let push = (2.0 * RADIUS - dist) * 0.5;
    let closing = (a.velocity - b.velocity).dot(n);
    let dv = if closing > 0.0 {
        n * closing * 0.65
    } else {
        Vec3::ZERO
    };
    a.nudge(-n * push, -dv);
    b.nudge(n * push, dv);
}

#[cfg(test)]
mod tests {
    use super::*;
    use atv_formats::aid::AiNode;

    fn square() -> RacingLine {
        let pts = [
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 100.0],
            [100.0, 0.0, 100.0],
            [100.0, 0.0, 0.0],
        ];
        let n = pts.len();
        let nodes = (0..n)
            .map(|i| AiNode {
                name: String::new(),
                pos: pts[i],
                forward: [0.0, 0.0, 1.0],
                up: [0.0, 1.0, 0.0],
                left: -5.0,
                right: 5.0,
                prev: vec![(i + n - 1) % n],
                next: vec![(i + 1) % n],
                speed_limit: (i == 2).then_some(10.0),
                flags: 1,
                boost: 0,
            })
            .collect();
        RacingLine::from_path(&AiPath { nodes })
    }

    #[test]
    fn line_geometry() {
        let l = square();
        assert_eq!(l.edges.len(), 4);
        assert!((l.lap_length() - 400.0).abs() < 1e-3);
        // Game +Z is sim -Z; the first edge runs down -Z and its right is +X.
        assert!(l.edges[0].dir.abs_diff_eq(Vec3::NEG_Z, 1e-5));
        assert!(l.edges[0].side.abs_diff_eq(Vec3::X, 1e-5));
        let (e, t) = l.locate(Vec3::new(1.0, 0.0, -50.0));
        assert_eq!(e, 0);
        assert!((t - 0.5).abs() < 1e-3);
    }

    #[test]
    fn pid_clamps_and_winds() {
        let mut p = Pid::new(0.04, 0.1, 0.0);
        assert!((p.update(10.0, 0.1) - 0.5).abs() < 1e-5);
        for _ in 0..100 {
            p.update(1000.0, 0.1);
        }
        assert_eq!(p.output, 1.0);
        // The wound-up integral holds the output off full brake at first...
        assert!(p.update(-1000.0, 0.1) > -1.0);
        // ...then unwinds while the output is unsaturated.
        for _ in 0..3 {
            p.update(-1000.0, 0.1);
        }
        assert_eq!(p.output, -1.0);
    }
}
