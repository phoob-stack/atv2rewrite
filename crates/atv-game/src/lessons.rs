//! Retail ATV Academy: ten lessons, as the retail executable runs them
//! (its TrainingSM, 0xbd480, and the lessons at 0xbd690..0xbe190). The
//! prototype's ATV Academy is free practice on Stadium 2 instead (see
//! [`crate::Mode::Training`]).
//!
//! - Control Training (game type 10) is lessons 1 to 5 on Stadium 4, Trick
//!   Training (type 11) lessons 6 to 10 on Stadium 3. Lesson N starts at
//!   `startN`.
//! - A lesson's boxes are the track's `TRBOX_LS<N>_...` locators: `RACE`
//!   (the run up), `FINISH` (the white box to stop in), `CHECK<k>` (gates
//!   the guide arrow leads through) and, in lesson 7, `DEADZONE` (the
//!   trucks' landing).
//! - Every frame (0xbe530) each box the quad is in has its say; in none of
//!   them the lesson fails ("stay inside the training area", or "stop
//!   inside the white box" once the white box was reached). Stopped (0.5
//!   m/s or slower) in `FINISH` passes it, once its task is done; reaching
//!   `FINISH` without the task done fails it.
//! - The tasks: 2 use the boost (the meter starts full; under full is
//!   used), 3 a wheelie, 4 a stoppie, 5 a bicycle, 7 boost but don't land
//!   in the dead zone (the meter starts half full), 8 a scissors, 9 a
//!   scissors and a split X, 10 a flip. The ground tricks count in `RACE`,
//!   the air ones anywhere.
//! - Lessons 1 to 5 fail on a cone knocked from their boxes; any respot
//!   fails a lesson (0x9d250).
//! - Before each lesson its page tells the task, then the controls
//!   (`ID_TR_LESSON_INTRO`, 0x9cb30); Skip Lesson goes on to the next.
//!   Passed, the next lesson follows; failed, the reason and Race Again?
//!   (0x9cea0). All five of a course passed completes it.
//!
//! The arrow floats over the quad pointing at the next `CHECK` box, as the
//! port's challenges show theirs; the original hangs it over the box.

use bevy::prelude::*;

use atv_formats::row::World;

use crate::controller::{self, Action, PadStyle, Ports};
use crate::crash::Wreck;
use crate::frontend::{FrontEnd, Screen};
use crate::knock::Knockable;
use crate::race::RaceProgress;
use crate::respot::Zone;
use crate::settings::Settings;
use crate::ui::{read_nav, spawn_menu_rows, BitmapText, FontId, MenuList, Nav, UiAssets, SCREEN_H, SCREEN_W};
use crate::{AppState, Args, Game, Mode, Player, Race, Selection};

pub const LESSONS: usize = 10;
/// Lessons in each course.
pub const PER_COURSE: usize = 5;
/// Stopped, in m/s (0x1c91cc).
const STOPPED: f32 = 0.5;

/// The course lesson `n` is in: its first lesson.
pub fn first_of(n: usize) -> usize {
    if n <= PER_COURSE { 1 } else { PER_COURSE + 1 }
}

pub fn track(n: usize) -> &'static str {
    if n <= PER_COURSE { "Stadium4" } else { "Stadium3" }
}

/// Lines of the retail `Strings_ENG.txt` (one per string key, in key
/// order: id = line).
mod line {
    /// "Lesson 1 - Acceleration": keys `..._1`, `_10`, `_2` .. `_9`.
    pub const NAMES: usize = 0x155;
    pub const SKIP_LESSON: usize = 0x29d;
    pub const RACE_AGAIN: usize = 0x25f;
    pub const TR_ADVANCED: usize = 0x2dc;
    pub const TR_BASIC: usize = 0x2dd;
    /// `TR_LESSON_10_TEXT_DESC`, `_GC`, `_PS2`, `_XBOX`, then lesson 1's,
    /// 2's .. 9's.
    pub const TEXTS: usize = 0x2e0;
    pub const FAILED: usize = 0x308;
    pub const PASSED: usize = 0x309;
    pub const PASSED_TEXT: usize = 0x30a;
    pub const FAILED_TO_FLIP: usize = 0x311;
    pub const FAILED_TO_BICYCLE: usize = 0x312;
    pub const FAILED_TO_BOOST: usize = 0x313;
    pub const FAILED_TO_FINISH: usize = 0x314;
    pub const FAILED_TO_LAND: usize = 0x316;
    pub const FAILED_TO_PRELOAD: usize = 0x317;
    pub const FAILED_TO_SCISSORS: usize = 0x318;
    pub const FAILED_TO_SPLIT_X: usize = 0x319;
    pub const FAILED_TO_STOP: usize = 0x31a;
    pub const FAILED_TO_STOPPIE: usize = 0x31b;
    pub const FAILED_TO_TRICK: usize = 0x31c;
    pub const FAILED_TO_WHEELIE: usize = 0x31d;
    pub const OUTSIDE: usize = 0x31e;
    pub const COMPLETE_TEXT: usize = 0x31f;
}

