//! Race rules on top of the driving: the start countdown, checkpoints and
//! laps, timing, and the wrong-way warning. The HUD draws what this keeps.

use bevy::prelude::*;

use atv_sim::track as trk;

#[derive(Resource, Debug)]
pub struct RaceProgress {
    pub laps: u32,
    /// Seconds until the lights go green; the quad is held until then.
    pub countdown: f32,
    /// Race time since the start.
    pub clock: f32,
    pub lap_start: f32,
    pub lap_times: Vec<f32>,
    /// Total time, once the last lap is done.
    pub finished: Option<f32>,
    /// Whether the next pass of checkpoint 1 completes a lap (false while
    /// a grid behind the line has yet to cross it).
    armed: bool,
    /// Race time of the last checkpoint and of the last completed lap.
    pub last_checkpoint: Option<f32>,
    pub last_lap: Option<f32>,
    /// Seconds spent going the wrong way.
    pub wrong_way: f32,
    /// "Return To Track", "Missed Checkpoint" or "Wrong Way", as the
    /// original warns a racer (see [`crate::respot`]).
    pub warning: Option<String>,
    /// How far round the current lap the quad is, 0..1.
    pub lap_fraction: f32,
    /// Arcade mode's checkpoint clock, if this race has one.
    pub arcade: Option<ArcadeClock>,
    /// Race time after which a rider still out on the course doesn't
    /// finish (career's `mTimePercentage` of the winner's time).
    pub cutoff: Option<f32>,
    pub dnf: bool,
}

/// Arcade's countdown: the race starts with `mArcadeStartTime` seconds
/// (Tracks.xml, 0x7e610) and each arcade checkpoint adds the seconds its
/// locator names (`Checkpoint 3 Arcade 30`, 0x648f0). Running out ends the
/// race.
#[derive(Debug, Clone)]
pub struct ArcadeClock {
    pub left: f32,
    /// Checkpoint index and the seconds it adds.
    gates: Vec<(usize, f32)>,
    /// Race time and seconds of the last extension.
    pub last_bonus: Option<(f32, f32)>,
    pub out_of_time: bool,
}

impl ArcadeClock {
    /// `gates`: from [`arcade_gates`].
    pub fn new(start: f32, gates: Vec<(usize, f32)>) -> Self {
        Self { left: start, gates, last_bonus: None, out_of_time: false }
    }
}

impl RaceProgress {
    pub const COUNTDOWN: f32 = 3.0;

    /// `armed`: the quad starts on or past checkpoint 1.
    pub fn new(laps: u32, armed: bool) -> Self {
        Self {
            laps: laps.max(1),
            countdown: Self::COUNTDOWN,
            clock: 0.0,
            lap_start: 0.0,
            lap_times: Vec::new(),
            finished: None,
            armed,
            last_checkpoint: None,
            last_lap: None,
            wrong_way: 0.0,
            warning: None,
            lap_fraction: 0.0,
            arcade: None,
            cutoff: None,
            dnf: false,
        }
    }

    pub fn with_arcade(mut self, clock: ArcadeClock) -> Self {
        self.arcade = Some(clock);
        self
    }

    pub fn out_of_time(&self) -> bool {
        self.arcade.as_ref().is_some_and(|a| a.out_of_time)
    }

    /// Finished, out of time, or past the cutoff.
    pub fn over(&self) -> bool {
        self.finished.is_some() || self.out_of_time() || self.dnf
    }

    /// Whether the next pass of checkpoint 1 completes a lap.
    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn started(&self) -> bool {
        self.countdown <= 0.0
    }

    pub fn racing(&self) -> bool {
        self.started() && !self.over()
    }

    pub fn tick(&mut self, dt: f32) {
        if self.countdown > 0.0 {
            self.countdown -= dt;
        } else if !self.over() {
            self.clock += dt;
            if self.cutoff.is_some_and(|c| self.clock >= c) {
                self.dnf = true;
            }
            if let Some(a) = self.arcade.as_mut() {
                a.left -= dt;
                if a.left <= 0.0 {
                    a.left = 0.0;
                    a.out_of_time = true;
                }
            }
        }
    }

