//! Kicks, as in the original: the attack button kicks out to the side of
//! the nearest rider (0x25bf0 picks the side, 0x27eb0 starts the kick). On
//! the kick clip's 12th frame (0x2dc50) a rider within 1 m of a point 1.2 m
//! out to that side and 0.3 m ahead is hit (0x2a200). Each hit takes some
//! of the rider's strength: all of it when they're weaker than the kicker
//! (`mStrength`), three quarters when equal, half when stronger. A hit
//! steals up to `AttackLoss` of the victim's speed boost and makes them
//! flinch; one that takes the last of their strength knocks them off and
//! steals all of it. Strength comes back over `mStrengthRestoreTime`
//! (Riders.xml). Computer riders with an `mAggression` of 3 or more kick
//! anyone within 2.2 m off to their side (0x630f0).

use std::sync::Arc;

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;

use atv_formats::setup;
use atv_formats::GameSource;
use atv_sim::Vehicle;

use crate::boost::Boost;
use crate::crash::{Cause, Way, Wreck};
use crate::opponents::Opponents;
use crate::race::RaceProgress;
use crate::rider::{Clip, ONE_SHOT_FPS};
use crate::{Args, Player, Race};

/// Where a kick lands: this far out to the side and ahead of the quad,
/// and how near it a rider has to be.
const SIDE: f32 = 1.2;
const AHEAD: f32 = 0.3;
const REACH: f32 = 1.0;
/// The kick clip's frame that lands.
const HIT_FRAME: f32 = 11.0;
/// Computer riders kick riders this near, at least 45 degrees off ahead.
const AI_RANGE: f32 = 2.2;
const AI_SIDE: f32 = 0.707;
/// And wait this long between kicks.
const AI_WAIT: f32 = 0.25;

#[derive(Resource, Clone, Copy, Debug)]
pub struct FightRules {
    /// Seconds for strength to come back in full.
    pub restore: f32,
    /// Boost a hit steals, at most.
    pub steal: f32,
    /// Scales what a computer rider steals.
    pub award_ai: f32,
}

impl FightRules {
    pub fn load(game: &GameSource) -> Self {
        let read = |p: &str| game.read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let restore = setup::root(&read("/Setup/Riders.xml")).and_then(|r| r.get("mStrengthRestoreTime")?.trim().parse().ok()).unwrap_or(2.0);
        let tricks = read("/Setup/Tricks.xml");
        let entries = setup::entries(&tricks);
        let boost = entries.iter().find(|e| e.name == "SpeedBoost");
        let num = |key: &str, default: f32| boost.and_then(|e| e.num(key)).unwrap_or(default);
        Self { restore: f32::max(restore, 0.1), steal: num("AttackLoss", 0.2), award_ai: num("AttackAwardAIMod", 1.0) }
    }
}

/// The kick (to the right, and mirrored) and the flinch from a hit (from
/// the right, and mirrored).
#[derive(Resource)]
pub struct FightClips {
    kick: [Arc<Clip>; 2],
    hit: [Arc<Clip>; 2],
}

impl FightClips {
    pub fn load(game: &GameSource, mirror_map: &[usize]) -> Option<Self> {
        let load = |name: &str| match Clip::load_from(game, &format!("/Animations/Generic/Bac/{name}.bac")) {
            Ok(c) => Some(c),
            Err(e) => {
                warn!("fight clip {name}: {e:#}");
                None
            }
        };
        let (kick, hit) = (load("Kick")?, load("Hit_from_right")?);
        let (kick_left, hit_left) = (kick.mirrored(mirror_map), hit.mirrored(mirror_map));
        Some(Self { kick: [Arc::new(kick), Arc::new(kick_left)], hit: [Arc::new(hit), Arc::new(hit_left)] })
    }

    fn seconds(clip: &Clip) -> f32 {
        (clip.frames().max(2) - 1) as f32 / ONE_SHOT_FPS
    }
}

/// One rider's side of the fighting: player 1's is a resource, every
/// other rider's is kept with their racer.
#[derive(Resource, Clone, Copy, Debug)]
pub struct Fighter {
    /// The rider's `mStrength` and `mAggression` (1-5).
    pub stat: f32,
    pub aggression: f32,
    /// Strength left, 0..1.
    pub health: f32,
    /// The kick under way: seconds in, and whether to the right.
    pub kick: Option<(f32, bool)>,
    /// Flinching from a hit: seconds in, and whether from the right.
    pub flinch: Option<(f32, bool)>,
    /// The attack button last step, to kick once per press.
    held: bool,
    /// A computer rider's wait before its next kick.
    wait: f32,
}

