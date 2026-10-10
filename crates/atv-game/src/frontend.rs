//! The front end: title, main menu, then rider, ATV and track select into a
//! race, using the original backgrounds, fonts and panels. Which background
//! and title each screen uses comes from the disc's `Setup/Frontend.xml`.
//! Arcade skips track select (it races `Setup/Arcade.xml`'s rounds);
//! Open Freestyle picks a time limit on it instead of laps. Options holds
//! the Audio, Camera and Controller pages (see [`crate::settings`]).
//! Career picks a championship, then rider and ATV, and comes back to the
//! championship's status page between rounds (see [`crate::career`]).
//! Multiplayer picks how many play, then each player picks a rider and ATV
//! in turn before track select (see [`crate::mp`]).

use bevy::input::gamepad::Gamepad;
use bevy::prelude::*;

use atv_formats::setup::{self, Entry};
use atv_formats::GameSource;

use crate::freestyle::{self, limit_label, MEDALS, TIME_LIMITS};
use crate::career::{self, Career, Champ, Run};
use crate::challenge;
use crate::hud::{ordinal, track_title};
use crate::settings::{CameraView, Settings, VOLUME_STEPS};
use crate::ui::{fullscreen, placed, read_nav_from, set_text, spawn_menu_rows, BitmapText, FontId, MenuList, Nav, UiAssets, SCREEN_H, SCREEN_W};
use crate::mp::MpChamp;
use crate::{AppState, Mode, Selection};

pub struct FrontEndPlugin;

impl Plugin for FrontEndPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrontEnd>()
            .add_systems(OnEnter(AppState::FrontEnd), enter)
            .add_systems(
                Update,
                (navigate, rebuild, update_rider_panel, update_quad_panel, update_track_panel, update_options, blink, showroom, turntable)
                    .chain()
                    .run_if(in_state(AppState::FrontEnd)),
            )
            .add_systems(OnEnter(AppState::Loading), show_loading)
            .add_systems(Update, finish_loading.run_if(in_state(AppState::Loading)));
    }
}

/// Riders, quads and tracks as the disc's setup files list them.
#[derive(Resource, Clone, Default)]
pub struct Roster {
    pub riders: Vec<RiderInfo>,
    pub quads: Vec<QuadInfo>,
    pub tracks: Vec<String>,
    /// Arcade's rounds, in order (the ones on this disc).
    pub arcade: Vec<String>,
    /// Open Freestyle's Bronze/Silver/Gold scores per [`TIME_LIMITS`] entry.
    pub freestyle_medals: Vec<Option<[u32; 3]>>,
    info: Vec<(String, TrackInfo)>,
    screens: Vec<Entry>,
}

/// Per-track rules from `Tracks.xml`.
#[derive(Clone, Copy, Debug)]
pub struct TrackInfo {
    /// Arcade's starting clock, seconds.
    pub arcade_start: f32,
    /// The AI's slowest lap, seconds; arcade's checkpoints share it out.
    pub worst_lap: f32,
    /// Sand, snow or ordinary ground (`mTyreType`): tyres and dirt.
    pub ground: crate::convert::Ground,
}

impl Default for TrackInfo {
    fn default() -> Self {
        Self { arcade_start: 40.0, worst_lap: 100.0, ground: crate::convert::Ground::Road }
    }
}

#[derive(Clone, Debug)]
pub struct RiderInfo {
    pub name: String,
    /// Model folder under `Characters` (`Male10`).
    pub model: String,
    pub born: String,
    pub from: String,
    /// Ability, technique, aggression, strength (1-5).
    pub stats: [f32; 4],
    /// Whether computer opponents may ride as this rider (not the
    /// `LockedNeverAI` secret riders).
    pub ai: bool,
}

#[derive(Clone, Debug)]
pub struct QuadInfo {
    pub name: String,
    /// Top speed, acceleration, stability, handling, air rating (1-5).
    pub stats: [f32; 5],
}

impl Roster {
    pub fn load(game: &GameSource) -> Self {
        let read = |p: &str| game.read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let riders = setup::entries(&read("/Setup/Riders.xml"))
            .into_iter()
            .filter_map(|e| {
                let model = e.get("mModelFilename")?.to_string();
                Some(RiderInfo {
                    name: e.name.replace('_', " ").to_uppercase(),
                    model,
                    born: e.get("mBirthDate").unwrap_or("").trim().to_uppercase(),
                    from: e.get("mResidence").unwrap_or("").trim().to_uppercase(),
                    stats: ["mAbility", "mTechnique", "mAggression", "mStrength"].map(|k| e.num(k).unwrap_or(0.0)),
                    ai: e.get("mAvailability") != Some("LockedNeverAI"),
                })
            })
            .collect();
        let quads = setup::entries(&read("/Setup/VehicleSetup.xml"))
            .into_iter()
            .filter(|e| game.exists(&format!("/Cars/{0}/{0}.params", e.name)))
            .map(|e| QuadInfo {
                stats: ["mTopSpeed", "mAcceleration", "mStability", "mHandling", "mAirRating"].map(|k| e.num(k).unwrap_or(0.0)),
                name: e.name,
            })
            .collect();
        let track_entries: Vec<Entry> = setup::entries(&read("/Setup/Tracks.xml"))
            .into_iter()
            .filter(|e| game.exists(&format!("/Levels/{0}/World/{0}.row", e.name)))
            .collect();
        let d = TrackInfo::default();
        let info = track_entries
            .iter()
            .map(|e| {
                (e.name.clone(), TrackInfo {
                    arcade_start: e.num("mArcadeStartTime").filter(|v| *v > 0.0).unwrap_or(d.arcade_start),
                    worst_lap: e.num("mAIWorstLapTime").filter(|v| *v > 0.0).unwrap_or(d.worst_lap),
                    ground: crate::convert::Ground::from_tyre_type(e.get("mTyreType").unwrap_or("")),
                })
            })
            .collect();
        let tracks: Vec<String> = track_entries.into_iter().map(|e| e.name).collect();
        let arcade = setup::entries(&read("/Setup/Arcade.xml"))
            .into_iter()
            .filter_map(|e| e.get("TrackName").map(str::to_owned))
            .filter(|t| tracks.iter().any(|k| k.eq_ignore_ascii_case(t)))
            .collect();
        let freestyle_medals = TIME_LIMITS.iter().map(|&l| freestyle::medal_targets(game, l)).collect();
        Self { riders, quads, tracks, arcade, freestyle_medals, info, screens: setup::entries(&read("/Setup/Frontend.xml")) }
    }

    pub fn info(&self, track: &str) -> TrackInfo {
        self.info.iter().find(|(t, _)| t.eq_ignore_ascii_case(track)).map(|i| i.1).unwrap_or_default()
    }

