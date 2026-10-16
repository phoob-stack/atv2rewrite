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
//!   (0x67050, 0x671e0), and one alongside (or closing fast from just
//!   behind) walls off its side of the corridor (retail 0x7a850).
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
//! - **Pacing** (0x6a3f0, 0x6a970): a [`Pacer`] runs "pace cars" round the
//!   track at the lap times `Setup/Tracks.xml` gives it
//!   (`mAIBestLapTime`..`mAIWorstLapTime`), and scales each rider's target
//!   speeds by how far ahead of or behind its pace car it is.
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
/// A quad's half-width and half-length, metres: the original measures its
/// model's bounding box (0x7bed0); these are where the port's quads touch
/// (side by side about 1.5 m apart).
const BODY_HALF_WIDTH: f32 = 0.8;
const BODY_HALF_LENGTH: f32 = 1.1;
/// A quad this far behind a computer rider and this much faster
/// (metres, m/s) counts as alongside it (0x79fc0 sets both).
const OVERTAKER_REACH: f32 = 5.0;
const OVERTAKER_SPEED: f32 = 10.0;
/// A driver that stays within this many metres for this many seconds is
/// respotted.
const PROGRESS_RADIUS: f32 = 10.0;
const PROGRESS_WINDOW: f32 = 8.0;
/// A driver that gets no further round the lap for this many seconds
/// (rolling back and forth at the foot of a climb) is respotted too; the
/// rewrite's.
const HEADWAY_TIMEOUT: f32 = 20.0;
/// Jump nodes only get a hop from quads at least this fast (40 mph).
const HOP_SPEED: f32 = 17.877_8;
/// Respots in a row at one spot, each from one more respot node back,
/// before the rider is put past the spot instead.
const SKIP_AFTER_REPEATS: u32 = 2;
/// A respot this far (metres) past the last one's spot is a fresh one.
const TROUBLE_REACH: f32 = 50.0;
/// Seconds of travel ahead a jump node keeps the pacer from slowing a
/// rider.
const JUMP_RUN_UP: f32 = 1.0;
/// ...and never less than this many metres.
const JUMP_RUN_UP_MIN: f32 = 10.0;
/// How a computer rider races: the prototype's settings (0x66200, 0x68430,
/// 0x67520) and retail's (0x79fc0, 0x7b990, 0x79af0).
struct Tactics {
    /// The engine boost (factor set at 0x66200 / 0x79fc0, applied at
    /// 0x68250 / 0x7b990): this long into the race a computer rider's
    /// engine torque goes up this many times, for the rest of the race...
    engine_boost: f32,
    /// ...and below this share of the quad's top speed the throttle is
    /// held to `1 / engine_boost`, which leaves them about as quick as
    /// stock until they are going fast.
    boost_speed: f32,
    /// A computer rider presses the boost button when the line runs
    /// straight (each link turning less than this cosine from the one
    /// before) for `BOOST_STRAIGHT` metres within `BOOST_LINKS` links.
    /// The prototype also wants the throttle flat out; retail wants at
    /// least half the top speed, a target speed above 75% of it and a
    /// quarter of the boost meter (the race checks that).
    boost_cos: f32,
    /// A computer rider rides with the nearest player when they are going
    /// faster than this (m/s)...
    pack_speed: f32,
    /// ...within this many metres.
    pack_range: f32,
    /// Closer behind than this (metres along the line), a boosted engine
    /// is held back.
    pack_close: f32,
    /// Throttle PID gains racing the line and riding with a player.
    race_gains: [f32; 3],
    pack_gains: [f32; 3],
    /// Steering PID gains.
    steer_gains: [f32; 3],
}

const PROTOTYPE: Tactics = Tactics {
    engine_boost: 2.5,
    boost_speed: 0.75,
    boost_cos: 0.94,
    pack_speed: 13.408,
    pack_range: 30.0,
    pack_close: 5.0,
    race_gains: [0.04, 0.1, 0.0],
    pack_gains: [0.015, 0.4, 0.1],
    steer_gains: [1.0, 0.2, 0.01],
};

const RETAIL: Tactics = Tactics {
    engine_boost: 2.0,
    boost_speed: 0.5,
    boost_cos: 0.9659,
    pack_speed: 17.878,
    pack_range: 20.0,
    pack_close: 8.0,
    race_gains: [0.04, 0.0, 0.0],
    pack_gains: [0.02, 0.25, 0.1],
    steer_gains: [1.2, 0.2, 0.04],
};