/// Where lesson `n`'s key sorts among `1`, `10`, `2` .. `9`.
fn sorted_one_first(n: usize) -> usize {
    match n {
        1 => 0,
        10 => 1,
        n => n,
    }
}

/// Where lesson `n`'s key sorts among `10`, `1`, `2` .. `9` (`_10_` before
/// `_1_`).
fn sorted_ten_first(n: usize) -> usize {
    if n == 10 { 0 } else { n }
}

/// The retail strings, one per line.
#[derive(Clone, Debug, Default)]
pub struct Strings(Vec<String>);

impl Strings {
    pub fn parse(text: &str) -> Self {
        Self(text.lines().map(|l| l.trim().to_string()).collect())
    }

    pub fn load(game: &atv_formats::GameSource) -> Self {
        Self::parse(&game.read("/Strings/Strings_ENG.txt").map(|b| b.iter().map(|&c| c as char).collect::<String>()).unwrap_or_default())
    }

    fn get(&self, line: usize) -> &str {
        self.0.get(line).map_or("", String::as_str)
    }

    /// "Lesson 1 - Acceleration".
    pub fn name(&self, n: usize) -> &str {
        self.get(line::NAMES + sorted_one_first(n))
    }

    /// The task (`TR_LESSON_N_TEXT_DESC`).
    pub fn task(&self, n: usize) -> &str {
        self.get(line::TEXTS + 4 * sorted_ten_first(n))
    }

    /// The controls, the Xbox text (`TR_LESSON_N_TEXT_XBOX`), unformatted.
    pub fn controls(&self, n: usize) -> &str {
        self.get(line::TEXTS + 4 * sorted_ten_first(n) + 3)
    }

    /// "Control Training" or "Trick Training".
    pub fn course(&self, n: usize) -> &str {
        self.get(if n <= PER_COURSE { line::TR_BASIC } else { line::TR_ADVANCED })
    }
}

/// The controls text as the original prints it (0xbeef0): `{...}` is left
/// out on the second layout (accelerating on the right stick), and `^N`
/// is a control's button. `button(i)` names control `i`: 0 accelerate, 1
/// brake, 3 boost, 4 bicycle, 6 pre-load, 8 trick button D.
pub fn format_controls(text: &str, right_stick: bool, button: impl Fn(u32) -> String) -> String {
    let mut out = String::new();
    let mut braced = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => braced = true,
            '}' => braced = false,
            _ if braced && right_stick => {}
            '^' if chars.peek().is_some_and(|d| d.is_ascii_digit()) => {
                let d = chars.next().and_then(|d| d.to_digit(10)).unwrap_or(0);
                out.push_str(&button(d));
            }
            c => out.push(c),
        }
    }
    out
}

/// Player 1's button for a control the lesson texts name.
fn button_name(settings: &Settings, style: PadStyle, i: u32) -> String {
    let action = match i {
        0 => Action::Accelerate,
        1 => Action::Brake,
        3 => Action::Boost,
        6 => Action::Hop,
        // `KeyDHeld`, the scissors' and split X's button.
        8 => Action::Trick1,
        // The port has no bicycle button: bicycles come from steering
        // hard.
        4 => return "BICYCLE".into(),
        _ => return "?".into(),
    };
    let b = settings.pads[0].bindings(0)[action.index()];
    let pad = b.pad.into_iter().flatten().next().map(|p| controller::pad_label(p, style));
    let key = b.keys.into_iter().flatten().next().map(controller::key_name);
    match (pad, key) {
        (Some(p), Some(k)) => format!("{p} ({k})"),
        (Some(p), None) => p.to_string(),
        (None, Some(k)) => k.to_string(),
        (None, None) => "-".into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Passed,
    Failed,
}

/// Why a lesson failed, when its own task doesn't say (+0x10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    Outside,
    Stop,
    Land,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Race,
    Finish,
    Deadzone,
    Check(usize),
    Other,
}

impl Kind {
    /// As the original reads the names, by what's in them (0x105860).
    fn of(name: &str) -> Self {
        let n = name.to_ascii_uppercase();
        if let Some(k) = n.split("CHECK").nth(1) {
            Kind::Check(k.trim().parse().unwrap_or(0))
        } else if n.contains("DEADZONE") {
            Kind::Deadzone
        } else if n.contains("FINISH") {
            Kind::Finish
        } else if n.contains("RACE") {
            Kind::Race
        } else {
            Kind::Other
        }
    }
}

/// What a lesson sees of the quad in one frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Frame<'a> {
    pub at: Vec3,
    pub speed: f32,
    /// The boost meter, 0..1.
    pub meter: f32,
    /// The move under way (see [`crate::freestyle::Session::current_move`]).
    pub trick: Option<&'a str>,
    pub cones_knocked: bool,
    /// The rider is off the quad.
    pub crashed: bool,
}

