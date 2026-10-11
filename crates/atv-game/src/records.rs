//! Track records, kept as the original keeps them: each track's best lap,
//! longest jump, biggest air, best wheelie and best bicycle, with who set
//! it on which rider and quad. They start as `Tracks.xml` has them (its
//! `Fastest_Lap` to `Longest_Bicycle`: 6 minutes, 10 m, 10 m, 5 s and 2 s on
//! every track) and belong to the game rather than a profile, as the
//! original keeps them in its system save (`Track_Records`, 0x12840); here
//! they're in `records.cfg` beside the options.
//!
//! At the results the original checks every human's race (0x14660, from
//! the results page 0x3ef60): the best lap, except on the stadiums, and the
//! longest jump, biggest air, best wheelie and best bicycle of the race.
//! A record broken takes the player's profile name, or in split screen the
//! player's own (0x88820). Challenges and training aren't checked (0x8006e).
//!
//! A jump is measured as the original's stunt manager measures it (0x61a00,
//! 0x62b20): it takes off when no wheel touches, from where the quad is
//! then, and lands when a wheel touches again, its length the distance
//! across the ground between the two. Its height is the most the ground
//! was below the quad on the way, cast straight down up to 50 m (0x1b6c0;
//! no ground that near and the original doesn't count it). Both count once
//! all four wheels are down. A jump that ends in a crash or a respot isn't
//! counted here.
//!
//! Open Freestyle's medal targets are records too (`Freestyle.xml`
//! `OpenFSRecords`): for each time limit a Gold, Silver and Bronze score
//! with who set it. A score above one takes its place at the results,
//! pushing the lower ones down and the Bronze out (0x18090, 0x182d0, from
//! 0x7ff60), so the targets rise as they're beaten. That's Open Freestyle
//! and Freestyle Battle only; ATV Academy and the challenges keep theirs.

use std::collections::HashMap;
use std::path::PathBuf;

use atv_formats::setup;
use atv_formats::GameSource;
use atv_sim::{TrackCollision, Vehicle};
use bevy::prelude::*;

use crate::freestyle::{limit_tag, TIME_LIMITS};
use crate::frontend::{RiderInfo, Roster};
use crate::race::fmt_time;
use crate::AppState;

/// How far down the ground is looked for under a quad in the air, metres
/// (0x1b6c0).
const REACH: f32 = 50.0;
/// Further than this in one physics step, metres, and the quad was put
/// somewhere rather than driven there.
const TELEPORT: f32 = 5.0;
/// The track select's record panels turn to their next record every 2.5 s
/// (+0xe4), the second and third 0.1 and 0.2 s ahead of the first (+0xe8).
const PANEL_TURN: f32 = 2.5;
const PANEL_STAGGER: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Lap,
    LongestJump,
    HighestJump,
    Wheelie,
    Bicycle,
    /// Open Freestyle's medal scores for a time limit.
    Gold,
    Silver,
    Bronze,
}

impl Kind {
    /// The track records, in the original's order (the record's offset in
    /// the track, +0xec).
    pub const ALL: [Kind; 5] = [Kind::Lap, Kind::LongestJump, Kind::HighestJump, Kind::Wheelie, Kind::Bicycle];
    /// Open Freestyle's medal records, best first (the original's slots).
    pub const MEDALS: [Kind; 3] = [Kind::Gold, Kind::Silver, Kind::Bronze];

    fn index(self) -> usize {
        self as usize
    }

