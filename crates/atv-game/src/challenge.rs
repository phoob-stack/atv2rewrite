//! Freestyle Challenges: twelve timed runs on Stadium 2, with Bronze,
//! Silver and Gold record times from `Setup/Freestyle.xml` (`FSChallenge`).
//!
//! The rules follow the original's challenge code:
//! - Challenge N's course is the `checkpoint` locators numbered N*100 to
//!   N*100+39 in `Stadium2.row`, passed in order; the run starts at the
//!   `startN` locator.
//! - Props whose flags carry challenge numbers (`FREE 1` cones,
//!   `GEN_Start_gate 1 2 3 5 6`, the floor arrows) are only out in those
//!   challenges (see [`Instance::in_play`]).
//! - The clock counts up; a time under a record (strictly) earns its
//!   medal. Hitting a cone adds a second.
//! - A better medal than before pays the medal's bonus in skill points
//!   once for each tier it climbs, as the original does (a first gold pays
//!   the gold bonus three times).
//! - Gold in challenge K+1 frees the rider of `UnlockableK`.
//! - Before the run, in place of the race intro, a page shows the
//!   challenge's name and task over the waiting quad, and the countdown
//!   starts on a button (`ID_FS_INTRO`, 0x55bf0, run by 0x7fcc0..0x7fd70).
//! - A challenge opens once every earlier one has a medal (or with the
//!   ALLCHALS cheat).
//!
//! The disc gives no rules for the trick challenges beyond their text, so
//! these are the rewrite's: in 5 and 7 each gate counts only when crossed
//! in a bicycle; in 6 the endo box must be crossed end to end in one endo.
//! Challenge 12 ("burst all the balloons") has no course on the disc.
//!
//! [`Instance::in_play`]: atv_formats::row::Instance::in_play

use bevy::prelude::*;

use atv_formats::row::World;
use atv_formats::{setup, GameSource};
use atv_sim::track as trk;

use crate::career::{Career, Rules};
use crate::freestyle::{Ground, PopupKind, Session, MEDALS};
use crate::race::{fmt_time, RaceProgress};
use crate::ui::{read_nav, set_text, BitmapText, FontId, Nav, UiAssets, SCREEN_H, SCREEN_W};
use crate::{AppState, Args, Mode, Player, Race, Selection};

pub const COUNT: usize = 12;
pub const TRACK: &str = "Stadium2";
const CONE_PENALTY: f32 = 1.0;
/// Quad half-width plus a cone's radius.
const CONE_HIT: f32 = 1.1;

/// One challenge as the select screen shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Def {
    /// Bronze, silver and gold record times, in seconds.
    pub records: [f32; 3],
    /// Skill points per tier for bronze, silver and gold.
    pub bonus: [u32; 3],
    /// The rider gold frees (`Jason_Luburgh`).
    pub reward: Option<String>,
    /// The challenge's instructions (`CHALLENGE_TEXT_N`).
    pub text: String,
    /// Gates in its course; 0 when the disc has none.
    pub gates: usize,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct Defs(pub Vec<Def>);

impl Defs {
    pub fn load(game: &GameSource) -> Self {
        let read = |p: &str| game.read(p).map(|b| b.iter().map(|&c| c as char).collect::<String>()).unwrap_or_default();
        let world = game.read(&format!("/Levels/{TRACK}/World/{TRACK}.row")).ok().and_then(|b| World::parse(&b).ok());
        let mut defs = Self::parse(&read("/Setup/Freestyle.xml"), &read("/Strings/Strings_ENG.txt"));
        if let Some(w) = world {
            for (i, d) in defs.0.iter_mut().enumerate() {
                d.gates = course(&w, i + 1).len();
            }
        }
        defs
    }