    /// Background and title for a screen from `Frontend.xml`.
    fn screen(&self, key: &str) -> (String, String) {
        let e = self.screens.iter().find(|e| e.name == key);
        let bg = e.and_then(|e| e.get("BACKGROUND0")).unwrap_or("Single2").to_string();
        let title = e.and_then(|e| e.get("mTitleText")).filter(|t| *t != "NONE").unwrap_or("").replace('_', " ");
        (bg, title)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Screen {
    #[default]
    Title,
    Main,
    Rider,
    Quad,
    Track,
    Options,
    Audio,
    Camera,
    Controller,
    CareerChamp,
    CareerStatus,
    CareerComplete,
    /// Open Freestyle or the challenges.
    FreestyleType,
    Challenges,
    /// Split screen: Single Race or Freestyle Battle, then how many play.
    MpMode,
    MpPlayers,
    /// The split-screen championship: which one, then its standings.
    MpChamp,
    MpChampStatus,
    /// Arcade with a saved round: carry on from it or start again.
    ArcadeNew,
}

/// The Options menu's rows, and the page each opens.
const OPTIONS: [(&str, Option<Screen>); 5] = [
    ("AUDIO", Some(Screen::Audio)),
    ("CAMERA", Some(Screen::Camera)),
    ("CONTROLLER", Some(Screen::Controller)),
    ("REPLAYS", None),
    ("CREDITS", None),
];

#[derive(Resource, Default)]
pub struct FrontEnd {
    pub screen: Screen,
    built: Option<Screen>,
    pub mode: Mode,
    /// Open Freestyle's time limit, an index into [`TIME_LIMITS`].
    pub limit: usize,
    pub rider: usize,
    pub quad: usize,
    pub track: usize,
    /// The Options menu's cursor.
    option: usize,
    /// The championship being set up.
    champ: Champ,
    /// Where to come back to from a race, instead of the usual.
    pub return_to: Option<Screen>,
    /// The Freestyle Challenge picked (1-based).
    challenge: usize,
    /// How many play (more than one is split screen), and whose rider and
    /// ATV are being picked (0-based).
    players: usize,
    picking: usize,
    /// A split-screen championship is being set up.
    mp_champ: bool,
    /// The Arcade round to start on.
    arcade_round: usize,
}

impl FrontEnd {
    pub fn at(screen: Screen) -> Self {
        Self { screen, ..default() }
    }
}

#[derive(Component)]
struct ScreenRoot;
#[derive(Component)]
struct Blink;
#[derive(Component)]
struct MugCursor;
#[derive(Component)]
enum Field {
    RiderName,
    Born,
    From,
    QuadName,
    TrackName,
    Laps,
    Targets,
}
#[derive(Component)]
struct StatBar(usize);
#[derive(Component)]
struct TrackPreview;
/// The lit part of a volume bar: 0 music, 1 sound effects.
#[derive(Component)]
struct VolumeBar(usize);
#[derive(Component)]
struct VibrationBox;

const MUG: f32 = 50.0;
const MUG_GAP: f32 = 64.0;
const MUG_X: f32 = 64.0;
const MUG_Y: f32 = 92.0;

fn enter(mut fe: ResMut<FrontEnd>, sel: Res<Selection>, roster: Res<Roster>, career: Res<Career>) {
    // Back from a race: straight to the main menu, cursors on the last pick;
    // a career goes on to its championship's status.
    if let Some(screen) = fe.return_to.take() {
        fe.screen = screen;
    } else if fe.built.is_some() {
        fe.screen = match &career.run {
            Some(r) if sel.mode == Mode::Career => {
                if r.over() {
                    Screen::CareerComplete
                } else {
                    Screen::CareerStatus
                }
            }
            _ => Screen::Main,
        };
    }
    fe.built = None;
    fe.mode = sel.mode;
    fe.limit = TIME_LIMITS.iter().position(|&l| l == sel.time_limit).unwrap_or(1);
    fe.rider = roster.riders.iter().position(|r| r.model.eq_ignore_ascii_case(&sel.rider)).unwrap_or(fe.rider);
    fe.quad = roster.quads.iter().position(|q| q.name.eq_ignore_ascii_case(&sel.quad)).unwrap_or(fe.quad);
    fe.track = roster.tracks.iter().position(|t| t.eq_ignore_ascii_case(&sel.track)).unwrap_or(fe.track);
    fe.challenge = sel.challenge.clamp(1, challenge::COUNT);
    fe.players = sel.players;
    fe.picking = 0;
}

/// Whether an earlier player has picked `model` in split screen.
fn taken(fe: &FrontEnd, sel: &Selection, model: &str) -> bool {
    fe.picking > 0 && std::iter::once(&sel.rider).chain(sel.others.iter().take(fe.picking - 1).map(|o| &o.0)).any(|m| m.eq_ignore_ascii_case(model))
}

#[allow(clippy::too_many_arguments)]
fn navigate(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mut fe: ResMut<FrontEnd>,
    roster: Res<Roster>,
    mut sel: ResMut<Selection>,
    mut lists: Query<&mut MenuList>,
    mut next: ResMut<NextState<AppState>>,
    mut exit: MessageWriter<AppExit>,
    mut settings: ResMut<Settings>,
    (mut career, rules, defs, mut mp_champ): (ResMut<Career>, Res<career::Rules>, Res<challenge::Defs>, ResMut<MpChamp>),
) {
    // In split screen each player picks their rider and ATV with their own pad.
    let picking = fe.players > 1 && matches!(fe.screen, Screen::Rider | Screen::Quad);
    let Some(nav) = read_nav_from(&keys, &pads, picking.then_some(fe.picking)) else { return };
    if fe.built != Some(fe.screen) {
        return;
    }
    let mut list = lists.single_mut().ok();
    match fe.screen {
        Screen::Title => {
            if matches!(nav, Nav::Accept | Nav::Start) {
                fe.screen = Screen::Main;
            }
        }
        Screen::Main => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                }
            }
            Nav::Accept | Nav::Start => match list.as_ref().map(|l| l.current().to_string()).as_deref() {
                Some("MULTIPLAYER") => fe.screen = Screen::MpMode,
                item => {
                    // Everything else is one player.
                    fe.players = 1;
                    sel.players = 1;
                    sel.others.clear();
                    mp_champ.run = None;
                    match item {
                        Some("FREESTYLE") => fe.screen = Screen::FreestyleType,
                        Some(item @ ("ARCADE" | "SINGLE RACE" | "TIME TRIAL")) => {
                            fe.mode = match item {
                                "ARCADE" => Mode::Arcade,
                                "TIME TRIAL" => Mode::TimeTrial,
                                _ => Mode::SingleRace,
                            };
                            sel.mode = fe.mode;
                            fe.arcade_round = 0;
                            let saved = career.arcade > 0 && career.arcade < roster.arcade.len();
                            fe.screen = if fe.mode == Mode::Arcade && saved { Screen::ArcadeNew } else { Screen::Rider };
                        }
                        Some("CAREER") => {
                            fe.mode = Mode::Career;
                            sel.mode = Mode::Career;
                            fe.screen = Screen::CareerChamp;
                        }
                        Some("OPTIONS") => fe.screen = Screen::Options,
                        Some("QUIT GAME") => {
                            exit.write(AppExit::Success);
                        }
                        _ => {}
                    }
                }
            },
            Nav::Back => fe.screen = Screen::Title,
            _ => {}
        },
        Screen::Rider => {
            let n = roster.riders.len().max(1);
            match nav {
                Nav::Left => fe.rider = (fe.rider + n - 1) % n,
                Nav::Right => fe.rider = (fe.rider + 1) % n,
                Nav::Up => fe.rider = (fe.rider + n - 8 % n) % n,
                Nav::Down => fe.rider = (fe.rider + 8) % n,
                Nav::Accept | Nav::Start => {
                    if let Some(r) = roster.riders.get(fe.rider) {
                        if taken(&fe, &sel, &r.model) {
                            return;
                        }
                        if fe.picking == 0 {
                            sel.rider = r.model.clone();
                        } else {
                            sel.others.truncate(fe.picking - 1);
                            let quad = sel.quad.clone();
                            sel.others.push((r.model.clone(), quad));
                        }
                    }
                    fe.screen = Screen::Quad;
                }
                Nav::Back if fe.picking > 0 => {
                    fe.picking -= 1;
                    fe.screen = Screen::Quad;
                }
                Nav::Back if fe.players > 1 => fe.screen = Screen::MpPlayers,
                Nav::Back if fe.mode == Mode::Career => fe.screen = Screen::CareerChamp,
                Nav::Back if fe.mode == Mode::Arcade && career.arcade > 0 && career.arcade < roster.arcade.len() => fe.screen = Screen::ArcadeNew,
                Nav::Back if fe.mode == Mode::Freestyle => fe.screen = Screen::FreestyleType,
                Nav::Back if fe.mode == Mode::Challenge => fe.screen = Screen::Challenges,
                Nav::Back => fe.screen = Screen::Main,
            }
        }
        Screen::Quad => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                    fe.quad = l.selected;
                }
            }
            Nav::Accept | Nav::Start => {
                if list.as_ref().is_some_and(|l| l.disabled.get(l.selected) == Some(&true)) {
                    return;
                }
                if let Some(q) = roster.quads.get(fe.quad) {
                    match fe.picking.checked_sub(1).and_then(|i| sel.others.get_mut(i)) {
                        Some(other) => other.1 = q.name.clone(),
                        None => sel.quad = q.name.clone(),
                    }
                }
                if fe.picking + 1 < fe.players {
                    // The next player picks theirs, from the first rider free.
                    fe.picking += 1;
                    let n = roster.riders.len();
                    if let Some(i) = (0..n).map(|i| (fe.rider + i) % n).find(|&i| !taken(&fe, &sel, &roster.riders[i].model)) {
                        fe.rider = i;
                    }
                    fe.screen = Screen::Rider;
                } else if fe.mp_champ && fe.players > 1 {
                    fe.screen = Screen::MpChamp;
                } else if fe.mode == Mode::Career {
                    let rider = roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&sel.rider));
                    let name = rider.map_or(sel.rider.to_uppercase(), |r| r.name.clone());
                    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
                    career.start(fe.champ, (&name, &sel.rider), &sel.quad, &career_pool(&roster, &rules, fe.champ), seed);
                    career.save();
                    fe.screen = Screen::CareerStatus;
                } else if fe.mode == Mode::Challenge {
                    sel.track = challenge::TRACK.into();
                    sel.laps = 1;
                    sel.challenge = fe.challenge;
                    next.set(AppState::Loading);
                } else if fe.mode == Mode::Arcade {
                    // Arcade starts on its first round, or the saved one.
                    if let Some(t) = roster.arcade.get(fe.arcade_round) {
                        sel.round = fe.arcade_round;
                        sel.track = t.clone();
                        sel.laps = 3;
                        next.set(AppState::Loading);
                    }
                } else {
                    fe.screen = Screen::Track;
                }
            }
            Nav::Back => fe.screen = Screen::Rider,
            _ => {}
        },
        Screen::Track => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                    fe.track = l.selected;
                }
            }
            Nav::Left | Nav::Right if fe.mode == Mode::Freestyle => {
                let n = TIME_LIMITS.len();
                fe.limit = if nav == Nav::Left { (fe.limit + n - 1) % n } else { (fe.limit + 1) % n };
                sel.time_limit = TIME_LIMITS[fe.limit];
            }
            Nav::Left => sel.laps = (sel.laps + 8) % 9 + 1,
            Nav::Right => sel.laps = sel.laps % 9 + 1,
            Nav::Accept | Nav::Start => {
                if let Some(t) = roster.tracks.get(fe.track) {
                    sel.track = t.clone();
                }
                next.set(AppState::Loading);
            }
            Nav::Back => fe.screen = Screen::Quad,
        },
        Screen::Options => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                    fe.option = l.selected;
                }
            }
            Nav::Accept | Nav::Start => {
                if let Some((_, Some(page))) = OPTIONS.get(fe.option) {
                    fe.screen = *page;
                }
            }
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::Audio => {
            let row = list.as_ref().map_or(0, |l| l.selected);
            let level = if row == 0 { &mut settings.music } else { &mut settings.effects };
            match nav {
                Nav::Up | Nav::Down => {
                    if let Some(l) = list.as_mut() {
                        l.step(nav);
                    }
                }
                Nav::Left => *level = level.saturating_sub(1),
                Nav::Right => *level = (*level + 1).min(VOLUME_STEPS),
                Nav::Accept | Nav::Start | Nav::Back => {
                    settings.save();
                    fe.screen = Screen::Options;
                }
            }
        }
        Screen::Camera => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                    settings.camera = CameraView::ALL[l.selected.min(CameraView::ALL.len() - 1)];
                }
            }
            Nav::Accept | Nav::Start | Nav::Back => {
                settings.save();
                fe.screen = Screen::Options;
            }
            _ => {}
        },
        Screen::Controller => match nav {
            Nav::Accept | Nav::Left | Nav::Right => settings.vibration = !settings.vibration,
            Nav::Start | Nav::Back => {
                settings.save();
                fe.screen = Screen::Options;
            }
            _ => {}
        },
        Screen::CareerChamp => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                }
            }
            Nav::Accept | Nav::Start => {
                let Some(l) = list.as_ref() else { return };
                match champ_items(&career).get(l.selected) {
                    Some((_, None)) => {
                        if let Some(r) = &career.run {
                            sel.rider = r.rider.clone();
                            sel.quad = r.quad.clone();
                        }
                        fe.screen = Screen::CareerStatus;
                    }
                    Some((_, Some(c))) if career.is_open(*c) => {
                        fe.champ = *c;
                        fe.screen = Screen::Rider;
                    }
                    _ => {}
                }
            }
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::ArcadeNew => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                }
            }
            Nav::Accept | Nav::Start => {
                // The saved round first, then a new game.
                fe.arcade_round = if list.as_ref().is_some_and(|l| l.selected == 0) { career.arcade } else { 0 };
                fe.screen = Screen::Rider;
            }
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::CareerStatus => match nav {
            Nav::Accept | Nav::Start => {
                let Some(run) = &career.run else { return };
                if run.over() {
                    fe.screen = Screen::CareerComplete;
                    return;
                }
                let Some(track) = rules.tracks[run.champ.index()].get(run.round) else { return };
                sel.mode = Mode::Career;
                sel.track = track.clone();
                sel.round = run.round;
                sel.laps = career::LAPS;
                sel.rider = run.rider.clone();
                sel.quad = run.quad.clone();
                sel.field = run.opponents();
                next.set(AppState::Loading);
            }
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::CareerComplete => {
            if matches!(nav, Nav::Accept | Nav::Start | Nav::Back) {
                career.run = None;
                career.save();
                fe.screen = Screen::Main;
            }
        }
        Screen::FreestyleType => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                }
            }
            Nav::Accept | Nav::Start => {
                let challenges = list.as_ref().is_some_and(|l| l.selected == 1);
                fe.mode = if challenges { Mode::Challenge } else { Mode::Freestyle };
                sel.mode = fe.mode;
                fe.screen = if challenges { Screen::Challenges } else { Screen::Rider };
            }
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::Challenges => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                    fe.challenge = l.selected + 1;
                    // The details beside the list follow the cursor.
                    fe.built = None;
                }
            }
            Nav::Accept | Nav::Start => {
                let n = fe.challenge;
                if challenge::is_open(&career, n) && defs.get(n).is_some_and(|d| d.gates > 0) {
                    sel.challenge = n;
                    fe.screen = Screen::Rider;
                }
            }
            Nav::Back => fe.screen = Screen::FreestyleType,
            _ => {}
        },
        Screen::MpChamp => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                }
            }
            Nav::Accept | Nav::Start => {
                let champ = Champ::ALL[list.as_ref().map_or(0, |l| l.selected).min(2)];
                let name = |model: &str| roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(model)).map_or(model.to_uppercase(), |r| r.name.clone());
                let players: Vec<(String, String)> =
                    std::iter::once(&sel.rider).chain(sel.others.iter().map(|o| &o.0)).take(fe.players).map(|m| (name(m), m.clone())).collect();
                let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
                mp_champ.start(champ, &players, &sel.quad, &career_pool(&roster, &rules, champ), seed);
                fe.screen = Screen::MpChampStatus;
            }
            Nav::Back => fe.screen = Screen::Quad,
            _ => {}
        },
        Screen::MpChampStatus => match nav {
            Nav::Accept | Nav::Start => {
                let Some(run) = &mp_champ.run else { return };
                if run.over() {
                    mp_champ.run = None;
                    fe.screen = Screen::Main;
                    return;
                }
                let Some(track) = rules.tracks[run.champ.index()].get(run.round) else { return };
                sel.mode = Mode::SingleRace;
                sel.track = track.clone();
                sel.round = run.round;
                sel.laps = career::LAPS;
                sel.field = run.opponents();
                next.set(AppState::Loading);
            }
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::MpMode => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                }
            }
            Nav::Accept | Nav::Start => {
                let Some(l) = list.as_ref() else { return };
                if l.disabled.get(l.selected) == Some(&true) {
                    return;
                }
                fe.mode = if l.selected == 1 { Mode::Freestyle } else { Mode::SingleRace };
                sel.mode = fe.mode;
                fe.mp_champ = l.selected == 2;
                mp_champ.run = None;
                fe.screen = Screen::MpPlayers;
            }
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::MpPlayers => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                }
            }
            Nav::Accept | Nav::Start => {
                fe.players = list.as_ref().map_or(2, |l| l.selected + 2);
                sel.players = fe.players;
                sel.others.clear();
                fe.picking = 0;
                fe.screen = Screen::Rider;
            }
            Nav::Back => fe.screen = Screen::MpMode,
            _ => {}
        },
    }
}

