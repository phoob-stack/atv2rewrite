//! Freestyle Challenges: twelve timed runs on Stadium 2, with Bronze,
//! Silver and Gold record times from `Setup/Freestyle.xml` (`FSChallenge`).
//!
//! The rules follow the original's challenge code (game type 5):
//! - Challenge N's course is the `checkpoint` locators numbered N*100 to
//!   N*100+39 in `Stadium2.row`, passed in order (0x90bb0); the run is won
//!   at the last one (0x99d50). A gate is the far (+Z) face of its box,
//!   passed when the quad touches it (0x64440, 0x64ac0). Every run starts
//!   at `start1` (0x86e00).
//! - Props whose flags carry challenge numbers (`FREE 1` cones,
//!   `GEN_Start_gate 1 2 3 5 6`, the floor arrows) are only out in those
//!   challenges (see [`Instance::in_play`]).
//! - The clock counts up from the start (0x265a0); a time under a record
//!   (strictly) earns its medal (0x99cb0). There is no time limit.
//! - The first respot ends the run, failed (0x803c0, 0x25f20).
//! - Cones cost nothing, whatever the challenge text says: the original
//!   arms a 5 s penalty (0x9ad90) only on a prop type it never makes. Nor
//!   does it check tricks: the bicycle and endo challenges (5 to 7) only
//!   want their gates.
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
//! Challenge 12 ("burst all the balloons") has no course on the disc, so
//! it can't be won, in the original either.
//!
//! [`Instance::in_play`]: atv_formats::row::Instance::in_play

use bevy::prelude::*;

use atv_formats::row::World;
use atv_formats::{setup, GameSource};

use crate::career::{Career, Rules};
use crate::freestyle::{MedalFigure, PopupKind, Session, MEDALS};
use crate::hud::{FIGURE, LABEL};
use crate::race::{self, fmt_time, hull, Gate, RaceProgress};
use crate::ui::{read_nav, set_text, BitmapText, FontId, Nav, UiAssets, SCREEN_H, SCREEN_W};
use crate::{AppState, Args, Mode, Player, Race, Selection};

pub const COUNT: usize = 12;
pub const TRACK: &str = "Stadium2";

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

/// Whether challenge `n` can be picked: every earlier one has a medal.
pub fn is_open(career: &Career, n: usize) -> bool {
    career.cheats & crate::unlocks::CHALS != 0 || (1..n).all(|k| career.challenges.get(&k).is_some_and(|c| c.1.is_some()))
}

/// Challenge `n`'s gates, in order (`checkpoint 600 endo box` counts as
/// 600).
pub fn course(world: &World, n: usize) -> Vec<Gate> {
    let lo = n as u32 * 100;
    race::gates(world, |name| {
        let id: u32 = name.trim().to_ascii_lowercase().strip_prefix("checkpoint")?.split_whitespace().next()?.parse().ok()?;
        (lo..lo + 40).contains(&id).then_some(id)
    })
}

/// The challenge being run.
#[derive(Resource, Debug)]
pub struct Course {
    pub n: usize,
    pub gates: Vec<Gate>,
    pub next: usize,
    /// Ended by a respot.
    pub failed: bool,
}

impl Course {
    pub fn new(world: &World, n: usize) -> Self {
        Self { n, gates: course(world, n), next: 0, failed: false }
    }

    fn reset(&mut self) {
        self.next = 0;
        self.failed = false;
    }

    pub fn done(&self) -> bool {
        !self.gates.is_empty() && self.next >= self.gates.len()
    }

    /// One step with the quad's hull at `hull`; the message to show.
    fn step(&mut self, hull: &[Vec3]) -> Option<String> {
        if !self.gates.get(self.next)?.touched(hull) {
            return None;
        }
        self.next += 1;
        (!self.done()).then(|| format!("GATE {}/{}", self.next, self.gates.len()))
    }
}

/// Keeps the course: the gates, the finish, and the respot that fails it.
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
        race.respots = 0;
    } else if !p.over() && race.respots > 0 {
        course.failed = true;
        p.dnf = true;
    } else if !p.over() {
        if let Some(text) = course.step(&hull(&player.0)) {
            session.popup(text, PopupKind::Trick);
        }
        if course.done() {
            p.finished = Some(p.clock);
        }
    }
    race.next_checkpoint = course.next.min(race.checkpoints.len().saturating_sub(1));
}

