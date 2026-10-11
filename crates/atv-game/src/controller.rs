//! Controller configuration: each controller port's button layout and
//! vibration, as the original's Control Configuration page keeps them
//! (0x49d20: a row per controller with its vibration toggle and one of
//! three layouts, saved per player as `Player%d` `Shocks` and `BtnLayout`,
//! 0x12c80). The original's layouts are its table at 0x1d2128 (three
//! layouts of 21 actions, read by 0x87d90); here they are the PC pad's
//! version of each. On top of them a Custom layout rebinds every action
//! on the pad and the keyboard.
//!
//! Each port's setup drives the pad in that port. The keyboard belongs to
//! players 1 and 2: alone, player 1's keys drive; in split screen a key
//! another player has as their first choice is theirs, so player 1's
//! spare arrow keys go to player 2.

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::prelude::*;

/// Controller ports, as on the Xbox.
pub const PORTS: usize = 4;
/// The original's layouts; one past them is Custom.
pub const PRESETS: usize = 3;
pub const CUSTOM: usize = PRESETS;
/// A stick pushed this far counts; below it the stick is at rest.
const DEAD: f32 = 0.15;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Accelerate,
    Brake,
    SteerLeft,
    SteerRight,
    LeanForward,
    LeanBack,
    Hop,
    Boost,
    Kick,
    Trick1,
    Trick2,
    Trick3,
    Trick4,
    Camera,
    Respot,
    Restart,
}

impl Action {
    pub const ALL: [Action; 16] = [
        Action::Accelerate,
        Action::Brake,
        Action::SteerLeft,
        Action::SteerRight,
        Action::LeanForward,
        Action::LeanBack,
        Action::Hop,
        Action::Boost,
        Action::Kick,
        Action::Trick1,
        Action::Trick2,
        Action::Trick3,
        Action::Trick4,
        Action::Camera,
        Action::Respot,
        Action::Restart,
    ];
    pub const TRICKS: [Action; 4] = [Action::Trick1, Action::Trick2, Action::Trick3, Action::Trick4];

    pub fn label(self) -> &'static str {
        match self {
            Action::Accelerate => "ACCELERATE",
            Action::Brake => "BRAKE AND REVERSE",
            Action::SteerLeft => "STEER LEFT",
            Action::SteerRight => "STEER RIGHT",
            Action::LeanForward => "LEAN FORWARD",
            Action::LeanBack => "LEAN BACK",
            Action::Hop => "PRE-LOAD AND HOP",
            Action::Boost => "SPEED BOOST",
            Action::Kick => "KICK",
            Action::Trick1 => "TRICK BUTTON 1",
            Action::Trick2 => "TRICK BUTTON 2",
            Action::Trick3 => "TRICK BUTTON 3",
            Action::Trick4 => "TRICK BUTTON 4",
            Action::Camera => "CHANGE CAMERA",
            Action::Respot => "BACK ON TRACK",
            Action::Restart => "RESTART",
        }
    }

    /// Its name in the options file.
    fn key(self) -> &'static str {
        match self {
            Action::Accelerate => "accelerate",
            Action::Brake => "brake",
            Action::SteerLeft => "steer_left",
            Action::SteerRight => "steer_right",
            Action::LeanForward => "lean_forward",
            Action::LeanBack => "lean_back",
            Action::Hop => "hop",
            Action::Boost => "boost",
            Action::Kick => "kick",
            Action::Trick1 => "trick1",
            Action::Trick2 => "trick2",
            Action::Trick3 => "trick3",
            Action::Trick4 => "trick4",
            Action::Camera => "camera",
            Action::Respot => "respot",
            Action::Restart => "restart",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&a| a == self).unwrap_or(0)
    }

    /// Pad buttons that are trick buttons in the air don't do this there
    /// (the original's X brakes on the ground and tricks in the air).
    fn grounded(self) -> bool {
        matches!(self, Action::Brake | Action::Hop | Action::Boost | Action::Kick)
    }
}

/// A pad button, or a stick pushed one way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PadInput {
    Button(GamepadButton),
    Axis(GamepadAxis, bool),
}

use GamepadAxis as Ax;
use GamepadButton as Bt;
use PadInput::{Axis, Button};

