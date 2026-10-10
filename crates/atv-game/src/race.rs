//! Race rules on top of the driving: the start countdown, checkpoints and
//! laps, timing, and the wrong-way warning. The HUD draws what this keeps.

use bevy::prelude::*;

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
    /// Seconds spent driving away from the next checkpoint.
    pub wrong_way: f32,
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
/// (Tracks.xml) and the arcade checkpoints (`Checkpoint N arcade`
/// locators) and the line each add time. Running out ends the race.
#[derive(Debug, Clone)]
pub struct ArcadeClock {
    pub left: f32,
    /// Checkpoint index (0 = the line) and the seconds it adds.
    gates: Vec<(usize, f32)>,
    /// Race time and seconds of the last extension.
    pub last_bonus: Option<(f32, f32)>,
    pub out_of_time: bool,
}

impl ArcadeClock {
    /// `gates`: indices of the arcade checkpoints in lap order (without the
    /// line). Each gate, and the line, adds the share of `lap_time` it takes
    /// to reach the next one (the disc gives no per-gate figure; this is the
    /// track's slowest AI lap time spread over the lap).
    pub fn new(start: f32, lap_time: f32, checkpoints: usize, gates: &[usize]) -> Self {
        let n = checkpoints.max(1);
        let mut all: Vec<usize> = std::iter::once(0).chain(gates.iter().copied().filter(|&g| g > 0 && g < n)).collect();
        all.sort_unstable();
        all.dedup();
        let gates = all
            .iter()
            .enumerate()
            .map(|(i, &g)| {
                let next = all.get(i + 1).copied().unwrap_or(n);
                (g, lap_time * (next - g) as f32 / n as f32)
            })
            .collect();
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
}

/// Indices into [`atv_sim::track::checkpoints`] of the checkpoints that
/// also have an arcade locator (`Checkpoint 3 arcade`; Forest 3 spells it
/// `acade`).
pub fn arcade_gates(world: &atv_formats::row::World) -> Vec<usize> {
    let number = |name: &str| -> Option<u32> { name.trim().to_ascii_lowercase().strip_prefix("checkpoint ")?.split_whitespace().next()?.parse().ok() };
    let mut all: Vec<u32> = world.locators.iter().filter_map(|l| number(&l.name)).collect();
    all.sort_unstable();
    all.dedup();
    let arcade: Vec<u32> = world
        .locators
        .iter()
        .filter(|l| l.name.to_ascii_lowercase().contains("cade"))
        .filter_map(|l| number(&l.name))
        .collect();
    all.iter().enumerate().filter(|(_, n)| arcade.contains(n)).map(|(i, _)| i).collect()
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
    fn arcade_clock() {
        // 10 checkpoints, gates at 3 and 7: the line buys 3/10 of a lap,
        // gate 3 buys 4/10 and gate 7 the last 3/10.
        let mut p = RaceProgress::new(1, true).with_arcade(ArcadeClock::new(5.0, 100.0, 10, &[3, 7]));
        p.tick(RaceProgress::COUNTDOWN + 0.01);
        p.tick(1.0);
        p.reach(3);
        assert!((p.arcade.as_ref().unwrap().left - 44.0).abs() < 0.01);
        p.reach(4);
        assert!((p.arcade.as_ref().unwrap().left - 44.0).abs() < 0.01, "plain checkpoints add nothing");
        p.tick(50.0);
        assert!(p.out_of_time() && !p.racing());
        let clock = p.clock;
        p.tick(1.0);
        assert_eq!(p.clock, clock, "the clock stops once out of time");
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
