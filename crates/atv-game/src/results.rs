//! A race's results page and the Race Again page, as the original's.
//!
//! The results (0x3f260, its rows 0x3ea30) cover the screen with the
//! `ResultsOverlay` bands under "Race Results" and list every racer in the
//! order they finished: "(1) Name", their time and their trick points, under
//! "Position", "Time" and "Points" (strings 0x191, 0x17b, 0x1f1, 0x178).
//! Computer riders' rows shade from yellow towards the back of the field,
//! players' are orange. Riders still out on the course are given the time
//! their pace would bring them home in, as the original brings them all home
//! when the results come up (0x813e0). Under the list one box shows the
//! race's best lap, longest jump, biggest air, best wheelie and best bicycle
//! with who set them (0x3ef60, 0x3e4a0), a different one every 2.5 seconds
//! (0x4e760). X watches the replay, for a lone rider only; A carries on
//! (0x7fe40): the record pages, then "Race Again?" with Yes and No (0x34990,
//! 0x80790).
//!
//! Time Trial has its own page (0x40080) on `ResultsOverlay2`: the rider's
//! five fastest laps under "Best Lap Times", then the track's lap record and
//! who holds it.
//!
//! Open Freestyle's (0x40620, filled by 0x3ff20) lists the session's trick
//! points, longest endo, best trick combo and what that jump scored under
//! its time limit, with the bests' box but for the best lap; it carries on
//! to "Try Again?" (0x34bc0), the same page under another title.
//!
//! A challenge's (0x55900, filled by 0x55780) has no bands: under
//! "Freestyle", whether it was passed and the medal won, and the skill
//! points it paid; "Try Again?" follows.
//!
//! A championship round's (0x3a900, filled by 0x3a160) has three views
//! that Up and Down turn through: the field by finishing order with times
//! and race points, by trick score with the trick ranking's bonuses, and by
//! championship points with the round's total and the career total.

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;

use crate::career::Career;
use crate::challenge::Outcome;
use crate::freestyle::{Session, MEDALS};
use crate::frontend::Roster;
use crate::hud::RaceMenu;
use crate::opponents::Opponents;
use crate::race::RaceProgress;
use crate::record_page::RecordPages;
use crate::records::{Feats, Kind, Record};
use crate::replay;
use crate::ui::{fullscreen, read_nav, spawn_menu_rows, two_tone, BitmapText, FontId, MenuList, Nav, UiAssets, UiTex, SCREEN_H, SCREEN_W};
use crate::{AppState, Selection};

/// Sizes in screen units: the page title (scale 0.9 in the big font,
/// 0x472e0), the column heads (0.9) and rows (0.7) in `MgrammeSmall`, the
/// box's kind (8) and name and value (7) in `MgrammeSmall` in the space of a
/// box an eighth of the screen tall, and "Race Again?" (1.25 in the big
/// font). The original sizes glyphs by their texels (0x107300 divides them
/// by 320 across and 240 down a screen 2 units each way), and
/// `MgrammeSmall`'s page is half the size of the big font's, so its text is
/// half the big font's size at the same scale: 12.42 units a unit of scale
/// after its own 1.6.
const TITLE: f32 = 31.0;
const HEAD: f32 = 17.9;
const ROW: f32 = 13.9;
const KIND: f32 = 19.9;
const LINE: f32 = 17.4;
const AGAIN: f32 = 43.0;
/// Where things sit: the title, the heads, the first row with a full field
/// of eight and the gap between rows, the columns (0x3f260), and the box
/// (0x3e560, 0x4cbf0: 60 tall, its bottom 30 below y 331).
const TITLE_Y: f32 = 56.0;
const HEAD_Y: f32 = 120.0;
const FIRST_ROW_Y: f32 = 150.0;
const ROW_GAP: f32 = 20.0;
const POSITION_X: f32 = 30.0;
const NAME_X: f32 = 25.0;
const TIME_X: f32 = 403.0;
const POINTS_X: f32 = 550.0;
const BOX_TOP: f32 = 301.0;
const BOX_H: f32 = 60.0;
const AGAIN_Y: f32 = 156.0;
const YES_Y: f32 = 276.0;
/// Time Trial's page (0x40080): the rider's name right-aligned on the
/// heads' line, the lap and record rows' columns, and the "Time" head's
/// line, where a race's first row would be with three racers (the rider and
/// the two ghost racers, 0x3e430).
const TT_NAME_X: f32 = 610.0;
const TT_LAP_X: f32 = 50.0;
const TT_TIME_X: f32 = 230.0;
const TT_HOLDER_X: f32 = 320.0;
const TT_TIME_Y: f32 = 200.0;
/// Laps listed at most.
const TT_LAPS: usize = 5;
/// Seconds each race best is shown for.
const BEST_TIME: f32 = 2.5;
/// Open Freestyle's labels, right-aligned, and values, left-aligned
/// (0x40620), and its first row: two rows under the heads.
const FS_LABEL_X: f32 = 334.0;
const FS_VALUE_X: f32 = 348.0;
const FS_FIRST_Y: f32 = HEAD_Y + 2.0 * ROW_GAP;
/// A championship round's views' value columns, centred (0x39cc0): Time
/// and Points, Trick Score and Bonuses, Total and Career Total.
const ROUND_COLUMNS: [[(f32, &str); 2]; 3] = [[(305.0, "TIME"), (450.0, "POINTS")], [(300.0, "TRICK SCORE"), (506.0, "BONUSES")], [(300.0, "TOTAL"), (506.0, "CAREER TOTAL")]];
/// A challenge's verdict and skill points (`MgrammeSmall` at 1) and the
/// top of each, and the message's box (0x55900).
const VERDICT: f32 = 19.9;
const VERDICT_Y: f32 = 84.0;
const MESSAGE_Y: f32 = 192.0;
const MESSAGE_W: f32 = 512.0;
const POINTS_Y: f32 = 336.0;
/// The time limits' names (0x40620), in [`crate::freestyle::TIME_LIMITS`]' order.
const LIMIT_NAMES: [&str; 7] = ["ONE MINUTE", "THREE MINUTES", "FIVE MINUTES", "TEN MINUTES", "FIFTEEN MINUTES", "THIRTY MINUTES", "UNLIMITED TIME"];