/// Every pad input that can be bound, with its name (Start is kept for
/// pausing).
const PAD_INPUTS: [(PadInput, &str); 23] = [
    (Button(Bt::South), "A"),
    (Button(Bt::East), "B"),
    (Button(Bt::West), "X"),
    (Button(Bt::North), "Y"),
    (Button(Bt::LeftTrigger), "LB"),
    (Button(Bt::RightTrigger), "RB"),
    (Button(Bt::LeftTrigger2), "LT"),
    (Button(Bt::RightTrigger2), "RT"),
    (Button(Bt::Select), "BACK"),
    (Button(Bt::LeftThumb), "L STICK CLICK"),
    (Button(Bt::RightThumb), "R STICK CLICK"),
    (Button(Bt::DPadUp), "D-PAD UP"),
    (Button(Bt::DPadDown), "D-PAD DOWN"),
    (Button(Bt::DPadLeft), "D-PAD LEFT"),
    (Button(Bt::DPadRight), "D-PAD RIGHT"),
    (Axis(Ax::LeftStickX, false), "L STICK LEFT"),
    (Axis(Ax::LeftStickX, true), "L STICK RIGHT"),
    (Axis(Ax::LeftStickY, true), "L STICK UP"),
    (Axis(Ax::LeftStickY, false), "L STICK DOWN"),
    (Axis(Ax::RightStickX, false), "R STICK LEFT"),
    (Axis(Ax::RightStickX, true), "R STICK RIGHT"),
    (Axis(Ax::RightStickY, true), "R STICK UP"),
    (Axis(Ax::RightStickY, false), "R STICK DOWN"),
];

/// Every key that can be bound, with its name (Escape is kept for
/// pausing).
const KEYS: [(KeyCode, &str); 97] = [
    (KeyCode::KeyA, "A"),
    (KeyCode::KeyB, "B"),
    (KeyCode::KeyC, "C"),
    (KeyCode::KeyD, "D"),
    (KeyCode::KeyE, "E"),
    (KeyCode::KeyF, "F"),
    (KeyCode::KeyG, "G"),
    (KeyCode::KeyH, "H"),
    (KeyCode::KeyI, "I"),
    (KeyCode::KeyJ, "J"),
    (KeyCode::KeyK, "K"),
    (KeyCode::KeyL, "L"),
    (KeyCode::KeyM, "M"),
    (KeyCode::KeyN, "N"),
    (KeyCode::KeyO, "O"),
    (KeyCode::KeyP, "P"),
    (KeyCode::KeyQ, "Q"),
    (KeyCode::KeyR, "R"),
    (KeyCode::KeyS, "S"),
    (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"),
    (KeyCode::KeyV, "V"),
    (KeyCode::KeyW, "W"),
    (KeyCode::KeyX, "X"),
    (KeyCode::KeyY, "Y"),
    (KeyCode::KeyZ, "Z"),
    (KeyCode::Digit0, "0"),
    (KeyCode::Digit1, "1"),
    (KeyCode::Digit2, "2"),
    (KeyCode::Digit3, "3"),
    (KeyCode::Digit4, "4"),
    (KeyCode::Digit5, "5"),
    (KeyCode::Digit6, "6"),
    (KeyCode::Digit7, "7"),
    (KeyCode::Digit8, "8"),
    (KeyCode::Digit9, "9"),
    (KeyCode::ArrowUp, "UP"),
    (KeyCode::ArrowDown, "DOWN"),
    (KeyCode::ArrowLeft, "LEFT"),
    (KeyCode::ArrowRight, "RIGHT"),
    (KeyCode::Numpad0, "NUM 0"),
    (KeyCode::Numpad1, "NUM 1"),
    (KeyCode::Numpad2, "NUM 2"),
    (KeyCode::Numpad3, "NUM 3"),
    (KeyCode::Numpad4, "NUM 4"),
    (KeyCode::Numpad5, "NUM 5"),
    (KeyCode::Numpad6, "NUM 6"),
    (KeyCode::Numpad7, "NUM 7"),
    (KeyCode::Numpad8, "NUM 8"),
    (KeyCode::Numpad9, "NUM 9"),
    (KeyCode::NumpadAdd, "NUM PLUS"),
    (KeyCode::NumpadSubtract, "NUM MINUS"),
    (KeyCode::NumpadMultiply, "NUM STAR"),
    (KeyCode::NumpadDivide, "NUM SLASH"),
    (KeyCode::NumpadDecimal, "NUM DOT"),
    (KeyCode::NumpadEnter, "NUM ENTER"),
    (KeyCode::Space, "SPACE"),
    (KeyCode::ShiftLeft, "LEFT SHIFT"),
    (KeyCode::ShiftRight, "RIGHT SHIFT"),
    (KeyCode::ControlLeft, "LEFT CTRL"),
    (KeyCode::ControlRight, "RIGHT CTRL"),
    (KeyCode::AltLeft, "LEFT ALT"),
    (KeyCode::AltRight, "RIGHT ALT"),
    (KeyCode::Enter, "ENTER"),
    (KeyCode::Tab, "TAB"),
    (KeyCode::Backspace, "BACKSPACE"),
    (KeyCode::Insert, "INSERT"),
    (KeyCode::Delete, "DELETE"),
    (KeyCode::Home, "HOME"),
    (KeyCode::End, "END"),
    (KeyCode::PageUp, "PAGE UP"),
    (KeyCode::PageDown, "PAGE DOWN"),
    (KeyCode::Comma, "COMMA"),
    (KeyCode::Period, "PERIOD"),
    (KeyCode::Slash, "SLASH"),
    (KeyCode::Semicolon, "SEMICOLON"),
    (KeyCode::Quote, "QUOTE"),
    (KeyCode::BracketLeft, "LEFT BRACKET"),
    (KeyCode::BracketRight, "RIGHT BRACKET"),
    (KeyCode::Minus, "MINUS"),
    (KeyCode::Equal, "EQUALS"),
    (KeyCode::Backquote, "BACKQUOTE"),
    (KeyCode::Backslash, "BACKSLASH"),
    (KeyCode::CapsLock, "CAPS LOCK"),
    (KeyCode::F1, "F1"),
    (KeyCode::F2, "F2"),
    (KeyCode::F3, "F3"),
    (KeyCode::F4, "F4"),
    (KeyCode::F5, "F5"),
    (KeyCode::F6, "F6"),
    (KeyCode::F7, "F7"),
    (KeyCode::F8, "F8"),
    (KeyCode::F9, "F9"),
    (KeyCode::F10, "F10"),
    (KeyCode::F11, "F11"),
    (KeyCode::F12, "F12"),
    (KeyCode::ContextMenu, "MENU"),
];

pub fn pad_name(p: PadInput) -> &'static str {
    PAD_INPUTS.iter().find(|(i, _)| *i == p).map_or("?", |(_, n)| n)
}

