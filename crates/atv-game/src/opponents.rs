//! Computer riders in a single race. Each one drives its own quad with
//! the original game's AI driver (`atv_sim::ai`) along the track's racing
//! line, keeps its own laps, and is ranked against the player. Riders are
//! thrown off by the same crash rules as the player's. The original's
//! pacer holds them to the track's AI lap times and races the ones near a
//! player against that player.

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use atv_formats::GameSource;
use atv_formats::aid::AiPath;
use atv_formats::setup;
use atv_formats::row::World as TrackWorld;
use atv_sim::ai::{AiDriver, Pacer, RacingLine, Traffic, bump, pace_rating, tune_for_ai};
use atv_sim::track as trk;
use atv_sim::{Input, TrackCollision, Vehicle};

use crate::convert::{self, Loader, WheelVisual};
use crate::crash::{CrashClips, Wreck};
use crate::aitricks::AiTricks;
use crate::attack::Fighter;
use crate::boost::{Boost, BoostRules};
use crate::freestyle::{self, Dir, Session, StuntClips};
use crate::frontend::Roster;
use crate::race::RaceProgress;
use crate::rider::Rider;
use crate::{AppState, Player, Race};

/// Quads the AI leaves alone: the port's Charger can't get off the line on
/// sand yet (its 1.2 mass spins the wheels), so it would never finish.
pub(crate) const NOT_FOR_AI: &[&str] = &["Charger"];
/// Same reach as the player's checkpoints.
const CHECKPOINT_RADIUS: f32 = 25.0;

pub struct OpponentsPlugin;

impl Plugin for OpponentsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (drive_opponents, crate::attack::kicks)
                .chain()
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
    /// The track's AI pacer, and the pace car each racer follows.
    pacer: Option<Pacer>,
    pace_car: Vec<usize>,
    /// Keeps track of how far round player 1 is, for the pacer.
    player_track: AiDriver,
}

impl Opponents {
    /// Split-screen player `n`'s (2 to 4) quad.
    pub fn human(&self, n: usize) -> Option<&Racer> {
        self.racers.iter().find(|r| r.human == Some(n))
    }

    /// Drives player 1's quad as the computer riders drive theirs (the
    /// attract demo).
    pub fn drive_player(&mut self, v: &Vehicle, dt: f32) -> Input {
        let traffic: Vec<Traffic> = self
            .racers
            .iter()
            .map(|r| Traffic {
                position: r.vehicle.position,
                velocity: r.vehicle.velocity,
                player: r.human.is_some() && r.wreck.fall.is_none(),
                boosting: r.boost.on,
            })
            .collect();
        self.player_track.drive(&self.line, v, &traffic, dt)
    }

