//! What the player has opened, as the original's profile `Unlockables`
//! (saved at 0x12ed0): tracks, riders and ATVs, in every mode.
//!
//! - Tracks: those `Setup/Tracks.xml` marks `mAvailable="1"` (Dunes 1,
//!   Forest 1 and Swamp 1) are open from the start. The others open when
//!   an Arcade or Career round is raced on them (each round's status page
//!   sets the track's bit, 0x95f60 and 0x97050), and winning an Arcade
//!   round opens the next round's track at once with a "New Track"
//!   message (0x7e290). Single Race and Time Trial list every track but
//!   race only the open ones ("This track can be unlocked by reaching it
//!   within arcade or career mode", 0x52580), and Choose Your Tracks lists
//!   only the open ones. A track no Arcade round or championship visits
//!   (the Stadium) can't be reached that way, so here it stays open.
//! - Riders: `Setup/Riders.xml`'s `Unlocked` riders are open. The pros
//!   (`LockedHuman`) open with freestyle challenge golds, and the two
//!   secret riders (`LockedNeverAI`) are hidden on rider select until then
//!   (0x4ef10, 0x4f8f7).
//! - ATVs: `Setup/VehicleSetup.xml`'s `Unlocked` ATVs are open; the others
//!   come with career gold medals ("Earn %d more gold medals in career
//!   mode", 0x41b70). Computer riders never ride the `LockedBoth` ATC.
//! - Cheats: the original takes these as a new profile's name (table at
//!   0x1cef14, applied at 0x18f20): ALLOFEM (all of the below), ALLRIDERS,
//!   ALLBIKES, ALLTRACKS, ALLTRICKS (skill points up to the top level),
//!   ALLCHAMPS (every championship open) and ALLCHALS (every challenge
//!   open). Here `--cheat NAME` enters one; it stays in the save.

use bevy::prelude::*;

use crate::career::{Career, Rules};
use crate::challenge;
use crate::frontend::{QuadInfo, RiderInfo, Roster};
use crate::{AppState, Mode, Selection};

pub const RIDERS: u32 = 0x2;
pub const BIKES: u32 = 0x4;
pub const TRACKS: u32 = 0x8;
pub const TRICKS: u32 = 0x10;
pub const CHAMPS: u32 = 0x20;
pub const CHALS: u32 = 0x40;

/// The original's cheat names and what each opens (its `NOHUD` isn't
/// here).
pub const CHEATS: [(&str, u32); 7] = [
    ("ALLOFEM", RIDERS | BIKES | TRACKS | TRICKS | CHAMPS | CHALS),
    ("ALLRIDERS", RIDERS),
    ("ALLBIKES", BIKES),
    ("ALLTRACKS", TRACKS),
    ("ALLTRICKS", TRICKS),
    ("ALLCHAMPS", CHAMPS),
    ("ALLCHALS", CHALS),
];

pub const TRACK_LOCKED: &str = "THIS TRACK CAN BE UNLOCKED BY REACHING IT WITHIN ARCADE OR CAREER MODE";
pub const RIDER_LOCKED: &str = "EARN GOLD MEDALS IN FREESTYLE CHALLENGE TO UNLOCK PRO RIDERS";
pub const RIDER_TAKEN: &str = "THIS RIDER IS LOCKED BECAUSE ANOTHER PLAYER ALREADY CHOSE THEM";

pub struct UnlocksPlugin;

impl Plugin for UnlocksPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Race), reach_track);
    }
}

/// A cheat's bits, by name (any case).
pub fn cheat_bits(name: &str) -> Option<u32> {
    CHEATS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name.trim())).map(|c| c.1)
}

/// Enters a cheat, as naming a profile after it does in the original.
/// False for a name that isn't one.
pub fn apply_cheat(career: &mut Career, rules: &Rules, name: &str) -> bool {
    let Some(bits) = cheat_bits(name) else { return false };
    career.cheats |= bits;
    if bits & TRICKS != 0 {
        let top = rules.skill_levels.last().copied().unwrap_or(0);
        career.skill_points = career.skill_points.max(top);
    }
    if bits & CHAMPS != 0 {
        career.open = [true; 4];
    }
    true
}

/// Carries progress from before tracks were saved as reached: the
/// tracks of career finishes, the Arcade rounds up to the saved one and
/// the championship's rounds so far.
pub fn upgrade(career: &mut Career, roster: &Roster) {
    let mut reached: Vec<String> = career.best.keys().cloned().collect();
    if career.arcade > 0 {
        reached.extend(roster.arcade.iter().take(career.arcade + 1).cloned());
    }
    if let Some(r) = &career.run {
        reached.extend(r.schedule.iter().take(r.round + 1).map(|(t, _)| t.clone()));
    }
    career.reached.extend(reached);
}

pub fn track_open(career: &Career, roster: &Roster, rules: &Rules, track: &str) -> bool {
    let is = |t: &String| t.eq_ignore_ascii_case(track);
    career.cheats & TRACKS != 0
        || roster.open_tracks.iter().any(is)
        || career.reached.iter().any(is)
        || !(roster.arcade.iter().any(is) || rules.tracks.iter().flatten().any(is))
}

