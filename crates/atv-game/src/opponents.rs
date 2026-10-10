//! Computer riders in a single race. Each one drives its own quad with
//! the original game's AI driver (`atv_sim::ai`) along the track's racing
//! line, keeps its own laps, and is ranked against the player.

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::input::gamepad::Gamepad;
use bevy::prelude::*;

use atv_formats::GameSource;
use atv_formats::aid::AiPath;
use atv_formats::row::World as TrackWorld;
use atv_sim::ai::{AiDriver, RacingLine, Traffic, bump, tune_for_ai};
use atv_sim::track as trk;
use atv_sim::{Input, TrackCollision, Vehicle, VehicleParams};

use crate::convert::{self, Loader, WheelVisual};
use crate::freestyle::{self, Dir, Session, StuntClips};
use crate::frontend::Roster;
use crate::race::RaceProgress;
use crate::rider::Rider;
use crate::{AppState, Player, Race};

/// Quads the AI leaves alone: the port's Charger can't get off the line on
/// sand yet (its 1.2 mass spins the wheels), so it would never finish.
const NOT_FOR_AI: &[&str] = &["Charger"];
/// Same reach as the player's checkpoints.
const CHECKPOINT_RADIUS: f32 = 25.0;

pub struct OpponentsPlugin;

impl Plugin for OpponentsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            drive_opponents
                .after(crate::drive)
                .run_if(in_state(AppState::Race)),
        )
        .add_systems(
            Update,
            sync_opponents
                .before(crate::rider::pose_riders)
                .run_if(in_state(AppState::Race)),
        )
        .add_systems(OnExit(AppState::Race), |mut commands: Commands| {
            commands.remove_resource::<Opponents>();
            commands.remove_resource::<Standings>();
        });
    }
}

#[derive(Resource)]
pub struct Opponents {
    line: RacingLine,
    pub racers: Vec<Racer>,
}

impl Opponents {
    /// Split-screen player `n`'s (2 to 4) quad.
    pub fn human(&self, n: usize) -> Option<&Racer> {
        self.racers.iter().find(|r| r.human == Some(n))
    }
}

pub struct Racer {
    /// Rider's name as the menus show it.
    pub name: String,
    /// Rider's model folder.
    pub model: String,
    pub vehicle: Vehicle,
    driver: AiDriver,
    pub progress: RaceProgress,
    next_checkpoint: usize,
    input: Input,
    /// The quad's root entity.
    pub body: Entity,
    upside_down: f32,
    /// A split-screen player (2 to 4) drives this quad; the AI driver only
    /// keeps track and brings it home after the flag.
    pub human: Option<usize>,
    /// A player's tricks (and in Freestyle Battle, their score).
    pub session: Option<Session>,
}

/// Marks a computer rider's seat, so the player's rider systems leave its
/// rider alone.
#[derive(Component)]
pub struct OpponentRider;

/// The running order, refreshed every physics step.
#[derive(Resource, Default)]
pub struct Standings {
    /// The player's place, 1-based.
    pub player: usize,
    pub racers: usize,
    /// Each computer rider's place and lap progress (0..1), in racer order.
    pub opponents: Vec<(usize, f32)>,
}

