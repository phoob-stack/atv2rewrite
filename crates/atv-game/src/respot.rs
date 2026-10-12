//! Automatic respots: the original puts a racer back on the course when
//! they drive into a `Respot` box, stray from the course or miss a
//! checkpoint (0x910f0 and 0x908f0, run from the racer update 0x26e30).
//! They are on in the shipped game (`mRespotsOn` in the disc's user.xml)
//! on every track whose Tracks.xml entry sets `mAllowsForcedRespots`, which
//! is all but the Stadium, and only in races: freestyle and the challenges
//! (game types 4, 5, 8 and 9) leave them out.
//!
//! * A `Respot` locator is a box centred on the locator, its scale the full
//!   size (0x652d0). A quad inside one is put back at once.
//! * More than 10 m outside the AI line's corridor for 5 s, it's put back,
//!   with "Return To Track - n" from the first second on.
//! * On a stretch of the line that leads to neither the next checkpoint nor
//!   the one just passed for 4 s, it's put back, with "Missed Checkpoint -
//!   n" from half a second on.
//! * Facing back down the line for 2 s shows "Wrong Way" (0x902f0); that
//!   one needs no `mAllowsForcedRespots`.
//!
//! None of it runs while the rider is off the quad. A respot puts the quad
//! on the line at the furthest point the rider reached where both ends of
//! the line's stretch allow a respot (0x90400, 0x8fd30). Every respot in a
//! race goes there, after a crash or on the rider's say-so too (0x90ac0);
//! with another racer within 10 m of the spot, racer i of n goes i/n of the
//! way across the corridor from its left edge, so player 1, the first,
//! goes to the left edge.
//!
//! A respot isn't sudden (0x25f20, 0x267f0): the racer fades out over half
//! a second, is put back, then blinks back in over three seconds, and
//! asking for another respot meanwhile does nothing.

use bevy::prelude::*;

use atv_formats::rom::Locator;
use atv_formats::row::World;
use atv_sim::ai::RacingLine;
use atv_sim::track::to_sim;
use atv_sim::TrackCollision;

/// Metres outside the corridor that count as off the course, and the
/// seconds off it before the warning and the respot (0x1c3148, 0x1c3144,
/// 0x1c3140).
const OFF_COURSE: f32 = 10.0;
const OFF_COURSE_WARN: f32 = 1.0;
const OFF_COURSE_RESPOT: f32 = 5.0;
/// Seconds on the wrong stretch before the warning and the respot
/// (0x1c3150, 0x1c314c).
const MISSED_WARN: f32 = 0.5;
const MISSED_RESPOT: f32 = 4.0;
/// Seconds facing back down the line before "Wrong Way" (0x1c3154).
pub const WRONG_WAY: f32 = 2.0;
/// This many corridor widths out, the quad's stretch is looked for over
/// the whole line rather than near the last one (0x1cd224).
const LOST: f32 = 3.0;
/// The sector of a node with no way on (none in the shipped lines).
const DEAD_END: i32 = -2;
/// With another racer this near the respot point, metres, the quad goes to
/// the edge of the corridor (0x1c3160, squared).
const CROWDED: f32 = 10.0;
/// Seconds fading out before the respot (0x25f20), and blinking after it
/// (a sixth of the time to half, 0x1b7940 and 0x1cd214).
const FADE_OUT: f32 = 0.5;
const BLINK: f32 = 3.0;
/// The blink: `(sin(34π × t/6 + 3π/2) + 1) / 2` (0x1b793c, 0x1b7938), so
/// from clear to solid nine times, the last as it ends.
const BLINK_RATE: f32 = 34.0 * std::f32::consts::PI / 6.0;

/// Where a respot is up to (the racer's state at +0x2c).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum Fade {
    #[default]
    None,
    /// Fading out, seconds left.
    Out(f32),
    /// Blinking, seconds since the racer was put back.
    Blink(f32),
}

impl Fade {
    /// Asks for a respot; false when one is already under way.
    pub fn ask(&mut self) -> bool {
        let idle = *self == Fade::None;
        if idle {
            *self = Fade::Out(FADE_OUT);
        }
        idle
    }

