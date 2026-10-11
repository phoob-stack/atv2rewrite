//! The front end: title, main menu, then rider, ATV and track select into a
//! race, using the original backgrounds, fonts and panels. Which background
//! and title each screen uses comes from the disc's `Setup/Frontend.xml`.
//! Arcade skips track select (it races `Setup/Arcade.xml`'s rounds);
//! Open Freestyle picks a time limit on it instead of laps. Options holds
//! the Audio, Camera and Controller pages (see [`crate::settings`]).
//! Career picks a championship, then rider and ATV, and comes back to the
//! championship's status page between rounds (see [`crate::career`]); the
//! Custom championship picks its tracks, laps and points split first.
//! Multiplayer picks how many play, then each player picks a rider and ATV
//! in turn before track select (see [`crate::mp`]). Y (or T) on rider
//! select, track select and the career status opens the Trick Book (see
//! [`crate::trickbook`]).
//!
//! Player profiles are the original's three pages: Available Profiles
//! (load one of ten, or make one), Choose A Rider (the riders open from the
//! start, each in one profile at most) and Name Your Rider (an on-screen
//! keyboard, or typing; a cheat's name enters the cheat, see
//! [`crate::unlocks`]). Pressing start without a profile loaded opens them,
//! as does Options. Career and Arcade ride the profile's rider: the
//! original has no rider select for them.

use bevy::input::gamepad::Gamepad;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;

use atv_formats::setup::{self, Entry};
use atv_formats::GameSource;

use crate::arcade::{self, ArcadeField};
use crate::freestyle::{self, limit_label, TrickDef, MEDALS, TIME_LIMITS};
use crate::career::{self, Career, Champ, CustomSetup, Plan, Profile, Run, PROFILES};
use crate::challenge;
use crate::hud::{ordinal, track_title};
use crate::settings::{CameraView, Settings, VOLUME_STEPS};
use crate::ui::{fullscreen, placed, read_nav_from, set_text, spawn_menu_rows, BitmapText, FontId, MenuList, Nav, UiAssets, SCREEN_H, SCREEN_W};
use crate::mp::MpChamp;
use crate::records::{self, TrackRecords};
use crate::trickbook::{self, TrickDemo};
use crate::unlocks;
use crate::{AppState, Mode, Selection};

pub struct FrontEndPlugin;

impl Plugin for FrontEndPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrontEnd>()
            .add_systems(OnEnter(AppState::FrontEnd), enter)
            .add_systems(
                Update,
                (
                    navigate,
                    rebuild,
                    update_rider_panel,
                    update_quad_panel,
                    update_track_panel,
                    update_record_panels,
                    update_options,
                    blink,
                    showroom,
                    demo_clip,
                    trickbook::demo,
                    turntable,
                )
                    .chain()
                    .run_if(in_state(AppState::FrontEnd)),
            )
            .add_systems(Update, finish_loading.run_if(in_state(AppState::Loading)));
    }
}

/// Riders, quads and tracks as the disc's setup files list them.
#[derive(Resource, Clone, Default)]
pub struct Roster {
    pub riders: Vec<RiderInfo>,
    pub quads: Vec<QuadInfo>,
    pub tracks: Vec<String>,
    /// Tracks open from the start (`mAvailable="1"` in `Tracks.xml`).
    pub open_tracks: Vec<String>,
    /// Tracks the attract demo races on (`mAvailableForDemo="1"`).
    pub demo_tracks: Vec<String>,
    /// Arcade's rounds, in order (the ones on this disc, up to the
    /// original's fifteen).
    pub arcade: Vec<String>,
    /// Open Freestyle's Bronze/Silver/Gold scores per [`TIME_LIMITS`] entry.
    pub freestyle_medals: Vec<Option<[u32; 3]>>,
    /// `Tricks.xml`, and its order in the Trick Book.
    pub tricks: Vec<TrickDef>,
    book: Vec<usize>,
    info: Vec<(String, TrackInfo)>,
    screens: Vec<Entry>,
}

/// Per-track rules from `Tracks.xml`.
#[derive(Clone, Copy, Debug)]
pub struct TrackInfo {
    /// Arcade's starting clock, seconds.
    pub arcade_start: f32,
    /// Sand, snow or ordinary ground (`mTyreType`): tyres and dirt.
    pub ground: crate::convert::Ground,
    /// Whether riders off the course are put back (`mAllowsForcedRespots`:
    /// all but the Stadium).
    pub forced_respots: bool,
}

impl Default for TrackInfo {
    fn default() -> Self {
        Self { arcade_start: 40.0, ground: crate::convert::Ground::Road, forced_respots: false }
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
    /// Locked for the player until unlocked (`mAvailability` `Locked...`):
    /// the pros and the secret riders.
    pub locked: bool,
    /// Skill level (1 to 7, `mSkillRating`): the tricks this rider pulls
    /// when another player rides them.
    pub skill: u8,
}

#[derive(Clone, Debug)]
pub struct QuadInfo {
    pub name: String,
    /// Top speed, acceleration, stability, handling, air rating (1-5).
    pub stats: [f32; 5],
    /// Locked for the player until unlocked (`mAvailability` other than
    /// `Unlocked`).
    pub locked: bool,
    /// Whether computer riders may ride it (not `LockedBoth`).
    pub ai: bool,
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
                    locked: e.get("mAvailability").is_some_and(|a| a.starts_with("Locked")),
                    skill: e.num("mSkillRating").map_or(1, |l| l.clamp(1.0, freestyle::MAX_LEVEL as f32) as u8),
                })
            })
            .collect();
        let quads = setup::entries(&read("/Setup/VehicleSetup.xml"))
            .into_iter()
            .filter(|e| game.exists(&format!("/Cars/{0}/{0}.params", e.name)))
            .map(|e| QuadInfo {
                stats: ["mTopSpeed", "mAcceleration", "mStability", "mHandling", "mAirRating"].map(|k| e.num(k).unwrap_or(0.0)),
                locked: e.get("mAvailability").is_some_and(|a| a != "Unlocked"),
                ai: e.get("mAvailability") != Some("LockedBoth"),
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
                    ground: crate::convert::Ground::from_tyre_type(e.get("mTyreType").unwrap_or("")),
                    forced_respots: e.get("mAllowsForcedRespots") == Some("1"),
                })
            })
            .collect();
        let open_tracks = track_entries.iter().filter(|e| e.get("mAvailable") == Some("1")).map(|e| e.name.clone()).collect();
        let demo_tracks = track_entries.iter().filter(|e| e.get("mAvailableForDemo") == Some("1")).map(|e| e.name.clone()).collect();
        let tracks: Vec<String> = track_entries.into_iter().map(|e| e.name).collect();
        let arcade = setup::entries(&read("/Setup/Arcade.xml"))
            .into_iter()
            .filter_map(|e| e.get("TrackName").map(str::to_owned))
            .filter(|t| tracks.iter().any(|k| k.eq_ignore_ascii_case(t)))
            .take(arcade::ROUNDS)
            .collect();
        let freestyle_medals = TIME_LIMITS.iter().map(|&l| freestyle::medal_targets(game, l)).collect();
        let tricks = freestyle::load_tricks(game).0;
        let book = trickbook::book(&tricks);
        Self { riders, quads, tracks, open_tracks, demo_tracks, arcade, freestyle_medals, tricks, book, info, screens: setup::entries(&read("/Setup/Frontend.xml")) }
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
    /// The split-screen championship's standings (its tracks are picked
    /// on the Custom pages).
    MpChampStatus,
    /// Every trick, a page each.
    TrickBook,
    /// Arcade with a saved round: carry on from it or start again.
    ArcadeNew,
    /// Arcade's grid and who went out before each round, and the field at
    /// the end.
    ArcadeStatus,
    ArcadeComplete,
    /// The Custom championship's tracks and laps, then its points split.
    CustomTracks,
    CustomPoints,
    /// The ten player profiles: load one, or pick an empty slot to make one.
    ProfileSelect,
    /// A new profile's rider, then its name.
    ProfileRider,
    ProfileName,
}

/// The Options menu's rows, and the page each opens.
const OPTIONS: [(&str, Option<Screen>); 6] = [
    ("AUDIO", Some(Screen::Audio)),
    ("CAMERA", Some(Screen::Camera)),
    ("CONTROLLER", Some(Screen::Controller)),
    ("REPLAYS", None),
    ("PROFILES", Some(Screen::ProfileSelect)),
    ("CREDITS", None),
];

/// Name Your Rider's keys (0x51390): three rows of characters, then
/// delete, space and OK.
const KEY_ROWS: [&str; 3] = ["ABCDEFGHIJKLMN", "OPQRSTUVWXYZ", "1234567890-\"*"];
const KEY_WORDS: [&str; 3] = ["DEL", "SPACE", "OK"];
/// The longest name the original takes.
const NAME_LEN: usize = 12;
const NO_PROFILES: &str = "THERE HAVE BEEN NO PLAYER PROFILES LOADED, YOU'LL HAVE TO CREATE ONE NOW.";
const ENTER_NAME: &str = "PLEASE ENTER YOUR NAME";

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
    /// The championship being set up, and a Custom one's tracks and points.
    champ: Champ,
    custom: CustomSetup,
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
    /// The Trick Book's page, and the screen it was opened from.
    trick: usize,
    book_from: Screen,
    /// Profile Select's slot (where a new profile goes), the screen it was
    /// opened from, and what each slot holds.
    slot: usize,
    profile_from: Screen,
    profiles: Vec<Option<Career>>,
    /// Name Your Rider: the new profile's rider, its name so far and the
    /// key highlighted (row, column).
    new_rider: String,
    name: String,
    key: (usize, usize),
    /// Cheats entered as a name, for the next profile made or loaded.
    cheats: u32,
    /// A message box over the screen (title, text) until it's closed.
    popup: Option<(String, String)>,
}

impl FrontEnd {
    pub fn at(screen: Screen) -> Self {
        Self { screen, ..default() }
    }

    /// Press Start is up, with no message box over it.
    pub fn waiting_on_title(&self) -> bool {
        self.screen == Screen::Title && self.built == Some(Screen::Title) && self.popup.is_none()
    }
}

#[derive(Component)]
struct ScreenRoot;
#[derive(Component)]
pub(crate) struct Blink;
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
    /// Why the highlighted rider, ATV or track is locked.
    Locked,
}
#[derive(Component)]
struct StatBar(usize);
#[derive(Component)]
struct TrackPreview;
/// A line of the track select's record panel `.0`.
#[derive(Component)]
struct RecordText(usize, RecordLine);
#[derive(Clone, Copy, PartialEq)]
enum RecordLine {
    Kind,
    Value,
    Name,
}
/// The holder's mug on record panel `.0`.
#[derive(Component)]
struct RecordMug(usize);
/// The lit part of a volume bar: 0 music, 1 sound effects.
#[derive(Component)]
struct VolumeBar(usize);
#[derive(Component)]
struct VibrationBox;