/// The championship menu: carrying on with one in progress, then the three.
fn champ_items(career: &Career) -> Vec<(String, Option<Champ>)> {
    let mut items = Vec::new();
    if let Some(r) = career.run.as_ref().filter(|r| !r.over()) {
        items.push((format!("CONTINUE {}", r.champ.label()), None));
    }
    items.extend(Champ::ALL.map(|c| (c.label().to_string(), Some(c))));
    items
}

/// The riders a championship may draw its field from, as (name, model).
fn career_pool(roster: &Roster, rules: &career::Rules, champ: Champ) -> Vec<(String, String)> {
    let allowed = &rules.riders[champ.index()];
    let pool: Vec<(String, String)> = roster
        .riders
        .iter()
        .filter(|r| allowed.iter().any(|a| a.replace('_', " ").eq_ignore_ascii_case(&r.name)))
        .map(|r| (r.name.clone(), r.model.clone()))
        .collect();
    if pool.len() >= career::FIELD - 1 {
        pool
    } else {
        roster.riders.iter().filter(|r| r.ai).map(|r| (r.name.clone(), r.model.clone())).collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn rebuild(
    mut commands: Commands,
    mut fe: ResMut<FrontEnd>,
    roster: Res<Roster>,
    settings: Res<Settings>,
    (career, rules, defs, mp_champ): (Res<Career>, Res<career::Rules>, Res<challenge::Defs>, Res<MpChamp>),
    ui: Option<Res<UiAssets>>,
    roots: Query<Entity, With<ScreenRoot>>,
    sel: Res<Selection>,
) {
    let Some(ui) = ui else { return };
    if fe.built == Some(fe.screen) {
        return;
    }
    for e in &roots {
        commands.entity(e).despawn();
    }
    fe.built = Some(fe.screen);
    // Frontend.xml's screen names per mode.
    let (rider_key, atv_key, track_key) = match fe.mode {
        Mode::SingleRace => ("SingleRace_Rider", "SingleRace_ATV", "SingleRace_Track"),
        Mode::TimeTrial => ("TimeTrial_Rider", "TimeTrial_ATV", "TimeTrial_Track"),
        Mode::Arcade => ("Arcade_Rider", "Arcade_ATV", "SingleRace_Track"),
        Mode::Freestyle => ("Freestyle_Open_Rider", "Freestyle_Open_ATV", "Freestyle_Open"),
        Mode::Career => ("Profile_Rider", "Career_ATV", "SingleRace_Track"),
        Mode::Challenge => ("Freestyle_Open_Rider", "Freestyle_Challenge_ATV", "Freestyle_Challenge"),
    };
    let split = fe.players > 1;
    let (rider_key, atv_key, track_key) = match (split, fe.mode) {
        (true, Mode::Freestyle) => ("Multiplayer_Freestyle_Rider", "Multiplayer_Freestyle_ATV", "Multiplayer_Freestyle"),
        (true, _) => ("Multiplayer_SingleRace_Rider", "Multiplayer_SingleRace_ATV", "Multiplayer_SingleRace_Track"),
        _ => (rider_key, atv_key, track_key),
    };
    let label = match (split, fe.mode) {
        (true, Mode::Freestyle) => "FREESTYLE BATTLE",
        (true, _) if fe.mp_champ => "CHAMPIONSHIP",
        (true, _) => "MULTIPLAYER",
        _ => fe.mode.label(),
    };
    let (key, watermark) = match fe.screen {
        Screen::Title => ("Main_Menu", ""),
        Screen::Main => ("Main_Menu", ""),
        Screen::Rider => (rider_key, label),
        Screen::Quad => (atv_key, label),
        Screen::Track => (track_key, label),
        Screen::Options => ("Options", "OPTIONS"),
        Screen::Audio => ("Options_Sound", "OPTIONS"),
        Screen::Camera => ("Options_Camera", "OPTIONS"),
        Screen::Controller => ("Options_Controller", "OPTIONS"),
        Screen::CareerChamp => ("Career_Newgame", "CAREER"),
        Screen::CareerStatus => (career.run.as_ref().map_or("Career_Status_Amateur", |r| r.champ.status_screen()), "CAREER"),
        Screen::CareerComplete => ("Career_Complete", "CAREER"),
        Screen::FreestyleType => ("Freestyle_Picktype", "FREESTYLE"),
        Screen::Challenges => ("Freestyle_Challenge", "FREESTYLE"),
        Screen::MpMode => ("Multiplayer_New_Game", ""),
        Screen::MpPlayers => ("Multiplayer_Num_Players", label),
        Screen::MpChamp => ("Career_Newgame", "CHAMPIONSHIP"),
        Screen::MpChampStatus => (mp_champ.run.as_ref().map_or("Career_Status_Amateur", |r| r.champ.status_screen()), "CHAMPIONSHIP"),
        Screen::ArcadeNew => ("Arcade_Newgame", "ARCADE"),
    };
    let (bg, title) = roster.screen(key);

    // Background stretched over the window; the screen itself is laid out
    // on a centred 640x480 frame.
    let root = commands
        .spawn((fullscreen(), ScreenRoot, DespawnOnExit(AppState::FrontEnd), BackgroundColor(Color::srgb(0.02, 0.03, 0.1))))
        .with_children(|r| {
            if let Some(tex) = ui.background(&bg) {
                let (mut n, img) = tex.node(0.0, 0.0);
                n.position_type = PositionType::Absolute;
                n.width = percent(100);
                n.height = percent(100);
                r.spawn((n, img));
            }
        })
        .id();
    let frame = commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(50.0),
            margin: UiRect::left(px(-SCREEN_W / 2.0)),
            width: px(SCREEN_W),
            height: px(SCREEN_H),
            ..default()
        })
        .id();
    commands.entity(root).add_child(frame);

    let mut kids: Vec<Entity> = Vec::new();
    if !title.is_empty() {
        let e = commands.spawn((BitmapText::new(FontId::Menu, 20.0, title.clone()), ui_at(40.0, 28.0))).id();
        kids.push(e);
    }
    if !watermark.is_empty() {
        let w = ui.font(FontId::Small).width(watermark, 9.0);
        let e = commands.spawn((BitmapText::new(FontId::Small, 9.0, watermark).color(Color::srgba(0.7, 0.8, 1.0, 0.8)), ui_at(SCREEN_W - 40.0 - w, 36.0))).id();
        kids.push(e);
    }
    // Whose pick it is, in split screen.
    if split && matches!(fe.screen, Screen::Rider | Screen::Quad) {
        let t = format!("PLAYER {}", fe.picking + 1);
        let w = ui.font(FontId::Menu).width(&t, 16.0);
        kids.push(commands.spawn((BitmapText::new(FontId::Menu, 16.0, t).color(Color::srgb(1.0, 0.85, 0.3)), ui_at(SCREEN_W - 40.0 - w, 52.0))).id());
    }

    match fe.screen {
        Screen::Title => {
            let w = 460.0;
            kids.push(commands.spawn(placed(ui.logo.node(w, w / 4.0), (SCREEN_W - w) / 2.0, 120.0)).id());
            let t = "PRESS START";
            let tw = ui.font(FontId::Menu).width(t, 18.0);
            kids.push(commands.spawn((BitmapText::new(FontId::Menu, 18.0, t), ui_at((SCREEN_W - tw) / 2.0, 330.0), Blink)).id());
        }
        Screen::Main => {
            let w = 400.0;
            kids.push(commands.spawn(placed(ui.logo.node(w, w / 4.0), (SCREEN_W - w) / 2.0, 34.0)).id());
            let current = match fe.mode {
                _ if fe.players > 1 => 5,
                Mode::Arcade => 0,
                Mode::SingleRace => 2,
                Mode::TimeTrial => 3,
                Mode::Freestyle | Mode::Challenge => 4,
                Mode::Career => 1,
            };
            let mut off = vec![];
            if roster.arcade.is_empty() {
                off.push("ARCADE");
            }
            let list = MenuList::new(&["ARCADE", "CAREER", "SINGLE RACE", "TIME TRIAL", "FREESTYLE", "MULTIPLAYER", "OPTIONS", "QUIT GAME"], 280.0)
                .with_row(32.0)
                .select(current)
                .disable(&off);
            kids.push(menu(&mut commands, &ui, list, (SCREEN_W - 280.0) / 2.0, 160.0));
        }
        Screen::Rider => {
            // The selection square goes behind the mugs; its glow frames them.
            let sq = ui.design.get("SELECTION_SQUARE").rect;
            let (mut n, img) = ui.design.tex.region(sq, (MUG + 22.0) / sq.width());
            n.position_type = PositionType::Absolute;
            kids.push(commands.spawn((n, img, MugCursor)).id());
            for (i, rider) in roster.riders.iter().enumerate().take(24) {
                let (x, y) = mug_pos(i);
                let rect = Rect::new((i % 8) as f32 * 64.0, (i / 8) as f32 * 64.0, (i % 8) as f32 * 64.0 + 64.0, (i / 8) as f32 * 64.0 + 64.0);
                let scale = ui.mugs.size.x / 512.0;
                let rect = Rect::from_corners(rect.min * scale, rect.max * scale);
                let (mut n, mut img) = ui.mugs.region(rect, MUG / rect.width());
                if taken(&fe, &sel, &rider.model) {
                    // Another player has them.
                    img.color = Color::srgb(0.25, 0.25, 0.25);
                }
                n.position_type = PositionType::Absolute;
                n.left = px(x);
                n.top = px(y);
                kids.push(commands.spawn((n, img)).id());
            }
            kids.push(bio_panel(&mut commands, &ui, 64.0, 296.0, &["ABILITY", "TECHNIQUE", "AGGRESSION", "STRENGTH"], true));
        }
        Screen::Quad => {
            let names: Vec<String> = roster.quads.iter().map(|q| q.name.to_uppercase()).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let mut list = MenuList::new(&refs, 200.0).with_row(26.0).select(fe.quad);
            if fe.mode == Mode::Career {
                // Career's unlockable ATVs come with gold medals.
                let locked: Vec<String> = rules.locked_atvs(career.golds()).into_iter().map(|(a, _)| a.to_uppercase()).collect();
                let locked: Vec<&str> = locked.iter().map(String::as_str).collect();
                list = list.disable(&locked);
                fe.quad = list.selected;
            }
            kids.push(menu(&mut commands, &ui, list, 40.0, 80.0));
            kids.push(bio_panel(&mut commands, &ui, 270.0, 250.0, &["TOP SPEED", "ACCELERATION", "STABILITY", "HANDLING", "AIR RATING"], false));
        }
        Screen::Track => {
            let names: Vec<String> = roster.tracks.iter().map(|t| track_title(t)).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let list = MenuList::new(&refs, 200.0).with_row(20.0).select(fe.track);
            kids.push(menu(&mut commands, &ui, list, 40.0, 70.0));
            let side = 250.0;
            let preview = commands
                .spawn((
                    Node { position_type: PositionType::Absolute, left: px(300), top: px(76), width: px(side), height: px(side), ..default() },
                    ImageNode::default().with_mode(bevy::ui::widget::NodeImageMode::Stretch),
                    TrackPreview,
                ))
                .id();
            kids.push(preview);
            kids.push(commands.spawn((BitmapText::new(FontId::Menu, 16.0, ""), ui_at(300.0, 340.0), Field::TrackName)).id());
            let freestyle = fe.mode == Mode::Freestyle;
            let laps = commands
                .spawn(Node { position_type: PositionType::Absolute, left: px(300), top: px(370), align_items: AlignItems::Center, column_gap: px(10), ..default() })
                .with_children(|r| {
                    r.spawn(BitmapText::new(FontId::Small, 10.0, if freestyle { "TIME" } else { "LAPS" }));
                    let (n, mut img) = ui.design.node("DARKLRARROW", 0.6);
                    img.flip_x = true;
                    r.spawn((n, img));
                    r.spawn((if freestyle { BitmapText::new(FontId::Menu, 13.0, "") } else { BitmapText::new(FontId::Chrome, 16.0, "3") }, Field::Laps));
                    r.spawn(ui.design.node("DARKLRARROW", 0.6));
                })
                .id();
            kids.push(laps);
            if freestyle {
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 8.0, "").color(Color::srgb(1.0, 0.85, 0.3)), ui_at(300.0, 398.0), Field::Targets)).id());
            }
        }
        Screen::Options => {
            let names = OPTIONS.map(|(n, _)| n);
            let off: Vec<&str> = OPTIONS.iter().filter(|(_, page)| page.is_none()).map(|(n, _)| *n).collect();
            let list = MenuList::new(&names, 280.0).with_row(36.0).select(fe.option).disable(&off);
            kids.push(menu(&mut commands, &ui, list, (SCREEN_W - 280.0) / 2.0, 130.0));
        }
        Screen::Audio => {
            let row = 64.0;
            let list = MenuList::new(&["MUSIC VOLUME", "SOUND EFFECTS VOLUME"], 300.0).with_row(row);
            kids.push(menu(&mut commands, &ui, list, 40.0, 150.0));
            for i in 0..2 {
                kids.push(volume_bar(&mut commands, &ui, i, 370.0, 150.0 + i as f32 * row + 2.0));
            }
            kids.push(hint(&mut commands, &ui, "LEFT AND RIGHT CHANGE THE VOLUME", 330.0));
        }
        Screen::Camera => {
            let names = CameraView::ALL.map(CameraView::label);
            let list = MenuList::new(&names, 320.0).with_row(36.0).select(settings.camera.index());
            kids.push(menu(&mut commands, &ui, list, (SCREEN_W - 320.0) / 2.0, 150.0));
            kids.push(hint(&mut commands, &ui, "PRESS C OR BACK IN A RACE TO CHANGE CAMERA", 300.0));
        }
        Screen::Controller => {
            let list = MenuList::new(&["VIBRATION"], 220.0).with_row(40.0);
            kids.push(menu(&mut commands, &ui, list, 150.0, 80.0));
            let (n, img) = ui.checkbox.node("ON", 0.5);
            kids.push(commands.spawn((placed((n, img), 400.0, 84.0), VibrationBox)).id());
            kids.push(controls_table(&mut commands, &ui));
        }
        Screen::CareerChamp => {
            let items = champ_items(&career);
            let names: Vec<&str> = items.iter().map(|(n, _)| n.as_str()).collect();
            let locked: Vec<&str> = items.iter().filter(|(_, c)| c.is_some_and(|c| !career.is_open(c))).map(|(n, _)| n.as_str()).collect();
            let list = MenuList::new(&names, 320.0).with_row(36.0).disable(&locked);
            kids.push(menu(&mut commands, &ui, list, (SCREEN_W - 320.0) / 2.0, 120.0));
            let level = rules.skill_level(career.skill_points);
            let lines = [
                format!("SKILL LEVEL {level}   -   {} SKILL POINTS", career.skill_points),
                format!("CERTIFICATE  {}", rules.certificate(career.skill_points)),
                format!("GOLD MEDALS  {}", career.golds()),
            ];
            for (i, l) in lines.iter().enumerate() {
                kids.push(hint(&mut commands, &ui, l, 300.0 + 18.0 * i as f32));
            }
            if !locked.is_empty() {
                kids.push(hint(&mut commands, &ui, "FINISH IN THE TOP 3 OF A CHAMPIONSHIP TO OPEN THE NEXT", 370.0));
            }
        }
        Screen::ArcadeNew => {
            let cont = format!("CONTINUE FROM ROUND {}", career.arcade + 1);
            let list = MenuList::new(&[cont.as_str(), "NEW GAME"], 360.0).with_row(36.0);
            kids.push(menu(&mut commands, &ui, list, (SCREEN_W - 360.0) / 2.0, 170.0));
            if let Some(t) = roster.arcade.get(career.arcade) {
                kids.push(hint(&mut commands, &ui, &format!("ROUND {} OF {}  -  {}", career.arcade + 1, roster.arcade.len(), track_title(t)), 330.0));
            }
        }
        Screen::MpPlayers => {
            let list = MenuList::new(&["TWO PLAYERS", "THREE PLAYERS", "FOUR PLAYERS"], 320.0).with_row(36.0).select(fe.players.clamp(2, 4) - 2);
            kids.push(menu(&mut commands, &ui, list, (SCREEN_W - 320.0) / 2.0, 170.0));
            let note = if fe.mode == Mode::Freestyle { "MOST POINTS BEFORE THE TIME RUNS OUT WINS" } else { "COMPUTER RIDERS MAKE THE FIELD UP TO FOUR" };
            kids.push(hint(&mut commands, &ui, note, 330.0));
        }
        Screen::MpMode => {
            let list = MenuList::new(&["SINGLE RACE", "FREESTYLE BATTLE", "CHAMPIONSHIP"], 320.0)
                .with_row(36.0)
                .select(if fe.mp_champ { 2 } else { usize::from(fe.mode == Mode::Freestyle) });
            kids.push(menu(&mut commands, &ui, list, (SCREEN_W - 320.0) / 2.0, 170.0));
        }
        Screen::FreestyleType => {
            let list = MenuList::new(&["OPEN FREESTYLE", "FREESTYLE CHALLENGE"], 320.0).with_row(36.0).select(usize::from(fe.mode == Mode::Challenge));
            kids.push(menu(&mut commands, &ui, list, (SCREEN_W - 320.0) / 2.0, 170.0));
        }
        Screen::Challenges => {
            let open: Vec<bool> = (1..=challenge::COUNT).map(|n| challenge::is_open(&career, n)).collect();
            let names: Vec<String> = (1..=challenge::COUNT)
                .map(|n| {
                    let medal = career.challenges.get(&n).and_then(|c| c.1).map_or(String::new(), |m| format!("  {}", MEDALS[m]));
                    if open[n - 1] { format!("CHALLENGE {n}{medal}") } else { "LOCKED CHALLENGE".to_string() }
                })
                .collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let list = MenuList::new(&refs, 220.0).with_row(26.0).select(fe.challenge - 1);
            kids.push(menu(&mut commands, &ui, list, 40.0, 70.0));
            let n = fe.challenge;
            let x = 290.0;
            let mut top = 76.0;
            if let Some(tex) = ui.challenges.get(&n) {
                let w = 150.0;
                let h = w * tex.size.y / tex.size.x.max(1.0);
                kids.push(commands.spawn(placed(tex.node(w, h), x, top)).id());
                top += h + 8.0;
            }
            kids.push(commands.spawn((BitmapText::new(FontId::Menu, 14.0, format!("CHALLENGE {n}")), ui_at(x, top))).id());
            top += 22.0;
            let def = defs.get(n).cloned().unwrap_or_default();
            let mut lines: Vec<(String, Color)> = Vec::new();
            let plain = Color::srgb(0.85, 0.9, 1.0);
            if !open[n - 1] {
                lines.push(("TO UNLOCK THIS CHALLENGE YOU'LL NEED TO EARN MEDALS".into(), plain));
                lines.push(("IN THE EARLIER ONES.".into(), plain));
            } else if def.gates == 0 {
                lines.push(("THIS CHALLENGE'S COURSE IS NOT ON THE DISC".into(), plain));
            }
            if open[n - 1] {
                lines.extend(challenge::wrap(&def.text.to_uppercase(), 48).into_iter().map(|l| (l, plain)));
                lines.push((String::new(), plain));
                for (l, won) in challenge::summary(&def, career.challenges.get(&n).copied()) {
                    lines.push((l, if won { Color::srgb(1.0, 0.85, 0.3) } else { Color::WHITE }));
                }
            }
            for (l, c) in lines {
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 8.0, l).color(c), ui_at(x, top))).id());
                top += 15.0;
            }
        }
        Screen::MpChamp => {
            let names = Champ::ALL.map(Champ::label);
            let list = MenuList::new(&names, 320.0).with_row(36.0);
            kids.push(menu(&mut commands, &ui, list, (SCREEN_W - 320.0) / 2.0, 150.0));
            kids.push(hint(&mut commands, &ui, "FIVE ROUNDS AGAINST EACH OTHER AND THE CHAMPIONSHIP'S RIDERS", 330.0));
        }
        Screen::MpChampStatus => {
            if let Some(run) = &mp_champ.run {
                let head = if run.over() {
                    let leader = run.standings().first().map_or(String::new(), |e| e.name.clone());
                    format!("{} CHAMPIONSHIP  -  {leader} WINS", run.champ.label())
                } else {
                    let track = rules.tracks[run.champ.index()].get(run.round).map_or(String::new(), |t| track_title(t));
                    format!("ROUND {} OF {}  -  {}", run.round + 1, career::ROUNDS, track)
                };
                kids.push(commands.spawn((BitmapText::new(FontId::Menu, 14.0, head), ui_at(40.0, 80.0))).id());
                kids.push(standings_table(&mut commands, run, 40.0, 116.0));
                if let Some(tex) = rules.tracks[run.champ.index()].get(run.round).and_then(|t| ui.tracks.get(&t.to_ascii_lowercase())) {
                    kids.push(commands.spawn(placed(tex.node(220.0, 220.0), 380.0, 116.0)).id());
                }
            }
        }
        Screen::CareerStatus | Screen::CareerComplete => {
            if let Some(run) = &career.run {
                let complete = fe.screen == Screen::CareerComplete || run.over();
                let head = if complete {
                    format!("{} CHAMPIONSHIP  -  YOU FINISHED {}", run.champ.label(), ordinal(run.player_place()))
                } else {
                    let track = rules.tracks[run.champ.index()].get(run.round).map_or(String::new(), |t| track_title(t));
                    format!("ROUND {} OF {}  -  {}", run.round + 1, career::ROUNDS, track)
                };
                kids.push(commands.spawn((BitmapText::new(FontId::Menu, 14.0, head), ui_at(40.0, 80.0))).id());
                kids.push(standings_table(&mut commands, run, 40.0, 116.0));
                if complete {
                    let next = Champ::ALL.get(run.champ.index() + 1).filter(|c| career.is_open(**c));
                    let msg = match next {
                        Some(c) if run.player_place() <= 3 => format!("{} CHAMPIONSHIP OPEN", c.label()),
                        _ if run.player_place() > 3 => "FINISH IN THE TOP 3 TO OPEN THE NEXT CHAMPIONSHIP".to_string(),
                        _ => "ALL CHAMPIONSHIPS OPEN".to_string(),
                    };
                    kids.push(hint(&mut commands, &ui, &msg, 380.0));
                } else if let Some(tex) = rules.tracks[run.champ.index()].get(run.round).and_then(|t| ui.tracks.get(&t.to_ascii_lowercase())) {
                    kids.push(commands.spawn(placed(tex.node(220.0, 220.0), 380.0, 116.0)).id());
                }
            }
        }
    }

    // Button hints along the bottom.
    let hints: &[(&str, &str)] = match fe.screen {
        Screen::Title => &[],
        Screen::Main => &[("controlbuttoncross", "SELECT")],
        Screen::Audio | Screen::Camera => &[("controlbuttoncross", "OK")],
        Screen::CareerChamp => &[("controlbuttoncross", "SELECT"), ("controlbuttoncircle", "BACK")],
        Screen::Controller => &[("controlbuttoncross", "CHANGE"), ("controlbuttoncircle", "BACK")],
        Screen::CareerStatus => &[("controlbuttoncross", "RACE"), ("controlbuttoncircle", "BACK")],
        Screen::MpChampStatus if mp_champ.run.as_ref().is_some_and(|r| r.over()) => &[("controlbuttoncross", "OK")],
        Screen::MpChampStatus => &[("controlbuttoncross", "RACE"), ("controlbuttoncircle", "BACK")],
        Screen::CareerComplete => &[("controlbuttoncross", "OK")],
        _ => &[("controlbuttoncross", "SELECT"), ("controlbuttoncircle", "BACK")],
    };
    let footer = commands
        .spawn(Node { position_type: PositionType::Absolute, right: px(40), top: px(436), column_gap: px(18), align_items: AlignItems::Center, ..default() })
        .with_children(|f| {
            for (button, label) in hints {
                f.spawn(Node { align_items: AlignItems::Center, column_gap: px(5), ..default() }).with_children(|h| {
                    let s = ui.pad.get(button).rect;
                    h.spawn(ui.pad.tex.region(s, 22.0 / s.height()));
                    h.spawn(BitmapText::new(FontId::Small, 8.0, *label));
                });
            }
        })
        .id();
    kids.push(footer);
    commands.entity(frame).add_children(&kids);
}