    /// Its element in `Tracks.xml`.
    fn tag(self) -> &'static str {
        match self {
            Kind::Lap => "Fastest_Lap",
            Kind::LongestJump => "Longest_Jump",
            Kind::HighestJump => "Highest_Jump",
            Kind::Wheelie => "Longest_Wheelie",
            Kind::Bicycle => "Longest_Bicycle",
            Kind::Gold => "GoldRecord",
            Kind::Silver => "SilverRecord",
            Kind::Bronze => "BronzeRecord",
        }
    }

    /// Its key in `records.cfg`.
    fn key(self) -> &'static str {
        match self {
            Kind::Lap => "lap",
            Kind::LongestJump => "jump",
            Kind::HighestJump => "air",
            Kind::Wheelie => "wheelie",
            Kind::Bicycle => "bicycle",
            Kind::Gold => "gold",
            Kind::Silver => "silver",
            Kind::Bronze => "bronze",
        }
    }

    /// The original's name for it (0x4cd60: strings 0xac, 0x10d, 0xcf,
    /// 0x10e and 0x10b; the record page's 199, 0x1c6 and 0x36, 0x4d8d0).
    pub fn label(self) -> &'static str {
        match self {
            Kind::Lap => "BEST LAP",
            Kind::LongestJump => "LONGEST JUMP",
            Kind::HighestJump => "BIGGEST AIR",
            Kind::Wheelie => "BEST WHEELIE",
            Kind::Bicycle => "BEST BICYCLE",
            Kind::Gold => "GOLD MEDAL SCORE",
            Kind::Silver => "SILVER MEDAL SCORE",
            Kind::Bronze => "BRONZE MEDAL SCORE",
        }
    }

    /// As the original shows it short (0x4cdf0): a time, metres or seconds.
    pub fn show(self, value: f32) -> String {
        match self {
            Kind::Lap => fmt_time(value),
            Kind::LongestJump | Kind::HighestJump => format!("{value:.1}M"),
            Kind::Wheelie | Kind::Bicycle => format!("{value:.1}S"),
            Kind::Gold | Kind::Silver | Kind::Bronze => format!("{value:.0}"),
        }
    }

    /// As the track select shows it, long (0x4cdf0): a time, "metres" or
    /// "Seconds" (strings 0x21f and 0x1c1).
    pub fn long(self, value: f32) -> String {
        match self {
            Kind::Lap => fmt_time(value),
            Kind::LongestJump | Kind::HighestJump => format!("{value:.1} METRES"),
            Kind::Wheelie | Kind::Bicycle => format!("{value:.1} SECONDS"),
            Kind::Gold | Kind::Silver | Kind::Bronze => format!("{value:.0} POINTS"),
        }
    }

    /// Whether `value` beats `record`: a lap by being quicker, the rest by
    /// being more.
    fn beats(self, value: f32, record: f32) -> bool {
        if self == Kind::Lap { value < record } else { value > record }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub value: f32,
    pub name: String,
    /// The rider's model folder (`Male10`), empty when not known.
    pub rider: String,
    pub quad: String,
}

/// What one human did in a race, for [`TrackRecords::check`].
#[derive(Debug, Clone, Default)]
pub struct Entry {
    /// 1 to 4.
    pub player: usize,
    pub name: String,
    pub rider: String,
    pub quad: String,
    pub best_lap: Option<f32>,
    pub longest_jump: f32,
    pub highest_jump: f32,
    pub wheelie: f32,
    pub bicycle: f32,
    /// The freestyle score.
    pub score: u32,
}

impl Entry {
    fn value(&self, kind: Kind) -> Option<f32> {
        match kind {
            Kind::Lap => self.best_lap,
            Kind::LongestJump => Some(self.longest_jump),
            Kind::HighestJump => Some(self.highest_jump),
            Kind::Wheelie => Some(self.wheelie),
            Kind::Bicycle => Some(self.bicycle),
            Kind::Gold | Kind::Silver | Kind::Bronze => Some(self.score as f32),
        }
    }
}

/// A record broken at the results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Broken {
    pub kind: Kind,
    pub value: f32,
    /// Who broke it, 1 to 4.
    pub player: usize,
}

#[derive(Resource, Debug, Default)]
pub struct TrackRecords {
    /// `Tracks.xml`'s, by track (lower case).
    defaults: HashMap<String, [Record; 5]>,
    /// The ones broken since, by track (lower case) and kind.
    set: HashMap<(String, Kind), Record>,
    /// Open Freestyle's medal records from `Freestyle.xml`, by time limit
    /// (an index into [`TIME_LIMITS`]), best first.
    medal_defaults: Vec<Option<[Record; 3]>>,
    /// The time limits whose medal records have changed since.
    medals: HashMap<usize, [Record; 3]>,
}

