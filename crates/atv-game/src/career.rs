//! Career: the Amateur, Qualifying (Open) and Professional championships
//! from `Setup/Career.xml`, five rounds each against seven computer riders.
//!
//! The rules follow the original's career code:
//! - Each race ranks the eight riders twice, by finishing order and by
//!   trick score, and gives championship points for each from the
//!   `PositionPoints` and `TrickPoints` tables.
//! - The player's trick score is added to their skill points twice (once
//!   only, capped at skill level `mMaxSkillForAmateur`, in Amateur). The
//!   skill level is the highest `SkillLevelPoints` reached, and the
//!   certificate the highest `CertificateSkillLevels` the level reaches.
//! - Finishing a championship in the top three opens the next one.
//! - Winning a career round on a track for the first time earns a gold
//!   medal; each `Unlockable` ATV comes free at its count of golds.
//! - Once the winner is home, a rider who isn't in by `mTimePercentage`
//!   of the winner's time doesn't finish, and places last.
//! - The field is drawn once per championship from the riders
//!   `RiderAvailability` allows in it.
//! - A top-3 Professional finish opens the Custom championship (the
//!   original's `mCustomUnlocked`). Its rounds are the tracks the player
//!   adds on Choose Your Tracks, each with its own laps (1 to 20; a new
//!   track takes the last laps set, 1 at first), and Point Distribution
//!   shares each place's race and trick points (together 50 for a win) out
//!   between the two rankings in steps of 5% (0x426a0, 0x42ea0, 0x43680).
//!
//! Progress belongs to a player profile, as in the original: up to ten,
//! each with its own name and rider, saved as `profiles/<n>.cfg` beside
//! the options. Before a profile is made it goes to `career.cfg`, and the
//! first profile made carries that progress on.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use bevy::prelude::*;

use atv_formats::{setup, GameSource};

/// Riders in a career race, the player included.
pub const FIELD: usize = 8;
pub const ROUNDS: usize = 5;
pub const LAPS: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Champ {
    #[default]
    Amateur,
    /// `QUAL` in the original's menus, `Open` in `Career.xml`.
    Qualifying,
    Professional,
    /// The player's own: their tracks, laps and points split.
    Custom,
}

impl Champ {
    /// The three with set tracks (`TrackOrder`).
    pub const ALL: [Champ; 3] = [Champ::Amateur, Champ::Qualifying, Champ::Professional];
    pub const EVERY: [Champ; 4] = [Champ::Amateur, Champ::Qualifying, Champ::Professional, Champ::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Champ::Amateur => "AMATEUR",
            Champ::Qualifying => "QUALIFYING",
            Champ::Professional => "PROFESSIONAL",
            Champ::Custom => "CUSTOM",
        }
    }

    /// The championship a top-3 finish in this one opens.
    pub fn next(self) -> Option<Champ> {
        Self::EVERY.get(self.index() + 1).copied()
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// `Career.xml`'s name for it (`TrackOrder`, `RiderAvailability`).
    fn key(self) -> &'static str {
        match self {
            Champ::Amateur => "Amateur",
            Champ::Qualifying => "Open",
            Champ::Professional => "Professional",
            Champ::Custom => "Custom",
        }
    }

    /// `Frontend.xml`'s status screen for it.
    pub fn status_screen(self) -> &'static str {
        match self {
            Champ::Amateur => "Career_Status_Amateur",
            Champ::Qualifying => "Career_Status_Pro",
            Champ::Professional => "Career_Status_Pro2",
            Champ::Custom => "Career_Status_Custom",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::EVERY.into_iter().find(|c| c.key() == s)
    }
}

/// `Setup/Career.xml`.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct Rules {
    pub max_skill_amateur: usize,
    /// Points needed for skill levels 1 to 7.
    pub skill_levels: Vec<u32>,
    /// Lowest skill level of each certificate: Amateur, Professional,
    /// Superstar, Unknown.
    pub certificates: [u32; 4],
    pub position_points: [u32; FIELD],
    pub trick_points: [u32; FIELD],
    pub tracks: [Vec<String>; 3],
    /// Rider entry names (`Doug_Gust`) allowed in each championship
    /// ([`Champ::index`]).
    pub riders: [Vec<String>; 4],
    /// Gold medals needed, and the ATV it unlocks.
    pub unlockables: Vec<(u32, String)>,
    /// A rider who hasn't finished by this share of the winner's time
    /// doesn't finish (`mTimePercentage`).
    pub time_percentage: f32,
}

impl Default for Rules {
    fn default() -> Self {
        Self::parse("")
    }
}

pub const CERTIFICATES: [&str; 4] = ["AMATEUR", "PROFESSIONAL", "SUPERSTAR", "UNKNOWN"];