fn ui_at(left: f32, top: f32) -> Node {
    Node { position_type: PositionType::Absolute, left: px(left), top: px(top), ..default() }
}

fn menu(commands: &mut Commands, ui: &UiAssets, list: MenuList, left: f32, top: f32) -> Entity {
    let e = commands.spawn_empty().id();
    spawn_menu_rows(commands, ui, e, &list);
    commands.entity(e).insert((list, Node { position_type: PositionType::Absolute, left: px(left), top: px(top), flex_direction: FlexDirection::Column, ..default() }));
    e
}

/// A volume bar: the unlit bar with the lit one over it, cut to the level.
fn volume_bar(commands: &mut Commands, ui: &UiAssets, which: usize, left: f32, top: f32) -> Entity {
    let off = ui.sound_bar.get("BAR_OFF").rect;
    commands
        .spawn(Node { position_type: PositionType::Absolute, left: px(left), top: px(top), width: px(off.width()), height: px(off.height()), ..default() })
        .with_children(|b| {
            b.spawn(placed(ui.sound_bar.node("BAR_OFF", 1.0), 0.0, 0.0));
            b.spawn((Node { position_type: PositionType::Absolute, width: px(0), height: percent(100), overflow: Overflow::clip(), ..default() }, VolumeBar(which)))
                .with_children(|clip| {
                    clip.spawn(placed(ui.sound_bar.node("BAR_ON", 1.0), 0.0, 0.0));
                });
        })
        .id()
}