    /// Moves it on; true when the racer must be put back now.
    pub fn step(&mut self, dt: f32) -> bool {
        match *self {
            Fade::None => false,
            Fade::Out(left) if left > dt => {
                *self = Fade::Out(left - dt);
                false
            }
            Fade::Out(_) => {
                *self = Fade::Blink(0.0);
                true
            }
            Fade::Blink(t) => {
                *self = if t + dt < BLINK { Fade::Blink(t + dt) } else { Fade::None };
                false
            }
        }
    }

    /// How solid the racer is drawn.
    pub fn alpha(&self) -> f32 {
        match *self {
            Fade::None => 1.0,
            Fade::Out(left) => left / FADE_OUT,
            Fade::Blink(t) => 0.5 - 0.5 * (t * BLINK_RATE).cos(),
        }
    }
}

/// Shows player 1's quad and rider while the respot has them at least half
/// solid. The original draws them see-through (it sets the model's colour,
/// 0x267f0), but the rewrite's textures keep gloss in their alpha, so here
/// they blink out instead.
pub fn show_fade(race: Option<Res<crate::Race>>, mut quads: Query<&mut Visibility, With<crate::QuadBody>>) {
    let shown = race.is_none_or(|r| r.fade.alpha() >= 0.5);
    for mut vis in &mut quads {
        vis.set_if_neq(if shown { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// A `Respot` box.
#[derive(Debug, Clone, Copy)]
pub struct Zone {
    centre: Vec3,
    axes: [Vec3; 3],
    half: Vec3,
}

impl Zone {
    pub fn of(l: &Locator) -> Self {
        let r = &l.rotation;
        Zone {
            centre: to_sim(l.pos),
            axes: [to_sim([r[0], r[1], r[2]]), to_sim([r[3], r[4], r[5]]), to_sim([r[6], r[7], r[8]])],
            half: Vec3::from(l.scale).abs() * 0.5,
        }
    }

    pub fn centre(&self) -> Vec3 {
        self.centre
    }

    pub fn contains(&self, p: Vec3) -> bool {
        let d = p - self.centre;
        (0..3).all(|i| self.axes[i].dot(d).abs() <= self.half[i])
    }
}

/// The track's `Respot` boxes (any locator named `Respot...`, 0x65c50).
pub fn zones(world: &World) -> Vec<Zone> {
    world.locators.iter().filter(|l| l.name.get(..6).is_some_and(|n| n.eq_ignore_ascii_case("respot"))).map(Zone::of).collect()
}

/// Keeps watch over player 1 in a race.
pub struct Marshal {
    line: RacingLine,
    lap_len: f32,
    /// The gate each node leads to (an index into the lap's gates).
    sector: Vec<i32>,
    /// Each gate locator's stretch of line, and its gate.
    anchors: Vec<(usize, usize)>,
    zones: Vec<Zone>,
    /// Whether this track puts riders back (`mAllowsForcedRespots`).
    forced: bool,
    /// The quad's stretch of line and how far along it (unclamped).
    edge: usize,
    t: f32,
    /// Metres driven down the line, and the lap position it was last
    /// measured from.
    run: f32,
    lap_pos: f32,
    /// Where a respot puts the quad: stretch, how far along, and `run`
    /// there.
    spot: Option<(usize, f32, f32)>,
    off_course: f32,
    missed: f32,
    /// Seconds spent facing back down the line.
    pub backwards: f32,
}

impl Marshal {
    /// `gates` are the lap's gate locators, each its gate's index and the
    /// middle of its face, in the track's locator order.
    pub fn new(line: RacingLine, gates: &[(usize, Vec3)], zones: Vec<Zone>, forced: bool, start: Vec3) -> Option<Self> {
        if line.is_empty() {
            return None;
        }
        let anchors: Vec<(usize, usize)> = gates.iter().map(|&(i, g)| (nearest(&line, 0..line.edges.len(), g).0, i)).collect();
        let sector = sectors(&line, &anchors);
        let (edge, t) = nearest(&line, 0..line.edges.len(), start);
        let lap_pos = line.lap_position(edge, t);
        Some(Marshal { lap_len: line.lap_length(), line, sector, anchors, zones, forced, edge, t, run: 0.0, lap_pos, spot: None, off_course: 0.0, missed: 0.0, backwards: 0.0 })
    }

    /// One step of the race, with the rider on the quad. `next` is the
    /// index of the next gate of `gates`. True when the quad must be put
    /// back.
    pub fn watch(&mut self, p: Vec3, forward: Vec3, next: usize, gates: usize, dt: f32) -> bool {
        if self.forced && self.zones.iter().any(|z| z.contains(p)) {
            return true;
        }
        self.follow(p);
        let lap_pos = self.line.lap_position(self.edge, self.t);
        let mut moved = lap_pos - self.lap_pos;
        if moved > self.lap_len * 0.5 {
            moved -= self.lap_len;
        } else if moved < -self.lap_len * 0.5 {
            moved += self.lap_len;
        }
        self.run += moved;
        self.lap_pos = lap_pos;

        let e = &self.line.edges[self.edge];
        let (a, b) = (self.sector[e.from], self.sector[e.to]);
        // Neither end leads to the next gate or the last one passed.
        let wrong = gates > 0 && {
            let (next, prev) = (next as i32, ((next + gates - 1) % gates) as i32);
            a != next && b != next && a != prev && b != prev && a != DEAD_END
        };
        self.backwards = if forward.dot(e.dir) < 0.0 { self.backwards + dt } else { 0.0 };
        if !wrong && gates > 0 {
            self.keep_spot(next);
        }
        if !self.forced {
            return false;
        }
        // The original measures from the middle of the corridor but
        // against its edges, so a lopsided corridor reaches further on its
        // wide side (0x8fba0).
        let (l, r) = self.line.corridor(self.edge, self.t);
        let mid = self.lateral(self.edge, p) - (l + r) * 0.5;
        let out = if mid >= 0.0 { mid - r > OFF_COURSE } else { mid - l < -OFF_COURSE };
        self.off_course = if out { self.off_course + dt } else { 0.0 };
        if self.off_course > OFF_COURSE_RESPOT {
            return true;
        }
        self.missed = if wrong { self.missed + dt } else { 0.0 };
        self.missed > MISSED_RESPOT
    }

    /// The warning over the race, if any (strings 0xe0, 0xdd and 0xe3).
    pub fn warning(&self) -> Option<String> {
        // The seconds left, rounded up.
        let left = |limit: f32, t: f32| (limit - t + 1.0) as i32;
        if self.off_course > OFF_COURSE_WARN {
            Some(left(OFF_COURSE_RESPOT, self.off_course)).filter(|&n| n > 0).map(|n| format!("RETURN TO TRACK - {n}"))
        } else if self.missed > MISSED_WARN {
            Some(left(MISSED_RESPOT, self.missed)).filter(|&n| n > 0).map(|n| format!("MISSED CHECKPOINT - {n}"))
        } else if self.backwards > WRONG_WAY {
            Some("WRONG WAY".to_string())
        } else {
            None
        }
    }

    /// Where to put the quad back, facing down the line, and starts the
    /// watch over from there. `others` are where the other racers are.
    pub fn respot(&mut self, collision: &TrackCollision, others: &[Vec3]) -> (Vec3, Quat) {
        let (e, t) = match self.spot {
            Some((e, t, run)) => {
                self.run = run;
                (e, t)
            }
            None => {
                // Back down the line to a node a respot may use.
                let mut e = self.edge;
                for _ in 0..self.line.edges.len() {
                    let from = &self.line.nodes[self.line.edges[e].from];
                    match from.inc.first() {
                        Some(&p) if !from.respot => e = p,
                        _ => break,
                    }
                }
                (e, self.t)
            }
        };
        let t = t.clamp(0.0, 1.0);
        let mut p = self.line.point(e, t);
        if others.iter().any(|o| o.distance(p) < CROWDED) {
            // Player 1 is the first racer: the corridor's left edge.
            p += self.line.edges[e].side * self.line.corridor(e, t).0;
        }
        // The nearer ground over or under the line.
        let ground = [Vec3::NEG_Y, Vec3::Y].into_iter().filter_map(|d| collision.raycast(p, d, 200.0)).min_by(|a, b| a.t.total_cmp(&b.t)).map_or(p.y, |h| h.point.y);
        let fwd = self.line.edges[e].dir.with_y(0.0).normalize_or(Vec3::NEG_Z);
        self.edge = e;
        self.t = t;
        self.lap_pos = self.line.lap_position(e, t);
        self.off_course = 0.0;
        self.missed = 0.0;
        self.backwards = 0.0;
        (p.with_y(ground + 0.6), Quat::from_rotation_arc(Vec3::NEG_Z, fwd))
    }

    /// Sideways from the line on `edge`, metres (right is positive).
    fn lateral(&self, edge: usize, p: Vec3) -> f32 {
        let e = &self.line.edges[edge];
        (p - self.line.nodes[e.from].pos).dot(e.side)
    }

    /// Finds the quad's stretch near the last one, or over the whole line
    /// when it's far out (0x659c0, 0x64a40).
    fn follow(&mut self, p: Vec3) {
        let mut near = vec![self.edge];
        for _ in 0..2 {
            for i in 0..near.len() {
                let e = &self.line.edges[near[i]];
                for n in [e.from, e.to] {
                    for &x in self.line.nodes[n].out.iter().chain(&self.line.nodes[n].inc) {
                        if !near.contains(&x) {
                            near.push(x);
                        }
                    }
                }
            }
        }
        let (mut e, mut t) = nearest(&self.line, near.into_iter(), p);
        let lat = self.lateral(e, p);
        let (l, r) = self.line.corridor(e, t);
        if (lat < 0.0 && lat < l * LOST) || (lat > 0.0 && lat > r * LOST) {
            (e, t) = nearest(&self.line, 0..self.line.edges.len(), p);
        }
        self.edge = e;
        self.t = t;
    }

    /// Moves the respot point up to the quad when it has got further than
    /// before on a stretch inside one sector whose ends both allow a respot,
    /// and not the next gate's own stretch.
    fn keep_spot(&mut self, next: usize) {
        let e = &self.line.edges[self.edge];
        let good = self.t <= 1.0
            && self.sector[e.from] == self.sector[e.to]
            && self.line.nodes[e.from].respot
            && self.line.nodes[e.to].respot
            && !self.anchors.contains(&(self.edge, next));
        match self.spot {
            Some((_, _, run)) if !(good && self.run > run) => {}
            _ => self.spot = Some((self.edge, self.t, self.run)),
        }
    }
}

/// The original's measure from `p` to a stretch (0x64310): the sideways
/// distance squared alongside it, else the squared distance to the nearer
/// end. Returns how far along the stretch `p` is (unclamped) too.
fn measure(line: &RacingLine, edge: usize, p: Vec3) -> (f32, f32) {
    let e = &line.edges[edge];
    let a = line.nodes[e.from].pos;
    let d = p - a;
    let t = d.dot(e.dir) / e.len;
    let m = if (0.0..1.0).contains(&t) {
        d.dot(e.side).powi(2)
    } else {
        p.distance_squared(if t < 0.0 { a } else { line.nodes[e.to].pos })
    };
    (t, m)
}

/// The stretch of `edges` nearest `p`, and how far along it.
fn nearest(line: &RacingLine, edges: impl Iterator<Item = usize>, p: Vec3) -> (usize, f32) {
    let mut best = (0, 0.0, f32::MAX);
    for e in edges {
        let (t, m) = measure(line, e, p);
        if m < best.2 {
            best = (e, t, m);
        }
    }
    (best.0, best.1)
}

/// Which gate each node leads to (0x65230): each gate's stretch starts the
/// gate's sector, which runs back down the line to the stretch of another
/// gate, gate by gate in locator order, a later walk overwriting an earlier
/// one; nodes off those walks (shortcuts) take the sector of the node they
/// lead to.
fn sectors(line: &RacingLine, anchors: &[(usize, usize)]) -> Vec<i32> {
    let n = line.nodes.len();
    let starts: Vec<usize> = anchors.iter().map(|&(e, _)| line.edges[e].from).collect();
    let mut sector = vec![-1; n];
    let walk_back = |sector: &mut Vec<i32>, from: usize, s: i32| {
        sector[from] = s;
        let mut at = from;
        for _ in 0..n {
            let Some(&e) = line.nodes[at].inc.first() else { break };
            at = line.edges[e].from;
            if starts.contains(&at) {
                break;
            }
            sector[at] = s;
        }
    };
    for (&s, &(_, gate)) in starts.iter().zip(anchors) {
        walk_back(&mut sector, s, gate as i32);
    }
    loop {
        let mut changed = false;
        for i in 0..n {
            if sector[i] != -1 {
                continue;
            }
            let s = line.nodes[i].out.first().map_or(DEAD_END, |&e| sector[line.edges[e].to]);
            if s != -1 {
                walk_back(&mut sector, i, s);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    sector
}

#[cfg(test)]
mod tests {
    use super::*;
    use atv_formats::aid::{AiNode, AiPath};

    /// A square loop 400 m round, a node every 50 m half a metre over the
    /// ground, the corridor 6 m each side.
    fn square() -> RacingLine {
        let mut nodes = Vec::new();
        let corners = [[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]];
        for i in 0..8 {
            let (a, b) = (corners[i / 2], corners[(i / 2 + 1) % 4]);
            let f = (i % 2) as f32 * 0.5;
            let pos = [a[0] + (b[0] - a[0]) * f, 0.5, a[1] + (b[1] - a[1]) * f];
            nodes.push(AiNode { name: String::new(), pos, forward: [0.0; 3], up: [0.0, 1.0, 0.0], left: -6.0, right: 6.0, prev: vec![(i + 7) % 8], next: vec![(i + 1) % 8], speed_limit: None, flags: 1, boost: 0 });
        }
        RacingLine::from_path(&AiPath { nodes })
    }

    /// Gate middles just past every other node.
    fn gates() -> Vec<(usize, Vec3)> {
        [[1.0, 0.0, 0.0], [100.0, 0.0, 1.0], [99.0, 0.0, 100.0], [0.0, 0.0, 99.0]].map(to_sim).into_iter().enumerate().collect()
    }

    fn ground() -> TrackCollision {
        let c = [[-50.0, -50.0], [150.0, -50.0], [150.0, 150.0], [-50.0, 150.0]].map(|[x, z]| to_sim([x, 0.0, z]));
        TrackCollision::new(vec![[c[0], c[1], c[2]], [c[0], c[2], c[3]]], 10.0)
    }

    fn at(x: f32, z: f32) -> Vec3 {
        to_sim([x, 0.0, z])
    }

    #[test]
    fn sectors_run_back_from_each_gate() {
        let line = square();
        let anchors: Vec<(usize, usize)> = gates().iter().map(|&(i, g)| (nearest(&line, 0..line.edges.len(), g).0, i)).collect();
        assert_eq!(anchors.iter().map(|&(e, _)| line.edges[e].from).collect::<Vec<_>>(), [0, 2, 4, 6]);
        assert_eq!(sectors(&line, &anchors), [0, 1, 1, 2, 2, 3, 3, 0]);
    }

    #[test]
    fn respots_off_course_and_on_missed_checkpoints() {
        let mut m = Marshal::new(square(), &gates(), Vec::new(), true, at(60.0, 0.0)).unwrap();
        let east = to_sim([1.0, 0.0, 0.0]);
        let dt = 0.125;
        // Down the first side for the second gate.
        for i in 0..20 {
            assert!(!m.watch(at(60.0 + i as f32, 0.0), east, 1, 4, dt));
        }
        assert_eq!(m.warning(), None);
        // 20 m off to the side: put back after 5 s.
        let mut respot = None;
        for i in 0..60 {
            if m.watch(at(80.0, -20.0), east, 1, 4, dt) {
                respot = Some(i);
                break;
            }
            if i == 12 {
                assert_eq!(m.warning().as_deref(), Some("RETURN TO TRACK - 4"));
            }
        }
        assert_eq!(respot, Some(40));
        // On the line as far as it got (alongside counts), on the ground,
        // facing on.
        let (p, rot) = m.respot(&ground(), &[at(60.0, 0.0)]);
        assert!((p - to_sim([80.0, 0.6, 0.0])).length() < 0.01, "{p}");
        assert!((rot * Vec3::NEG_Z).dot(east) > 0.99);
        assert_eq!(m.warning(), None);
        // A sector back, behind the gate just passed: no harm.
        for _ in 0..60 {
            assert!(!m.watch(at(0.0, 30.0), to_sim([0.0, 0.0, -1.0]), 1, 4, dt));
        }
        // Two sectors on, with the second gate never passed.
        let mut respot = None;
        for i in 0..60 {
            if m.watch(at(100.0, 70.0), to_sim([0.0, 0.0, 1.0]), 1, 4, dt) {
                respot = Some(i);
                break;
            }
            if i == 8 {
                assert_eq!(m.warning().as_deref(), Some("MISSED CHECKPOINT - 3"));
            }
        }
        assert_eq!(respot, Some(32));
    }

    #[test]
    fn crowded_spots_go_to_the_left_edge() {
        let mut m = Marshal::new(square(), &gates(), Vec::new(), true, at(60.0, 0.0)).unwrap();
        let east = to_sim([1.0, 0.0, 0.0]);
        for i in 0..20 {
            m.watch(at(60.0 + i as f32, 0.0), east, 1, 4, 0.125);
        }
        // Someone 8 m along: over to the left of the way on, 6 m out.
        let (p, rot) = m.respot(&ground(), &[at(87.0, 3.0)]);
        let left = rot * Vec3::NEG_X;
        assert!((p - to_sim([79.0, 0.6, 0.0]) - left * 6.0).length() < 0.01, "{p}");
    }

    #[test]
    fn respots_fade_out_and_blink_back() {
        let mut f = Fade::default();
        assert!(f.ask());
        assert!(!f.ask());
        let dt = 1.0 / 64.0;
        let steps = (0..100).position(|_| f.step(dt)).unwrap();
        assert_eq!(steps, 31);
        // Clear as it's put back, then solid nine times.
        assert!(f.alpha() < 0.01);
        assert!(!f.ask());
        let mut solid = 0;
        let mut last = 0.0;
        while f != Fade::None {
            f.step(dt);
            let a = f.alpha();
            solid += (a > 0.9 && last <= 0.9) as u32;
            last = a;
        }
        assert_eq!(solid, 9);
        assert!(f.ask());
    }

    #[test]
    fn wrong_way_needs_no_forced_respots() {
        let mut m = Marshal::new(square(), &gates(), Vec::new(), false, at(10.0, 0.0)).unwrap();
        for _ in 0..25 {
            assert!(!m.watch(at(50.0, -30.0), to_sim([-1.0, 0.0, 0.0]), 1, 4, 0.125));
        }
        assert_eq!(m.warning().as_deref(), Some("WRONG WAY"));
    }

    #[test]
    fn respot_boxes() {
        let l = Locator { name: "Respot".into(), pos: [10.0, 0.0, 20.0], scale: [8.0, 4.0, 2.0], rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] };
        let z = Zone::of(&l);
        assert!(z.contains(to_sim([13.9, 1.9, 20.9])));
        assert!(!z.contains(to_sim([14.1, 0.0, 20.0])));
        assert!(!z.contains(to_sim([10.0, 0.0, 21.1])));
        let zone = Zone::of(&Locator { pos: [50.0, 0.0, 0.0], ..l });
        let mut m = Marshal::new(square(), &gates(), vec![zone], true, at(10.0, 0.0)).unwrap();
        assert!(m.watch(at(50.0, 0.5), to_sim([1.0, 0.0, 0.0]), 1, 4, 0.125));
    }
}