impl Rules {
    pub fn load(game: &GameSource) -> Self {
        Self::parse(&game.read("/Setup/Career.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default())
    }

    pub fn parse(xml: &str) -> Self {
        let root = setup::root(xml);
        let num = |m: &Option<std::collections::HashMap<String, String>>, k: &str| m.as_ref().and_then(|m| m.get(k)).and_then(|v| v.trim().parse::<f32>().ok());
        let table = |entry: &str, d: [u32; FIELD]| {
            let m = setup::entries(xml).into_iter().find(|e| e.name == entry).map(|e| e.attrs);
            std::array::from_fn(|i| num(&m, &format!("Position{i}")).map_or(d[i], |v| v as u32))
        };
        let points = [25, 20, 16, 10, 8, 5, 1, 0];
        let levels = setup::entries(xml).into_iter().find(|e| e.name == "SkillLevelPoints").map(|e| e.attrs);
        let default_levels = [0, 3000, 6000, 12000, 24000, 48000, 96000];
        let skill_levels = (1..=7).map(|l| num(&levels, &format!("SkillLevel{l}")).map_or(default_levels[l - 1], |v| v as u32)).collect();
        let certs = setup::entries(xml).into_iter().find(|e| e.name == "CertificateSkillLevels").map(|e| e.attrs);
        let certificates = [("Amateur", 0), ("Professional", 4), ("Superstar", 7), ("Unknown", 8)].map(|(k, d)| num(&certs, k).map_or(d, |v| v as u32));
        let tracks = Champ::ALL.map(|c| {
            let m = setup::child(xml, "TrackOrder", c.key());
            (0..ROUNDS).filter_map(|r| m.as_ref()?.get(&format!("Round{r}")).cloned()).collect()
        });
        let avail = setup::children(xml, "RiderAvailability");
        let riders = Champ::EVERY.map(|c| avail.iter().filter(|e| e.get(c.key()) == Some("1")).map(|e| e.name.clone()).collect());
        let unlockables = setup::entries(xml)
            .into_iter()
            .filter(|e| e.name.starts_with("Unlockable") && e.name != "Unlockable" && e.get("mATV") == Some("1"))
            .filter_map(|e| Some((e.num("mCondition")? as u32, e.get("mData")?.to_string())))
            .collect();
        Self {
            time_percentage: num(&root, "mTimePercentage").filter(|&v| v > 1.0).unwrap_or(1.3),
            max_skill_amateur: root.as_ref().and_then(|r| r.get("mMaxSkillForAmateur")).and_then(|v| v.parse().ok()).unwrap_or(7),
            skill_levels,
            certificates,
            position_points: table("PositionPoints", points),
            trick_points: table("TrickPoints", points),
            tracks,
            riders,
            unlockables,
        }
    }

    /// Skill level for a number of skill points (1 to 7).
    pub fn skill_level(&self, points: u32) -> usize {
        self.skill_levels.iter().rposition(|&p| p <= points).map_or(1, |i| i + 1)
    }

    pub fn certificate(&self, points: u32) -> &'static str {
        let level = self.skill_level(points) as u32;
        let i = self.certificates.iter().rposition(|&c| c <= level).unwrap_or(0);
        CERTIFICATES[i]
    }

    /// A set championship's rounds: its `TrackOrder`, [`LAPS`] each (none
    /// for Custom, whose tracks the player picks).
    pub fn schedule(&self, champ: Champ) -> Vec<(String, u32)> {
        self.tracks.get(champ.index()).map_or(Vec::new(), |t| t.iter().map(|t| (t.clone(), LAPS)).collect())
    }

    /// Custom's points tables (race, trick ranking) for a split: `split`%
    /// of each place's race and trick points together go to the trick
    /// ranking, the rest to the race (0x43680).
    pub fn split_points(&self, split: u32) -> ([u32; FIELD], [u32; FIELD]) {
        let split = split.min(100);
        let total: [u32; FIELD] = std::array::from_fn(|i| self.position_points[i] + self.trick_points[i]);
        let bonus = total.map(|t| t * split / 100);
        (std::array::from_fn(|i| total[i] - bonus[i]), bonus)
    }
}

/// One rider in a championship.
#[derive(Clone, Debug, PartialEq)]
pub struct Entrant {
    pub name: String,
    /// Model folder (`Male10`), empty for the player.
    pub model: String,
    pub points: u32,
    pub player: bool,
}

/// What a championship races: which one, its rounds (track and laps) and,
/// for Custom, the points split.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub champ: Champ,
    pub schedule: Vec<(String, u32)>,
    pub split: Option<u32>,
}

impl Plan {
    /// A set championship as `Career.xml` has it.
    pub fn set(rules: &Rules, champ: Champ) -> Self {
        Self { champ, schedule: rules.schedule(champ), split: None }
    }
}