const MUG: f32 = 50.0;
/// The track select's three record panels (0x519d0, 0x51920), right of
/// the track's picture: one every `RECORD_GAP` down.
const RECORD_X: f32 = 452.0;
const RECORD_Y: f32 = 76.0;
const RECORD_W: f32 = 180.0;
const RECORD_H: f32 = 72.0;
const RECORD_GAP: f32 = 80.0;
const RECORD_MUG: f32 = 48.0;
const RECORD_PAD: f32 = 8.0;
/// Room for a panel's text, right of the mug.
const RECORD_TEXT_W: f32 = RECORD_W - 3.0 * RECORD_PAD - RECORD_MUG;
/// The panels' colour (0x4d360).
const RECORD_BG: Color = Color::srgb(0.15, 0.27, 0.27);
const MUG_GAP: f32 = 64.0;
const MUG_X: f32 = 64.0;
const MUG_Y: f32 = 92.0;
/// Choose A Rider's twelve, six to a row.
const PROFILE_MUGS_ROW: usize = 6;
const PROFILE_MUG_X: f32 = (SCREEN_W - (PROFILE_MUGS_ROW - 1) as f32 * MUG_GAP - MUG) / 2.0;
const PROFILE_MUG_Y: f32 = 110.0;

fn enter(mut fe: ResMut<FrontEnd>, sel: Res<Selection>, roster: Res<Roster>, career: Res<Career>, defs: Res<challenge::Defs>) {
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
    if fe.screen == Screen::ArcadeStatus {
        fe.arcade_round = career.arcade;
    }
    fe.built = None;
    fe.mode = sel.mode;
    fe.limit = TIME_LIMITS.iter().position(|&l| l == sel.time_limit).unwrap_or(1);
    fe.rider = roster.riders.iter().position(|r| r.model.eq_ignore_ascii_case(&sel.rider)).unwrap_or(fe.rider);
    if roster.riders.get(fe.rider).is_some_and(|r| !unlocks::rider_open(&career, &defs, r)) {
        fe.rider = roster.riders.iter().position(|r| unlocks::rider_open(&career, &defs, r)).unwrap_or(0);
    }
    fe.quad = roster.quads.iter().position(|q| q.name.eq_ignore_ascii_case(&sel.quad)).unwrap_or(fe.quad);
    fe.track = roster.tracks.iter().position(|t| t.eq_ignore_ascii_case(&sel.track)).unwrap_or(fe.track);
    fe.challenge = sel.challenge.clamp(1, challenge::COUNT);
    fe.players = sel.players;
    fe.picking = 0;
    fe.profiles = Career::profiles();
}

/// Whether an earlier player has picked `model` in split screen.
fn taken(fe: &FrontEnd, sel: &Selection, model: &str) -> bool {
    fe.picking > 0 && std::iter::once(&sel.rider).chain(sel.others.iter().take(fe.picking - 1).map(|o| &o.0)).any(|m| m.eq_ignore_ascii_case(model))
}

/// Whether Y opens the Trick Book on this screen (or closes it).
pub fn offers_book(screen: Screen) -> bool {
    matches!(screen, Screen::Rider | Screen::Track | Screen::CareerStatus | Screen::TrickBook)
}

/// The skill level the Trick Book shows tricks against: the profile's,
/// from career and challenge skill points, or while another player picks
/// a rider, that rider's own rating. The second value: whether locked
/// tricks are shown (the profile's) or skipped.
fn book_level(fe: &FrontEnd, roster: &Roster, career: &Career, rules: &career::Rules) -> (u8, bool) {
    match roster.riders.get(fe.rider) {
        Some(r) if fe.picking > 0 && fe.book_from == Screen::Rider => (r.skill, false),
        _ => (rules.skill_level(career.skill_points) as u8, true),
    }
}

/// The rider and ATV the Trick Book shows performing: the one highlighted
/// on rider select, the career's own, or the last picked.
fn book_rider(fe: &FrontEnd, roster: &Roster, sel: &Selection, career: &Career) -> (String, String) {
    let quad = match fe.picking.checked_sub(1).and_then(|i| sel.others.get(i)) {
        Some(o) => o.1.clone(),
        None => sel.quad.clone(),
    };
    match (&career.run, fe.book_from) {
        (_, Screen::Rider) => (roster.riders.get(fe.rider).map_or(sel.rider.clone(), |r| r.model.clone()), quad),
        (Some(r), Screen::CareerStatus) => (r.rider.clone(), r.quad.clone()),
        _ => (sel.rider.clone(), sel.quad.clone()),
    }
}