pub fn key_name(k: KeyCode) -> &'static str {
    KEYS.iter().find(|(i, _)| *i == k).map_or("?", |(_, n)| n)
}

/// Names in the options file: the shown name with `_` for spaces.
fn token(name: &str) -> String {
    name.replace(' ', "_")
}

fn parse_pad(t: &str) -> Option<PadInput> {
    PAD_INPUTS.iter().find(|(_, n)| token(n) == t).map(|(p, _)| *p)
}

fn parse_key(t: &str) -> Option<KeyCode> {
    KEYS.iter().find(|(_, n)| token(n) == t).map(|(k, _)| *k)
}

/// Whether a key can be bound (it has a name; Escape pauses).
pub fn bindable_key(k: KeyCode) -> bool {
    KEYS.iter().any(|(i, _)| *i == k)
}

/// One action's inputs: two on the pad and two on the keyboard, either
/// of which does it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Binding {
    pub pad: [Option<PadInput>; 2],
    pub keys: [Option<KeyCode>; 2],
}

pub type Bindings = [Binding; Action::ALL.len()];

/// The pad's side of each preset, from the original's three (0x1d2128):
/// 1 drives on A and X (here also the triggers, as the PC port always
/// has), 2 on the right stick, 3 on the triggers with Y hopping. Steering
/// is the left stick or the d-pad and leaning the stick (here the d-pad
/// too) in all three; the trick buttons are the port's (see
/// [`crate::freestyle::TRICK_PAD`]).
fn preset_pad(layout: usize, action: Action) -> [Option<PadInput>; 2] {
    let one = |p: PadInput| [Some(p), None];
    let two = |a: PadInput, b: PadInput| [Some(a), Some(b)];
    match (layout, action) {
        (1, Action::Accelerate) => one(Axis(Ax::RightStickY, true)),
        (1, Action::Brake) => one(Axis(Ax::RightStickY, false)),
        (2, Action::Accelerate) => one(Button(Bt::RightTrigger2)),
        (2, Action::Brake) => one(Button(Bt::LeftTrigger2)),
        (2, Action::Hop) => two(Button(Bt::North), Button(Bt::RightTrigger)),
        (2, Action::Boost) => one(Button(Bt::South)),
        (_, Action::Accelerate) => two(Button(Bt::RightTrigger2), Button(Bt::South)),
        (_, Action::Brake) => two(Button(Bt::LeftTrigger2), Button(Bt::West)),
        (_, Action::SteerLeft) => two(Axis(Ax::LeftStickX, false), Button(Bt::DPadLeft)),
        (_, Action::SteerRight) => two(Axis(Ax::LeftStickX, true), Button(Bt::DPadRight)),
        (_, Action::LeanForward) => two(Axis(Ax::LeftStickY, true), Button(Bt::DPadUp)),
        (_, Action::LeanBack) => two(Axis(Ax::LeftStickY, false), Button(Bt::DPadDown)),
        (_, Action::Hop) => two(Button(Bt::East), Button(Bt::RightTrigger)),
        (_, Action::Boost) => one(Button(Bt::North)),
        (_, Action::Kick) => one(Button(Bt::LeftTrigger)),
        (_, Action::Trick1) => one(Button(Bt::West)),
        (_, Action::Trick2) => one(Button(Bt::North)),
        (_, Action::Trick3) => one(Button(Bt::LeftTrigger)),
        (_, Action::Trick4) => one(Button(Bt::RightTrigger)),
        (_, Action::Camera) => one(Button(Bt::Select)),
        (_, Action::Respot | Action::Restart) => [None, None],
    }
}