/// A championship in progress.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub champ: Champ,
    /// Next round to race (0-based); the number of rounds once it's over.
    pub round: usize,
    pub rider: String,
    pub quad: String,
    pub field: Vec<Entrant>,
    /// Each round's track and laps.
    pub schedule: Vec<(String, u32)>,
    /// Custom's points split (see [`Rules::split_points`]).
    pub split: Option<u32>,
}

impl Run {
    pub fn new(plan: Plan, rider: String, quad: String, field: Vec<Entrant>) -> Self {
        Self { champ: plan.champ, round: 0, rider, quad, field, schedule: plan.schedule, split: plan.split }
    }

    pub fn rounds(&self) -> usize {
        self.schedule.len()
    }

    pub fn over(&self) -> bool {
        self.round >= self.rounds()
    }

    /// The next round's track and laps.
    pub fn next_round(&self) -> Option<(&str, u32)> {
        self.schedule.get(self.round).map(|(t, l)| (t.as_str(), *l))
    }

    /// The points tables (race, trick ranking) this championship scores by.
    pub fn points(&self, rules: &Rules) -> ([u32; FIELD], [u32; FIELD]) {
        match self.split {
            Some(s) => rules.split_points(s),
            None => (rules.position_points, rules.trick_points),
        }
    }

    /// The field by points, best first (the player first on a tie).
    pub fn standings(&self) -> Vec<&Entrant> {
        let mut s: Vec<&Entrant> = self.field.iter().collect();
        s.sort_by_key(|e| (std::cmp::Reverse(e.points), !e.player));
        s
    }

    pub fn player_place(&self) -> usize {
        self.standings().iter().position(|e| e.player).map_or(FIELD, |i| i + 1)
    }

    /// The computer riders' models, in field order.
    pub fn opponents(&self) -> Vec<String> {
        self.field.iter().filter(|e| !e.player).map(|e| e.model.clone()).collect()
    }
}

/// Player profiles the original keeps (0x16340).
pub const PROFILES: usize = 10;

/// Whose progress a [`Career`] is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    /// 0-based; saved as `profiles/<slot + 1>.cfg`.
    pub slot: usize,
    pub name: String,
    /// Their rider's model (`Male11`), which Career and Arcade ride.
    pub rider: String,
}

/// What one race earned, for the results screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Award {
    pub place: usize,
    pub position_points: u32,
    pub trick_points: u32,
    pub skill_points: u32,
    /// The new skill level, when it went up.
    pub new_level: Option<usize>,
    pub gold: bool,
    pub unlocked: Vec<String>,
}

#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Career {
    pub skill_points: u32,
    /// Qualifying, Professional and Custom opened (Amateur always is).
    pub open: [bool; 4],
    /// Best career finish per track (1-based).
    pub best: BTreeMap<String, usize>,
    pub run: Option<Run>,
    /// Freestyle challenges run (1-based): best time and best medal
    /// (0 bronze to 2 gold). The challenges' bonuses go to the same skill
    /// points.
    pub challenges: BTreeMap<usize, (f32, Option<usize>)>,
    /// The Arcade round to carry on from (0: start afresh), as the
    /// original's `mArcadeProgress`.
    pub arcade: usize,
    /// Tracks raced in an Arcade or Career round, which opens them (see
    /// [`crate::unlocks`]).
    pub reached: BTreeSet<String>,
    /// Cheats entered ([`crate::unlocks::CHEATS`] bits).
    pub cheats: u32,
    /// The profile this is, none before one is made.
    pub profile: Option<Profile>,
}

