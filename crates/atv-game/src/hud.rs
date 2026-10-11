//! The in-race HUD, drawn with the original art: the rev gauge with its
//! needle, preload meter, speed and gear; lap, time and best-lap readouts;
//! the lap progress meter; the 3-2-1-GO countdown, checkpoint banner and
//! wrong-way warning; arcade's checkpoint clock; and the pause and results
//! menus for every mode, the pause menu with the original's Audio, Camera
//! and Controller pages. Freestyle's score readouts are in `freestyle`.

use bevy::input::gamepad::Gamepad;
use bevy::prelude::*;

use crate::boost::Boost;
use crate::career::{self, Award, Career, Run};
use crate::challenge::{self, Outcome};
use crate::mp;
use crate::freestyle::{limit_label, Session, MEDALS, TIME_LIMITS};
use crate::frontend::{self, FrontEnd, Roster, Screen};
use crate::opponents::{Opponents, Standings};
use crate::race::{fmt_time, RaceProgress};
use crate::records::{self, Broken, Feats, TrackRecords};
use crate::replay;
use crate::settings::{CameraView, Settings, VOLUME_STEPS};
use crate::unlocks;
use crate::ui::{fullscreen, placed, read_nav, set_text, spawn_menu_rows, BitmapText, FontId, MenuList, Nav, UiAssets};
use crate::{AppState, Mode, Player, Selection};

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        // The attract demo shows its own page, and has no menus.
        let no_demo = not(resource_exists::<crate::demo::Demo>);
        // Nor can the race be paused while the intro flies or a
        // challenge's intro page is up.
        let no_intro = not(resource_exists::<crate::intro::Intro>);
        let no_briefing = not(resource_exists::<crate::challenge::Briefing>);
        app.add_systems(OnEnter(AppState::Race), spawn_hud.run_if(no_demo.clone()))
            .add_systems(OnExit(AppState::Race), unpause)
            .add_systems(
                Update,
                (update_gauge, update_readouts, update_meter, update_banners, race_menus.run_if(no_demo).run_if(no_intro).run_if(no_briefing)).run_if(in_state(AppState::Race)),
            );
    }
}

/// Gauge scale, screen units per texel of `HUDTPAGE`.
const GAUGE: f32 = 0.8;
/// Needle angle (degrees clockwise from up) at 1000 rpm, and per 1000 rpm,
/// measured from the dial face's 1..8 markings.
const NEEDLE_AT_1K: f32 = -134.0;
const NEEDLE_PER_1K: f32 = 22.1;

#[derive(Component)]
struct NeedleHub;
#[derive(Component)]
struct PreloadArc;
/// The speed boost meter: the disc in the small dial, filling clockwise.
#[derive(Component)]
struct BoostDisc;
#[derive(Component)]
enum Readout {
    Speed,
    Gear,
    Lap,
    Time,
    LapTime,
    Best,
    /// Time Trial: the track's lap record, which a faster lap beats on
    /// the spot (0x9fa60).
    Record,
    /// Arcade: seconds left, and the round.
    TimeLeft,
    Round,
    /// Time Trial: a row of the lap list (0x9fbd0), see
    /// [`RaceProgress::recent_laps`].
    LapRow(usize),
}
#[derive(Component)]
struct BonusText;
#[derive(Component)]
struct MeterPin {
    top: f32,
    length: f32,
    /// A computer rider's marker (index into the racers), or the player's.
    opponent: Option<usize>,
    scale: f32,
}
/// The number on a lap meter marker for places the sheet has no art for.
#[derive(Component)]
struct PinLabel;
#[derive(Component)]
struct Countdown;
#[derive(Component)]
struct CheckpointBanner;
#[derive(Component)]
struct SplitText;
#[derive(Component)]
struct LapBanner;
#[derive(Component)]
struct WrongWay;
#[derive(Component)]
pub(crate) struct RaceMenu {
    results: bool,
    /// The pause menu's Audio, Camera or Controller page, when one is open.
    pub(crate) page: Option<PausePage>,
}

/// The pages the original's race menu opens (0x4a5c0): the Options pages,
/// changed mid-race.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PausePage {
    Audio,
    Camera,
    Controller,
}

impl PausePage {
    /// Its item on the pause menu.
    fn item(self) -> &'static str {
        match self {
            PausePage::Audio => "AUDIO",
            PausePage::Camera => "CAMERA",
            PausePage::Controller => "CONTROLLER",
        }
    }
}