    /// `strings`: the language file, one value per line in key order.
    pub fn parse(xml: &str, strings: &str) -> Self {
        let lines: Vec<&str> = strings.lines().collect();
        // `CHALLENGE_TEXT_1`, `_10`, `_11`, `_12`, `_2` .. `_9` sort to
        // these lines.
        let mut keys: Vec<String> = (1..=COUNT).map(|n| n.to_string()).collect();
        keys.sort();
        let entries = setup::children(xml, "FSChallenge");
        let rewards = setup::entries(xml);
        Self(
            (1..=COUNT)
                .map(|n| {
                    let tag = format!("Challenge_{n}");
                    let e = entries.iter().find(|e| e.name == tag);
                    let block = xml.split(&format!("<{tag} ")).nth(1).and_then(|b| b.split(&format!("</{tag}>")).next()).unwrap_or("");
                    let record = |medal: &str| {
                        let rec = block.split(&format!("<{medal}Record")).nth(1)?;
                        rec.split("Record=\"").nth(1)?.split('"').next()?.trim().parse::<f32>().ok()
                    };
                    let bonus = |k: &str, d: u32| e.and_then(|e| e.num(k)).map_or(d, |v| v as u32);
                    let line = 91 + keys.iter().position(|k| *k == n.to_string()).unwrap_or(0);
                    Def {
                        records: [record("Bronze"), record("Silver"), record("Gold")].map(|r| r.unwrap_or(0.0)),
                        bonus: [bonus("mBronzeBonus", 200), bonus("mSilverBonus", 500), bonus("mGoldBonus", 1000)],
                        reward: rewards
                            .iter()
                            .find(|u| u.name == format!("Unlockable{}", n - 1) && u.get("mRider") == Some("1"))
                            .and_then(|u| u.get("mData"))
                            .map(str::to_string),
                        text: lines.get(line).map_or(String::new(), |l| l.trim().to_string()),
                        gates: 0,
                    }
                })
                .collect(),
        )
    }

    pub fn get(&self, n: usize) -> Option<&Def> {
        self.0.get(n.checked_sub(1)?)
    }
}

impl Def {
    /// The medal a time earns (0 bronze to 2 gold).
    pub fn medal(&self, time: f32) -> Option<usize> {
        if self.records == [0.0; 3] {
            return None;
        }
        (0..3).rev().find(|&i| time < self.records[i])
    }
}

/// The trick a challenge's gates want.
pub fn needs(n: usize) -> Option<Ground> {
    match n {
        5 | 7 => Some(Ground::Bicycle),
        6 => Some(Ground::Endo),
        _ => None,
    }
}

/// Whether challenge `n` can be picked: every earlier one has a medal.
pub fn is_open(career: &Career, n: usize) -> bool {
    career.cheats & crate::unlocks::CHALS != 0 || (1..n).all(|k| career.challenges.get(&k).is_some_and(|c| c.1.is_some()))
}

/// The gate a quad has to pass: the locator's box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gate {
    pub center: Vec3,
    rot: Quat,
    half: Vec3,
}

impl Gate {
    fn local(&self, p: Vec3) -> Vec3 {
        self.rot.inverse() * (p - self.center)
    }

    /// Inside the box; thin gates get a metre either side, so a fast quad
    /// can't step over one.
    fn contains(&self, p: Vec3) -> bool {
        let l = self.local(p).abs();
        l.x <= self.half.x && l.y <= self.half.y && l.z <= self.half.z.max(1.0)
    }
}

/// Challenge `n`'s gates, in order (`checkpoint 600 endo box` counts as
/// 600; a repeated number keeps its first box).
pub fn course(world: &World, n: usize) -> Vec<Gate> {
    let lo = n as u32 * 100;
    let mut gates: Vec<(u32, Gate)> = world
        .locators
        .iter()
        .filter_map(|l| {
            let name = l.name.trim().to_ascii_lowercase();
            let id: u32 = name.strip_prefix("checkpoint")?.split_whitespace().next()?.parse().ok()?;
            (lo..lo + 40).contains(&id).then(|| (id, Gate { center: trk::to_sim(l.pos), rot: trk::rotation_to_sim(&l.rotation), half: Vec3::from(l.scale).abs() }))
        })
        .collect();
    gates.sort_by_key(|g| g.0);
    gates.dedup_by_key(|g| g.0);
    gates.into_iter().map(|g| g.1).collect()
}