impl Career {
    fn dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "atv2").map(|d| d.config_dir().to_path_buf())
    }

    fn profile_path(slot: usize) -> Option<PathBuf> {
        Self::dir().map(|d| d.join("profiles").join(format!("{}.cfg", slot + 1)))
    }

    fn path(&self) -> Option<PathBuf> {
        match &self.profile {
            Some(p) => Self::profile_path(p.slot),
            None => Self::dir().map(|d| d.join("career.cfg")),
        }
    }

    /// The progress made before any profile.
    pub fn load() -> Self {
        Self::dir().and_then(|d| std::fs::read_to_string(d.join("career.cfg")).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    /// A saved profile, if that slot has one.
    pub fn load_profile(slot: usize) -> Option<Self> {
        let mut c = Self::parse(&std::fs::read_to_string(Self::profile_path(slot)?).ok()?);
        c.profile.as_mut()?.slot = slot;
        Some(c)
    }

    /// Every profile slot, empty or not.
    pub fn profiles() -> Vec<Option<Self>> {
        (0..PROFILES).map(Self::load_profile).collect()
    }

    pub fn new() -> Self {
        Self { open: [true, false, false, false], ..default() }
    }

    pub fn is_open(&self, c: Champ) -> bool {
        c == Champ::Amateur || self.open[c.index()]
    }

    pub fn golds(&self) -> u32 {
        self.best.values().filter(|&&p| p == 1).count() as u32
    }

    /// Fills in the rounds of a set championship saved before they were
    /// saved with it.
    pub fn upgrade(&mut self, rules: &Rules) {
        if let Some(r) = self.run.as_mut().filter(|r| r.schedule.is_empty()) {
            r.schedule = rules.schedule(r.champ);
        }
    }

    /// Starts a championship with a field drawn from the riders allowed in
    /// it (models from the roster, never the player's own rider).
    pub fn start(&mut self, plan: Plan, rider: (&str, &str), quad: &str, pool: &[(String, String)], seed: u32) {
        let mut seed = seed | 1;
        let mut pick = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as usize % n.max(1)
        };
        let mut pool: Vec<&(String, String)> = pool.iter().filter(|(_, m)| !m.eq_ignore_ascii_case(rider.1)).collect();
        let mut field = vec![Entrant { name: rider.0.to_string(), model: rider.1.to_string(), points: 0, player: true }];
        while field.len() < FIELD && !pool.is_empty() {
            let (name, model) = pool.swap_remove(pick(pool.len()));
            field.push(Entrant { name: name.clone(), model: model.clone(), points: 0, player: false });
        }
        self.run = Some(Run::new(plan, rider.1.to_string(), quad.to_string(), field));
    }

    /// Scores a finished round. `order`: field indices in finishing order
    /// (first = winner); `tricks`: each entrant's trick score, by field index.
    pub fn score(&mut self, rules: &Rules, track: &str, order: &[usize], tricks: &[u32]) -> Option<Award> {
        let golds_before = self.golds();
        let run = self.run.as_mut().filter(|r| !r.over())?;
        let (position_points, trick_points) = run.points(rules);
        let mut award = Award::default();
        // Trick ranking: higher score first, finishing order breaks ties.
        let mut by_tricks = order.to_vec();
        by_tricks.sort_by_key(|&i| std::cmp::Reverse(tricks.get(i).copied().unwrap_or(0)));
        for (place, &i) in order.iter().enumerate() {
            let pts = position_points.get(place).copied().unwrap_or(0);
            if let Some(e) = run.field.get_mut(i) {
                e.points += pts;
                if e.player {
                    award.place = place + 1;
                    award.position_points = pts;
                }
            }
        }
        for (place, &i) in by_tricks.iter().enumerate() {
            let pts = trick_points.get(place).copied().unwrap_or(0);
            if let Some(e) = run.field.get_mut(i) {
                e.points += pts;
                if e.player {
                    award.trick_points = pts;
                }
            }
        }
        let player = run.field.iter().position(|e| e.player).unwrap_or(0);
        let score = tricks.get(player).copied().unwrap_or(0);
        let level_before = rules.skill_level(self.skill_points);
        let before = self.skill_points;
        self.skill_points = self.skill_points.saturating_add(score);
        if run.champ == Champ::Amateur {
            let cap = rules.skill_levels.get(rules.max_skill_amateur.saturating_sub(1)).copied().unwrap_or(u32::MAX);
            self.skill_points = self.skill_points.min(cap.max(before));
        } else {
            self.skill_points = self.skill_points.saturating_add(score);
        }
        award.skill_points = self.skill_points - before;
        let level = rules.skill_level(self.skill_points);
        award.new_level = (level > level_before).then_some(level);

        // Medals are the set championships' (a Custom round can be a lap).
        if run.champ != Champ::Custom {
            let best = self.best.entry(track.to_string()).or_insert(usize::MAX);
            award.gold = award.place == 1 && *best != 1;
            *best = (*best).min(award.place.max(1));
        }
        let golds = self.golds();
        award.unlocked = rules.unlockables.iter().filter(|(c, _)| *c > golds_before && *c <= golds).map(|(_, a)| a.clone()).collect();

        let run = self.run.as_mut()?;
        run.round += 1;
        if run.over() && run.player_place() <= 3 {
            // The next championship opens.
            if let Some(next) = run.champ.next() {
                self.open[next.index()] = true;
            }
        }
        Some(award)
    }

    pub fn parse(text: &str) -> Self {
        let mut c = Self::new();
        let mut run: Option<Run> = None;
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "skill_points" => c.skill_points = v.parse().unwrap_or(0),
                "qualifying" => c.open[1] = v == "true",
                "professional" => c.open[2] = v == "true",
                "custom" => c.open[3] = v == "true",
                "arcade" => c.arcade = v.parse().unwrap_or(0),
                "reached" => {
                    c.reached.insert(v.to_string());
                }
                "cheat" => c.cheats |= crate::unlocks::cheat_bits(v).unwrap_or(0),
                "profile" => c.profile.get_or_insert_default().name = v.to_string(),
                "profile_rider" => c.profile.get_or_insert_default().rider = v.to_string(),
                "best" => {
                    if let Some((t, p)) = v.rsplit_once(' ')
                        && let Ok(p) = p.parse()
                    {
                        c.best.insert(t.to_string(), p);
                    }
                }
                "challenge" => {
                    let mut parts = v.split_whitespace();
                    if let (Some(Ok(n)), Some(Ok(t)), Some(m)) = (parts.next().map(str::parse), parts.next().map(str::parse), parts.next()) {
                        c.challenges.insert(n, (t, m.parse().ok().filter(|&m: &usize| m < 3)));
                    }
                }
                "champ" => {
                    if let Some(champ) = Champ::from_key(v) {
                        run = Some(Run::new(Plan { champ, schedule: Vec::new(), split: None }, String::new(), String::new(), Vec::new()));
                    }
                }
                "round" => {
                    if let Some(r) = run.as_mut() {
                        r.round = v.parse::<usize>().unwrap_or(0);
                    }
                }
                "track" => {
                    // track laps
                    if let (Some(r), Some((t, l))) = (run.as_mut(), v.rsplit_once(' '))
                        && let Ok(l) = l.parse()
                    {
                        r.schedule.push((t.to_string(), l));
                    }
                }
                "split" => {
                    if let Some(r) = run.as_mut() {
                        r.split = v.parse::<u32>().ok().map(|s| s.min(100));
                    }
                }
                "rider" => {
                    if let Some(r) = run.as_mut() {
                        r.rider = v.to_string();
                    }
                }
                "quad" => {
                    if let Some(r) = run.as_mut() {
                        r.quad = v.to_string();
                    }
                }
                k @ ("player" | "entrant") => {
                    // points model name...
                    let mut parts = v.splitn(3, ' ');
                    if let (Some(r), Some(Ok(points)), Some(model)) = (run.as_mut(), parts.next().map(str::parse), parts.next()) {
                        let model = if model == "-" { String::new() } else { model.to_string() };
                        r.field.push(Entrant { name: parts.next().unwrap_or("").to_string(), model, points, player: k == "player" });
                    }
                }
                _ => {}
            }
        }
        c.run = run.filter(|r| r.field.iter().any(|e| e.player)).map(|mut r| {
            // Saves from before the rounds were saved have five.
            r.round = r.round.min(if r.schedule.is_empty() { ROUNDS } else { r.rounds() });
            r
        });
        c
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        if let Some(p) = &self.profile {
            s += &format!("profile = {}\nprofile_rider = {}\n", p.name, p.rider);
        }
        s += &format!("skill_points = {}\nqualifying = {}\nprofessional = {}\n", self.skill_points, self.open[1], self.open[2]);
        if self.open[3] {
            s += "custom = true\n";
        }
        if self.arcade > 0 {
            s += &format!("arcade = {}\n", self.arcade);
        }
        for (t, p) in &self.best {
            s += &format!("best = {t} {p}\n");
        }
        for t in &self.reached {
            s += &format!("reached = {t}\n");
        }
        for (name, bits, _) in crate::unlocks::CHEATS {
            if bits.count_ones() == 1 && self.cheats & bits != 0 {
                s += &format!("cheat = {name}\n");
            }
        }
        for (n, (t, m)) in &self.challenges {
            s += &format!("challenge = {n} {t:.2} {}\n", m.map_or("-".to_string(), |m| m.to_string()));
        }
        if let Some(r) = &self.run {
            s += &format!("champ = {}\nround = {}\nrider = {}\nquad = {}\n", r.champ.key(), r.round, r.rider, r.quad);
            for (t, l) in &r.schedule {
                s += &format!("track = {t} {l}\n");
            }
            if let Some(split) = r.split {
                s += &format!("split = {split}\n");
            }
            for e in &r.field {
                let model = if e.model.is_empty() { "-" } else { &e.model };
                s += &format!("{} = {} {} {}\n", if e.player { "player" } else { "entrant" }, e.points, model, e.name);
            }
        }
        s
    }

    pub fn save(&self) {
        let Some(path) = self.path() else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, self.to_text())
        };
        if let Err(e) = write() {
            warn!("couldn't save the career to {}: {e}", path.display());
        }
    }
}

