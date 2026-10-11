//! The record page, as the original's (`ID_RECORD_PAGE`, 0x4d8d0 and
//! 0x4e050): leaving a race's results, each human who broke a record sees
//! them listed under "Track record broken!" ("Stadium record broken!" in
//! freestyle, "... by player n!" in split screen) and "Please enter your
//! name" (strings 0x15a to 0x15e, 0x176), then types the name they go
//! under, the one they were set with already in the box. OK writes it into
//! each of them (0x4e160). The original takes the players in turn (0x80890)
//! once the results' button is pressed (0x7fe40), then carries on; here the
//! results menu's item picked goes ahead once the pages are done.

use std::collections::VecDeque;

use bevy::input::gamepad::Gamepad;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;

use crate::name_entry::{self, NameEntry};
use crate::records::{Broken, TrackRecords};
use crate::ui::{BitmapText, FontId, UiAssets, SCREEN_H, SCREEN_W};
use crate::AppState;

/// Name Your Rider's background (`Frontend.xml` `Profile_Name`).
const BACKGROUND: &str = "ProfileSelect1";

/// One player's records broken and the name they were set with.
#[derive(Debug, Clone, PartialEq)]
struct Page {
    player: usize,
    broken: Vec<Broken>,
    name: String,
}

#[derive(Resource, Debug)]
pub struct RecordPages {
    track: String,
    /// Open Freestyle's time limit, for its medal records.
    limit: Option<usize>,
    split: bool,
    stadium: bool,
    pages: VecDeque<Page>,
    /// The results menu's item picked, waiting for the pages.
    then: Option<String>,
    entry: NameEntry,
    built: bool,
}

impl RecordPages {
    /// A page for each player in `broken` (1 to 4, in turn), their names
    /// from `name`; None when no record was broken.
    pub fn new(track: &str, limit: Option<usize>, split: bool, stadium: bool, broken: &[Broken], name: impl Fn(usize) -> String) -> Option<Self> {
        let mut pages = VecDeque::new();
        for player in 1..=4 {
            let mine: Vec<Broken> = broken.iter().filter(|b| b.player == player).cloned().collect();
            if !mine.is_empty() {
                pages.push_back(Page { player, broken: mine, name: name(player) });
            }
        }
        (!pages.is_empty()).then(|| Self { track: track.to_string(), limit, split, stadium, pages, then: None, entry: NameEntry::default(), built: false })
    }

    /// Leaving the results with `item`: true when the pages come first.
    pub fn hold(&mut self, item: &str) -> bool {
        if self.then.is_some() || self.pages.is_empty() {
            return false;
        }
        self.then = Some(item.to_string());
        self.start_page();
        true
    }

    /// Whether a page is up, with the controls.
    pub fn open(&self) -> bool {
        self.then.is_some() && !self.pages.is_empty()
    }

    /// The item held, once every page is done.
    pub fn resume(&mut self) -> Option<String> {
        if self.pages.is_empty() { self.then.take() } else { None }
    }

    fn start_page(&mut self) {
        self.entry = NameEntry::new(self.pages.front().map_or("", |p| p.name.as_str()));
        self.built = false;
    }

    /// The page's heading (0x4d8d0).
    fn title(&self, player: usize) -> String {
        let what = if self.stadium { "STADIUM" } else { "TRACK" };
        if self.split { format!("{what} RECORD BROKEN BY PLAYER {player}!") } else { format!("{what} RECORD BROKEN!") }
    }

    /// OK: the name typed goes on the page's records and the next page
    /// comes up. Left as it was (or emptied), they keep the name in full.
    fn finish_page(&mut self, records: &mut TrackRecords) {
        let Some(page) = self.pages.pop_front() else { return };
        let typed = self.entry.name.trim();
        let name = if typed.is_empty() || self.entry.name == NameEntry::new(&page.name).name { page.name.as_str() } else { typed };
        for b in &page.broken {
            records.rename(&self.track, self.limit, b, name);
        }
        self.start_page();
    }
}

pub struct RecordPagePlugin;

impl Plugin for RecordPagePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, swallow_keys.after(bevy::input::InputSystems).run_if(resource_exists::<RecordPages>))
            .add_systems(Update, (run, draw, crate::frontend::blink).chain().run_if(in_state(AppState::Race)).run_if(resource_exists::<RecordPages>))
            .add_systems(OnExit(AppState::Race), |mut commands: Commands| commands.remove_resource::<RecordPages>());
    }
}

/// While a page is up the keys type rather than ride: the race's own keys
/// (Backspace's quick restart, R's respot, C's camera) don't see them.
fn swallow_keys(pages: Res<RecordPages>, mut keys: ResMut<ButtonInput<KeyCode>>) {
    if !pages.open() {
        return;
    }
    let moving = [KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::Enter, KeyCode::NumpadEnter, KeyCode::Escape];
    let typed: Vec<KeyCode> = keys.get_just_pressed().copied().filter(|k| !moving.contains(k)).collect();
    for k in typed {
        keys.clear_just_pressed(k);
    }
}

#[derive(Component)]
struct RecordPageUi;