impl TrackRecords {
    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "atv2").map(|d| d.config_dir().join("records.cfg"))
    }

    pub fn load(game: &GameSource, roster: &Roster) -> Self {
        let read = |path: &str| game.read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let mut r = Self::with_defaults(&read("/Setup/Tracks.xml"), &roster.riders);
        r.medal_defaults = medal_defaults(&read("/Setup/Freestyle.xml"), &roster.riders);
        if let Some(text) = Self::path().and_then(|p| std::fs::read_to_string(p).ok()) {
            r.parse(&text);
        }
        r
    }

    /// Each track's records from `Tracks.xml`, their riders by model
    /// (`RiderType` names a `Riders.xml` entry).
    fn with_defaults(xml: &str, riders: &[RiderInfo]) -> Self {
        let mut defaults = HashMap::new();
        for track in setup::entries(xml) {
            if let Some(records) = read_records(&setup::children(xml, &track.name), &Kind::ALL, riders) {
                defaults.insert(track.name.to_ascii_lowercase(), records);
            }
        }
        Self { defaults, ..default() }
    }

    /// The track's record of this kind, if the track has records.
    pub fn get(&self, track: &str, kind: Kind) -> Option<&Record> {
        let track = track.to_ascii_lowercase();
        self.set.get(&(track.clone(), kind)).or_else(|| self.defaults.get(&track).and_then(|r| r.get(kind.index())))
    }

    /// Open Freestyle's Gold, Silver and Bronze records for the time limit
    /// `limit` (an index into [`TIME_LIMITS`]).
    pub fn medals(&self, limit: usize) -> Option<&[Record; 3]> {
        self.medals.get(&limit).or_else(|| self.medal_defaults.get(limit)?.as_ref())
    }

    /// The session's Bronze, Silver and Gold targets for the time limit
    /// `limit` (0xa0e60), as [`crate::freestyle::Session`] keeps them.
    pub fn medal_targets(&self, limit: usize) -> Option<[u32; 3]> {
        self.medals(limit).map(|m| [2, 1, 0].map(|i| m[i].value as u32))
    }

    /// Puts each Open Freestyle score that beats one of the time limit's
    /// medal records in its place, the lower records moving down and the
    /// Bronze dropping out, as the original does at the results (0x18090,
    /// 0x182d0): best score first, each needing more than the record. Gives
    /// each record taken with who has it now.
    pub fn check_medals(&mut self, limit: usize, entries: &[Entry]) -> Vec<Broken> {
        let Some(mut table) = self.medals(limit).cloned() else { return Vec::new() };
        let mut order: Vec<&Entry> = entries.iter().collect();
        order.sort_by_key(|e| (std::cmp::Reverse(e.score), e.player));
        let mut broken = Vec::new();
        for e in order {
            let value = e.score as f32;
            let Some(slot) = table.iter().position(|r| r.value < value) else { continue };
            table[slot..].rotate_right(1);
            table[slot] = Record { value, name: e.name.clone(), rider: e.rider.clone(), quad: e.quad.clone() };
            broken.push(Broken { kind: Kind::MEDALS[slot], value, player: e.player });
        }
        if !broken.is_empty() {
            self.medals.insert(limit, table);
        }
        broken.sort_by_key(|b| b.kind.index());
        broken
    }

    /// Checks the humans' races on `track` and keeps the records they
    /// broke, as the original does at the results (0x14660): player by
    /// player, so a later player beating an earlier one's new record takes
    /// it. No lap records on the stadiums, whose laps aren't raced. Gives
    /// each record broken with who has it now.
    pub fn check(&mut self, track: &str, entries: &[Entry]) -> Vec<Broken> {
        let mut broken: Vec<Broken> = Vec::new();
        let stadium = track.to_ascii_lowercase().starts_with("stadium");
        for e in entries {
            for kind in Kind::ALL {
                if kind == Kind::Lap && stadium {
                    continue;
                }
                let (Some(value), Some(record)) = (e.value(kind), self.get(track, kind)) else { continue };
                if !kind.beats(value, record.value) {
                    continue;
                }
                let record = Record { value, name: e.name.clone(), rider: e.rider.clone(), quad: e.quad.clone() };
                self.set.insert((track.to_ascii_lowercase(), kind), record);
                broken.retain(|b| b.kind != kind);
                broken.push(Broken { kind, value, player: e.player });
            }
        }
        broken.sort_by_key(|b| b.kind.index());
        broken
    }

    /// `<track> <kind> = <value> <rider> <quad> <name>` for each record
    /// broken, `-` for a rider or quad not known, then `freestyle <limit>
    /// <medal> = ...` for each time limit's medal records once they've
    /// changed (`<limit>` in minutes, or `unlimited`).
    fn to_text(&self) -> String {
        let mut set: Vec<_> = self.set.iter().collect();
        set.sort_by(|a, b| (&a.0 .0, a.0 .1.index()).cmp(&(&b.0 .0, b.0 .1.index())));
        let or_dash = |s: &str| if s.is_empty() { "-".to_string() } else { s.replace(' ', "_") };
        let line = |key: String, r: &Record| format!("{key} = {} {} {} {}\n", r.value, or_dash(&r.rider), or_dash(&r.quad), r.name);
        let mut text: String = set.into_iter().map(|((track, kind), r)| line(format!("{track} {}", kind.key()), r)).collect();
        let mut medals: Vec<_> = self.medals.iter().collect();
        medals.sort_by_key(|m| *m.0);
        for (&limit, table) in medals {
            for (kind, r) in Kind::MEDALS.iter().zip(table) {
                text += &line(format!("freestyle {} {}", limit_key(limit), kind.key()), r);
            }
        }
        text
    }

    fn parse(&mut self, text: &str) {
        let from_dash = |s: &str| if s == "-" { String::new() } else { s.to_string() };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let mut parts = value.trim().splitn(4, ' ');
            let (Some(v), Some(rider), Some(quad)) = (parts.next().and_then(|v| v.parse().ok()), parts.next(), parts.next()) else { continue };
            let record = Record { value: v, name: parts.next().unwrap_or("").to_string(), rider: from_dash(rider), quad: from_dash(quad) };
            let key: Vec<&str> = key.split_whitespace().collect();
            match key[..] {
                ["freestyle", limit, medal] => {
                    let Some(limit) = (0..TIME_LIMITS.len()).find(|&l| limit_key(l) == limit) else { continue };
                    let Some(slot) = Kind::MEDALS.iter().position(|k| k.key() == medal) else { continue };
                    let Some(mut table) = self.medals(limit).cloned() else { continue };
                    table[slot] = record;
                    self.medals.insert(limit, table);
                }
                [track, kind] => {
                    let Some(kind) = Kind::ALL.into_iter().find(|k| k.key() == kind) else { continue };
                    self.set.insert((track.to_ascii_lowercase(), kind), record);
                }
                _ => {}
            }
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, self.to_text())
        };
        if let Err(e) = write() {
            warn!("couldn't save the track records to {}: {e}", path.display());
        }
    }
}