/// Spawns `count` computer riders on the grid slots behind the player.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    commands: &mut Commands,
    loader: &mut Loader,
    bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    game: &GameSource,
    roster: &Roster,
    sel: &crate::Selection,
    (world, collision): (&TrackWorld, &TrackCollision),
    session: &Session,
    count: usize,
) {
    if count == 0 {
        return;
    }
    let path = format!("/Levels/{t}/World/{t}.aid", t = sel.track);
    let line = match game
        .read(&path)
        .map_err(anyhow::Error::from)
        .and_then(|b| Ok(AiPath::parse(&b)?))
    {
        Ok(p) => RacingLine::from_path(&p),
        Err(e) => {
            warn!("no computer riders on {}: {e:#}", sel.track);
            return;
        }
    };
    if line.is_empty() {
        return;
    }
    let checkpoints = trk::checkpoints(world);
    // A different field each track, the same every time on a track.
    let mut seed = sel.track.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ b as u32).wrapping_mul(16_777_619)
    }) | 1;
    let mut pick = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize % n.max(1)
    };
    // Split screen: the other players first, behind player 1.
    let humans: Vec<_> = sel
        .others
        .iter()
        .take(sel.players.saturating_sub(1))
        .filter_map(|(rider, quad)| Some((roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(rider))?, quad.clone())))
        .collect();
    let taken = |m: &str| m.eq_ignore_ascii_case(&sel.rider) || humans.iter().any(|h| h.0.model.eq_ignore_ascii_case(m));
    let mut riders: Vec<_> = roster
        .riders
        .iter()
        .filter(|r| r.ai && !taken(&r.model))
        .collect();
    // A set field (career) races in its own order.
    let set_field: Vec<_> = sel
        .field
        .iter()
        .filter_map(|m| roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(m)))
        .collect();
    let all_quads: Vec<&str> = roster
        .quads
        .iter()
        .map(|q| q.name.as_str())
        .filter(|q| !NOT_FOR_AI.iter().any(|n| n.eq_ignore_ascii_case(q)))
        .collect();
    // A different quad each, not the player's while others are left.
    let mut quads: Vec<&str> = all_quads
        .iter()
        .copied()
        .filter(|q| !q.eq_ignore_ascii_case(&sel.quad) && !humans.iter().any(|h| h.1.eq_ignore_ascii_case(q)))
        .collect();
    let generic = game
        .read("/Cars/Generic/Generic.params")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let scoped = DespawnOnExit(AppState::Race);
    let mut racers = Vec::new();
    for i in 0..count.min(7) {
        if quads.is_empty() {
            quads = all_quads.clone();
        }
        let human = humans.get(i);
        if human.is_none() && ((riders.is_empty() && set_field.len() <= i - humans.len()) || quads.is_empty()) {
            break;
        }
        // A set field lists the computer riders only, after the players.
        let rider = match (human, set_field.get(i.saturating_sub(humans.len()))) {
            (Some(h), _) => h.0,
            (None, Some(r)) => *r,
            (None, None) if !sel.field.is_empty() => break,
            (None, None) => riders.swap_remove(pick(riders.len())),
        };
        let quad = match human {
            Some(h) => h.1.clone(),
            None => quads.swap_remove(pick(quads.len())).to_string(),
        };
        let own = game
            .read(&format!("/Cars/{quad}/{quad}.params"))
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        let mut params = VehicleParams::from_files(&generic, own.as_deref());
        let ability = (rider.stats[0] / 5.0).clamp(0.0, 1.0);
        if human.is_none() {
            tune_for_ai(&mut params, ability);
        }
        let wheel_x: Vec<f32> = params.wheels.iter().map(|w| w.offset[0]).collect();
        let (pos, rot) = trk::start_pose(world, collision, i + 2);
        let at = Transform::from_translation(pos).with_rotation(rot);
        let body = match loader.spawn_quad(commands, &quad, at, &wheel_x) {
            Ok(e) => e,
            Err(e) => {
                error!("failed to load quad {quad}: {e:#}");
                continue;
            }
        };
        commands.entity(body).insert(scoped.clone());
        let seat = commands
            .spawn((
                Transform::from_xyz(0.0, -convert::COM_HEIGHT, 0.0),
                Visibility::default(),
                OpponentRider,
            ))
            .id();
        commands.entity(body).add_child(seat);
        match loader
            .spawn_rider(commands, bindposes, &rider.model, seat)
            .and_then(|rig| Rider::new(game, rig))
        {
            Ok(r) => {
                commands.entity(seat).insert(r);
            }
            Err(e) => error!("failed to load rider {}: {e:#}", rider.model),
        }
        let mut driver = AiDriver::new(&line, pos, i as u32 + 1);
        driver.ability = ability;
        let on_line = checkpoints
            .first()
            .is_some_and(|&c| (c - pos).with_y(0.0).length() < CHECKPOINT_RADIUS);
        racers.push(Racer {
            name: rider.name.clone(),
            model: rider.model.clone(),
            vehicle: Vehicle::new(params, pos, rot),
            driver,
            progress: RaceProgress::new(sel.laps, on_line),
            next_checkpoint: usize::from(on_line && checkpoints.len() > 1),
            input: Input::default(),
            body,
            upside_down: 0.0,
            human: human.map(|_| i + 2),
            session: human.map(|_| session.clone()),
        });
        info!("computer rider {}: {} on {quad}", i + 1, rider.name);
    }
    commands.insert_resource(Opponents { line, racers });
    commands.init_resource::<Standings>();
}

/// How far through the race a rider is, for the running order: laps plus
/// the share of the current lap; finishers by their time. `to_line`: metres
/// to the start/finish line, which orders a grid still running up to it.
fn race_score(p: &RaceProgress, to_line: f32) -> f32 {
    match p.finished {
        Some(t) => 1e6 - t,
        None if !p.armed() => -to_line * 1e-4,
        None => p.lap_times.len() as f32 + p.lap_fraction,
    }
}

