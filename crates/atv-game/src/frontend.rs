//! The front end: title, main menu, then rider, ATV and track select into a
//! race, using the original backgrounds, fonts and panels. Which background
//! and title each screen uses comes from the disc's `Setup/Frontend.xml`.
//! Arcade skips track select (it races `Setup/Arcade.xml`'s rounds);
//! Open Freestyle picks a time limit on it instead of laps.

use bevy::input::gamepad::Gamepad;
use bevy::prelude::*;

use atv_formats::setup::{self, Entry};
use atv_formats::GameSource;

use crate::freestyle::{self, limit_label, MEDALS, TIME_LIMITS};
use crate::hud::track_title;
use crate::ui::{fullscreen, placed, read_nav, set_text, spawn_menu_rows, BitmapText, FontId, MenuList, Nav, UiAssets, SCREEN_H, SCREEN_W};
use crate::{AppState, Mode, Selection};

pub struct FrontEndPlugin;

impl Plugin for FrontEndPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrontEnd>()
            .add_systems(OnEnter(AppState::FrontEnd), enter)
            .add_systems(
                Update,
                (navigate, rebuild, update_rider_panel, update_quad_panel, update_track_panel, blink, showroom, turntable)
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
}

impl Default for TrackInfo {
    fn default() -> Self {
        Self { arcade_start: 40.0, worst_lap: 100.0 }
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
}

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

const MUG: f32 = 50.0;
const MUG_GAP: f32 = 64.0;
const MUG_X: f32 = 64.0;
const MUG_Y: f32 = 92.0;

fn enter(mut fe: ResMut<FrontEnd>, sel: Res<Selection>, roster: Res<Roster>) {
    // Back from a race: straight to the main menu, cursors on the last pick.
    if fe.built.is_some() {
        fe.screen = Screen::Main;
    }
    fe.built = None;
    fe.mode = sel.mode;
    fe.limit = TIME_LIMITS.iter().position(|&l| l == sel.time_limit).unwrap_or(1);
    fe.rider = roster.riders.iter().position(|r| r.model.eq_ignore_ascii_case(&sel.rider)).unwrap_or(fe.rider);
    fe.quad = roster.quads.iter().position(|q| q.name.eq_ignore_ascii_case(&sel.quad)).unwrap_or(fe.quad);
    fe.track = roster.tracks.iter().position(|t| t.eq_ignore_ascii_case(&sel.track)).unwrap_or(fe.track);
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
) {
    let Some(nav) = read_nav(&keys, &pads) else { return };
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
                Some(item @ ("ARCADE" | "SINGLE RACE" | "TIME TRIAL" | "FREESTYLE")) => {
                    fe.mode = match item {
                        "ARCADE" => Mode::Arcade,
                        "TIME TRIAL" => Mode::TimeTrial,
                        "FREESTYLE" => Mode::Freestyle,
                        _ => Mode::SingleRace,
                    };
                    sel.mode = fe.mode;
                    fe.screen = Screen::Rider;
                }
                Some("QUIT GAME") => {
                    exit.write(AppExit::Success);
                }
                _ => {}
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
                        sel.rider = r.model.clone();
                    }
                    fe.screen = Screen::Quad;
                }
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
                if let Some(q) = roster.quads.get(fe.quad) {
                    sel.quad = q.name.clone();
                }
                if fe.mode == Mode::Arcade {
                    // Arcade starts on its first round.
                    if let Some(t) = roster.arcade.first() {
                        sel.round = 0;
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
    }
}

fn rebuild(mut commands: Commands, mut fe: ResMut<FrontEnd>, roster: Res<Roster>, ui: Option<Res<UiAssets>>, roots: Query<Entity, With<ScreenRoot>>) {
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
    };
    let label = fe.mode.label();
    let (key, watermark) = match fe.screen {
        Screen::Title => ("Main_Menu", ""),
        Screen::Main => ("Main_Menu", ""),
        Screen::Rider => (rider_key, label),
        Screen::Quad => (atv_key, label),
        Screen::Track => (track_key, label),
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
                Mode::Arcade => 0,
                Mode::SingleRace => 2,
                Mode::TimeTrial => 3,
                Mode::Freestyle => 4,
            };
            // Career needs AI riders to race, and there are no options yet.
            let mut off = vec!["CAREER", "OPTIONS"];
            if roster.arcade.is_empty() {
                off.push("ARCADE");
            }
            let list = MenuList::new(&["ARCADE", "CAREER", "SINGLE RACE", "TIME TRIAL", "FREESTYLE", "OPTIONS", "QUIT GAME"], 280.0)
                .with_row(36.0)
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
            for (i, _) in roster.riders.iter().enumerate().take(24) {
                let (x, y) = mug_pos(i);
                let rect = Rect::new((i % 8) as f32 * 64.0, (i / 8) as f32 * 64.0, (i % 8) as f32 * 64.0 + 64.0, (i / 8) as f32 * 64.0 + 64.0);
                let scale = ui.mugs.size.x / 512.0;
                let rect = Rect::from_corners(rect.min * scale, rect.max * scale);
                let (mut n, img) = ui.mugs.region(rect, MUG / rect.width());
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
            let list = MenuList::new(&refs, 200.0).with_row(26.0).select(fe.quad);
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
    }

    // Button hints along the bottom.
    let hints: &[(&str, &str)] = match fe.screen {
        Screen::Title => &[],
        Screen::Main => &[("controlbuttoncross", "SELECT")],
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
                    Mode::Arcade => format!("ROUND {}  LOADING {}", sel.round + 1, track_title(&sel.track)),
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