/// The challenge being run.
#[derive(Resource, Debug)]
pub struct Course {
    pub n: usize,
    pub gates: Vec<Gate>,
    pub next: usize,
    /// Cones out in this challenge, and whether each has been hit.
    cones: Vec<(Vec3, bool)>,
    pub penalties: f32,
    need: Option<Ground>,
    /// Endo box: the end the current attempt started from.
    side: Option<f32>,
    /// Inside the next gate last step (one warning per pass).
    inside: bool,
}

impl Course {
    pub fn new(world: &World, n: usize) -> Self {
        let cones = world
            .instances
            .iter()
            .filter(|i| i.model.to_ascii_lowercase().starts_with("gen_cone") && i.challenges().contains(&n))
            .map(|i| (trk::to_sim(i.pos), false))
            .collect();
        Self { n, gates: course(world, n), next: 0, cones, penalties: 0.0, need: needs(n), side: None, inside: false }
    }

    fn reset(&mut self) {
        self.next = 0;
        self.penalties = 0.0;
        self.side = None;
        self.inside = false;
        for c in &mut self.cones {
            c.1 = false;
        }
    }

    pub fn done(&self) -> bool {
        !self.gates.is_empty() && self.next >= self.gates.len()
    }

    /// One step at `at` holding `trick`; the messages to show.
    fn step(&mut self, at: Vec3, trick: Option<Ground>) -> Vec<(String, PopupKind)> {
        let mut out = Vec::new();
        for c in self.cones.iter_mut().filter(|c| !c.1 && (c.0 - at).with_y(0.0).length() < CONE_HIT && (c.0.y - at.y).abs() < 2.0) {
            c.1 = true;
            self.penalties += CONE_PENALTY;
            out.push(("CONE  +1 SEC".to_string(), PopupKind::Bail));
        }
        let Some(gate) = self.gates.get(self.next).copied() else { return out };
        let inside = gate.contains(at);
        let entered = inside && !self.inside;
        self.inside = inside;
        match self.need {
            // The endo box: from one end to the other in one endo.
            Some(Ground::Endo) => {
                let l = gate.local(at);
                let at_end = inside && gate.half.z - l.z.abs() < 6.0;
                let endo = trick == Some(Ground::Endo);
                match self.side {
                    _ if !inside => self.side = None,
                    None if at_end && endo => self.side = Some(l.z.signum()),
                    Some(_) if !endo => {
                        self.side = None;
                        out.push(("ENDO LOST  -  START AGAIN FROM AN END".to_string(), PopupKind::Bail));
                    }
                    Some(s) if at_end && l.z.signum() != s => self.pass(&mut out),
                    _ => {}
                }
                if entered && !endo {
                    out.push(("ENDO ACROSS THE BOX".to_string(), PopupKind::Bonus));
                }
            }
            Some(need) if inside => {
                if trick == Some(need) {
                    self.pass(&mut out);
                } else if entered {
                    out.push(("BICYCLE THROUGH THE GATE".to_string(), PopupKind::Bonus));
                }
            }
            None if inside => self.pass(&mut out),
            _ => {}
        }
        out
    }

    fn pass(&mut self, out: &mut Vec<(String, PopupKind)>) {
        self.next += 1;
        self.inside = false;
        self.side = None;
        if !self.done() {
            out.push((format!("GATE {}/{}", self.next, self.gates.len()), PopupKind::Trick));
        }
    }
}

/// Keeps the course: gates, cones and the finish.
pub fn follow_course(
    sel: Res<Selection>,
    player: Option<Res<Player>>,
    course: Option<ResMut<Course>>,
    progress: Option<ResMut<RaceProgress>>,
    session: Option<ResMut<Session>>,
    race: Option<ResMut<Race>>,
) {
    let (Some(player), Some(mut course), Some(mut p), Some(mut session), Some(mut race)) = (player, course, progress, session, race) else { return };
    if sel.mode != Mode::Challenge {
        return;
    }
    if !p.started() {
        // On the grid (again, after a quick restart).
        course.reset();
    } else if p.finished.is_none() {
        for (text, kind) in course.step(player.0.position, session.ground_trick()) {
            session.popup(text, kind);
        }
        if course.done() {
            p.finished = Some(p.clock + course.penalties);
        }
    }
    race.next_checkpoint = course.next.min(race.checkpoints.len().saturating_sub(1));
}