/// The keyboard's side: player 1 has WASD (or the arrows), the number row
/// and the keys around them; player 2 the arrows and the number pad; the
/// others none.
fn default_keys(port: usize, action: Action) -> [Option<KeyCode>; 2] {
    use KeyCode as K;
    let k = |a: K, b: Option<K>| [Some(a), b];
    match port {
        0 => match action {
            Action::Accelerate => k(K::KeyW, Some(K::ArrowUp)),
            Action::Brake => k(K::KeyS, Some(K::ArrowDown)),
            Action::SteerLeft => k(K::KeyA, Some(K::ArrowLeft)),
            Action::SteerRight => k(K::KeyD, Some(K::ArrowRight)),
            Action::LeanForward => k(K::KeyI, None),
            Action::LeanBack => k(K::KeyK, None),
            Action::Hop => k(K::Space, None),
            Action::Boost => k(K::ShiftLeft, Some(K::ShiftRight)),
            Action::Kick => k(K::KeyE, None),
            Action::Trick1 => k(K::Digit1, Some(K::Numpad1)),
            Action::Trick2 => k(K::Digit2, Some(K::Numpad2)),
            Action::Trick3 => k(K::Digit3, Some(K::Numpad3)),
            Action::Trick4 => k(K::Digit4, Some(K::Numpad4)),
            Action::Camera => k(K::KeyC, None),
            Action::Respot => k(K::KeyR, None),
            Action::Restart => k(K::Backspace, None),
        },
        1 => match action {
            Action::Accelerate => k(K::ArrowUp, None),
            Action::Brake => k(K::ArrowDown, None),
            Action::SteerLeft => k(K::ArrowLeft, None),
            Action::SteerRight => k(K::ArrowRight, None),
            Action::LeanForward => k(K::Numpad8, None),
            Action::LeanBack => k(K::Numpad5, None),
            Action::Hop => k(K::ShiftRight, None),
            Action::Boost => k(K::Numpad0, None),
            Action::Kick => k(K::Numpad6, None),
            Action::Trick1 => k(K::Numpad1, None),
            Action::Trick2 => k(K::Numpad2, None),
            Action::Trick3 => k(K::Numpad3, None),
            Action::Trick4 => k(K::Numpad4, None),
            Action::Camera | Action::Respot | Action::Restart => [None, None],
        },
        _ => [None, None],
    }
}

/// A preset layout's bindings for controller `port`.
pub fn preset(layout: usize, port: usize) -> Bindings {
    Action::ALL.map(|a| Binding { pad: preset_pad(layout, a), keys: default_keys(port, a) })
}

/// One controller port's setup.
#[derive(Clone, Debug, PartialEq)]
pub struct PadSetup {
    /// A preset (0-based), or [`CUSTOM`].
    pub layout: usize,
    pub vibration: bool,
    /// The Custom layout's bindings, kept while another layout is picked.
    pub custom: Bindings,
}