/// Most laps a Custom round can have.
pub const MAX_LAPS: u32 = 20;
/// Point Distribution moves in steps of this many percent.
pub const SPLIT_STEP: u32 = 5;

/// One track on Choose Your Tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomRow {
    pub track: String,
    /// Its round (0-based) while it's in the championship.
    pub round: Option<usize>,
    pub laps: u32,
}

/// A Custom championship being set up: the original's Choose Your Tracks
/// and Point Distribution pages.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomSetup {
    pub rows: Vec<CustomRow>,
    pub cursor: usize,
    /// Setting the highlighted track's laps rather than moving.
    pub editing: bool,
    /// The laps a newly added track takes: the last set, 1 at first.
    pub laps: u32,
    /// Share of the points for the trick ranking, percent (50 at first).
    pub split: u32,
}

impl Default for CustomSetup {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl CustomSetup {
    pub fn new(tracks: &[String]) -> Self {
        let rows = tracks.iter().map(|t| CustomRow { track: t.clone(), round: None, laps: 1 }).collect();
        Self { rows, cursor: 0, editing: false, laps: 1, split: 50 }
    }

    /// The same setup over a new list of tracks (more opened since): the
    /// rounds chosen stay, in their order.
    pub fn with_tracks(&self, tracks: &[String]) -> Self {
        let rows: Vec<CustomRow> = tracks
            .iter()
            .map(|t| self.rows.iter().find(|r| &r.track == t).cloned().unwrap_or(CustomRow { track: t.clone(), round: None, laps: 1 }))
            .collect();
        let mut next = Self { rows, cursor: self.cursor, editing: false, ..*self };
        let mut order: Vec<usize> = (0..next.rows.len()).filter(|&i| next.rows[i].round.is_some()).collect();
        order.sort_by_key(|&i| next.rows[i].round);
        for (k, i) in order.into_iter().enumerate() {
            next.rows[i].round = Some(k);
        }
        // The cursor stays on its track, or on Continue.
        next.cursor = match self.rows.get(self.cursor) {
            Some(r) => next.rows.iter().position(|n| n.track == r.track).unwrap_or(0),
            None => next.rows.len(),
        };
        next
    }

