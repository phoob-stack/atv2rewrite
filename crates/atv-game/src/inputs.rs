//! Each player's controls as data: one record per player per physics
//! tick, read from the keyboard and pads before the tick runs. Driving,
//! boosting, tricks, kicks and respots work from these records rather
//! than from the keys and pads, so a record can just as well come from
//! somewhere else (another player's game over the network, a replay).
//!
//! The keys and pads are read every frame and the physics ticks at its
//! own rate, so a press (Back On Track, the quick restart) is kept until
//! a tick takes it; a held button is as it was on the latest frame.

use bevy::prelude::*;

use atv_sim::Input;

use crate::controller::{self, Action, PORTS};
use crate::freestyle::Dir;
use crate::settings::Settings;
use crate::{AppState, Race, Selection};

/// One player's controls for one tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerInput {
    pub throttle: f32,
    /// -1 left .. 1 right.
    pub steer: f32,
    /// -1 back .. 1 forward.
    pub lean: f32,
    /// Brake, pre-load and boost on the ground ([0]) and in the air
    /// ([1]): pad buttons that are also trick buttons don't do these in
    /// the air.
    pub brake: [f32; 2],
    pub hop: [bool; 2],
    pub boost: [bool; 2],
    /// The attack button, held (it only kicks on the ground).
    pub kick: bool,
    /// Held trick buttons, a bit each in [`Action::TRICKS`] order, and
    /// the stick's direction for them.
    pub tricks: u8,
    pub dir: Option<Dir>,
    /// Back On Track was pressed.
    pub respot: bool,
}

impl PlayerInput {
    /// Reads the controls (see [`controller::Reader`]). `slot`: in split
    /// screen, the player (0-based), with their own keys and pad; alone,
    /// every pad and player 1's keys drive.
    pub fn read(ctl: &controller::Reader, slot: Option<usize>) -> Self {
        let level = |a: Action, air: bool| ctl.level(slot, a, air);
        let both = |a: Action| [ctl.pressed(slot, a, false), ctl.pressed(slot, a, true)];
        let mut tricks = 0u8;
        for (i, &a) in Action::TRICKS.iter().enumerate() {
            if ctl.pressed(slot, a, true) {
                tricks |= 1 << i;
            }
        }
        // The keyboard's accelerate and brake keys push the stick up and
        // down for tricks too.
        let x = level(Action::SteerRight, true) - level(Action::SteerLeft, true);
        let y = level(Action::LeanForward, true).max(ctl.key_level(slot, Action::Accelerate))
            - level(Action::LeanBack, true).max(ctl.key_level(slot, Action::Brake));
        let dir = if x.abs().max(y.abs()) < 0.5 {
            None
        } else if x.abs() > y.abs() {
            Some(if x > 0.0 { Dir::Right } else { Dir::Left })
        } else {
            Some(if y > 0.0 { Dir::Up } else { Dir::Down })
        };
        Self {
            throttle: level(Action::Accelerate, false),
            steer: (level(Action::SteerRight, false) - level(Action::SteerLeft, false)).clamp(-1.0, 1.0),
            lean: (level(Action::LeanForward, false) - level(Action::LeanBack, false)).clamp(-1.0, 1.0),
            brake: [level(Action::Brake, false), level(Action::Brake, true)],
            hop: both(Action::Hop),
            boost: both(Action::Boost),
            kick: ctl.pressed(slot, Action::Kick, false),
            tricks,
            dir,
            respot: ctl.just_pressed(slot, Action::Respot),
        }
    }

    /// The driving controls on the ground or in the air.
    pub fn drive(&self, airborne: bool) -> Input {
        let air = airborne as usize;
        Input { throttle: self.throttle, brake: self.brake[air], steer: self.steer, lean: self.lean, jump: self.hop[air] }
    }

    /// The speed boost button.
    pub fn boost(&self, airborne: bool) -> bool {
        self.boost[airborne as usize]
    }

    /// Held trick buttons and the stick direction, for
    /// [`crate::freestyle::Session::step`].
    pub fn tricks(&self) -> (u8, Option<Dir>) {
        (self.tricks, self.dir)
    }

    /// A newer read on top of one a tick hasn't taken yet: held controls
    /// as they are now, presses kept.
    fn merge(&mut self, now: Self) {
        let respot = self.respot || now.respot;
        *self = Self { respot, ..now };
    }
}

/// The controls for this physics tick, player 1 first.
#[derive(Resource, Default)]
pub struct TickInputs {
    pub players: [PlayerInput; PORTS],
    /// The quick restart was pressed.
    pub restart: bool,
    /// Read since the last tick, waiting for the next.
    pending: [PlayerInput; PORTS],
    pending_restart: bool,
}

impl TickInputs {
    /// Player `n`'s (0-based) controls.
    pub fn player(&self, n: usize) -> &PlayerInput {
        &self.players[n.min(PORTS - 1)]
    }
}

pub struct InputsPlugin;

impl Plugin for InputsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TickInputs>()
            .add_systems(PreUpdate, read.after(controller::track_ports).run_if(in_state(AppState::Race)))
            .add_systems(FixedUpdate, take.before(crate::drive).run_if(in_state(AppState::Race)))
            .add_systems(OnExit(AppState::Race), |mut inputs: ResMut<TickInputs>| *inputs = TickInputs::default());
    }
}

/// Reads every player's controls for the next tick.
fn read(
    keys: Res<ButtonInput<KeyCode>>,
    pads: controller::Pads,
    settings: Res<Settings>,
    (race, sel): (Option<Res<Race>>, Res<Selection>),
    virt: Res<Time<Virtual>>,
    mut inputs: ResMut<TickInputs>,
) {
    let ctl = controller::Reader::new(&keys, &pads, &settings.pads);
    let players = race.map_or(sel.players, |r| r.players).clamp(1, PORTS);
    // Presses on a pause page are for its menu.
    let paused = virt.is_paused();
    for n in 0..players {
        let mut now = PlayerInput::read(&ctl, (players > 1).then_some(n));
        now.respot &= !paused;
        inputs.pending[n].merge(now);
    }
    inputs.pending_restart |= ctl.just_pressed(None, Action::Restart) && !paused;
}

/// Hands the tick what was read for it.
fn take(mut inputs: ResMut<TickInputs>) {
    let inputs = &mut *inputs;
    inputs.players = inputs.pending;
    inputs.restart = std::mem::take(&mut inputs.pending_restart);
    for p in &mut inputs.pending {
        p.respot = false;
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn a_press_waits_for_the_next_tick() {
        let mut inputs = TickInputs::default();
        // Pressed on one frame, let go on the next, before a tick ran.
        inputs.pending[0].merge(PlayerInput { respot: true, throttle: 1.0, ..default() });
        inputs.pending[0].merge(PlayerInput::default());
        inputs.pending_restart = true;
        let mut world = World::new();
        world.insert_resource(inputs);
        world.run_system_once(take).unwrap();
        let inputs = world.resource::<TickInputs>();
        assert!(inputs.players[0].respot && inputs.restart);
        assert_eq!(inputs.players[0].throttle, 0.0);
        world.run_system_once(take).unwrap();
        let inputs = world.resource::<TickInputs>();
        assert!(!inputs.players[0].respot && !inputs.restart);
    }
}