/// What a finished run earned.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub time: f32,
    pub penalties: f32,
    pub medal: Option<usize>,
    pub best_time: bool,
    pub skill_points: u32,
    pub new_level: Option<usize>,
    pub unlocked: Option<String>,
}

/// Records a finished run of challenge `n` in `time` (penalties included).
pub fn score(career: &mut Career, rules: &Rules, def: &Def, n: usize, time: f32, penalties: f32) -> Outcome {
    let medal = def.medal(time);
    let prev = career.challenges.get(&n).copied();
    let old = prev.and_then(|p| p.1);
    // Each tier climbed pays the new medal's bonus.
    let skill_points = match medal {
        Some(m) if old.is_none_or(|o| m > o) => (m as i32 - old.map_or(-1, |o| o as i32)) as u32 * def.bonus[m],
        _ => 0,
    };
    let level = rules.skill_level(career.skill_points);
    career.skill_points += skill_points;
    let new_level = Some(rules.skill_level(career.skill_points)).filter(|&l| l > level);
    let best_time = prev.is_none_or(|p| time < p.0);
    career.challenges.insert(n, (if best_time { time } else { prev.map_or(time, |p| p.0) }, medal.max(old)));
    let unlocked = (medal == Some(2) && old != Some(2)).then(|| def.reward.clone()).flatten();
    Outcome { time, penalties, medal, best_time, skill_points, new_level, unlocked }
}

/// A rider entry name as the menus show it (`JASON LUBURGH`).
pub fn rider_label(entry: &str) -> String {
    entry.replace('_', " ").to_uppercase()
}

/// The select screen's lines for a challenge.
pub fn summary(def: &Def, best: Option<(f32, Option<usize>)>) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let won = best.and_then(|b| b.1);
    for i in (0..3).rev() {
        let pay = if won.is_some_and(|w| w >= i) { "MEDAL WON".to_string() } else { format!("{} PTS", def.bonus[i]) };
        out.push((format!("{:<8}{}   {pay}", MEDALS[i], fmt_time(def.records[i])), won.is_some_and(|w| w >= i)));
    }
    if let Some((t, m)) = best {
        out.push((format!("YOUR BEST  {}{}", fmt_time(t), m.map_or(String::new(), |m| format!("  {}", MEDALS[m]))), false));
    }
    if let Some(r) = &def.reward {
        out.push((format!("GOLD UNLOCKS {}", rider_label(r)), false));
    }
    out
}

/// Splits text into lines of at most `width` characters.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.len() + 1 + word.len() <= width => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

pub struct ChallengePlugin;

impl Plugin for ChallengePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (spawn_hud.run_if(resource_added::<Course>), brief.run_if(resource_added::<Course>), update_hud, guide_arrow).chain().run_if(in_state(AppState::Race)),
        )
        .add_systems(FixedUpdate, crate::intro::hold.before(crate::drive).run_if(resource_exists::<Briefing>).run_if(in_state(AppState::Race)))
        // After the pause menu has looked at this frame's buttons, so the
        // one that starts the run doesn't also pause it.
        .add_systems(PostUpdate, (cover, crate::frontend::blink, dismiss).chain().run_if(resource_exists::<Briefing>).run_if(in_state(AppState::Race)))
        .add_systems(OnExit(AppState::Race), |mut commands: Commands| commands.remove_resource::<Briefing>());
    }
}

/// The challenge's intro page is up: the run waits for a button.
#[derive(Resource, Default)]
pub struct Briefing {
    /// The screens hidden for it, and how they were.
    hidden: Vec<(Entity, Visibility)>,
}

#[derive(Component)]
struct BriefingPage;