const GREY: Color = Color::srgb(0.7, 0.7, 0.7);
const NAVY: Color = Color::srgb(0.098, 0.192, 0.262);
const ORANGE: Color = Color::srgb(1.0, 0.5, 0.1);
const YELLOW: Color = Color::srgb(0.966, 0.966, 0.266);
const BOX_BG: Color = Color::srgb(0.15, 0.27, 0.27);
/// Time Trial's title outline.
const TT_BLUE: Color = Color::srgb(0.06, 0.09, 0.39);

/// A racer's line on the results.
#[derive(Clone, Debug, PartialEq)]
pub struct Finisher {
    pub name: String,
    /// Rider model, for their face in the box.
    pub rider: String,
    /// Race time, or the time their pace brings them home in; None for a
    /// rider who never got going.
    pub time: Option<f32>,
    pub points: u32,
    pub human: bool,
    /// Best lap, longest jump, biggest air, best wheelie and best bicycle
    /// (0 for none), in [`Kind::ALL`]'s order.
    pub feats: [f32; 5],
}

/// One of the race's bests and who set it.
#[derive(Clone, Debug, PartialEq)]
pub struct Best {
    pub kind: Kind,
    pub value: f32,
    pub name: String,
    pub rider: String,
}

/// A rider's line on a championship round's results (0x3a160).
#[derive(Clone, Debug, PartialEq)]
pub struct RoundRow {
    pub name: String,
    pub human: bool,
    /// Race time; None for a rider who didn't finish.
    pub time: Option<f32>,
    /// Race points for the finishing place.
    pub points: u32,
    pub tricks: u32,
    /// The trick ranking's points.
    pub bonus: u32,
    /// Championship points after the round.
    pub total: u32,
}

/// A championship round's three views (0x3a160), each a row per rider with
/// whether they're a player: by finishing order (`order`: row indices,
/// winner first) with time (or "DNF") and race points; by trick score, the
/// finishing order breaking ties, with the bonus; and by championship
/// points, a player first on a tie, with the round's points and the total.
pub fn round_views(rows: &[RoundRow], order: &[usize]) -> [Vec<(bool, [String; 3])>; 3] {
    let mut by_tricks = order.to_vec();
    by_tricks.sort_by_key(|&i| std::cmp::Reverse(rows.get(i).map_or(0, |r| r.tricks)));
    let mut by_total: Vec<usize> = (0..rows.len()).collect();
    by_total.sort_by_key(|&i| (std::cmp::Reverse(rows[i].total), !rows[i].human));
    let name = |n: usize, r: &RoundRow| format!("({}) {}", n + 1, r.name.to_uppercase());
    let view = |ranking: &[usize], cells: &dyn Fn(&RoundRow) -> [String; 2]| -> Vec<(bool, [String; 3])> {
        ranking
            .iter()
            .enumerate()
            .filter_map(|(n, &i)| rows.get(i).map(|r| (n, r)))
            .map(|(n, r)| {
                let [a, b] = cells(r);
                (r.human, [name(n, r), a, b])
            })
            .collect()
    };
    [
        view(order, &|r| [r.time.map_or("DNF".into(), |t| clock(Some(t))), r.points.to_string()]),
        view(&by_tricks, &|r| [r.tricks.to_string(), r.bonus.to_string()]),
        view(&by_total, &|r| [(r.points + r.bonus).to_string(), r.total.to_string()]),
    ]
}

/// Player 1's name: their profile's when they ride its rider, otherwise
/// their rider's, as anyone else goes by (0x88820, 0x88120).
pub fn player_name(sel: &Selection, roster: &Roster, career: &Career) -> String {
    let rider = || roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&sel.rider)).map_or_else(|| sel.rider.to_uppercase(), |r| r.name.clone());
    career.profile.as_ref().filter(|pr| pr.rider.eq_ignore_ascii_case(&sel.rider)).map_or_else(rider, |pr| pr.name.clone())
}

