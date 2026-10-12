//! The attract demo. Left on Press Start for `mAttractModeTime` with no
//! button pressed (0x98390), the original races computer riders on a
//! random track (0x97f90) with the TV director filming and its demo page
//! over the race (0x80ba0, 0x3c980: the big logo, "Press Start" and the
//! track). A button, the first rider home or `mDemoMaxTime` ends it
//! (0x80c30, 0x80d20), back on Press Start.
//!
//! Here player 1's seat is driven by the computer too, and the
//! [`crate::replay::Broadcast`] director films the race from the start.
//! As in the original, a third of the demos split the screen: the second
//! rider gets a view too, and half of those times the third and fourth as
//! well, laid out as split screen's two or four views (0x97f90 sets
//! `mHasCamera`, 0x86530 lays the views out). The director takes only the
//! first view (0x80ba0); the others follow their riders with the race
//! camera.

use bevy::input::gamepad::{Gamepad, GamepadAxis};
use bevy::prelude::*;

use crate::frontend::{self, FrontEnd, Roster, Screen};
use crate::hud::track_title;
use crate::intro::ShowInIntro;
use crate::mp::Overlay;
use crate::opponents::{Opponents, NOT_FOR_AI};
use crate::race::RaceProgress;
use crate::ui::{fullscreen, placed, BitmapText, FontId, UiAssets, SCREEN_H, SCREEN_W};
use crate::{AppState, Mode, Selection};

/// Seconds on Press Start with nothing pressed before the demo starts
/// (`mAttractModeTime` in `user.xml`, the original's default too).
pub const ATTRACT_TIME: f32 = 30.0;
/// The longest a demo race runs, seconds (`mDemoMaxTime`).
pub const MAX_TIME: f32 = 300.0;

/// A demo is on: the selection to put back afterwards, how long the race
/// has run and how many views it has.
#[derive(Resource)]
pub struct Demo {
    saved: Selection,
    clock: f32,
    leaving: bool,
    views: usize,
}

impl Demo {
    /// The views on the screen: player 1's seat, then the computer riders
    /// after it on the grid.
    pub fn views(&self) -> usize {
        self.views
    }
}

pub struct DemoPlugin;

impl Plugin for DemoPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, attract.run_if(in_state(AppState::FrontEnd)))
            .add_systems(OnEnter(AppState::Race), spawn_page.after(crate::setup).run_if(resource_exists::<Demo>))
            .add_systems(Update, (run, frontend::blink).run_if(in_state(AppState::Race)).run_if(resource_exists::<Demo>))
            .add_systems(OnExit(AppState::Race), finish);
    }
}

/// Whether anything was pressed or pushed this frame.
fn any_input(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>) -> bool {
    keys.get_pressed().next().is_some()
        || pads.iter().any(|p| {
            p.get_pressed().next().is_some()
                || [GamepadAxis::LeftStickX, GamepadAxis::LeftStickY, GamepadAxis::RightStickX, GamepadAxis::RightStickY]
                    .iter()
                    .any(|&a| p.get(a).unwrap_or(0.0).abs() > 0.3)
        })
}

/// Counts the time on Press Start with nothing pressed, and starts a demo
/// once it reaches [`ATTRACT_TIME`].
#[allow(clippy::too_many_arguments)]
fn attract(
    mut commands: Commands,
    real: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    fe: Res<FrontEnd>,
    roster: Res<Roster>,
    mut sel: ResMut<Selection>,
    mut next: ResMut<NextState<AppState>>,
    mut idle: Local<f32>,
) {
    if !fe.waiting_on_title() || any_input(&keys, &pads) {
        *idle = 0.0;
        return;
    }
    *idle += real.delta_secs();
    if *idle < ATTRACT_TIME {
        return;
    }
    *idle = 0.0;
    let mut seed = (real.elapsed().as_nanos() as u32) | 1;
    let Some((demo, views)) = pick(&roster, &sel, &mut seed) else { return };
    info!("demo: {} on {}, {views} view(s)", demo.track, demo.quad);
    commands.insert_resource(Demo { saved: std::mem::replace(&mut *sel, demo), clock: 0.0, leaving: false, views });
    next.set(AppState::Loading);
}

/// The demo race (0x97f90): a track the setup files mark
/// `mAvailableForDemo`, locked or not, and a computer rider and ATV for
/// player 1's seat; the computer riders fill the grid. Also gives the
/// views: one, or two a third of the time, and four half of those.
fn pick(roster: &Roster, sel: &Selection, seed: &mut u32) -> Option<(Selection, usize)> {
    let mut rand = |n: usize| {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed as usize % n.max(1)
    };
    let tracks = &roster.demo_tracks;
    let riders: Vec<_> = roster.riders.iter().filter(|r| r.ai).collect();
    let quads: Vec<_> = roster.quads.iter().filter(|q| q.ai && !NOT_FOR_AI.iter().any(|n| n.eq_ignore_ascii_case(&q.name))).collect();
    if tracks.is_empty() || riders.is_empty() || quads.is_empty() {
        return None;
    }
    let mut demo = sel.clone();
    demo.track = tracks[rand(tracks.len())].clone();
    demo.rider = riders[rand(riders.len())].model.clone();
    demo.quad = quads[rand(quads.len())].name.clone();
    demo.mode = Mode::SingleRace;
    demo.laps = 3;
    demo.players = 1;
    demo.others.clear();
    demo.field.clear();
    demo.grid = 1;
    demo.lesson = 0;
    let views = if rand(3) != 0 {
        1
    } else if rand(2) != 0 {
        2
    } else {
        4
    };
    Some((demo, views))
}

