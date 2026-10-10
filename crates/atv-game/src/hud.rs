//! The in-race HUD, drawn with the original art: the rev gauge with its
//! needle, preload meter, speed and gear; lap, time and best-lap readouts;
//! the lap progress meter; the 3-2-1-GO countdown, checkpoint banner and
//! wrong-way warning; arcade's checkpoint clock; and the pause and results
//! menus for every mode. Freestyle's score readouts are in `freestyle`.

use bevy::input::gamepad::Gamepad;
use bevy::prelude::*;

use crate::career::{self, Award, Career, Run};
use crate::challenge::{self, Outcome};
use crate::freestyle::{limit_label, Session, MEDALS};
use crate::frontend::{FrontEnd, Roster, Screen};
use crate::opponents::{Opponents, Standings};
use crate::race::{fmt_time, RaceProgress};
use crate::ui::{fullscreen, placed, read_nav, set_text, spawn_menu_rows, BitmapText, FontId, MenuList, Nav, UiAssets};
use crate::{AppState, Mode, Player, Selection};

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Race), spawn_hud)
            .add_systems(OnExit(AppState::Race), unpause)
            .add_systems(
                Update,
                (update_gauge, update_readouts, update_meter, update_banners, race_menus).run_if(in_state(AppState::Race)),
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
#[derive(Component)]
enum Readout {
    Speed,
    Gear,
    Lap,
    Time,
    LapTime,
    Best,
    /// Arcade: seconds left, and the round.
    TimeLeft,
    Round,
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
struct RaceMenu {
    results: bool,
}

fn spawn_hud(mut commands: Commands, assets: Option<Res<UiAssets>>, sel: Res<Selection>, roster: Res<Roster>) {
    let Some(ui) = assets else { return };
    let racing = !matches!(sel.mode, Mode::Freestyle | Mode::Challenge);
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
                // Gear in the small dial, speed in the display window.
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
                for (label, size, readout) in [("TIME", 18.0, Readout::Time), ("LAP", 11.0, Readout::LapTime), ("BEST", 11.0, Readout::Best)] {
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

fn update_gauge(
    player: Option<Res<Player>>,
    ui: Option<Res<UiAssets>>,
    mut needle: Query<&mut UiTransform, With<NeedleHub>>,
    mut arc: Query<(&mut ImageNode, &mut Visibility), With<PreloadArc>>,
    mut shown: Local<f32>,
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
}

fn update_readouts(
    player: Option<Res<Player>>,
    progress: Option<Res<RaceProgress>>,
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
/// the running order: the player, then each computer rider by its place;
/// anyone missing from the race comes last.
fn finishing_order(run: &Run, dnf: bool, (standings, opponents): (Option<&Standings>, Option<&Opponents>)) -> Vec<usize> {
    let mut placed: Vec<(usize, usize)> = run
        .field
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let place = if e.player && dnf {
                // Didn't finish: last of those placed.
                99
            } else if e.player {
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
    time: Res<Time<Real>>,
    mut countdown: Query<(&mut ImageNode, &mut Node, &mut UiTransform, &mut Visibility), With<Countdown>>,
    mut banner: Query<&mut Visibility, (With<CheckpointBanner>, Without<Countdown>)>,
    mut wrong: Query<&mut Visibility, (With<WrongWay>, Without<Countdown>, Without<CheckpointBanner>)>,
    mut split: Query<&mut BitmapText, (With<SplitText>, Without<LapBanner>)>,
    mut lap: Query<&mut BitmapText, (With<LapBanner>, Without<SplitText>)>,
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
    if let Ok(mut vis) = wrong.single_mut() {
        let blink = (time.elapsed_secs() * 3.0).fract() < 0.6;
        vis.set_if_neq(if p.wrong_way > 1.5 && p.racing() && blink { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// Escape / Start pauses the race; finishing shows the results. Both are
/// menus over the frozen race.
#[allow(clippy::too_many_arguments)]
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
    (mut career, rules, mut fe): (ResMut<Career>, Res<career::Rules>, ResMut<FrontEnd>),
    (defs, course): (Res<challenge::Defs>, Option<Res<challenge::Course>>),
) {
    let (Some(ui), Some(p)) = (ui, progress) else { return };
    let field = (field.0.as_deref(), field.1.as_deref());
    let nav = read_nav(&keys, &pads);
    // Results come up a moment after the line (or the clock running out).
    let over = match session.as_deref() {
        Some(s) if s.freestyle => s.over(p.clock),
        _ => p.over(),
    };
    *finished_for = if over { *finished_for + virt.delta_secs() } else { 0.0 };

    let (Ok((e, menu)), Ok(mut list)) = (menus.single(), lists.single_mut()) else {
        // Escape or Start pauses (Backspace is the quick restart).
        if (keys.just_pressed(KeyCode::Escape) || nav == Some(Nav::Start)) && !over {
            virt.pause();
            spawn_race_menu(&mut commands, &ui, false, &p, session.as_deref(), &selection, &roster, field, (None, None, None));
        } else if *finished_for > 2.5 && menus.is_empty() {
            let award = (selection.mode == Mode::Career).then(|| {
                let run = career.run.clone()?;
                let order = finishing_order(&run, p.dnf, field);
                let mut tricks = vec![0; run.field.len()];
                if let (Some(i), Some(s)) = (run.field.iter().position(|e| e.player), session.as_deref()) {
                    tricks[i] = s.score;
                }
                let award = career.score(&rules, &selection.track, &order, &tricks);
                career.save();
                award
            });
            let award = award.flatten();
            // A finished challenge run: its medal and bonus.
            let outcome = (selection.mode == Mode::Challenge)
                .then(|| {
                    let (time, def) = (p.finished?, defs.get(selection.challenge)?);
                    let penalties = course.as_ref().map_or(0.0, |c| c.penalties);
                    let o = challenge::score(&mut career, &rules, def, selection.challenge, time, penalties);
                    career.save();
                    Some(o)
                })
                .flatten();
            spawn_race_menu(&mut commands, &ui, true, &p, session.as_deref(), &selection, &roster, field, (award.as_ref(), career.run.as_ref(), outcome.as_ref()));
        }
        return;
    };
    let Some(nav) = nav else { return };
    let close = |commands: &mut Commands, virt: &mut Time<Virtual>| {
        commands.entity(e).despawn();
        virt.unpause();
    };
    match nav {
        Nav::Up | Nav::Down => list.step(nav),
        Nav::Start | Nav::Back if !menu.results => close(&mut commands, &mut virt),
        Nav::Accept => match list.current() {
            "CONTINUE" => close(&mut commands, &mut virt),
            "END SESSION" => {
                if let Some(s) = session.as_mut() {
                    s.ended = true;
                }
                close(&mut commands, &mut virt);
            }
            "RESTART" | "RACE AGAIN" | "TRY AGAIN" | "PLAY AGAIN" => next.set(AppState::Loading),
            "NEXT ROUND" => {
                if let Some(t) = roster.arcade.get(selection.round + 1) {
                    selection.round += 1;
                    selection.track = t.clone();
                    next.set(AppState::Loading);
                }
            }
            "CHAMPIONSHIP STATUS" => next.set(AppState::FrontEnd),
            "CHALLENGES" => {
                fe.return_to = Some(Screen::Challenges);
                next.set(AppState::FrontEnd);
            }
            "QUIT RACE" | "QUIT" | "MAIN MENU" => {
                fe.return_to = Some(Screen::Main);
                next.set(AppState::FrontEnd);
            }
            _ => {}
        },
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_race_menu(
    commands: &mut Commands,
    ui: &UiAssets,
    results: bool,
    p: &RaceProgress,
    session: Option<&Session>,
    sel: &Selection,
    roster: &Roster,
    (standings, opponents): (Option<&Standings>, Option<&Opponents>),
    (award, run, outcome): (Option<&Award>, Option<&Run>, Option<&Outcome>),
) {
    // Place and the winner when there were computer riders.
    let place = standings.filter(|s| s.racers > 1 && p.finished.is_some());
    let winner = place.filter(|s| s.player > 1).and_then(|s| {
        let i = s.opponents.iter().position(|o| o.0 == 1)?;
        opponents?.racers.get(i).map(|r| r.name.clone())
    });
    let freestyle = session.filter(|s| s.freestyle);
    let last_round = sel.round + 1 >= roster.arcade.len();
    let (title, items): (String, &[&str]) = match (results, sel.mode) {
        (false, Mode::Freestyle) if freestyle.is_some_and(|s| s.limit.is_none()) => ("PAUSED".into(), &["CONTINUE", "END SESSION", "RESTART", "QUIT"]),
        (false, Mode::Freestyle) => ("PAUSED".into(), &["CONTINUE", "RESTART", "QUIT"]),
        (false, Mode::Challenge) => ("PAUSED".into(), &["CONTINUE", "RESTART", "CHALLENGES", "QUIT"]),
        (false, _) => ("RACE PAUSED".into(), &["CONTINUE", "RESTART", "QUIT RACE"]),
        (true, Mode::Challenge) => (format!("CHALLENGE {} COMPLETE", sel.challenge), &["TRY AGAIN", "CHALLENGES", "MAIN MENU"]),
        (true, Mode::Freestyle) => ("FREESTYLE RESULTS".into(), &["PLAY AGAIN", "MAIN MENU"]),
        (true, Mode::Arcade) if p.out_of_time() => ("OUT OF TIME".into(), &["TRY AGAIN", "MAIN MENU"]),
        (true, Mode::Arcade) if last_round => ("ARCADE COMPLETED".into(), &["MAIN MENU"]),
        (true, Mode::Arcade) => (format!("ROUND {} COMPLETE", sel.round + 1), &["NEXT ROUND", "RACE AGAIN", "MAIN MENU"]),
        (true, Mode::Career) => (format!("ROUND {} COMPLETE", sel.round + 1), &["CHAMPIONSHIP STATUS", "MAIN MENU"]),
        (true, _) => ("RACE RESULTS".into(), &["RACE AGAIN", "MAIN MENU"]),
    };
    let list = MenuList::new(items, 220.0);
    let panel = ui.bio.get("BIOPANEL").rect;
    let rows = match (results, freestyle.is_some()) {
        (false, _) => 0,
        (true, true) => 7,
        (true, false) if let Some(o) = outcome => {
            2 + usize::from(o.penalties > 0.0) + usize::from(o.skill_points > 0) + usize::from(o.new_level.is_some()) + usize::from(o.unlocked.is_some())
        }
        (true, false) => {
            p.lap_times.len()
                + 1
                + usize::from(p.arcade.is_some() && p.finished.is_some())
                + usize::from(place.is_some())
                + usize::from(winner.is_some())
                + award.map_or(0, |a| 3 + usize::from(a.new_level.is_some()) + usize::from(a.gold) + a.unlocked.len())
        }
    };
    let ph = 120.0 + 18.0 * rows as f32 + 34.0 * items.len() as f32;
    let pw = 340.0;
    let menu = commands.spawn_empty().id();
    commands
        .spawn((
            Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..fullscreen() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.05, 0.45)),
            DespawnOnExit(AppState::Race),
            RaceMenu { results },
        ))
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
                        Mode::Career => format!("ROUND {}/{}  -  {}", sel.round + 1, career::ROUNDS, track_title(&sel.track)),
                        Mode::Challenge => format!("FREESTYLE CHALLENGE  -  {}", track_title(&sel.track)),
                        _ => format!("{}  -  {}", track_title(&sel.track), sel.quad.to_uppercase()),
                    };
                    box_.spawn(BitmapText::new(FontId::Small, 9.0, sub).color(Color::srgb(0.75, 0.85, 1.0)));
                    if let (true, Some(s)) = (results, freestyle) {
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
                            row(rows, "TIME", &fmt_time(o.time), o.best_time);
                            if o.penalties > 0.0 {
                                row(rows, "CONE PENALTIES", &format!("+{:.0} SEC", o.penalties), false);
                            }
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
                                row(rows, "UNLOCKED", &challenge::rider_label(r), true);
                            }
                        });
                    } else if results {
                        box_.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), margin: UiRect::vertical(px(6)), ..default() }).with_children(|rows| {
                            if let Some(s) = place {
                                row(rows, "POSITION", &format!("{} OF {}", ordinal(s.player), s.racers), s.player == 1);
                            }
                            if let Some(w) = &winner {
                                row(rows, "WINNER", w, false);
                            }
                            let best = p.best_lap();
                            for (i, t) in p.lap_times.iter().enumerate() {
                                let gold = best == Some(*t);
                                row(rows, &format!("LAP {}", i + 1), &fmt_time(*t), gold);
                            }
                            let total = if p.dnf { "DNF".to_string() } else { fmt_time(p.finished.unwrap_or(p.clock)) };
                            row(rows, "TOTAL", &total, false);
                            if let Some(a) = p.arcade.as_ref().filter(|_| p.finished.is_some()) {
                                row(rows, "TIME LEFT", &fmt_time(a.left), false);
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
                                    row(rows, "UNLOCKED", &atv.to_uppercase(), true);
                                }
                            }
                        });
                    }
                    box_.spawn(Node { margin: UiRect::top(px(8)), ..default() }).add_child(menu);
                });
        });
    spawn_menu_rows(commands, ui, menu, &list);
    commands.entity(menu).insert(list);
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
