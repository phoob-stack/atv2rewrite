//! Replays: every race is recorded as it runs, and the results can play it
//! back.
//!
//! The recording keeps what is drawn rather than what was simulated: each
//! quad's body and wheels, each rider's seat and joints, and the props
//! knocked about, sampled [`RATE`] times a second. Playback freezes the race
//! (as the pause menu does) and puts every one of those back where it was,
//! so tricks, crashes and kicks come back exactly as they looked.
//!
//! Once a lone player is home, the same director takes the race camera
//! behind the results, as the original's does (0x82cd0), moving from rider
//! to rider ([`Broadcast`]).
//!
//! The original recorded the player's pad instead and ran the race again
//! (0x87190, 0xf6a10). That needs a simulation that repeats itself to the
//! bit, which this one doesn't promise, and it couldn't rewind; keeping the
//! transforms can.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use bevy::camera::visibility::VisibilitySystems;

use crate::convert::{self, SkyDome, WheelVisual};
use crate::crash::Wreck;
use crate::freestyle::Session;
use crate::frontend::Roster;
use crate::race::{fmt_time, RaceProgress};
use crate::knock::Knockable;
use crate::opponents::Opponents;
use crate::rider::Rider;
use crate::ui::{fullscreen, read_nav, set_text, BitmapText, FontId, Nav, TextLayout, UiAssets};
use crate::settings::Settings;
use crate::{AppState, ChaseCam, Game, Player, QuadBody, Race, Selection};

pub mod director;

/// Samples a second.
pub const RATE: f32 = 30.0;
/// The longest recording, in seconds: as the original, whose buffer filled
/// after about six minutes, a longer race keeps its start.
const LONGEST: f32 = 15.0 * 60.0;

/// One entity's recorded transforms: a key whenever it moved, and one more
/// at the last sample before it moves again, so it holds still in between.
#[derive(Default)]
struct Track {
    keys: Vec<(u32, Vec3, Quat)>,
}

impl Track {
    /// Where it is at sample `tick`; the sample before was `prev` (further
    /// back than `tick - 1` when a slow frame covered several).
    fn push(&mut self, tick: u32, prev: u32, t: &Transform) {
        match self.keys.last() {
            Some(&(_, p, r)) if p.distance_squared(t.translation) < 1e-8 && r.dot(t.rotation).abs() > 1.0 - 1e-7 => {}
            Some(&(last, p, r)) => {
                if last < prev {
                    self.keys.push((prev, p, r));
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
    /// And each rider's name.
    pub names: Vec<String>,
    /// Each racer's [`director::GROUNDED`], [`director::ALL_DOWN`] and
    /// [`director::CRASHED`] whenever they changed.
    flags: Vec<Vec<(u32, u8)>>,
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

    /// Racer `i`'s flags `secs` into the recording.
    pub fn flags_at(&self, i: usize, secs: f32) -> u8 {
        let Some(changes) = self.flags.get(i) else { return 0 };
        let tick = (secs * RATE).max(0.0) as u32;
        let at = changes.partition_point(|c| c.0 <= tick);
        at.checked_sub(1).map_or(0, |j| changes[j].1)
    }

    /// Keys kept, for the log.
    fn keys(&self) -> usize {
        self.tracks.values().map(|t| t.keys.len()).sum()
    }
}

/// A replay playing: how far in, how fast, paused or not, and who the
/// cameras are watching.
#[derive(Resource)]
pub struct Playback {
    pub time: f32,
    /// Below zero, rewinding.
    pub speed: f32,
    pub paused: bool,
    /// The racer the cameras watch (index into [`Recording::quads`]).
    pub target: usize,
    pub(crate) director: director::Director,
    /// The race's own screens, hidden while it plays, and how they were.
    hidden: Option<Vec<(Entity, Visibility)>>,
    /// Back to the results at the end of this frame.
    leaving: bool,
}

impl Playback {
    pub fn new() -> Self {
        Self { time: 0.0, speed: 1.0, paused: false, target: 0, director: director::Director::default(), hidden: None, leaving: false }
    }
}

/// The replay's own captions.
#[derive(Component)]
struct ReplayUi;
#[derive(Component)]
enum Caption {
    Rider,
    Clock,
    Camera,
}

pub struct ReplayPlugin;

impl Plugin for ReplayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Race), |mut commands: Commands| {
            commands.insert_resource(Recording::default());
            commands.remove_resource::<Playback>();
            commands.remove_resource::<Broadcast>();
        })
        .add_systems(Update, controls.run_if(resource_exists::<Playback>).run_if(in_state(AppState::Race)))
        .add_systems(
            PostUpdate,
            (
                // The race intro flies the camera before the race starts.
                (record, broadcast).run_if(not(resource_exists::<Playback>)).run_if(not(resource_exists::<crate::intro::Intro>)),
                (play, captions).chain().run_if(resource_exists::<Playback>),
            )
                .before(TransformSystems::Propagate)
                .before(VisibilitySystems::VisibilityPropagate)
                .before(TextLayout)
                .run_if(in_state(AppState::Race)),
        );
    }
}