/// The demo page: the big logo, "PRESS START" flashing and the track,
/// over every view.
fn spawn_page(mut commands: Commands, ui: Option<Res<UiAssets>>, sel: Res<Selection>, overlay: Query<Entity, With<Overlay>>) {
    let Some(ui) = ui else { return };
    let mut page = commands.spawn((fullscreen(), crate::hud::HudRoot, DespawnOnExit(AppState::Race), Pickable::IGNORE, ShowInIntro));
    if let Ok(cam) = overlay.single() {
        page.insert(UiTargetCamera(cam));
    }
    page.with_children(|root| {
        root.spawn(Node {
            position_type: PositionType::Absolute,
            left: percent(50),
            margin: UiRect::left(px(-SCREEN_W / 2.0)),
            width: px(SCREEN_W),
            height: px(SCREEN_H),
            ..default()
        })
        .with_children(|frame| {
            // The prototype's logo page is 4:1, retail's 2:1: same height.
            let h = 320.0 / 4.0;
            let w = h * ui.logo.size.x / ui.logo.size.y;
            frame.spawn(placed(ui.logo.node(w, h), (SCREEN_W - w) / 2.0, 26.0));
            let at = |text: &str, font: FontId, size: f32, top: f32| {
                let width = ui.font(font).width(text, size);
                Node { position_type: PositionType::Absolute, left: px((SCREEN_W - width) / 2.0), top: px(top), ..default() }
            };
            let start = "PRESS START";
            frame.spawn((BitmapText::new(FontId::Menu, 18.0, start), at(start, FontId::Menu, 18.0, 392.0), frontend::Blink));
            let track = track_title(&sel.track);
            frame.spawn((BitmapText::new(FontId::Small, 9.0, track.clone()), at(&track, FontId::Small, 9.0, 424.0)));
        });
    });
}

/// Ends the demo on a button, the first rider home or [`MAX_TIME`]
/// (0x80c30, 0x80d20), back on Press Start.
#[allow(clippy::too_many_arguments)]
fn run(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mut demo: ResMut<Demo>,
    progress: Option<Res<RaceProgress>>,
    opponents: Option<Res<Opponents>>,
    mut fe: ResMut<FrontEnd>,
    mut next: ResMut<NextState<AppState>>,
) {
    demo.clock += time.delta_secs();
    let home = progress.is_some_and(|p| p.finished.is_some()) || opponents.is_some_and(|o| o.racers.iter().any(|r| r.progress.finished.is_some()));
    let pressed = keys.get_just_pressed().next().is_some() || pads.iter().any(|p| p.get_just_pressed().next().is_some());
    if !demo.leaving && (pressed || home || demo.clock > MAX_TIME) {
        demo.leaving = true;
        fe.return_to = Some(Screen::Title);
        next.set(AppState::FrontEnd);
    }
}

/// Puts the player's own selection back once the demo race is over.
fn finish(mut commands: Commands, demo: Option<Res<Demo>>, mut sel: ResMut<Selection>) {
    let Some(demo) = demo else { return };
    *sel = demo.saved.clone();
    commands.remove_resource::<Demo>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{QuadInfo, RiderInfo};

    fn roster() -> Roster {
        let rider = |model: &str, ai: bool| RiderInfo { name: model.into(), model: model.into(), born: String::new(), from: String::new(), stats: [3.0; 4], ai, locked: !ai, skill: 1 };
        let quad = |name: &str| QuadInfo { name: name.into(), stats: [0.0; 5], locked: false, ai: true };
        let mut r = Roster::default();
        r.riders = vec![rider("Male1", true), rider("Secret", false)];
        r.quads = vec![quad("Charger"), quad("Bandit")];
        r.demo_tracks = vec!["Forest2".into()];
        r
    }

    fn sel() -> Selection {
        Selection {
            track: "Dunes1".into(),
            quad: "Nitro".into(),
            rider: "Male3".into(),
            laps: 5,
            mode: Mode::Career,
            round: 2,
            failed: 0,
            time_limit: None,
            field: vec!["Male4".into()],
            grid: 4,
            challenge: 1,
            players: 2,
            others: vec![("Male2".into(), "Nitro".into())],
            lesson: 3,
            lessons_passed: 2,
            net: None,
        }
    }

    #[test]
    fn demo_races_a_computer_rider_alone() {
        let sel = sel();
        let mut seed = 7;
        let (demo, _) = pick(&roster(), &sel, &mut seed).unwrap();
        assert_eq!((demo.track.as_str(), demo.rider.as_str(), demo.quad.as_str()), ("Forest2", "Male1", "Bandit"));
        assert_eq!((demo.mode, demo.players, demo.others.len(), demo.lesson), (Mode::SingleRace, 1, 0, 0));
        let mut none = roster();
        none.demo_tracks.clear();
        assert!(pick(&none, &sel, &mut seed).is_none());
    }

    #[test]
    fn a_third_of_demos_split_the_screen() {
        let (roster, sel) = (roster(), sel());
        let mut seed = 12345;
        let mut counts = [0usize; 5];
        for _ in 0..6000 {
            counts[pick(&roster, &sel, &mut seed).unwrap().1] += 1;
        }
        assert_eq!(counts[0] + counts[3], 0, "one, two or four views");
        let near = |n: usize, want: usize| n.abs_diff(want) < 250;
        assert!(near(counts[1], 4000) && near(counts[2], 1000) && near(counts[4], 1000), "{counts:?}");
    }
}