/// What a run earned.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// None when a respot ended it.
    pub time: Option<f32>,
    pub medal: Option<usize>,
    /// The medal beats the challenge's best before (0x80500).
    pub improved: bool,
    pub best_time: bool,
    pub skill_points: u32,
    pub new_level: Option<usize>,
    pub unlocked: Option<String>,
}

impl Outcome {
    /// A run a respot ended: nothing is recorded.
    pub const FAILED: Self = Self { time: None, medal: None, improved: false, best_time: false, skill_points: 0, new_level: None, unlocked: None };
}

/// Records a finished run of challenge `n` in `time`.
pub fn score(career: &mut Career, rules: &Rules, def: &Def, n: usize, time: f32) -> Outcome {
    let medal = def.medal(time);
    let prev = career.challenges.get(&n).copied();
    let old = prev.and_then(|p| p.1);
    let improved = medal.is_some_and(|m| old.is_none_or(|o| m > o));
    // Each tier climbed pays the new medal's bonus.
    let skill_points = match medal {
        Some(m) if improved => (m as i32 - old.map_or(-1, |o| o as i32)) as u32 * def.bonus[m],
        _ => 0,
    };
    let level = rules.skill_level(career.skill_points);
    career.skill_points += skill_points;
    let new_level = Some(rules.skill_level(career.skill_points)).filter(|&l| l > level);
    let best_time = prev.is_none_or(|p| time < p.0);
    career.challenges.insert(n, (if best_time { time } else { prev.map_or(time, |p| p.0) }, medal.max(old)));
    let unlocked = (medal == Some(2) && old != Some(2)).then(|| def.reward.clone()).flatten();
    Outcome { time: Some(time), medal, improved, best_time, skill_points, new_level, unlocked }
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
}

/// The guide arrow over the next gate.
#[derive(Component)]
pub struct GuideArrow;

fn spawn_hud(mut commands: Commands, course: Res<Course>, defs: Res<Defs>, ui: Option<Res<UiAssets>>) {
    let records = defs.get(course.n).is_some_and(|d| d.records != [0.0; 3]);
    commands
        .spawn((crate::ui::fullscreen(), crate::hud::HudRoot, DespawnOnExit(AppState::Race), Pickable::IGNORE))
        .with_children(|root| {
            root.spawn(Node { position_type: PositionType::Absolute, left: px(24), top: px(18), flex_direction: FlexDirection::Column, row_gap: px(4), ..default() })
                .with_children(|c| {
                    c.spawn(BitmapText::new(FontId::Menu, 16.0, format!("CHALLENGE {}", course.n)));
                    c.spawn((BitmapText::new(FontId::Small, 9.0, ""), Readout::Gates));
                    // The medal list (0xa1930): the record times, Gold
                    // first, the best one still in reach lit.
                    if let (Some(ui), true) = (ui.as_deref(), records) {
                        crate::freestyle::spawn_medal_list(c, ui);
                    }
                });
            root.spawn(Node { position_type: PositionType::Absolute, right: px(24), top: px(18), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, row_gap: px(4), ..default() })
                .with_children(|c| {
                    c.spawn(Node { align_items: AlignItems::FlexEnd, column_gap: px(6), ..default() }).with_children(|r| {
                        r.spawn(BitmapText::new(FontId::Small, 8.0, "TIME"));
                        r.spawn((BitmapText::new(FontId::Chrome, 18.0, "0:00.00"), Readout::Time));
                    });
                });
        });
}