/// The model folder of the rider a records file's `RiderType` names (a
/// `Riders.xml` entry), empty when the roster hasn't them.
fn rider_model(key: &str, riders: &[RiderInfo]) -> String {
    let name = key.replace('_', " ").to_uppercase();
    riders.iter().find(|r| r.name == name).map_or(String::new(), |r| r.model.clone())
}

/// The records of `kinds` among `children`, each an element with `Record`,
/// `Name`, `RiderType` and `VehicleType`; None unless all are there.
fn read_records<const N: usize>(children: &[setup::Entry], kinds: &[Kind; N], riders: &[RiderInfo]) -> Option<[Record; N]> {
    let record = |kind: &Kind| {
        let e = children.iter().find(|c| c.name == kind.tag())?;
        Some(Record {
            value: e.num("Record")?,
            name: e.get("Name").unwrap_or("").to_string(),
            rider: rider_model(e.get("RiderType").unwrap_or(""), riders),
            quad: e.get("VehicleType").unwrap_or("").to_string(),
        })
    };
    kinds.iter().map(record).collect::<Option<Vec<_>>>()?.try_into().ok()
}

/// Each time limit's medal records from `Freestyle.xml` (`OpenFSRecords`,
/// `Time1Minute` to `TimeUnlimited`), best first.
fn medal_defaults(xml: &str, riders: &[RiderInfo]) -> Vec<Option<[Record; 3]>> {
    TIME_LIMITS.iter().map(|&l| read_records(&setup::children(xml, &limit_tag(l)), &Kind::MEDALS, riders)).collect()
}