/// Width of a volume bar's lit part for a level, in texels of its 128-wide
/// sprite: the 16 segments run from x = 3 to 124.
fn lit_width(level: u8) -> f32 {
    if level == 0 { 0.0 } else { 3.0 + 121.0 * f32::from(level.min(VOLUME_STEPS)) / f32::from(VOLUME_STEPS) }
}

/// The championship table: place, rider and points, the player in gold.
fn standings_table(commands: &mut Commands, run: &Run, left: f32, top: f32) -> Entity {
    commands
        .spawn(Node { position_type: PositionType::Absolute, left: px(left), top: px(top), flex_direction: FlexDirection::Column, row_gap: px(10), ..default() })
        .with_children(|t| {
            for (i, e) in [None].into_iter().chain(run.standings().into_iter().map(Some)).enumerate() {
                let color = match e {
                    None => Color::srgb(0.75, 0.82, 0.95),
                    Some(e) if e.player => Color::srgb(1.0, 0.85, 0.3),
                    Some(_) => Color::WHITE,
                };
                let cells = match e {
                    None => ["".to_string(), "RIDER".to_string(), "POINTS".to_string()],
                    Some(e) => [ordinal(i), e.name.clone(), e.points.to_string()],
                };
                t.spawn(Node { column_gap: px(10), ..default() }).with_children(|r| {
                    for (text, width) in cells.into_iter().zip([44.0, 200.0, 70.0]) {
                        r.spawn(Node { width: px(width), ..default() }).with_children(|c| {
                            c.spawn(BitmapText::new(FontId::Small, 9.0, text).color(color));
                        });
                    }
                });
            }
        })
        .id()
}

