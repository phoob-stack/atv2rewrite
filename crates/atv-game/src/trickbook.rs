//! The Trick Book (`Trickbook` in Frontend.xml): one page per trick in the
//! original's trick order, with its name, level, points and activation lit
//! on the trick pad picture, and the rider performing it beside. A trick
//! above the viewer's skill level is greyed with the skill points still
//! needed (0x536d0); for another player's rider, whose level is its own
//! skill rating, locked tricks are skipped (0x52850). The original opens
//! it with Y from rider select, track select and the career status page.

use std::sync::Arc;

use bevy::prelude::*;

use crate::controller::PadInput;
use crate::freestyle::{Dir, TrickDef};
use crate::rider::{Clip, Rider};
use crate::ui::{placed, BitmapText, FontId, UiAssets};

/// Trick ids 0 to 27 (the xbe's name table at 0x1d05d0): the book's order.
pub const ORDER: [&str; 28] = [
    "Scissors",
    "Superman",
    "Can_Can",
    "Split_X",
    "Seat_Grab",
    "Nac_Nac",
    "Saran_Wrap",
    "Bar_Hop",
    "Surfer",
    "Heart_Attack",
    "Clicker",
    "Rodeo",
    "Pendulum",
    "Lazy_Boy",
    "Spin_Bike",
    "Yogi",
    "Turn_Around",
    "Handstand_Spin",
    "Cliffhanger",
    "Cordova",
    "Air_Walk",
    "Break_Spin",
    "Scarecrow",
    "Reverse_Spin",
    "Pommel_Horse",
    "Log_Run",
    "Seat_Grab_Handstand",
    "Surfer_Flip",
];

/// The tricks in book order, as indices into `tricks`; any the original
/// doesn't number come after.
pub fn book(tricks: &[TrickDef]) -> Vec<usize> {
    let id = |t: &TrickDef| ORDER.iter().position(|n| n.replace('_', " ").eq_ignore_ascii_case(&t.name)).unwrap_or(ORDER.len());
    let mut order: Vec<usize> = (0..tricks.len()).collect();
    order.sort_by_key(|&i| id(&tricks[i]));
    order
}

/// The trick buttons' names on a pad and on the keyboard, in mask order,
/// with the first layout (the Trick Book names player 1's own, see
/// [`crate::controller::trick_names`]).
#[cfg(test)]
pub const PAD_NAMES: [&str; 4] = ["X", "Y", "LB", "RB"];
#[cfg(test)]
pub const KEY_NAMES: [&str; 4] = ["1", "2", "3", "4"];

/// "Y + LB + UP": the buttons to hold and the direction to push.
pub fn activation(def: &TrickDef, names: &[&str; 4]) -> String {
    let dir = match def.dir {
        Dir::Up => "UP",
        Dir::Down => "DOWN",
        Dir::Left => "LEFT",
        Dir::Right => "RIGHT",
    };
    let mut parts: Vec<&str> = (0..4).filter(|b| def.buttons & (1 << b) != 0).map(|b| names[b]).collect();
    parts.push(dir);
    parts.join(" + ")
}

/// Where a pad button sits on the `TrickPad_XBox` picture: Y, B, A and X
/// on the face, the shoulders on the top view, and the d-pad. Others (and
/// sticks) have no spot.
fn pad_spot(p: PadInput) -> Option<&'static str> {
    use bevy::input::gamepad::GamepadButton as B;
    let PadInput::Button(b) = p else { return None };
    Some(match b {
        B::North => "BUTTON4",
        B::East => "BUTTON5",
        B::South => "BUTTON6",
        B::West => "BUTTON7",
        B::LeftTrigger | B::LeftTrigger2 => "BUTTON8",
        B::RightTrigger | B::RightTrigger2 => "BUTTON9",
        B::DPadUp => "BUTTON0",
        B::DPadDown => "BUTTON1",
        B::DPadLeft => "BUTTON2",
        B::DPadRight => "BUTTON3",
        _ => return None,
    })
}

/// How much larger than drawn on the disc the lit buttons' glows are.
const GLOW: f32 = 1.3;

