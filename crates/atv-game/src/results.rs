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

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;

use crate::career::Career;
use crate::freestyle::Session;
use crate::frontend::Roster;
use crate::hud::RaceMenu;
use crate::opponents::Opponents;
use crate::race::RaceProgress;
use crate::record_page::RecordPages;
use crate::records::{Feats, Kind};
use crate::replay;
use crate::ui::{fullscreen, read_nav, spawn_menu_rows, two_tone, BitmapText, FontId, MenuList, Nav, UiAssets, SCREEN_H, SCREEN_W};
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
/// Seconds each race best is shown for.
const BEST_TIME: f32 = 2.5;

const GREY: Color = Color::srgb(0.7, 0.7, 0.7);
const NAVY: Color = Color::srgb(0.098, 0.192, 0.262);
const ORANGE: Color = Color::srgb(1.0, 0.5, 0.1);
const YELLOW: Color = Color::srgb(0.966, 0.966, 0.266);
const BOX_BG: Color = Color::srgb(0.15, 0.27, 0.27);

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

/// Everyone in a race, in the order they finished (0x3e080, by time).
pub fn finishers(p: &RaceProgress, session: Option<&Session>, feats: &Feats, sel: &Selection, roster: &Roster, career: &Career, opponents: Option<&Opponents>) -> Vec<Finisher> {
    // The player's profile names them when they ride its rider; anyone else
    // goes by their rider's name (0x88820, 0x88120).
    let rider_name = |model: &str| roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(model)).map_or_else(|| model.to_uppercase(), |r| r.name.clone());
    let name = career.profile.as_ref().filter(|pr| pr.rider.eq_ignore_ascii_case(&sel.rider)).map_or_else(|| rider_name(&sel.rider), |pr| pr.name.clone());
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
fn home_time(p: &RaceProgress) -> Option<f32> {
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
}

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

/// The results page for `finishers`, in finishing order.
pub fn spawn_page(commands: &mut Commands, ui: &UiAssets, finishers: &[Finisher], replay: bool, overlay: Option<Entity>) {
    let small = ui.font(FontId::Small);
    let first = FIRST_ROW_Y + 10.0 * (8.0 - finishers.len().min(8) as f32);
    commands
        .spawn((fullscreen(), ResultsPage { bests: bests(finishers), shown: 0, clock: 0.0, replay }, RaceMenu { results: true, page: None }, DespawnOnExit(AppState::Race), Pickable::IGNORE))
        .insert_if(UiTargetCamera(overlay.unwrap_or(Entity::PLACEHOLDER)), || overlay.is_some())
        .with_children(|root| {
            let (mut n, img) = ui.results_overlay.node(0.0, 0.0);
            n.position_type = PositionType::Absolute;
            n.width = percent(100);
            n.height = percent(100);
            root.spawn((n, img));
            root.spawn(frame()).with_children(|f| {
                let title = "RACE RESULTS";
                let w = ui.font(FontId::Inside).width(title, TITLE);
                f.spawn(at(SCREEN_W / 2.0, TITLE_Y, w, TITLE, 1)).with_child(two_tone(TITLE, title, GREY, NAVY));
                for (text, x, align) in [("POSITION", POSITION_X, 0), ("TIME", TIME_X, 1), ("POINTS", POINTS_X, 1)] {
                    f.spawn((BitmapText::new(FontId::Small, HEAD, text).color(GREY), at(x, HEAD_Y, small.width(text, HEAD), HEAD, align)));
                }
                for (i, r) in finishers.iter().enumerate() {
                    let y = first + ROW_GAP * i as f32;
                    let colour = row_colour(i, r.human);
                    let cells = [(format!("({}) {}", i + 1, r.name.to_uppercase()), NAME_X, 0), (clock(r.time), TIME_X, 1), (r.points.to_string(), POINTS_X, 1)];
                    for (text, x, align) in cells {
                        let w = small.width(&text, ROW);
                        f.spawn((BitmapText::new(FontId::Small, ROW, text).color(colour), at(x, y, w, ROW, align)));
                    }
                }
                // The race bests' box, a line at a time: the face, the name
                // over the value, and what it is to the right (0x4d360).
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
            });
            let mut buttons = Vec::new();
            if replay {
                buttons.push(("controlbuttonsquare", "REPLAY"));
            }
            buttons.push(("controlbuttoncross", "CONTINUE"));
            root.spawn(frame()).with_children(|f| hints(f, ui, &buttons));
        });
}

/// The Race Again page (0x34990): "Race Again?" in yellow over Yes and No.
pub fn spawn_race_again(commands: &mut Commands, ui: &UiAssets, overlay: Option<Entity>) {
    let list = MenuList::new(&["YES", "NO"], 272.0);
    let row = list.row;
    let menu = commands.spawn_empty().id();
    commands
        .spawn((fullscreen(), RaceAgain, RaceMenu { results: true, page: None }, DespawnOnExit(AppState::Race)))
        .insert_if(UiTargetCamera(overlay.unwrap_or(Entity::PLACEHOLDER)), || overlay.is_some())
        .with_children(|root| {
            root.spawn(frame()).with_children(|f| {
                let text = "RACE AGAIN?";
                let w = ui.font(FontId::Inside).width(text, AGAIN);
                f.spawn((BitmapText::new(FontId::Inside, AGAIN, text).color(YELLOW), Node { position_type: PositionType::Absolute, left: px((SCREEN_W - w) / 2.0), top: px(AGAIN_Y), ..default() }));
                f.spawn(Node { position_type: PositionType::Absolute, left: px((SCREEN_W - 272.0) / 2.0), top: px(YES_Y - row / 2.0), ..default() }).add_child(menu);
                hints(f, ui, &[("controlbuttoncross", "SELECT")]);
            });
        });
    spawn_menu_rows(commands, ui, menu, &list);
    commands.entity(menu).insert(list);
}

/// The results page's buttons: X for the replay, A on to the record pages
/// and then Race Again.
#[allow(clippy::too_many_arguments)]
fn run(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    ui: Option<Res<UiAssets>>,
    pages: Query<(Entity, &ResultsPage, Option<&UiTargetCamera>)>,
    mut records: Option<ResMut<RecordPages>>,
    playback: Option<Res<replay::Playback>>,
) {
    let (Ok((e, page, target)), Some(ui)) = (pages.single(), ui) else { return };
    if playback.is_some() || records.as_deref().is_some_and(RecordPages::open) {
        return;
    }
    let overlay = target.map(UiTargetCamera::entity);
    let carry_on = |commands: &mut Commands| {
        commands.entity(e).despawn();
        spawn_race_again(commands, &ui, overlay);
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
}
