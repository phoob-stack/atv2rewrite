//! Replays: every race is recorded as it runs, and the results can play it
//! back.
//!
//! The recording keeps what is drawn rather than what was simulated: each
//! quad's body and wheels, each rider's seat and joints, and the props
//! knocked about, sampled [`RATE`] times a second. Playback freezes the race
//! (as the pause menu does) and puts every one of those back where it was,
//! so tricks, crashes and kicks come back exactly as they looked.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::convert::WheelVisual;
use crate::knock::Knockable;
use crate::opponents::Opponents;
use crate::rider::Rider;
use crate::{AppState, QuadBody};

/// Samples a second.
pub const RATE: f32 = 30.0;

/// One entity's recorded transforms: a key whenever it moved, and one more
/// just before it starts moving again, so it holds still in between.
#[derive(Default)]
struct Track {
    keys: Vec<(u32, Vec3, Quat)>,
}

impl Track {
    fn push(&mut self, tick: u32, t: &Transform) {
        match self.keys.last() {
            Some(&(_, p, r)) if p.distance_squared(t.translation) < 1e-8 && r.dot(t.rotation).abs() > 1.0 - 1e-7 => {}
            Some(&(last, p, r)) => {
                if last + 1 < tick {
                    self.keys.push((tick - 1, p, r));
                }
                self.keys.push((tick, t.translation, t.rotation));
            }
            None => self.keys.push((tick, t.translation, t.rotation)),
        }
    }

    /// Where it was at `at` samples in (between samples, part way).
    fn sample(&self, at: f32) -> Option<(Vec3, Quat)> {
        let i = self.keys.partition_point(|k| k.0 as f32 <= at);
        match (i.checked_sub(1).and_then(|j| self.keys.get(j)), self.keys.get(i)) {
            (Some(a), Some(b)) => {
                let f = ((at - a.0 as f32) / (b.0 - a.0).max(1) as f32).clamp(0.0, 1.0);
                Some((a.1.lerp(b.1, f), a.2.slerp(b.2, f)))
            }
            (Some(a), None) => Some((a.1, a.2)),
            (None, Some(b)) => Some((b.1, b.2)),
            (None, None) => None,
        }
    }
}

/// The race so far.
#[derive(Resource, Default)]
pub struct Recording {
    tracks: HashMap<Entity, Track>,
    /// Samples taken.
    pub ticks: u32,
    /// Race time not yet sampled.
    carry: f32,
    /// Each racer's quad (player 1 first), for the cameras to follow.
    pub quads: Vec<Entity>,
}

impl Recording {
    /// Seconds recorded.
    pub fn duration(&self) -> f32 {
        self.ticks.saturating_sub(1) as f32 / RATE
    }

    /// Where `e` was `secs` into the recording.
    pub fn transform_at(&self, e: Entity, secs: f32) -> Option<(Vec3, Quat)> {
        self.tracks.get(&e)?.sample(secs * RATE)
    }
}

/// A replay playing: how far in, how fast, and paused or not.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Playback {
    pub time: f32,
    pub speed: f32,
    pub paused: bool,
}

pub struct ReplayPlugin;

impl Plugin for ReplayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Race), |mut commands: Commands| {
            commands.insert_resource(Recording::default());
            commands.remove_resource::<Playback>();
        })
        .add_systems(
            PostUpdate,
            (record.run_if(not(resource_exists::<Playback>)), play.run_if(resource_exists::<Playback>))
                .before(TransformSystems::Propagate)
                .run_if(in_state(AppState::Race)),
        );
    }
}

/// Samples everything that moves, on the race's own clock (nothing while
/// paused).
#[allow(clippy::type_complexity)]
fn record(
    time: Res<Time<Virtual>>,
    rec: Option<ResMut<Recording>>,
    opponents: Option<Res<Opponents>>,
    quads: Query<Entity, With<QuadBody>>,
    moving: Query<Entity, Or<(With<WheelVisual>, With<Knockable>)>>,
    riders: Query<(Entity, &Rider)>,
    transforms: Query<&Transform>,
) {
    let Some(mut rec) = rec else { return };
    if time.is_paused() {
        return;
    }
    rec.carry += time.delta_secs();
    if rec.ticks > 0 && rec.carry < 1.0 / RATE {
        return;
    }
    rec.carry = (rec.carry - 1.0 / RATE).max(0.0);
    if rec.quads.is_empty() {
        rec.quads = quads.iter().chain(opponents.iter().flat_map(|o| o.racers.iter().map(|r| r.body))).collect();
    }
    let tick = rec.ticks;
    let tracked: Vec<Entity> = rec
        .quads
        .clone()
        .into_iter()
        .chain(moving.iter())
        .chain(riders.iter().flat_map(|(e, r)| std::iter::once(e).chain(r.joints().iter().copied())))
        .collect();
    for e in tracked {
        if let Ok(t) = transforms.get(e) {
            rec.tracks.entry(e).or_default().push(tick, t);
        }
    }
    rec.ticks += 1;
}

/// Puts everything recorded where it was at the playback time.
fn play(real: Res<Time<Real>>, mut playback: ResMut<Playback>, rec: Option<Res<Recording>>, mut transforms: Query<&mut Transform>) {
    let Some(rec) = rec else { return };
    if !playback.paused {
        playback.time = (playback.time + real.delta_secs() * playback.speed).clamp(0.0, rec.duration());
    }
    let at = playback.time * RATE;
    for (&e, track) in &rec.tracks {
        let Some((p, r)) = track.sample(at) else { continue };
        if let Ok(mut t) = transforms.get_mut(e) {
            t.translation = p;
            t.rotation = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_still_between_moves() {
        let mut t = Track::default();
        let at = |x: f32| Transform::from_xyz(x, 0.0, 0.0);
        t.push(0, &at(0.0));
        t.push(1, &at(1.0));
        // Still for a while, then off again.
        for tick in 2..10 {
            t.push(tick, &at(1.0));
        }
        t.push(10, &at(3.0));
        assert_eq!(t.keys.len(), 4);
        assert_eq!(t.sample(0.5).unwrap().0.x, 0.5);
        assert_eq!(t.sample(5.0).unwrap().0.x, 1.0);
        assert_eq!(t.sample(9.5).unwrap().0.x, 2.0);
        assert_eq!(t.sample(20.0).unwrap().0.x, 3.0);
    }
}