impl Fighter {
    pub fn new(stat: f32, aggression: f32) -> Self {
        Self { stat, aggression, health: 1.0, kick: None, flinch: None, held: false, wait: 0.0 }
    }

    /// Starts a kick unless one is under way or the rider is flinching.
    fn start(&mut self, right: bool) {
        if self.kick.is_none() && self.flinch.is_none() {
            self.kick = Some((0.0, right));
        }
    }

    /// The clip to show over the riding pose, and how far through it.
    pub fn pose(&self, clips: &FightClips) -> Option<(Arc<Clip>, f32)> {
        let show = |c: &Arc<Clip>, t: f32| Some((c.clone(), (t / FightClips::seconds(c)).min(1.0)));
        match (self.kick, self.flinch) {
            (Some((t, right)), _) => show(&clips.kick[usize::from(!right)], t),
            (_, Some((t, right))) => show(&clips.hit[usize::from(!right)], t),
            _ => None,
        }
    }
}

/// How much strength a hit takes: weaker riders go down in one.
pub fn damage(victim: f32, attacker: f32) -> f32 {
    if victim < attacker {
        1.0
    } else if victim > attacker {
        0.5
    } else {
        0.75
    }
}

/// Where a kick to the right (or left) lands.
pub fn strike_point(v: &Vehicle, right: bool) -> Vec3 {
    v.position + v.right() * if right { SIDE } else { -SIDE } + v.forward() * AHEAD
}

/// The attack button: LB on the pad, E on the keyboard, or for player 2
/// in split screen the number pad's 6. On the ground only: LB is a trick
/// button in the air.
pub fn read_attack(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>, slot: Option<usize>) -> bool {
    let key = match slot {
        None | Some(0) => keys.pressed(KeyCode::KeyE),
        Some(1) => keys.pressed(KeyCode::Numpad6),
        Some(_) => false,
    };
    key || pads.iter().enumerate().any(|(n, pad)| slot.is_none_or(|s| s == n) && pad.pressed(GamepadButton::LeftTrigger))
}

/// What one rider is doing this step, for the kicking.
struct Seat {
    position: Vec3,
    right: Vec3,
    /// Riding (not thrown off) with the clock running.
    live: bool,
    grounded: bool,
    /// The split-screen slot of a player, or None for a computer rider.
    human: Option<Option<usize>>,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn kicks(
    time: Res<Time>,
    args: Res<Args>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    (rules, clips): (Option<Res<FightRules>>, Option<Res<FightClips>>),
    (race, progress): (Option<Res<Race>>, Option<Res<RaceProgress>>),
    (player, fighter, boost, wreck): (Option<Res<Player>>, Option<ResMut<Fighter>>, Option<ResMut<Boost>>, Option<ResMut<Wreck>>),
    opponents: Option<ResMut<Opponents>>,
) {
    let (Some(rules), Some(clips), Some(race), Some(progress), Some(player), Some(mut fighter), Some(mut boost), Some(mut wreck)) =
        (rules, clips, race, progress, player, fighter, boost, wreck)
    else {
        return;
    };
    let dt = time.delta_secs();
    let mut opp = opponents;
    let split = race.players > 1;
    let racers = opp.as_mut().map_or(&mut [][..], |o| &mut o.racers[..]);

    // Everyone's state, player 1 first.
    let seat = |v: &Vehicle, live: bool, human: Option<Option<usize>>| Seat {
        position: v.position,
        right: v.right(),
        live,
        grounded: v.grounded(),
        human,
    };
    let p1_live = progress.started() && progress.finished.is_none() && wreck.fall.is_none();
    let mut seats = vec![seat(&player.0, p1_live, if args.autodrive { None } else { Some(split.then_some(0)) })];
    for r in racers.iter() {
        let live = r.progress.started() && r.progress.finished.is_none() && r.wreck.fall.is_none();
        seats.push(seat(&r.vehicle, live, r.human.filter(|_| !args.autodrive).map(|n| Some(n - 1))));
    }
    let mut fighters: Vec<Fighter> = std::iter::once(*fighter).chain(racers.iter().map(|r| r.fighter)).collect();
    let mut boosts: Vec<Boost> = std::iter::once(*boost).chain(racers.iter().map(|r| r.boost)).collect();

    // Who kicks: a press of the attack button, or a computer rider with
    // someone alongside; towards the side the nearest rider is on.
    let kick_len = FightClips::seconds(&clips.kick[0]);
    let hit_len = FightClips::seconds(&clips.hit[0]);
    let mut strikes = Vec::new();
    for (i, s) in seats.iter().enumerate() {
        let f = &mut fighters[i];
        f.health = (f.health + dt / rules.restore).min(1.0);
        f.wait = (f.wait - dt).max(0.0);
        let nearest = seats
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(j, o)| (j, o.position - s.position))
            .min_by(|a, b| a.1.length_squared().total_cmp(&b.1.length_squared()));
        let side = nearest.map(|(_, d)| d.dot(s.right));
        let want = match s.human {
            Some(slot) => {
                let held = read_attack(&keys, &pads, slot);
                let pressed = held && !f.held;
                f.held = held;
                pressed.then(|| side.is_none_or(|d| d > 0.0))
            }
            None => nearest
                .filter(|&(j, d)| f.aggression * 0.2 > 0.5 && f.wait == 0.0 && seats[j].live && d.length() < AI_RANGE && d.normalize_or_zero().dot(s.right).abs() > AI_SIDE)
                .map(|(_, d)| d.dot(s.right) > 0.0),
        };
        if let Some(right) = want.filter(|_| s.live && s.grounded) {
            f.start(right);
            if s.human.is_none() && f.kick.is_some_and(|k| k.0 == 0.0) {
                f.wait = kick_len + AI_WAIT;
            }
        }
        if let Some((t, right)) = &mut f.kick {
            let before = *t;
            *t += dt;
            let at = HIT_FRAME / ONE_SHOT_FPS;
            if before < at && *t >= at && s.live {
                strikes.push((i, *right));
            }
            if *t >= kick_len || !s.live {
                f.kick = None;
            }
        }
        if let Some((t, _)) = &mut f.flinch {
            *t += dt;
            if *t >= hit_len || !s.live {
                f.flinch = None;
            }
        }
    }