    /// The quad reached checkpoint `index` (0 = the start/finish line).
    pub fn reach(&mut self, index: usize) {
        if !self.racing() {
            return;
        }
        if let Some(a) = self.arcade.as_mut()
            && let Some(&(_, secs)) = a.gates.iter().find(|g| g.0 == index)
        {
            a.left += secs;
            a.last_bonus = Some((self.clock, secs));
        }
        if index != 0 {
            self.last_checkpoint = Some(self.clock);
            return;
        }
        // From a grid behind the line, crossing it is part of lap 1.
        if !self.armed {
            self.armed = true;
            return;
        }
        self.lap_times.push(self.clock - self.lap_start);
        self.lap_start = self.clock;
        self.last_lap = Some(self.clock);
        if self.lap_times.len() as u32 >= self.laps {
            self.finished = Some(self.clock);
        }
    }

    /// The lap being driven, 1-based.
    pub fn lap(&self) -> u32 {
        (self.lap_times.len() as u32 + 1).min(self.laps)
    }

    pub fn lap_time(&self) -> f32 {
        if self.finished.is_some() { self.lap_times.last().copied().unwrap_or(0.0) } else { self.clock - self.lap_start }
    }

    pub fn best_lap(&self) -> Option<f32> {
        self.lap_times.iter().copied().reduce(f32::min)
    }

    /// Time Trial's lap list (0x9fbd0): the last laps, oldest first, then
    /// the lap being ridden (`true`), four rows at most. The original
    /// loses lap 4 when it shifts the list; this keeps every lap.
    pub fn recent_laps(&self) -> Vec<(f32, bool)> {
        let riding = self.finished.is_none();
        let keep = if riding { 3 } else { 4 };
        let done = &self.lap_times[self.lap_times.len().saturating_sub(keep)..];
        let current = riding.then(|| (if self.started() { self.lap_time() } else { 0.0 }, true));
        done.iter().map(|&t| (t, false)).chain(current).collect()
    }
}

/// The arcade checkpoints, as indices into [`atv_sim::track::checkpoints`],
/// and the seconds each adds: the number after `Arcade` in the locator's
/// name (`Checkpoint 3 Arcade 30`, read by 0x65c50), added up over every
/// locator with that number (Swamp 2 has each of its four twice, and the
/// original counts each). The start line adds nothing.
///
/// The original matches `Arcade` by case, which leaves out Dunes 3's and
/// Glacier 3's `arcade 30` and Forest 3's `Acade 30`; with those gone,
/// rounds 11, 12 and 15 can't be won in time, so here any case and the
/// misspelling count.
pub fn arcade_gates(world: &atv_formats::row::World) -> Vec<(usize, f32)> {
    let number = |name: &str| trk::checkpoint_number(name);
    let mut all: Vec<u32> = world.locators.iter().filter_map(|l| number(&l.name)).collect();
    all.sort_unstable();
    all.dedup();
    // As `atol` reads it: the digits after any spaces; 0 is no time.
    let seconds = |name: &str| -> Option<f32> {
        let name = name.to_ascii_lowercase();
        let rest = name[name.find("cade")? + "cade".len()..].trim_start();
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok().filter(|&s| s > 0).map(|s| s as f32)
    };
    let mut gates: Vec<(usize, f32)> = Vec::new();
    for l in &world.locators {
        let Some((i, secs)) = (|| Some((all.binary_search(&number(&l.name)?).ok()?, seconds(&l.name)?)))() else { continue };
        match gates.iter_mut().find(|g| g.0 == i) {
            Some(g) => g.1 += secs,
            None => gates.push((i, secs)),
        }
    }
    gates.sort_by_key(|g| g.0);
    gates
}

/// The quad's hull for touching a gate: three spheres along it (front
/// rack, middle, rear rack), each about as wide as the quad.
const HULL: [f32; 3] = [0.75, 0.0, -0.85];
const HULL_RADIUS: f32 = 0.6;

/// One face of a gate: a rectangle, `half` its half-size in its own x
/// and y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub center: Vec3,
    rot: Quat,
    half: Vec2,
}

impl Face {
    /// The far (+Z, in the game's frame) face of a locator's box (0x64ac0):
    /// `pos ± sx/2·R0 ± sy/2·R1 + sz/2·R2`.
    fn of(l: &atv_formats::rom::Locator) -> Self {
        let rot = trk::rotation_to_sim(&l.rotation);
        let half = Vec3::from(l.scale).abs() * 0.5;
        // The sim frame mirrors Z, so the game's +Z is the box's local -Z.
        Self { center: trk::to_sim(l.pos) + rot * Vec3::new(0.0, 0.0, -half.z), rot, half: half.truncate() }
    }

