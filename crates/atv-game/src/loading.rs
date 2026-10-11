//! The race loading screen, laid out as the original's (0x57d40): the
//! track's picture (`Track_<track>_LARGE`) under `LoadingOverlay`'s bands,
//! "<area> : <track>" in the top band, a racing tip across the grey bands
//! and "Loading <mode> Game" in the bottom one.
//!
//! Its text is the original's big two-tone text. A size here is a
//! capital's cell height in screen units: the cell's 0.1035 of the font
//! page times the text's scale times 1.385 (0x479a0), times 240 since the
//! original's screen is 2 units tall. The original also draws glyphs 1.2
//! times wider than their texels (0.9 of the scale, +0x38, across 640
//! units rather than 480), so the tip wraps where its wider lines would.

use bevy::prelude::*;

use crate::ui::{fullscreen, two_tone, FontId, UiAssets, SCREEN_H, SCREEN_W};
use crate::{AppState, Mode, Selection};

/// Lines of `Strings_ENG.txt`, which holds one value per line in the order
/// of the executable's string keys (0x1cf9a8). The keys are noted.
const LOADING_X_GAME: usize = 0x108;

/// The areas' and tracks' names, by the names the setup files use
/// (`Industrial` reads "Works").
const NAMES: [(&str, usize); 23] = [
    ("Dunes", 0x9e),
    ("Dunes1", 0x9f),
    ("Dunes2", 0xa0),
    ("Dunes3", 0xa1),
    ("Forest", 0xaf),
    ("Forest1", 0xb0),
    ("Forest2", 0xb1),
    ("Forest3", 0xb2),
    ("Glacier", 0xc2),
    ("Glacier1", 0xc3),
    ("Glacier2", 0xc4),
    ("Glacier3", 0xc5),
    ("Industrial", 0xe5),
    ("Industrial1", 0xe6),
    ("Industrial2", 0xe7),
    ("Industrial3", 0xe8),
    ("Stadium", 0x1d5),
    ("Stadium1", 0x1d6),
    ("Stadium2", 0x1d7),
    ("Swamp", 0x1e3),
    ("Swamp1", 0x1e4),
    ("Swamp2", 0x1e5),
    ("Swamp3", 0x1e6),
];

/// The tips as title and text (0x57bf0): `TIP` with `TIP_1` to `TIP_9`,
/// `LONGTIP` with `LONGTIP_1` to `LONGTIP_3`, then `HINT` with `HINT_1`
/// and `HINT_2`.
const TIPS: [(usize, usize); 14] = [
    (0x1f3, 0x1f4),
    (0x1f3, 0x1f5),
    (0x1f3, 0x1f6),
    (0x1f3, 0x1f7),
    (0x1f3, 0x1f8),
    (0x1f3, 0x1f9),
    (0x1f3, 0x1fa),
    (0x1f3, 0x1fb),
    (0x1f3, 0x1fc),
    (0x10f, 0x110),
    (0x10f, 0x111),
    (0x10f, 0x112),
    (0xd0, 0xd1),
    (0xd0, 0xd2),
];
/// The original draws one of the first 13 (0xaabf0(13)), so `HINT_2`
/// never shows.
const TIP_CHOICES: usize = 13;
const LONGTIP: usize = 0x10f;

/// Sizes (scale 0.65, 0.6, 0.4 and the long tips' 0.3) and where each
/// text is centred, in screen units.
const CAPTION: f32 = 22.4;
const CAPTION_Y: f32 = 50.0;
const TIP_TITLE: f32 = 20.6;
const TIP_TITLE_Y: f32 = 258.0;
const TIP: f32 = 13.8;
const LONG_TIP: f32 = 10.3;
const MODE_Y: f32 = 430.0;
/// The tip's box, its text from the top (0x21).
const TIP_BOX: Rect = Rect { min: Vec2::new(32.0, 276.0), max: Vec2::new(608.0, 356.0) };
const STRETCH: f32 = 1.2;

const BLUE: Color = Color::srgb(0.2, 0.4, 0.8);
const GREY: Color = Color::srgb(0.7, 0.7, 0.7);

/// The language file, a value per line.
struct Strings(Vec<String>);

impl Strings {
    fn parse(text: &str) -> Self {
        Self(text.lines().map(|l| l.trim().to_string()).collect())
    }

    fn get(&self, line: usize) -> &str {
        self.0.get(line).map_or("", String::as_str)
    }

    fn name(&self, key: &str) -> Option<&str> {
        NAMES.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|&(_, l)| self.get(l)).filter(|s| !s.is_empty())
    }

    /// "Dunes : Lagoon": the area (the track without its number), then
    /// the track.
    fn caption(&self, track: &str) -> String {
        let area = track.trim_end_matches(|c: char| c.is_ascii_digit());
        format!("{} : {}", self.name(area).unwrap_or(area), self.name(track).unwrap_or(track))
    }
}

/// The game type's name (0x86410).
fn mode_line(mode: Mode, players: usize, demo: bool) -> usize {
    let split = players > 1;
    match mode {
        _ if demo => 0x94,                    // DEMO
        Mode::SingleRace if split => 0x14d,   // MULTIPLAYER_SINGLE_RACE
        Mode::SingleRace => 0x1c7,            // SINGLE_RACE
        Mode::TimeTrial => 0x1f2,             // TIME_TRIAL
        Mode::Arcade => 0xd,                  // ARCADE
        Mode::Career if split => 0x14b,       // MULTIPLAYER_CHAMPIONSHIP
        Mode::Career => 0x43,                 // CAREER
        Mode::Freestyle if split => 0xb5,     // FREESTYLE_BATTLE
        Mode::Freestyle => 0x16c,             // OPEN_FREESTYLE
        Mode::Challenge => 0x54,              // CHALLENGE_FREESTYLE
    }
}

