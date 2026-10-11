//! Typing a name as the original does (`nameentrypanel`): the name in its
//! box over a keyboard of letters and digits with DEL, SPACE and OK, picked
//! with the arrows or a pad, or typed. Name Your Rider and the record page
//! use it.

use bevy::input::gamepad::Gamepad;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;

use crate::frontend::Blink;
use crate::ui::{read_nav_from, BitmapText, FontId, Nav, UiAssets, SCREEN_W};

pub const KEY_ROWS: [&str; 3] = ["ABCDEFGHIJKLMN", "OPQRSTUVWXYZ", "1234567890-\"*"];
pub const KEY_WORDS: [&str; 3] = ["DEL", "SPACE", "OK"];
/// The longest name the original takes.
pub const NAME_LEN: usize = 12;

const GOLD: Color = Color::srgb(1.0, 0.85, 0.3);

/// A name being typed and the key highlighted (row, column).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NameEntry {
    pub name: String,
    pub key: (usize, usize),
}

impl NameEntry {
    /// Starts from `name`, in the keyboard's capitals and cut to length.
    pub fn new(name: &str) -> Self {
        let mut e = Self::default();
        for c in name.chars().map(|c| c.to_ascii_uppercase()) {
            e.push(c);
        }
        e
    }

    fn push(&mut self, c: char) {
        if self.name.chars().count() < NAME_LEN && !(c == ' ' && self.name.is_empty()) {
            self.name.push(c);
        }
    }

    /// A key typed: true when it typed or deleted.
    pub fn type_key(&mut self, k: &KeyboardInput) -> bool {
        if k.key_code == KeyCode::Backspace {
            self.name.pop();
            return true;
        }
        let Some(text) = &k.text else { return false };
        let mut typed = false;
        for c in text.chars().map(|c| c.to_ascii_uppercase()) {
            if c == ' ' || KEY_ROWS.iter().any(|r| r.contains(c)) {
                self.push(c);
                typed = true;
            }
        }
        typed
    }

    /// Moves about the keyboard or presses the key highlighted; true for
    /// OK (or Start). Back is the caller's.
    pub fn nav(&mut self, nav: Nav) -> bool {
        let row_len = |r: usize| KEY_ROWS.get(r).map_or(KEY_WORDS.len(), |k| k.chars().count());
        let (r, c) = self.key;
        match nav {
            Nav::Left => self.key.1 = (c + row_len(r) - 1) % row_len(r),
            Nav::Right => self.key.1 = (c + 1) % row_len(r),
            Nav::Up | Nav::Down => {
                // To the key across from this one.
                let to = if nav == Nav::Up { (r + 3) % 4 } else { (r + 1) % 4 };
                self.key = (to, ((2 * c + 1) * row_len(to) / (2 * row_len(r))).min(row_len(to) - 1));
            }
            Nav::Accept if r < KEY_ROWS.len() => self.push(KEY_ROWS[r].chars().nth(c).unwrap_or(' ')),
            Nav::Accept if c == 0 => {
                self.name.pop();
            }
            Nav::Accept if c == 1 => self.push(' '),
            Nav::Accept | Nav::Start => return true,
            Nav::Back | Nav::Extra => {}
        }
        false
    }
}

/// Moves while typing a name, where letters, space and backspace type:
/// the arrows, Enter for OK, Escape to go back, and the pads as usual.
pub fn read_nav(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>) -> Option<Nav> {
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

fn at(left: f32, top: f32) -> Node {
    Node { position_type: PositionType::Absolute, left: px(left), top: px(top), ..default() }
}

/// Draws the name in its box `box_top` down and the keyboard from
/// `keys_top`, across the middle of a 640-wide screen; gives the entities
/// for the caller to parent. Also gives where the keyboard's panel ends.
pub fn spawn(commands: &mut Commands, ui: &UiAssets, entry: &NameEntry, box_top: f32, keys_top: f32) -> (Vec<Entity>, f32) {
    let mut kids = Vec::new();
    let small = ui.name_entry.get("SMALLNAMEPANEL").rect;
    let (bw, bs) = (320.0, 320.0 / small.width());
    let bx = (SCREEN_W - bw) / 2.0;
    kids.push(commands.spawn(crate::ui::placed(ui.name_entry.node("SMALLNAMEPANEL", bs), bx, box_top)).id());
    let menu_font = ui.font(FontId::Menu);
    let w = menu_font.width(&entry.name, 18.0);
    let tx = (SCREEN_W - w) / 2.0;
    let ty = box_top + (small.height() * bs - 18.0) / 2.0;
    kids.push(commands.spawn((BitmapText::new(FontId::Menu, 18.0, entry.name.clone()), at(tx, ty))).id());
    if entry.name.chars().count() < NAME_LEN {
        kids.push(commands.spawn((BitmapText::new(FontId::Menu, 18.0, "_").color(GOLD), at(tx + w + 2.0, ty), Blink)).id());
    }
    let panel = ui.name_entry.get("NAMEPANEL").rect;
    let (pw, ps) = (540.0, 540.0 / panel.width());
    let px0 = (SCREEN_W - pw) / 2.0;
    kids.push(commands.spawn(crate::ui::placed(ui.name_entry.node("NAMEPANEL", ps), px0, keys_top)).id());
    let row_h = (panel.height() * ps - 24.0) / 4.0;
    let mut rows: Vec<Vec<String>> = KEY_ROWS.iter().map(|r| r.chars().map(String::from).collect()).collect();
    rows.push(KEY_WORDS.iter().map(|w| w.to_string()).collect());
    for (r, keys) in rows.iter().enumerate() {
        let words = r == KEY_ROWS.len();
        let step = if words { 150.0 } else { 36.0 };
        let x_mid = SCREEN_W / 2.0 - step * (keys.len() - 1) as f32 / 2.0;
        for (c, key) in keys.iter().enumerate() {
            let on = entry.key == (r, c);
            let size = if on { 20.0 } else { 16.0 };
            let kw = menu_font.width(key, size);
            let y = keys_top + 12.0 + r as f32 * row_h + (row_h - size) / 2.0;
            let text = BitmapText::new(FontId::Menu, size, key.clone()).color(if on { GOLD } else { Color::WHITE });
            kids.push(commands.spawn((text, at(x_mid + step * c as f32 - kw / 2.0, y))).id());
        }
    }
    (kids, keys_top + panel.height() * ps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_start_in_capitals_and_cut_to_length() {
        assert_eq!(NameEntry::new("Phoob the quick").name, "PHOOB THE QU");
        assert_eq!(NameEntry::new(" x").name, "X", "no leading space");
    }

    #[test]
    fn the_keys_type_delete_and_finish() {
        let mut e = NameEntry::new("AB");
        assert!(!e.nav(Nav::Accept), "A types");
        assert_eq!(e.name, "ABA");
        e.key = (3, 0);
        assert!(!e.nav(Nav::Accept));
        assert_eq!(e.name, "AB", "DEL");
        e.key = (3, 1);
        e.nav(Nav::Accept);
        assert_eq!(e.name, "AB ", "SPACE");
        e.key = (3, 2);
        assert!(e.nav(Nav::Accept), "OK");
        assert!(e.nav(Nav::Start), "Start is OK too");
        e.nav(Nav::Up);
        assert_eq!(e.key.0, 2, "up from the words to the digits");
    }
}
