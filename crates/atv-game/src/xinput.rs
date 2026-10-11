//! Pads read through XInput, for Steam Input on Windows. Steam presents
//! the Steam Controller, the Deck and every pad it manages to the game as
//! XInput pads (it hooks XInput in the game's process), but not through
//! Windows.Gaming.Input, which gilrs reads. So when Steam started the game
//! the pads come from XInput instead, as Bevy gamepads like gilrs's; see
//! [`PadReader`].

/// Where the pads are read from.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PadReader {
    /// XInput when Steam started the game (Steam Input), else native.
    #[default]
    Auto,
    /// gilrs: Xbox, PlayStation, Switch and other pads as Windows sees them.
    Native,
    /// XInput only: Xbox pads and the pads Steam Input presents.
    XInput,
}

impl PadReader {
    pub const ALL: [PadReader; 3] = [PadReader::Auto, PadReader::Native, PadReader::XInput];

    /// Its name in the options file (`pad_reader`).
    pub fn key(self) -> &'static str {
        match self {
            PadReader::Auto => "auto",
            PadReader::Native => "native",
            PadReader::XInput => "xinput",
        }
    }

    pub fn parse(v: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.key().eq_ignore_ascii_case(v.trim()))
    }

    /// Whether to read XInput (and leave gilrs out). Only on Windows.
    pub fn xinput(self) -> bool {
        cfg!(windows)
            && match self {
                PadReader::Auto => started_by_steam(),
                PadReader::Native => false,
                PadReader::XInput => true,
            }
    }
}

/// Whether Steam started the game (as a Steam or non-Steam game), from the
/// variables it sets for what it launches.
fn started_by_steam() -> bool {
    ["SteamGameId", "SteamAppId", "SteamClientLaunch", "SteamOverlayGameId"].iter().any(|v| std::env::var_os(v).is_some_and(|v| !v.is_empty()))
}