/// Opens the intro page (0x55bf0): the challenge's name, its task in a
/// box, and "Press Start" (string 0x17c). `--no-intro` skips it, and so
/// does the autopilot, with no one to press a button.
fn brief(mut commands: Commands, course: Res<Course>, defs: Res<Defs>, ui: Option<Res<UiAssets>>, args: Res<Args>) {
    let Some(ui) = ui else { return };
    if args.no_intro || args.autodrive {
        return;
    }
    let text = defs.get(course.n).map_or(String::new(), |d| d.text.to_uppercase());
    let panel = crate::frontend::popup(&mut commands, &ui, &format!("CHALLENGE {}", course.n), &text);
    // The whole screen dims, not just the page's 4:3 frame.
    commands.entity(panel).remove::<BackgroundColor>();
    let start = "PRESS START";
    let width = ui.font(FontId::Menu).width(start, 18.0);
    let prompt = commands
        .spawn((
            BitmapText::new(FontId::Menu, 18.0, start),
            Node { position_type: PositionType::Absolute, left: px((SCREEN_W - width) / 2.0), top: px(400.0), ..default() },
            crate::frontend::Blink,
        ))
        .id();
    commands
        .spawn((crate::ui::fullscreen(), BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)), DespawnOnExit(AppState::Race), Pickable::IGNORE, BriefingPage))
        .with_children(|root| {
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: percent(50),
                margin: UiRect::left(px(-SCREEN_W / 2.0)),
                width: px(SCREEN_W),
                height: px(SCREEN_H),
                ..default()
            })
            .add_children(&[panel, prompt]);
        });
    commands.insert_resource(Briefing::default());
}

/// Keeps the HUD hidden while the page is up (0xa4600).
#[allow(clippy::type_complexity)]
fn cover(mut briefing: ResMut<Briefing>, mut screens: Query<(Entity, &mut Visibility), (With<Node>, Without<ChildOf>, With<DespawnOnExit<AppState>>, Without<BriefingPage>)>) {
    for (e, mut v) in &mut screens {
        if !briefing.hidden.iter().any(|h| h.0 == e) {
            briefing.hidden.push((e, *v));
            *v = Visibility::Hidden;
        }
    }
}