    /// Tracks in the championship so far.
    pub fn count(&self) -> usize {
        self.rows.iter().filter(|r| r.round.is_some()).count()
    }

    /// Adds the highlighted track as the next round, or takes it out and
    /// moves the later rounds up.
    pub fn toggle(&mut self) {
        let n = self.count();
        let laps = self.laps;
        let Some(row) = self.rows.get_mut(self.cursor) else { return };
        match row.round.take() {
            None => {
                row.round = Some(n);
                row.laps = laps;
            }
            Some(gone) => {
                for r in &mut self.rows {
                    if let Some(k) = r.round.as_mut().filter(|k| **k > gone) {
                        *k -= 1;
                    }
                }
                self.editing = false;
            }
        }
    }

    /// One more or one fewer lap on the highlighted track.
    pub fn step_laps(&mut self, more: bool) {
        let Some(row) = self.rows.get_mut(self.cursor).filter(|r| r.round.is_some()) else { return };
        row.laps = if more { (row.laps + 1).min(MAX_LAPS) } else { row.laps.saturating_sub(1).max(1) };
        self.laps = row.laps;
    }

    pub fn step_split(&mut self, more: bool) {
        self.split = if more { (self.split + SPLIT_STEP).min(100) } else { self.split.saturating_sub(SPLIT_STEP) };
    }