fn spawn_hud(mut commands: Commands, assets: Option<Res<UiAssets>>, sel: Res<Selection>, roster: Res<Roster>) {
    let Some(ui) = assets else { return };
    let racing = !matches!(sel.mode, Mode::Freestyle | Mode::Challenge | Mode::Training);
    let rounds = roster.arcade.len();
    let hud = &ui.hud;
    let dial = hud.get("DIAL");
    let origin = dial.rect.min;
    let local = |p: Vec2| (p - origin) * GAUGE;
    let hub = local(dial.pivot.unwrap_or(dial.rect.center()));
    let needle = hud.get("NEEDLE");
    let needle_pivot = needle.pivot.unwrap_or(Vec2::new(needle.rect.center().x, needle.rect.max.y));
    let slot = hud.get("DIAL_PRELOAD").rect;
    let boost_slot = hud.get("DIAL_BOOST").rect;
    let dot = hud.get("WEIRD_DOT").rect;
    let dial_size = dial.rect.size() * GAUGE;

    commands
        .spawn((fullscreen(), DespawnOnExit(AppState::Race), Pickable::IGNORE))
        .with_children(|root| {
            // Rev gauge, bottom right.
            root.spawn(Node {
                position_type: PositionType::Absolute,
                right: px(14),
                bottom: px(8),
                width: px(dial_size.x),
                height: px(dial_size.y),
                ..default()
            })
            .with_children(|g| {
                g.spawn(placed(hud.node("DIAL", GAUGE), 0.0, 0.0));
                let p = local(slot.min);
                let (mut n, _) = hud.tex.region(slot, GAUGE);
                n.position_type = PositionType::Absolute;
                n.left = px(p.x);
                n.top = px(p.y);
                g.spawn((n, ImageNode::new(ui.preload_steps[0].clone()).with_mode(bevy::ui::widget::NodeImageMode::Stretch), PreloadArc, Visibility::Hidden));
                // The boost meter in the small dial, the gear over it,
                // speed in the display window.
                let p = local(boost_slot.min);
                let (mut n, _) = hud.tex.region(boost_slot, GAUGE);
                n.position_type = PositionType::Absolute;
                n.left = px(p.x);
                n.top = px(p.y);
                g.spawn((n, ImageNode::new(ui.boost_steps[0].clone()).with_mode(bevy::ui::widget::NodeImageMode::Stretch), BoostDisc, Visibility::Hidden));
                let b = local(boost_slot.center());
                g.spawn((BitmapText::new(FontId::Chrome, 15.0, "N"), centred(b.x, b.y - 8.0, 20.0), Readout::Gear));
                let speed_box = local(Vec2::new(152.0, 132.0) * hud.tex.size / Vec2::new(512.0, 256.0));
                g.spawn((
                    BitmapText::new(FontId::Chrome, 13.0, "0"),
                    Node {
                        position_type: PositionType::Absolute,
                        right: px(dial_size.x - speed_box.x),
                        top: px(speed_box.y - 6.0),
                        ..default()
                    },
                    Readout::Speed,
                ));
                // The needle turns about the dial's hub; it hangs off a
                // zero-size node there so it rotates about its own pivot.
                g.spawn((
                    Node { position_type: PositionType::Absolute, left: px(hub.x), top: px(hub.y), ..default() },
                    UiTransform::from_rotation(Rot2::degrees(NEEDLE_AT_1K - NEEDLE_PER_1K)),
                    NeedleHub,
                ))
                .with_children(|h| {
                    let off = (needle.rect.min - needle_pivot) * GAUGE;
                    h.spawn(placed(hud.tex.region(needle.rect, GAUGE), off.x, off.y));
                });
                let d = dot.size() * GAUGE * 0.7;
                let (mut n, img) = hud.tex.region(dot, GAUGE * 0.7);
                n.position_type = PositionType::Absolute;
                n.left = px(hub.x - d.x / 2.0);
                n.top = px(hub.y - d.y / 2.0);
                g.spawn((n, img));
            });

            if !racing {
                // Freestyle has its own readouts and no laps.
                spawn_messages(root, &ui);
                return;
            }
            // Lap, top left (and arcade's round under it).
            root.spawn(Node { position_type: PositionType::Absolute, left: px(24), top: px(18), flex_direction: FlexDirection::Column, row_gap: px(4), ..default() })
                .with_children(|c| {
                    c.spawn(Node { align_items: AlignItems::FlexEnd, column_gap: px(6), ..default() }).with_children(|r| {
                        r.spawn(BitmapText::new(FontId::Small, 9.0, "LAP"));
                        r.spawn((BitmapText::new(FontId::Chrome, 22.0, "1/3"), Readout::Lap));
                    });
                    if sel.mode == Mode::Arcade {
                        c.spawn((BitmapText::new(FontId::Small, 8.0, format!("ROUND {}/{rounds}", sel.round + 1)).color(Color::srgb(0.75, 0.85, 1.0)), Readout::Round));
                    }
                });

            // Arcade's clock, top centre.
            if sel.mode == Mode::Arcade {
                root.spawn(Node {
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    top: px(14),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: px(2),
                    ..default()
                })
                .with_children(|c| {
                    c.spawn(BitmapText::new(FontId::Small, 8.0, "TIME LEFT"));
                    c.spawn((BitmapText::new(FontId::Chrome, 26.0, "0:00.00"), Readout::TimeLeft));
                    c.spawn((BitmapText::new(FontId::Chrome, 13.0, "").color(Color::srgb(0.5, 1.0, 0.45)), BonusText));
                });
            }

            // Times, top right.
            root.spawn(Node {
                position_type: PositionType::Absolute,
                right: px(24),
                top: px(18),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexEnd,
                row_gap: px(4),
                ..default()
            })
            .with_children(|c| {
                // Time Trial lists the record, the best lap and the last
                // laps, the one being ridden at the bottom, with the
                // "Current" label on the first row (0xa0660).
                let rows: Vec<_> = if sel.mode == Mode::TimeTrial {
                    [("TIME", 18.0, Readout::Time), ("RECORD", 11.0, Readout::Record), ("BEST", 11.0, Readout::Best)]
                        .into_iter()
                        .chain((0..4).map(|i| (if i == 0 { "CURRENT" } else { "" }, 11.0, Readout::LapRow(i))))
                        .collect()
                } else {
                    vec![("TIME", 18.0, Readout::Time), ("LAP", 11.0, Readout::LapTime), ("BEST", 11.0, Readout::Best)]
                };
                for (label, size, readout) in rows {
                    c.spawn(Node { align_items: AlignItems::FlexEnd, column_gap: px(6), ..default() }).with_children(|r| {
                        r.spawn(BitmapText::new(FontId::Small, 8.0, label));
                        r.spawn((BitmapText::new(FontId::Chrome, size, "0:00.00"), readout));
                    });
                }
            });

            // Lap progress meter down the right edge: start at the bottom,
            // chequered flag at the top, the rider's marker in between.
            let m = &ui.meter;
            let line = m.get("line").rect;
            let ms = 0.42;
            let (top, length) = (112.0, line.height() * ms);
            root.spawn(Node { position_type: PositionType::Absolute, right: px(30), top: px(top), width: px(30), height: px(length), ..default() })
                .with_children(|c| {
                    c.spawn(placed(m.node("line", ms), 15.0 - line.width() * ms / 2.0, 0.0));
                    let f = m.get("flag").rect.size() * ms * 1.2;
                    c.spawn(placed(m.node("flag", ms * 1.2), 15.0 - f.x / 2.0, -f.y - 2.0));
                    // Each rider's marker shows their place; the computer
                    // riders' are smaller and under the player's.
                    for (opponent, scale) in (0..7).map(|i| (Some(i), 0.16)).chain([(None, 0.24)]) {
                        let p = ui.positions.get("P1").rect.size() * scale;
                        let (n, img) = ui.positions.node("P1", scale);
                        let pin = MeterPin { top: -p.y / 2.0, length, opponent, scale };
                        let vis = if opponent.is_some() { Visibility::Hidden } else { Visibility::Inherited };
                        let mut n = placed((n, img), 15.0 - p.x / 2.0, length - p.y / 2.0);
                        n.0.border_radius = BorderRadius::MAX;
                        n.0.justify_content = JustifyContent::Center;
                        n.0.align_items = AlignItems::Center;
                        c.spawn((n, pin, vis, BackgroundColor(Color::NONE))).with_child((BitmapText::new(FontId::Chrome, 64.0 * scale * 0.6, ""), PinLabel));
                    }
                });

            spawn_messages(root, &ui);
        });
}

/// Centre-screen messages: the countdown, checkpoint banner, lap and
/// finish banners and the wrong-way warning.
fn spawn_messages(root: &mut ChildSpawnerCommands, ui: &UiAssets) {
    root.spawn((Node { position_type: PositionType::Absolute, width: percent(100), top: px(150), justify_content: JustifyContent::Center, ..default() },))
        .with_children(|c| {
            let (n, img) = ui.countdown.node("three", 1.0);
            c.spawn((n, img, Countdown, UiTransform::default(), Visibility::Hidden));
        });
    root.spawn(Node {
        position_type: PositionType::Absolute,
        width: percent(100),
        top: px(92),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: px(6),
        ..default()
    })
    .with_children(|c| {
        c.spawn((ui.meter.node("checkpoint", 1.25), CheckpointBanner, Visibility::Hidden));
        c.spawn((BitmapText::new(FontId::Chrome, 14.0, ""), SplitText));
        c.spawn((BitmapText::new(FontId::Menu, 20.0, ""), LapBanner));
        c.spawn((BitmapText::new(FontId::Menu, 24.0, "WRONG WAY").color(Color::srgb(1.0, 0.25, 0.1)), WrongWay, Visibility::Hidden));
    });
}

