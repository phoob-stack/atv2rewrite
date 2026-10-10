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
        }
    }

    /// Whether the next pass of checkpoint 1 completes a lap.
    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn started(&self) -> bool {
        self.countdown <= 0.0
    }

    pub fn racing(&self) -> bool {
        self.started() && self.finished.is_none()
    }

    pub fn tick(&mut self, dt: f32) {
        if self.countdown > 0.0 {
            self.countdown -= dt;
        } else if self.finished.is_none() {
            self.clock += dt;
        }
    }

    /// The quad reached checkpoint `index` (0 = the start/finish line).
    pub fn reach(&mut self, index: usize) {
        if !self.racing() {
            return;
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