impl PadSetup {
    pub fn new(port: usize) -> Self {
        Self { layout: 0, vibration: true, custom: preset(0, port) }
    }

    pub fn bindings(&self, port: usize) -> Bindings {
        if self.layout == CUSTOM { self.custom } else { preset(self.layout, port) }
    }

    /// "LAYOUT 2" (the original's `%s %d`), or "CUSTOM".
    pub fn layout_label(&self) -> String {
        if self.layout == CUSTOM { "CUSTOM".to_string() } else { format!("LAYOUT {}", self.layout + 1) }
    }

    /// Steps through the layouts, as left and right do on the page.
    pub fn step_layout(&mut self, forward: bool) {
        let n = PRESETS + 1;
        self.layout = if forward { (self.layout + 1) % n } else { (self.layout + n - 1) % n };
    }

    /// Changes one input of an action, starting Custom from the layout in
    /// use. `slot` 0-1 is the pad's, 2-3 the keyboard's; `None` clears it.
    pub fn rebind(&mut self, port: usize, action: Action, slot: usize, input: Option<Input>) {
        if self.layout != CUSTOM {
            self.custom = self.bindings(port);
            self.layout = CUSTOM;
        }
        let b = &mut self.custom[action.index()];
        match (slot, input) {
            (0 | 1, None) => b.pad[slot] = None,
            (0 | 1, Some(Input::Pad(p))) => b.pad[slot] = Some(p),
            (2 | 3, None) => b.keys[slot - 2] = None,
            (2 | 3, Some(Input::Key(k))) => b.keys[slot - 2] = Some(k),
            _ => {}
        }
    }

    /// Back to the first layout and the port's own keys.
    pub fn reset(&mut self, port: usize) {
        self.layout = 0;
        self.custom = preset(0, port);
    }

    /// `pad1_layout = 2` and so on, for the options file.
    pub fn to_text(&self, port: usize) -> String {
        let n = port + 1;
        let layout = if self.layout == CUSTOM { "custom".to_string() } else { (self.layout + 1).to_string() };
        let mut s = format!("pad{n}_layout = {layout}\npad{n}_vibration = {}\n", self.vibration);
        if self.custom != preset(0, port) {
            for (a, b) in Action::ALL.iter().zip(&self.custom) {
                let pad = b.pad.map(|p| p.map_or("-".to_string(), |p| token(pad_name(p))));
                let keys = b.keys.map(|k| k.map_or("-".to_string(), |k| token(key_name(k))));
                s += &format!("pad{n}_{} = {} {} / {} {}\n", a.key(), pad[0], pad[1], keys[0], keys[1]);
            }
        }
        s
    }

    /// Reads one `padN_...` line's key (after `padN_`) and value.
    pub fn parse_line(&mut self, key: &str, value: &str) {
        match key {
            "layout" if value == "custom" => self.layout = CUSTOM,
            "layout" => self.layout = value.parse::<usize>().ok().filter(|n| (1..=PRESETS).contains(n)).map_or(self.layout, |n| n - 1),
            "vibration" => self.vibration = value == "true",
            _ => {
                let Some(a) = Action::ALL.iter().find(|a| a.key() == key) else { return };
                let (pad, keys) = value.split_once('/').unwrap_or((value, ""));
                let b = &mut self.custom[a.index()];
                let mut pad = pad.split_whitespace().map(parse_pad);
                b.pad = [pad.next().flatten(), pad.next().flatten()];
                let mut keys = keys.split_whitespace().map(parse_key);
                b.keys = [keys.next().flatten(), keys.next().flatten()];
            }
        }
    }
}

pub fn default_setups() -> [PadSetup; PORTS] {
    std::array::from_fn(PadSetup::new)
}

/// A pad input or a key, as the remap page takes one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Input {
    Pad(PadInput),
    Key(KeyCode),
}

/// What's pressed this frame for the remap page: the first new key, or a
/// pad button or stick direction newly pushed on any pad. `held` carries
/// which sticks were already pushed, so a stick still held from before
/// doesn't count.
pub fn captured(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>, held: &mut Vec<PadInput>) -> Option<Input> {
    if let Some(k) = keys.get_just_pressed().copied().find(|k| bindable_key(*k)) {
        return Some(Input::Key(k));
    }
    let mut now = Vec::new();
    let mut found = None;
    for pad in pads.iter() {
        for (p, _) in PAD_INPUTS {
            match p {
                Button(b) if pad.just_pressed(b) => found = found.or(Some(p)),
                Axis(..) if pad_level(pad, p) > 0.6 => {
                    if !held.contains(&p) {
                        found = found.or(Some(p));
                    }
                    now.push(p);
                }
                _ => {}
            }
        }
    }
    *held = now;
    found.map(Input::Pad)
}