/// Samples everything that moves, on the race's own clock (nothing while
/// paused).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn record(
    time: Res<Time<Virtual>>,
    rec: Option<ResMut<Recording>>,
    opponents: Option<Res<Opponents>>,
    (player, wreck): (Option<Res<Player>>, Option<Res<Wreck>>),
    sel: Res<Selection>,
    roster: Res<Roster>,
    quads: Query<Entity, With<QuadBody>>,
    moving: Query<Entity, Or<(With<WheelVisual>, With<Knockable>, With<crate::ghost::Ghostly>)>>,
    riders: Query<(Entity, &Rider)>,
    transforms: Query<&Transform>,
) {
    let Some(mut rec) = rec else { return };
    if time.is_paused() {
        return;
    }
    // A frame slower than a sample covers several, so the replay keeps
    // the race's pace whatever the frame rate.
    rec.carry += time.delta_secs();
    let due = if rec.ticks == 0 { 1 } else { (rec.carry * RATE) as u32 };
    if due == 0 || rec.ticks as f32 >= LONGEST * RATE {
        return;
    }
    rec.carry = if rec.ticks == 0 { 0.0 } else { rec.carry - due as f32 / RATE };
    if rec.quads.is_empty() {
        rec.quads = quads.iter().chain(opponents.iter().flat_map(|o| o.racers.iter().map(|r| r.body))).collect();
        let me = roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&sel.rider)).map_or("PLAYER 1".to_string(), |r| r.name.clone());
        rec.names = std::iter::once(me).chain(opponents.iter().flat_map(|o| o.racers.iter().map(|r| r.name.clone()))).collect();
    }
    let prev = rec.ticks.saturating_sub(1);
    let tick = rec.ticks + due - 1;
    let now = live_flags(player.as_deref(), wreck.as_deref(), opponents.as_deref());
    let racers = now.len().max(rec.flags.len());
    rec.flags.resize_with(racers, Vec::new);
    for (changes, f) in rec.flags.iter_mut().zip(now) {
        if changes.last().is_none_or(|c| c.1 != f) {
            changes.push((tick, f));
        }
    }
    let tracked: Vec<Entity> = rec
        .quads
        .clone()
        .into_iter()
        .chain(moving.iter())
        .chain(riders.iter().flat_map(|(e, r)| std::iter::once(e).chain(r.joints().iter().copied())))
        .collect();
    for e in tracked {
        if let Ok(t) = transforms.get(e) {
            rec.tracks.entry(e).or_default().push(tick, prev, t);
        }
    }
    rec.ticks = tick + 1;
}

/// A racer's [`director::GROUNDED`], [`director::ALL_DOWN`] and
/// [`director::CRASHED`].
fn racer_flags(v: &atv_sim::Vehicle, crashed: bool) -> u8 {
    let mut f = 0;
    if v.grounded() {
        f |= director::GROUNDED;
    }
    if v.all_wheels_down() {
        f |= director::ALL_DOWN;
    }
    if crashed {
        f |= director::CRASHED;
    }
    f
}

/// Every racer's flags now, player 1 first.
fn live_flags(player: Option<&Player>, wreck: Option<&Wreck>, opponents: Option<&Opponents>) -> Vec<u8> {
    player
        .iter()
        .map(|p| racer_flags(&p.0, wreck.is_some_and(|w| w.fall.is_some())))
        .chain(opponents.iter().flat_map(|o| o.racers.iter().map(|r| racer_flags(&r.vehicle, r.wreck.fall.is_some()))))
        .collect()
}

/// The director filming the race once a lone player is home, and who it
/// is watching (player 1 first, then the computer riders).
#[derive(Resource)]
pub struct Broadcast {
    director: director::Director,
    target: usize,
}