/// Everyone in a race, in the order they finished (0x3e080, by time).
pub fn finishers(p: &RaceProgress, session: Option<&Session>, feats: &Feats, sel: &Selection, roster: &Roster, career: &Career, opponents: Option<&Opponents>) -> Vec<Finisher> {
    let name = player_name(sel, roster, career);
    let jumps = |player: usize| feats.0.get(player - 1).map_or((0.0, 0.0), |j| (j.longest, j.highest));
    let line = |name: String, rider: &str, p: &RaceProgress, s: Option<&Session>, human: bool, jumps: (f32, f32)| Finisher {
        name,
        rider: rider.to_string(),
        time: home_time(p),
        points: s.map_or(0, |s| s.score),
        human,
        feats: [p.best_lap().unwrap_or(0.0), jumps.0, jumps.1, s.map_or(0.0, |s| s.longest_wheelie), s.map_or(0.0, |s| s.longest_bicycle)],
    };
    let mut out = vec![line(name, &sel.rider, p, session, true, jumps(1))];
    for r in opponents.iter().flat_map(|o| &o.racers) {
        // Only the players' jumps are measured.
        let j = r.human.map_or((0.0, 0.0), jumps);
        out.push(line(r.name.clone(), &r.model, &r.progress, r.session.as_ref(), r.human.is_some(), j));
    }
    in_order(out)
}

/// A racer's time: home, or the race time scaled by how much of the race
/// they have ridden (0x813e0).
pub fn home_time(p: &RaceProgress) -> Option<f32> {
    if let Some(t) = p.finished {
        return Some(t);
    }
    let done = (p.lap_times.len() as f32 + p.lap_fraction.clamp(0.0, 1.0)) / p.laps.max(1) as f32;
    (done > 0.0 && p.clock > 0.0).then(|| (p.clock / done.min(1.0)).max(p.clock))
}

fn in_order(mut v: Vec<Finisher>) -> Vec<Finisher> {
    v.sort_by(|a, b| a.time.unwrap_or(f32::MAX).total_cmp(&b.time.unwrap_or(f32::MAX)));
    v
}

/// The race's bests (0x3ef60): the lowest lap and the most of the rest, the
/// first to set one keeping it on a tie; none where nobody set one.
pub fn bests(finishers: &[Finisher]) -> Vec<Best> {
    Kind::ALL
        .iter()
        .enumerate()
        .filter_map(|(k, &kind)| {
            let better = |a: f32, b: f32| if kind == Kind::Lap { a < b } else { a > b };
            let mut best: Option<&Finisher> = None;
            for f in finishers.iter().filter(|f| f.feats[k] > 0.0) {
                if best.is_none_or(|b| better(f.feats[k], b.feats[k])) {
                    best = Some(f);
                }
            }
            best.map(|f| Best { kind, value: f.feats[k], name: f.name.clone(), rider: f.rider.clone() })
        })
        .collect()
}

/// A time as the results show it, "%02d:%02d:%02d" in minutes, seconds and
/// hundredths (0x3dd10). A rider who never reached the start line has no
/// pace to bring them home at (the original divides by nothing) and shows
/// dashes.
pub fn clock(t: Option<f32>) -> String {
    let Some(t) = t else { return "--:--:--".into() };
    let h = (t.max(0.0) * 100.0) as u32;
    format!("{:02}:{:02}:{:02}", h / 6000, h / 100 % 60, h % 100)
}

/// A row's colour: players orange, computer riders yellow fading down the
/// field (0x3ea30).
fn row_colour(row: usize, human: bool) -> Color {
    if human {
        return ORANGE;
    }
    let c = (0.85 - row as f32 * 0.05).max(0.0);
    Color::srgb(c, c, 0.05)
}

/// The results page up, and its box of race bests.
#[derive(Component)]
pub struct ResultsPage {
    bests: Vec<Best>,
    shown: usize,
    clock: f32,
    replay: bool,
    then: Then,
    /// A championship round's view shown (0 to 2).
    view: Option<usize>,
}

impl ResultsPage {
    fn new(bests: Vec<Best>, replay: bool) -> Self {
        Self { bests, shown: 0, clock: 0.0, replay, then: Then::RaceAgain, view: None }
    }
}

/// What follows a results page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Then {
    RaceAgain,
    /// "Try Again?", for freestyle and challenges (0x34bc0).
    TryAgain,
    /// The round's summary menu, already up and hidden under the page.
    Menu,
}

/// A championship round's column, shown in one of the page's views.
#[derive(Component)]
struct RoundView(usize);

/// The Race Again page.
#[derive(Component)]
pub struct RaceAgain;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum BestText {
    Kind,
    Name,
    Value,
}

#[derive(Component)]
struct BestMug;

pub struct ResultsPlugin;

impl Plugin for ResultsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (run, show_best).chain().run_if(in_state(AppState::Race)));
    }
}

/// A node placing text of `width` with its left edge, middle or right edge
/// (`align` 0, 1, 2) on `x`, centred down on `y`, in the 640x480 frame.
fn at(x: f32, y: f32, width: f32, size: f32, align: u8) -> Node {
    let left = match align {
        0 => x,
        1 => x - width / 2.0,
        _ => x - width,
    };
    Node { position_type: PositionType::Absolute, left: px(left), top: px(y - size / 2.0), ..default() }
}

/// A centred 640x480 frame for a page's text.
fn frame() -> Node {
    Node { position_type: PositionType::Absolute, left: percent(50), margin: UiRect::left(px(-SCREEN_W / 2.0)), width: px(SCREEN_W), height: px(SCREEN_H), ..default() }
}