#[cfg(windows)]
pub use imp::XInputPlugin;

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use bevy::input::gamepad::{
        GamepadAxis, GamepadButton, GamepadConnection, GamepadConnectionEvent, GamepadRumbleRequest, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
    };
    use bevy::input::InputSystems;
    use bevy::prelude::*;

    #[repr(C)]
    #[derive(Clone, Copy, Default, PartialEq)]
    struct XInputGamepad {
        buttons: u16,
        left_trigger: u8,
        right_trigger: u8,
        thumb_lx: i16,
        thumb_ly: i16,
        thumb_rx: i16,
        thumb_ry: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct XInputState {
        packet: u32,
        gamepad: XInputGamepad,
    }

    #[repr(C)]
    struct XInputVibration {
        left_motor: u16,
        right_motor: u16,
    }

    #[link(name = "xinput1_4", kind = "raw-dylib")]
    unsafe extern "system" {
        fn XInputGetState(user: u32, state: *mut XInputState) -> u32;
        fn XInputSetState(user: u32, vibration: *mut XInputVibration) -> u32;
    }

    const SLOTS: usize = 4;
    const OK: u32 = 0;

    /// XInput's buttons by bit.
    const BUTTONS: [(u16, GamepadButton); 14] = [
        (0x0001, GamepadButton::DPadUp),
        (0x0002, GamepadButton::DPadDown),
        (0x0004, GamepadButton::DPadLeft),
        (0x0008, GamepadButton::DPadRight),
        (0x0010, GamepadButton::Start),
        (0x0020, GamepadButton::Select),
        (0x0040, GamepadButton::LeftThumb),
        (0x0080, GamepadButton::RightThumb),
        (0x0100, GamepadButton::LeftTrigger),
        (0x0200, GamepadButton::RightTrigger),
        (0x1000, GamepadButton::South),
        (0x2000, GamepadButton::East),
        (0x4000, GamepadButton::West),
        (0x8000, GamepadButton::North),
    ];

    /// The four XInput slots: their gamepad entities, last state, and the
    /// rumbles running on each.
    #[derive(Resource, Default)]
    struct Slots {
        pads: [Option<Entity>; SLOTS],
        last: [Option<XInputGamepad>; SLOTS],
        /// Seconds until each empty slot is looked at again (asking an
        /// empty slot is slow).
        retry: [f32; SLOTS],
        rumbles: [Vec<(f32, f32, Duration)>; SLOTS],
        /// Whether each slot's motors are running.
        buzzing: [bool; SLOTS],
    }

    pub struct XInputPlugin;

    impl Plugin for XInputPlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<Slots>().add_systems(PreUpdate, read.before(InputSystems)).add_systems(PostUpdate, rumble);
        }
    }

    fn stick(v: i16) -> f32 {
        (f32::from(v) / 32767.0).clamp(-1.0, 1.0)
    }

    fn read(
        mut commands: Commands,
        mut slots: ResMut<Slots>,
        time: Res<Time<Real>>,
        mut events: MessageWriter<RawGamepadEvent>,
        mut connections: MessageWriter<GamepadConnectionEvent>,
        mut buttons: MessageWriter<RawGamepadButtonChangedEvent>,
        mut axes: MessageWriter<RawGamepadAxisChangedEvent>,
    ) {
        let dt = time.delta_secs();
        for n in 0..SLOTS {
            if slots.last[n].is_none() {
                slots.retry[n] -= dt;
                if slots.retry[n] > 0.0 {
                    continue;
                }
                slots.retry[n] = 1.0;
            }
            let mut state = XInputState::default();
            // SAFETY: a plain call with a valid out pointer.
            let connected = unsafe { XInputGetState(n as u32, &mut state) } == OK;
            let entity = *slots.pads[n].get_or_insert_with(|| commands.spawn_empty().id());
            let before = slots.last[n];
            if !connected {
                if before.is_some() {
                    slots.last[n] = None;
                    let event = GamepadConnectionEvent::new(entity, GamepadConnection::Disconnected);
                    events.write(event.clone().into());
                    connections.write(event);
                }
                continue;
            }
            let now = state.gamepad;
            if before.is_none() {
                let event = GamepadConnectionEvent::new(entity, GamepadConnection::Connected { name: format!("XInput Controller {}", n + 1), vendor_id: None, product_id: None });
                events.write(event.clone().into());
                connections.write(event);
            }
            if before == Some(now) {
                continue;
            }
            let was = before.unwrap_or_default();
            let mut button = |b: GamepadButton, v: f32| {
                buttons.write(RawGamepadButtonChangedEvent::new(entity, b, v));
                events.write(RawGamepadButtonChangedEvent::new(entity, b, v).into());
            };
            for (bit, b) in BUTTONS {
                if before.is_none() || (now.buttons ^ was.buttons) & bit != 0 {
                    button(b, if now.buttons & bit != 0 { 1.0 } else { 0.0 });
                }
            }
            if before.is_none() || now.left_trigger != was.left_trigger {
                button(GamepadButton::LeftTrigger2, f32::from(now.left_trigger) / 255.0);
            }
            if before.is_none() || now.right_trigger != was.right_trigger {
                button(GamepadButton::RightTrigger2, f32::from(now.right_trigger) / 255.0);
            }
            let sticks = [
                (GamepadAxis::LeftStickX, now.thumb_lx, was.thumb_lx),
                (GamepadAxis::LeftStickY, now.thumb_ly, was.thumb_ly),
                (GamepadAxis::RightStickX, now.thumb_rx, was.thumb_rx),
                (GamepadAxis::RightStickY, now.thumb_ry, was.thumb_ry),
            ];
            for (axis, v, old) in sticks {
                if before.is_none() || v != old {
                    axes.write(RawGamepadAxisChangedEvent::new(entity, axis, stick(v)));
                    events.write(RawGamepadAxisChangedEvent::new(entity, axis, stick(v)).into());
                }
            }
            slots.last[n] = Some(now);
        }
    }

    /// Bevy's rumble requests on the XInput pads (gilrs, which would play
    /// them, is left out).
    fn rumble(mut slots: ResMut<Slots>, time: Res<Time<Real>>, mut requests: MessageReader<GamepadRumbleRequest>) {
        for r in requests.read() {
            let (gamepad, add) = match r {
                GamepadRumbleRequest::Add { gamepad, duration, intensity } => (*gamepad, Some((intensity.strong_motor, intensity.weak_motor, *duration))),
                GamepadRumbleRequest::Stop { gamepad } => (*gamepad, None),
            };
            let Some(n) = slots.pads.iter().position(|p| *p == Some(gamepad)) else { continue };
            match add {
                Some(r) => slots.rumbles[n].push(r),
                None => slots.rumbles[n].clear(),
            }
        }
        let dt = time.delta();
        for n in 0..SLOTS {
            if slots.last[n].is_none() || (slots.rumbles[n].is_empty() && !slots.buzzing[n]) {
                continue;
            }
            for r in &mut slots.rumbles[n] {
                r.2 = r.2.saturating_sub(dt);
            }
            slots.rumbles[n].retain(|r| !r.2.is_zero());
            let (strong, weak) = slots.rumbles[n].iter().fold((0.0f32, 0.0f32), |(s, w), r| (s + r.0, w + r.1));
            let motor = |v: f32| (v.clamp(0.0, 1.0) * 65535.0) as u16;
            let mut v = XInputVibration { left_motor: motor(strong), right_motor: motor(weak) };
            // SAFETY: a plain call with a valid pointer.
            unsafe { XInputSetState(n as u32, &mut v) };
            slots.buzzing[n] = !slots.rumbles[n].is_empty();
        }
    }
}