/// Once a one-player race or session is over, the director takes the
/// camera (0x82cd0, in the style and mode `CameraDirector.xml` gives: all
/// its cameras, moving between riders), and in the attract demo it films
/// the whole race.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn broadcast(
    mut commands: Commands,
    (time, real): (Res<Time<Virtual>>, Res<Time<Real>>),
    (game, sel, settings): (Res<Game>, Res<Selection>, Res<Settings>),
    (progress, session, race): (Option<Res<RaceProgress>>, Option<Res<Session>>, Option<Res<Race>>),
    (player, wreck, opponents): (Option<Res<Player>>, Option<Res<Wreck>>, Option<Res<Opponents>>),
    live: Option<ResMut<Broadcast>>,
    mut cams: Query<(Entity, &mut Projection, Option<&crate::mp::Split>), With<ChaseCam>>,
    mut transforms: Query<&mut Transform>,
    sky: Query<Entity, With<SkyDome>>,
    mut riders: Query<&mut Visibility, With<Rider>>,
    demo: Option<Res<crate::demo::Demo>>,
) {
    let (Some(p), Some(player)) = (progress, player.as_deref()) else { return };
    // The attract demo is filmed from the start, every rider a computer's.
    let players = usize::from(demo.is_none());
    let mut live = match live {
        Some(live) => live,
        None => {
            let over = match session.as_deref() {
                Some(s) if s.freestyle => s.over(p.clock),
                _ => p.over(),
            };
            if (over && sel.players <= 1) || demo.is_some() {
                let mut director = director::Director::default();
                director.load(&game.0, &sel.track, settings.camera);
                commands.insert_resource(Broadcast { director, target: 0 });
            }
            return;
        }
    };
    let racers: Vec<(Vec3, Quat)> =
        std::iter::once(&player.0).chain(opponents.iter().flat_map(|o| o.racers.iter().map(|r| &r.vehicle))).map(|v| (v.position, v.rotation)).collect();
    let flags = live_flags(Some(player), wreck.as_deref(), opponents.as_deref());
    let quad = |i: usize, _: f32| racers.get(i).copied();
    let flag = |i: usize, _: f32| flags.get(i).copied().unwrap_or(0);
    let dt = time.delta_secs();
    let mut scene = director::Scene {
        quad: &quad,
        flags: &flag,
        racers: racers.len(),
        target: live.target.min(racers.len() - 1),
        time: time.elapsed_secs(),
        collision: race.as_deref().map(|r| &r.collision),
        stick: Vec2::ZERO,
        real_dt: real.delta_secs(),
    };
    scene.target = live.director.pick_rider(&scene, players, dt);
    live.target = scene.target;
    let shot = live.director.shot(&scene, dt);
    // The first view: the attract demo's others stay on their riders.
    let Some((cam, mut proj, _)) = cams.iter_mut().find(|c| c.2.is_none_or(|s| s.0 == 0)) else { return };
    if let Ok(mut t) = transforms.get_mut(cam) {
        t.translation = shot.eye;
        t.rotation = shot.rotation;
    }
    if let Projection::Perspective(p) = proj.as_mut() {
        p.fov = shot.fov;
    }
    if let Some(mut t) = sky.iter().next().and_then(|e| transforms.get_mut(e).ok()) {
        t.translation = shot.eye - Vec3::Y * convert::SKY_RADIUS * 0.08;
    }
    // Every rider shows, whatever the race camera hid.
    for mut v in &mut riders {
        v.set_if_neq(Visibility::Inherited);
    }
}

/// How fast rewinding and fast forward go.
const WIND: f32 = 3.0;

/// The replay's controls. The original's are play, pause, start again and
/// back to the results; rewinding and fast forward while held, the rider
/// to watch and the camera are extras.
fn controls(keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>, mut playback: ResMut<Playback>, rec: Option<Res<Recording>>) {
    // The press that started the replay comes through on its first frame.
    if playback.hidden.is_none() {
        return;
    }
    let Some(rec) = rec else { return };
    let held = |k: &[KeyCode], b: &[GamepadButton]| k.iter().any(|k| keys.pressed(*k)) || pads.iter().any(|p| b.iter().any(|b| p.pressed(*b)));
    let rewind = held(&[KeyCode::ArrowLeft, KeyCode::KeyA], &[GamepadButton::DPadLeft, GamepadButton::LeftTrigger2]);
    let forward = held(&[KeyCode::ArrowRight, KeyCode::KeyD], &[GamepadButton::DPadRight, GamepadButton::RightTrigger2]);
    playback.speed = match (rewind, forward) {
        (true, false) => -WIND,
        (false, true) => WIND,
        _ => 1.0,
    };
    // Winding plays even when paused; the end holds the last moment.
    if rewind || forward {
        playback.paused = false;
    }
    match read_nav(&keys, &pads) {
        Some(Nav::Accept) => {
            // At the end, play starts it again.
            if playback.time >= rec.duration() {
                playback.time = 0.0;
                playback.paused = false;
            } else {
                playback.paused = !playback.paused;
            }
        }
        Some(nav @ (Nav::Up | Nav::Down)) if rec.quads.len() > 1 => {
            let n = rec.quads.len();
            let step = if nav == Nav::Up { n - 1 } else { 1 };
            playback.target = (playback.target + step) % n;
            playback.director.cut();
        }
        Some(Nav::Extra) => playback.director.next_camera(),
        Some(Nav::Back | Nav::Start) => playback.leaving = true,
        _ => {}
    }
    if keys.just_pressed(KeyCode::KeyC) || pads.iter().any(|p| p.just_pressed(GamepadButton::Select)) {
        playback.director.next_camera();
    }
    if keys.just_pressed(KeyCode::KeyR) || pads.iter().any(|p| p.just_pressed(GamepadButton::West)) {
        playback.time = 0.0;
        playback.paused = false;
    }
}