fn run(
    mut pages: ResMut<RecordPages>,
    mut records: Option<ResMut<TrackRecords>>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mut typing: MessageReader<KeyboardInput>,
) {
    if !pages.open() || !pages.built {
        typing.clear();
        return;
    }
    let mut changed = false;
    for k in typing.read() {
        if k.state == ButtonState::Pressed {
            changed |= pages.entry.type_key(k);
        }
    }
    if let Some(nav) = name_entry::read_nav(&keys, &pads) {
        if pages.entry.nav(nav)
            && let Some(r) = records.as_deref_mut()
        {
            pages.finish_page(r);
            r.save();
        }
        changed = true;
    }
    if changed {
        pages.built = false;
    }
}

fn draw(mut commands: Commands, mut pages: ResMut<RecordPages>, ui: Option<Res<UiAssets>>, old: Query<Entity, With<RecordPageUi>>) {
    if pages.built && pages.open() {
        return;
    }
    for e in &old {
        commands.entity(e).despawn();
    }
    let Some(ui) = ui else { return };
    let Some(page) = pages.pages.front().filter(|_| pages.then.is_some()).cloned() else { return };
    pages.built = true;
    let label = Color::srgb(0.75, 0.82, 0.95);
    let mut kids = Vec::new();
    let centred = |commands: &mut Commands, font: FontId, size: f32, text: String, top: f32, color: Color| {
        let w = ui.font(font).width(&text, size);
        commands.spawn((BitmapText::new(font, size, text).color(color), at((SCREEN_W - w) / 2.0, top))).id()
    };
    kids.push(centred(&mut commands, FontId::Menu, 22.0, pages.title(page.player), 36.0, Color::WHITE));
    // Each record: its name, then what it is now.
    let mut y = 80.0;
    for b in &page.broken {
        let name = b.kind.label();
        let w = ui.font(FontId::Small).width(name, 9.0);
        kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, name).color(label), at(SCREEN_W / 2.0 - 12.0 - w, y))).id());
        kids.push(commands.spawn((BitmapText::new(FontId::Small, 9.0, b.kind.long(b.value)), at(SCREEN_W / 2.0 + 12.0, y))).id());
        y += 16.0;
    }
    kids.push(centred(&mut commands, FontId::Small, 9.0, "PLEASE ENTER YOUR NAME".into(), y + 10.0, label));
    let (keyboard, _) = name_entry::spawn(&mut commands, &ui, &pages.entry, y + 30.0, y + 90.0);
    kids.extend(keyboard);
    // A front-end page over the race, on Name Your Rider's background.
    commands
        .spawn((crate::ui::fullscreen(), BackgroundColor(Color::srgb(0.02, 0.03, 0.1)), GlobalZIndex(40), DespawnOnExit(AppState::Race), RecordPageUi))
        .with_children(|r| {
            if let Some(tex) = ui.background(BACKGROUND) {
                let (mut n, img) = tex.node(0.0, 0.0);
                n.position_type = PositionType::Absolute;
                n.width = Val::Percent(100.0);
                n.height = Val::Percent(100.0);
                r.spawn((n, img));
            }
            r.spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                margin: UiRect { left: px(-SCREEN_W / 2.0), top: px(-SCREEN_H / 2.0), ..default() },
                width: px(SCREEN_W),
                height: px(SCREEN_H),
                ..default()
            })
            .add_children(&kids);
        });
}

fn at(left: f32, top: f32) -> Node {
    Node { position_type: PositionType::Absolute, left: px(left), top: px(top), ..default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::Kind;

    fn broken(kind: Kind, player: usize) -> Broken {
        Broken { kind, value: 10.0, player }
    }

    #[test]
    fn each_player_gets_a_page_in_turn() {
        let b = [broken(Kind::Lap, 2), broken(Kind::LongestJump, 1), broken(Kind::Wheelie, 2)];
        let mut p = RecordPages::new("Dunes1", None, true, false, &b, |n| format!("P{n}")).unwrap();
        assert_eq!(p.pages.iter().map(|p| (p.player, p.broken.len())).collect::<Vec<_>>(), [(1, 1), (2, 2)]);
        assert!(!p.open(), "nothing shows until the results are left");
        assert!(p.hold("MAIN MENU"));
        assert!(p.open());
        assert_eq!(p.entry.name, "P1", "the name it was set with");
        assert_eq!(p.title(1), "TRACK RECORD BROKEN BY PLAYER 1!");
        assert!(p.resume().is_none(), "not while pages are left");
        p.pages.clear();
        assert_eq!(p.resume().as_deref(), Some("MAIN MENU"));
        assert!(!p.hold("REPLAY"), "done once");
        assert!(RecordPages::new("Dunes1", None, false, false, &[], |_| String::new()).is_none());
    }

    #[test]
    fn ok_puts_the_name_on_the_records() {
        let mut records = TrackRecords::default();
        let mut p = RecordPages::new("Dunes1", None, false, false, &[broken(Kind::Lap, 1)], |_| "Jason Luburgh".into()).unwrap();
        p.hold("MAIN MENU");
        assert_eq!(p.entry.name, "JASON LUBURG", "cut to the box");
        p.finish_page(&mut records);
        assert!(!p.open());
        assert_eq!(p.resume().as_deref(), Some("MAIN MENU"));
    }

    #[test]
    fn freestyle_records_are_the_stadiums() {
        let p = RecordPages::new("Stadium2", Some(0), false, true, &[broken(Kind::Gold, 1)], |_| "ACE".into()).unwrap();
        assert_eq!(p.title(1), "STADIUM RECORD BROKEN!");
    }
}