fn hint(commands: &mut Commands, ui: &UiAssets, text: &str, top: f32) -> Entity {
    let w = ui.font(FontId::Small).width(text, 8.0);
    commands.spawn((BitmapText::new(FontId::Small, 8.0, text).color(Color::srgb(0.75, 0.82, 0.95)), ui_at((SCREEN_W - w) / 2.0, top))).id()
}

/// The controls, pad and keyboard, beside the front of the big pad.
fn controls_table(commands: &mut Commands, ui: &UiAssets) -> Entity {
    const ROWS: [(&str, &str, &str); 10] = [
        ("ACCELERATE", "RT OR A", "W OR UP"),
        ("BRAKE AND REVERSE", "LT OR X", "S OR DOWN"),
        ("STEER", "LEFT STICK", "A D OR LEFT RIGHT"),
        ("LEAN", "LEFT STICK", "I K"),
        ("PRE-LOAD AND HOP", "B OR RB", "SPACE"),
        ("TRICKS IN THE AIR", "X Y LB RB", "1 2 3 4"),
        ("CHANGE CAMERA", "BACK", "C"),
        ("PAUSE", "START", "ESCAPE"),
        ("BACK ON TRACK", "", "R"),
        ("RESTART", "", "BACKSPACE"),
    ];
    let grey = Color::srgb(0.75, 0.82, 0.95);
    commands
        .spawn(Node { position_type: PositionType::Absolute, left: px(24), top: px(140), column_gap: px(14), align_items: AlignItems::Center, ..default() })
        .with_children(|t| {
            // The front of the pad is the bottom of the page.
            let front = Rect::new(20.0, 280.0, 340.0, 512.0);
            let scale = ui.big_pad.size.x / 512.0;
            t.spawn(ui.big_pad.region(Rect::from_corners(front.min * scale, front.max * scale), 0.62 / scale));
            t.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(9), ..default() }).with_children(|c| {
                for (i, (action, pad, keys)) in [("", "PAD", "KEYBOARD")].into_iter().chain(ROWS).enumerate() {
                    c.spawn(Node { column_gap: px(8), ..default() }).with_children(|r| {
                        for (text, width, color) in [(action, 128.0, Color::WHITE), (pad, 88.0, grey), (keys, 116.0, grey)] {
                            let color = if i == 0 { Color::srgb(1.0, 0.85, 0.3) } else { color };
                            r.spawn(Node { width: px(width), ..default() }).with_children(|cell| {
                                cell.spawn(BitmapText::new(FontId::Small, 7.0, text).color(color));
                            });
                        }
                    });
                }
            });
        })
        .id()
}