/// Puts everything recorded where it was at the playback time and points
/// the camera; leaving, back to how the race ended.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn play(
    mut commands: Commands,
    real: Res<Time<Real>>,
    (game, sel, settings, pads): (Res<Game>, Res<Selection>, Res<Settings>, Query<&Gamepad>),
    mut playback: ResMut<Playback>,
    rec: Option<Res<Recording>>,
    race: Option<Res<Race>>,
    ui: Option<Res<UiAssets>>,
    mut transforms: Query<&mut Transform>,
    mut cams: Query<(Entity, &mut Projection), With<ChaseCam>>,
    sky: Query<Entity, With<SkyDome>>,
    mut screens: Query<(Entity, &mut Visibility), (With<Node>, Without<ChildOf>, Without<ReplayUi>)>,
    mut riders: Query<&mut Visibility, (With<Rider>, Without<Node>)>,
    captions: Query<Entity, With<ReplayUi>>,
) {
    let Some(rec) = rec else { return };
    if playback.hidden.is_none() {
        let hidden = screens
            .iter_mut()
            .map(|(e, mut v)| {
                let was = *v;
                *v = Visibility::Hidden;
                (e, was)
            })
            .collect();
        playback.hidden = Some(hidden);
        if ui.is_some() {
            spawn_captions(&mut commands);
        }
        playback.director.load(&game.0, &sel.track, settings.camera);
        info!("replay: {:.1} s, {} racers, {} keys", rec.duration(), rec.quads.len(), rec.keys());
    }
    let end = rec.duration();
    if playback.leaving {
        playback.time = end;
    } else if !playback.paused {
        playback.time = (playback.time + real.delta_secs() * playback.speed).clamp(0.0, end);
        if playback.time >= end || (playback.time <= 0.0 && playback.speed < 0.0) {
            playback.paused = playback.time >= end;
        }
    }
    let at = playback.time * RATE;
    for (&e, track) in &rec.tracks {
        let Some((p, r)) = track.sample(at) else { continue };
        if let Ok(mut t) = transforms.get_mut(e) {
            t.translation = p;
            t.rotation = r;
        }
    }
    if playback.leaving {
        for (e, was) in playback.hidden.take().unwrap_or_default() {
            if let Ok((_, mut v)) = screens.get_mut(e) {
                *v = was;
            }
        }
        for e in &captions {
            commands.entity(e).despawn();
        }
        for (_, mut proj) in &mut cams {
            if let Projection::Perspective(p) = proj.as_mut() {
                p.fov = director::FOV;
            }
        }
        commands.remove_resource::<Playback>();
        return;
    }
    // Every rider shows, whatever the race camera hid.
    for mut v in &mut riders {
        v.set_if_neq(Visibility::Inherited);
    }
    let Some((cam, mut proj)) = cams.iter_mut().next() else { return };
    let quad = |i: usize, secs: f32| rec.quads.get(i).and_then(|&e| rec.transform_at(e, secs));
    let flags = |i: usize, secs: f32| rec.flags_at(i, secs);
    let target = playback.target.min(rec.quads.len().saturating_sub(1));
    let time = playback.time;
    let dt = if playback.paused { 0.0 } else { real.delta_secs() * playback.speed.abs() };
    let stick = pads.iter().map(|p| Vec2::new(p.get(GamepadAxis::RightStickX).unwrap_or(0.0), p.get(GamepadAxis::RightStickY).unwrap_or(0.0))).find(|s| s.length() > 0.2).unwrap_or(Vec2::ZERO);
    let scene = director::Scene {
        quad: &quad,
        flags: &flags,
        racers: rec.quads.len(),
        target,
        time,
        collision: race.as_deref().map(|r| &r.collision),
        stick,
        real_dt: real.delta_secs(),
    };
    let shot = playback.director.shot(&scene, dt);
    if let Ok(mut t) = transforms.get_mut(cam) {
        t.translation = shot.eye;
        t.rotation = shot.rotation;
    }
    if let Projection::Perspective(p) = proj.as_mut() {
        p.fov = shot.fov;
    }
    if let Some(mut t) = sky.iter().next().and_then(|e| transforms.get_mut(e).ok()) {
        t.translation = shot.eye - Vec3::Y * convert::SKY_RADIUS * 0.08;
    }
}