/// The pad buttons along the bottom, as the front end draws them.
fn hints(parent: &mut ChildSpawnerCommands, ui: &UiAssets, hints: &[(&str, &str)]) {
    parent
        .spawn(Node { position_type: PositionType::Absolute, right: px(40), top: px(436), column_gap: px(18), align_items: AlignItems::Center, ..default() })
        .with_children(|f| {
            for (button, label) in hints {
                f.spawn(Node { align_items: AlignItems::Center, column_gap: px(5), ..default() }).with_children(|h| {
                    let s = ui.pad.get(button).rect;
                    h.spawn(ui.pad.tex.region(s, 22.0 / s.height()));
                    h.spawn(BitmapText::new(FontId::Small, 8.0, *label));
                });
            }
        });
}

/// A results page over the race: `bands`, if any, stretched over the screen, the
/// page title (0x3a850) in grey outlined in `outline`, whatever `body` puts
/// in the 640x480 frame, and the pad hints.
fn spawn_frame(commands: &mut Commands, ui: &UiAssets, bands: Option<&UiTex>, (title, outline): (&str, Color), page: ResultsPage, overlay: Option<Entity>, body: impl FnOnce(&mut ChildSpawnerCommands)) {
    let mut buttons = Vec::new();
    if page.replay {
        buttons.push(("controlbuttonsquare", "REPLAY"));
    }
    buttons.push(("controlbuttoncross", "CONTINUE"));
    commands
        .spawn((fullscreen(), page, RaceMenu { results: true, page: None }, DespawnOnExit(AppState::Race), Pickable::IGNORE))
        .insert_if(UiTargetCamera(overlay.unwrap_or(Entity::PLACEHOLDER)), || overlay.is_some())
        .with_children(|root| {
            if let Some(bands) = bands {
                let (mut n, img) = bands.node(0.0, 0.0);
                n.position_type = PositionType::Absolute;
                n.width = percent(100);
                n.height = percent(100);
                root.spawn((n, img));
            }
            root.spawn(frame()).with_children(|f| {
                let w = ui.font(FontId::Inside).width(title, TITLE);
                f.spawn(at(SCREEN_W / 2.0, TITLE_Y, w, TITLE, 1)).with_child(two_tone(TITLE, title, GREY, outline));
                body(f);
            });
            root.spawn(frame()).with_children(|f| hints(f, ui, &buttons));
        });
}

/// A line of `MgrammeSmall` text at `size` placed as [`at`] places it.
fn small_text(f: &mut ChildSpawnerCommands, ui: &UiAssets, (x, y, align): (f32, f32, u8), size: f32, text: impl Into<String>, colour: Color) {
    let text = text.into();
    let w = ui.font(FontId::Small).width(&text, size);
    f.spawn((BitmapText::new(FontId::Small, size, text).color(colour), at(x, y, w, size, align)));
}

/// The results page for `finishers`, in finishing order.
pub fn spawn_page(commands: &mut Commands, ui: &UiAssets, finishers: &[Finisher], replay: bool, overlay: Option<Entity>) {
    let first = FIRST_ROW_Y + 10.0 * (8.0 - finishers.len().min(8) as f32);
    let page = ResultsPage::new(bests(finishers), replay);
    spawn_frame(commands, ui, Some(&ui.results_overlay), ("RACE RESULTS", NAVY), page, overlay, |f| {
        for (text, x, align) in [("POSITION", POSITION_X, 0), ("TIME", TIME_X, 1), ("POINTS", POINTS_X, 1)] {
            small_text(f, ui, (x, HEAD_Y, align), HEAD, text, GREY);
        }
        for (i, r) in finishers.iter().enumerate() {
            let y = first + ROW_GAP * i as f32;
            let colour = row_colour(i, r.human);
            let cells = [(format!("({}) {}", i + 1, r.name.to_uppercase()), NAME_X, 0), (clock(r.time), TIME_X, 1), (r.points.to_string(), POINTS_X, 1)];
            for (text, x, align) in cells {
                small_text(f, ui, (x, y, align), ROW, text, colour);
            }
        }
        bests_box(f, ui);
    });
}

/// The race bests' box under a page's list, a line at a time: the face, the
/// name over the value, and what it is to the right (0x4d360). [`show_best`]
/// fills it.
fn bests_box(f: &mut ChildSpawnerCommands, ui: &UiAssets) {
    f.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(BOX_TOP), width: px(SCREEN_W), height: px(BOX_H), ..default() }, BackgroundColor(BOX_BG.with_alpha(0.0))))
        .with_children(|b| {
            let (mut m, img) = ui.mugs.region(Rect::new(0.0, 0.0, 1.0, 1.0), 1.0);
            m.position_type = PositionType::Absolute;
            m.left = px(SCREEN_W * 0.04);
            m.top = px(BOX_H * 0.05);
            m.width = px(BOX_H * 0.9);
            m.height = px(BOX_H * 0.9);
            b.spawn((m, img, Visibility::Hidden, BestMug));
            let x = SCREEN_W * 0.12;
            b.spawn((BitmapText::new(FontId::Small, LINE, "").color(YELLOW), BestText::Name, Node { position_type: PositionType::Absolute, left: px(x), top: px(BOX_H * 0.25 - LINE / 2.0), ..default() }));
            b.spawn((BitmapText::new(FontId::Small, LINE, "").color(YELLOW), BestText::Value, Node { position_type: PositionType::Absolute, left: px(x), top: px(BOX_H * 0.75 - LINE / 2.0), ..default() }));
            b.spawn((BitmapText::new(FontId::Small, KIND, "").color(ORANGE), BestText::Kind, Node { position_type: PositionType::Absolute, right: px(SCREEN_W * 0.04), top: px(BOX_H * 0.5 - KIND / 2.0), ..default() }));
        });
}