/// The lesson being run.
#[derive(Resource, Debug)]
pub struct Lesson {
    pub n: usize,
    boxes: Vec<(Kind, Zone)>,
    /// The lesson's cones and where each stood, once the run has begun.
    cones: Option<Vec<(Entity, Vec3)>>,
    state: Option<Verdict>,
    /// The page's verdict, once the run is over.
    pub verdict: Option<Verdict>,
    reason: Option<Reason>,
    /// Reached the white box (+0x1c).
    finish: bool,
    /// Ended by a respot (+0x20).
    respotted: bool,
    /// Coming down badly (+0x24).
    landing: bool,
    /// The task's parts done (+0x50, +0x54).
    did: [bool; 2],
    /// The last `CHECK` box passed (+0x18).
    pub check: usize,
}

impl Lesson {
    pub fn new(world: &World, n: usize) -> Self {
        let prefix = format!("TRBOX_LS{n}_");
        let boxes = world
            .locators
            .iter()
            .filter(|l| l.name.trim().to_ascii_uppercase().starts_with(&prefix))
            .map(|l| (Kind::of(l.name.trim().strip_prefix(&prefix).unwrap_or(&l.name)), Zone::of(l)))
            .collect();
        Self { n, boxes, cones: None, state: None, verdict: None, reason: None, finish: false, respotted: false, landing: false, did: [false; 2], check: 0 }
    }

    pub fn has_course(&self) -> bool {
        self.boxes.iter().any(|b| b.0 == Kind::Finish)
    }

    /// The boost meter at the start (0xbd770, 0xbda80).
    pub fn meter(&self) -> Option<f32> {
        match self.n {
            2 => Some(1.0),
            7 => Some(0.5),
            _ => None,
        }
    }

    /// The boxes whose cones count (0xbe720 ..): lessons 1 to 5 only.
    fn cone_boxes(&self) -> impl Iterator<Item = &Zone> {
        let kinds: &[Kind] = match self.n {
            2 => &[Kind::Race],
            1 | 3..=5 => &[Kind::Race, Kind::Finish],
            _ => &[],
        };
        self.boxes.iter().filter(move |b| kinds.contains(&b.0)).map(|b| &b.1)
    }

    /// The box the arrow leads to: `CHECK` one past the last passed.
    pub fn target(&self) -> Option<Vec3> {
        self.boxes.iter().find(|b| b.0 == Kind::Check(self.check + 1)).map(|b| b.1.centre())
    }

    /// One frame of the run (0xbe530) and the verdict after it, given the
    /// respots so far (0x9d250).
    pub fn step(&mut self, f: &Frame, respots: u32) -> Option<Verdict> {
        if self.verdict.is_some() {
            return self.verdict;
        }
        // The original's rider value +0x120 over 2, falling back under 0.8.
        if f.crashed {
            self.landing = true;
            self.reason = Some(Reason::Land);
        } else if self.landing {
            self.landing = false;
            self.reason = Some(Reason::Land);
        }
        let mut inside = false;
        for i in 0..self.boxes.len() {
            let (kind, zone) = self.boxes[i];
            if !zone.contains(f.at) {
                continue;
            }
            inside = true;
            if let Kind::Check(k) = kind {
                self.check = k;
            }
            self.on_box(kind, f);
        }
        if !inside {
            self.state = Some(Verdict::Failed);
            self.reason = Some(if self.finish { Reason::Stop } else { Reason::Outside });
        }
        if respots > 0 {
            self.respotted = true;
            self.verdict = Some(Verdict::Failed);
        } else {
            self.verdict = self.state;
        }
        self.verdict
    }

    /// The lessons' own rules for a box the quad is in.
    fn on_box(&mut self, kind: Kind, f: &Frame) {
        let stopped = f.speed.abs() <= STOPPED;
        let is = |names: &[&str]| f.trick.is_some_and(|t| names.contains(&t));
        let fail = Some(Verdict::Failed);
        match self.n {
            1 => {
                if f.cones_knocked {
                    self.state = fail;
                } else if kind == Kind::Finish {
                    self.finish = true;
                    if stopped {
                        self.state = Some(Verdict::Passed);
                    }
                }
            }
            2 => {
                if kind == Kind::Race {
                    if f.cones_knocked {
                        self.state = fail;
                        return;
                    }
                    if f.meter < 1.0 {
                        self.did[0] = true;
                    }
                }
                self.finish_with(kind, stopped, self.did[0]);
            }
            3..=5 => {
                let task: &[&str] = match self.n {
                    3 => &["WHEELIE"],
                    4 => &["STOPPIE"],
                    _ => &["LEFT BICYCLE", "RIGHT BICYCLE"],
                };
                if kind == Kind::Race {
                    if is(task) {
                        self.did[0] = true;
                        return;
                    }
                    if f.cones_knocked {
                        self.state = fail;
                        return;
                    }
                }
                self.finish_with(kind, stopped, self.did[0]);
            }
            7 => match kind {
                Kind::Deadzone => {
                    self.state = fail;
                    self.did[0] = true;
                }
                Kind::Race => {
                    if f.meter < 0.5 {
                        self.did[1] = true;
                    }
                }
                Kind::Finish => {
                    self.finish = true;
                    if stopped {
                        self.state = Some(Verdict::Passed);
                    }
                }
                _ => {}
            },
            8 | 10 => {
                let task: &[&str] = if self.n == 8 { &["SCISSORS"] } else { &["BACKFLIP", "FRONTFLIP"] };
                if is(task) {
                    self.did[0] = true;
                }
                self.finish_with(kind, stopped, self.did[0]);
            }
            9 => {
                if is(&["SCISSORS"]) {
                    self.did[0] = true;
                }
                if is(&["SPLIT X"]) {
                    self.did[1] = true;
                }
                self.finish_with(kind, stopped, self.did[0] && self.did[1]);
            }
            _ => {
                if kind == Kind::Finish {
                    self.finish = true;
                    if stopped {
                        self.state = Some(Verdict::Passed);
                    }
                }
            }
        }
    }