    /// How far `p` is from the rectangle.
    fn distance(&self, p: Vec3) -> f32 {
        let l = self.rot.inverse() * (p - self.center);
        let on = l.truncate().clamp(-self.half, self.half);
        (l - on.extend(0.0)).length()
    }
}

/// A checkpoint gate: the face of each locator with its number (a few
/// tracks repeat a number, and either copy counts). The original passes a
/// checkpoint when the quad's hull touches it (0x64440).
#[derive(Clone, Debug, PartialEq)]
pub struct Gate {
    faces: Vec<Face>,
}

impl Gate {
    /// Where the guide arrow points.
    pub fn center(&self) -> Vec3 {
        self.faces[0].center
    }

    /// Whether a hull sphere at any of `points` touches a face.
    pub fn touched(&self, points: &[Vec3]) -> bool {
        self.faces.iter().any(|f| points.iter().any(|&p| f.distance(p) <= HULL_RADIUS))
    }
}

/// The gates of the locators `number` gives a number, in its order.
pub fn gates(world: &atv_formats::row::World, number: impl Fn(&str) -> Option<u32>) -> Vec<Gate> {
    let mut faces: Vec<(u32, Face)> = world.locators.iter().filter_map(|l| Some((number(&l.name)?, Face::of(l)))).collect();
    faces.sort_by_key(|f| f.0);
    let mut gates: Vec<(u32, Gate)> = Vec::new();
    for (id, f) in faces {
        match gates.last_mut() {
            Some(g) if g.0 == id => g.1.faces.push(f),
            _ => gates.push((id, Gate { faces: vec![f] })),
        }
    }
    gates.into_iter().map(|g| g.1).collect()
}

/// A lap's gates, in the order of [`atv_sim::track::checkpoints`].
pub fn lap_gates(world: &atv_formats::row::World) -> Vec<Gate> {
    gates(world, trk::checkpoint_number)
}

/// The middle of each lap gate locator's face with the index of its gate
/// in [`lap_gates`], in the track's locator order (the order the original
/// sets its gates up in, 0x65c50).
pub fn lap_gate_faces(world: &atv_formats::row::World) -> Vec<(usize, Vec3)> {
    let mut numbers: Vec<u32> = world.locators.iter().filter_map(|l| trk::checkpoint_number(&l.name)).collect();
    numbers.sort_unstable();
    numbers.dedup();
    world.locators.iter().filter_map(|l| Some((numbers.binary_search(&trk::checkpoint_number(&l.name)?).ok()?, Face::of(l).center))).collect()
}

/// Points along the quad for its hull spheres.
pub fn hull(v: &atv_sim::vehicle::Vehicle) -> [Vec3; 3] {
    let mid = v.position + v.up() * 0.3;
    HULL.map(|z| mid + v.forward() * z)
}