/// A Time Trial's laps as its results list them (0x3fbd0): numbered from 1,
/// fastest first, five at most.
pub fn fastest_laps(laps: &[f32]) -> Vec<(usize, f32)> {
    let mut v: Vec<(usize, f32)> = laps.iter().copied().enumerate().map(|(i, t)| (i + 1, t)).collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v.truncate(TT_LAPS);
    v
}

/// Time Trial's results (0x40080, filled by 0x3fbd0): "Best Lap Times" over
/// the rider's fastest laps, and the track's lap record with who holds it.
pub fn spawn_time_trial(commands: &mut Commands, ui: &UiAssets, name: &str, laps: &[f32], record: Option<&Record>, replay: bool, overlay: Option<Entity>) {
    let page = ResultsPage::new(Vec::new(), replay);
    let laps = fastest_laps(laps);
    spawn_frame(commands, ui, Some(&ui.results_overlay2), ("TIME TRIAL", TT_BLUE), page, overlay, |f| {
        small_text(f, ui, (NAME_X, HEAD_Y, 0), HEAD, "BEST LAP TIMES", GREY);
        small_text(f, ui, (TT_NAME_X, HEAD_Y, 2), HEAD, name.to_uppercase(), GREY);
        small_text(f, ui, (TT_TIME_X, TT_TIME_Y, 1), HEAD, "TIME", ORANGE);
        let row = |i: usize| TT_TIME_Y + ROW_GAP / 2.0 + ROW_GAP * (i + 1) as f32;
        for (i, &(n, t)) in laps.iter().enumerate() {
            small_text(f, ui, (TT_LAP_X, row(i), 0), ROW, format!("LAP {n}"), YELLOW);
            small_text(f, ui, (TT_TIME_X, row(i), 1), ROW, clock(Some(t)), YELLOW);
        }
        if let Some(r) = record {
            // The original puts it on the fifth lap's row; a fifth lap pushes
            // it down one here.
            let y = row(TT_LAPS - 1 + usize::from(laps.len() >= TT_LAPS));
            small_text(f, ui, (TT_LAP_X, y, 0), ROW, "RECORD", ORANGE);
            small_text(f, ui, (TT_TIME_X, y, 1), ROW, clock(Some(r.value)), ORANGE);
            small_text(f, ui, (TT_HOLDER_X, y, 0), ROW, r.name.to_uppercase(), ORANGE);
        }
    });
}