/// How far one pad input is pushed, 0 to 1.
fn pad_level(pad: &Gamepad, p: PadInput) -> f32 {
    match p {
        Button(b) => {
            let v = pad.get(b).unwrap_or(0.0);
            if pad.pressed(b) { v.max(1.0) } else { v.clamp(0.0, 1.0) }
        }
        Axis(a, positive) => {
            let v = pad.get(a).unwrap_or(0.0) * if positive { 1.0 } else { -1.0 };
            if v < DEAD { 0.0 } else { v.min(1.0) }
        }
    }
}

/// Reads the actions with the controller setups. `slot`: in split screen,
/// the player (0-based), who has their own pad (the slot-th) and keys;
/// alone, every pad drives (each with its own port's layout) and player
/// 1's keys.
pub struct Reader<'a> {
    keys: &'a ButtonInput<KeyCode>,
    pads: Vec<&'a Gamepad>,
    setups: &'a [PadSetup; PORTS],
}

impl<'a> Reader<'a> {
    pub fn new(keys: &'a ButtonInput<KeyCode>, pads: &'a Query<&Gamepad>, setups: &'a [PadSetup; PORTS]) -> Self {
        Self { keys, pads: pads.iter().collect(), setups }
    }

    fn port_bindings(&self, port: usize) -> Bindings {
        let port = port.min(PORTS - 1);
        self.setups[port].bindings(port)
    }

    /// Whether player `port`'s `k` counts: in split screen a key another
    /// player has as their first choice is theirs (the earlier player's,
    /// if both do).
    fn key_is_mine(&self, port: usize, k: KeyCode, split: bool) -> bool {
        if !split {
            return true;
        }
        let mine_first = self.port_bindings(port).iter().any(|b| b.keys[0] == Some(k));
        !(0..PORTS).filter(|&o| o != port).any(|o| self.port_bindings(o).iter().any(|b| b.keys[0] == Some(k)) && (!mine_first || o < port))
    }

    /// How far `action` is pushed, 0 to 1. `air`: pad buttons that are
    /// trick buttons don't brake, hop, boost or kick in the air.
    pub fn level(&self, slot: Option<usize>, action: Action, air: bool) -> f32 {
        let skip = |b: &Bindings, input: Input| {
            air && action.grounded()
                && Action::TRICKS.iter().any(|t| {
                    let t = &b[t.index()];
                    match input {
                        Input::Pad(p) => t.pad.contains(&Some(p)),
                        Input::Key(k) => t.keys.contains(&Some(k)),
                    }
                })
        };
        let mut v: f32 = 0.0;
        let port = slot.unwrap_or(0);
        let b = self.port_bindings(port);
        for k in b[action.index()].keys.into_iter().flatten() {
            if self.keys.pressed(k) && self.key_is_mine(port, k, slot.is_some()) && !skip(&b, Input::Key(k)) {
                v = 1.0;
            }
        }
        for (n, pad) in self.pads.iter().copied().enumerate().filter(|(n, _)| slot.is_none_or(|s| s == *n)) {
            let b = self.port_bindings(n);
            for p in b[action.index()].pad.into_iter().flatten() {
                if !skip(&b, Input::Pad(p)) {
                    v = v.max(pad_level(pad, p));
                }
            }
        }
        v
    }

    pub fn pressed(&self, slot: Option<usize>, action: Action, air: bool) -> bool {
        self.level(slot, action, air) > 0.5
    }

    /// Whether `action` was pressed this frame (sticks don't count).
    pub fn just_pressed(&self, slot: Option<usize>, action: Action) -> bool {
        let port = slot.unwrap_or(0);
        let keys = self.port_bindings(port)[action.index()].keys.into_iter().flatten().any(|k| self.keys.just_pressed(k) && self.key_is_mine(port, k, slot.is_some()));
        keys || self.pads.iter().copied().enumerate().filter(|(n, _)| slot.is_none_or(|s| s == *n)).any(|(n, pad)| {
            self.port_bindings(n)[action.index()].pad.into_iter().flatten().any(|p| matches!(p, Button(b) if pad.just_pressed(b)))
        })
    }