/// A node centred on (x, y), `w` units wide.
fn centred(x: f32, y: f32, w: f32) -> Node {
    Node { position_type: PositionType::Absolute, left: px(x - w / 2.0), top: px(y), width: px(w), justify_content: JustifyContent::Center, ..default() }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_gauge(
    player: Option<Res<Player>>,
    ui: Option<Res<UiAssets>>,
    boost: Option<Res<Boost>>,
    mut needle: Query<&mut UiTransform, With<NeedleHub>>,
    mut arc: Query<(&mut ImageNode, &mut Visibility), (With<PreloadArc>, Without<BoostDisc>)>,
    mut disc: Query<(&mut ImageNode, &mut Visibility), With<BoostDisc>>,
    (mut shown, mut pulse): (Local<f32>, Local<f32>),
    time: Res<Time>,
) {
    let (Some(player), Some(ui)) = (player, ui) else { return };
    let v = &player.0;
    // A little needle lag, like a real gauge.
    let k = 1.0 - (-14.0 * time.delta_secs()).exp();
    *shown += (v.revs.clamp(0.0, 8600.0) - *shown) * k;
    if let Ok(mut t) = needle.single_mut() {
        t.rotation = Rot2::degrees(NEEDLE_AT_1K + (*shown / 1000.0 - 1.0) * NEEDLE_PER_1K);
    }
    if let Ok((mut img, mut vis)) = arc.single_mut() {
        let n = ui.preload_steps.len();
        let step = (v.preload.clamp(0.0, 1.0) * n as f32).round() as usize;
        vis.set_if_neq(if step == 0 { Visibility::Hidden } else { Visibility::Inherited });
        if step > 0 && img.image != ui.preload_steps[step - 1] {
            img.image = ui.preload_steps[step - 1].clone();
        }
    }
    // The boost disc throbs, its alpha the sine of a phase going round
    // at 2 a second over half a turn (0xa2ee0).
    *pulse += 2.0 * time.delta_secs();
    if *pulse > std::f32::consts::PI {
        *pulse -= std::f32::consts::PI;
    }
    if let (Ok((mut img, mut vis)), Some(b)) = (disc.single_mut(), boost) {
        let n = ui.boost_steps.len();
        let step = (b.meter.clamp(0.0, 1.0) * n as f32).ceil() as usize;
        vis.set_if_neq(if step == 0 { Visibility::Hidden } else { Visibility::Inherited });
        if step > 0 && img.image != ui.boost_steps[step - 1] {
            img.image = ui.boost_steps[step - 1].clone();
        }
        img.color = Color::srgba(1.0, 1.0, 1.0, pulse.sin());
    }
}

fn update_readouts(
    player: Option<Res<Player>>,
    progress: Option<Res<RaceProgress>>,
    (records, sel): (Option<Res<TrackRecords>>, Res<Selection>),
    mut q: Query<(&Readout, &mut BitmapText), Without<BonusText>>,
    mut bonus: Query<&mut BitmapText, With<BonusText>>,
) {
    let (Some(player), Some(p)) = (player, progress) else { return };
    if let (Ok(mut t), Some(a)) = (bonus.single_mut(), p.arcade.as_ref()) {
        let s = match a.last_bonus {
            Some((at, secs)) if p.clock - at < 2.0 => format!("+{}", fmt_time(secs)),
            _ => String::new(),
        };
        set_text(&mut t, &s);
    }
    let v = &player.0;
    let laps = if sel.mode == Mode::TimeTrial { p.recent_laps() } else { Vec::new() };
    for (r, mut t) in &mut q {
        let s = match r {
            // The dial face reads KPH.
            Readout::Speed => format!("{:.0}", v.velocity.length() * 3.6),
            Readout::Gear => match v.gear {
                0 => "R".into(),
                g => g.to_string(),
            },
            Readout::Lap => format!("{}/{}", p.lap(), p.laps),
            Readout::Time => fmt_time(p.finished.unwrap_or(p.clock)),
            Readout::LapTime => fmt_time(if p.started() { p.lap_time() } else { 0.0 }),
            Readout::Best => p.best_lap().map_or("-:--.--".into(), fmt_time),
            Readout::Record => {
                let record = records.as_deref().and_then(|r| r.get(&sel.track, records::Kind::Lap)).map(|r| r.value);
                record.into_iter().chain(p.best_lap()).reduce(f32::min).map_or("-:--.--".into(), fmt_time)
            }
            Readout::TimeLeft => {
                let left = p.arcade.as_ref().map_or(0.0, |a| a.left);
                // Red in the last ten seconds.
                let c = if left < 10.0 && p.started() { Color::srgb(1.0, 0.3, 0.2) } else { Color::WHITE };
                if t.color != c {
                    t.color = c;
                }
                fmt_time(left)
            }
            Readout::Round => continue,
            Readout::LapRow(i) => match laps.get(*i) {
                Some(&(time, current)) => {
                    // The lap being ridden is yellow.
                    let c = if current { Color::srgb(1.0, 1.0, 0.0) } else { Color::WHITE };
                    if t.color != c {
                        t.color = c;
                    }
                    fmt_time(time)
                }
                None => String::new(),
            },
        };
        set_text(&mut t, &s);
    }
}

#[allow(clippy::type_complexity)]
fn update_meter(
    progress: Option<Res<RaceProgress>>,
    standings: Option<Res<Standings>>,
    ui: Option<Res<UiAssets>>,
    mut pins: Query<(
        &MeterPin,
        &Children,
        &mut Node,
        &mut ImageNode,
        &mut BackgroundColor,
        &mut Visibility,
    )>,
    mut labels: Query<&mut BitmapText, With<PinLabel>>,
) {
    let (Some(p), Some(ui)) = (progress, ui) else {
        return;
    };
    let mine = if p.finished.is_some() {
        1.0
    } else {
        p.lap_fraction.clamp(0.0, 1.0)
    };
    for (pin, children, mut n, mut img, mut bg, mut vis) in &mut pins {
        let (place, f) = match (pin.opponent, standings.as_deref()) {
            (None, s) => (s.filter(|s| s.racers > 1).map_or(1, |s| s.player), mine),
            (Some(i), Some(s)) => match s.opponents.get(i) {
                Some(&(place, f)) => (place, f.clamp(0.0, 1.0)),
                None => {
                    vis.set_if_neq(Visibility::Hidden);
                    continue;
                }
            },
            (Some(_), None) => {
                vis.set_if_neq(Visibility::Hidden);
                continue;
            }
        };
        vis.set_if_neq(Visibility::Inherited);
        // The sheet only has badges for 1st to 4th (its P5-P8 cells are
        // empty); lower places get the number on a dark disc.
        let badge = place <= 4;
        let rect = ui.positions.get(&format!("P{}", place.clamp(1, 4))).rect;
        if img.rect != Some(rect) {
            img.rect = Some(rect);
            n.width = px(rect.width() * pin.scale);
            n.height = px(rect.height() * pin.scale);
        }
        let alpha = if pin.opponent.is_some() { 0.75 } else { 1.0 };
        let (tint, disc) = if badge {
            (Color::srgba(1.0, 1.0, 1.0, alpha), Color::NONE)
        } else {
            (Color::NONE, Color::srgba(0.1, 0.1, 0.15, alpha))
        };
        if img.color != tint {
            img.color = tint;
            bg.0 = disc;
        }
        for c in children.iter() {
            if let Ok(mut t) = labels.get_mut(c) {
                set_text(
                    &mut t,
                    &if badge {
                        String::new()
                    } else {
                        place.to_string()
                    },
                );
            }
        }
        n.top = px(pin.top + pin.length * (1.0 - f));
    }
}

/// `1ST`, `2ND`, ...
/// The career field's finishing order (indices into the run's field) from
/// the running order: the player, then each computer rider (or split-screen
/// player) by its place; anyone missing from the race comes last.
fn finishing_order(run: &Run, dnf: bool, (standings, opponents): (Option<&Standings>, Option<&Opponents>)) -> Vec<usize> {
    let mut placed: Vec<(usize, usize)> = run
        .field
        .iter()
        .enumerate()
        .map(|(i, e)| {
            // Player 1 is first in the field; other players race as racers.
            let p1 = e.player && i == 0;
            let place = if p1 && dnf {
                // Didn't finish: last of those placed.
                99
            } else if p1 {
                standings.map_or(1, |s| s.player)
            } else {
                opponents
                    .and_then(|o| o.racers.iter().position(|r| r.model.eq_ignore_ascii_case(&e.model)))
                    .and_then(|k| standings?.opponents.get(k).map(|o| o.0))
                    .unwrap_or(100 + i)
            };
            (place, i)
        })
        .collect();
    placed.sort();
    placed.into_iter().map(|(_, i)| i).collect()
}

/// Everyone's trick score in a championship round, by field index: player
/// 1's, and the other players' and computer riders' from their own tricks.
fn trick_scores(run: &Run, session: Option<&Session>, opponents: Option<&Opponents>) -> Vec<u32> {
    run.field
        .iter()
        .enumerate()
        .map(|(i, e)| match opponents {
            _ if e.player && i == 0 => session.map_or(0, |s| s.score),
            Some(o) => o.racers.iter().find(|r| r.model.eq_ignore_ascii_case(&e.model)).and_then(|r| r.session.as_ref()).map_or(0, |s| s.score),
            None => 0,
        })
        .collect()
}

/// Each human's race for the track records: player 1's, then the other
/// split-screen players'. A record takes the profile's name, or in split
/// screen each player's rider's, as the original takes each controller's
/// player name there (0x14660).
fn record_entries(
    p: &RaceProgress,
    session: Option<&Session>,
    feats: &Feats,
    sel: &Selection,
    roster: &Roster,
    career: &Career,
    opponents: Option<&Opponents>,
) -> Vec<records::Entry> {
    let rider_name = roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&sel.rider)).map_or_else(|| sel.rider.to_uppercase(), |r| r.name.clone());
    let name = career.profile.as_ref().filter(|_| sel.players <= 1).map_or(rider_name, |pr| pr.name.clone());
    let entry = |player: usize, name: String, rider: &str, quad: &str, p: &RaceProgress, s: Option<&Session>| {
        let jumps = feats.0.get(player - 1).copied().unwrap_or_default();
        records::Entry {
            player,
            name,
            rider: rider.to_string(),
            quad: quad.to_string(),
            best_lap: p.best_lap(),
            longest_jump: jumps.longest,
            highest_jump: jumps.highest,
            wheelie: s.map_or(0.0, |s| s.longest_wheelie),
            bicycle: s.map_or(0.0, |s| s.longest_bicycle),
            score: s.map_or(0, |s| s.score),
        }
    };
    let mut out = vec![entry(1, name, &sel.rider, &sel.quad, p, session)];
    for r in opponents.iter().flat_map(|o| &o.racers) {
        if let Some(n) = r.human.filter(|&n| n >= 2) {
            let quad = sel.others.get(n - 2).map_or("", |o| o.1.as_str());
            out.push(entry(n, r.name.clone(), &r.model, quad, &r.progress, r.session.as_ref()));
        }
    }
    out
}

pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "TH",
        (1, _) => "ST",
        (2, _) => "ND",
        (3, _) => "RD",
        _ => "TH",
    };
    format!("{n}{suffix}")
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_banners(
    progress: Option<Res<RaceProgress>>,
    session: Option<Res<Session>>,
    ui: Option<Res<UiAssets>>,
    mut countdown: Query<(&mut ImageNode, &mut Node, &mut UiTransform, &mut Visibility), With<Countdown>>,
    mut banner: Query<&mut Visibility, (With<CheckpointBanner>, Without<Countdown>)>,
    mut wrong: Query<(&mut BitmapText, &mut Visibility), (With<WrongWay>, Without<Countdown>, Without<CheckpointBanner>, Without<SplitText>, Without<LapBanner>)>,
    mut split: Query<&mut BitmapText, (With<SplitText>, Without<LapBanner>, Without<WrongWay>)>,
    mut lap: Query<&mut BitmapText, (With<LapBanner>, Without<SplitText>, Without<WrongWay>)>,
    menus: Query<(), With<RaceMenu>>,
) {
    let (Some(p), Some(ui)) = (progress, ui) else { return };
    let quiet = !menus.is_empty();
    // 3, 2, 1 for a second each, then GO for one more.
    if let Ok((mut img, mut node, mut t, mut vis)) = countdown.single_mut() {
        let c = p.countdown;
        let (name, phase) = if c > 2.0 {
            (Some("three"), 3.0 - c)
        } else if c > 1.0 {
            (Some("two"), 2.0 - c)
        } else if c > 0.0 {
            (Some("one"), 1.0 - c)
        } else if p.clock < 1.0 && !p.over() {
            (Some("go"), p.clock)
        } else {
            (None, 0.0)
        };
        match name.filter(|_| !quiet) {
            Some(name) => {
                let s = ui.countdown.get(name);
                if img.rect != Some(ui.countdown.tex.image_rect(s.rect)) {
                    img.rect = Some(ui.countdown.tex.image_rect(s.rect));
                    node.width = px(s.rect.width());
                    node.height = px(s.rect.height());
                }
                // Pops in large and settles, fading at the end.
                let phase = phase.clamp(0.0, 1.0);
                t.scale = Vec2::splat(1.0 + 0.6 * (1.0 - phase).powi(3));
                img.color = Color::srgba(1.0, 1.0, 1.0, (1.0 - phase).min(0.25) * 4.0);
                vis.set_if_neq(Visibility::Inherited);
            }
            None => {
                vis.set_if_neq(Visibility::Hidden);
            }
        }
    }
    let recent = |at: Option<f32>, secs: f32| at.is_some_and(|t| p.clock - t < secs);
    let lap_done = recent(p.last_lap, 2.5) && p.finished.is_none();
    if let Ok(mut vis) = banner.single_mut() {
        vis.set_if_neq(if (recent(p.last_checkpoint, 1.5) || lap_done) && !quiet { Visibility::Inherited } else { Visibility::Hidden });
    }
    if let Ok(mut t) = split.single_mut() {
        let s = if quiet {
            String::new()
        } else if lap_done {
            p.lap_times.last().map(|&l| fmt_time(l)).unwrap_or_default()
        } else if recent(p.last_checkpoint, 1.5) {
            fmt_time(p.lap_time())
        } else {
            String::new()
        };
        set_text(&mut t, &s);
    }
    let time_up = session.as_ref().is_some_and(|s| s.freestyle && s.over(p.clock));
    if let Ok(mut t) = lap.single_mut() {
        let s = if quiet {
            String::new()
        } else if time_up {
            "TIME UP".to_string()
        } else if p.out_of_time() {
            "OUT OF TIME".to_string()
        } else if p.dnf {
            "DID NOT FINISH".to_string()
        } else if p.finished.is_some() {
            "FINISHED".to_string()
        } else if lap_done && p.lap() == p.laps {
            "FINAL LAP".to_string()
        } else if lap_done {
            format!("LAP {}", p.lap())
        } else {
            String::new()
        };
        set_text(&mut t, &s);
    }
    if let Ok((mut t, mut vis)) = wrong.single_mut() {
        // The original's warnings, held steady (0x902f0).
        let text = p.warning.clone().or_else(|| (p.wrong_way > crate::respot::WRONG_WAY).then(|| "WRONG WAY".to_string())).filter(|_| p.racing());
        if let Some(text) = &text {
            set_text(&mut t, text);
        }
        vis.set_if_neq(if text.is_some() { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// Escape / Start pauses the race; finishing shows the results. Both are
/// menus over the frozen race.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn race_menus(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    ui: Option<Res<UiAssets>>,
    progress: Option<Res<RaceProgress>>,
    mut session: Option<ResMut<Session>>,
    mut selection: ResMut<Selection>,
    roster: Res<Roster>,
    mut virt: ResMut<Time<Virtual>>,
    menus: Query<(Entity, &RaceMenu)>,
    mut lists: Query<&mut MenuList>,
    mut next: ResMut<NextState<AppState>>,
    mut finished_for: Local<f32>,
    field: (Option<Res<Standings>>, Option<Res<Opponents>>),
    (mut career, rules, mut fe, mut settings, (mut records, feats, demo)): (
        ResMut<Career>,
        Res<career::Rules>,
        ResMut<FrontEnd>,
        ResMut<Settings>,
        (Option<ResMut<TrackRecords>>, Res<Feats>, Option<Res<crate::demo::Demo>>),
    ),
    (defs, course, overlay, mut mp_champ, replay): (Res<challenge::Defs>, Option<Res<challenge::Course>>, Query<Entity, With<mp::Overlay>>, ResMut<mp::MpChamp>, Option<Res<replay::Playback>>),
) {
    // The replay has the controls while it plays.
    if replay.is_some() {
        return;
    }
    let overlay = overlay.iter().next();
    let (Some(ui), Some(p)) = (ui, progress) else { return };
    let field = (field.0.as_deref(), field.1.as_deref());
    let nav = read_nav(&keys, &pads);
    // Results come up a moment after the line (or the clock running out).
    let over = match session.as_deref() {
        Some(s) if s.freestyle => s.over(p.clock),
        // Split screen waits for the other players.
        _ if selection.players > 1 => mp::all_home(&p, field.1),
        _ => p.over(),
    };
    *finished_for = if over { *finished_for + virt.delta_secs() } else { 0.0 };

    let (Ok((e, menu)), Ok(mut list)) = (menus.single(), lists.single_mut()) else {
        // Escape or Start pauses (Backspace is the quick restart).
        if (keys.just_pressed(KeyCode::Escape) || nav == Some(Nav::Start)) && !over {
            virt.pause();
            spawn_race_menu(&mut commands, &ui, false, &p, session.as_deref(), &selection, &roster, field, (None, None, None, None, &[]), overlay, "RETURN TO RACE");
        } else if *finished_for > 2.5 && menus.is_empty() {
            let award = (selection.mode == Mode::Career).then(|| {
                let run = career.run.clone()?;
                let order = finishing_order(&run, p.dnf, field);
                let tricks = trick_scores(&run, session.as_deref(), field.1);
                let award = career.score(&rules, &selection.track, &order, &tricks);
                career.save();
                award
            });
            let award = award.flatten();
            // A split-screen championship round.
            let champ_round = selection.players > 1 && selection.mode == Mode::SingleRace && mp_champ.run.is_some();
            if champ_round && let Some(run) = mp_champ.run.clone() {
                let order = finishing_order(&run, p.dnf, field);
                let tricks = trick_scores(&run, session.as_deref(), field.1);
                mp_champ.score(&rules, &order, &tricks);
                mp_champ.keep(&mut career);
            }
            // A challenge run: its medal and bonus, or the respot that
            // failed it.
            let outcome = (selection.mode == Mode::Challenge)
                .then(|| {
                    if course.as_ref().is_some_and(|c| c.failed) {
                        return Some(Outcome::FAILED);
                    }
                    let (time, def) = (p.finished?, defs.get(selection.challenge)?);
                    let o = challenge::score(&mut career, &rules, def, selection.challenge, time);
                    career.save();
                    Some(o)
                })
                .flatten();
            // An Arcade round won: the next is where Arcade carries on, and
            // its track opens; the field lines up in the finishing order and
            // its last three go out. Not won: one of the round's three
            // attempts gone, and with the last of them Arcade starts again
            // from the first round.
            let mut new_track = None;
            if selection.mode == Mode::Arcade {
                if arcade_won(&p, field.0) {
                    let next = selection.round + 1;
                    if let Some(f) = career.arcade_field.as_mut() {
                        let places: Vec<(&str, usize)> = match field {
                            (Some(s), Some(o)) => o.racers.iter().zip(&s.opponents).map(|(r, p)| (r.model.as_str(), p.0)).collect(),
                            _ => Vec::new(),
                        };
                        f.regrid(|m| places.iter().find(|p| p.0.eq_ignore_ascii_case(m)).map(|p| p.1));
                        if next < roster.arcade.len() {
                            f.knock_out(&roster, &selection.rider, next);
                        }
                    }
                    career.arcade = if next < roster.arcade.len() { next } else { 0 };
                    if let Some(t) = roster.arcade.get(next).filter(|t| !unlocks::track_open(&career, &roster, &rules, t)) {
                        career.reached.insert(t.clone());
                        new_track = Some(t.clone());
                    }
                } else {
                    selection.failed += 1;
                    if selection.failed >= ARCADE_ATTEMPTS {
                        career.arcade = 0;
                        career.arcade_field = None;
                    }
                }
                career.save();
            }
            // The track records broken, except in a challenge or by the
            // demo's computer rider (0x8006e), then Open Freestyle's medal
            // records taken (0x7ff60).
            let broken = match records.as_mut() {
                Some(r) if selection.mode != Mode::Challenge && demo.is_none() => {
                    let entries = record_entries(&p, session.as_deref(), &feats, &selection, &roster, &career, field.1);
                    let mut b = r.check(&selection.track, &entries);
                    if selection.mode == Mode::Freestyle
                        && let Some(limit) = TIME_LIMITS.iter().position(|&l| l == selection.time_limit)
                    {
                        b.extend(r.check_medals(limit, &entries));
                    }
                    if !b.is_empty() {
                        r.save();
                    }
                    b
                }
                _ => Vec::new(),
            };
            // The championship this round was in: a single race isn't one,
            // whatever the profile has saved.
            let run = match selection.mode {
                _ if champ_round => mp_champ.run.as_ref(),
                Mode::Career => career.run.as_ref(),
                _ => None,
            };
            spawn_race_menu(&mut commands, &ui, true, &p, session.as_deref(), &selection, &roster, field, (award.as_ref(), run, outcome.as_ref(), new_track.as_deref(), &broken), overlay, "");
        }
        return;
    };
    let Some(nav) = nav else { return };
    let close = |commands: &mut Commands, virt: &mut Time<Virtual>| {
        commands.entity(e).despawn();
        virt.unpause();
    };
    // The Audio, Camera and Controller pages change the settings as they go
    // (music and ambience follow at once, the camera as the race resumes) and
    // go back to the pause menu.
    if let Some(page) = menu.page {
        let mut redraw = false;
        match nav {
            Nav::Up | Nav::Down => {
                list.step(nav);
                if page == PausePage::Camera {
                    settings.camera = CameraView::ALL[list.selected.min(CameraView::ALL.len() - 1)];
                }
            }
            Nav::Left | Nav::Right if page == PausePage::Audio => {
                let level = if list.selected == 0 { &mut settings.music } else { &mut settings.effects };
                *level = if nav == Nav::Left { level.saturating_sub(1) } else { (*level + 1).min(VOLUME_STEPS) };
                redraw = true;
            }
            Nav::Accept | Nav::Left | Nav::Right if page == PausePage::Controller => {
                settings.vibration = !settings.vibration;
                redraw = true;
            }
            Nav::Accept | Nav::Start | Nav::Back => {
                settings.save();
                commands.entity(e).despawn();
                spawn_race_menu(&mut commands, &ui, false, &p, session.as_deref(), &selection, &roster, field, (None, None, None, None, &[]), overlay, page.item());
            }
            _ => {}
        }
        if redraw {
            commands.entity(e).despawn();
            spawn_pause_page(&mut commands, &ui, page, &settings, list.selected, overlay);
        }
        return;
    }
    match nav {
        Nav::Up | Nav::Down => list.step(nav),
        Nav::Start | Nav::Back if !menu.results => close(&mut commands, &mut virt),
        Nav::Accept => match list.current() {
            item @ ("AUDIO" | "CAMERA" | "CONTROLLER") => {
                let page = [PausePage::Audio, PausePage::Camera, PausePage::Controller].into_iter().find(|p| p.item() == item).unwrap_or(PausePage::Audio);
                commands.entity(e).despawn();
                spawn_pause_page(&mut commands, &ui, page, &settings, 0, overlay);
            }
            // Arcade's last round won: on to Arcade Complete.
            "CONTINUE" if menu.results && selection.mode == Mode::Arcade => {
                fe.return_to = Some(Screen::ArcadeComplete);
                next.set(AppState::FrontEnd);
            }
            "RETURN TO RACE" => close(&mut commands, &mut virt),
            "REPLAY" => commands.insert_resource(replay::Playback::new()),
            "END SESSION" => {
                if let Some(s) = session.as_mut() {
                    s.ended = true;
                }
                close(&mut commands, &mut virt);
            }
            "RESTART" | "RESTART RACE" | "RESTART CHALLENGE" | "RACE AGAIN" | "TRY AGAIN" | "PLAY AGAIN" => next.set(AppState::Loading),
            // Arcade Status shows the next round's grid first.
            "NEXT ROUND" => {
                selection.failed = 0;
                fe.return_to = Some(Screen::ArcadeStatus);
                next.set(AppState::FrontEnd);
            }
            "CHAMPIONSHIP STATUS" => {
                if selection.mode != Mode::Career {
                    fe.return_to = Some(Screen::MpChampStatus);
                }
                next.set(AppState::FrontEnd);
            }
            "CHALLENGES" => {
                fe.return_to = Some(Screen::Challenges);
                next.set(AppState::FrontEnd);
            }
            "QUIT RACE" | "MAIN MENU" => {
                fe.return_to = Some(Screen::Main);
                next.set(AppState::FrontEnd);
            }
            _ => {}
        },
        _ => {}
    }
}

/// Tries at an Arcade round (the original's "Attempts: %d/3").
const ARCADE_ATTEMPTS: usize = 3;

/// An Arcade round is won by finishing in time and, with computer riders,
/// in first place ("You need to finish in first place to move onto the
/// next round").
fn arcade_won(p: &RaceProgress, standings: Option<&Standings>) -> bool {
    p.finished.is_some() && !p.out_of_time() && standings.is_none_or(|s| s.racers <= 1 || s.player == 1)
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn spawn_race_menu(
    commands: &mut Commands,
    ui: &UiAssets,
    results: bool,
    p: &RaceProgress,
    session: Option<&Session>,
    sel: &Selection,
    roster: &Roster,
    (standings, opponents): (Option<&Standings>, Option<&Opponents>),
    (award, run, outcome, new_track, broken): (Option<&Award>, Option<&Run>, Option<&Outcome>, Option<&str>, &[Broken]),
    overlay: Option<Entity>,
    focus: &str,
) {
    // Split screen: each other player's place and time.
    let others: Vec<(usize, String)> = opponents
        .map(|o| {
            o.racers
                .iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    let n = r.human?;
                    let place = standings.and_then(|s| s.opponents.get(i)).map_or(String::new(), |o| ordinal(o.0));
                    let time = r.progress.finished.map_or("DNF".to_string(), fmt_time);
                    Some((n, format!("{place}  {time}")))
                })
                .collect()
        })
        .unwrap_or_default();
    // Place and the winner when there were computer riders.
    let place = standings.filter(|s| s.racers > 1 && p.finished.is_some());
    let winner = place.filter(|s| s.player > 1).and_then(|s| {
        let i = s.opponents.iter().position(|o| o.0 == 1)?;
        opponents?.racers.get(i).map(|r| r.name.clone())
    });
    let freestyle = session.filter(|s| s.freestyle);
    // Freestyle Battle: every player's score.
    let battle = freestyle.zip(opponents).map(|(s, o)| mp::battle_scores(s, o)).filter(|b| b.len() > 1);
    let last_round = sel.round + 1 >= roster.arcade.len();
    let (title, items): (String, &[&str]) = match (results, sel.mode) {
        // The original's race menu (0x4a5c0), with Freestyle's end and
        // the challenge list.
        (false, Mode::Freestyle) if freestyle.is_some_and(|s| s.limit.is_none()) => ("PAUSED".into(), &["AUDIO", "CAMERA", "CONTROLLER", "RETURN TO RACE", "END SESSION", "RESTART", "QUIT RACE"]),
        (false, Mode::Freestyle) => ("PAUSED".into(), &["AUDIO", "CAMERA", "CONTROLLER", "RETURN TO RACE", "RESTART", "QUIT RACE"]),
        (false, Mode::Challenge) => ("PAUSED".into(), &["AUDIO", "CAMERA", "CONTROLLER", "RETURN TO RACE", "RESTART CHALLENGE", "CHALLENGES", "QUIT RACE"]),
        // ATV Academy never ends; the original keeps Restart Race (0x1a7).
        (false, Mode::Training) => ("PAUSED".into(), &["AUDIO", "CAMERA", "CONTROLLER", "RETURN TO RACE", "RESTART RACE", "QUIT RACE"]),
        (false, _) => ("RACE PAUSED".into(), &["AUDIO", "CAMERA", "CONTROLLER", "RETURN TO RACE", "RESTART RACE", "QUIT RACE"]),
        // Passed with a medal (strings 0x58, 0x53; 0x55780).
        (true, Mode::Challenge) => {
            let passed = outcome.is_some_and(|o| o.medal.is_some());
            (format!("CHALLENGE {}", if passed { "PASSED" } else { "FAILED" }), &["TRY AGAIN", "CHALLENGES", "MAIN MENU"])
        }
        (true, Mode::Freestyle) if battle.is_some() => ("FREESTYLE BATTLE".into(), &["PLAY AGAIN", "MAIN MENU"]),
        (true, Mode::Freestyle) => ("FREESTYLE RESULTS".into(), &["PLAY AGAIN", "MAIN MENU"]),
        (true, Mode::Arcade) if !arcade_won(p, standings) => {
            let title = if p.out_of_time() { "OUT OF TIME" } else { "ARCADE RESULTS" };
            let items: &[&str] = if sel.failed >= ARCADE_ATTEMPTS { &["MAIN MENU"] } else { &["TRY AGAIN", "MAIN MENU"] };
            (title.into(), items)
        }
        // The field has moved on, so a round won isn't raced again.
        (true, Mode::Arcade) if last_round => ("ARCADE COMPLETED".into(), &["CONTINUE"]),
        (true, Mode::Arcade) => (format!("ROUND {} COMPLETE", sel.round + 1), &["NEXT ROUND", "MAIN MENU"]),
        (true, Mode::Career) => (format!("ROUND {} COMPLETE", sel.round + 1), &["CHAMPIONSHIP STATUS", "MAIN MENU"]),
        (true, Mode::SingleRace) if run.is_some() => (format!("ROUND {} COMPLETE", sel.round + 1), &["CHAMPIONSHIP STATUS", "MAIN MENU"]),
        (true, _) => ("RACE RESULTS".into(), &["RACE AGAIN", "MAIN MENU"]),
    };
    // One player's race can be watched again.
    let mut items = items.to_vec();
    if results && sel.players <= 1 {
        let at = if items.len() > 1 { items.len() - 1 } else { items.len() };
        items.insert(at, "REPLAY");
    }
    let list = MenuList::new(&items, 220.0).select(items.iter().position(|i| *i == focus).unwrap_or(0));
    let panel = ui.bio.get("BIOPANEL").rect;
    let rows = match (results, freestyle.is_some()) {
        (false, _) => 0,
        (true, true) if let Some(b) = &battle => 1 + b.len(),
        (true, true) => 7,
        (true, false) if let Some(o) = outcome => {
            if o.time.is_none() { 1 } else { 2 + usize::from(o.skill_points > 0) + usize::from(o.new_level.is_some()) + usize::from(o.unlocked.is_some()) }
        }
        (true, false) => {
            p.lap_times.len()
                + 1
                + usize::from(p.arcade.is_some() && p.finished.is_some())
                + usize::from(new_track.is_some())
                + if sel.mode == Mode::Arcade && !arcade_won(p, standings) { 1 + 3 * usize::from(!p.out_of_time()) } else { 0 }
                + usize::from(place.is_some())
                + usize::from(winner.is_some())
                + others.len()
                + award.map_or(0, |a| 3 + usize::from(a.new_level.is_some()) + usize::from(a.gold) + a.unlocked.len())
        }
    };
    let rows = rows + if results && !broken.is_empty() { 1 + broken.len() } else { 0 };
    let ph = 120.0 + 18.0 * rows as f32 + 34.0 * items.len() as f32;
    let pw = 340.0;
    let menu = commands.spawn_empty().id();
    commands
        .spawn((
            Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..fullscreen() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.05, 0.45)),
            DespawnOnExit(AppState::Race),
            RaceMenu { results, page: None },
        ))
        .insert_if(UiTargetCamera(overlay.unwrap_or(Entity::PLACEHOLDER)), || overlay.is_some())
        .with_children(|c| {
            c.spawn(Node { width: px(pw), height: px(ph), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, padding: UiRect::all(px(16)), row_gap: px(8), ..default() })
                .with_children(|box_| {
                    let (mut n, img) = ui.bio.tex.region(panel, 1.0);
                    n.position_type = PositionType::Absolute;
                    n.left = px(0);
                    n.top = px(0);
                    n.width = percent(100);
                    n.height = percent(100);
                    box_.spawn((n, img));
                    box_.spawn(BitmapText::new(FontId::Menu, 17.0, title));
                    let sub = match sel.mode {
                        Mode::Freestyle => format!("{}  -  {}", track_title(&sel.track), limit_label(sel.time_limit)),
                        Mode::Arcade => format!("ROUND {}/{}  -  {}", sel.round + 1, roster.arcade.len(), track_title(&sel.track)),
                        Mode::Career => format!("ROUND {}/{}  -  {}", sel.round + 1, run.map_or(career::ROUNDS, Run::rounds), track_title(&sel.track)),
                        Mode::SingleRace if run.is_some() => format!("ROUND {}/{}  -  {}", sel.round + 1, run.map_or(career::ROUNDS, Run::rounds), track_title(&sel.track)),
                        Mode::Challenge => format!("FREESTYLE CHALLENGE  -  {}", track_title(&sel.track)),
                        Mode::Training => format!("ATV ACADEMY  -  {}", track_title(&sel.track)),
                        _ => format!("{}  -  {}", track_title(&sel.track), sel.quad.to_uppercase()),
                    };
                    box_.spawn(BitmapText::new(FontId::Small, 9.0, sub).color(Color::srgb(0.75, 0.85, 1.0)));
                    if let (true, Some(b)) = (results, &battle) {
                        box_.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), margin: UiRect::vertical(px(6)), ..default() }).with_children(|rows| {
                            let mut order = b.clone();
                            order.sort_by_key(|&(n, score)| (std::cmp::Reverse(score), n));
                            let top = order[0].1;
                            let winners: Vec<String> = order.iter().filter(|o| o.1 == top).map(|o| format!("PLAYER {}", o.0)).collect();
                            row(rows, if winners.len() > 1 { "DRAW" } else { "WINNER" }, &winners.join(" + "), true);
                            for &(n, score) in &order {
                                row(rows, &format!("{} PLAYER {n}", ordinal(mp::place(b, n))), &score.to_string(), score == top);
                            }
                        });
                    } else if let (true, Some(s)) = (results, freestyle) {
                        box_.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), margin: UiRect::vertical(px(6)), ..default() }).with_children(|rows| {
                            let medal = s.medal();
                            row(rows, "SCORE", &s.score.to_string(), medal.is_some());
                            let m = match (medal, s.medals) {
                                (Some(i), _) => format!("{} MEDAL", MEDALS[i]),
                                (None, Some(m)) => format!("BRONZE AT {}", m[0]),
                                (None, None) => "-".into(),
                            };
                            row(rows, "MEDAL", &m, medal.is_some());
                            let best = s.best_trick.as_ref().map_or("-".to_string(), |(n, pts)| format!("{n} {pts}"));
                            row(rows, "BEST TRICK", &best, false);
                            row(rows, "BEST JUMP", &s.best_jump.to_string(), false);
                            row(rows, "TRICKS LANDED", &s.landed.to_string(), false);
                            row(rows, "SPOTS FOUND", &format!("{}/{}", s.spots_found(), s.spots.len()), false);
                            row(rows, "LONGEST AIR", &format!("{:.1}S", s.longest_air), false);
                        });
                    } else if let (true, Some(o)) = (results, outcome) {
                        box_.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), margin: UiRect::vertical(px(6)), ..default() }).with_children(|rows| {
                            let Some(time) = o.time else {
                                row(rows, "RESPOTTED", "RUN OVER", false);
                                return;
                            };
                            row(rows, "TIME", &fmt_time(time), o.best_time);
                            match o.medal {
                                Some(m) => row(rows, "MEDAL", MEDALS[m], true),
                                None => row(rows, "NO MEDAL", "RECORDS NOT BEATEN", false),
                            }
                            if o.skill_points > 0 {
                                row(rows, "SKILL POINTS", &format!("+{}", o.skill_points), false);
                            }
                            if let Some(l) = o.new_level {
                                row(rows, "SKILL LEVEL", &format!("UP TO {l}"), true);
                            }
                            if let Some(r) = &o.unlocked {
                                row(rows, "NEW RIDER", &challenge::rider_label(r), true);
                            }
                        });
                    } else if results {
                        box_.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), margin: UiRect::vertical(px(6)), ..default() }).with_children(|rows| {
                            if let Some(s) = place {
                                let label = if others.is_empty() { "POSITION" } else { "PLAYER 1" };
                                row(rows, label, &format!("{} OF {}", ordinal(s.player), s.racers), s.player == 1);
                            }
                            for (n, text) in &others {
                                row(rows, &format!("PLAYER {n}"), text, text.starts_with("1ST"));
                            }
                            if let Some(w) = &winner {
                                row(rows, "WINNER", w, false);
                            }
                            let best = p.best_lap();
                            for (i, t) in p.lap_times.iter().enumerate() {
                                let gold = best == Some(*t);
                                row(rows, &format!("LAP {}", i + 1), &fmt_time(*t), gold);
                            }
                            // In split screen, not home when the wait ran out is a DNF too.
                            let total = if p.dnf || (!others.is_empty() && p.finished.is_none()) { "DNF".to_string() } else { fmt_time(p.finished.unwrap_or(p.clock)) };
                            row(rows, "TOTAL", &total, false);
                            if let Some(a) = p.arcade.as_ref().filter(|_| p.finished.is_some()) {
                                row(rows, "TIME LEFT", &fmt_time(a.left), false);
                            }
                            if let Some(t) = new_track {
                                row(rows, "NEW TRACK", &track_title(t), true);
                            }
                            if sel.mode == Mode::Arcade && !arcade_won(p, standings) {
                                if !p.out_of_time() {
                                    // The original's message, line for line.
                                    for line in ["YOU NEED TO FINISH", "IN FIRST PLACE TO MOVE", "ONTO THE NEXT ROUND"] {
                                        row(rows, line, "", false);
                                    }
                                }
                                let over = if sel.failed >= ARCADE_ATTEMPTS { "BACK TO ROUND 1" } else { "" };
                                row(rows, &format!("ATTEMPTS: {}/{ARCADE_ATTEMPTS}", sel.failed.min(ARCADE_ATTEMPTS)), over, false);
                            }
                            if let Some(a) = award {
                                row(rows, "RACE POINTS", &format!("+{}", a.position_points), false);
                                row(rows, "TRICK POINTS", &format!("+{}", a.trick_points), false);
                                let standing = run.map_or(String::new(), |r| ordinal(r.player_place()));
                                row(rows, "CHAMPIONSHIP", &standing, false);
                                row(rows, "SKILL POINTS", &format!("+{}", a.skill_points), false);
                                if let Some(l) = a.new_level {
                                    row(rows, "SKILL LEVEL", &format!("UP TO {l}"), true);
                                }
                                if a.gold {
                                    row(rows, "GOLD MEDAL", "", true);
                                }
                                for atv in &a.unlocked {
                                    row(rows, "NEW ATV", &atv.to_uppercase(), true);
                                }
                            }
                        });
                    }
                    if results && !broken.is_empty() {
                        box_.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), ..default() }).with_children(|rows| {
                            // The original's own page for them says "Track
                            // record broken!", or the stadium's in freestyle
                            // (0x4e050).
                            let title = if sel.mode == Mode::Freestyle { "STADIUM RECORD BROKEN!" } else { "TRACK RECORD BROKEN!" };
                            row(rows, title, "", true);
                            for b in broken {
                                let label = if sel.players > 1 { format!("P{} {}", b.player, b.kind.label()) } else { b.kind.label().to_string() };
                                row(rows, &label, &b.kind.show(b.value), true);
                            }
                        });
                    }
                    box_.spawn(Node { margin: UiRect::top(px(8)), ..default() }).add_child(menu);
                });
        });
    spawn_menu_rows(commands, ui, menu, &list);
    commands.entity(menu).insert(list);
}