/// Open Freestyle's list (0x3ff20): the session's trick points, longest
/// endo, best trick combo and what that jump scored. The original has no
/// cages or pickups to count.
pub fn freestyle_rows(s: &Session) -> [(&'static str, String); 6] {
    [
        ("TRICK POINTS", s.score.to_string()),
        ("BEST ENDO", format!("{:.2} SECONDS", s.longest_endo)),
        ("CAGES", "N/A".into()),
        ("PICKUPS", "N/A".into()),
        ("BEST TRICK COMBO", format!("X {}", s.best_combo.0)),
        ("BEST TRICK", s.best_combo.1.to_string()),
    ]
}

/// Open Freestyle's results (0x40620): the time limit's name at the top,
/// [`freestyle_rows`] under it, and the box of bests, with no best lap to
/// show (0x3e560).
pub fn spawn_freestyle(commands: &mut Commands, ui: &UiAssets, session: &Session, limit: Option<usize>, bests: Vec<Best>, replay: bool, overlay: Option<Entity>) {
    let mut page = ResultsPage::new(bests.into_iter().filter(|b| b.kind != Kind::Lap).collect(), replay);
    page.then = Then::TryAgain;
    spawn_frame(commands, ui, Some(&ui.results_overlay), ("OPEN FREESTYLE", TT_BLUE), page, overlay, |f| {
        if let Some(name) = limit.and_then(|l| LIMIT_NAMES.get(l)) {
            small_text(f, ui, (FS_LABEL_X, HEAD_Y, 2), HEAD, "TIME", ORANGE);
            small_text(f, ui, (FS_VALUE_X, HEAD_Y, 0), HEAD, *name, YELLOW);
        }
        // A wider gap under the trick points.
        for (i, (label, value)) in freestyle_rows(session).into_iter().enumerate() {
            let y = FS_FIRST_Y + ROW_GAP * (i + usize::from(i > 0)) as f32;
            small_text(f, ui, (FS_LABEL_X, y, 2), ROW, label, ORANGE);
            small_text(f, ui, (FS_VALUE_X, y, 0), ROW, value, YELLOW);
        }
        bests_box(f, ui);
    });
}

/// A championship round's results (0x3a900): "Race Results" over the
/// three [`round_views`], the first showing, and the box of the race's
/// bests. Continue reveals the round's summary menu, already up under it.
pub fn spawn_round(commands: &mut Commands, ui: &UiAssets, rows: &[RoundRow], order: &[usize], bests: Vec<Best>, replay: bool, overlay: Option<Entity>) {
    let mut page = ResultsPage::new(bests, replay);
    page.then = Then::Menu;
    page.view = Some(0);
    let views = round_views(rows, order);
    let first = FIRST_ROW_Y + 10.0 * (8.0 - rows.len().min(8) as f32);
    spawn_frame(commands, ui, Some(&ui.results_overlay), ("RACE RESULTS", TT_BLUE), page, overlay, |f| {
        for (v, lines) in views.iter().enumerate() {
            let shown = if v == 0 { Visibility::Inherited } else { Visibility::Hidden };
            f.spawn((frame(), RoundView(v), shown)).with_children(|c| {
                let [(x1, h1), (x2, h2)] = ROUND_COLUMNS[v];
                for (text, x, align) in [("POSITION", NAME_X, 0), (h1, x1, 1), (h2, x2, 1)] {
                    small_text(c, ui, (x, HEAD_Y, align), HEAD, text, GREY);
                }
                for (i, (human, cells)) in lines.iter().enumerate() {
                    let y = first + ROW_GAP * i as f32;
                    let colour = row_colour(i, *human);
                    for (text, x, align) in [(&cells[0], NAME_X, 0), (&cells[1], x1, 1), (&cells[2], x2, 1)] {
                        small_text(c, ui, (x, y, align), ROW, text.clone(), colour);
                    }
                }
            });
        }
        bests_box(f, ui);
    });
}

/// A challenge's lines (0x55780): passed or failed, what the run won (the
/// medal, new or not, or no record beaten; nothing when a respot failed
/// it), and the skill points a new medal paid.
pub fn challenge_lines(o: &Outcome) -> (&'static str, String, Option<String>) {
    if o.time.is_none() {
        return ("CHALLENGE FAILED", String::new(), None);
    }
    let Some(m) = o.medal else { return ("CHALLENGE FAILED", "YOU FAILED TO BEAT ANY OF THE RECORDS.".into(), None) };
    let medal = MEDALS.get(m).copied().unwrap_or_default();
    if !o.improved {
        return ("CHALLENGE PASSED", format!("CHALLENGE COMPLETED AT {medal} LEVEL."), None);
    }
    let points = (o.skill_points > 0).then(|| format!("{} SKILL POINTS AWARDED", o.skill_points));
    ("CHALLENGE PASSED", format!("CONGRATULATIONS, YOU'VE GOT THE {medal} MEDAL!"), points)
}

/// Splits `text` into lines no wider than `width` at `size`.
fn wrap_to(ui: &UiAssets, text: &str, size: f32, width: f32) -> Vec<String> {
    let font = ui.font(FontId::Small);
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if font.width(&format!("{l} {word}"), size) <= width => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// A challenge's results (0x55900) over the race, with no bands: under
/// "Freestyle", [`challenge_lines`]: the verdict in white, the message in
/// grey in a box from x 64 to 576 hanging from y 192, and the skill points
/// in orange from y 336, then "Try Again?".
pub fn spawn_challenge(commands: &mut Commands, ui: &UiAssets, outcome: &Outcome, overlay: Option<Entity>) {
    let mut page = ResultsPage::new(Vec::new(), false);
    page.then = Then::TryAgain;
    let (verdict, message, points) = challenge_lines(outcome);
    spawn_frame(commands, ui, None, ("FREESTYLE", NAVY), page, overlay, |f| {
        small_text(f, ui, (SCREEN_W / 2.0, VERDICT_Y + VERDICT / 2.0, 1), VERDICT, verdict, Color::WHITE);
        for (i, line) in wrap_to(ui, &message, ROW, MESSAGE_W).into_iter().enumerate() {
            small_text(f, ui, (SCREEN_W / 2.0, MESSAGE_Y + ROW / 2.0 + ROW * i as f32, 1), ROW, line, GREY);
        }
        if let Some(points) = points {
            small_text(f, ui, (SCREEN_W / 2.0, POINTS_Y + VERDICT / 2.0, 1), VERDICT, points, ORANGE);
        }
    });
}

/// The Race Again page (0x34990): "Race Again?" in yellow over Yes and No;
/// with `title` "Try Again?", the page freestyle and challenges get (0x34bc0).
pub fn spawn_race_again(commands: &mut Commands, ui: &UiAssets, title: &str, overlay: Option<Entity>) {
    let list = MenuList::new(&["YES", "NO"], 272.0);
    let row = list.row;
    let menu = commands.spawn_empty().id();
    commands
        .spawn((fullscreen(), RaceAgain, RaceMenu { results: true, page: None }, DespawnOnExit(AppState::Race)))
        .insert_if(UiTargetCamera(overlay.unwrap_or(Entity::PLACEHOLDER)), || overlay.is_some())
        .with_children(|root| {
            root.spawn(frame()).with_children(|f| {
                let w = ui.font(FontId::Inside).width(title, AGAIN);
                f.spawn((BitmapText::new(FontId::Inside, AGAIN, title).color(YELLOW), Node { position_type: PositionType::Absolute, left: px((SCREEN_W - w) / 2.0), top: px(AGAIN_Y), ..default() }));
                f.spawn(Node { position_type: PositionType::Absolute, left: px((SCREEN_W - 272.0) / 2.0), top: px(YES_Y - row / 2.0), ..default() }).add_child(menu);
                hints(f, ui, &[("controlbuttoncross", "SELECT")]);
            });
        });
    spawn_menu_rows(commands, ui, menu, &list);
    commands.entity(menu).insert(list);
}

/// The results page's buttons: X for the replay, A on to the record pages
/// and then Race Again, Try Again or the round's summary; Up and Down turn
/// a championship round's views.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn run(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    ui: Option<Res<UiAssets>>,
    mut pages: Query<(Entity, &mut ResultsPage, Option<&UiTargetCamera>)>,
    mut records: Option<ResMut<RecordPages>>,
    playback: Option<Res<replay::Playback>>,
    mut views: Query<(&RoundView, &mut Visibility)>,
    mut menus: Query<&mut Visibility, (With<RaceMenu>, Without<ResultsPage>, Without<RoundView>)>,
) {
    let (Ok((e, mut page, target)), Some(ui)) = (pages.single_mut(), ui) else { return };
    if playback.is_some() || records.as_deref().is_some_and(RecordPages::open) {
        return;
    }
    let overlay = target.map(UiTargetCamera::entity);
    let then = page.then;
    let mut carry_on = |commands: &mut Commands| {
        commands.entity(e).despawn();
        match then {
            Then::RaceAgain => spawn_race_again(commands, &ui, "RACE AGAIN?", overlay),
            Then::TryAgain => spawn_race_again(commands, &ui, "TRY AGAIN?", overlay),
            Then::Menu => {
                for mut v in &mut menus {
                    *v = Visibility::Inherited;
                }
            }
        }
    };
    // The record pages are done.
    if records.as_mut().and_then(|r| r.resume()).is_some() {
        carry_on(&mut commands);
        return;
    }
    let nav = read_nav(&keys, &pads);
    if matches!(nav, Some(Nav::Accept | Nav::Start)) {
        // Each player who broke a record names it first (0x7fe40).
        if !records.as_mut().is_some_and(|r| r.hold("RACE AGAIN")) {
            carry_on(&mut commands);
        }
    } else if page.replay && (keys.just_pressed(KeyCode::KeyR) || pads.iter().any(|p| p.just_pressed(GamepadButton::West))) {
        commands.insert_resource(replay::Playback::new());
    } else if let (Some(view), Some(step @ (Nav::Up | Nav::Down))) = (page.view, nav) {
        // Up and Down turn through a championship round's views (0x3a040).
        let to = if step == Nav::Up { view.saturating_sub(1) } else { (view + 1).min(ROUND_COLUMNS.len() - 1) };
        page.view = Some(to);
        for (v, mut vis) in &mut views {
            *vis = if v.0 == to { Visibility::Inherited } else { Visibility::Hidden };
        }
    }
}

/// Turns the box to the next race best every [`BEST_TIME`] (0x4e760), and
/// keeps its line, face and all, on the one shown.
#[allow(clippy::too_many_arguments)]
fn show_best(
    time: Res<Time<Real>>,
    ui: Option<Res<UiAssets>>,
    roster: Res<Roster>,
    mut pages: Query<&mut ResultsPage>,
    mut lines: Query<(&BestText, &mut BitmapText)>,
    mut mugs: Query<(&mut ImageNode, &mut Visibility), With<BestMug>>,
    mut boxes: Query<&mut BackgroundColor>,
    parents: Query<&ChildOf, With<BestMug>>,
) {
    let (Ok(mut page), Some(ui)) = (pages.single_mut(), ui) else { return };
    if page.bests.len() > 1 {
        page.clock += time.delta_secs();
        if page.clock >= BEST_TIME {
            page.clock = 0.0;
            page.shown = (page.shown + 1) % page.bests.len();
        }
    }
    let best = page.bests.get(page.shown);
    for (which, mut t) in &mut lines {
        let text = match (which, best) {
            (_, None) => String::new(),
            (BestText::Kind, Some(b)) => b.kind.label().to_string(),
            (BestText::Name, Some(b)) => b.name.to_uppercase(),
            (BestText::Value, Some(b)) => b.kind.show(b.value),
        };
        if t.text != text {
            t.text = text;
        }
    }
    let rider = best.and_then(|b| roster.riders.iter().position(|r| !b.rider.is_empty() && r.model.eq_ignore_ascii_case(&b.rider)));
    for (mut img, mut vis) in &mut mugs {
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
    // The box shows only while it has a best to show.
    for p in &parents {
        if let Ok(mut bg) = boxes.get_mut(p.parent()) {
            let c = BOX_BG.with_alpha(if best.is_some() { 1.0 } else { 0.0 });
            if bg.0 != c {
                bg.0 = c;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finisher(name: &str, time: Option<f32>, feats: [f32; 5]) -> Finisher {
        Finisher { name: name.into(), rider: String::new(), time, points: 0, human: false, feats }
    }

    #[test]
    fn time_trial_lists_the_five_fastest_laps_by_number() {
        let laps = [95.0, 91.5, 93.0, 99.0, 90.25, 92.0];
        assert_eq!(fastest_laps(&laps), vec![(5, 90.25), (2, 91.5), (6, 92.0), (3, 93.0), (1, 95.0)]);
        assert!(fastest_laps(&[]).is_empty());
    }

    #[test]
    fn rows_go_by_time_and_times_read_as_the_original() {
        let order = in_order(vec![finisher("B", Some(95.5), [0.0; 5]), finisher("C", None, [0.0; 5]), finisher("A", Some(61.25), [0.0; 5])]);
        assert_eq!(order.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["A", "B", "C"]);
        assert_eq!(clock(Some(61.25)), "01:01:25");
        assert_eq!(clock(Some(5.0)), "00:05:00");
        assert_eq!(clock(None), "--:--:--");
    }

    #[test]
    fn riders_still_out_are_brought_home_at_their_pace() {
        let mut p = RaceProgress::new(3, true);
        assert_eq!(home_time(&p), None);
        (p.clock, p.lap_times, p.lap_fraction) = (120.0, vec![50.0, 50.0], 0.4);
        let t = home_time(&p).unwrap();
        assert!((t - 120.0 / (2.4 / 3.0)).abs() < 1e-3, "{t}");
        p.finished = Some(130.0);
        assert_eq!(home_time(&p), Some(130.0));
    }

    #[test]
    fn bests_take_the_lowest_lap_and_the_most_of_the_rest() {
        let f = [
            finisher("A", Some(60.0), [20.0, 12.0, 0.0, 1.5, 0.0]),
            finisher("B", Some(61.0), [19.5, 12.0, 3.0, 0.0, 0.0]),
            finisher("C", Some(62.0), [0.0, 30.0, 0.0, 0.0, 0.0]),
        ];
        let b = bests(&f);
        let got: Vec<(Kind, &str)> = b.iter().map(|b| (b.kind, b.name.as_str())).collect();
        assert_eq!(got, [(Kind::Lap, "B"), (Kind::LongestJump, "C"), (Kind::HighestJump, "B"), (Kind::Wheelie, "A")]);
        // A tie stays with the first to set it.
        let tie = bests(&f[..2]);
        assert_eq!((tie[1].kind, tie[1].name.as_str()), (Kind::LongestJump, "A"));
    }

    #[test]
    fn computer_rows_fade_down_the_field() {
        assert_eq!(row_colour(0, false), Color::srgb(0.85, 0.85, 0.05));
        let c = row_colour(3, false).to_srgba();
        assert!((c.red - 0.7).abs() < 1e-5 && c.red == c.green && c.blue == 0.05, "{c:?}");
        assert_eq!(row_colour(5, true), ORANGE);
    }

    #[test]
    fn a_round_lists_the_field_three_ways() {
        let row = |name: &str, human, time, points, tricks, bonus, total| RoundRow { name: name.into(), human, time, points, tricks, bonus, total };
        let rows = [
            row("Dana", true, Some(95.5), 20, 300, 25, 45),
            row("Ann", false, Some(90.0), 25, 300, 20, 45),
            row("Bo", false, None, 0, 900, 16, 60),
        ];
        let [by_time, by_tricks, by_total] = round_views(&rows, &[1, 0, 2]);
        let line = |v: &(bool, [String; 3])| v.1.join(" | ");
        assert_eq!(by_time.iter().map(line).collect::<Vec<_>>(), ["(1) ANN | 01:30:00 | 25", "(2) DANA | 01:35:50 | 20", "(3) BO | DNF | 0"]);
        // A trick-score tie goes by finishing order.
        assert_eq!(by_tricks.iter().map(line).collect::<Vec<_>>(), ["(1) BO | 900 | 16", "(2) ANN | 300 | 20", "(3) DANA | 300 | 25"]);
        // A points tie puts the player first.
        assert_eq!(by_total.iter().map(line).collect::<Vec<_>>(), ["(1) BO | 16 | 60", "(2) DANA | 45 | 45", "(3) ANN | 45 | 45"]);
        assert!(by_total[1].0);
    }

    #[test]
    fn challenge_lines_read_as_the_original() {
        let won = |medal, improved, skill_points| Outcome { time: Some(20.0), medal, improved, skill_points, ..Outcome::FAILED };
        assert_eq!(challenge_lines(&Outcome::FAILED), ("CHALLENGE FAILED", String::new(), None));
        assert_eq!(challenge_lines(&won(None, false, 0)).1, "YOU FAILED TO BEAT ANY OF THE RECORDS.");
        assert_eq!(challenge_lines(&won(Some(2), true, 1000)), ("CHALLENGE PASSED", "CONGRATULATIONS, YOU'VE GOT THE GOLD MEDAL!".into(), Some("1000 SKILL POINTS AWARDED".into())));
        assert_eq!(challenge_lines(&won(Some(0), true, 0)).2, None, "no points, no line");
        assert_eq!(challenge_lines(&won(Some(1), false, 0)).1, "CHALLENGE COMPLETED AT SILVER LEVEL.");
    }

    #[test]
    fn freestyle_rows_read_as_the_original() {
        let mut s = Session::test_with(Vec::new());
        s.score = 4930;
        s.longest_endo = 1.256;
        s.best_combo = (3, 1440);
        let rows = freestyle_rows(&s).map(|(label, value)| format!("{label} {value}"));
        assert_eq!(rows, ["TRICK POINTS 4930", "BEST ENDO 1.26 SECONDS", "CAGES N/A", "PICKUPS N/A", "BEST TRICK COMBO X 3", "BEST TRICK 1440"]);
        assert_eq!(LIMIT_NAMES.len(), crate::freestyle::TIME_LIMITS.len());
    }
}