/// Lap progress (0..1) from the checkpoint being driven to and the
/// distance to it, as the player's meter shows it.
fn lap_fraction(checkpoints: &[Vec3], next: usize, at: Vec3, armed: bool) -> f32 {
    let n = checkpoints.len();
    if n == 0 || (next == 0 && !armed) {
        return 0.0;
    }
    let cp = checkpoints[next];
    let prev = checkpoints[(next + n - 1) % n];
    let leg = (cp - prev).with_y(0.0).length().max(1.0);
    let along = (1.0 - (cp - at).with_y(0.0).length() / leg).clamp(0.0, 1.0);
    (((next + n - 1) % n) as f32 + along) / n as f32
}

#[allow(clippy::too_many_arguments)]
fn drive_opponents(
    time: Res<Time>,
    race: Option<Res<Race>>,
    player: Option<ResMut<Player>>,
    progress: Option<Res<RaceProgress>>,
    opponents: Option<ResMut<Opponents>>,
    standings: Option<ResMut<Standings>>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    args: Res<crate::Args>,
    session: Option<Res<Session>>,
) {
    let (Some(race), Some(mut player), Some(progress), Some(mut opp), Some(mut standings)) =
        (race, player, progress, opponents, standings)
    else {
        return;
    };
    let dt = time.delta_secs();
    let opp = &mut *opp;
    let cps = &race.checkpoints;
    let n = cps.len();
    let traffic: Vec<Traffic> = std::iter::once(&player.0)
        .chain(opp.racers.iter().map(|r| &r.vehicle))
        .map(|v| Traffic {
            position: v.position,
            velocity: v.velocity,
        })
        .collect();
    for (i, r) in opp.racers.iter_mut().enumerate() {
        r.progress.tick(dt);
        let others: Vec<Traffic> = traffic
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i + 1)
            .map(|(_, t)| *t)
            .collect();
        // Freestyle Battle runs on player 1's clock, with no laps.
        let battle = r.session.as_ref().is_some_and(|s| s.freestyle);
        let time_up = battle && session.as_ref().is_some_and(|s| s.over(progress.clock));
        r.input = if !r.progress.started() {
            Input::default()
        } else if time_up {
            Input { brake: if r.vehicle.speed_mph() > 2.0 { 0.3 } else { 0.0 }, ..default() }
        } else {
            let ai = r.driver.drive(&opp.line, &r.vehicle, &others, dt);
            match r.human {
                // A player drives until the flag; the AI brings them home
                // (and drives for them with --autodrive).
                Some(n) if r.progress.finished.is_none() && !args.autodrive => crate::read_input(&keys, &pads, !r.vehicle.grounded(), Some(n - 1)),
                _ => ai,
            }
        };
        r.vehicle.step(&r.input, dt, &race.collision);
        if let (Some(s), Some(n)) = (r.session.as_mut(), r.human) {
            let (mut buttons, mut dir) = freestyle::read_tricks(&keys, &pads, Some(n - 1));
            if args.autodrive && s.active.is_none() && !r.vehicle.grounded() {
                // The autopilot shows off, as for player 1.
                let k = (s.time * 0.7) as usize + n;
                buttons = 1 << (k % 4);
                dir = Some([Dir::Up, Dir::Right, Dir::Down, Dir::Left][k / 4 % 4]);
            }
            s.step(&r.vehicle, buttons, dir, dt, r.progress.started() && !time_up && r.progress.finished.is_none());
        }

        // Checkpoints. The racing line's alternative routes can pass wide
        // of one, so reaching the one after it counts for both.
        if n > 0 && !battle {
            let near = |k: usize| {
                (cps[k % n] - r.vehicle.position).with_y(0.0).length() < CHECKPOINT_RADIUS
            };
            let next = r.next_checkpoint % n;
            if near(next) {
                r.progress.reach(next);
                r.next_checkpoint = (next + 1) % n;
            } else if n > 2 && near(next + 1) {
                r.progress.reach(next);
                r.progress.reach((next + 1) % n);
                r.next_checkpoint = (next + 2) % n;
            }
            r.progress.lap_fraction = lap_fraction(
                cps,
                r.next_checkpoint % n,
                r.vehicle.position,
                r.progress.armed(),
            );
        }

        // Back on the line when the driver gives up, the quad has been on
        // its back a while, or it fell out of the world.
        r.upside_down = if r.vehicle.up().y < 0.2 {
            r.upside_down + dt
        } else {
            0.0
        };
        let fell = r.vehicle.position.y < race.safe.0.y.min(race.start.0.y) - 60.0;
        let gave_up = r.driver.wants_respot() && (r.human.is_none() || r.progress.finished.is_some());
        if gave_up || r.upside_down > 2.5 || fell {
            let (p, rot) = r.driver.respot(&opp.line);
            let ground = race
                .collision
                .raycast(p + Vec3::Y * 3.0, Vec3::NEG_Y, 20.0)
                .map_or(p.y, |h| h.point.y);
            r.vehicle.reset(p.with_y(ground + 0.6), rot);
            r.upside_down = 0.0;
        }
    }

    // Quads knock each other about, the player's included.
    for i in 0..opp.racers.len() {
        bump(&mut player.0, &mut opp.racers[i].vehicle);
        for j in i + 1..opp.racers.len() {
            let (a, b) = opp.racers.split_at_mut(j);
            bump(&mut a[i].vehicle, &mut b[0].vehicle);
        }
    }

    // Running order.
    let to_line = |v: &Vehicle| {
        cps.first()
            .map_or(0.0, |&c| (c - v.position).with_y(0.0).length())
    };
    let mine = race_score(&progress, to_line(&player.0));
    let scores: Vec<f32> = opp
        .racers
        .iter()
        .map(|r| race_score(&r.progress, to_line(&r.vehicle)))
        .collect();
    // Ahead of a rider: everyone with a better score (the player wins ties).
    let place = |i: usize| {
        1 + usize::from(mine >= scores[i])
            + scores
                .iter()
                .enumerate()
                .filter(|&(j, &o)| j != i && o > scores[i])
                .count()
    };
    standings.racers = opp.racers.len() + 1;
    standings.player = 1 + scores.iter().filter(|&&o| o > mine).count();
    standings.opponents = opp
        .racers
        .iter()
        .enumerate()
        .map(|(i, r)| (place(i), r.progress.lap_fraction))
        .collect();
}