    /// In the white box: stopped with the task done passes; without it
    /// done, moving or not, fails.
    fn finish_with(&mut self, kind: Kind, stopped: bool, done: bool) {
        if kind != Kind::Finish {
            return;
        }
        self.finish = true;
        if !done {
            self.state = Some(Verdict::Failed);
        } else if stopped {
            self.state = Some(Verdict::Passed);
        }
    }

    /// Why it failed (the lessons' text at 0xbd6b0 ..): a line of
    /// `Strings_ENG.txt`, or none for a cone.
    pub fn fail_line(&self) -> Option<usize> {
        let [did0, did1] = self.did;
        let respot = self.respotted && self.reason != Some(Reason::Outside) && !(self.n > PER_COURSE && self.landing);
        if respot {
            return Some(line::FAILED_TO_FINISH);
        }
        let task = |l: usize| (!did0 && self.finish).then_some(l);
        let own = match self.n {
            2 => task(line::FAILED_TO_BOOST),
            3 => task(line::FAILED_TO_WHEELIE),
            4 => task(line::FAILED_TO_STOPPIE),
            5 => task(line::FAILED_TO_BICYCLE),
            8 => task(line::FAILED_TO_SCISSORS),
            10 => task(line::FAILED_TO_FLIP),
            7 => did0.then_some(if did1 { line::FAILED_TO_PRELOAD } else { line::FAILED_TO_BOOST }),
            9 if self.finish => match (did0, did1) {
                (true, false) => Some(line::FAILED_TO_SPLIT_X),
                (false, true) => Some(line::FAILED_TO_SCISSORS),
                (false, false) => Some(line::FAILED_TO_TRICK),
                _ => None,
            },
            _ => None,
        };
        own.or(match self.reason {
            Some(Reason::Outside) => Some(line::OUTSIDE),
            Some(Reason::Stop) => Some(line::FAILED_TO_STOP),
            Some(Reason::Land) => Some(line::FAILED_TO_LAND),
            None => None,
        })
    }
}

pub struct LessonsPlugin;

impl Plugin for LessonsPlugin {
    fn build(&self, app: &mut App) {
        let lesson = |sel: Res<Selection>| sel.mode == Mode::Training && sel.lesson > 0;
        app.add_systems(OnEnter(AppState::Race), start.after(crate::setup).run_if(lesson))
            .add_systems(FixedUpdate, follow.after(crate::drive).run_if(resource_exists::<Lesson>).run_if(in_state(AppState::Race)))
            .add_systems(FixedUpdate, crate::intro::hold.before(crate::drive).run_if(resource_exists::<Page>).run_if(in_state(AppState::Race)))
            .add_systems(Update, (conclude, guide_arrow).run_if(resource_exists::<Lesson>).run_if(in_state(AppState::Race)))
            // After the pause menu has looked at this frame's buttons.
            .add_systems(PostUpdate, (cover, crate::frontend::blink, answer).chain().run_if(resource_exists::<Page>).run_if(in_state(AppState::Race)))
            .add_systems(OnExit(AppState::Race), |mut commands: Commands| {
                commands.remove_resource::<Lesson>();
                commands.remove_resource::<Page>();
                commands.remove_resource::<IntroText>();
            });
    }
}