/// A time limit's key in `records.cfg`: its minutes, or `unlimited`.
fn limit_key(limit: usize) -> String {
    match TIME_LIMITS.get(limit).copied().flatten() {
        Some(m) => m.to_string(),
        None => "unlimited".into(),
    }
}

/// The record the track select's panel `panel` (0 to 2) shows `secs` after
/// the screen came up (0x519d0, 0x4e760): each has all five in turn, the
/// first from the best lap, the second from the longest jump and the third
/// from the biggest air.
pub fn panel_kind(panel: usize, secs: f32) -> Kind {
    let turns = ((secs + panel as f32 * PANEL_STAGGER) / PANEL_TURN) as usize;
    Kind::ALL[(panel + turns) % Kind::ALL.len()]
}

/// One rider's jumps in a race (the stunt manager's +0xd8 to +0xf8).
#[derive(Debug, Clone, Copy, Default)]
pub struct Jumps {
    flying: bool,
    from: Vec2,
    /// The last jump's length and height.
    length: f32,
    peak: f32,
    last: Option<Vec3>,
    pub longest: f32,
    pub highest: f32,
}

impl Jumps {
    /// One physics step of `v`; `counting` false while the rider is off the
    /// quad or the race isn't on.
    pub fn step(&mut self, v: &Vehicle, collision: &TrackCollision, counting: bool) {
        let down = v.wheels.iter().filter(|w| w.contact).count();
        let height = if down == 0 { collision.raycast(v.position, Vec3::NEG_Y, REACH).map(|h| h.t) } else { None };
        self.advance(v.position, down, v.wheels.len(), height, counting);
    }

    /// `down` of `wheels` touching; `height`, the ground's distance below
    /// the quad when there's ground within reach.
    fn advance(&mut self, at: Vec3, down: usize, wheels: usize, height: Option<f32>, counting: bool) {
        let teleported = self.last.is_some_and(|l| (at - l).length() > TELEPORT);
        self.last = Some(at);
        if !counting || teleported {
            *self = Jumps { longest: self.longest, highest: self.highest, last: self.last, ..default() };
            return;
        }
        if !self.flying && down == 0 {
            self.flying = true;
            self.from = at.xz();
            self.peak = 0.0;
        } else if self.flying && down > 0 {
            self.flying = false;
            self.length = (at.xz() - self.from).length();
        }
        if self.flying
            && let Some(h) = height
        {
            self.peak = self.peak.max(h);
        }
        if wheels > 0 && down == wheels {
            self.longest = self.longest.max(self.length);
            self.highest = self.highest.max(self.peak);
        }
    }
}

/// Every human's jumps this race: player 1 first, then split-screen players
/// 2 to 4.
#[derive(Resource, Debug, Default)]
pub struct Feats(pub [Jumps; 4]);

pub struct RecordsPlugin;

impl Plugin for RecordsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Feats>()
            .add_systems(Startup, |mut commands: Commands, game: Res<crate::Game>, roster: Res<Roster>| {
                commands.insert_resource(TrackRecords::load(&game.0, &roster));
            })
            .add_systems(OnEnter(AppState::Race), |mut feats: ResMut<Feats>| *feats = Feats::default());
    }
}