fn update_options(
    fe: Res<FrontEnd>,
    settings: Res<Settings>,
    ui: Option<Res<UiAssets>>,
    mut bars: Query<(&VolumeBar, &mut Node)>,
    mut boxes: Query<&mut ImageNode, With<VibrationBox>>,
) {
    let Some(ui) = ui else { return };
    if !matches!(fe.screen, Screen::Audio | Screen::Controller) {
        return;
    }
    for (b, mut n) in &mut bars {
        let w = px(lit_width(if b.0 == 0 { settings.music } else { settings.effects }));
        if n.width != w {
            n.width = w;
        }
    }
    for mut img in &mut boxes {
        let rect = ui.checkbox.tex.image_rect(ui.checkbox.get(if settings.vibration { "ON" } else { "OFF" }).rect);
        if img.rect != Some(rect) {
            img.rect = Some(rect);
        }
    }
}

fn mug_pos(i: usize) -> (f32, f32) {
    (MUG_X + (i % 8) as f32 * MUG_GAP, MUG_Y + (i / 8) as f32 * MUG_GAP)
}

/// The blue bio panel: a name, two detail lines (riders) and 1-5 rating
/// bars made of the attribute-bar sprites.
fn bio_panel(commands: &mut Commands, ui: &UiAssets, left: f32, top: f32, stats: &[&str], rider: bool) -> Entity {
    let panel = ui.bio.get("BIOPANEL").rect;
    let (w, h) = (if rider { 512.0 } else { 330.0 }, if rider { 128.0 } else { 160.0 });
    let blank = ui.design.get("ATTRIBBAR_BLANK").rect;
    let bar_w = 110.0;
    let bar_s = bar_w / blank.width();
    commands
        .spawn(Node { position_type: PositionType::Absolute, left: px(left), top: px(top), width: px(w), height: px(h), padding: UiRect::all(px(12)), column_gap: px(16), ..default() })
        .with_children(|p| {
            let (mut n, img) = ui.bio.tex.region(panel, 1.0);
            n.position_type = PositionType::Absolute;
            n.left = px(0);
            n.top = px(0);
            n.width = percent(100);
            n.height = percent(100);
            p.spawn((n, img, ZIndex(-1)));
            if rider {
                p.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(8), width: px(220), ..default() }).with_children(|c| {
                    c.spawn((BitmapText::new(FontId::Menu, 14.0, ""), Field::RiderName));
                    c.spawn((BitmapText::new(FontId::Small, 8.0, "").color(Color::srgb(0.8, 0.9, 1.0)), Field::Born));
                    c.spawn((BitmapText::new(FontId::Small, 8.0, "").color(Color::srgb(0.8, 0.9, 1.0)), Field::From));
                });
            }
            p.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), ..default() }).with_children(|c| {
                if !rider {
                    c.spawn((BitmapText::new(FontId::Menu, 14.0, ""), Field::QuadName, Node { margin: UiRect::bottom(px(4)), ..default() }));
                }
                for (i, label) in stats.iter().enumerate() {
                    c.spawn(Node { align_items: AlignItems::Center, column_gap: px(8), ..default() }).with_children(|r| {
                        r.spawn(Node { width: px(96), ..default() }).with_children(|l| {
                            l.spawn(BitmapText::new(FontId::Small, 7.0, *label));
                        });
                        r.spawn(Node { width: px(bar_w), height: px(blank.height() * bar_s), ..default() }).with_children(|b| {
                            b.spawn(placed(ui.design.tex.region(blank, bar_s), 0.0, 0.0));
                            // The lit bar is clipped to the rating.
                            b.spawn((
                                Node { position_type: PositionType::Absolute, width: px(0), height: percent(100), overflow: Overflow::clip(), ..default() },
                                StatBar(i),
                            ))
                            .with_children(|clip| {
                                clip.spawn(placed(ui.design.node("ATTRIBBAR_FULL", bar_s), 0.0, 0.0));
                            });
                        });
                    });
                }
            });
        })
        .id()
}

