//! Every quad in a race as one list. Each racer has an id fixed for the
//! race (player 1 is 0, everyone else 1, 2, ... in grid order), says who
//! drives it ([`Control`]), and can be read as data ([`RacerState`]). A
//! racer someone else's game runs is a [`Control::Puppet`]: it isn't
//! driven here, only shown where it's told.
//!
//! Player 1's quad is kept in its own resources ([`crate::Player`],
//! [`Boost`], [`Fighter`], ...) and everyone else's in
//! [`Opponents::racers`]; [`AllRacers`] reads them alike, by id.

use std::collections::VecDeque;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use atv_sim::QuadPose;

use crate::attack::Fighter;
use crate::boost::Boost;
use crate::freestyle::Session;
use crate::opponents::Opponents;
use crate::{AppState, Controls, Player};

/// A racer's number for the race. Every quad's body carries its racer's.
#[derive(Component, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct RacerId(pub u8);

impl RacerId {
    pub const PLAYER_1: Self = Self(0);
}

/// Who drives a racer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Control {
    /// A split-screen player (1 to 4), on this machine's keys and pads.
    Player(usize),
    /// A computer rider.
    Computer,
    /// Another game drives it; it's shown where that game says.
    Puppet,
}

impl Control {
    /// The split-screen player driving.
    pub fn human(self) -> Option<usize> {
        match self {
            Control::Player(n) => Some(n),
            _ => None,
        }
    }
}

/// What another game needs to show a racer: the quad (see
/// [`QuadPose`]), the rider's lean, boost, kick or flinch, and the trick
/// being pulled.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RacerState {
    pub quad: QuadPose,
    /// The rider's lean, from the stick (-1 back .. 1 forward).
    pub lean: f32,
    pub boost: Boost,
    /// A kick, or flinching from one: seconds in, and whether to (or
    /// from) the right.
    pub kick: Option<(f32, bool)>,
    pub flinch: Option<(f32, bool)>,
    /// Which of the rider's tricks, and seconds into it.
    pub trick: Option<(usize, f32)>,
}

/// Every racer in the race, read alike.
#[derive(SystemParam)]
pub struct AllRacers<'w> {
    player: Option<Res<'w, Player>>,
    controls: Option<Res<'w, Controls>>,
    boost: Option<Res<'w, Boost>>,
    fighter: Option<Res<'w, Fighter>>,
    session: Option<Res<'w, Session>>,
    opponents: Option<Res<'w, Opponents>>,
}

impl AllRacers<'_> {
    /// Racer `id` as data.
    pub fn state(&self, id: RacerId) -> Option<RacerState> {
        if id != RacerId::PLAYER_1 {
            return self.opponents.as_ref()?.racers.iter().find(|r| r.id == id).map(|r| r.state());
        }
        let fighter = self.fighter.as_deref();
        Some(RacerState {
            quad: self.player.as_ref()?.0.pose(),
            lean: self.controls.as_ref().map_or(0.0, |c| c.0.lean),
            boost: self.boost.as_deref().copied().unwrap_or_default(),
            kick: fighter.and_then(|f| f.kick),
            flinch: fighter.and_then(|f| f.flinch),
            trick: self.session.as_ref().and_then(|s| s.active.as_ref()).map(|a| (a.def, a.t)),
        })
    }
}

pub struct RacersPlugin;

impl Plugin for RacersPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (remember, tell_puppets).chain().after(crate::drive).before(crate::opponents::drive_opponents).run_if(resource_exists::<Echo>).run_if(in_state(AppState::Race)),
        )
        .add_systems(OnExit(AppState::Race), |mut commands: Commands| commands.remove_resource::<Echo>());
    }
}

/// `--puppet-test`: the computer riders are puppets that repeat what
/// player 1 did, each a second later than the one before, to show what
/// another game's quads look like here.
#[derive(Resource, Default)]
pub struct Echo {
    /// Player 1 as each step left them, with the time.
    past: VecDeque<(f32, RacerState)>,
}

/// Keeps player 1 as each step left them.
fn remember(time: Res<Time>, all: AllRacers, mut echo: ResMut<Echo>) {
    if let Some(state) = all.state(RacerId::PLAYER_1) {
        echo.past.push_back((time.elapsed_secs(), state));
    }
}

/// Shows each puppet player 1 as they were its id in seconds ago.
fn tell_puppets(time: Res<Time>, opponents: Option<ResMut<Opponents>>, mut echo: ResMut<Echo>) {
    let Some(mut opp) = opponents else { return };
    let now = time.elapsed_secs();
    let span = time.delta_secs();
    let mut oldest = now;
    for r in opp.racers.iter_mut().filter(|r| r.control == Control::Puppet) {
        let when = now - r.id.0 as f32;
        // The last step at or before then.
        if let Some((t, s)) = echo.past.iter().take_while(|(t, _)| *t <= when).last() {
            r.tell(s, span);
            oldest = oldest.min(*t);
        } else {
            oldest = f32::MIN;
        }
    }
    while echo.past.front().is_some_and(|(t, _)| *t < oldest) {
        echo.past.pop_front();
    }
}