    /// Where to put player 1's quad back on the line, when the computer
    /// driving it has given up getting round.
    pub fn respot_player(&mut self) -> Option<(Vec3, Quat)> {
        self.player_track.wants_respot().then(|| self.player_track.respot(&self.line))
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
    /// The rider's tricks (and in Freestyle Battle, a player's score).
    pub session: Option<Session>,
    /// A computer rider's trick picker.
    tricks: AiTricks,
    /// Thrown off, by the same rules as player 1.
    pub wreck: Wreck,
    pub boost: Boost,
    /// Kicks given and taken.
    pub fighter: Fighter,
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
    /// How far the player leads second place or, when not leading, trails
    /// the leader, metres round the racing line (the HUD's "Lead").
    pub lead: f32,
}

/// Spawns `count` computer riders on the grid slots around the player.
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
    let mut line = line;
    if let Some(&finish) = checkpoints.first() {
        line.set_start(finish);
    }
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
    // The Stadium has no checkpoints and no AI lap times: the original
    // never races computer riders there, and they'd only circle it.
    let count = if checkpoints.is_empty() {
        count.min(humans.len())
    } else {
        count
    };
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
        .filter(|q| q.ai)
        .map(|q| q.name.as_str())
        .filter(|q| !NOT_FOR_AI.iter().any(|n| n.eq_ignore_ascii_case(q)))
        .collect();
    // A different quad each, not the player's while others are left.
    let mut quads: Vec<&str> = all_quads
        .iter()
        .copied()
        .filter(|q| !q.eq_ignore_ascii_case(&sel.quad) && !humans.iter().any(|h| h.1.eq_ignore_ascii_case(q)))
        .collect();
    let tuning = crate::tuning::load();
    let scoped = DespawnOnExit(AppState::Race);
    let mut racers = Vec::new();
    let mut ratings = Vec::new();
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
        let mut params = crate::tuning::quad_params(game, &tuning, &quad, human.is_some());
        if human.is_none() {
            tune_for_ai(&mut params, (rider.stats[0] / 5.0).clamp(0.0, 1.0));
        }
        let wheel_x: Vec<f32> = params.wheels.iter().map(|w| w.offset[0]).collect();
        // The slots around the player's (Arcade's can be further back).
        let slot = if i + 1 < sel.player_slot() { i + 1 } else { i + 2 };
        let (pos, rot) = trk::start_pose(world, collision, slot);
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
        if human.is_none() {
            driver = driver.with_engine_boost();
        }
        let q = roster.quads.iter().find(|q| q.name.eq_ignore_ascii_case(&quad)).map_or([0.0; 5], |q| q.stats);
        ratings.push(match human {
            // Players don't take a place behind a pace car.
            Some(_) => f32::MIN,
            None => pace_rating(rider.stats[0], rider.stats[1], q[0], q[1], q[3]),
        });
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
            // Other players and computer riders pull the tricks of their
            // rider's own skill rating.
            session: Some({
                let mut s = session.clone();
                s.level = rider.skill;
                s
            }),
            tricks: AiTricks::new(i as u32 * 7919 + 17),
            wreck: Wreck::default(),
            boost: Boost::default(),
            fighter: Fighter::new(rider.stats[3], rider.stats[2]),
        });
        info!("computer rider {}: {} on {quad}", i + 1, rider.name);
    }
    let track = setup::entries(&String::from_utf8_lossy(&game.read("/Setup/Tracks.xml").unwrap_or_default()))
        .into_iter()
        .find(|e| e.name.eq_ignore_ascii_case(&sel.track));
    let lap_time = |worst| track.as_ref().and_then(|e| e.ai_lap_time(worst)).unwrap_or(0.0);
    let pacer = Pacer::new(
        lap_time(false),
        lap_time(true),
        racers.len() + 1,
        1 + humans.len(),
        line.lap_length(),
    );
    let mut pacer = pacer;
    let pace_car = pacer.as_mut().map(|p| p.assign(&ratings)).unwrap_or_default();
    let player_track = AiDriver::new(&line, trk::start_pose(world, collision, sel.player_slot()).0, 0);
    commands.insert_resource(Opponents { line, racers, pacer, pace_car, player_track });
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

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn drive_opponents(
    time: Res<Time>,
    race: Option<Res<Race>>,
    player: Option<ResMut<Player>>,
    progress: Option<Res<RaceProgress>>,
    opponents: Option<ResMut<Opponents>>,
    standings: Option<ResMut<Standings>>,
    (keys, pads, settings): (Res<ButtonInput<KeyCode>>, crate::controller::Pads, Res<crate::settings::Settings>),
    args: Res<crate::Args>,
    (session, crash_clips, boost_rules): (Option<Res<Session>>, Option<Res<CrashClips>>, Option<Res<BoostRules>>),
    (player_boost, player_wreck): (Option<Res<Boost>>, Option<Res<Wreck>>),
) {
    let (Some(race), Some(mut player), Some(progress), Some(mut opp), Some(mut standings)) =
        (race, player, progress, opponents, standings)
    else {
        return;
    };
    let dt = time.delta_secs();
    let controls = crate::controller::Reader::new(&keys, &pads, &settings.pads);
    let opp = &mut *opp;
    let cps = &race.checkpoints;
    let n = cps.len();
    // The pace cars set off with the race.
    opp.player_track.follow(&opp.line, player.0.position);
    if progress.started()
        && let Some(pacer) = opp.pacer.as_mut()
    {
        let players: Vec<f32> = std::iter::once(opp.player_track.distance())
            .chain(opp.racers.iter().filter(|r| r.human.is_some()).map(|r| r.driver.distance()))
            .collect();
        let line = &opp.line;
        let distances: Vec<f32> = opp
            .racers
            .iter_mut()
            .map(|r| {
                r.driver.follow(line, r.vehicle.position);
                r.driver.distance()
            })
            .collect();
        pacer.start(&distances);
        pacer.tick(dt, &players);
    }
    let traffic: Vec<Traffic> = std::iter::once(Traffic {
        position: player.0.position,
        velocity: player.0.velocity,
        player: player_wreck.as_ref().is_none_or(|w| w.fall.is_none()),
        boosting: player_boost.as_ref().is_some_and(|b| b.on),
    })
    .chain(opp.racers.iter().map(|r| Traffic {
        position: r.vehicle.position,
        velocity: r.vehicle.velocity,
        player: r.human.is_some() && r.wreck.fall.is_none(),
        boosting: r.boost.on,
    }))
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
        let crashed = r.wreck.fall.is_some();
        let mut held = false;
        r.input = if !r.progress.started() && !crashed {
            // Held on the grid, where players can rev for a turbo start.
            match r.human {
                Some(n) if !args.autodrive => {
                    Input { throttle: crate::read_input(&controls, false, Some(n - 1)).throttle, ..default() }
                }
                _ => Input::default(),
            }
        } else if crashed {
            // Thrown off: the quad rolls on by itself.
            Input::default()
        } else if time_up {
            Input { brake: if r.vehicle.speed_mph() > 2.0 { 0.3 } else { 0.0 }, ..default() }
        } else {
            if let (Some(pacer), Some(&car), None) = (&opp.pacer, opp.pace_car.get(i), r.human) {
                r.driver.pace = pacer.scale(car, r.driver.distance());
            }
            let ai = r.driver.drive(&opp.line, &r.vehicle, &others, dt);
            match r.human {
                // A player drives until the flag; the AI brings them home
                // (and drives for them with --autodrive).
                Some(n) if r.progress.finished.is_none() && !args.autodrive => {
                    held = crate::read_boost(&controls, !r.vehicle.grounded(), Some(n - 1));
                    crate::read_input(&controls, !r.vehicle.grounded(), Some(n - 1))
                }
                // The autopilot spends a full meter.
                Some(_) => {
                    held = r.progress.finished.is_none() && (r.boost.on || r.boost.meter >= 1.0);
                    ai
                }
                // Computer riders boost flat out on a straight (0x67520);
                // retail's only while a quarter of the meter is left
                // (0x7b990).
                None => {
                    held = r.progress.finished.is_none()
                        && r.driver.boost
                        && (!r.vehicle.params.retail || r.boost.meter > 0.25);
                    ai
                }
            }
        };
        if held {
            r.input.throttle = 1.0;
        }
        let banked = r.session.as_mut().map_or(0, |s| s.take_banked());
        r.vehicle.drive_scale = boost_rules.as_deref().map_or(1.0, |rules| r.boost.step(rules, held, banked, dt))
            * r.driver.torque_scale();
        let before = (r.vehicle.velocity, r.vehicle.angular_velocity);
        r.vehicle.set_grid(!r.progress.started());
        r.vehicle.step(&r.input, dt, &race.collision);
        let mut back_on = false;
        if crashed {
            back_on = r.wreck.advance(crash_clips.as_deref(), &race.collision, dt);
            if back_on {
                r.wreck.fall = None;
            }
        } else if r.progress.started()
            && !time_up
            && let Some((cause, way)) = r.wreck.check(&r.vehicle, before, &race.collision, dt)
        {
            let p = r.vehicle.position;
            let pick = ((p.x * 12.9898 + p.z * 78.233 + i as f32).sin() * 43758.547).fract().abs();
            r.wreck.start(&r.vehicle, cause, way, pick);
            if let Some(rules) = boost_rules.as_deref() {
                r.boost.crashed(rules);
            }
            debug!("{} crashed ({cause:?})", r.name);
        }
        let live = r.progress.started() && !time_up && r.progress.finished.is_none();
        match (r.session.as_mut(), r.human) {
            (Some(s), Some(n)) => {
                let (mut buttons, mut dir) = freestyle::read_tricks(&controls, Some(n - 1));
                if args.autodrive && s.active.is_none() && !r.vehicle.grounded() {
                    // The autopilot shows off, as for player 1.
                    let k = (s.time * 0.7) as usize + n;
                    buttons = 1 << (k % 4);
                    dir = Some([Dir::Up, Dir::Right, Dir::Down, Dir::Left][k / 4 % 4]);
                }
                s.step(&r.vehicle, &race.collision, (buttons, dir), dt, live, crashed);
            }
            (Some(s), None) => {
                if live && r.wreck.fall.is_none() {
                    r.tricks.step(s, &r.vehicle, &race.collision);
                }
                s.step(&r.vehicle, &race.collision, (0, None), dt, live, crashed);
            }
            _ => {}
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
        // A thrown rider gets back on there too.
        r.upside_down = if r.vehicle.up().y < 0.2 {
            r.upside_down + dt
        } else {
            0.0
        };
        let fell = r.vehicle.position.y < race.safe.0.y.min(race.start.0.y) - 60.0;
        let gave_up = r.driver.wants_respot() && (r.human.is_none() || r.progress.finished.is_some());
        let crashed = r.wreck.fall.is_some();
        if back_on || fell || (!crashed && (gave_up || r.upside_down > 2.5)) {
            r.wreck.fall = None;
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
    // Against second place when leading, the leader otherwise (0xa1510),
    // from the distances the pacer keeps once the race is on.
    let rival = if standings.player == 1 { 2 } else { 1 };
    standings.lead = standings
        .opponents
        .iter()
        .position(|o| o.0 == rival)
        .filter(|_| progress.started() && opp.pacer.is_some())
        .map_or(0.0, |i| opp.player_track.distance() - opp.racers[i].driver.distance());
}

#[allow(clippy::type_complexity)]
fn sync_opponents(
    time: Res<Time>,
    opponents: Option<Res<Opponents>>,
    (clips, crash_clips, fight_clips): (Option<Res<StuntClips>>, Option<Res<CrashClips>>, Option<Res<crate::attack::FightClips>>),
    mut bodies: Query<&mut Transform, (Without<WheelVisual>, Without<OpponentRider>)>,
    mut wheels: Query<(&WheelVisual, &mut Transform), Without<OpponentRider>>,
    mut riders: Query<(&ChildOf, &mut Rider, &mut Transform), With<OpponentRider>>,
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
    for (parent, mut rider, mut seat) in &mut riders {
        if let Some(r) = opp.racers.iter().find(|r| r.body == parent.parent()) {
            // Thrown off: the rider tumbles where the crash puts them, as
            // player 1's does.
            match (&r.wreck.fall, crash_clips.as_deref().and_then(|c| r.wreck.pose(c))) {
                (Some(f), Some(pose)) => {
                    rider.set_crash(Some(pose));
                    let quad = Transform::from_translation(r.vehicle.position).with_rotation(r.vehicle.rotation);
                    let world = Transform::from_translation(f.body).with_rotation(f.heading);
                    let local = quad.compute_affine().inverse() * world.compute_affine();
                    *seat = Transform::from_matrix(Mat4::from(local));
                    continue;
                }
                _ if rider.crashed() => {
                    rider.set_crash(None);
                    *seat = Transform::from_xyz(0.0, -convert::COM_HEIGHT, 0.0);
                }
                _ => {}
            }
            rider.update_controls(
                r.vehicle.steer,
                r.input.lean,
                r.vehicle.preload,
                time.delta_secs(),
            );
            // A kick or a flinch, else the trick in progress, a player's or
            // a computer rider's.
            let stunt = fight_clips.as_deref().and_then(|c| r.fighter.pose(c)).or_else(|| {
                r.session.as_ref().zip(clips.as_deref()).and_then(|(s, clips)| {
                    let a = s.active.as_ref()?;
                    Some((clips.0.get(a.def)?.clone()?, a.t / s.tricks[a.def].total))
                })
            });
            rider.set_stunt(stunt, time.delta_secs());
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