/// A lesson's page is up: its intro (the run waits) or its end.
#[derive(Resource, Debug)]
pub struct Page {
    kind: PageKind,
    /// The screens hidden for it, and how they were.
    hidden: Vec<(Entity, Visibility)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageKind {
    /// The task, then the controls.
    Intro(usize),
    Passed,
    /// The course is over: completed, or not all passed.
    Over,
    Failed,
}

#[derive(Component)]
struct PageRoot;

/// The guide arrow over the next `CHECK` box.
#[derive(Component)]
pub struct LessonArrow;

/// Sets the lesson up once the track is in: its boxes, the boost meter,
/// the arrow, the title and the intro page.
#[allow(clippy::too_many_arguments)]
fn start(
    mut commands: Commands,
    game: Res<Game>,
    sel: Res<Selection>,
    args: Res<Args>,
    ui: Option<Res<UiAssets>>,
    settings: Res<Settings>,
    ports: Res<Ports>,
) {
    let world = game.0.read(&format!("/Levels/{t}/World/{t}.row", t = sel.track)).ok().and_then(|b| World::parse(&b).ok());
    let Some(world) = world else { return };
    let lesson = Lesson::new(&world, sel.lesson);
    if !lesson.has_course() {
        warn!("{} has no lesson {} on the disc", sel.track, sel.lesson);
        return;
    }
    let strings = Strings::load(&game.0);
    if let Some(m) = lesson.meter() {
        commands.queue(move |w: &mut bevy::ecs::world::World| {
            if let Some(mut b) = w.get_resource_mut::<crate::boost::Boost>() {
                b.meter = m;
            }
        });
    }
    commands.insert_resource(lesson);
    let Some(ui) = ui else { return };
    // The lesson's name over the HUD.
    let head = strings.name(sel.lesson).to_uppercase();
    let width = ui.font(FontId::Menu).width(&head, 14.0);
    commands.spawn((
        BitmapText::new(FontId::Menu, 14.0, head.clone()),
        Node { position_type: PositionType::Absolute, left: percent(50), top: px(18), margin: UiRect::left(px(-width / 2.0)), ..default() },
        crate::hud::HudRoot,
        DespawnOnExit(AppState::Race),
        Pickable::IGNORE,
    ));
    if args.no_intro || args.autodrive {
        return;
    }
    let style = ports.style(0);
    let controls = format_controls(strings.controls(sel.lesson), settings.pads[0].layout == 1, |i| button_name(&settings, style, i));
    let pages = [strings.task(sel.lesson).to_string(), controls];
    open_page(&mut commands, &ui, PageKind::Intro(0), &head, &pages[0].to_uppercase(), &intro_prompt(&strings, 0));
    commands.insert_resource(IntroText { head, pages });
}

/// The intro page's two texts.
#[derive(Resource)]
struct IntroText {
    head: String,
    pages: [String; 2],
}

fn intro_prompt(strings: &Strings, page: usize) -> Vec<String> {
    let skip = strings.get(line::SKIP_LESSON).to_uppercase();
    vec![if page == 0 { "CONTINUE".into() } else { "START".into() }, if skip.is_empty() { "SKIP LESSON".into() } else { skip }]
}

/// Opens a page: its heading, its text in the original's message box, and
/// a menu under it.
fn open_page(commands: &mut Commands, ui: &UiAssets, kind: PageKind, head: &str, text: &str, items: &[String]) {
    let panel = crate::frontend::popup(commands, ui, head, text);
    // The whole screen dims, not just the page's 4:3 frame.
    commands.entity(panel).remove::<BackgroundColor>();
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let list = MenuList::new(&refs, 220.0).with_row(30.0);
    let menu = commands.spawn_empty().id();
    spawn_menu_rows(commands, ui, menu, &list);
    commands.entity(menu).insert((list, Node { position_type: PositionType::Absolute, left: px((SCREEN_W - 220.0) / 2.0), top: px(SCREEN_H - 40.0 - 30.0 * items.len() as f32), flex_direction: FlexDirection::Column, ..default() }));
    commands
        .spawn((crate::ui::fullscreen(), BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)), DespawnOnExit(AppState::Race), Pickable::IGNORE, PageRoot))
        .with_children(|root| {
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: percent(50),
                margin: UiRect::left(px(-SCREEN_W / 2.0)),
                width: px(SCREEN_W),
                height: px(SCREEN_H),
                ..default()
            })
            .add_children(&[panel, menu]);
        });
    commands.insert_resource(Page { kind, hidden: Vec::new() });
}

/// Watches the run and keeps the arrow's target.
#[allow(clippy::too_many_arguments)]
fn follow(
    mut lesson: ResMut<Lesson>,
    player: Option<Res<Player>>,
    progress: Option<Res<RaceProgress>>,
    race: Option<Res<Race>>,
    session: Option<Res<crate::freestyle::Session>>,
    boost: Option<Res<crate::boost::Boost>>,
    wreck: Option<Res<Wreck>>,
    cones: Query<(Entity, &GlobalTransform), With<Knockable>>,
) {
    let (Some(player), Some(p), Some(race)) = (player, progress, race) else { return };
    if !p.started() || lesson.verdict.is_some() {
        return;
    }
    // The cones in the lesson's boxes, as they stood when it began
    // (0xbe620); one moved from there is knocked.
    if lesson.cones.is_none() {
        let mine: Vec<(Entity, Vec3)> = cones.iter().map(|(e, t)| (e, t.translation())).filter(|(_, at)| lesson.cone_boxes().any(|z| z.contains(*at))).collect();
        lesson.cones = Some(mine);
    }
    let knocked = lesson.cones.iter().flatten().any(|&(e, was)| cones.get(e).is_ok_and(|(_, t)| t.translation().distance(was) > 0.25));
    let v = &player.0;
    let frame = Frame {
        at: v.position,
        speed: v.velocity.length(),
        meter: boost.map_or(0.0, |b| b.meter),
        trick: session.as_deref().and_then(|s| s.current_move()),
        cones_knocked: knocked,
        crashed: wreck.is_some_and(|w| w.fall.is_some()),
    };
    if let Some(verdict) = lesson.step(&frame, race.respots) {
        info!("lesson {} {verdict:?} at {:.1?}, {:.1} m/s ({:?})", lesson.n, frame.at, frame.speed, lesson.fail_line());
    }
}