/// Opens the Trick Book from `fe.screen`, on an unlocked page if locked
/// ones are skipped.
fn open_book(fe: &mut FrontEnd, roster: &Roster, career: &Career, rules: &career::Rules) {
    fe.book_from = fe.screen;
    fe.screen = Screen::TrickBook;
    let (level, all) = book_level(fe, roster, career, rules);
    if !all && roster.book.get(fe.trick).is_none_or(|&i| roster.tricks[i].level > level) {
        fe.trick = roster.book.iter().position(|&i| roster.tricks[i].level <= level).unwrap_or(0);
    }
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
    mut typing: MessageReader<KeyboardInput>,
) {
    // Name Your Rider takes typing (read every frame, so only keys pressed
    // on it count).
    let naming = fe.screen == Screen::ProfileName && fe.built == Some(fe.screen) && fe.popup.is_none();
    let mut typed = false;
    for k in typing.read() {
        if naming && k.state == ButtonState::Pressed {
            typed |= type_key(&mut fe, k);
        }
    }
    if typed {
        fe.built = None;
        return;
    }
    // In split screen each player picks their rider and ATV with their own pad.
    let picking = fe.players > 1 && matches!(fe.screen, Screen::Rider | Screen::Quad);
    let nav = if fe.screen == Screen::ProfileName { name_nav(&keys, &pads) } else { read_nav_from(&keys, &pads, picking.then_some(fe.picking)) };
    let Some(nav) = nav else { return };
    if fe.built != Some(fe.screen) {
        return;
    }
    if fe.popup.is_some() {
        if matches!(nav, Nav::Accept | Nav::Start | Nav::Back) {
            fe.popup = None;
            fe.built = None;
        }
        return;
    }
    let mut list = lists.single_mut().ok();
    match fe.screen {
        Screen::Title => {
            if matches!(nav, Nav::Accept | Nav::Start) {
                if career.profile.is_some() {
                    fe.screen = Screen::Main;
                } else {
                    open_profiles(&mut fe, &roster, &career, Screen::Title);
                }
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
                    sel.field.clear();
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
                            match fe.mode {
                                Mode::Arcade if saved => fe.screen = Screen::ArcadeNew,
                                Mode::Arcade => new_arcade(&mut fe, &mut career, &roster, &sel),
                                _ => to_rider(&mut fe, &mut sel, &career),
                            }
                        }
                        Some("CAREER") => {
                            fe.mode = Mode::Career;
                            sel.mode = Mode::Career;
                            fe.screen = Screen::CareerChamp;
                        }
                        // The original's TrainingSM (0x9d6c0): the profile's
                        // rider picks an ATV, then practises on Stadium 2.
                        Some("ATV ACADEMY") => {
                            fe.mode = Mode::Training;
                            sel.mode = Mode::Training;
                            to_rider(&mut fe, &mut sel, &career);
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
            let shown = |i: usize| roster.riders.get(i).is_some_and(|r| unlocks::rider_shown(&career, &defs, r));
            match nav {
                Nav::Left | Nav::Right | Nav::Up | Nav::Down => fe.rider = step_rider(fe.rider, nav, roster.riders.len(), shown),
                Nav::Accept | Nav::Start => {
                    if let Some(r) = roster.riders.get(fe.rider) {
                        if taken(&fe, &sel, &r.model) || !unlocks::rider_open(&career, &defs, r) {
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
                Nav::Back if fe.mode == Mode::Arcade => fe.screen = Screen::ArcadeStatus,
                Nav::Back if fe.mode == Mode::Freestyle => fe.screen = Screen::FreestyleType,
                Nav::Back if fe.mode == Mode::Challenge => fe.screen = Screen::Challenges,
                Nav::Back => fe.screen = Screen::Main,
                Nav::Extra => open_book(&mut fe, &roster, &career, &rules),
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
                if list.as_ref().is_some_and(|l| l.blocked()) {
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
                    if let Some(i) = (0..n).map(|i| (fe.rider + i) % n).find(|&i| !taken(&fe, &sel, &roster.riders[i].model) && unlocks::rider_open(&career, &defs, &roster.riders[i])) {
                        fe.rider = i;
                    }
                    fe.screen = Screen::Rider;
                } else if (fe.mp_champ && fe.players > 1) || (fe.mode == Mode::Career && fe.champ == Champ::Custom) {
                    // Custom and the split-screen championship pick their
                    // tracks; they start out as last set (none at first),
                    // with any tracks opened since.
                    let tracks = custom_tracks(&roster, &rules, &career);
                    if fe.custom.rows.iter().map(|r| &r.track).ne(tracks.iter()) {
                        fe.custom = fe.custom.with_tracks(&tracks);
                    }
                    fe.screen = Screen::CustomTracks;
                } else if fe.mode == Mode::Career {
                    start_career(&mut career, Plan::set(&rules, fe.champ), &roster, &rules, &sel);
                    fe.screen = Screen::CareerStatus;
                } else if fe.mode == Mode::Training {
                    sel.track = crate::TRAINING_TRACK.into();
                    next.set(AppState::Loading);
                } else if fe.mode == Mode::Challenge {
                    sel.track = challenge::TRACK.into();
                    sel.laps = 1;
                    sel.challenge = fe.challenge;
                    next.set(AppState::Loading);
                } else if fe.mode == Mode::Arcade {
                    // Arcade starts on its first round, or the saved one,
                    // with its field (a rider the player now rides swapped
                    // for another).
                    if let Some(t) = roster.arcade.get(fe.arcade_round) {
                        let field = career.arcade_field.get_or_insert_with(|| ArcadeField::new(&roster, &sel.rider, fe.arcade_round));
                        field.without(&roster, &sel.rider, fe.arcade_round);
                        sel.field = field.grid_order();
                        sel.grid = field.player_slot();
                        sel.round = fe.arcade_round;
                        sel.failed = 0;
                        sel.track = t.clone();
                        // One lap, as the original's ArcadeSM sets (0x95e70).
                        sel.laps = 1;
                        next.set(AppState::Loading);
                    }
                } else {
                    fe.screen = Screen::Track;
                }
            }
            Nav::Back if rides_profile(&fe, &career) => {
                fe.screen = match fe.mode {
                    Mode::Career => Screen::CareerChamp,
                    Mode::Training => Screen::Main,
                    _ => Screen::ArcadeStatus,
                };
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
                if list.as_ref().is_some_and(|l| l.blocked()) {
                    return;
                }
                if let Some(t) = roster.tracks.get(fe.track) {
                    sel.track = t.clone();
                }
                next.set(AppState::Loading);
            }
            Nav::Back => fe.screen = Screen::Quad,
            Nav::Extra => open_book(&mut fe, &roster, &career, &rules),
        },
        Screen::Options => match nav {
            Nav::Up | Nav::Down => {
                if let Some(l) = list.as_mut() {
                    l.step(nav);
                    fe.option = l.selected;
                }
            }
            Nav::Accept | Nav::Start => match OPTIONS.get(fe.option) {
                Some((_, Some(Screen::ProfileSelect))) => open_profiles(&mut fe, &roster, &career, Screen::Options),
                Some((_, Some(page))) => fe.screen = *page,
                _ => {}
            },
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
                Nav::Extra => {}
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
                        to_rider(&mut fe, &mut sel, &career);
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
                if list.as_ref().is_some_and(|l| l.selected == 0) {
                    fe.arcade_round = career.arcade;
                    if career.arcade_field.is_none() {
                        career.arcade_field = Some(ArcadeField::new(&roster, &arcade_rider(&career, &sel), career.arcade));
                    }
                    fe.screen = Screen::ArcadeStatus;
                } else {
                    new_arcade(&mut fe, &mut career, &roster, &sel);
                }
            }
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::ArcadeStatus => match nav {
            Nav::Accept | Nav::Start => to_rider(&mut fe, &mut sel, &career),
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::ArcadeComplete => {
            if matches!(nav, Nav::Accept | Nav::Start | Nav::Back) {
                career.arcade_field = None;
                career.save();
                fe.screen = Screen::Main;
            }
        }
        Screen::CareerStatus => match nav {
            Nav::Accept | Nav::Start => {
                let Some(run) = &career.run else { return };
                if run.over() {
                    fe.screen = Screen::CareerComplete;
                    return;
                }
                let Some((track, laps)) = run.next_round() else { return };
                sel.mode = Mode::Career;
                sel.track = track.to_string();
                sel.round = run.round;
                sel.laps = laps;
                sel.rider = run.rider.clone();
                sel.quad = run.quad.clone();
                sel.field = run.opponents();
                next.set(AppState::Loading);
            }
            Nav::Back => fe.screen = Screen::Main,
            Nav::Extra => open_book(&mut fe, &roster, &career, &rules),
            _ => {}
        },
        Screen::TrickBook => match nav {
            Nav::Left | Nav::Right => {
                let n = roster.book.len().max(1);
                let (level, all) = book_level(&fe, &roster, &career, &rules);
                for _ in 0..n {
                    fe.trick = if nav == Nav::Left { (fe.trick + n - 1) % n } else { (fe.trick + 1) % n };
                    if all || roster.book.get(fe.trick).is_some_and(|&i| roster.tricks[i].level <= level) {
                        break;
                    }
                }
                fe.built = None;
            }
            Nav::Accept | Nav::Back | Nav::Start | Nav::Extra => fe.screen = fe.book_from,
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
        Screen::MpChampStatus => match nav {
            Nav::Accept | Nav::Start => {
                let Some(run) = &mp_champ.run else { return };
                if run.over() {
                    mp_champ.run = None;
                    fe.screen = Screen::Main;
                    return;
                }
                let Some((track, laps)) = run.next_round() else { return };
                sel.mode = Mode::SingleRace;
                sel.track = track.to_string();
                sel.round = run.round;
                sel.laps = laps;
                sel.field = run.opponents();
                next.set(AppState::Loading);
            }
            Nav::Back => fe.screen = Screen::Main,
            _ => {}
        },
        Screen::CustomTracks => {
            let cs = &mut fe.custom;
            let on_continue = cs.cursor >= cs.rows.len();
            let ready = cs.count() > 0;
            match nav {
                Nav::Up | Nav::Down if cs.editing => cs.step_laps(nav == Nav::Up),
                Nav::Up | Nav::Down => {
                    if let Some(l) = list.as_mut() {
                        l.step(nav);
                        cs.cursor = l.selected;
                    }
                }
                Nav::Right if !on_continue && cs.rows[cs.cursor].round.is_some() => cs.editing = true,
                Nav::Left => cs.editing = false,
                Nav::Accept if cs.editing => cs.editing = false,
                Nav::Accept if !on_continue => cs.toggle(),
                Nav::Accept | Nav::Start | Nav::Extra => {
                    if !ready {
                        return;
                    }
                    cs.editing = false;
                    // The split-screen championship skips the points split
                    // until Custom is open, as the original does.
                    if fe.mp_champ && fe.players > 1 && !career.is_open(Champ::Custom) {
                        let plan = Plan { split: None, ..fe.custom.plan() };
                        start_mp_champ(&mut mp_champ, &mut career, plan, &roster, &rules, &sel, fe.players);
                        fe.screen = Screen::MpChampStatus;
                    } else {
                        fe.screen = Screen::CustomPoints;
                    }
                    return;
                }
                Nav::Back if cs.editing => cs.editing = false,
                Nav::Back => {
                    fe.screen = Screen::Quad;
                    return;
                }
                _ => return,
            }
            // The rounds, laps and hints follow each change.
            fe.built = None;
        }
        Screen::CustomPoints => match nav {
            Nav::Left | Nav::Right => {
                fe.custom.step_split(nav == Nav::Right);
                fe.built = None;
            }
            Nav::Accept | Nav::Start if fe.mp_champ && fe.players > 1 => {
                start_mp_champ(&mut mp_champ, &mut career, fe.custom.plan(), &roster, &rules, &sel, fe.players);
                fe.screen = Screen::MpChampStatus;
            }
            Nav::Accept | Nav::Start => {
                start_career(&mut career, fe.custom.plan(), &roster, &rules, &sel);
                fe.screen = Screen::CareerStatus;
            }
            Nav::Back => fe.screen = Screen::CustomTracks,
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
        Screen::ProfileSelect => match nav {
            Nav::Left | Nav::Right => {
                fe.slot = if nav == Nav::Left { (fe.slot + PROFILES - 1) % PROFILES } else { (fe.slot + 1) % PROFILES };
                fe.built = None;
            }
            Nav::Accept | Nav::Start => match fe.profiles.get(fe.slot).cloned().flatten() {
                Some(mut loaded) => {
                    loaded.upgrade(&rules);
                    unlocks::upgrade(&mut loaded, &roster);
                    unlocks::apply_bits(&mut loaded, &rules, std::mem::take(&mut fe.cheats));
                    loaded.save();
                    *career = loaded;
                    settings.profile = Some(fe.slot);
                    settings.save();
                    fe.screen = profiles_done(&fe);
                }
                None => new_profile(&mut fe, &roster),
            },
            Nav::Back => fe.screen = fe.profile_from,
            _ => {}
        },
        Screen::ProfileRider => {
            let riders = profile_riders(&roster);
            let n = riders.len();
            if n == 0 {
                return;
            }
            let k = riders.iter().position(|&i| i == fe.rider).unwrap_or(0);
            match nav {
                Nav::Left => fe.rider = riders[(k + n - 1) % n],
                Nav::Right => fe.rider = riders[(k + 1) % n],
                Nav::Up => fe.rider = riders[(k + n - PROFILE_MUGS_ROW % n) % n],
                Nav::Down => fe.rider = riders[(k + PROFILE_MUGS_ROW) % n],
                Nav::Accept | Nav::Start => {
                    let Some(r) = roster.riders.get(fe.rider) else { return };
                    if in_profile(&fe, &r.model) {
                        return;
                    }
                    fe.new_rider = r.model.clone();
                    fe.name.clear();
                    fe.key = (0, 0);
                    fe.screen = Screen::ProfileName;
                }
                Nav::Back if fe.profiles.iter().any(Option::is_some) => fe.screen = Screen::ProfileSelect,
                Nav::Back => fe.screen = fe.profile_from,
                Nav::Extra => {}
            }
        }
        Screen::ProfileName => {
            let row_len = |r: usize| KEY_ROWS.get(r).map_or(KEY_WORDS.len(), |k| k.chars().count());
            let (r, c) = fe.key;
            match nav {
                Nav::Left => fe.key.1 = (c + row_len(r) - 1) % row_len(r),
                Nav::Right => fe.key.1 = (c + 1) % row_len(r),
                Nav::Up | Nav::Down => {
                    // To the key across from this one.
                    let to = if nav == Nav::Up { (r + 3) % 4 } else { (r + 1) % 4 };
                    fe.key = (to, ((2 * c + 1) * row_len(to) / (2 * row_len(r))).min(row_len(to) - 1));
                }
                Nav::Accept if r < KEY_ROWS.len() => {
                    let ch = KEY_ROWS[r].chars().nth(c).unwrap_or(' ');
                    push_char(&mut fe, ch);
                }
                Nav::Accept if c == 0 => {
                    fe.name.pop();
                }
                Nav::Accept if c == 1 => push_char(&mut fe, ' '),
                Nav::Accept | Nav::Start => {
                    let name = fe.name.trim().to_string();
                    if let Some((bits, msg)) = unlocks::cheat(&name) {
                        // The cheat goes to the next profile made or loaded.
                        fe.cheats |= bits;
                        fe.name.clear();
                        fe.popup = Some(("CHEAT".into(), msg.into()));
                    } else if name.is_empty() {
                        fe.popup = Some((String::new(), ENTER_NAME.into()));
                    } else {
                        create_profile(&mut fe, &mut career, &rules, &mut settings, name);
                        fe.screen = profiles_done(&fe);
                    }
                }
                Nav::Back => fe.screen = Screen::ProfileRider,
                Nav::Extra => {}
            }
            fe.built = None;
        }
    }
}

/// Opens Available Profiles from `from`; with no profiles saved, goes
/// straight on to make the first, as the original does.
fn open_profiles(fe: &mut FrontEnd, roster: &Roster, career: &Career, from: Screen) {
    fe.profiles = Career::profiles();
    fe.profile_from = from;
    fe.slot = career.profile.as_ref().map_or(0, |p| p.slot);
    if fe.profiles.iter().all(Option::is_none) {
        new_profile(fe, roster);
        fe.popup = Some((String::new(), NO_PROFILES.into()));
    } else {
        fe.screen = Screen::ProfileSelect;
    }
}

/// Where loading or making a profile goes on to.
fn profiles_done(fe: &FrontEnd) -> Screen {
    if fe.profile_from == Screen::Options { Screen::Options } else { Screen::Main }
}

/// Choose A Rider for a profile in `fe.slot`, on the first rider no
/// profile has.
fn new_profile(fe: &mut FrontEnd, roster: &Roster) {
    let riders = profile_riders(roster);
    fe.rider = riders.iter().copied().find(|&i| !in_profile(fe, &roster.riders[i].model)).or(riders.first().copied()).unwrap_or(0);
    fe.screen = Screen::ProfileRider;
}

/// Makes the profile named on Name Your Rider and loads it, with any
/// cheats entered. The first profile carries on the progress made before
/// profiles.
fn create_profile(fe: &mut FrontEnd, career: &mut Career, rules: &career::Rules, settings: &mut Settings, name: String) {
    let first = career.profile.is_none() && fe.profiles.iter().all(Option::is_none);
    let mut new = if first { career.clone() } else { Career::new() };
    new.profile = Some(Profile { slot: fe.slot, name, rider: fe.new_rider.clone() });
    unlocks::apply_bits(&mut new, rules, std::mem::take(&mut fe.cheats));
    new.save();
    *career = new;
    settings.profile = Some(fe.slot);
    settings.save();
    fe.profiles = Career::profiles();
}

/// The riders a profile can have: the ones open from the start (not the
/// pros or the secret riders), as Choose A Rider offers them.
fn profile_riders(roster: &Roster) -> Vec<usize> {
    roster.riders.iter().enumerate().filter(|(_, r)| !r.locked).map(|(i, _)| i).collect()
}

/// Whether a saved profile has this rider (no two may share one).
fn in_profile(fe: &FrontEnd, model: &str) -> bool {
    fe.profiles.iter().flatten().filter_map(|c| c.profile.as_ref()).any(|p| p.rider.eq_ignore_ascii_case(model))
}

/// Whether the player rides the profile's rider: in Career, Arcade and
/// ATV Academy.
fn rides_profile(fe: &FrontEnd, career: &Career) -> bool {
    career.profile.is_some() && fe.players <= 1 && matches!(fe.mode, Mode::Career | Mode::Arcade | Mode::Training)
}

/// On to rider select, or past it to ATV select on the profile's rider.
fn to_rider(fe: &mut FrontEnd, sel: &mut Selection, career: &Career) {
    match career.profile.as_ref().filter(|_| rides_profile(fe, career)) {
        Some(p) => {
            sel.rider = p.rider.clone();
            fe.screen = Screen::Quad;
        }
        None => fe.screen = Screen::Rider,
    }
}

/// The rider Arcade's field is drawn around: the profile's, or the last
/// picked.
fn arcade_rider(career: &Career, sel: &Selection) -> String {
    career.profile.as_ref().map_or(sel.rider.clone(), |p| p.rider.clone())
}

/// A new Arcade game: round 1 and a new field, shown on Arcade Status.
fn new_arcade(fe: &mut FrontEnd, career: &mut Career, roster: &Roster, sel: &Selection) {
    fe.arcade_round = 0;
    career.arcade_field = Some(ArcadeField::new(roster, &arcade_rider(career, sel), 0));
    fe.screen = Screen::ArcadeStatus;
}

/// A key typed on Name Your Rider: true when it typed or deleted.
fn type_key(fe: &mut FrontEnd, k: &KeyboardInput) -> bool {
    if k.key_code == KeyCode::Backspace {
        fe.name.pop();
        return true;
    }
    let Some(text) = &k.text else { return false };
    let mut typed = false;
    for c in text.chars().map(|c| c.to_ascii_uppercase()) {
        if c == ' ' || KEY_ROWS.iter().any(|r| r.contains(c)) {
            push_char(fe, c);
            typed = true;
        }
    }
    typed
}

fn push_char(fe: &mut FrontEnd, c: char) {
    if fe.name.chars().count() < NAME_LEN && !(c == ' ' && fe.name.is_empty()) {
        fe.name.push(c);
    }
}

/// Moves on Name Your Rider, where letters, space and backspace type: the
/// arrows, Enter for OK, Escape to go back, and the pads as usual.
fn name_nav(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>) -> Option<Nav> {
    let k = |c: KeyCode| keys.just_pressed(c);
    if k(KeyCode::ArrowUp) {
        Some(Nav::Up)
    } else if k(KeyCode::ArrowDown) {
        Some(Nav::Down)
    } else if k(KeyCode::ArrowLeft) {
        Some(Nav::Left)
    } else if k(KeyCode::ArrowRight) {
        Some(Nav::Right)
    } else if k(KeyCode::Enter) || k(KeyCode::NumpadEnter) {
        Some(Nav::Start)
    } else if k(KeyCode::Escape) {
        Some(Nav::Back)
    } else {
        read_nav_from(&ButtonInput::default(), pads, None)
    }
}

/// The championship menu: carrying on with one in progress, then the four.
fn champ_items(career: &Career) -> Vec<(String, Option<Champ>)> {
    let mut items = Vec::new();
    if let Some(r) = career.run.as_ref().filter(|r| !r.over()) {
        items.push((format!("CONTINUE {}", r.champ.label()), None));
    }
    items.extend(Champ::EVERY.map(|c| (c.label().to_string(), Some(c))));
    items
}

/// Starts a championship for the picked rider and ATV, and saves it.
fn start_career(career: &mut Career, plan: Plan, roster: &Roster, rules: &career::Rules, sel: &Selection) {
    // The profile's name on its own rider.
    let rider = roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&sel.rider));
    let name = match &career.profile {
        Some(p) if p.rider.eq_ignore_ascii_case(&sel.rider) => p.name.clone(),
        _ => rider.map_or(sel.rider.to_uppercase(), |r| r.name.clone()),
    };
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
    let pool = career_pool(roster, rules, plan.champ);
    career.start(plan, (&name, &sel.rider), &sel.quad, &pool, seed);
    career.save();
}

/// Starts the split-screen championship for the players' picks, as the
/// profile's championship, and saves it.
fn start_mp_champ(mp_champ: &mut MpChamp, career: &mut Career, plan: Plan, roster: &Roster, rules: &career::Rules, sel: &Selection, players: usize) {
    let name = |model: &str| roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(model)).map_or(model.to_uppercase(), |r| r.name.clone());
    let picks: Vec<(String, String)> = std::iter::once(&sel.rider).chain(sel.others.iter().map(|o| &o.0)).take(players).map(|m| (name(m), m.clone())).collect();
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
    mp_champ.start(plan, &picks, &sel.quad, &career_pool(roster, rules, Champ::Custom), seed);
    mp_champ.keep(career);
}

/// The tracks Choose Your Tracks lists: the championships' fifteen that
/// are open, in `Tracks.xml` order.
fn custom_tracks(roster: &Roster, rules: &career::Rules, career: &Career) -> Vec<String> {
    roster
        .tracks
        .iter()
        .filter(|t| rules.tracks.iter().flatten().any(|c| c.eq_ignore_ascii_case(t)) && unlocks::track_open(career, roster, rules, t))
        .cloned()
        .collect()
}

/// Moves the rider select cursor, over riders it doesn't show.
fn step_rider(from: usize, nav: Nav, n: usize, shown: impl Fn(usize) -> bool) -> usize {
    let n = n.max(1);
    let mut i = match nav {
        Nav::Left => (from + n - 1) % n,
        Nav::Right => (from + 1) % n,
        Nav::Up => (from + n - 8 % n) % n,
        Nav::Down => (from + 8) % n,
        _ => return from,
    };
    for _ in 0..n {
        if shown(i) {
            return i;
        }
        i = if nav == Nav::Right { (i + 1) % n } else { (i + n - 1) % n };
    }
    from
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
        // TrainingSM opens Open Freestyle's ATV page (page 0x26).
        Mode::Training => ("Freestyle_Open_Rider", "Freestyle_Open_ATV", "Freestyle_Open"),
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
        Screen::MpChampStatus => (mp_champ.run.as_ref().map_or("Career_Status_Amateur", |r| r.champ.status_screen()), "CHAMPIONSHIP"),
        Screen::ArcadeNew => ("Arcade_Newgame", "ARCADE"),
        Screen::ArcadeStatus => ("Arcade_Status", "ARCADE"),
        Screen::ArcadeComplete => ("Arcade_Complete", "ARCADE"),
        Screen::TrickBook => ("Trickbook", ""),
        Screen::CustomTracks => ("Career_Custom_Tracks", if split { "CHAMPIONSHIP" } else { "CAREER" }),
        Screen::CustomPoints => ("Career_Custom_Points_Distrib", if split { "CHAMPIONSHIP" } else { "CAREER" }),
        Screen::ProfileSelect => ("Profile_Select", ""),
        Screen::ProfileRider => ("Profile_Rider", ""),
        Screen::ProfileName => ("Profile_Name", ""),
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
                _ if fe.players > 1 => 4,
                Mode::Arcade => 0,
                Mode::Career => 1,
                Mode::SingleRace => 2,
                Mode::Freestyle | Mode::Challenge => 3,
                Mode::TimeTrial => 5,
                Mode::Training => 6,
            };
            let mut off = vec![];
            if roster.arcade.is_empty() {
                off.push("ARCADE");
            }
            // The original's order (0x47f70), and a way out.
            let list = MenuList::new(&["ARCADE", "CAREER", "SINGLE RACE", "FREESTYLE", "MULTIPLAYER", "TIME TRIAL", "ATV ACADEMY", "OPTIONS", "QUIT GAME"], 280.0)
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
                if !unlocks::rider_shown(&career, &defs, rider) {
                    continue;
                }
                // Dimmed when another player has them, or they're locked.
                let dim = taken(&fe, &sel, &rider.model) || !unlocks::rider_open(&career, &defs, rider);
                kids.push(mug(&mut commands, &ui, i, mug_pos(i), dim));
            }
            kids.push(bio_panel(&mut commands, &ui, 64.0, 296.0, &["ABILITY", "TECHNIQUE", "AGGRESSION", "STRENGTH"], true));
            kids.push(locked_line(&mut commands, 278.0));
        }
        Screen::ProfileRider => {
            let sq = ui.design.get("SELECTION_SQUARE").rect;
            let (mut n, img) = ui.design.tex.region(sq, (MUG + 22.0) / sq.width());
            n.position_type = PositionType::Absolute;
            kids.push(commands.spawn((n, img, MugCursor)).id());
            for (k, i) in profile_riders(&roster).into_iter().enumerate() {
                // Another profile's rider is dimmed.
                kids.push(mug(&mut commands, &ui, i, profile_mug_pos(k), in_profile(&fe, &roster.riders[i].model)));
            }
            kids.push(bio_panel(&mut commands, &ui, 64.0, 296.0, &["ABILITY", "TECHNIQUE", "AGGRESSION", "STRENGTH"], true));
            kids.push(locked_line(&mut commands, 278.0));
        }
        Screen::ProfileSelect => {
            // The ten slots in a row, the selection square behind the one
            // picked and the empty ones dimmed.
            let (tile, gap, y) = (48.0, 8.0, 92.0);
            let x0 = (SCREEN_W - (PROFILES as f32 * tile + (PROFILES - 1) as f32 * gap)) / 2.0;
            let sq = ui.design.get("SELECTION_SQUARE").rect;
            let (mut n, img) = ui.design.tex.region(sq, (tile + 20.0) / sq.width());
            n.position_type = PositionType::Absolute;
            n.left = px(x0 + fe.slot as f32 * (tile + gap) - 10.0);
            n.top = px(y - 10.0);
            kids.push(commands.spawn((n, img)).id());
            let scale = ui.profile_numbers.size.x / 512.0;
            for i in 0..PROFILES {
                let corner = Vec2::new((i % 8) as f32, (i / 8) as f32) * 64.0;
                let rect = Rect::from_corners(corner * scale, (corner + 64.0) * scale);
                let (mut n, mut img) = ui.profile_numbers.region(rect, tile / rect.width());
                if fe.profiles.get(i).is_none_or(Option::is_none) {
                    img.color = Color::srgb(0.35, 0.35, 0.35);
                }
                n.position_type = PositionType::Absolute;
                n.left = px(x0 + i as f32 * (tile + gap));
                n.top = px(y);
                kids.push(commands.spawn((n, img)).id());
            }
            let saved = fe.profiles.get(fe.slot).and_then(Option::as_ref);
            let name = saved.and_then(|c| c.profile.as_ref()).map_or("EMPTY PROFILE".to_string(), |p| p.name.clone());
            let w = ui.font(FontId::Menu).width(&name, 20.0);
            kids.push(commands.spawn((BitmapText::new(FontId::Menu, 20.0, name), ui_at((SCREEN_W - w) / 2.0, 170.0))).id());
            if let Some(c) = saved {
                let rider = c.profile.as_ref().and_then(|p| roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&p.rider)));
                if let Some(r) = rider {
                    kids.push(hint(&mut commands, &ui, &r.name, 200.0));
                }
                for (i, (label, value)) in profile_stats(c, &roster, &rules).into_iter().enumerate() {
                    let y = 236.0 + 22.0 * i as f32;
                    kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, label).color(Color::srgb(0.75, 0.82, 0.95)), ui_at(190.0, y))).id());
                    kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, value), ui_at(380.0, y))).id());
                }
            }
        }
        Screen::ProfileName => {
            let gold = Color::srgb(1.0, 0.85, 0.3);
            if let Some(r) = roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&fe.new_rider)) {
                kids.push(hint(&mut commands, &ui, &r.name, 76.0));
            }
            // The name in its box, then the keyboard.
            let small = ui.name_entry.get("SMALLNAMEPANEL").rect;
            let (bw, bs) = (320.0, 320.0 / small.width());
            let (bx, by) = ((SCREEN_W - bw) / 2.0, 96.0);
            kids.push(commands.spawn(placed(ui.name_entry.node("SMALLNAMEPANEL", bs), bx, by)).id());
            let menu_font = ui.font(FontId::Menu);
            let w = menu_font.width(&fe.name, 18.0);
            let tx = (SCREEN_W - w) / 2.0;
            let ty = by + (small.height() * bs - 18.0) / 2.0;
            kids.push(commands.spawn((BitmapText::new(FontId::Menu, 18.0, fe.name.clone()), ui_at(tx, ty))).id());
            if fe.name.chars().count() < NAME_LEN {
                kids.push(commands.spawn((BitmapText::new(FontId::Menu, 18.0, "_").color(gold), ui_at(tx + w + 2.0, ty), Blink)).id());
            }
            let panel = ui.name_entry.get("NAMEPANEL").rect;
            let (pw, ps) = (540.0, 540.0 / panel.width());
            let (px0, py0) = ((SCREEN_W - pw) / 2.0, 156.0);
            kids.push(commands.spawn(placed(ui.name_entry.node("NAMEPANEL", ps), px0, py0)).id());
            let row_h = (panel.height() * ps - 24.0) / 4.0;
            let mut rows: Vec<Vec<String>> = KEY_ROWS.iter().map(|r| r.chars().map(String::from).collect()).collect();
            rows.push(KEY_WORDS.iter().map(|w| w.to_string()).collect());
            for (r, keys) in rows.iter().enumerate() {
                let words = r == KEY_ROWS.len();
                let step = if words { 150.0 } else { 36.0 };
                let x_mid = SCREEN_W / 2.0 - step * (keys.len() - 1) as f32 / 2.0;
                for (c, key) in keys.iter().enumerate() {
                    let on = fe.key == (r, c);
                    let size = if on { 20.0 } else { 16.0 };
                    let kw = menu_font.width(key, size);
                    let y = py0 + 12.0 + r as f32 * row_h + (row_h - size) / 2.0;
                    let text = BitmapText::new(FontId::Menu, size, key.clone()).color(if on { gold } else { Color::WHITE });
                    kids.push(commands.spawn((text, ui_at(x_mid + step * c as f32 - kw / 2.0, y))).id());
                }
            }
            kids.push(hint(&mut commands, &ui, "TYPE A NAME OR PICK THE LETTERS  -  ENTER WHEN DONE", py0 + panel.height() * ps + 14.0));
        }
        Screen::Quad => {
            let names: Vec<String> = roster.quads.iter().map(|q| q.name.to_uppercase()).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            // Unlockable ATVs come with career gold medals.
            let list = MenuList::new(&refs, 200.0).with_row(26.0).select(fe.quad).lock(|i| unlocks::atv_lock(&career, &rules, &roster.quads[i]).is_some());
            kids.push(menu(&mut commands, &ui, list, 40.0, 80.0));
            kids.push(bio_panel(&mut commands, &ui, 270.0, 250.0, &["TOP SPEED", "ACCELERATION", "STABILITY", "HANDLING", "AIR RATING"], false));
            kids.push(locked_line(&mut commands, 418.0));
        }
        Screen::Track => {
            let names: Vec<String> = roster.tracks.iter().map(|t| track_title(t)).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            // Races only go to open tracks; Open Freestyle roams any.
            let race = fe.mode != Mode::Freestyle;
            let list = MenuList::new(&refs, 200.0)
                .with_row(20.0)
                .select(fe.track)
                .lock(|i| race && !unlocks::track_open(&career, &roster, &rules, &roster.tracks[i]));
            kids.push(menu(&mut commands, &ui, list, 40.0, 70.0));
            kids.push(locked_line(&mut commands, 418.0));
            let (left, side) = (250.0, 196.0);
            let preview = commands
                .spawn((
                    Node { position_type: PositionType::Absolute, left: px(left), top: px(RECORD_Y), width: px(side), height: px(side), ..default() },
                    ImageNode::default().with_mode(bevy::ui::widget::NodeImageMode::Stretch),
                    TrackPreview,
                ))
                .id();
            kids.push(preview);
            kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, "RECORDS"), ui_at(RECORD_X, RECORD_Y - 14.0))).id());
            for k in 0..3 {
                kids.push(record_panel(&mut commands, &ui, k));
            }
            kids.push(commands.spawn((BitmapText::new(FontId::Menu, 16.0, ""), ui_at(left, 290.0), Field::TrackName)).id());
            let freestyle = fe.mode == Mode::Freestyle;
            let laps = commands
                .spawn(Node { position_type: PositionType::Absolute, left: px(left), top: px(326), align_items: AlignItems::Center, column_gap: px(10), ..default() })
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
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 8.0, "").color(Color::srgb(1.0, 0.85, 0.3)), ui_at(left, 354.0), Field::Targets)).id());
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
        Screen::MpChampStatus => {
            if let Some(run) = &mp_champ.run {
                let head = if run.over() {
                    let leader = run.standings().first().map_or(String::new(), |e| e.name.clone());
                    format!("{} CHAMPIONSHIP  -  {leader} WINS", run.champ.label())
                } else {
                    round_head(run)
                };
                kids.push(commands.spawn((BitmapText::new(FontId::Menu, 14.0, head), ui_at(40.0, 80.0))).id());
                kids.push(standings_table(&mut commands, run, 40.0, 116.0));
                if let Some(tex) = run.next_round().and_then(|(t, _)| ui.tracks.get(&t.to_ascii_lowercase())) {
                    kids.push(commands.spawn(placed(tex.node(220.0, 220.0), 380.0, 116.0)).id());
                }
            }
        }
        Screen::TrickBook => {
            let (level, all) = book_level(&fe, &roster, &career, &rules);
            if let Some(def) = roster.book.get(fe.trick).and_then(|&i| roster.tricks.get(i)) {
                let locked = def.level > level;
                let plain = Color::srgb(0.75, 0.82, 0.95);
                let (x, y) = (360.0, 76.0);
                kids.extend(trickbook::heading(&mut commands, def, locked, (x, y)));
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 8.0, "ACTIVATION").color(plain), ui_at(x, y + 76.0))).id());
                for (i, (dev, names)) in [("PAD", &trickbook::PAD_NAMES), ("KEYS", &trickbook::KEY_NAMES)].into_iter().enumerate() {
                    let t = format!("{dev}  {}", trickbook::activation(def, names));
                    kids.push(commands.spawn((BitmapText::new(FontId::Small, 8.0, t), ui_at(x, y + 92.0 + 15.0 * i as f32))).id());
                }
                kids.push(trickbook::pad_picture(&mut commands, &ui, def, locked, (x - 6.0, y + 122.0), 0.9));
                // The page count under the rider, between arrows.
                let count = format!("{} OF {}", fe.trick + 1, roster.book.len());
                let w = ui.font(FontId::Small).width(&count, 9.0);
                let mid = (TRICK_VIEW.min.x + TRICK_VIEW.max.x) / 2.0;
                let row = commands
                    .spawn(Node { position_type: PositionType::Absolute, left: px(mid - w / 2.0 - 30.0), top: px(TRICK_VIEW.max.y + 10.0), align_items: AlignItems::Center, column_gap: px(10), ..default() })
                    .with_children(|r| {
                        let (n, mut img) = ui.design.node("DARKLRARROW", 0.6);
                        img.flip_x = true;
                        r.spawn((n, img));
                        r.spawn(BitmapText::new(FontId::Small, 9.0, count));
                        r.spawn(ui.design.node("DARKLRARROW", 0.6));
                    })
                    .id();
                kids.push(row);
                let skill = if all {
                    format!("SKILL LEVEL {level}   -   {} SKILL POINTS", career.skill_points)
                } else {
                    let name = roster.riders.get(fe.rider).map_or(String::new(), |r| r.name.clone());
                    format!("{name}  -  SKILL LEVEL {level}")
                };
                kids.push(hint(&mut commands, &ui, &skill, 398.0));
                if locked {
                    // The disc's "Earn %d more skill points in career and challenge mode".
                    let need = rules.skill_levels.get(usize::from(def.level) - 1).map_or(0, |p| p.saturating_sub(career.skill_points));
                    let msg = format!("EARN {need} MORE SKILL POINTS IN CAREER AND CHALLENGE MODE");
                    let w = ui.font(FontId::Small).width(&msg, 8.0);
                    kids.push(commands.spawn((BitmapText::new(FontId::Small, 8.0, msg).color(Color::srgb(1.0, 0.85, 0.3)), ui_at((SCREEN_W - w) / 2.0, 414.0))).id());
                }
            }
        }
        Screen::CustomTracks => {
            let cs = &fe.custom;
            let (top, row) = (96.0, 20.0);
            let mut names: Vec<String> = cs.rows.iter().map(|r| track_title(&r.track)).collect();
            names.push("CONTINUE".into());
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let mut list = MenuList::new(&refs, 200.0).with_row(row).select(cs.cursor);
            if cs.count() == 0 {
                list = list.disable(&["CONTINUE"]);
            }
            kids.push(menu(&mut commands, &ui, list, 100.0, top));
            // Column heads, then each round's number and laps beside its track.
            let dim = Color::srgb(0.75, 0.82, 0.95);
            let gold = Color::srgb(1.0, 0.85, 0.3);
            for (text, x) in [("ROUND", 40.0), ("TRACK", 176.0), ("LAPS", 318.0)] {
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 8.0, text).color(dim), ui_at(x, top - 18.0))).id());
            }
            for (i, r) in cs.rows.iter().enumerate() {
                let Some(round) = r.round else { continue };
                let y = top + i as f32 * row + 5.0;
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, (round + 1).to_string()), ui_at(56.0, y))).id());
                let editing = cs.editing && i == cs.cursor;
                let laps = if editing { format!("< {} >", r.laps) } else { r.laps.to_string() };
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, laps).color(if editing { gold } else { Color::WHITE }), ui_at(if editing { 316.0 } else { 328.0 }, y))).id());
            }
            if let Some(tex) = cs.rows.get(cs.cursor).and_then(|r| ui.tracks.get(&r.track.to_ascii_lowercase())) {
                kids.push(commands.spawn(placed(tex.node(200.0, 200.0), 400.0, 96.0)).id());
            }
            let lines: &[&str] = match cs.rows.get(cs.cursor) {
                _ if cs.editing => &["UP AND DOWN SET THE LAPS"],
                Some(r) if r.round.is_some() => &["RIGHT SETS ITS LAPS", "SELECT TAKES IT OUT"],
                Some(_) => &["SELECT ADDS IT AS A ROUND"],
                None => &["ON TO THE POINTS"],
            };
            for (k, l) in lines.iter().enumerate() {
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 8.0, *l).color(dim), ui_at(400.0, 310.0 + 15.0 * k as f32))).id());
            }
            let total = match cs.count() {
                1 => "1 ROUND".to_string(),
                n => format!("{n} ROUNDS"),
            };
            kids.push(commands.spawn((BitmapText::new(FontId::Menu, 13.0, total), ui_at(400.0, 360.0))).id());
        }
        Screen::CustomPoints => {
            // As the original lays it out: each place's race points, its bar
            // with the slider on it, then its bonus (trick) points.
            let split = fe.custom.split;
            let (race, bonus) = rules.split_points(split);
            let (bar_x, top, row) = (228.0, 124.0, 30.6);
            let dim = Color::srgb(0.75, 0.82, 0.95);
            let small = ui.font(FontId::Small);
            for (text, right, x) in [("RACE POINTS", true, bar_x - 6.0), ("BONUS POINTS", false, bar_x + 190.0)] {
                let x = if right { x - small.width(text, 8.0) } else { x };
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 8.0, text).color(dim), ui_at(x, top - 20.0))).id());
            }
            kids.push(commands.spawn(placed(ui.points.node("HBAR8", 1.0), bar_x, top)).id());
            for i in 0..career::FIELD {
                let y = top + i as f32 * row;
                let place = ordinal(i + 1);
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, place.clone()).color(dim), ui_at(150.0 - small.width(&place, 9.0), y + 10.0))).id());
                let r = race[i].to_string();
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, r.clone()), ui_at(bar_x - 6.0 - small.width(&r, 9.0), y + 10.0))).id());
                kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, bonus[i].to_string()), ui_at(bar_x + 190.0, y + 10.0))).id());
                kids.push(commands.spawn(placed(ui.points.node("BLOB1", 1.0), bar_x + 5.0 + 1.44 * split as f32, y)).id());
            }
            kids.push(hint(&mut commands, &ui, &format!("{}% RACE   -   {split}% TRICKS", 100 - split), 380.0));
            kids.push(hint(&mut commands, &ui, "LEFT AND RIGHT SHARE THE POINTS OUT  -  BONUS POINTS GO BY TRICK SCORE", 398.0));
        }
        Screen::ArcadeStatus | Screen::ArcadeComplete => {
            if let Some(field) = &career.arcade_field {
                let complete = fe.screen == Screen::ArcadeComplete;
                let track = roster.arcade.get(fe.arcade_round);
                let head = match track {
                    _ if complete => "CONGRATULATIONS".to_string(),
                    Some(t) => format!("ROUND {} OF {}  -  {}", fe.arcade_round + 1, roster.arcade.len(), track_title(t)),
                    None => String::new(),
                };
                kids.push(commands.spawn((BitmapText::new(FontId::Menu, 14.0, head), ui_at(40.0, 80.0))).id());
                let player = match &career.profile {
                    Some(p) => p.name.clone(),
                    None => roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&sel.rider)).map_or("YOU".to_string(), |r| r.name.clone()),
                };
                kids.push(arcade_table(&mut commands, field, &roster, (&player, rules.certificate(career.skill_points)), &rules, !complete, (40.0, 116.0)));
                if complete {
                    kids.push(hint(&mut commands, &ui, &format!("YOU WON ALL {} ROUNDS", roster.arcade.len()), 380.0));
                } else if let Some(tex) = track.and_then(|t| ui.tracks.get(&t.to_ascii_lowercase())) {
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
                    round_head(run)
                };
                kids.push(commands.spawn((BitmapText::new(FontId::Menu, 14.0, head), ui_at(40.0, 80.0))).id());
                kids.push(standings_table(&mut commands, run, 40.0, 116.0));
                if complete {
                    let next = run.champ.next().filter(|c| career.is_open(*c));
                    let msg = match next {
                        Some(c) if run.player_place() <= 3 => format!("{} CHAMPIONSHIP OPEN", c.label()),
                        _ if run.player_place() > 3 => "FINISH IN THE TOP 3 TO OPEN THE NEXT CHAMPIONSHIP".to_string(),
                        _ => "ALL CHAMPIONSHIPS OPEN".to_string(),
                    };
                    kids.push(hint(&mut commands, &ui, &msg, 380.0));
                } else if let Some(tex) = run.next_round().and_then(|(t, _)| ui.tracks.get(&t.to_ascii_lowercase())) {
                    kids.push(commands.spawn(placed(tex.node(220.0, 220.0), 380.0, 116.0)).id());
                }
            }
        }
    }

    if let Some((head, text)) = &fe.popup {
        kids.push(popup(&mut commands, &ui, head, text));
    }

    // Button hints along the bottom.
    let hints: &[(&str, &str)] = match fe.screen {
        _ if fe.popup.is_some() => &[("controlbuttoncross", "OK")],
        Screen::ProfileSelect if fe.profiles.get(fe.slot).is_none_or(Option::is_none) => &[("controlbuttoncross", "CREATE PROFILE"), ("controlbuttoncircle", "BACK")],
        Screen::Title => &[],
        Screen::Main => &[("controlbuttoncross", "SELECT")],
        Screen::Audio | Screen::Camera => &[("controlbuttoncross", "OK")],
        Screen::CareerChamp => &[("controlbuttoncross", "SELECT"), ("controlbuttoncircle", "BACK")],
        Screen::Controller => &[("controlbuttoncross", "CHANGE"), ("controlbuttoncircle", "BACK")],
        Screen::CareerStatus => &[("controlbuttoncross", "RACE"), ("YButton", "TRICK BOOK"), ("controlbuttoncircle", "BACK")],
        Screen::Rider | Screen::Track => &[("controlbuttoncross", "SELECT"), ("YButton", "TRICK BOOK"), ("controlbuttoncircle", "BACK")],
        Screen::TrickBook => &[("controlbuttoncircle", "BACK")],
        Screen::MpChampStatus if mp_champ.run.as_ref().is_some_and(|r| r.over()) => &[("controlbuttoncross", "OK")],
        Screen::MpChampStatus => &[("controlbuttoncross", "RACE"), ("controlbuttoncircle", "BACK")],
        Screen::CareerComplete | Screen::ArcadeComplete => &[("controlbuttoncross", "OK")],
        Screen::ArcadeStatus => &[("controlbuttoncross", "RACE"), ("controlbuttoncircle", "BACK")],
        Screen::CustomTracks if fe.custom.cursor < fe.custom.rows.len() && !fe.custom.editing => {
            let on = fe.custom.rows[fe.custom.cursor].round.is_some();
            if fe.custom.count() > 0 {
                &[("controlbuttoncross", if on { "REMOVE TRACK" } else { "ADD TRACK" }), ("YButton", "CONTINUE"), ("controlbuttoncircle", "BACK")]
            } else {
                &[("controlbuttoncross", "ADD TRACK"), ("controlbuttoncircle", "BACK")]
            }
        }
        Screen::CustomTracks if fe.custom.editing => &[("controlbuttoncross", "OK"), ("controlbuttoncircle", "BACK")],
        Screen::CustomTracks => &[("controlbuttoncross", "CONTINUE"), ("controlbuttoncircle", "BACK")],
        Screen::CustomPoints => &[("controlbuttoncross", "CONTINUE"), ("controlbuttoncircle", "BACK")],
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
pub(crate) fn lit_width(level: u8) -> f32 {
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

/// Arcade Status's table, as the original's: the grid front to back
/// ("1. NAME", new riders marked out) and each rider's ranking, then who
/// went out after the last round (Arcade Complete: the final order alone).
/// `player`: the player's name and ranking.
fn arcade_table(commands: &mut Commands, field: &ArcadeField, roster: &Roster, player: (&str, &str), rules: &career::Rules, with_out: bool, (left, top): (f32, f32)) -> Entity {
    let dim = Color::srgb(0.75, 0.82, 0.95);
    let gold = Color::srgb(1.0, 0.85, 0.3);
    let new = Color::srgb(0.55, 1.0, 0.55);
    let gone = Color::srgb(1.0, 0.5, 0.45);
    let rider = |model: &str| roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(model));
    let named = |model: &str| rider(model).map_or(model.to_ascii_uppercase(), |r| r.name.clone());
    let ranked = |model: &str| rider(model).map_or("", |r| arcade::ranking(rules, r));
    let head = if with_out { "GRID POSITIONS" } else { "FINAL ORDER" };
    let mut rows: Vec<([String; 2], Color)> = vec![([head.into(), "RANKING".into()], dim)];
    let mut grid: Vec<(usize, Option<&arcade::Entrant>)> = field.riders.iter().map(|e| (e.grid, Some(e))).collect();
    grid.push((field.grid, None));
    grid.sort_by_key(|g| g.0);
    for (i, (_, e)) in grid.into_iter().enumerate() {
        rows.push(match e {
            None => ([format!("{}. {}", i + 1, player.0), player.1.to_string()], gold),
            Some(e) => ([format!("{}. {}", i + 1, named(&e.model)), ranked(&e.model).to_string()], if e.rounds == 0 { new } else { Color::WHITE }),
        });
    }
    if with_out {
        for m in field.just_out() {
            rows.push(([format!("OUT: {}", named(m)), ranked(m).to_string()], gone));
        }
    }
    commands
        .spawn(Node { position_type: PositionType::Absolute, left: px(left), top: px(top), flex_direction: FlexDirection::Column, row_gap: px(10), ..default() })
        .with_children(|t| {
            for (cells, color) in rows {
                t.spawn(Node { column_gap: px(10), ..default() }).with_children(|r| {
                    for (text, width) in cells.into_iter().zip([210.0, 110.0]) {
                        r.spawn(Node { width: px(width), ..default() }).with_children(|c| {
                            c.spawn(BitmapText::new(FontId::Small, 9.0, text).color(color));
                        });
                    }
                });
            }
        })
        .id()
}

/// "ROUND 2 OF 5  -  FOREST 1" for a championship's next round.
fn round_head(run: &Run) -> String {
    let (track, laps) = run.next_round().map_or((String::new(), 0), |(t, l)| (track_title(t), l));
    let laps = if run.champ == Champ::Custom { format!("  -  {laps} {}", if laps == 1 { "LAP" } else { "LAPS" }) } else { String::new() };
    format!("ROUND {} OF {}  -  {track}{laps}", run.round + 1, run.rounds())
}

/// A centred line saying why the highlighted item is locked
/// ([`Field::Locked`], empty while it isn't).
fn locked_line(commands: &mut Commands, top: f32) -> Entity {
    commands
        .spawn(Node { position_type: PositionType::Absolute, left: px(0), top: px(top), width: px(SCREEN_W), justify_content: JustifyContent::Center, ..default() })
        .with_children(|r| {
            r.spawn((BitmapText::new(FontId::Small, 8.0, "").color(Color::srgb(1.0, 0.85, 0.3)), Field::Locked));
        })
        .id()
}

/// A message box over the screen, as the original's (`ID_PROFILE_MSGBOX`,
/// `ID_CHEAT_PAGE`).
pub(crate) fn popup(commands: &mut Commands, ui: &UiAssets, head: &str, text: &str) -> Entity {
    let panel = ui.bio.get("BIOPANEL").rect;
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, width: px(SCREEN_W), height: px(SCREEN_H), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75)),
        ))
        .with_children(|o| {
            o.spawn((
                Node {
                    width: px(440),
                    min_height: px(130),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    row_gap: px(10),
                    padding: UiRect::all(px(20)),
                    ..default()
                },
                // The panel is see-through; the screen shouldn't show through it.
                BackgroundColor(Color::srgba(0.02, 0.04, 0.12, 0.9)),
            ))
            .with_children(|p| {
                let (mut n, img) = ui.bio.tex.region(panel, 1.0);
                n.position_type = PositionType::Absolute;
                n.left = px(0);
                n.top = px(0);
                n.width = percent(100);
                n.height = percent(100);
                p.spawn((n, img, ZIndex(-1)));
                if !head.is_empty() {
                    p.spawn(BitmapText::new(FontId::Menu, 18.0, head).color(Color::srgb(1.0, 0.85, 0.3)));
                }
                for line in challenge::wrap(text, 40) {
                    p.spawn(BitmapText::new(FontId::Small, 9.0, line));
                }
            });
        })
        .id()
}