    // The kicks that land.
    let mut knocked: Vec<(usize, bool)> = Vec::new();
    for (a, right) in strikes {
        let v = match a {
            0 => &player.0,
            n => &racers[n - 1].vehicle,
        };
        let point = strike_point(v, right);
        for b in 0..seats.len() {
            if b == a || !seats[b].live || knocked.iter().any(|k| k.0 == b) || seats[b].position.distance(point) > REACH {
                continue;
            }
            let from_right = (seats[a].position - seats[b].position).dot(seats[b].right) > 0.0;
            let kicker = fighters[a].stat;
            let victim = &mut fighters[b];
            victim.health -= damage(victim.stat, kicker);
            let mut stolen = if victim.health <= 0.0 {
                victim.health = 0.0;
                victim.kick = None;
                knocked.push((b, from_right));
                boosts[b].meter
            } else {
                victim.kick = None;
                victim.flinch = Some((0.0, from_right));
                boosts[b].meter.min(rules.steal)
            };
            boosts[b].add(-stolen);
            if seats[a].human.is_none() {
                stolen *= rules.award_ai;
            }
            boosts[a].add(stolen);
            debug!("kick: rider {a} hit rider {b} (strength left {:.2}, stole {stolen:.2} boost)", fighters[b].health);
        }
    }

    // Back where they belong.
    *fighter = fighters[0];
    *boost = boosts[0];
    for (r, (f, b)) in racers.iter_mut().zip(fighters.into_iter().zip(boosts).skip(1)) {
        r.fighter = f;
        r.boost = b;
    }
    // Knocked off, away from the kicker.
    for (b, from_right) in knocked {
        let pick = (seats[b].position.x * 7.31 + seats[b].position.z * 3.17).sin().abs();
        match b {
            0 => wreck.start(&player.0, Cause::Impact, Way::Side(!from_right), pick),
            n => {
                let r = &mut racers[n - 1];
                r.wreck.start(&r.vehicle, Cause::Impact, Way::Side(!from_right), pick);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weaker_riders_go_down_in_one() {
        assert_eq!(damage(2.0, 4.0), 1.0);
        assert_eq!(damage(4.0, 4.0), 0.75);
        assert_eq!(damage(5.0, 1.0), 0.5);
    }

    #[test]
    fn one_kick_at_a_time_and_none_while_flinching() {
        let mut f = Fighter::new(3.0, 3.0);
        f.start(true);
        f.start(false);
        assert_eq!(f.kick, Some((0.0, true)));
        f.kick = None;
        f.flinch = Some((0.1, false));
        f.start(true);
        assert_eq!(f.kick, None);
    }
}