/// The trick pad picture with the trick's buttons and its direction on the
/// d-pad lit (as 0x52a40 lights them), `scale` units per texel. `buttons`:
/// the pad's trick buttons, in mask order.
pub fn pad_picture(commands: &mut Commands, ui: &UiAssets, def: &TrickDef, buttons: [Option<PadInput>; 4], locked: bool, (left, top): (f32, f32), scale: f32) -> Entity {
    let sheet = &ui.trick_pad;
    let pad = sheet.get("PAD").rect;
    let dim = Color::srgb(0.45, 0.45, 0.45);
    let spot = |name: &str| (sheet.get(name).rect.center() - pad.min) * scale;
    let dir = match def.dir {
        Dir::Up => "BUTTON0",
        Dir::Down => "BUTTON1",
        Dir::Left => "BUTTON2",
        Dir::Right => "BUTTON3",
    };
    commands
        .spawn(Node { position_type: PositionType::Absolute, left: px(left), top: px(top), width: px(pad.width() * scale), height: px(pad.height() * scale), ..default() })
        .with_children(|p| {
            let (n, mut img) = sheet.node("PAD", scale);
            if locked {
                img.color = dim;
            }
            p.spawn(placed((n, img), 0.0, 0.0));
            if locked {
                return;
            }
            let lit = (0..4).filter(|b| def.buttons & (1 << b) != 0).filter_map(|b| buttons[b].and_then(pad_spot)).chain([dir]);
            for at in lit {
                let shoulder = matches!(at, "BUTTON8" | "BUTTON9");
                let name = if shoulder { "SHOULDER1" } else { "BUTTON" };
                // Bigger and gold, so they show on the dark pad.
                let size = sheet.get(name).rect.size() * scale * GLOW;
                let (n, mut img) = sheet.node(name, scale * GLOW);
                img.color = Color::srgb(1.0, 0.85, 0.2);
                // The shoulder glow is drawn for the left trigger.
                img.flip_x = at == "BUTTON9";
                let c = spot(at) - size * 0.5;
                p.spawn(placed((n, img), c.x, c.y));
            }
        })
        .id()
}

/// The page's text: name, level and points.
pub fn heading(commands: &mut Commands, def: &TrickDef, locked: bool, (left, top): (f32, f32)) -> Vec<Entity> {
    let (name, gold) = if locked { (Color::srgb(0.5, 0.5, 0.55), Color::srgb(0.5, 0.45, 0.3)) } else { (Color::WHITE, Color::srgb(1.0, 0.85, 0.3)) };
    let at = |top: f32| Node { position_type: PositionType::Absolute, left: px(left), top: px(top), ..default() };
    vec![
        commands.spawn((BitmapText::new(FontId::Menu, 18.0, def.name.clone()).color(name), at(top))).id(),
        commands.spawn((BitmapText::new(FontId::Small, 9.0, format!("LEVEL {}", def.level)).color(name), at(top + 28.0))).id(),
        commands.spawn((BitmapText::new(FontId::Menu, 14.0, format!("{} POINTS", def.points)).color(gold), at(top + 46.0))).id(),
    ]
}

/// The rider in the trick book's showroom, performing a trick on a loop.
#[derive(Component)]
pub struct TrickDemo {
    pub clip: Option<Arc<Clip>>,
    /// Seconds the trick takes.
    pub total: f32,
    pub t: f32,
}

/// Rest between showings, seconds.
const DEMO_GAP: f32 = 0.8;

/// Plays each demo's trick, rests, and plays it again.
pub fn demo(time: Res<Time>, mut riders: Query<(&mut Rider, &mut TrickDemo)>, mut joints: Query<&mut Transform>) {
    let dt = time.delta_secs();
    for (mut r, mut d) in &mut riders {
        d.t = (d.t + dt) % (d.total + DEMO_GAP);
        let stunt = d.clip.clone().filter(|_| d.t < d.total).map(|c| (c, d.t / d.total));
        r.update_controls(0.0, 0.0, 0.0, dt);
        r.set_stunt(stunt, dt);
        r.apply(dt, &mut joints);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trick(name: &str, buttons: u8, dir: Dir) -> TrickDef {
        TrickDef { name: name.into(), level: 1, buttons, dir, points: 100, hold_points: 20, total: 1.0, end: 0.4, clip: String::new() }
    }

    #[test]
    fn book_order_and_activation() {
        let tricks = [trick("SURFER FLIP", 0b0110, Dir::Up), trick("NEW ONE", 1, Dir::Up), trick("SCISSORS", 1, Dir::Left), trick("SUPERMAN", 1, Dir::Down)];
        assert_eq!(book(&tricks), vec![2, 3, 0, 1]);
        assert_eq!(activation(&tricks[3], &PAD_NAMES), "X + DOWN");
        assert_eq!(activation(&tricks[0], &PAD_NAMES), "Y + LB + UP");
        assert_eq!(activation(&tricks[0], &KEY_NAMES), "2 + 3 + UP");
    }
}