/// Available Profiles' figures for a profile, as the original lists them.
fn profile_stats(c: &Career, roster: &Roster, rules: &career::Rules) -> [(&'static str, String); 6] {
    let champ = Champ::ALL.into_iter().rev().find(|&ch| c.is_open(ch)).unwrap_or_default();
    // Arcade's tracks reached.
    let tracks: std::collections::BTreeSet<String> = roster.arcade.iter().map(|t| t.to_ascii_lowercase()).collect();
    let arcade = tracks.iter().filter(|t| c.reached.iter().any(|r| r.eq_ignore_ascii_case(t))).count();
    let medals = c.challenges.values().filter(|ch| ch.1.is_some()).count();
    [
        ("SKILL LEVEL", rules.skill_level(c.skill_points).to_string()),
        ("SKILL POINTS", c.skill_points.to_string()),
        ("RANKING", rules.certificate(c.skill_points).to_string()),
        ("CHAMPIONSHIP", champ.label().to_string()),
        ("ARCADE", format!("{arcade}/{}", tracks.len())),
        ("CHALLENGE", format!("{medals}/{}", challenge::COUNT)),
    ]
}

fn hint(commands: &mut Commands, ui: &UiAssets, text: &str, top: f32) -> Entity {
    let w = ui.font(FontId::Small).width(text, 8.0);
    commands.spawn((BitmapText::new(FontId::Small, 8.0, text).color(Color::srgb(0.75, 0.82, 0.95)), ui_at((SCREEN_W - w) / 2.0, top))).id()
}

/// The controls, pad and keyboard, beside the front of the big pad.
/// The controls, as the Controller pages list them: action, pad, keys.
pub(crate) const CONTROLS: [(&str, &str, &str); 12] = [
    ("ACCELERATE", "RT OR A", "W OR UP"),
    ("BRAKE AND REVERSE", "LT OR X", "S OR DOWN"),
    ("STEER", "LEFT STICK", "A D OR LEFT RIGHT"),
    ("LEAN", "LEFT STICK", "I K"),
    ("PRE-LOAD AND HOP", "B OR RB", "SPACE"),
    ("SPEED BOOST", "Y", "SHIFT"),
    ("KICK", "LB", "E"),
    ("TRICKS IN THE AIR", "X Y LB RB", "1 2 3 4"),
    ("CHANGE CAMERA", "BACK", "C"),
    ("PAUSE", "START", "ESCAPE"),
    ("BACK ON TRACK", "", "R"),
    ("RESTART", "", "BACKSPACE"),
];

fn controls_table(commands: &mut Commands, ui: &UiAssets) -> Entity {
    let grey = Color::srgb(0.75, 0.82, 0.95);
    commands
        .spawn(Node { position_type: PositionType::Absolute, left: px(24), top: px(140), column_gap: px(14), align_items: AlignItems::Center, ..default() })
        .with_children(|t| {
            // The front of the pad is the bottom of the page.
            let front = Rect::new(20.0, 280.0, 340.0, 512.0);
            let scale = ui.big_pad.size.x / 512.0;
            t.spawn(ui.big_pad.region(Rect::from_corners(front.min * scale, front.max * scale), 0.62 / scale));
            t.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(9), ..default() }).with_children(|c| {
                for (i, (action, pad, keys)) in [("", "PAD", "KEYBOARD")].into_iter().chain(CONTROLS).enumerate() {
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

/// Where Choose A Rider puts its `k`th rider.
fn profile_mug_pos(k: usize) -> (f32, f32) {
    (PROFILE_MUG_X + (k % PROFILE_MUGS_ROW) as f32 * MUG_GAP, PROFILE_MUG_Y + (k / PROFILE_MUGS_ROW) as f32 * MUG_GAP)
}

/// Rider `i`'s mug from the `RiderMugs` page, eight 64-texel faces a row.
fn mug(commands: &mut Commands, ui: &UiAssets, i: usize, (x, y): (f32, f32), dim: bool) -> Entity {
    let corner = Vec2::new((i % 8) as f32, (i / 8) as f32) * 64.0;
    let scale = ui.mugs.size.x / 512.0;
    let rect = Rect::from_corners(corner * scale, (corner + 64.0) * scale);
    let (mut n, mut img) = ui.mugs.region(rect, MUG / rect.width());
    if dim {
        img.color = Color::srgb(0.25, 0.25, 0.25);
    }
    n.position_type = PositionType::Absolute;
    n.left = px(x);
    n.top = px(y);
    commands.spawn((n, img)).id()
}

/// Track select record panel `k`: the holder's mug, then the record's name,
/// its value and who holds it (0x4d360), filled in by
/// [`update_record_panels`].
fn record_panel(commands: &mut Commands, ui: &UiAssets, k: usize) -> Entity {
    let (mut n, img) = ui.mugs.region(Rect::new(0.0, 0.0, 1.0, 1.0), 1.0);
    n.width = px(RECORD_MUG);
    n.height = px(RECORD_MUG);
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(RECORD_X),
                top: px(RECORD_Y + k as f32 * RECORD_GAP),
                width: px(RECORD_W),
                height: px(RECORD_H),
                padding: UiRect::all(px(RECORD_PAD)),
                column_gap: px(RECORD_PAD),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(RECORD_BG),
        ))
        .with_children(|p| {
            p.spawn((n, img, Visibility::Hidden, RecordMug(k)));
            p.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), ..default() }).with_children(|c| {
                c.spawn((BitmapText::new(FontId::Small, 9.0, ""), RecordText(k, RecordLine::Kind)));
                c.spawn((BitmapText::new(FontId::Chrome, 13.0, "").color(Color::srgb(1.0, 0.85, 0.2)), RecordText(k, RecordLine::Value)));
                c.spawn((BitmapText::new(FontId::Small, 8.0, "").color(Color::srgb(0.75, 0.85, 1.0)), RecordText(k, RecordLine::Name)));
            });
        })
        .id()
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

#[allow(clippy::too_many_arguments)]
fn update_rider_panel(
    fe: Res<FrontEnd>,
    roster: Res<Roster>,
    (sel, career, defs): (Res<Selection>, Res<Career>, Res<challenge::Defs>),
    mut cursor: Query<&mut Node, (With<MugCursor>, Without<StatBar>)>,
    mut fields: Query<(&Field, &mut BitmapText)>,
    mut bars: Query<(&StatBar, &mut Node), Without<MugCursor>>,
) {
    let profile = fe.screen == Screen::ProfileRider;
    if fe.screen != Screen::Rider && !profile {
        return;
    }
    let Some(r) = roster.riders.get(fe.rider) else { return };
    if let Ok(mut n) = cursor.single_mut() {
        let (x, y) = if profile { profile_riders(&roster).iter().position(|&i| i == fe.rider).map_or((0.0, 0.0), profile_mug_pos) } else { mug_pos(fe.rider) };
        n.left = px(x - 11.0);
        n.top = px(y - 11.0);
    }
    for (f, mut t) in &mut fields {
        match f {
            Field::RiderName => set_text(&mut t, &r.name),
            Field::Born => set_text(&mut t, &format!("BORN  {}", r.born)),
            Field::From => set_text(&mut t, &format!("FROM  {}", r.from)),
            Field::Locked => {
                let why = if profile {
                    if in_profile(&fe, &r.model) { unlocks::RIDER_IN_PROFILE } else { "" }
                } else if taken(&fe, &sel, &r.model) {
                    unlocks::RIDER_TAKEN
                } else if !unlocks::rider_open(&career, &defs, r) {
                    unlocks::RIDER_LOCKED
                } else {
                    ""
                };
                set_text(&mut t, why);
            }
            _ => {}
        }
    }
    for (b, mut n) in &mut bars {
        n.width = percent(r.stats.get(b.0).copied().unwrap_or(0.0) / 5.0 * 100.0);
    }
}

fn update_quad_panel(
    fe: Res<FrontEnd>,
    roster: Res<Roster>,
    (career, rules): (Res<Career>, Res<career::Rules>),
    mut fields: Query<(&Field, &mut BitmapText)>,
    mut bars: Query<(&StatBar, &mut Node)>,
) {
    if fe.screen != Screen::Quad {
        return;
    }
    let Some(q) = roster.quads.get(fe.quad) else { return };
    for (f, mut t) in &mut fields {
        match f {
            Field::QuadName => set_text(&mut t, &q.name.to_uppercase()),
            Field::Locked => set_text(&mut t, &unlocks::atv_lock(&career, &rules, q).map_or(String::new(), unlocks::atv_locked_text)),
            _ => {}
        }
    }
    for (b, mut n) in &mut bars {
        n.width = percent(q.stats.get(b.0).copied().unwrap_or(0.0) / 5.0 * 100.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn update_track_panel(
    fe: Res<FrontEnd>,
    roster: Res<Roster>,
    sel: Res<Selection>,
    (career, rules): (Res<Career>, Res<career::Rules>),
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
            Field::Locked => {
                let locked = fe.mode != Mode::Freestyle && !unlocks::track_open(&career, &roster, &rules, t);
                set_text(&mut text, if locked { unlocks::TRACK_LOCKED } else { "" });
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

/// Shows the highlighted track's records on the track select's panels,
/// each turning over in its own time.
fn update_record_panels(
    time: Res<Time>,
    (fe, roster, records): (Res<FrontEnd>, Res<Roster>, Option<Res<TrackRecords>>),
    ui: Option<Res<UiAssets>>,
    mut texts: Query<(&RecordText, &mut BitmapText)>,
    mut mugs: Query<(&RecordMug, &mut ImageNode, &mut Visibility)>,
    mut clock: Local<f32>,
) {
    let (Some(ui), Some(records)) = (ui, records) else { return };
    if fe.screen != Screen::Track {
        *clock = 0.0;
        return;
    }
    *clock += time.delta_secs();
    let Some(track) = roster.tracks.get(fe.track) else { return };
    let shown = |k: usize| {
        let kind = records::panel_kind(k, *clock);
        (kind, records.get(track, kind))
    };
    let small = ui.font(FontId::Small);
    for (t, mut text) in &mut texts {
        let (kind, record) = shown(t.0);
        let (line, size) = match (t.1, record) {
            (RecordLine::Kind, _) => (kind.label().to_string(), 9.0),
            (RecordLine::Value, Some(r)) => (kind.long(r.value), 13.0),
            (RecordLine::Name, Some(r)) => (r.name.to_uppercase(), 8.0),
            (_, None) => (String::new(), text.size),
        };
        // A long name is drawn smaller to fit, as the original fits it
        // (0x3df20).
        let size = if t.1 == RecordLine::Value { size } else { size.min(size * RECORD_TEXT_W / small.width(&line, size).max(1.0)) };
        set_text(&mut text, &line);
        if text.size != size {
            text.size = size;
        }
    }
    for (m, mut img, mut vis) in &mut mugs {
        let rider = shown(m.0).1.and_then(|r| roster.riders.iter().position(|i| !r.rider.is_empty() && i.model.eq_ignore_ascii_case(&r.rider)));
        vis.set_if_neq(if rider.is_some() { Visibility::Inherited } else { Visibility::Hidden });
        if let Some(i) = rider {
            let corner = Vec2::new((i % 8) as f32, (i / 8) as f32) * 64.0;
            let scale = ui.mugs.size.x / 512.0;
            let rect = Some(ui.mugs.image_rect(Rect::from_corners(corner * scale, (corner + 64.0) * scale)));
            if img.rect != rect {
                img.rect = rect;
            }
        }
    }
}

pub(crate) fn blink(time: Res<Time>, mut q: Query<&mut Visibility, With<Blink>>) {
    for mut v in &mut q {
        v.set_if_neq(if (time.elapsed_secs() * 1.5).fract() < 0.65 { Visibility::Inherited } else { Visibility::Hidden });
    }
}

fn finish_loading(mut frames: Local<u32>, mut next: ResMut<NextState<AppState>>) {
    *frames += 1;
    if *frames >= 3 {
        *frames = 0;
        next.set(AppState::Race);
    }
}

/// The ATV select screen's turning 3D quad, drawn by its own camera over
/// the menu in the empty top-right of the screen; on the Trick Book, the
/// rider on their quad performing the page's trick.
#[derive(Component)]
struct Showroom {
    quad: String,
    rider: Option<String>,
}
#[derive(Component)]
struct Turntable;
#[derive(Component)]
struct ShowroomCamera;

/// Screen-unit area of the 640x480 frame the quad is shown in.
const SHOWROOM: Rect = Rect { min: Vec2::new(270.0, 64.0), max: Vec2::new(620.0, 244.0) };
/// The Trick Book's, left of the page's text.
const TRICK_VIEW: Rect = Rect { min: Vec2::new(30.0, 70.0), max: Vec2::new(340.0, 360.0) };

#[allow(clippy::too_many_arguments)]
fn showroom(
    mut commands: Commands,
    (fe, roster, sel, career): (Res<FrontEnd>, Res<Roster>, Res<Selection>, Res<Career>),
    game: Res<crate::Game>,
    shown: Query<(Entity, &Showroom)>,
    mut cams: Query<&mut Camera, With<ShowroomCamera>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
) {
    let book = fe.screen == Screen::TrickBook;
    let want = match fe.screen {
        Screen::Quad => roster.quads.get(fe.quad).map(|q| (q.name.clone(), None)),
        Screen::TrickBook => {
            let (rider, quad) = book_rider(&fe, &roster, &sel, &career);
            Some((quad, Some(rider)))
        }
        _ => None,
    };
    let current = shown.iter().next().map(|(_, s)| (s.quad.clone(), s.rider.clone()));
    if current != want {
        for (e, _) in &shown {
            commands.entity(e).despawn();
        }
        if let Some((quad, rider)) = want {
            // The rider and their tricks stand taller than the quad alone.
            let (eye, look) = if book { (Vec3::new(0.0, 1.5, 4.4), Vec3::new(0.0, 0.8, 0.0)) } else { (Vec3::new(0.0, 0.85, 2.7), Vec3::new(0.0, 0.25, 0.0)) };
            let root = commands
                .spawn((Transform::default(), Visibility::default(), Showroom { quad: quad.clone(), rider: rider.clone() }, DespawnOnExit(AppState::FrontEnd)))
                .with_children(|r| {
                    r.spawn((
                        Camera3d::default(),
                        Camera { order: 1, clear_color: ClearColorConfig::None, ..default() },
                        bevy::core_pipeline::tonemapping::Tonemapping::None,
                        Transform::from_translation(eye).looking_at(look, Vec3::Y),
                        ShowroomCamera,
                    ));
                    r.spawn((
                        DirectionalLight { illuminance: 7000.0, ..default() },
                        Transform::default().looking_to(Vec3::new(-0.4, -0.8, -0.5), Vec3::Y),
                    ));
                })
                .id();
            let generic = game.0.read("/Cars/Generic/Generic.params").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let own = game.0.read(&format!("/Cars/{quad}/{quad}.params")).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
            let params = atv_sim::VehicleParams::from_files(&generic, own.as_deref());
            let wheel_x: Vec<f32> = params.wheels.iter().map(|w| w.offset[0]).collect();
            let mut loader = crate::convert::Loader::new(&game.0, &mut meshes, &mut materials, &mut images);
            match loader.spawn_quad(&mut commands, &quad, Transform::from_xyz(0.0, crate::convert::COM_HEIGHT, 0.0), &wheel_x) {
                Ok(body) => {
                    if let Some(rider) = &rider {
                        // Seated as in a race: the riding clips are in the
                        // quad model's space, below the physics body.
                        let seat = commands.spawn((Transform::from_xyz(0.0, -crate::convert::COM_HEIGHT, 0.0), Visibility::default())).id();
                        commands.entity(body).add_child(seat);
                        match loader.spawn_rider(&mut commands, &mut bindposes, rider, seat).and_then(|rig| crate::rider::Rider::new(&game.0, rig)) {
                            Ok(r) => {
                                commands.entity(seat).insert((r, TrickDemo { clip: None, total: 1.0, t: 0.0 }));
                            }
                            Err(e) => error!("failed to load rider {rider}: {e:#}"),
                        }
                    }
                    let table = commands.spawn((Transform::from_rotation(Quat::from_rotation_y(-0.6)), Visibility::default(), Turntable)).add_child(body).id();
                    commands.entity(root).add_child(table);
                }
                Err(e) => error!("failed to load quad {quad}: {e:#}"),
            }
        }
    }
    // Keep the camera's viewport on the showroom area as the window changes.
    let Ok(w) = windows.single() else { return };
    let area = if book { TRICK_VIEW } else { SHOWROOM };
    let scale = w.height() / SCREEN_H * w.scale_factor();
    let left = (w.physical_width() as f32 - SCREEN_W * scale) / 2.0;
    for mut cam in &mut cams {
        let pos = Vec2::new(left + area.min.x * scale, area.min.y * scale).max(Vec2::ZERO);
        let size = (area.size() * scale).min(Vec2::new(w.physical_width() as f32, w.physical_height() as f32) - pos);
        if size.x >= 1.0 && size.y >= 1.0 {
            cam.viewport = Some(bevy::camera::Viewport { physical_position: pos.as_uvec2(), physical_size: size.as_uvec2(), ..default() });
        }
    }
}

/// Gives the Trick Book's rider the page's trick to perform, none while
/// it is locked (the original only shows unlocked ones).
fn demo_clip(
    fe: Res<FrontEnd>,
    roster: Res<Roster>,
    game: Res<crate::Game>,
    (career, rules): (Res<Career>, Res<career::Rules>),
    mut demos: Query<&mut TrickDemo>,
    mut clips: Local<std::collections::HashMap<String, Option<std::sync::Arc<crate::rider::Clip>>>>,
) {
    if fe.screen != Screen::TrickBook {
        return;
    }
    let (level, _) = book_level(&fe, &roster, &career, &rules);
    let def = roster.book.get(fe.trick).and_then(|&i| roster.tricks.get(i)).filter(|d| d.level <= level);
    let clip = def.and_then(|d| {
        clips
            .entry(d.clip.clone())
            .or_insert_with(|| match crate::rider::Clip::load_from(&game.0, &format!("/Animations/Stunts/Bac/{}.bac", d.clip)) {
                Ok(c) => Some(std::sync::Arc::new(c)),
                Err(e) => {
                    warn!("stunt {}: {e:#}", d.clip);
                    None
                }
            })
            .clone()
    });
    for mut d in &mut demos {
        let same = match (&d.clip, &clip) {
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            *d = TrickDemo { clip: clip.clone(), total: def.map_or(1.0, |d| d.total), t: 0.0 };
        }
    }
}

fn turntable(time: Res<Time>, mut q: Query<&mut Transform, With<Turntable>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.6);
    }
}