#[allow(clippy::type_complexity)]
fn sync_opponents(
    time: Res<Time>,
    opponents: Option<Res<Opponents>>,
    clips: Option<Res<StuntClips>>,
    mut bodies: Query<&mut Transform, Without<WheelVisual>>,
    mut wheels: Query<(&WheelVisual, &mut Transform), Without<OpponentRider>>,
    mut riders: Query<(&ChildOf, &mut Rider), With<OpponentRider>>,
) {
    let Some(opp) = opponents else { return };
    for r in &opp.racers {
        if let Ok(mut t) = bodies.get_mut(r.body) {
            t.translation = r.vehicle.position;
            t.rotation = r.vehicle.rotation;
        }
    }
    for (w, mut t) in &mut wheels {
        if let Some(r) = opp.racers.iter().find(|r| r.body == w.owner) {
            crate::place_wheel(w, &mut t, &r.vehicle);
        }
    }
    for (parent, mut rider) in &mut riders {
        if let Some(r) = opp.racers.iter().find(|r| r.body == parent.parent()) {
            rider.update_controls(
                r.vehicle.steer,
                r.input.lean,
                r.vehicle.preload,
                time.delta_secs(),
            );
            // A player's trick in progress.
            let stunt = r.session.as_ref().zip(clips.as_deref()).and_then(|(s, clips)| {
                let a = s.active.as_ref()?;
                Some((clips.0.get(a.def)?.clone()?, a.t / s.tricks[a.def].total))
            });
            if r.human.is_some() {
                rider.set_stunt(stunt, time.delta_secs());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_follows_checkpoints() {
        let cps = [
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -100.0),
            Vec3::new(100.0, 0.0, -100.0),
            Vec3::new(100.0, 0.0, 0.0),
        ];
        assert_eq!(
            lap_fraction(&cps, 0, Vec3::new(50.0, 0.0, 0.0), false),
            0.0,
            "not yet over the line"
        );
        let f = lap_fraction(&cps, 1, Vec3::new(0.0, 0.0, -50.0), true);
        assert!((f - 0.125).abs() < 1e-4, "{f}");
        let f = lap_fraction(&cps, 0, Vec3::new(50.0, 0.0, 0.0), true);
        assert!((f - 0.875).abs() < 1e-4, "{f}");
    }

    #[test]
    fn finishers_rank_by_time() {
        let mut a = RaceProgress::new(1, true);
        let mut b = RaceProgress::new(1, true);
        a.finished = Some(100.0);
        b.finished = Some(90.0);
        assert!(race_score(&b, 0.0) > race_score(&a, 0.0));
        let mut c = RaceProgress::new(1, true);
        c.lap_fraction = 0.99;
        assert!(race_score(&a, 0.0) > race_score(&c, 0.0));
        let (near, far) = (RaceProgress::new(1, false), RaceProgress::new(1, false));
        assert!(
            race_score(&near, 10.0) > race_score(&far, 50.0),
            "closer to the line leads the run-up"
        );
        assert!(race_score(&c, 0.0) > race_score(&near, 10.0));
    }
}