/// `m:ss.hh`, as the original shows race and lap times.
pub fn fmt_time(t: f32) -> String {
    let hundredths = (t.max(0.0) * 100.0) as u32;
    format!("{}:{:02}.{:02}", hundredths / 6000, hundredths / 100 % 60, hundredths % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laps_and_finish() {
        let mut p = RaceProgress::new(2, true);
        p.reach(1);
        assert!(p.last_checkpoint.is_none(), "held during the countdown");
        p.tick(RaceProgress::COUNTDOWN + 0.01);
        for _ in 0..100 {
            p.tick(0.1);
        }
        p.reach(1);
        p.reach(0);
        assert_eq!(p.lap_times.len(), 1);
        assert_eq!(p.lap(), 2);
        p.tick(5.0);
        p.reach(0);
        assert!(p.finished.is_some());
        assert_eq!(p.best_lap(), Some(5.0));
        assert_eq!(fmt_time(65.432), "1:05.43");
    }

    #[test]
    fn recent_laps_keep_the_last_three_and_the_lap_being_ridden() {
        let mut p = RaceProgress::new(6, true);
        assert_eq!(p.recent_laps(), vec![(0.0, true)], "the current lap reads 0 before the start");
        p.tick(RaceProgress::COUNTDOWN + 0.01);
        for lap in 1..=5 {
            p.tick(lap as f32);
            p.reach(0);
        }
        p.tick(0.5);
        let laps: Vec<_> = p.recent_laps().into_iter().map(|(t, current)| ((t * 10.0).round() as u32, current)).collect();
        assert_eq!(laps, vec![(30, false), (40, false), (50, false), (5, true)]);
        p.tick(6.0);
        p.reach(0);
        assert!(p.finished.is_some());
        let laps: Vec<_> = p.recent_laps().into_iter().map(|(t, current)| ((t * 10.0).round() as u32, current)).collect();
        assert_eq!(laps, vec![(30, false), (40, false), (50, false), (65, false)], "no lap is ridden after the finish");
    }

    #[test]
    fn arcade_clock() {
        let mut p = RaceProgress::new(1, true).with_arcade(ArcadeClock::new(5.0, vec![(3, 30.0), (7, 20.0)]));
        p.tick(RaceProgress::COUNTDOWN + 0.01);
        p.tick(1.0);
        p.reach(3);
        assert!((p.arcade.as_ref().unwrap().left - 34.0).abs() < 0.01);
        p.reach(4);
        assert!((p.arcade.as_ref().unwrap().left - 34.0).abs() < 0.01, "plain checkpoints add nothing");
        p.tick(50.0);
        assert!(p.out_of_time() && !p.racing());
        let clock = p.clock;
        p.tick(1.0);
        assert_eq!(p.clock, clock, "the clock stops once out of time");
    }

    #[test]
    fn arcade_seconds_from_names() {
        let locator = |name: &str| atv_formats::rom::Locator { name: name.to_string(), pos: [0.0; 3], scale: [1.0; 3], rotation: [0.0; 9] };
        let names = ["Startpoint 1", "Checkpoint 2", "Checkpoint 3 Arcade 30", "checkpoint 4 Arcade 15", "Checkpoint 5 arcade 30", "Checkpoint 6 Acade 30", "Checkpoint 7 Arcade 20", "Checkpoint 7 Arcade 20", "Checkpoint 8 Arcade 0"];
        let world = atv_formats::row::World { fog_color: 0, materials: vec![], locators: names.map(locator).to_vec(), lights: vec![], instances: vec![], sprites: vec![], meshes: vec![] };
        // The start line and checkpoints 2..8 are indices 0..7; a repeated
        // one adds twice.
        assert_eq!(arcade_gates(&world), vec![(2, 30.0), (3, 15.0), (4, 30.0), (5, 30.0), (6, 40.0)]);
    }

    #[test]
    fn lap_gates_from_the_line() {
        let locator = |name: &str, x: f32, z: f32| atv_formats::rom::Locator { name: name.to_string(), pos: [x, 0.0, z], scale: [20.0, 10.0, 1.0], rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] };
        let locators = vec![locator("Checkpoint 3", 0.0, 200.0), locator("Startpoint 1", 0.0, 0.0), locator("Checkpoint 2", 0.0, 100.0), locator("checkpoint 2 ", 80.0, 100.0), locator("Respot", 0.0, 50.0)];
        let world = atv_formats::row::World { fog_color: 0, materials: vec![], locators, lights: vec![], instances: vec![], sprites: vec![], meshes: vec![] };
        let gates = lap_gates(&world);
        assert_eq!(gates.len(), 3);
        // The line first; game z 0.5 is sim -0.5.
        assert_eq!(gates[0].center(), Vec3::new(0.0, 0.0, -0.5));
        // Either copy of checkpoint 2, anywhere across its 20 m.
        assert!(gates[1].touched(&[Vec3::new(9.5, 0.0, -100.5)]));
        assert!(gates[1].touched(&[Vec3::new(75.0, 0.0, -100.5)]));
        assert!(!gates[1].touched(&[Vec3::new(40.0, 0.0, -100.5)]));
    }

    #[test]
    fn grid_behind_the_line() {
        let mut p = RaceProgress::new(3, false);
        p.tick(RaceProgress::COUNTDOWN + 1.0);
        p.tick(1.0);
        p.reach(0);
        assert!(p.lap_times.is_empty(), "first pass starts lap 1");
        p.tick(10.0);
        p.reach(0);
        assert_eq!(p.lap_times, vec![11.0], "the run up to the line counts towards lap 1");
    }
}