/// "REPLAY", who's being watched, the clock and the controls.
fn spawn_captions(commands: &mut Commands) {
    let grey = Color::srgb(0.75, 0.85, 1.0);
    commands.spawn((fullscreen(), ReplayUi, DespawnOnExit(AppState::Race), Pickable::IGNORE)).with_children(|root| {
        root.spawn(Node { position_type: PositionType::Absolute, left: px(24), top: px(18), flex_direction: FlexDirection::Column, row_gap: px(4), ..default() })
            .with_children(|c| {
                c.spawn(BitmapText::new(FontId::Menu, 17.0, "REPLAY"));
                c.spawn((BitmapText::new(FontId::Small, 9.0, ""), Caption::Rider));
            });
        root.spawn((BitmapText::new(FontId::Chrome, 15.0, ""), Node { position_type: PositionType::Absolute, right: px(24), top: px(18), ..default() }, Caption::Clock));
        root.spawn(Node {
            position_type: PositionType::Absolute,
            bottom: px(14),
            width: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: px(4),
            ..default()
        })
        .with_children(|c| {
            c.spawn((BitmapText::new(FontId::Small, 9.0, ""), Caption::Camera));
            c.spawn(BitmapText::new(FontId::Small, 8.0, CONTROLS).color(grey));
        });
    });
}

/// The controls, as the captions list them.
const CONTROLS: &str = "A / ENTER PAUSE   X / R RESTART   LEFT / RIGHT WIND   UP / DOWN RIDER   Y / C CAMERA   B / ESC RESULTS";

fn captions(playback: Option<Res<Playback>>, rec: Option<Res<Recording>>, mut texts: Query<(&mut BitmapText, &Caption)>) {
    // Gone once play has left.
    let (Some(playback), Some(rec)) = (playback, rec) else { return };
    for (mut t, which) in &mut texts {
        let s = match which {
            Caption::Rider => rec.names.get(playback.target).cloned().unwrap_or_default(),
            Caption::Clock => {
                let state = if playback.paused {
                    "  PAUSED"
                } else if playback.speed < 0.0 {
                    "  <<"
                } else if playback.speed > 1.0 {
                    "  >>"
                } else {
                    ""
                };
                format!("{}{state}", fmt_time(playback.time))
            }
            Caption::Camera => playback.director.label(),
        };
        set_text(&mut t, &s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_still_between_moves() {
        let mut t = Track::default();
        let at = |x: f32| Transform::from_xyz(x, 0.0, 0.0);
        t.push(0, 0, &at(0.0));
        t.push(1, 0, &at(1.0));
        // Still for a while, then off again.
        for tick in 2..10 {
            t.push(tick, tick - 1, &at(1.0));
        }
        t.push(10, 9, &at(3.0));
        assert_eq!(t.keys.len(), 4);
        assert_eq!(t.sample(0.5).unwrap().0.x, 0.5);
        assert_eq!(t.sample(5.0).unwrap().0.x, 1.0);
        assert_eq!(t.sample(9.5).unwrap().0.x, 2.0);
        assert_eq!(t.sample(20.0).unwrap().0.x, 3.0);
    }

    #[test]
    fn slow_frames_move_smoothly() {
        let mut t = Track::default();
        let at = |x: f32| Transform::from_xyz(x, 0.0, 0.0);
        // One frame every four samples, moving all the while.
        t.push(0, 0, &at(0.0));
        t.push(3, 0, &at(3.0));
        t.push(7, 3, &at(7.0));
        assert_eq!(t.keys.len(), 3);
        assert_eq!(t.sample(1.5).unwrap().0.x, 1.5);
        assert_eq!(t.sample(5.0).unwrap().0.x, 5.0);
    }
}