/// Splits `text` into lines no wider than `width` by `measure`.
fn wrap(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if measure(&format!("{l} {word}")) <= width => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

#[derive(Component)]
struct LoadingScreen;

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        // Drawn once the 2D art is in: on entering the state, or a frame in
        // when a race from the command line starts in it.
        app.add_systems(Update, show.run_if(in_state(AppState::Loading)).run_if(not(any_with_component::<LoadingScreen>)));
    }
}

/// A full-width row whose text is centred on the point `y` down.
fn centred(y: f32, size: f32) -> Node {
    Node { position_type: PositionType::Absolute, left: px(0), width: px(SCREEN_W), top: px(y - size / 2.0), justify_content: JustifyContent::Center, ..default() }
}

/// Drawn for a couple of frames before the race loads, since loading
/// blocks.
fn show(
    mut commands: Commands,
    ui: Option<Res<UiAssets>>,
    game: Res<crate::Game>,
    sel: Res<Selection>,
    demo: Option<Res<crate::demo::Demo>>,
) {
    let Some(ui) = ui else { return };
    let text = game.0.read("/Strings/Strings_ENG.txt").map(|b| b.iter().map(|&c| c as char).collect::<String>()).unwrap_or_default();
    let strings = Strings::parse(&text);
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as usize);
    let (title, tip) = TIPS[seed % TIP_CHOICES];
    let tip_size = if title == LONGTIP { LONG_TIP } else { TIP };
    let font = ui.font(FontId::Inside);
    let lines = wrap(strings.get(tip), TIP_BOX.width(), |l| font.width(l, tip_size) * STRETCH);
    let mode = strings.get(LOADING_X_GAME).replace("%s", strings.get(mode_line(sel.mode, sel.players, demo.is_some())));
    commands.spawn((fullscreen(), BackgroundColor(Color::BLACK), DespawnOnExit(AppState::Loading), LoadingScreen)).with_children(|r| {
        r.spawn(Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(50.0),
            margin: UiRect::left(px(-SCREEN_W / 2.0)),
            width: px(SCREEN_W),
            height: px(SCREEN_H),
            ..default()
        })
        .with_children(|f| {
            let picture = ui.tracks.get(&sel.track.to_ascii_lowercase()).unwrap_or(&ui.loading);
            for tex in [picture, &ui.loading_overlay] {
                let (mut n, img) = tex.node(SCREEN_W, SCREEN_H);
                n.position_type = PositionType::Absolute;
                f.spawn((n, img));
            }
            f.spawn(centred(CAPTION_Y, CAPTION)).with_child(two_tone(CAPTION, strings.caption(&sel.track), BLUE, Color::WHITE));
            f.spawn(centred(TIP_TITLE_Y, TIP_TITLE)).with_child(two_tone(TIP_TITLE, strings.get(title), Color::BLACK, GREY));
            f.spawn(Node {
                position_type: PositionType::Absolute,
                left: px(TIP_BOX.min.x),
                top: px(TIP_BOX.min.y),
                width: px(TIP_BOX.width()),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|b| {
                for l in lines {
                    b.spawn(two_tone(tip_size, l, GREY, Color::BLACK));
                }
            });
            f.spawn(centred(MODE_Y, TIP)).with_child(two_tone(TIP, mode, BLUE, Color::WHITE));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings() -> Strings {
        let mut lines = vec![String::new(); 0x210];
        for (l, v) in [(0x9e, "Dunes"), (0x9f, "Lagoon"), (0xe5, "Works"), (0xe6, "Quarry pass"), (0x108, "Loading %s Game"), (0x14d, "Multiplayer Single Race")] {
            lines[l] = v.to_string();
        }
        Strings::parse(&lines.join("\r\n"))
    }

    #[test]
    fn captions_name_the_area_then_the_track() {
        let s = strings();
        assert_eq!(s.caption("Dunes1"), "Dunes : Lagoon");
        assert_eq!(s.caption("Industrial1"), "Works : Quarry pass");
        // A track the table doesn't know keeps its own name.
        assert_eq!(s.caption("Moon2"), "Moon : Moon2");
    }

    #[test]
    fn split_screen_races_name_their_own_game() {
        let s = strings();
        let mode = s.get(LOADING_X_GAME).replace("%s", s.get(mode_line(Mode::SingleRace, 2, false)));
        assert_eq!(mode, "Loading Multiplayer Single Race Game");
        assert_eq!(mode_line(Mode::SingleRace, 1, true), 0x94);
        assert_eq!(mode_line(Mode::Freestyle, 3, false), 0xb5);
    }

    #[test]
    fn the_last_hint_is_never_picked() {
        assert!(TIPS[..TIP_CHOICES].iter().all(|&t| t != (0xd0, 0xd2)));
        assert_eq!(TIPS[..TIP_CHOICES].iter().filter(|t| t.0 == LONGTIP).count(), 3);
    }

    #[test]
    fn tips_wrap_by_width() {
        let lines = wrap("aa bb cc dd", 5.0, |l| l.len() as f32);
        assert_eq!(lines, ["aa bb", "cc dd"]);
    }
}