const ENGINE_BOOST_DELAY: f32 = 6.0;
const BOOST_THROTTLE: f32 = 0.99;
const BOOST_STRAIGHT: f32 = 100.0;
const BOOST_LINKS: usize = 5;
/// Retail boosts only while the target speed is above this share of the
/// quad's top speed.
const RETAIL_BOOST_TARGET: f32 = 0.75;

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
    /// How far round the main loop each node is, metres from node 0
    /// (nodes on alternative routes are shared out between where the route
    /// leaves and rejoins the loop).
    lap_pos: Vec<f32>,
    lap_len: f32,
    /// Lap position of the start/finish line (see [`Self::set_start`]).
    start: f32,
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
        let mut line = Self {
            nodes,
            edges,
            lap_pos: Vec::new(),
            lap_len: 0.0,
            start: 0.0,
        };
        line.lap_len = line.lap_length();
        line.lap_pos = line.lap_positions();
        line
    }

    fn lap_positions(&self) -> Vec<f32> {
        let n = self.nodes.len();
        let mut pos = vec![None; n];
        if let Some(mut e) = self.nodes.first().and_then(|n| n.out.first().copied()) {
            let mut d = 0.0;
            pos[0] = Some(0.0);
            for _ in 0..self.edges.len() {
                let to = self.edges[e].to;
                d += self.edges[e].len;
                if to == 0 || pos[to].is_some() {
                    break;
                }
                pos[to] = Some(d);
                match self.nodes[to].out.first() {
                    Some(&next) => e = next,
                    None => break,
                }
            }
        }
        // Off the loop: walk back and forward to it, and share the gap.
        let main = pos.clone();
        for (i, p) in pos.iter_mut().enumerate() {
            if p.is_some() {
                continue;
            }
            let walk = |forward: bool| {
                let (mut at, mut d) = (i, 0.0);
                for _ in 0..n {
                    let e = if forward {
                        self.nodes[at].out.first()
                    } else {
                        self.nodes[at].inc.first()
                    };
                    let &e = e?;
                    d += self.edges[e].len;
                    at = if forward {
                        self.edges[e].to
                    } else {
                        self.edges[e].from
                    };
                    if let Some(m) = main[at] {
                        return Some((m, d));
                    }
                }
                None
            };
            *p = match (walk(false), walk(true)) {
                (Some((a, da)), Some((mut b, db))) => {
                    if b < a {
                        b += self.lap_len;
                    }
                    Some((a + (b - a) * da / (da + db).max(1e-3)) % self.lap_len.max(1.0))
                }
                (Some((a, da)), None) => Some(a + da),
                (None, Some((b, db))) => Some((b - db).max(0.0)),
                (None, None) => Some(0.0),
            };
        }
        pos.into_iter().map(|p| p.unwrap_or(0.0)).collect()
    }

    /// Puts the start of the lap at `p` (the start/finish line), so lap
    /// positions and distances count from there rather than from node 0,
    /// which on some tracks sits well round the lap.
    pub fn set_start(&mut self, p: Vec3) {
        if self.is_empty() {
            return;
        }
        self.start = 0.0;
        let (e, t) = self.locate(p);
        self.start = self.lap_position(e, t);
    }

    /// How far round the lap `t` along `edge` is, metres from the start
    /// line (node 0 until [`Self::set_start`]).
    pub fn lap_position(&self, edge: usize, t: f32) -> f32 {
        let e = &self.edges[edge];
        let a = self.lap_pos[e.from];
        let mut b = self.lap_pos[e.to];
        if b < a - self.lap_len * 0.5 {
            b += self.lap_len;
        }
        let p = a + (b - a) * t.clamp(0.0, 1.0) - self.start;
        if self.lap_len > 0.0 {
            p.rem_euclid(self.lap_len)
        } else {
            p
        }
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
        self.update_terms(err, err, err, dt)
    }

    /// The original's PID takes a separate input for each term.
    fn update_terms(&mut self, p_in: f32, i_in: f32, d_in: f32, dt: f32) -> f32 {
        let [kp, ki, kd] = self.gains;
        let p = (p_in * kp).clamp(-1.0, 1.0);
        if ki == 0.0 {
            self.integral = 0.0;
        } else if self.output > -1.0 && self.output < 1.0 {
            self.integral = (self.integral + dt * ki * i_in).clamp(-1.0, 1.0);
        }
        let d = match self.last {
            Some(last) if kd != 0.0 && dt > 0.0 => ((d_in - last) / dt * kd).clamp(-1.0, 1.0),
            _ => 0.0,
        };
        self.last = Some(d_in);
        self.output = (p + self.integral + d).clamp(-1.0, 1.0);
        self.output
    }
}