/// The pause menu's Audio, Camera or Controller page, as the Options pages
/// lay them out: the volume bars, the camera views, or vibration and the
/// controls.
fn spawn_pause_page(commands: &mut Commands, ui: &UiAssets, page: PausePage, settings: &Settings, row: usize, overlay: Option<Entity>) {
    let (items, selected, note): (Vec<&str>, usize, &str) = match page {
        PausePage::Audio => (vec!["MUSIC VOLUME", "SOUND EFFECTS VOLUME"], row, "LEFT AND RIGHT CHANGE THE VOLUME"),
        PausePage::Camera => (CameraView::ALL.map(CameraView::label).to_vec(), settings.camera.index(), "C OR BACK CHANGES IT IN THE RACE TOO"),
        PausePage::Controller => (vec!["VIBRATION"], 0, ""),
    };
    let width = if page == PausePage::Audio { 270.0 } else { 220.0 };
    let list = MenuList::new(&items, width).select(selected);
    let rows = list.row;
    let pw = if page == PausePage::Camera { 400.0 } else { 460.0 };
    let panel = ui.bio.get("BIOPANEL").rect;
    let grey = Color::srgb(0.75, 0.85, 1.0);
    let menu = commands.spawn_empty().id();
    commands
        .spawn((
            Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..fullscreen() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.05, 0.45)),
            DespawnOnExit(AppState::Race),
            RaceMenu { results: false, page: Some(page) },
        ))
        .insert_if(UiTargetCamera(overlay.unwrap_or(Entity::PLACEHOLDER)), || overlay.is_some())
        .with_children(|c| {
            c.spawn(Node { width: px(pw), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, padding: UiRect::all(px(16)), row_gap: px(8), ..default() })
                .with_children(|box_| {
                    let (mut n, img) = ui.bio.tex.region(panel, 1.0);
                    n.position_type = PositionType::Absolute;
                    n.left = px(0);
                    n.top = px(0);
                    n.width = percent(100);
                    n.height = percent(100);
                    box_.spawn((n, img));
                    box_.spawn(BitmapText::new(FontId::Menu, 17.0, page.item()));
                    box_.spawn(Node { column_gap: px(14), margin: UiRect::top(px(8)), ..default() }).with_children(|r| {
                        r.spawn(Node::default()).add_child(menu);
                        // Beside each row: its volume bar, or the checkbox.
                        r.spawn(Node { flex_direction: FlexDirection::Column, ..default() }).with_children(|col| {
                            let levels: &[u8] = match page {
                                PausePage::Audio => &[settings.music, settings.effects],
                                _ => &[],
                            };
                            for &level in levels {
                                col.spawn(Node { height: px(rows), align_items: AlignItems::Center, ..default() }).with_children(|cell| volume_bar(cell, ui, level));
                            }
                            if page == PausePage::Controller {
                                col.spawn(Node { height: px(rows), align_items: AlignItems::Center, ..default() }).with_children(|cell| {
                                    cell.spawn(ui.checkbox.node(if settings.vibration { "ON" } else { "OFF" }, 0.5));
                                });
                            }
                        });
                    });
                    if page == PausePage::Controller {
                        box_.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), margin: UiRect::vertical(px(6)), ..default() }).with_children(|t| {
                            for (i, (action, pad, keys)) in [("", "PAD", "KEYBOARD")].into_iter().chain(frontend::CONTROLS).enumerate() {
                                t.spawn(Node { column_gap: px(8), ..default() }).with_children(|r| {
                                    for (text, width, color) in [(action, 140.0, Color::WHITE), (pad, 92.0, grey), (keys, 130.0, grey)] {
                                        let color = if i == 0 { Color::srgb(1.0, 0.85, 0.3) } else { color };
                                        r.spawn(Node { width: px(width), ..default() }).with_children(|cell| {
                                            cell.spawn(BitmapText::new(FontId::Small, 7.0, text).color(color));
                                        });
                                    }
                                });
                            }
                        });
                    }
                    if !note.is_empty() {
                        box_.spawn(BitmapText::new(FontId::Small, 8.0, note).color(grey));
                    }
                    box_.spawn(BitmapText::new(FontId::Small, 8.0, "PRESS BACK FOR THE PAUSE MENU").color(grey));
                });
        });
    spawn_menu_rows(commands, ui, menu, &list);
    commands.entity(menu).insert(list);
}