/// The end page once the run is decided: the game stops behind it.
fn conclude(mut commands: Commands, lesson: Res<Lesson>, page: Option<Res<Page>>, ui: Option<Res<UiAssets>>, game: Res<Game>, mut sel: ResMut<Selection>, mut virt: ResMut<Time<Virtual>>, args: Res<Args>) {
    let (Some(verdict), None, Some(ui)) = (lesson.verdict, page, ui) else { return };
    if args.autodrive {
        return;
    }
    let strings = Strings::load(&game.0);
    virt.pause();
    let n = lesson.n;
    let last = first_of(n) + PER_COURSE - 1;
    match verdict {
        Verdict::Passed => {
            sel.lessons_passed += 1;
            if n < last {
                open_page(&mut commands, &ui, PageKind::Passed, &strings.get(line::PASSED).to_uppercase(), &strings.get(line::PASSED_TEXT).to_uppercase(), &["CONTINUE".into()]);
            } else {
                let (head, text) = completion(&strings, n, sel.lessons_passed);
                open_page(&mut commands, &ui, PageKind::Over, &head, &text, &["CONTINUE".into()]);
            }
        }
        Verdict::Failed => {
            let why = lesson.fail_line().map_or(String::new(), |l| strings.get(l).to_uppercase());
            let again = strings.get(line::RACE_AGAIN).to_uppercase();
            let text = if why.is_empty() { again } else { format!("{why} {again}") };
            open_page(&mut commands, &ui, PageKind::Failed, &strings.get(line::FAILED).to_uppercase(), &text, &["YES".into(), "NO".into()]);
        }
    }
}

/// The page after a course's last lesson (0x9cd90): all five passed
/// completes it.
fn completion(strings: &Strings, n: usize, passed: usize) -> (String, String) {
    if passed >= PER_COURSE {
        ("CONGRATULATIONS".into(), format!("{}{}.", strings.get(line::COMPLETE_TEXT), strings.course(n)).to_uppercase())
    } else {
        (strings.get(line::PASSED).to_uppercase(), strings.get(line::PASSED_TEXT).to_uppercase())
    }
}

/// Keeps the HUD hidden while a page is up.
#[allow(clippy::type_complexity)]
fn cover(mut page: ResMut<Page>, mut screens: Query<(Entity, &mut Visibility), (With<Node>, Without<ChildOf>, With<DespawnOnExit<AppState>>, Without<PageRoot>)>) {
    for (e, mut v) in &mut screens {
        if !page.hidden.iter().any(|h| h.0 == e) {
            page.hidden.push((e, *v));
            *v = Visibility::Hidden;
        }
    }
}

/// The pages' menus: the intro's Continue, Start and Skip Lesson, the
/// end's Continue or Race Again.
#[allow(clippy::too_many_arguments)]
fn answer(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&bevy::input::gamepad::Gamepad>,
    mut page: ResMut<Page>,
    pages: Query<Entity, With<PageRoot>>,
    mut screens: Query<&mut Visibility, Without<PageRoot>>,
    mut lists: Query<&mut MenuList>,
    (ui, game, text): (Option<Res<UiAssets>>, Res<Game>, Option<Res<IntroText>>),
    (mut sel, mut fe, mut next, mut virt): (ResMut<Selection>, ResMut<FrontEnd>, ResMut<NextState<AppState>>, ResMut<Time<Virtual>>),
) {
    let Some(nav) = read_nav(&keys, &pads) else { return };
    let Ok(mut list) = lists.single_mut() else { return };
    match nav {
        Nav::Up | Nav::Down => {
            list.step(nav);
            return;
        }
        Nav::Accept | Nav::Start => {}
        _ => return,
    }
    let pick = list.selected;
    for e in &pages {
        commands.entity(e).despawn();
    }
    let close = |commands: &mut Commands, page: &mut Page, screens: &mut Query<&mut Visibility, Without<PageRoot>>| {
        for (e, was) in page.hidden.drain(..) {
            if let Ok(mut v) = screens.get_mut(e) {
                *v = was;
            }
        }
        commands.remove_resource::<Page>();
    };
    let last = first_of(sel.lesson) + PER_COURSE - 1;
    let to_academy = |fe: &mut FrontEnd, next: &mut NextState<AppState>| {
        fe.return_to = Some(Screen::Training);
        next.set(AppState::FrontEnd);
    };
    match page.kind {
        PageKind::Intro(0) if pick == 0 => {
            // On to the controls.
            let (Some(ui), Some(text)) = (ui, text) else { return };
            let strings = Strings::load(&game.0);
            page.kind = PageKind::Intro(1);
            let hidden = std::mem::take(&mut page.hidden);
            open_page(&mut commands, &ui, PageKind::Intro(1), &text.head, &text.pages[1].to_uppercase(), &intro_prompt(&strings, 1));
            // `open_page` replaced the resource; carry the hidden screens.
            commands.queue(move |w: &mut bevy::ecs::world::World| {
                if let Some(mut p) = w.get_resource_mut::<Page>() {
                    p.hidden = hidden;
                }
            });
        }
        PageKind::Intro(_) if pick == 0 => close(&mut commands, &mut page, &mut screens),
        PageKind::Intro(_) => {
            // Skip Lesson: on to the next, or back to the Academy after
            // the course's last.
            close(&mut commands, &mut page, &mut screens);
            if sel.lesson < last {
                sel.lesson += 1;
                next.set(AppState::Loading);
            } else {
                to_academy(&mut fe, &mut next);
            }
        }
        PageKind::Passed => {
            virt.unpause();
            close(&mut commands, &mut page, &mut screens);
            sel.lesson += 1;
            next.set(AppState::Loading);
        }
        PageKind::Over => {
            virt.unpause();
            close(&mut commands, &mut page, &mut screens);
            to_academy(&mut fe, &mut next);
        }
        PageKind::Failed => {
            virt.unpause();
            close(&mut commands, &mut page, &mut screens);
            if pick == 0 {
                next.set(AppState::Loading);
            } else {
                to_academy(&mut fe, &mut next);
            }
        }
    }
}