pub fn rider_open(career: &Career, defs: &challenge::Defs, rider: &RiderInfo) -> bool {
    !rider.locked
        || career.cheats & RIDERS != 0
        || (1..=challenge::COUNT).any(|n| {
            career.challenges.get(&n).and_then(|c| c.1) == Some(2)
                && defs.get(n).and_then(|d| d.reward.as_deref()).is_some_and(|r| challenge::rider_label(r) == rider.name)
        })
}

/// Whether rider select shows a rider at all: the secret riders only
/// once they're open.
pub fn rider_shown(career: &Career, defs: &challenge::Defs, rider: &RiderInfo) -> bool {
    rider.ai || rider_open(career, defs, rider)
}

/// Why an ATV is locked: the career gold medals it still needs (0 when
/// no medal count opens it), or `None` when it's open.
pub fn atv_lock(career: &Career, rules: &Rules, quad: &QuadInfo) -> Option<u32> {
    let medals = rules.unlockables.iter().find(|(_, a)| a.eq_ignore_ascii_case(&quad.name)).map(|u| u.0);
    if career.cheats & BIKES != 0 || (!quad.locked && medals.is_none()) {
        return None;
    }
    let golds = career.golds();
    match medals {
        Some(m) if m <= golds => None,
        Some(m) => Some(m - golds),
        None => Some(0),
    }
}

pub fn atv_locked_text(more: u32) -> String {
    if more == 0 {
        "THIS ATV IS LOCKED FOR SOME REASON, YOUR GUESS IS AS GOOD AS MINE".into()
    } else {
        format!("EARN {more} MORE GOLD MEDALS IN CAREER MODE")
    }
}

/// An Arcade or Career round opens its track.
fn reach_track(sel: Res<Selection>, mut career: ResMut<Career>) {
    if matches!(sel.mode, Mode::Arcade | Mode::Career) && !career.reached.iter().any(|t| t.eq_ignore_ascii_case(&sel.track)) {
        career.reached.insert(sel.track.clone());
        career.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Roster {
        let mut r = Roster::default();
        r.tracks = ["Dunes1", "Dunes2", "Forest1", "Stadium1"].map(String::from).to_vec();
        r.open_tracks = vec!["Dunes1".into(), "Forest1".into()];
        r.arcade = vec!["Dunes1".into(), "Forest1".into(), "Dunes2".into()];
        r
    }

    #[test]
    fn tracks_open_as_reached() {
        let (roster, rules) = (roster(), Rules::default());
        let mut c = Career::new();
        assert!(track_open(&c, &roster, &rules, "Dunes1"));
        assert!(!track_open(&c, &roster, &rules, "Dunes2"));
        // Nothing reaches the Stadium, so it isn't locked.
        assert!(track_open(&c, &roster, &rules, "Stadium1"));
        c.reached.insert("Dunes2".into());
        assert!(track_open(&c, &roster, &rules, "dunes2"));
        let c = Career::parse(&c.to_text());
        assert!(track_open(&c, &roster, &rules, "Dunes2"));
    }

    #[test]
    fn old_saves_keep_their_arcade_tracks() {
        let roster = roster();
        let mut c = Career { arcade: 2, ..Career::new() };
        upgrade(&mut c, &roster);
        assert!(track_open(&c, &roster, &Rules::default(), "Dunes2"));
    }

    #[test]
    fn atvs_open_with_golds_or_a_cheat() {
        let rules = Rules { unlockables: vec![(2, "Skyline".into())], ..Rules::default() };
        let quad = |name: &str, locked| QuadInfo { name: name.into(), stats: [0.0; 5], locked, ai: true };
        let mut c = Career::new();
        assert_eq!(atv_lock(&c, &rules, &quad("Bandit", false)), None);
        assert_eq!(atv_lock(&c, &rules, &quad("Skyline", true)), Some(2));
        assert_eq!(atv_lock(&c, &rules, &quad("Mystery", true)), Some(0));
        c.best.insert("Dunes1".into(), 1);
        c.best.insert("Forest1".into(), 1);
        assert_eq!(atv_lock(&c, &rules, &quad("Skyline", true)), None);
        assert!(apply_cheat(&mut c, &rules, "allbikes"));
        assert_eq!(atv_lock(&c, &rules, &quad("Mystery", true)), None);
        assert!(!apply_cheat(&mut c, &rules, "BUBBA"));
    }

    #[test]
    fn cheats_are_saved() {
        let rules = Rules::default();
        let mut c = Career::new();
        apply_cheat(&mut c, &rules, "ALLOFEM");
        assert_eq!(c.open, [true; 4]);
        assert_eq!(c.skill_points, *rules.skill_levels.last().unwrap());
        let back = Career::parse(&c.to_text());
        assert_eq!(back.cheats, CHEATS[0].1);
    }
}
