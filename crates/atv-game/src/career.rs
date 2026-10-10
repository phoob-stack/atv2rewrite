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
//!
//! Progress is saved to `career.cfg` beside the options.

use std::collections::BTreeMap;
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
}

impl Champ {
    pub const ALL: [Champ; 3] = [Champ::Amateur, Champ::Qualifying, Champ::Professional];

    pub fn label(self) -> &'static str {
        match self {
            Champ::Amateur => "AMATEUR",
            Champ::Qualifying => "QUALIFYING",
            Champ::Professional => "PROFESSIONAL",
        }
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
        }
    }

    /// `Frontend.xml`'s status screen for it.
    pub fn status_screen(self) -> &'static str {
        match self {
            Champ::Amateur => "Career_Status_Amateur",
            Champ::Qualifying => "Career_Status_Pro",
            Champ::Professional => "Career_Status_Pro2",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == s)
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
    /// Rider entry names (`Doug_Gust`) allowed in each championship.
    pub riders: [Vec<String>; 3],
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
        let riders = Champ::ALL.map(|c| avail.iter().filter(|e| e.get(c.key()) == Some("1")).map(|e| e.name.clone()).collect());
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

    /// ATVs still locked with this many gold medals, and the golds each needs.
    pub fn locked_atvs(&self, golds: u32) -> Vec<(String, u32)> {
        self.unlockables.iter().filter(|(c, _)| *c > golds).map(|(c, atv)| (atv.clone(), *c)).collect()
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

/// A championship in progress.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub champ: Champ,
    /// Next round to race (0-based); [`ROUNDS`] once it's over.
    pub round: usize,
    pub rider: String,
    pub quad: String,
    pub field: Vec<Entrant>,
}

impl Run {
    pub fn over(&self) -> bool {
        self.round >= ROUNDS
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
    /// Qualifying and Professional opened (Amateur always is).
    pub open: [bool; 3],
    /// Best career finish per track (1-based).
    pub best: BTreeMap<String, usize>,
    pub run: Option<Run>,
}

impl Career {
    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "atv2").map(|d| d.config_dir().join("career.cfg"))
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn new() -> Self {
        Self { open: [true, false, false], ..default() }
    }

    pub fn is_open(&self, c: Champ) -> bool {
        c == Champ::Amateur || self.open[c.index()]
    }

    pub fn golds(&self) -> u32 {
        self.best.values().filter(|&&p| p == 1).count() as u32
    }

    /// Starts a championship with a field drawn from the riders allowed in
    /// it (models from the roster, never the player's own rider).
    pub fn start(&mut self, champ: Champ, rider: (&str, &str), quad: &str, pool: &[(String, String)], seed: u32) {
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
        self.run = Some(Run { champ, round: 0, rider: rider.1.to_string(), quad: quad.to_string(), field });
    }

    /// Scores a finished round. `order`: field indices in finishing order
    /// (first = winner); `tricks`: each entrant's trick score, by field index.
    pub fn score(&mut self, rules: &Rules, track: &str, order: &[usize], tricks: &[u32]) -> Option<Award> {
        let golds_before = self.golds();
        let run = self.run.as_mut().filter(|r| !r.over())?;
        let mut award = Award::default();
        // Trick ranking: higher score first, finishing order breaks ties.
        let mut by_tricks = order.to_vec();
        by_tricks.sort_by_key(|&i| std::cmp::Reverse(tricks.get(i).copied().unwrap_or(0)));
        for (place, &i) in order.iter().enumerate() {
            let pts = rules.position_points.get(place).copied().unwrap_or(0);
            if let Some(e) = run.field.get_mut(i) {
                e.points += pts;
                if e.player {
                    award.place = place + 1;
                    award.position_points = pts;
                }
            }
        }
        for (place, &i) in by_tricks.iter().enumerate() {
            let pts = rules.trick_points.get(place).copied().unwrap_or(0);
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

        let best = self.best.entry(track.to_string()).or_insert(usize::MAX);
        award.gold = award.place == 1 && *best != 1;
        *best = (*best).min(award.place.max(1));
        let golds = self.golds();
        award.unlocked = rules.unlockables.iter().filter(|(c, _)| *c > golds_before && *c <= golds).map(|(_, a)| a.clone()).collect();

        let run = self.run.as_mut()?;
        run.round += 1;
        if run.over() && run.player_place() <= 3 {
            // The next championship opens.
            if let Some(next) = Champ::ALL.get(run.champ.index() + 1) {
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
                "best" => {
                    if let Some((t, p)) = v.rsplit_once(' ')
                        && let Ok(p) = p.parse()
                    {
                        c.best.insert(t.to_string(), p);
                    }
                }
                "champ" => {
                    if let Some(champ) = Champ::from_key(v) {
                        run = Some(Run { champ, round: 0, rider: String::new(), quad: String::new(), field: Vec::new() });
                    }
                }
                "round" => {
                    if let Some(r) = run.as_mut() {
                        r.round = v.parse::<usize>().unwrap_or(0).min(ROUNDS);
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
        c.run = run.filter(|r| r.field.iter().any(|e| e.player));
        c
    }

    pub fn to_text(&self) -> String {
        let mut s = format!("skill_points = {}\nqualifying = {}\nprofessional = {}\n", self.skill_points, self.open[1], self.open[2]);
        for (t, p) in &self.best {
            s += &format!("best = {t} {p}\n");
        }
        if let Some(r) = &self.run {
            s += &format!("champ = {}\nround = {}\nrider = {}\nquad = {}\n", r.champ.key(), r.round, r.rider, r.quad);
            for e in &r.field {
                let model = if e.model.is_empty() { "-" } else { &e.model };
                s += &format!("{} = {} {} {}\n", if e.player { "player" } else { "entrant" }, e.points, model, e.name);
            }
        }
        s
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
            warn!("couldn't save the career to {}: {e}", path.display());
        }
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
        c.start(Champ::Qualifying, ("ME", "Male1"), "Bandit", &pool(), 7);
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
        c.start(Champ::Amateur, ("ME", "Male1"), "Bandit", &pool(), 3);
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
    fn save_round_trip() {
        let rules = Rules::parse(XML);
        let mut c = Career::new();
        c.start(Champ::Professional, ("JOHNNY HALE", "Male5"), "Storm", &pool(), 11);
        let order: Vec<usize> = (0..FIELD).rev().collect();
        c.score(&rules, "Dunes3", &order, &[10; FIELD]);
        c.open[1] = true;
        assert_eq!(Career::parse(&c.to_text()), c);
        assert_eq!(Career::parse("junk"), Career::new());
    }
}