    /// The rounds, in order.
    pub fn plan(&self) -> Plan {
        let mut rounds: Vec<&CustomRow> = self.rows.iter().filter(|r| r.round.is_some()).collect();
        rounds.sort_by_key(|r| r.round);
        Plan { champ: Champ::Custom, schedule: rounds.into_iter().map(|r| (r.track.clone(), r.laps)).collect(), split: Some(self.split) }
    }
}

/// Sets the race's cutoff once the first computer rider is home. (If the
/// player wins, the race is already over for them.)
pub fn dnf_cutoff(
    sel: Res<crate::Selection>,
    rules: Res<Rules>,
    opponents: Option<Res<crate::opponents::Opponents>>,
    progress: Option<ResMut<crate::race::RaceProgress>>,
) {
    let (Some(opp), Some(mut p)) = (opponents, progress) else { return };
    if sel.mode != crate::Mode::Career || p.cutoff.is_some() {
        return;
    }
    if let Some(winner) = opp.racers.iter().filter_map(|r| r.progress.finished).reduce(f32::min) {
        p.cutoff = Some(winner * rules.time_percentage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<Root FileVersion="1.0" mMaxSkillForAmateur="2" mTimePercentage="1.3">
   <SkillLevelPoints SkillLevel1="0" SkillLevel2="3000" SkillLevel3="6000" SkillLevel4="12000" SkillLevel5="24000" SkillLevel6="48000" SkillLevel7="96000"/>
   <CertificateSkillLevels Amateur="0" Professional="4" Superstar="7" Unknown="8"/>
   <PositionPoints Position0="25" Position1="20" Position2="16" Position3="10" Position4="8" Position5="5" Position6="1" Position7="0"/>
   <TrickPoints Position0="25" Position1="20" Position2="16" Position3="10" Position4="8" Position5="5" Position6="1" Position7="0"/>
   <TrackOrder>
      <Amateur Round0="Dunes1" Round1="Forest1" Round2="Swamp1" Round3="Industrial1" Round4="Glacier1"/>
      <Open Round0="Dunes2" Round1="Forest2" Round2="Swamp2" Round3="Industrial2" Round4="Glacier2"/>
      <Professional Round0="Dunes3" Round1="Forest3" Round2="Swamp3" Round3="Industrial3" Round4="Glacier3"/>
   </TrackOrder>
   <RiderAvailability>
      <Doug_Gust Amateur="0" Open="0" Professional="1" Custom="1"/>
      <Cletus_Jones Amateur="1" Open="0" Professional="0" Custom="1"/>
   </RiderAvailability>
   <Unlockable mCondition="2" mRider="0" mATV="1" mData="Skyline"/>
   <Unlockable0 mCondition="2" mRider="0" mATV="1" mData="Skyline"/>
   <Unlockable1 mCondition="5" mRider="0" mATV="1" mData="Nitro"/>
</Root>"#;

    fn pool() -> Vec<(String, String)> {
        (1..=9).map(|i| (format!("RIDER {i}"), format!("Male{i}"))).collect()
    }

    #[test]
    fn rules_from_xml() {
        let r = Rules::parse(XML);
        assert_eq!(r.max_skill_amateur, 2);
        assert_eq!(r.tracks[1][4], "Glacier2");
        assert_eq!(r.riders[0], ["Cletus_Jones"]);
        assert_eq!(r.riders[2], ["Doug_Gust"]);
        assert_eq!(r.unlockables, [(2, "Skyline".to_string()), (5, "Nitro".to_string())]);
        assert_eq!((r.skill_level(0), r.skill_level(2999), r.skill_level(3000), r.skill_level(1 << 20)), (1, 1, 2, 7));
        assert_eq!((r.certificate(0), r.certificate(12000), r.certificate(96000)), ("AMATEUR", "PROFESSIONAL", "SUPERSTAR"));
    }

    #[test]
    fn a_round_scores_both_rankings() {
        let rules = Rules::parse(XML);
        let mut c = Career::new();
        c.start(Plan::set(&rules, Champ::Qualifying), ("ME", "Male1"), "Bandit", &pool(), 7);
        let run = c.run.as_ref().unwrap();
        assert_eq!(run.field.len(), FIELD);
        assert!(run.field.iter().filter(|e| !e.player).all(|e| e.model != "Male1"));
        // Player second over the line, best at tricks.
        let order = [3, 0, 1, 2, 4, 5, 6, 7];
        let mut tricks = vec![0; FIELD];
        tricks[0] = 1000;
        let a = c.score(&rules, "Dunes2", &order, &tricks).unwrap();
        assert_eq!((a.place, a.position_points, a.trick_points, a.skill_points), (2, 20, 25, 2000));
        let run = c.run.as_ref().unwrap();
        assert_eq!(run.field[0].points, 45);
        // Rider 3 won the race and was first of the trickless riders.
        assert_eq!(run.field[3].points, 25 + 20);
        assert_eq!(run.round, 1);
        assert!(!a.gold);
    }

    #[test]
    fn amateur_skill_is_capped_and_winning_opens_the_next() {
        let rules = Rules::parse(XML);
        let mut c = Career::new();
        c.start(Plan::set(&rules, Champ::Amateur), ("ME", "Male1"), "Bandit", &pool(), 3);
        let order: Vec<usize> = (0..FIELD).collect();
        let mut tricks = vec![0; FIELD];
        tricks[0] = 5000;
        let mut unlocked = Vec::new();
        for (i, t) in rules.tracks[0].clone().iter().enumerate() {
            let a = c.score(&rules, t, &order, &tricks).unwrap();
            assert!(a.gold);
            if i == 0 {
                assert_eq!(a.new_level, Some(2));
            }
            unlocked.extend(a.unlocked);
        }
        // Capped at level 2's points in Amateur.
        assert_eq!(c.skill_points, 3000);
        assert_eq!(unlocked, ["Skyline", "Nitro"]);
        assert!(c.run.as_ref().unwrap().over());
        assert!(c.is_open(Champ::Qualifying) && !c.is_open(Champ::Professional));
        assert!(c.score(&rules, "Dunes1", &order, &tricks).is_none());
    }

    #[test]
    fn custom_tracks_laps_and_points() {
        let rules = Rules::parse(XML);
        let tracks: Vec<String> = ["Dunes1", "Forest1", "Swamp1"].map(String::from).to_vec();
        let mut cs = CustomSetup::new(&tracks);
        // Swamp 1 first, then Dunes 1 with 3 laps, then Forest 1 (taking
        // the last laps set); taking Swamp 1 out moves the others up.
        cs.cursor = 2;
        cs.toggle();
        cs.cursor = 0;
        cs.toggle();
        cs.step_laps(true);
        cs.step_laps(true);
        cs.cursor = 1;
        cs.step_laps(true);
        assert_eq!(cs.rows[1].laps, 1, "laps only change on a track that's in");
        cs.toggle();
        cs.cursor = 2;
        cs.toggle();
        assert_eq!(cs.plan().schedule, [("Dunes1".to_string(), 3), ("Forest1".to_string(), 3)]);
        // A track opened since joins the list; the rounds stay.
        let more: Vec<String> = ["Dunes1", "Dunes2", "Forest1", "Swamp1"].map(String::from).to_vec();
        let wider = cs.with_tracks(&more);
        assert_eq!(wider.plan().schedule, cs.plan().schedule);
        assert_eq!((wider.rows[1].round, wider.rows[wider.cursor].track.as_str()), (None, "Swamp1"));
        for _ in 0..30 {
            cs.cursor = 0;
            cs.step_laps(true);
            cs.step_split(false);
        }
        assert_eq!((cs.rows[0].laps, cs.split), (MAX_LAPS, 0));
        // 50% shares each place's 25 + 25 evenly; 0% puts it all on the race.
        assert_eq!(rules.split_points(50).0[0..3], [25, 20, 16]);
        assert_eq!(rules.split_points(50).1[0..3], [25, 20, 16]);
        assert_eq!(rules.split_points(0).0[0..3], [50, 40, 32]);
        assert_eq!(rules.split_points(30).1[0..3], [15, 12, 9]);

        // Two rounds; winning both opens nothing and earns no medals.
        let mut c = Career::new();
        c.start(cs.plan(), ("ME", "Male1"), "Bandit", &pool(), 5);
        let order: Vec<usize> = (0..FIELD).collect();
        let a = c.score(&rules, "Dunes1", &order, &[0; FIELD]).unwrap();
        assert_eq!((a.position_points, a.trick_points, a.gold), (50, 0, false));
        assert_eq!(c.run.as_ref().unwrap().next_round(), Some(("Forest1", 3)));
        c.score(&rules, "Forest1", &order, &[0; FIELD]);
        assert!(c.run.as_ref().unwrap().over());
        assert_eq!(c.golds(), 0);
        assert_eq!(Career::parse(&c.to_text()), c);
    }

    #[test]
    fn professional_opens_custom() {
        let rules = Rules::parse(XML);
        let mut c = Career::new();
        c.start(Plan::set(&rules, Champ::Professional), ("ME", "Male1"), "Bandit", &pool(), 9);
        let order: Vec<usize> = (0..FIELD).collect();
        while c.score(&rules, "Dunes3", &order, &[0; FIELD]).is_some() {}
        assert!(c.is_open(Champ::Custom));
        assert!(Career::parse(&c.to_text()).is_open(Champ::Custom));
    }

    #[test]
    fn old_saves_get_their_rounds() {
        let rules = Rules::parse(XML);
        let mut c = Career::parse("champ = Open\nround = 2\nplayer = 30 - ME\n");
        assert!(c.run.as_ref().unwrap().over());
        c.upgrade(&rules);
        let run = c.run.as_ref().unwrap();
        assert_eq!((run.round, run.rounds(), run.next_round()), (2, ROUNDS, Some(("Swamp2", LAPS))));
    }

    #[test]
    fn save_round_trip() {
        let rules = Rules::parse(XML);
        let mut c = Career::new();
        c.start(Plan::set(&rules, Champ::Professional), ("JOHNNY HALE", "Male5"), "Storm", &pool(), 11);
        let order: Vec<usize> = (0..FIELD).rev().collect();
        c.score(&rules, "Dunes3", &order, &[10; FIELD]);
        c.open[1] = true;
        c.challenges.insert(3, (14.25, Some(1)));
        c.challenges.insert(12, (99.5, None));
        c.arcade = 6;
        assert_eq!(Career::parse(&c.to_text()), c);
        c.profile = Some(Profile { slot: 0, name: "BUBBA 2".into(), rider: "Male11".into() });
        assert_eq!(Career::parse(&c.to_text()), c);
        assert_eq!(Career::parse("junk"), Career::new());
    }
}