/// A volume bar lit to `level`, as on the Options Audio page.
fn volume_bar(parent: &mut ChildSpawnerCommands, ui: &UiAssets, level: u8) {
    let off = ui.sound_bar.get("BAR_OFF").rect;
    parent.spawn(Node { width: px(off.width()), height: px(off.height()), ..default() }).with_children(|b| {
        b.spawn(placed(ui.sound_bar.node("BAR_OFF", 1.0), 0.0, 0.0));
        b.spawn(Node { position_type: PositionType::Absolute, width: px(frontend::lit_width(level)), height: percent(100), overflow: Overflow::clip(), ..default() }).with_children(|clip| {
            clip.spawn(placed(ui.sound_bar.node("BAR_ON", 1.0), 0.0, 0.0));
        });
    });
}

fn row(parent: &mut ChildSpawnerCommands, label: &str, value: &str, best: bool) {
    let c = if best { Color::srgb(1.0, 0.85, 0.2) } else { Color::WHITE };
    parent.spawn(Node { width: px(220), justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexEnd, ..default() }).with_children(|r| {
        r.spawn(BitmapText::new(FontId::Small, 9.0, label).color(c));
        r.spawn(BitmapText::new(FontId::Chrome, 13.0, value).color(c));
    });
}

fn unpause(mut virt: ResMut<Time<Virtual>>) {
    virt.unpause();
}

/// "Dunes1" -> "DUNES 1".
pub fn track_title(track: &str) -> String {
    let split = track.find(|c: char| c.is_ascii_digit()).unwrap_or(track.len());
    format!("{} {}", &track[..split], &track[split..]).trim().to_uppercase()
}