fn update_hud(
    (course, defs): (Option<Res<Course>>, Res<Defs>),
    progress: Option<Res<RaceProgress>>,
    mut readouts: Query<(&Readout, &mut BitmapText), Without<MedalFigure>>,
    mut medals: Query<(&MedalFigure, &mut BitmapText), Without<Readout>>,
) {
    let (Some(c), Some(p)) = (course, progress) else { return };
    // The best medal the time so far still earns (0xa1040).
    if let Some(d) = defs.get(c.n) {
        let lit = d.medal(p.finished.unwrap_or(p.clock));
        for (MedalFigure(i), mut t) in &mut medals {
            let col = if lit == Some(2 - i) { FIGURE } else { LABEL };
            if t.color != col {
                t.color = col;
            }
            set_text(&mut t, &fmt_time(d.records[2 - i]));
        }
    }
    for (r, mut t) in &mut readouts {
        let text = match r {
            Readout::Time => fmt_time(p.finished.unwrap_or(p.clock)),
            Readout::Gates if c.gates.is_empty() => "NO COURSE ON THE DISC".to_string(),
            Readout::Gates => format!("GATES {}/{}", c.next, c.gates.len()),
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
                let dir = (g.center() - at).with_y(0.0).normalize_or(Vec3::NEG_Z);
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
        let o = score(&mut c, &rules, d.get(1).unwrap(), 1, 14.5);
        assert_eq!((o.medal, o.skill_points, o.unlocked.as_deref()), (Some(0), 200, None));
        assert!(o.improved);
        assert!(is_open(&c, 2));
        // Bronze to gold climbs two tiers: twice the gold bonus.
        let o = score(&mut c, &rules, d.get(1).unwrap(), 1, 12.0);
        assert_eq!((o.medal, o.skill_points, o.unlocked.as_deref()), (Some(2), 2000, Some("Jason_Luburgh")));
        assert_eq!(c.skill_points, 2200);
        // No better medal, no bonus; the best time stays.
        let o = score(&mut c, &rules, d.get(1).unwrap(), 1, 12.5);
        assert_eq!((o.skill_points, o.best_time, o.unlocked, o.improved), (0, false, None, false));
        assert_eq!(c.challenges[&1], (12.0, Some(2)));
        // A first gold pays three times.
        let o = score(&mut c, &rules, d.get(2).unwrap(), 2, 10.0);
        assert_eq!(o.skill_points, 300);
    }

    fn world(locators: &[(&str, [f32; 3])]) -> World {
        let box_ = |(name, pos): &(&str, [f32; 3])| atv_formats::rom::Locator { name: name.to_string(), pos: *pos, scale: [10.0, 6.0, 2.0], rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] };
        World { fog_color: 0, materials: vec![], locators: locators.iter().map(box_).collect(), lights: vec![], instances: vec![], sprites: vec![], meshes: vec![] }
    }

    #[test]
    fn a_gate_is_its_boxs_far_face() {
        // A 10 x 6 x 2 box at game z 10: its +Z face is at game z 11, sim -11.
        let gates = course(&world(&[("checkpoint 100", [0.0, 0.0, 10.0])]), 1);
        assert_eq!(gates[0].center(), Vec3::new(0.0, 0.0, -11.0));
        let touched = |p: Vec3| gates[0].touched(&[p]);
        assert!(touched(Vec3::new(0.0, 0.0, -11.0)));
        assert!(touched(Vec3::new(4.9, 2.9, -11.5)));
        // The box's middle is a metre short of the face; past an edge
        // doesn't count either.
        assert!(!touched(Vec3::new(0.0, 0.0, -10.0)));
        assert!(!touched(Vec3::new(5.7, 0.0, -11.0)));
    }

    #[test]
    fn gates_in_order() {
        let w = world(&[("checkpoint 101", [0.0, 0.0, 20.0]), ("Checkpoint 100 ", [0.0, 0.0, 10.0]), ("checkpoint 102", [0.0, 0.0, 30.0]), ("checkpoint 102", [50.0, 0.0, 30.0]), ("checkpoint 200", [0.0, 0.0, 40.0])]);
        let mut c = Course::new(&w, 1);
        assert_eq!(c.gates.len(), 3);
        let at = |z: f32| [Vec3::new(0.0, 0.0, -z)];
        // The second gate doesn't count before the first.
        assert_eq!(c.step(&at(21.0)), None);
        assert_eq!(c.next, 0);
        assert_eq!(c.step(&at(11.0)).as_deref(), Some("GATE 1/3"));
        c.step(&at(21.0));
        // Either copy of a repeated number counts; the last gate finishes.
        assert_eq!(c.step(&[Vec3::new(50.0, 0.0, -31.0)]), None);
        assert!(c.done());
        c.reset();
        assert_eq!((c.next, c.failed), (0, false));
    }

    #[test]
    fn wraps_words() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
    }
}