fn update_rider_panel(
    fe: Res<FrontEnd>,
    roster: Res<Roster>,
    mut cursor: Query<&mut Node, (With<MugCursor>, Without<StatBar>)>,
    mut fields: Query<(&Field, &mut BitmapText)>,
    mut bars: Query<(&StatBar, &mut Node), Without<MugCursor>>,
) {
    if fe.screen != Screen::Rider {
        return;
    }
    let Some(r) = roster.riders.get(fe.rider) else { return };
    if let Ok(mut n) = cursor.single_mut() {
        let (x, y) = mug_pos(fe.rider);
        n.left = px(x - 11.0);
        n.top = px(y - 11.0);
    }
    for (f, mut t) in &mut fields {
        match f {
            Field::RiderName => set_text(&mut t, &r.name),
            Field::Born => set_text(&mut t, &format!("BORN  {}", r.born)),
            Field::From => set_text(&mut t, &format!("FROM  {}", r.from)),
            _ => {}
        }
    }
    for (b, mut n) in &mut bars {
        n.width = percent(r.stats.get(b.0).copied().unwrap_or(0.0) / 5.0 * 100.0);
    }
}

fn update_quad_panel(fe: Res<FrontEnd>, roster: Res<Roster>, mut fields: Query<(&Field, &mut BitmapText)>, mut bars: Query<(&StatBar, &mut Node)>) {
    if fe.screen != Screen::Quad {
        return;
    }
    let Some(q) = roster.quads.get(fe.quad) else { return };
    for (f, mut t) in &mut fields {
        if let Field::QuadName = f {
            set_text(&mut t, &q.name.to_uppercase());
        }
    }
    for (b, mut n) in &mut bars {
        n.width = percent(q.stats.get(b.0).copied().unwrap_or(0.0) / 5.0 * 100.0);
    }
}

fn update_track_panel(
    fe: Res<FrontEnd>,
    roster: Res<Roster>,
    sel: Res<Selection>,
    ui: Option<Res<UiAssets>>,
    mut fields: Query<(&Field, &mut BitmapText)>,
    mut preview: Query<&mut ImageNode, With<TrackPreview>>,
) {
    let Some(ui) = ui else { return };
    if fe.screen != Screen::Track {
        return;
    }
    let Some(t) = roster.tracks.get(fe.track) else { return };
    for (f, mut text) in &mut fields {
        match f {
            Field::TrackName => set_text(&mut text, &track_title(t)),
            Field::Laps if fe.mode == Mode::Freestyle => set_text(&mut text, &limit_label(TIME_LIMITS[fe.limit])),
            Field::Laps => set_text(&mut text, &sel.laps.to_string()),
            Field::Targets => {
                let s = match roster.freestyle_medals.get(fe.limit).copied().flatten() {
                    Some(m) => (0..3).rev().map(|i| format!("{} {}", MEDALS[i], m[i])).collect::<Vec<_>>().join("   "),
                    None => String::new(),
                };
                set_text(&mut text, &s);
            }
            _ => {}
        }
    }
    if let (Ok(mut img), Some(tex)) = (preview.single_mut(), ui.tracks.get(&t.to_ascii_lowercase()))
        && img.image != tex.image
    {
        img.image = tex.image.clone();
    }
}

fn blink(time: Res<Time>, mut q: Query<&mut Visibility, With<Blink>>) {
    for mut v in &mut q {
        v.set_if_neq(if (time.elapsed_secs() * 1.5).fract() < 0.65 { Visibility::Inherited } else { Visibility::Hidden });
    }
}

#[derive(Component)]
struct LoadingScreen;

/// The loading screen is drawn for a couple of frames before the race
/// loads, since loading blocks.
fn show_loading(mut commands: Commands, ui: Option<Res<UiAssets>>, sel: Res<Selection>) {
    let Some(ui) = ui else { return };
    commands
        .spawn((fullscreen(), BackgroundColor(Color::BLACK), DespawnOnExit(AppState::Loading), LoadingScreen))
        .with_children(|r| {
            r.spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                margin: UiRect::left(px(-SCREEN_W / 2.0)),
                width: px(SCREEN_W),
                height: px(SCREEN_H),
                ..default()
            })
            .with_children(|f| {
                let (mut n, img) = ui.loading.node(SCREEN_W, SCREEN_H);
                n.position_type = PositionType::Absolute;
                f.spawn((n, img));
                let t = match sel.mode {
                    Mode::Arcade | Mode::Career => format!("ROUND {}  LOADING {}", sel.round + 1, track_title(&sel.track)),
                    Mode::Challenge => format!("CHALLENGE {}  LOADING", sel.challenge),
                    _ => format!("LOADING {}", track_title(&sel.track)),
                };
                let w = ui.font(FontId::Menu).width(&t, 14.0);
                f.spawn((BitmapText::new(FontId::Menu, 14.0, t), ui_at(SCREEN_W - 32.0 - w, 440.0)));
            });
        });
}

fn finish_loading(mut frames: Local<u32>, mut next: ResMut<NextState<AppState>>) {
    *frames += 1;
    if *frames >= 3 {
        *frames = 0;
        next.set(AppState::Race);
    }
}

/// The ATV select screen's turning 3D quad, drawn by its own camera over
/// the menu in the empty top-right of the screen.
#[derive(Component)]
struct Showroom {
    quad: usize,
}
#[derive(Component)]
struct Turntable;
#[derive(Component)]
struct ShowroomCamera;

/// Screen-unit area of the 640x480 frame the quad is shown in.
const SHOWROOM: Rect = Rect { min: Vec2::new(270.0, 64.0), max: Vec2::new(620.0, 244.0) };

#[allow(clippy::too_many_arguments)]
fn showroom(
    mut commands: Commands,
    fe: Res<FrontEnd>,
    roster: Res<Roster>,
    game: Res<crate::Game>,
    shown: Query<(Entity, &Showroom)>,
    mut cams: Query<&mut Camera, With<ShowroomCamera>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let want = (fe.screen == Screen::Quad).then_some(fe.quad);
    let current = shown.iter().next();
    if current.map(|(_, s)| s.quad) != want {
        for (e, _) in &shown {
            commands.entity(e).despawn();
        }
        if let Some(i) = want
            && let Some(q) = roster.quads.get(i)
        {
            let root = commands
                .spawn((Transform::default(), Visibility::default(), Showroom { quad: i }, DespawnOnExit(AppState::FrontEnd)))
                .with_children(|r| {
                    r.spawn((
                        Camera3d::default(),
                        Camera { order: 1, clear_color: ClearColorConfig::None, ..default() },
                        bevy::core_pipeline::tonemapping::Tonemapping::None,
                        Transform::from_xyz(0.0, 0.85, 2.7).looking_at(Vec3::new(0.0, 0.25, 0.0), Vec3::Y),
                        ShowroomCamera,
                    ));
                    r.spawn((
                        DirectionalLight { illuminance: 7000.0, ..default() },
                        Transform::default().looking_to(Vec3::new(-0.4, -0.8, -0.5), Vec3::Y),
                    ));
                })
                .id();
            let generic = game.0.read("/Cars/Generic/Generic.params").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let own = game.0.read(&format!("/Cars/{0}/{0}.params", q.name)).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
            let params = atv_sim::VehicleParams::from_files(&generic, own.as_deref());
            let wheel_x: Vec<f32> = params.wheels.iter().map(|w| w.offset[0]).collect();
            let mut loader = crate::convert::Loader::new(&game.0, &mut meshes, &mut materials, &mut images);
            match loader.spawn_quad(&mut commands, &q.name, Transform::from_xyz(0.0, crate::convert::COM_HEIGHT, 0.0), &wheel_x) {
                Ok(quad) => {
                    let table = commands.spawn((Transform::from_rotation(Quat::from_rotation_y(-0.6)), Visibility::default(), Turntable)).add_child(quad).id();
                    commands.entity(root).add_child(table);
                }
                Err(e) => error!("failed to load quad {}: {e:#}", q.name),
            }
        }
    }
    // Keep the camera's viewport on the showroom area as the window changes.
    let Ok(w) = windows.single() else { return };
    let scale = w.height() / SCREEN_H * w.scale_factor();
    let left = (w.physical_width() as f32 - SCREEN_W * scale) / 2.0;
    for mut cam in &mut cams {
        let pos = Vec2::new(left + SHOWROOM.min.x * scale, SHOWROOM.min.y * scale).max(Vec2::ZERO);
        let size = (SHOWROOM.size() * scale).min(Vec2::new(w.physical_width() as f32, w.physical_height() as f32) - pos);
        if size.x >= 1.0 && size.y >= 1.0 {
            cam.viewport = Some(bevy::camera::Viewport { physical_position: pos.as_uvec2(), physical_size: size.as_uvec2(), ..default() });
        }
    }
}

fn turntable(time: Res<Time>, mut q: Query<&mut Transform, With<Turntable>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.6);
    }
}