/// Measures the humans' jumps, after each physics step.
pub fn watch(
    race: Option<Res<crate::Race>>,
    player: Option<Res<crate::Player>>,
    progress: Option<Res<crate::race::RaceProgress>>,
    (session, wreck): (Option<Res<crate::freestyle::Session>>, Option<Res<crate::crash::Wreck>>),
    opponents: Option<Res<crate::opponents::Opponents>>,
    demo: Option<Res<crate::demo::Demo>>,
    mut feats: ResMut<Feats>,
) {
    let (Some(race), Some(player), Some(progress)) = (race, player, progress) else { return };
    // The attract demo's computer rider breaks no records.
    if demo.is_some() {
        return;
    }
    // Racing, or in freestyle until the time is up.
    let on = |p: &crate::race::RaceProgress, s: Option<&crate::freestyle::Session>| p.racing() && !s.is_some_and(|s| s.freestyle && s.over(p.clock));
    let crashed = wreck.is_some_and(|w| w.fall.is_some());
    feats.0[0].step(&player.0, &race.collision, on(&progress, session.as_deref()) && !crashed);
    for r in opponents.iter().flat_map(|o| &o.racers) {
        if let Some(j) = r.human.and_then(|n| feats.0.get_mut(n.wrapping_sub(1))) {
            j.step(&r.vehicle, &race.collision, on(&r.progress, r.session.as_ref()) && r.wreck.fall.is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<Root FileVersion="1.0000000">
   <Dunes1 mAvailable="1" mLapRecord="150">
      <mAmbientColour A="1.0" R="0.4" G="0.4" B="0.5"/>
      <Platforms>
         <PC mGlobalTint="1.0" mMaxUniqueBikes="100"/>
      </Platforms>
      <Fastest_Lap Record="360" Name="Doug Gust" RiderType="Doug_Gust" VehicleType="Bandit"/>
      <Longest_Jump Record="10" Name="Dana Creech" RiderType="Dana_Creech" VehicleType="Blaze"/>
      <Highest_Jump Record="10" Name="Tim Farr" RiderType="Tim_Farr" VehicleType="Bullet"/>
      <Longest_Wheelie Record="5" Name="Shane Hitt" RiderType="Shane_Hitt" VehicleType="Challenger"/>
      <Longest_Bicycle Record="2" Name="Kory Ellis" RiderType="Kory_Ellis" VehicleType="Storm"/>
   </Dunes1>
   <Stadium1 mAvailable="1">
      <Fastest_Lap Record="360" Name="Doug Gust" RiderType="Doug_Gust" VehicleType="Bandit"/>
      <Longest_Jump Record="10" Name="Dana Creech" RiderType="Dana_Creech" VehicleType="Blaze"/>
      <Highest_Jump Record="10" Name="Tim Farr" RiderType="Tim_Farr" VehicleType="Bullet"/>
      <Longest_Wheelie Record="5" Name="Shane Hitt" RiderType="Shane_Hitt" VehicleType="Challenger"/>
      <Longest_Bicycle Record="2" Name="Kory Ellis" RiderType="Kory_Ellis" VehicleType="Storm"/>
   </Stadium1>
   <Moon1 mAvailable="1"/>
</Root>"#;

    const FREESTYLE_XML: &str = r#"<Root FileVersion="1.0000000" mNumUnlockables="12">
   <Unlockable mCondition="0" mRider="1" mATV="0" mData="Jason_Luburgh"/>
   <OpenFSRecords>
      <Time1Minute>
         <BronzeRecord Record="100" Name="Doug Gust" RiderType="Doug_Gust" VehicleType="Bandit"/>
         <SilverRecord Record="200" Name="Jason Luburgh" RiderType="Jason_Luburgh" VehicleType="Bandit"/>
         <GoldRecord Record="300" Name="Dana Creech" RiderType="Dana_Creech" VehicleType="Blaze"/>
      </Time1Minute>
   </OpenFSRecords>
</Root>"#;

    fn riders() -> Vec<RiderInfo> {
        let rider = |name: &str, model: &str| RiderInfo {
            name: name.into(),
            model: model.into(),
            born: String::new(),
            from: String::new(),
            stats: [3.0; 4],
            ai: true,
            locked: false,
            skill: 1,
        };
        vec![rider("DOUG GUST", "Male10"), rider("DANA CREECH", "Female2")]
    }

    fn records() -> TrackRecords {
        let mut r = TrackRecords::with_defaults(XML, &riders());
        r.medal_defaults = medal_defaults(FREESTYLE_XML, &riders());
        r
    }

    fn scored(player: usize, score: u32) -> Entry {
        Entry { score, ..entry(player, None, 0.0) }
    }

    /// Each medal record's score and holder, best first.
    fn board(r: &TrackRecords) -> Vec<(f32, String)> {
        r.medals(0).unwrap().iter().map(|m| (m.value, m.name.clone())).collect()
    }

    fn entry(player: usize, lap: Option<f32>, jump: f32) -> Entry {
        Entry { player, name: format!("P{player}"), rider: "Male1".into(), quad: "Bandit".into(), best_lap: lap, longest_jump: jump, ..default() }
    }

    #[test]
    fn defaults_come_from_the_tracks_file() {
        let r = records();
        let lap = r.get("DUNES1", Kind::Lap).unwrap();
        assert_eq!((lap.value, lap.name.as_str(), lap.rider.as_str(), lap.quad.as_str()), (360.0, "Doug Gust", "Male10", "Bandit"));
        assert_eq!(r.get("Dunes1", Kind::Bicycle).unwrap().value, 2.0);
        // A rider the roster doesn't have.
        assert_eq!(r.get("Dunes1", Kind::HighestJump).unwrap().rider, "");
        assert!(r.get("Moon1", Kind::Lap).is_none());
    }

    #[test]
    fn quicker_laps_and_longer_jumps_break_records() {
        let mut r = records();
        let broken = r.check("Dunes1", &[entry(1, Some(95.5), 9.0)]);
        assert_eq!(broken, [Broken { kind: Kind::Lap, value: 95.5, player: 1 }]);
        assert_eq!(r.get("Dunes1", Kind::Lap).unwrap().name, "P1");
        // Slower than the new record: nothing.
        assert!(r.check("Dunes1", &[entry(1, Some(96.0), 0.0)]).is_empty());
        // Split screen: player 2 beats player 1's new jump record.
        let broken = r.check("Dunes1", &[entry(1, None, 20.0), entry(2, None, 25.0)]);
        assert_eq!(broken, [Broken { kind: Kind::LongestJump, value: 25.0, player: 2 }]);
        assert_eq!(r.get("Dunes1", Kind::LongestJump).unwrap().name, "P2");
    }

    #[test]
    fn medal_records_come_from_the_freestyle_file() {
        let r = records();
        let gold = &r.medals(0).unwrap()[0];
        assert_eq!((gold.value, gold.name.as_str(), gold.rider.as_str(), gold.quad.as_str()), (300.0, "Dana Creech", "Female2", "Blaze"));
        assert_eq!(r.medal_targets(0), Some([100, 200, 300]));
        // No records for three minutes in this file.
        assert!(r.medals(1).is_none());
    }

    #[test]
    fn a_score_above_a_medal_record_takes_its_place() {
        let mut r = records();
        // Above Silver: Silver moves down to Bronze, Bronze drops out.
        assert_eq!(r.check_medals(0, &[scored(1, 250)]), [Broken { kind: Kind::Silver, value: 250.0, player: 1 }]);
        assert_eq!(board(&r), [(300.0, "Dana Creech".into()), (250.0, "P1".into()), (200.0, "Jason Luburgh".into())]);
        assert_eq!(r.medal_targets(0), Some([200, 250, 300]));
        // Level with Gold isn't above it.
        assert_eq!(r.check_medals(0, &[scored(1, 300)])[0].kind, Kind::Silver);
        // Below Bronze: nothing.
        assert!(r.check_medals(0, &[scored(1, 50)]).is_empty());
        // Freestyle Battle: the best score goes in first.
        let broken = r.check_medals(0, &[scored(1, 350), scored(2, 400)]);
        assert_eq!(broken, [Broken { kind: Kind::Gold, value: 400.0, player: 2 }, Broken { kind: Kind::Silver, value: 350.0, player: 1 }]);
        assert_eq!(board(&r), [(400.0, "P2".into()), (350.0, "P1".into()), (300.0, "Dana Creech".into())]);
    }

    #[test]
    fn medal_records_save_and_load() {
        let mut r = records();
        r.check_medals(0, &[scored(1, 1250)]);
        let text = r.to_text();
        assert_eq!(text, "freestyle 1 gold = 1250 Male1 Bandit P1\nfreestyle 1 silver = 300 Female2 Blaze Dana Creech\nfreestyle 1 bronze = 200 - Bandit Jason Luburgh\n");
        let mut back = records();
        back.parse(&text);
        assert_eq!(back.medals(0), r.medals(0));
        assert!(back.medals(1).is_none());
    }

    #[test]
    fn no_lap_records_on_the_stadiums() {
        let mut r = records();
        assert!(r.check("Stadium1", &[entry(1, Some(30.0), 0.0)]).is_empty());
    }

    #[test]
    fn records_broken_save_and_load() {
        let mut r = records();
        let mut e = entry(1, Some(81.25), 33.5);
        e.name = "SAM O NEIL".into();
        e.rider = String::new();
        r.check("Dunes1", &[e]);
        let text = r.to_text();
        assert_eq!(text, "dunes1 lap = 81.25 - Bandit SAM O NEIL\ndunes1 jump = 33.5 - Bandit SAM O NEIL\n");
        let mut back = records();
        back.parse(&text);
        assert_eq!(back.get("Dunes1", Kind::LongestJump), r.get("Dunes1", Kind::LongestJump));
        assert_eq!(back.get("Dunes1", Kind::Lap).unwrap().value, 81.25);
        // The rest stay as the tracks file has them.
        assert_eq!(back.get("Dunes1", Kind::Wheelie).unwrap().value, 5.0);
    }

    #[test]
    fn panels_turn_over_a_little_apart() {
        assert_eq!([0, 1, 2].map(|k| panel_kind(k, 0.0)), [Kind::Lap, Kind::LongestJump, Kind::HighestJump]);
        // The third turns first.
        assert_eq!([0, 1, 2].map(|k| panel_kind(k, 2.35)), [Kind::Lap, Kind::LongestJump, Kind::Wheelie]);
        assert_eq!([0, 1, 2].map(|k| panel_kind(k, 2.5)), [Kind::LongestJump, Kind::HighestJump, Kind::Wheelie]);
        // Round to the start after five turns.
        assert_eq!(panel_kind(0, 12.6), Kind::Lap);
    }

    #[test]
    fn jumps_count_once_all_four_wheels_are_down() {
        let mut j = Jumps::default();
        let at = |x: f32| Vec3::new(x, 0.0, 0.0);
        j.advance(at(0.0), 4, 4, None, true);
        // Off at x 1, the ground 3 m below at the top.
        j.advance(at(1.0), 0, 4, Some(1.0), true);
        j.advance(at(3.0), 0, 4, Some(3.0), true);
        j.advance(at(4.5), 0, 4, None, true);
        // Down on two wheels at x 5: measured, not counted yet.
        j.advance(at(5.0), 2, 4, Some(0.5), true);
        assert_eq!((j.longest, j.highest), (0.0, 0.0));
        j.advance(at(5.2), 4, 4, Some(0.5), true);
        assert_eq!((j.longest, j.highest), (4.0, 3.0));
    }

    #[test]
    fn crashes_and_respots_drop_the_jump() {
        let mut j = Jumps::default();
        let at = |x: f32| Vec3::new(x, 0.0, 0.0);
        j.advance(at(0.0), 0, 4, Some(2.0), true);
        j.advance(at(4.0), 0, 4, Some(2.0), true);
        j.advance(at(4.2), 0, 4, Some(2.0), false);
        j.advance(at(4.4), 4, 4, Some(0.5), true);
        assert_eq!((j.longest, j.highest), (0.0, 0.0));
        // Put back further on, landing on all four.
        j.advance(at(0.0), 0, 4, Some(2.0), true);
        j.advance(at(4.0), 0, 4, Some(2.0), true);
        j.advance(at(40.0), 4, 4, Some(0.5), true);
        assert_eq!((j.longest, j.highest), (0.0, 0.0));
    }
}