/// The arrow floats over the quad and points at the next `CHECK` box;
/// hidden past the last.
fn guide_arrow(time: Res<Time>, lesson: Res<Lesson>, player: Option<Res<Player>>, mut arrows: Query<(&mut Transform, &mut Visibility), With<LessonArrow>>) {
    let Some(player) = player else { return };
    let at = player.0.position;
    for (mut t, mut vis) in &mut arrows {
        match lesson.target().filter(|_| lesson.verdict.is_none()) {
            Some(target) => {
                // The model points down its own -Z once in the Bevy frame.
                let dir = (target - at).with_y(0.0).normalize_or(Vec3::NEG_Z);
                let bob = 0.15 * (time.elapsed_secs() * 3.0).sin();
                *t = Transform::from_translation(at + Vec3::Y * (2.1 + bob)).looking_to(-dir, Vec3::Y).with_scale(Vec3::splat(0.07));
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atv_formats::rom::Locator;

    fn world(locators: &[(&str, [f32; 3], [f32; 3])]) -> World {
        let loc = |(name, pos, scale): &(&str, [f32; 3], [f32; 3])| Locator { name: name.to_string(), pos: *pos, scale: *scale, rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] };
        World { fog_color: 0, materials: vec![], material_wraps: vec![], locators: locators.iter().map(loc).collect(), lights: vec![], instances: vec![], sprites: vec![], meshes: vec![] }
    }

    /// A run up along game +Z (sim -Z) to a white box, with a gate.
    fn course(n: usize) -> Lesson {
        let w = world(&[
            (&format!("TRBOX_LS{n}_RACE"), [0.0, 0.0, 50.0], [20.0, 20.0, 100.0]),
            (&format!("TRBOX_LS{n}_FINISH"), [0.0, 0.0, 110.0], [20.0, 20.0, 20.0]),
            (&format!("TRBOX_LS{n}_CHECK1"), [0.0, 0.0, 99.0], [20.0, 20.0, 2.0]),
            (&format!("TRBOX_LS{n}_DEADZONE"), [30.0, 0.0, 50.0], [20.0, 20.0, 20.0]),
            (&format!("TRBOX_LS{}_FINISH", n + 10), [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
        ]);
        Lesson::new(&w, n)
    }

    fn at(z: f32) -> Frame<'static> {
        Frame { at: Vec3::new(0.0, 0.0, -z), speed: 10.0, ..default() }
    }

    fn stopped(z: f32) -> Frame<'static> {
        Frame { speed: 0.2, ..at(z) }
    }

    #[test]
    fn boxes_by_name() {
        let l = course(1);
        // `TRBOX_LS11_` isn't lesson 1's.
        assert_eq!(l.boxes.iter().map(|b| b.0).collect::<Vec<_>>(), [Kind::Race, Kind::Finish, Kind::Check(1), Kind::Deadzone]);
        assert!(l.has_course());
        assert_eq!(l.target(), Some(Vec3::new(0.0, 0.0, -99.0)));
    }

    #[test]
    fn stopping_in_the_white_box_passes_lesson_1() {
        let mut l = course(1);
        assert_eq!(l.step(&at(10.0), 0), None);
        // Through the white box at speed: still going.
        assert_eq!(l.step(&at(110.0), 0), None);
        assert!(l.finish);
        assert_eq!(l.step(&stopped(112.0), 0), Some(Verdict::Passed));
    }

    #[test]
    fn leaving_the_boxes_fails() {
        let mut l = course(1);
        assert_eq!(l.step(&at(-20.0), 0), Some(Verdict::Failed));
        assert_eq!(l.fail_line(), Some(line::OUTSIDE));
        // Past the white box: didn't stop in it.
        let mut l = course(1);
        l.step(&at(110.0), 0);
        assert_eq!(l.step(&at(130.0), 0), Some(Verdict::Failed));
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_STOP));
    }

    #[test]
    fn a_respot_or_a_cone_fails() {
        let mut l = course(1);
        assert_eq!(l.step(&at(10.0), 1), Some(Verdict::Failed));
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_FINISH));
        let mut l = course(1);
        assert_eq!(l.step(&Frame { cones_knocked: true, ..at(10.0) }, 0), Some(Verdict::Failed));
        assert_eq!(l.fail_line(), None);
    }

    #[test]
    fn lesson_2_wants_the_boost() {
        let mut l = course(2);
        assert_eq!(l.meter(), Some(1.0));
        assert_eq!(l.step(&Frame { meter: 1.0, ..at(10.0) }, 0), None);
        // In the white box unboosted, even moving.
        assert_eq!(l.step(&Frame { meter: 1.0, ..at(110.0) }, 0), Some(Verdict::Failed));
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_BOOST));
        let mut l = course(2);
        l.step(&Frame { meter: 0.9, ..at(10.0) }, 0);
        assert_eq!(l.step(&Frame { meter: 0.8, ..at(110.0) }, 0), None);
        assert_eq!(l.step(&Frame { meter: 0.8, ..stopped(110.0) }, 0), Some(Verdict::Passed));
    }

    #[test]
    fn ground_tricks_count_on_the_run_up() {
        let mut l = course(3);
        l.step(&Frame { trick: Some("STOPPIE"), ..at(10.0) }, 0);
        assert_eq!(l.step(&stopped(110.0), 0), Some(Verdict::Failed));
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_WHEELIE));
        let mut l = course(5);
        // A cone knocked while doing the trick doesn't count (0xbeb90).
        l.step(&Frame { trick: Some("RIGHT BICYCLE"), cones_knocked: true, ..at(10.0) }, 0);
        assert_eq!(l.step(&stopped(110.0), 0), Some(Verdict::Passed));
    }

    #[test]
    fn lesson_7_dead_zone() {
        let mut l = course(7);
        assert_eq!(l.meter(), Some(0.5));
        l.step(&Frame { meter: 0.3, ..at(10.0) }, 0);
        let dead = Frame { at: Vec3::new(30.0, 0.0, -50.0), ..at(0.0) };
        assert_eq!(l.step(&dead, 0), Some(Verdict::Failed));
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_PRELOAD));
        let mut l = course(7);
        l.step(&dead, 0);
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_BOOST));
        let mut l = course(7);
        assert_eq!(l.step(&stopped(110.0), 0), Some(Verdict::Passed));
    }

    #[test]
    fn lesson_9_wants_both_tricks() {
        let mut l = course(9);
        l.step(&Frame { trick: Some("SCISSORS"), ..at(10.0) }, 0);
        assert_eq!(l.step(&stopped(110.0), 0), Some(Verdict::Failed));
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_SPLIT_X));
        let mut l = course(9);
        l.step(&Frame { trick: Some("SCISSORS"), ..at(10.0) }, 0);
        l.step(&Frame { trick: Some("SPLIT X"), ..at(20.0) }, 0);
        assert_eq!(l.step(&at(110.0), 0), None);
        assert_eq!(l.step(&stopped(110.0), 0), Some(Verdict::Passed));
        let mut l = course(9);
        l.step(&stopped(110.0), 0);
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_TRICK));
    }

    #[test]
    fn a_crash_then_respot_in_trick_training_is_a_bad_landing() {
        let mut l = course(10);
        l.step(&Frame { crashed: true, ..at(10.0) }, 0);
        assert_eq!(l.step(&Frame { crashed: true, ..at(10.0) }, 1), Some(Verdict::Failed));
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_LAND));
        // In Control Training the respot says it.
        let mut l = course(3);
        l.step(&Frame { crashed: true, ..at(10.0) }, 1);
        assert_eq!(l.fail_line(), Some(line::FAILED_TO_FINISH));
    }

    #[test]
    fn checks_lead_the_arrow() {
        let mut l = course(6);
        l.step(&at(99.0), 0);
        assert_eq!(l.check, 1);
        assert_eq!(l.target(), None);
    }

    #[test]
    fn strings_by_key_order() {
        let mut lines = vec![String::new(); 0x320];
        for (i, n) in [1, 10, 2, 3, 4, 5, 6, 7, 8, 9].iter().enumerate() {
            lines[line::NAMES + i] = format!("Lesson {n}");
        }
        for (i, n) in [10, 1, 2, 3, 4, 5, 6, 7, 8, 9].iter().enumerate() {
            lines[line::TEXTS + 4 * i] = format!("desc {n}");
            lines[line::TEXTS + 4 * i + 3] = format!("xbox {n}");
        }
        let s = Strings::parse(&lines.join("\r\n"));
        for n in 1..=LESSONS {
            assert_eq!(s.name(n), format!("Lesson {n}"));
            assert_eq!(s.task(n), format!("desc {n}"));
            assert_eq!(s.controls(n), format!("xbox {n}"));
        }
    }

    #[test]
    fn controls_text() {
        let raw = "Accelerate using{ ^0 or} up on the right thumbstick.";
        let name = |i: u32| format!("<{i}>");
        assert_eq!(format_controls(raw, false, name), "Accelerate using <0> or up on the right thumbstick.");
        assert_eq!(format_controls(raw, true, name), "Accelerate using up on the right thumbstick.");
    }
}