/// Where another racer is, for passing and for riding with a player.
#[derive(Debug, Clone, Copy)]
pub struct Traffic {
    pub position: Vec3,
    pub velocity: Vec3,
    /// A player on their quad (not thrown off): computer riders ride
    /// with them.
    pub player: bool,
    /// Using the speed boost.
    pub boosting: bool,
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
    /// The [`Pacer`]'s scale on the speeds the driver aims for (1 = on
    /// pace); set by the race each frame.
    pub pace: f32,
    /// Last steering target and target speed (for debugging views).
    pub target: Vec3,
    pub target_speed: f32,
    /// The rider wants to use the speed boost this frame (it needs some in
    /// its meter, which the race keeps).
    pub boost: bool,
    /// Seconds of racing left before the engine boost kicks in; `None` for
    /// a quad that doesn't get it (a player's, on autopilot).
    engine_delay: Option<f32>,
    /// Racing a retail quad (its .params say so): retail's tactics.
    retail: bool,
    steer_pid: Pid,
    speed_pid: Pid,
    /// Sideways shift of the target from the corridor middle, metres.
    offset: f32,
    mode: Mode,
    mode_timer: f32,
    stuck_timer: f32,
    stuck_attempts: u32,
    /// The furthest round the lap the rider has got, and how long ago.
    headway: (f32, f32),
    stuck_at: Vec3,
    off_line: f32,
    /// Where the current no-progress window started, and its age.
    window: (Vec3, f32, f32),
    respot: bool,
    /// The edge whose end node, a jump node, the rider has just passed.
    passed_jump: Option<usize>,
    /// Edge chosen at each branch node, so a driver commits to one route.
    choice: Option<(usize, usize)>,
    /// Where respots keep happening: the furthest edge and distance the
    /// rider got to, and how many respots in a row came short of it.
    last_respot: Option<(usize, f32, u32)>,
    rng: u32,
    /// Distance covered round the line since the start, metres.
    odometer: Odometer,
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
            target: position,
            target_speed: 0.0,
            boost: false,
            engine_delay: None,
            retail: false,
            steer_pid: Pid::new(PROTOTYPE.steer_gains[0], PROTOTYPE.steer_gains[1], PROTOTYPE.steer_gains[2]),
            speed_pid: Pid::new(PROTOTYPE.race_gains[0], PROTOTYPE.race_gains[1], PROTOTYPE.race_gains[2]),
            offset: 0.0,
            mode: Mode::Race,
            mode_timer: 0.0,
            stuck_timer: 0.0,
            stuck_attempts: 0,
            headway: (f32::MIN, 0.0),
            stuck_at: position,
            off_line: 0.0,
            window: (position, 0.0, 0.0),
            respot: false,
            passed_jump: None,
            choice: None,
            last_respot: None,
            rng: seed.wrapping_mul(2_654_435_761).max(1),
            odometer: Odometer::default(),
        }
    }

    /// A computer rider's driver: its quad gets the original's engine
    /// boost (see [`Self::torque_scale`]).
    pub fn with_engine_boost(mut self) -> Self {
        self.engine_delay = Some(ENGINE_BOOST_DELAY);
        self
    }

    /// What the quad's engine torque is multiplied by: the computer
    /// riders' engine boost once it has kicked in, else 1.
    pub fn torque_scale(&self) -> f32 {
        match self.engine_delay {
            Some(d) if d <= 0.0 => self.tactics().engine_boost,
            _ => 1.0,
        }
    }

    fn tactics(&self) -> &'static Tactics {
        if self.retail { &RETAIL } else { &PROTOTYPE }
    }

    /// Keeps track of a quad someone else drives (a player), for the
    /// distance it has covered.
    pub fn follow(&mut self, line: &RacingLine, pos: Vec3) {
        if line.is_empty() {
            return;
        }
        self.track(line, pos);
        self.odometer
            .update(line.lap_position(self.edge, self.t), line.lap_len);
    }

    /// Distance covered round the racing line since the start, metres
    /// (negative on the grid behind node 0).
    pub fn distance(&self) -> f32 {
        self.odometer.total
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
                    Some(e) => {
                        if line.nodes[line.edges[self.edge].to].boost > 0 {
                            self.passed_jump = Some(self.edge);
                        }
                        self.edge = e;
                    }
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
    /// facing down the line, and the driver is reset there. Repeats at the
    /// same node go further back for a longer run-up, then (the rewrite's
    /// safety net) past the spot the rider keeps failing at.
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
        // Respotting again without getting past where the last one
        // happened means the quad can't get going from there (a spot it
        // tips off, a jump it falls short of), so each repeat goes back one
        // more respot node for a longer run-up.
        let d = self.distance();
        let (furthest, at, repeats) = match self.last_respot {
            Some((edge, at, n)) if d < at + TROUBLE_REACH => {
                if d > at {
                    (self.edge, d, n + 1)
                } else {
                    (edge, at, n + 1)
                }
            }
            _ => (self.edge, d, 0),
        };
        self.last_respot = Some((furthest, at, repeats));
        if repeats < SKIP_AFTER_REPEATS {
            for _ in 0..repeats {
                if let Some(&p) = line.nodes[line.edges[e].from].inc.first() {
                    e = back_to_respot(p);
                }
            }
        } else if let Some(ahead) = self.respot_ahead(line, furthest) {
            // Still stuck with a longer run-up: the rewrite puts it past
            // the spot instead.
            e = ahead;
            self.last_respot = None;
        }
        let edge = &line.edges[e];
        let (l, r) = line.corridor(e, 0.0);
        let pos = line.nodes[edge.from].pos + edge.side * ((l + r) * 0.5);
        let fwd = edge.dir.with_y(0.0).normalize_or(Vec3::NEG_Z);
        let rot = Quat::from_rotation_arc(Vec3::NEG_Z, fwd);
        self.edge = e;
        self.t = 0.0;
        self.respot = false;
        self.passed_jump = None;
        self.mode = Mode::Race;
        self.stuck_timer = 0.0;
        self.stuck_attempts = 0;
        self.headway = (f32::MIN, 0.0);
        self.off_line = 0.0;
        self.window = (pos, 0.0, 0.0);
        self.offset = 0.0;
        self.steer_pid.reset();
        self.speed_pid.reset();
        (pos, rot)
    }

    /// The first edge after `edge` that starts at a respot-able node.
    fn respot_ahead(&mut self, line: &RacingLine, edge: usize) -> Option<usize> {
        let mut e = edge;
        for _ in 0..line.edges.len() {
            e = self.next_edge(line, e)?;
            if line.nodes[line.edges[e].from].respot {
                return Some(e);
            }
        }
        None
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
        self.retail = v.params.retail;
        let tactics = self.tactics();
        self.follow(line, pos);
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
        let gap = self.plan_passing(line, pos, v.velocity, fwd, lateral, left, right, traffic, dt);

        let look = width.min(gap).max(1.05 * speed.abs()).max(1.0);
        let decel = 0.8 * v.params.brake_curve.scale.abs();
        let brake_look = if decel > 1e-6 {
            (speed * speed / (2.0 * decel)).min(MAX_BRAKE_LOOKAHEAD)
        } else {
            0.0
        };

        // The rewrite's own: the port's quads are quicker than the
        // original's between jumps, so a rider well ahead of its pace car
        // would be held back to a crawl and come up to some jumps too slow
        // to clear them (Swamp 2's ramp onto the jetty). Coming up to a
        // jump node (the last second or 10 m before it), the pacer
        // doesn't slow it. Any longer and Swamp 2 riders lap 25 s quicker
        // than the original's window.
        let pace = if self.pace < 1.0
            && self.jump_ahead(line, (speed.abs() * JUMP_RUN_UP).max(JUMP_RUN_UP_MIN))
        {
            1.0
        } else {
            self.pace
        };

        // Walk forward along the line.
        let top = Self::top_speed(v);
        let mut target_speed = f32::MAX;
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
                // 0x68760: the pacer slows every limit when the rider is
                // ahead, but only lifts the top speed when it is behind.
                let limit = line.nodes[line.edges[e].to].speed_limit;
                let mut allowed = (limit * limit + 2.0 * decel * reach.max(0.0)).sqrt();
                if allowed <= top {
                    if pace < 1.0 {
                        allowed *= pace;
                    }
                } else {
                    allowed = pace * top;
                }
                target_speed = target_speed.min(allowed);
            }
            if target.is_some() && reach > brake_look {
                break;
            }
            dist = reach;
            // The steering target past a split: pick the branch now, so the
            // quad isn't aimed down one branch and switched to the other at
            // the split (Glacier 1's routes round the snow mound).
            let next = if target.is_none() { self.next_edge(line, e) } else { self.peek_edge(line, e) };
            match next {
                Some(n) => e = n,
                None => break,
            }
        }
        let target = target.unwrap_or_else(|| line.point(e, 1.0));
        let target_speed = if target_speed == f32::MAX {
            pace * top
        } else {
            target_speed
        };
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
        if let Some(d) = &mut self.engine_delay {
            *d -= dt;
        }
        let boosted = self.torque_scale() > 1.0;
        let mut input = Input { hop: Some(self.hop(line, v)), ..Default::default() };
        self.boost = false;
        match self.mode {
            Mode::Race => {
                self.steer_pid.gains = tactics.steer_gains;
                input.steer = self.steer_pid.update(sine, dt);
                // 0x68430: when the nearest racer is a player going faster
                // than 30 mph within 30 m (retail 0x7b990: 40 mph within
                // 20 m), a computer rider rides with
                // them, closing the gap along the line (the original pairs
                // riders that are each other's nearest first, 0x25bf0;
                // this takes the nearest). It never brakes for them, and
                // ignores the speed limits and the pacer while it does.
                let nearest = traffic
                    .iter()
                    .min_by(|a, b| {
                        a.position
                            .distance_squared(pos)
                            .total_cmp(&b.position.distance_squared(pos))
                    })
                    .filter(|t| t.player);
                let pack = nearest.filter(|t| {
                    self.engine_delay.is_some()
                        && t.velocity.length() > tactics.pack_speed
                        && t.position.distance(pos) < tactics.pack_range
                });
                let they_boost = nearest.is_some_and(|t| t.boosting);
                let (out, max_throttle, max_brake, racing) = if let Some(t) = pack {
                    let gap = (t.position - pos).dot(edge.dir);
                    self.speed_pid.gains = tactics.pack_gains;
                    let out = self
                        .speed_pid
                        .update_terms(t.velocity.length(), gap, gap, dt);
                    // Held back while the boost is on and it is close
                    // behind or the other rider is boosting.
                    let cap = if (gap >= tactics.pack_close && !they_boost) || !boosted {
                        1.0
                    } else {
                        1.0 / tactics.engine_boost
                    };
                    (out, cap, 0.0, false)
                } else {
                    self.speed_pid.gains = tactics.race_gains;
                    let out = self.speed_pid.update(target_speed - speed, dt);
                    // With the engine boost on, the throttle is held back
                    // below 75% of top speed (retail 50%) or while the
                    // nearest racer boosts.
                    let cap = if boosted && (speed < tactics.boost_speed * top || they_boost) {
                        1.0 / tactics.engine_boost
                    } else {
                        1.0
                    };
                    (out, cap, 1.0, true)
                };
                // On a straight the rider boosts: in the prototype
                // (0x67520) flat out with nothing holding it back, in
                // retail (0x7b990) at half its top speed or more while it
                // wants three quarters of it.
                let wants_boost = if self.retail {
                    speed >= tactics.boost_speed * top && target_speed > RETAIL_BOOST_TARGET * top
                } else {
                    out >= BOOST_THROTTLE && max_throttle >= BOOST_THROTTLE
                };
                self.boost = racing
                    && self.engine_delay.is_some()
                    && wants_boost
                    && self.straight_ahead(line) >= BOOST_STRAIGHT;
                if out >= 0.0 {
                    input.throttle = out.min(max_throttle);
                } else if speed > 0.894 {
                    input.brake = (-out).min(max_brake);
                }
            }
            Mode::Reverse => {
                self.steer_pid.gains[0] = 0.3;
                input.steer = -self.steer_pid.update(sine, dt).signum();
                input.brake = 1.0;
            }
        }
        input
    }

    /// How far the line runs straight from the start of the rider's
    /// current link (0x66f10): links are added while each turns less than
    /// 20 degrees from the one before (their `side`s, so steep links count
    /// as turns too), up to `BOOST_LINKS` of them, until `BOOST_STRAIGHT`
    /// metres.
    fn straight_ahead(&self, line: &RacingLine) -> f32 {
        let mut e = self.edge;
        let mut run = 0.0;
        for _ in 0..BOOST_LINKS {
            let Some(&next) = line.nodes[line.edges[e].to].out.first() else {
                break;
            };
            if line.edges[e].side.dot(line.edges[next].side) <= self.tactics().boost_cos {
                break;
            }
            run += line.edges[e].len;
            if run >= BOOST_STRAIGHT {
                break;
            }
            e = next;
        }
        run
    }

    /// Whether a jump node (one with a hop boost) is within `reach` metres
    /// down the line.
    fn jump_ahead(&self, line: &RacingLine, reach: f32) -> bool {
        let mut e = self.edge;
        let mut dist = -line.edges[e].len * self.t;
        for _ in 0..line.edges.len() {
            dist += line.edges[e].len;
            if dist > reach {
                return false;
            }
            if line.nodes[line.edges[e].to].boost > 0 {
                return true;
            }
            match self.peek_edge(line, e) {
                Some(n) => e = n,
                None => return false,
            }
        }
        false
    }

    /// Jump nodes (0x66380, 0x66460; the hop is retail 0x6cf00): passing
    /// one at 40 mph or more, the original hops with strength `boost / 100`
    /// times how well the quad's heading lines up with the line, on that
    /// tick only, and with 0 on every other tick.
    fn hop(&mut self, line: &RacingLine, v: &Vehicle) -> f32 {
        let Some(e) = self.passed_jump.take() else { return 0.0 };
        let e = &line.edges[e];
        if v.velocity.length() < HOP_SPEED {
            return 0.0;
        }
        let along = v
            .forward()
            .with_y(0.0)
            .normalize_or_zero()
            .dot(e.dir.with_y(0.0).normalize_or_zero());
        (along * f32::from(line.nodes[e.to].boost) / 100.0).clamp(0.0, 1.0)
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_passing(
        &mut self,
        line: &RacingLine,
        pos: Vec3,
        vel: Vec3,
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
        // 0x7a850: a quad alongside (the two overlapping along the line)
        // walls off its side of the corridor, so the rider doesn't steer
        // into it; with no room left between, it holds its line.
        let dir = line.edges[self.edge].dir.with_y(0.0).normalize_or(fwd);
        let (mut lo, mut hi) = (lateral.min(left), lateral.max(right));
        for t in traffic {
            let d = t.position - pos;
            let along = d.dot(dir);
            let closing = (t.velocity - vel).dot(dir);
            let reach = 2.0 * BODY_HALF_LENGTH;
            let alongside = along.abs() < reach;
            // ...and so does one coming up fast from just behind.
            let overtaking = along < 0.0 && along >= -(reach + OVERTAKER_REACH) && closing >= OVERTAKER_SPEED;
            if !alongside && !overtaking {
                continue;
            }
            let lat = lateral + d.dot(side);
            if lat < lateral {
                lo = lo.max(lat + BODY_HALF_WIDTH);
            } else {
                hi = hi.min(lat - BODY_HALF_WIDTH);
            }
        }
        let (lo, hi) = (lo + BODY_HALF_WIDTH, hi - BODY_HALF_WIDTH);
        if lo <= hi {
            self.offset = (mid + self.offset).clamp(lo, hi) - mid;
        } else {
            self.offset = lateral - mid;
        }
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
        let d = self.distance();
        if d > self.headway.0 {
            self.headway = (d, 0.0);
        } else {
            self.headway.1 += dt;
            if self.headway.1 > HEADWAY_TIMEOUT {
                self.respot = true;
            }
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

/// The retail game's tweaks to an AI quad's handling (rider skill setup
/// 0x66200, applied at 0x681d0): the brake pedal moves instantly, the
/// quad grips harder sideways and keeps more of its yaw damping in a slide
/// the abler the rider is, and the side forces push at the height of the
/// centre of mass, so they don't roll the quad. `ability` is the rider's Ability
/// characteristic out of 5, as 0-1.
///
/// The original also makes the handbrake ("slide") pedal instant; the
/// port's quads have no handbrake input.
///
/// Retail rewrote it (0x7b880): the brake and handbrake pedals still move
/// instantly and the side forces still push at the centre of mass, but
/// the ability-scaled grip is gone. Instead the wheels spin far less
/// (DRIVEFORCESPINSCALE 0.2), steering moves at 12/s both ways, wheelies
/// are gentler and better damped (WHEELIETORQUE0MPH 0.6, 100MPH 8,
/// WHEELIEDAMPING 8) and the rider leans harder in the air
/// (INAIRLEANSCALE 18).
pub fn tune_for_ai(params: &mut VehicleParams, ability: f32) {
    if params.retail {
        params.brake_rate = 1e6;
        params.brake_return_rate = 1e6;
        params.drive_force_spin_scale = [0.2; 2];
        params.force_point = 0.0;
        params.in_air_lean = 18.0;
        params.wheelie_torque_0mph = 0.6;
        params.wheelie_torque_100mph = 8.0;
        params.wheelie_damping = 8.0;
        params.steer_rate = [12.0; 2];
        params.steer_return_rate = [12.0; 2];
        return;
    }
    let skill = 1.0 + 0.5 * ability.clamp(0.0, 1.0);
    params.brake_rate = 1e6;
    params.brake_return_rate = 1e6;
    params.torque_slide_scale = [0.75 / skill; 2];
    params.side_max = params.side_max.map(|m| m * 1.15 * skill);
    params.force_point = 0.0;
}

/// Distance covered round a looped line, from lap positions.
#[derive(Debug, Clone, Default)]
struct Odometer {
    last: Option<f32>,
    total: f32,
}

impl Odometer {
    fn update(&mut self, lap_pos: f32, lap_len: f32) {
        let half = lap_len * 0.5;
        match self.last {
            // On the grid behind the line counts as short of it.
            None => {
                self.total = if lap_pos > half {
                    lap_pos - lap_len
                } else {
                    lap_pos
                }
            }
            Some(last) => {
                let mut d = lap_pos - last;
                if d < -half {
                    d += lap_len;
                } else if d > half {
                    d -= lap_len;
                }
                self.total += d;
            }
        }
        self.last = Some(lap_pos);
    }
}

/// Pace scale limits and the lead (metres) that takes it to 0 (0x69950).
const PACE_MAX: f32 = 1.5;
const PACE_MIN: f32 = 0.5;
const PACE_RANGE: f32 = 100.0;
/// Riders sharing one pace car.
const RIDERS_PER_PACE: usize = 3;
/// How far (metres) a pace car may be pulled from its schedule towards a
/// player, and how fast.
const CHASE_LIMIT: f32 = 600.0;
const CHASE_RATE: f32 = 20.0;

#[derive(Debug, Clone)]
struct PaceCar {
    /// Metres per second: a lap in this pace car's lap time.
    speed: f32,
    covered: f32,
    /// Pull towards the player it is chasing, metres.
    offset: f32,
}

/// The original's AI pacer (0x69950, 0x6a3f0, 0x6a970): pace cars lap the
/// track at lap times spread from the track's best to its worst AI lap
/// time, one for every three riders plus one. The riders are ranked by
/// [`pace_rating`] and the best three follow the fastest car, the next
/// three the next, and so on. A rider's target speeds scale by 1 less its
/// lead over its car per 100 m, kept to 0.5-1.5. A car with a player near
/// it is pulled towards them (up to 600 m off its schedule), so the riders
/// round a player race them rather than the clock.
#[derive(Debug, Clone)]
pub struct Pacer {
    cars: Vec<PaceCar>,
    /// The best-rated rider, whose start the pace cars set off from.
    leader: usize,
    started: bool,
}

impl Pacer {
    /// `racers` counts every quad on the grid, players included. `None`
    /// when the track gives no AI lap times.
    pub fn new(
        best: f32,
        worst: f32,
        racers: usize,
        players: usize,
        lap_length: f32,
    ) -> Option<Self> {
        if !(best > 0.0 && worst >= best && lap_length > 0.0) {
            return None;
        }
        let n = racers / RIDERS_PER_PACE + 1;
        let cars = (0..n)
            .map(|i| {
                let time = match (players, n) {
                    // With nobody to race, everyone runs at the best time.
                    (0, _) => best,
                    (_, 1) => (best + worst) * 0.5,
                    _ => best + (worst - best) * i as f32 / (n - 1) as f32,
                };
                PaceCar {
                    speed: lap_length / time,
                    covered: 0.0,
                    offset: 0.0,
                }
            })
            .collect();
        Some(Self {
            cars,
            leader: 0,
            started: false,
        })
    }

    /// Which pace car each rider follows, from their [`pace_rating`]s
    /// (best first; ties keep grid order).
    pub fn assign(&mut self, ratings: &[f32]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..ratings.len()).collect();
        order.sort_by(|&a, &b| ratings[b].total_cmp(&ratings[a]));
        self.leader = order.first().copied().unwrap_or(0);
        let mut car = vec![0; ratings.len()];
        for (rank, &i) in order.iter().enumerate() {
            car[i] = (rank / RIDERS_PER_PACE).min(self.cars.len() - 1);
        }
        car
    }

    /// Sets the pace cars off level with the best-rated rider, as the
    /// original does on its first pacing frame (0x6a0c0), so a grid behind
    /// the start line isn't counted as lost ground. `distances`: each
    /// rider's [`AiDriver::distance`], in [`Self::assign`] order. Only the
    /// first call counts.
    pub fn start(&mut self, distances: &[f32]) {
        if self.started {
            return;
        }
        self.started = true;
        let d = distances.get(self.leader).copied().unwrap_or(0.0);
        for c in &mut self.cars {
            c.covered = d;
        }
    }

    /// Runs the pace cars on once the race is under way. `players`: the
    /// distance each player has covered (see [`AiDriver::distance`]).
    pub fn tick(&mut self, dt: f32, players: &[f32]) {
        // Each player pulls the car nearest them, one player a car.
        let mut chased: Vec<Option<f32>> = vec![None; self.cars.len()];
        for &p in players {
            let nearest = (0..self.cars.len()).min_by(|&a, &b| {
                let d = |c: &PaceCar| (c.covered + c.offset - p).abs();
                d(&self.cars[a]).total_cmp(&d(&self.cars[b]))
            });
            if let Some(i) = nearest
                && chased[i].is_none()
            {
                chased[i] = Some(p);
            }
        }
        let k = (CHASE_RATE * dt).min(1.0);
        for (c, chase) in self.cars.iter_mut().zip(chased) {
            c.covered += c.speed * dt;
            c.offset = match chase {
                Some(p) => {
                    (c.offset + (p - (c.covered + c.offset)) * k).clamp(-CHASE_LIMIT, CHASE_LIMIT)
                }
                None => 0.0,
            };
        }
    }

    /// The speed scale for a rider following `car` who has covered
    /// `distance` metres.
    pub fn scale(&self, car: usize, distance: f32) -> f32 {
        let Some(c) = self.cars.get(car) else {
            return 1.0;
        };
        let lead = distance - (c.covered + c.offset);
        (1.0 - lead / PACE_RANGE).clamp(PACE_MIN, PACE_MAX)
    }
}

/// How a rider ranks for the pacer (0x25ab0): ability and technique (0-5)
/// count half, the quad's top speed, acceleration and handling (0-5) the
/// other half.
pub fn pace_rating(
    ability: f32,
    technique: f32,
    top_speed: f32,
    acceleration: f32,
    handling: f32,
) -> f32 {
    ((ability + technique) * 0.2 * 0.5 + (top_speed + acceleration + handling) * 0.2 / 3.0) * 0.5
}

/// Two quads touching: see [`crate::vehicle::bump`].
pub fn bump(a: &mut Vehicle, b: &mut Vehicle) {
    crate::vehicle::bump(a, b);
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
    fn lap_distance_runs_on_over_the_line() {
        let l = square();
        assert!((l.lap_position(1, 0.5) - 150.0).abs() < 1e-3);
        assert!((l.lap_position(3, 0.5) - 350.0).abs() < 1e-3);
        let mut o = Odometer::default();
        // On the grid just behind the line.
        o.update(390.0, 400.0);
        assert!((o.total + 10.0).abs() < 1e-3);
        for p in [5.0, 200.0, 395.0, 10.0] {
            o.update(p, 400.0);
        }
        assert!((o.total - 410.0).abs() < 1e-3);
        // Counted from a start line half way along the third side.
        let mut l = l;
        l.set_start(l.point(2, 0.5));
        assert!(l.lap_position(2, 0.5).abs() < 1e-2);
        assert!((l.lap_position(1, 0.5) - 300.0).abs() < 1e-2);
    }

    #[test]
    fn pacer_spreads_lap_times_and_scales() {
        // Seven on the grid with a player: three cars at 79, 80 and 81 s.
        let mut p = Pacer::new(79.0, 81.0, 7, 1, 1580.0).unwrap();
        assert_eq!(p.cars.len(), 3);
        assert!((p.cars[0].speed - 20.0).abs() < 1e-3);
        assert!((p.cars[2].speed - 1580.0 / 81.0).abs() < 1e-3);
        // The best three riders follow the fastest car.
        let cars = p.assign(&[0.1, 0.9, 0.5, 0.7, 0.3, 0.2]);
        assert_eq!(cars, vec![1, 0, 0, 0, 1, 1]);
        // The cars set off from the best rider's place on the grid, once.
        p.start(&[-30.0, -10.0, -40.0, -50.0, -60.0, -70.0]);
        p.start(&[0.0; 6]);
        p.tick(1.0, &[]);
        assert!((p.scale(0, 10.0) - 1.0).abs() < 1e-3);
        assert!((p.scale(0, 60.0) - 0.5).abs() < 1e-3);
        assert!((p.scale(0, -10.0) - 1.2).abs() < 1e-3);
        assert!((p.scale(0, -110.0) - 1.5).abs() < 1e-3);
        // A player pulls the car nearest them, at most 600 m.
        for _ in 0..200 {
            p.tick(1.0 / 60.0, &[-300.0]);
        }
        assert!((p.scale(2, -300.0) - 1.0).abs() < 0.01);
        assert!(Pacer::new(0.0, 0.0, 4, 1, 1000.0).is_none());
    }

    #[test]
    fn engine_boost_and_straights() {
        let l = square();
        let mut d = AiDriver::new(&l, Vec3::ZERO, 1);
        assert_eq!(d.torque_scale(), 1.0);
        d = d.with_engine_boost();
        assert_eq!(d.torque_scale(), 1.0);
        d.engine_delay = Some(0.0);
        assert_eq!(d.torque_scale(), PROTOTYPE.engine_boost);
        // Every side of the square turns 90 degrees into the next.
        assert_eq!(d.straight_ahead(&l), 0.0);
    }

    #[test]
    fn riders_keep_off_quads_alongside() {
        let l = square();
        let pos = Vec3::new(0.0, 0.0, -50.0);
        let fwd = Vec3::NEG_Z;
        let vel = fwd * 20.0;
        let quad = |x: f32, z: f32, speed: f32| Traffic {
            position: Vec3::new(x, 0.0, z),
            velocity: fwd * speed,
            player: false,
            boosting: false,
        };
        let mut d = AiDriver::new(&l, pos, 1);
        d.follow(&l, pos);
        let offset = |d: &mut AiDriver, traffic: &[Traffic]| {
            d.offset = 0.0;
            d.plan_passing(&l, pos, vel, fwd, 0.0, -5.0, 5.0, traffic, 1.0 / 60.0);
            d.offset
        };
        // Alone, it keeps to the middle.
        assert_eq!(offset(&mut d, &[]), 0.0);
        // A quad touching its right side moves its line left, clear of it.
        assert!((offset(&mut d, &[quad(1.0, -50.5, 20.0)]) + 0.6).abs() < 1e-4);
        // Hemmed in on both sides, it holds its line.
        assert_eq!(offset(&mut d, &[quad(1.0, -50.0, 20.0), quad(-1.0, -50.0, 20.0)]), 0.0);
        // A quad 4 m behind counts only when it closes 10 m/s faster.
        assert_eq!(offset(&mut d, &[quad(1.0, -46.0, 25.0)]), 0.0);
        assert!(offset(&mut d, &[quad(1.0, -46.0, 31.0)]) < -0.5);
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