/// A or Start closes the page and the countdown begins (0x7fd70); the HUD
/// comes back (0x83730).
fn dismiss(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&bevy::input::gamepad::Gamepad>,
    mut briefing: ResMut<Briefing>,
    pages: Query<Entity, With<BriefingPage>>,
    mut screens: Query<&mut Visibility, Without<BriefingPage>>,
) {
    if !matches!(read_nav(&keys, &pads), Some(Nav::Accept | Nav::Start)) {
        return;
    }
    for (e, was) in briefing.hidden.drain(..) {
        if let Ok(mut v) = screens.get_mut(e) {
            *v = was;
        }
    }
    for e in &pages {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<Briefing>();
}

#[derive(Component)]
enum Readout {
    Time,
    Gates,
    Penalties,
}

/// The guide arrow over the next gate.
#[derive(Component)]
pub struct GuideArrow;

fn spawn_hud(mut commands: Commands, course: Res<Course>, defs: Res<Defs>) {
    let records = defs.get(course.n).map_or(String::new(), |d| {
        (0..3).rev().map(|i| format!("{} {}", MEDALS[i], fmt_time(d.records[i]))).collect::<Vec<_>>().join("   ")
    });
    commands
        .spawn((crate::ui::fullscreen(), DespawnOnExit(AppState::Race), Pickable::IGNORE))
        .with_children(|root| {
            root.spawn(Node { position_type: PositionType::Absolute, left: px(24), top: px(18), flex_direction: FlexDirection::Column, row_gap: px(4), ..default() })
                .with_children(|c| {
                    c.spawn(BitmapText::new(FontId::Menu, 16.0, format!("CHALLENGE {}", course.n)));
                    c.spawn((BitmapText::new(FontId::Small, 9.0, ""), Readout::Gates));
                    c.spawn(BitmapText::new(FontId::Small, 8.0, records).color(Color::srgb(1.0, 0.85, 0.3)));
                });
            root.spawn(Node { position_type: PositionType::Absolute, right: px(24), top: px(18), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, row_gap: px(4), ..default() })
                .with_children(|c| {
                    c.spawn(Node { align_items: AlignItems::FlexEnd, column_gap: px(6), ..default() }).with_children(|r| {
                        r.spawn(BitmapText::new(FontId::Small, 8.0, "TIME"));
                        r.spawn((BitmapText::new(FontId::Chrome, 18.0, "0:00.00"), Readout::Time));
                    });
                    c.spawn((BitmapText::new(FontId::Small, 8.0, "").color(Color::srgb(1.0, 0.45, 0.3)), Readout::Penalties));
                });
        });
}

fn update_hud(course: Option<Res<Course>>, progress: Option<Res<RaceProgress>>, mut readouts: Query<(&Readout, &mut BitmapText)>) {
    let (Some(c), Some(p)) = (course, progress) else { return };
    for (r, mut t) in &mut readouts {
        let text = match r {
            Readout::Time => fmt_time(p.finished.unwrap_or(p.clock + c.penalties)),
            Readout::Gates if c.gates.is_empty() => "NO COURSE ON THE DISC".to_string(),
            Readout::Gates => format!("GATES {}/{}", c.next, c.gates.len()),
            Readout::Penalties if c.penalties > 0.0 => format!("PENALTIES +{:.0} SEC", c.penalties),
            Readout::Penalties => String::new(),
        };
        set_text(&mut t, &text);
    }
}

/// The arrow floats over the quad and points at the next gate; hidden
/// once done. The model is about 20 m long, so it is shrunk to a pointer.
fn guide_arrow(time: Res<Time>, course: Option<Res<Course>>, player: Option<Res<Player>>, mut arrows: Query<(&mut Transform, &mut Visibility), With<GuideArrow>>) {
    let (Some(c), Some(player)) = (course, player) else { return };
    let at = player.0.position;
    for (mut t, mut vis) in &mut arrows {
        match c.gates.get(c.next) {
            Some(g) => {
                // The model points down its own -Z once in the Bevy frame.
                let dir = (g.center - at).with_y(0.0).normalize_or(Vec3::NEG_Z);
                let bob = 0.15 * (time.elapsed_secs() * 3.0).sin();
                *t = Transform::from_translation(at + Vec3::Y * (2.1 + bob)).looking_to(-dir, Vec3::Y).with_scale(Vec3::splat(0.07));
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<Root FileVersion="1.0" mNumUnlockables="12">
   <Unlockable mCondition="0" mRider="1" mATV="0" mData="Nobody"/>
   <Unlockable0 mCondition="0" mRider="1" mATV="0" mData="Jason_Luburgh"/>
   <Unlockable1 mCondition="1" mRider="1" mATV="0" mData="Keith_Little"/>
   <FSChallenge>
      <Challenge_1 mBronzeBonus="200" mSilverBonus="500" mGoldBonus="1000">
         <BronzeRecord Record="15" Name="A"/>
         <SilverRecord Record="14" Name="B"/>
         <GoldRecord Record="13" Name="C"/>
      </Challenge_1>
      <Challenge_2 mBronzeBonus="20" mSilverBonus="50" mGoldBonus="100">
         <BronzeRecord Record="18" Name="A"/>
         <SilverRecord Record="16" Name="B"/>
         <GoldRecord Record="14" Name="C"/>
      </Challenge_2>
   </FSChallenge>
</Root>"#;

    fn strings() -> String {
        let mut lines = vec!["x".to_string(); 91];
        lines.extend(["text 1", "text 10", "text 11", "text 12", "text 2"].map(String::from));
        lines.join("\r\n")
    }

    #[test]
    fn defs_from_xml() {
        let d = Defs::parse(XML, &strings());
        assert_eq!(d.0.len(), COUNT);
        let one = d.get(1).unwrap();
        assert_eq!(one.records, [15.0, 14.0, 13.0]);
        assert_eq!(one.bonus, [200, 500, 1000]);
        assert_eq!(one.reward.as_deref(), Some("Jason_Luburgh"));
        assert_eq!(one.text, "text 1");
        assert_eq!(d.get(2).unwrap().text, "text 2");
        assert_eq!(d.get(2).unwrap().bonus, [20, 50, 100]);
        assert_eq!(d.get(12).unwrap().text, "text 12");
        assert_eq!(d.get(3).unwrap().records, [0.0; 3]);
        // Strictly under a record.
        assert_eq!(one.medal(13.0), Some(1));
        assert_eq!(one.medal(12.9), Some(2));
        assert_eq!(one.medal(15.0), None);
    }

    #[test]
    fn medals_pay_per_tier_and_gold_unlocks() {
        let d = Defs::parse(XML, &strings());
        let rules = Rules::parse("");
        let mut c = Career::new();
        assert!(is_open(&c, 1) && !is_open(&c, 2));
        let o = score(&mut c, &rules, d.get(1).unwrap(), 1, 14.5, 0.0);
        assert_eq!((o.medal, o.skill_points, o.unlocked.as_deref()), (Some(0), 200, None));
        assert!(is_open(&c, 2));
        // Bronze to gold climbs two tiers: twice the gold bonus.
        let o = score(&mut c, &rules, d.get(1).unwrap(), 1, 12.0, 1.0);
        assert_eq!((o.medal, o.skill_points, o.unlocked.as_deref()), (Some(2), 2000, Some("Jason_Luburgh")));
        assert_eq!(c.skill_points, 2200);
        // No better medal, no bonus; the best time stays.
        let o = score(&mut c, &rules, d.get(1).unwrap(), 1, 12.5, 0.0);
        assert_eq!((o.skill_points, o.best_time, o.unlocked), (0, false, None));
        assert_eq!(c.challenges[&1], (12.0, Some(2)));
        // A first gold pays three times.
        let o = score(&mut c, &rules, d.get(2).unwrap(), 2, 10.0, 0.0);
        assert_eq!(o.skill_points, 300);
    }

    #[test]
    fn gates_in_order_and_cones() {
        let gate = |z: f32| Gate { center: Vec3::new(0.0, 0.0, z), rot: Quat::IDENTITY, half: Vec3::new(5.0, 5.0, 0.5) };
        let mut c = Course { n: 1, gates: vec![gate(10.0), gate(20.0)], next: 0, cones: vec![(Vec3::new(0.0, 0.0, 5.0), false)], penalties: 0.0, need: None, side: None, inside: false };
        // The second gate doesn't count before the first.
        c.step(Vec3::new(0.0, 0.0, 20.0), None);
        assert_eq!(c.next, 0);
        c.step(Vec3::new(0.3, 0.0, 5.0), None);
        c.step(Vec3::new(0.3, 0.0, 5.1), None);
        assert_eq!(c.penalties, 1.0);
        c.step(Vec3::new(0.0, 0.0, 10.4), None);
        c.step(Vec3::new(0.0, 0.0, 20.0), None);
        assert!(c.done());
        // A bicycle gate wants the bicycle.
        c.reset();
        c.need = Some(Ground::Bicycle);
        c.step(Vec3::new(0.0, 0.0, 10.0), Some(Ground::Wheelie));
        assert_eq!(c.next, 0);
        c.step(Vec3::new(0.0, 0.0, 10.0), Some(Ground::Bicycle));
        assert_eq!(c.next, 1);
    }

    #[test]
    fn endo_box_end_to_end() {
        let gate = Gate { center: Vec3::ZERO, rot: Quat::IDENTITY, half: Vec3::new(10.0, 10.0, 30.0) };
        let mut c = Course { n: 6, gates: vec![gate], next: 0, cones: Vec::new(), penalties: 0.0, need: Some(Ground::Endo), side: None, inside: false };
        let endo = Some(Ground::Endo);
        // Starting mid-box doesn't count; nor does dropping the endo.
        c.step(Vec3::new(0.0, 0.0, 0.0), endo);
        c.step(Vec3::new(0.0, 0.0, -27.0), endo);
        assert!(!c.done());
        c.step(Vec3::new(0.0, 0.0, 40.0), None);
        c.step(Vec3::new(0.0, 0.0, 27.0), endo);
        c.step(Vec3::new(0.0, 0.0, 0.0), None);
        c.step(Vec3::new(0.0, 0.0, -27.0), endo);
        assert!(!c.done());
        c.step(Vec3::new(0.0, 0.0, 40.0), None);
        c.step(Vec3::new(0.0, 0.0, 27.0), endo);
        c.step(Vec3::new(0.0, 0.0, 0.0), endo);
        c.step(Vec3::new(0.0, 0.0, -27.0), endo);
        assert!(c.done());
    }

    #[test]
    fn wraps_words() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
    }
}