    /// The keyboard's accelerate and brake keys, which also push the
    /// stick up and down for tricks.
    pub fn key_level(&self, slot: Option<usize>, action: Action) -> f32 {
        let port = slot.unwrap_or(0);
        let held = self.port_bindings(port)[action.index()].keys.into_iter().flatten().any(|k| self.keys.pressed(k) && self.key_is_mine(port, k, slot.is_some()));
        if held { 1.0 } else { 0.0 }
    }
}

/// Each action's inputs as the Controller pages list them: action, pad,
/// keys ("RT OR A").
pub fn table(setup: &PadSetup, port: usize) -> Vec<(&'static str, String, String)> {
    let b = setup.bindings(port);
    Action::ALL
        .iter()
        .zip(b)
        .map(|(a, b)| {
            let pad: Vec<&str> = b.pad.into_iter().flatten().map(pad_name).collect();
            let keys: Vec<&str> = b.keys.into_iter().flatten().map(key_name).collect();
            (a.label(), pad.join(" OR "), keys.join(" OR "))
        })
        .collect()
}

/// The trick buttons' names on player 1's pad and keyboard, in mask
/// order, for the Trick Book.
pub fn trick_names(setup: &PadSetup) -> ([&'static str; 4], [&'static str; 4]) {
    let b = setup.bindings(0);
    let first = |a: &Action| &b[a.index()];
    (
        Action::TRICKS.map(|a| first(&a).pad.into_iter().flatten().next().map_or("-", pad_name)),
        Action::TRICKS.map(|a| first(&a).keys.into_iter().flatten().next().map_or("-", key_name)),
    )
}

/// Player 1's first pad button for each trick button, for the Trick
/// Book's pad picture.
pub fn trick_pad(setup: &PadSetup) -> [Option<PadInput>; 4] {
    let b = setup.bindings(0);
    Action::TRICKS.map(|a| b[a.index()].pad.into_iter().flatten().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_lines_round_trip() {
        let mut s = PadSetup::new(1);
        s.vibration = false;
        s.rebind(1, Action::Hop, 0, Some(Input::Pad(Axis(Ax::RightStickY, false))));
        s.rebind(1, Action::Kick, 3, Some(Input::Key(KeyCode::PageDown)));
        s.rebind(1, Action::Boost, 2, None);
        let text = s.to_text(1);
        let mut back = PadSetup::new(1);
        for line in text.lines() {
            let (k, v) = line.split_once('=').unwrap();
            back.parse_line(k.trim().strip_prefix("pad2_").unwrap(), v.trim());
        }
        assert_eq!(back, s);
        assert_eq!(back.layout_label(), "CUSTOM");
    }

    #[test]
    fn untouched_setups_write_no_bindings() {
        let s = PadSetup::new(0);
        assert_eq!(s.to_text(0), "pad1_layout = 1\npad1_vibration = true\n");
    }

    #[test]
    fn editing_a_preset_starts_custom_from_it() {
        let mut s = PadSetup::new(0);
        s.layout = 1;
        s.rebind(0, Action::Kick, 1, Some(Input::Pad(Button(Bt::East))));
        assert_eq!(s.layout, CUSTOM);
        assert_eq!(s.custom[Action::Accelerate.index()].pad[0], Some(Axis(Ax::RightStickY, true)));
        assert_eq!(s.custom[Action::Kick.index()].pad, [Some(Button(Bt::LeftTrigger)), Some(Button(Bt::East))]);
        s.reset(0);
        assert_eq!(s, PadSetup::new(0));
    }

    #[test]
    fn layouts_cycle_through_custom() {
        let mut s = PadSetup::new(0);
        let labels: Vec<String> = (0..4)
            .map(|_| {
                s.step_layout(true);
                s.layout_label()
            })
            .collect();
        assert_eq!(labels, ["LAYOUT 2", "LAYOUT 3", "CUSTOM", "LAYOUT 1"]);
        s.step_layout(false);
        assert_eq!(s.layout_label(), "CUSTOM");
    }

    #[test]
    fn every_name_parses_back() {
        for (p, n) in PAD_INPUTS {
            assert_eq!(parse_pad(&token(n)), Some(p));
        }
        for (k, n) in KEYS {
            assert_eq!(parse_key(&token(n)), Some(k));
        }
    }
}
