//! Tricks and Open Freestyle.
//!
//! Tricks come from the disc's `Setup/Tricks.xml`: each is a button (or a
//! pair of buttons) plus a stick direction, pulled while airborne. The rider
//! plays the trick's stunt animation; landing before its recovery part has
//! started is a bail. Holding the buttons at a trick's peak adds its hold
//! points. Scoring is the retail stunt manager's (0x75bf0): tricks,
//! flips, spins, wheelies, bicycles and endos and trick boxes string into
//! a combo that pays, times the number of different moves in it, once the
//! quad has sat on all four wheels for a moment; a bail or a crash loses
//! it. Tricks work in every mode; only freestyle shows the score.
//!
//! Open Freestyle is free roaming of a track against an optional time
//! limit, with Bronze/Silver/Gold targets from `Setup/Freestyle.xml`.
//! ATV Academy is the same on Stadium 2, with no limit or targets.
//! Besides tricks it scores flying through the stadiums' trick boxes
//! (`Setup/TrickBoxes.xml` and the `TBOXn` locators).
//!
//! Addresses here are the retail xbe's (`ATV.exe`'s minus 0x3ef440).

use std::sync::Arc;

use bevy::prelude::*;

use atv_formats::row::World;
use atv_formats::setup;
use atv_formats::GameSource;
use atv_sim::track as trk;
use atv_sim::{TrackCollision, Vehicle};

use crate::race::fmt_time;
use crate::rider::Clip;
use crate::hud::{FIGURE, LABEL};
use crate::ui::{set_text, BitmapText, FontId, UiAssets};
use crate::{AppState, Mode, Selection};

/// The time limits Open Freestyle offers, in the disc's record order
/// (`None` = unlimited).
pub const TIME_LIMITS: [Option<u32>; 7] = [Some(1), Some(3), Some(5), Some(10), Some(15), Some(30), None];

pub fn limit_label(limit: Option<u32>) -> String {
    match limit {
        Some(1) => "1 MINUTE".into(),
        Some(m) => format!("{m} MINUTES"),
        None => "UNLIMITED".into(),
    }
}

/// A trick's buttons, as a mask: the disc's D, A, B and E buttons in
/// order of the tricks' level (1 to 4), [`crate::controller::Action::TRICKS`] (X, Y, LB and
/// RB, or 1 to 4, unless remapped). The three two-button combos are
/// levels 5-7.
fn combo_mask(combo: &str) -> Option<u8> {
    Some(match combo.trim().to_ascii_lowercase().as_str() {
        "keydheld" => 0b0001,
        "keyaheld" => 0b0010,
        "keybheld" => 0b0100,
        "keyeheld" => 0b1000,
        // Combos rise with difficulty: 1+2, 2+3, 3+4.
        "keycombo1" => 0b0011,
        "keycombo3" => 0b0110,
        "keycombo2" => 0b1100,
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub struct TrickDef {
    /// "SEAT GRAB".
    pub name: String,
    /// Skill level (1 to 7) a rider needs to pull it (`mLevel`).
    pub level: u8,
    pub buttons: u8,
    pub dir: Dir,
    pub points: u32,
    pub hold_points: u32,
    /// Seconds for the whole trick, and for its recovery at the end.
    pub total: f32,
    pub end: f32,
    /// Stunt clip under `Animations/Stunts/Bac`.
    pub clip: String,
}

/// The highest skill level, at which every trick can be pulled.
pub const MAX_LEVEL: u8 = 7;

/// A ground trick held on two wheels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ground {
    Wheelie,
    /// On the left wheels, the right ones up.
    BicycleLeft,
    BicycleRight,
    Endo,
}

impl Ground {
    /// The call-out, the game's own strings ("Left Bicycle", 0x153; the
    /// `Endo` key reads "Stoppie").
    fn label(self) -> &'static str {
        match self {
            Ground::Wheelie => "WHEELIE",
            Ground::BicycleLeft => "LEFT BICYCLE",
            Ground::BicycleRight => "RIGHT BICYCLE",
            Ground::Endo => "STOPPIE",
        }
    }
}

/// How a ground trick pays (`Tricks.xml`): `score` points a second to
/// begin with, that rate growing by `accel` of itself a second, the
/// trick's total held to `max`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundRate {
    pub score: f32,
    pub accel: f32,
    pub max: f32,
}

/// The trick points that aren't air tricks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scoring {
    /// Wheelie, left and right bicycle, endo.
    pub ground: [GroundRate; 4],
    pub back_flip: u32,
    pub front_flip: u32,
    pub spin_cw: u32,
    pub spin_ccw: u32,
}

impl Scoring {
    /// The retail game's values.
    #[cfg(test)]
    pub const RETAIL: Scoring = Scoring {
        ground: [
            GroundRate { score: 300.0, accel: 0.0001, max: 9999.0 },
            GroundRate { score: 400.0, accel: 0.001, max: 99999.0 },
            GroundRate { score: 400.0, accel: 0.001, max: 99999.0 },
            GroundRate { score: 300.0, accel: 0.0015, max: 99999.0 },
        ],
        back_flip: 400,
        front_flip: 600,
        spin_cw: 600,
        spin_ccw: 600,
    };

    fn rate(&self, g: Ground) -> GroundRate {
        self.ground[g as usize]
    }

    /// Reads `Tricks.xml`. The retail disc has `Wheelie`, `Bicycle_Left`,
    /// `Bicycle_Right` and `Endo` with `Score`, `Accel` and `Max` (0x732d0),
    /// and the flips' and spins' points. The prototype's only has
    /// `<Wheelie Wheelie="20"/>` and the like, read as 20 points a quarter
    /// second, and no flips: those keep 1000 a flip and 600 a spin.
    fn load(entries: &[setup::Entry]) -> Self {
        let find = |name: &str| entries.iter().find(|e| e.name == name);
        let proto = |name: &str| {
            let per_quarter = find(name).and_then(|e| e.num(name)).unwrap_or(20.0);
            GroundRate { score: per_quarter * 4.0, accel: 0.0, max: f32::MAX }
        };
        let rate = |name: &str, old: &str| match find(name).filter(|e| e.get("Score").is_some()) {
            Some(e) => GroundRate {
                score: e.num("Score").unwrap_or(0.0),
                accel: e.num("Accel").unwrap_or(0.0),
                max: e.num("Max").unwrap_or(f32::MAX),
            },
            None => proto(old),
        };
        let points = |name: &str, default: u32| find(name).and_then(|e| e.num("Score")).map_or(default, |v| v as u32);
        Self {
            ground: [rate("Wheelie", "Wheelie"), rate("Bicycle_Left", "Bicycle"), rate("Bicycle_Right", "Bicycle"), rate("Endo", "Endo")],
            back_flip: points("Backward_Flip", 1000),
            front_flip: points("Forward_Flip", 1000),
            spin_cw: points("Clockwise_Spin", 600),
            spin_ccw: points("Counter_Clockwise_Spin", 600),
        }
    }
}

pub fn load_tricks(game: &GameSource) -> (Vec<TrickDef>, Scoring) {
    let xml = game.read("/Setup/Tricks.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let stems = crate::rider::stunt_stems(game);
    let entries = setup::entries(&xml);
    let scoring = Scoring::load(&entries);
    let tricks = entries
        .iter()
        .filter_map(|e| {
            let combo = e.get("mControlCombo")?;
            let (buttons, stick) = combo.split_once(',')?;
            let dir = match stick.trim().to_ascii_lowercase().as_str() {
                "stickup" => Dir::Up,
                "stickdown" => Dir::Down,
                "stickleft" => Dir::Left,
                "stickright" => Dir::Right,
                _ => return None,
            };
            let clip = stunt_clip(e.get("mMainClip").unwrap_or(""), &stems)?;
            Some(TrickDef {
                name: e.name.replace('_', " ").to_uppercase(),
                level: e.num("mLevel").map_or(1, |l| l.clamp(1.0, MAX_LEVEL as f32) as u8),
                buttons: combo_mask(buttons)?,
                dir,
                points: e.num("mScorePoints").unwrap_or(100.0) as u32,
                hold_points: e.num("mHoldPoints").unwrap_or(0.0) as u32,
                total: e.num("mTotalTrickTimeSec").unwrap_or(1.0).max(0.2),
                end: e.num("mTrickEndTimeSec").unwrap_or(0.3).max(0.0),
                clip,
            })
        })
        .collect();
    (tricks, scoring)
}

/// The stunt file for a Tricks.xml clip name ("Anim Stunt Seat Grab" ->
/// `seat_grab`). Most match once spaces and case are dropped; the rest are
/// named differently on the disc.
fn stunt_clip(main: &str, stems: &[String]) -> Option<String> {
    let squash = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let key = squash(main.trim().strip_prefix("Anim Stunt").unwrap_or(main));
    let key = match key.as_str() {
        "scarecrow" => "crucifixtion".to_string(),
        "scissors" => "legsplt".to_string(),
        "seatgrabhstand" => "seatgrabhandstand".to_string(),
        "spinbike" => "bikespin".to_string(),
        "reversespin" => "reversespingrab".to_string(),
        "splitx" => "splitsx".to_string(),
        "yogi" => "y".to_string(),
        _ => key,
    };
    stems.iter().find(|s| squash(s) == key).cloned()
}

/// A time limit's element in `Setup/Freestyle.xml`'s `OpenFSRecords`.
pub fn limit_tag(limit: Option<u32>) -> String {
    match limit {
        Some(1) => "Time1Minute".to_string(),
        Some(m) => format!("Time{m}Minutes"),
        None => "TimeUnlimited".to_string(),
    }
}

pub const MEDALS: [&str; 3] = ["BRONZE", "SILVER", "GOLD"];

/// A trick box: fly into any of its volumes for its points (or, linked to
/// another box, into this one and then that one).
#[derive(Clone, Debug)]
struct TrickBox {
    name: String,
    points: u32,
    linked: Option<usize>,
    /// Only pays in a combo that's under way (`NeedsTrick`).
    needs_trick: bool,
    volumes: Vec<(Vec3, Quat, Vec3)>,
}

/// The track's trick boxes (0x72ab0): `TBOXn` locators named under the
/// track in `TrickBoxes.xml`. Boxes with a `GroundTrick` child (Stadium 2's
/// wheelie, endo and bicycle zones, with Bronze/Silver/Gold targets) are
/// for the retail lessons and never pay here (the lookup at 0x72a20 skips
/// them), and nor do boxes worth nothing.
fn load_boxes(game: &GameSource, track: &str, world: &World) -> Vec<TrickBox> {
    let xml = game.read("/Setup/TrickBoxes.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let Some(block) = xml.split(&format!("<{track}>")).nth(1).and_then(|b| b.split(&format!("</{track}>")).next()) else { return Vec::new() };
    let attr = |tag: &str, key: &str| tag.split(&format!("{key}=\"")).nth(1).and_then(|r| r.split('"').next()).map(str::trim).map(str::to_owned);
    let mut defs: Vec<(String, String, u32, String, bool)> = Vec::new();
    let tags: Vec<&str> = block.split('<').skip(1).collect();
    for (i, tag) in tags.iter().enumerate() {
        let Some(id) = tag.split_whitespace().next().filter(|t| t.starts_with("TBOX")) else { continue };
        let points: u32 = attr(tag, "Points").and_then(|p| p.parse().ok()).unwrap_or(0);
        let ground = !tag.trim_end().ends_with("/>") && tags.get(i + 1).is_some_and(|t| t.starts_with("GroundTrick"));
        if points == 0 || ground {
            continue;
        }
        let needs_trick = attr(tag, "NeedsTrick").is_some_and(|n| n != "0" && !n.is_empty());
        defs.push((id.to_string(), attr(tag, "Name").unwrap_or_default().replace('_', " ").to_uppercase(), points, attr(tag, "LinkedBox").unwrap_or_default(), needs_trick));
    }
    let volumes = |id: &str| -> Vec<(Vec3, Quat, Vec3)> {
        world
            .locators
            .iter()
            .filter(|l| l.name.trim().eq_ignore_ascii_case(id))
            .map(|l| (trk::to_sim(l.pos), trk::rotation_to_sim(&l.rotation), Vec3::from(l.scale).abs()))
            .collect()
    };
    let placed: Vec<_> = defs.into_iter().map(|d| (volumes(&d.0), d)).filter(|(v, _)| !v.is_empty()).collect();
    placed
        .iter()
        .map(|(volumes, (_, name, points, link, needs_trick))| TrickBox {
            name: name.clone(),
            points: *points,
            linked: placed.iter().position(|(_, d)| d.0.eq_ignore_ascii_case(link)),
            needs_trick: *needs_trick,
            volumes: volumes.clone(),
        })
        .collect()
}

/// Places worth finding: checkpoints, respots and the stadiums' photo
/// boxes, merged when close together.
fn load_spots(world: &World) -> Vec<Vec3> {
    let mut spots: Vec<Vec3> = Vec::new();
    for l in &world.locators {
        let n = l.name.trim().to_ascii_lowercase();
        if !(n.starts_with("checkpoint") || n.starts_with("respot") || n.starts_with("photobox") || n == "startpoint") {
            continue;
        }
        let p = trk::to_sim(l.pos);
        if spots.iter().all(|s| s.distance(p) > 30.0) {
            spots.push(p);
        }
    }
    spots
}

const SPOT_RADIUS: f32 = 14.0;
/// Seconds airborne before trick buttons count (skips bumps), and before
/// touching down counts as a landing.
const TRICK_AIR: f32 = 0.3;
const LANDING_AIR: f32 = 0.2;
/// Buttons must be steady this long, so pressing a combo's two buttons a
/// frame apart doesn't start the one-button trick.
const COMBO_WINDOW: f32 = 0.06;
/// All four wheels down this long ends a combo (0x75bf0).
const SETTLE: f32 = 0.1;
/// A ground trick pays once held longer than this, seconds.
const GROUND_MIN: f32 = 1.0;
/// Below this speed (m/s) a wheelie or bicycle doesn't start, and one
/// under way stops gaining points; an endo needs no speed.
const GROUND_SPEED: f32 = 2.0;
/// Changing straight from one ground trick to another starts the new one
/// this far in, seconds.
const GROUND_SWITCH: f32 = 0.5;
/// The Big Air bonus: points, for coming within `BIG_AIR_MARGIN` metres of
/// the track's Biggest Air record, then not again for `BIG_AIR_WAIT`
/// seconds (0x75bf0, the stunt manager's `Big_Air` box at +0x1f4).
const BIG_AIR_POINTS: u32 = 100;
const BIG_AIR_MARGIN: f32 = 0.5;
const BIG_AIR_WAIT: f32 = 7.0;
/// How far down the ground is looked for under a flying quad.
const GROUND_REACH: f32 = 50.0;

/// Points as the original adds them to a combo: rounded up to fives
/// (0x74260, 0x74380).
fn fives(points: u32) -> u32 {
    points.div_ceil(5) * 5
}

#[derive(Clone, Debug)]
pub struct ActiveTrick {
    pub def: usize,
    /// Seconds into the trick (paused at the peak while held).
    pub t: f32,
    /// The buttons were let go, so the trick can't be held any more.
    released: bool,
}

impl ActiveTrick {
    /// A trick another game says is `t` seconds in.
    pub fn shown(def: usize, t: f32) -> Self {
        Self { def, t, released: true }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PopupKind {
    Trick,
    Total,
    Bail,
    Bonus,
}

#[derive(Clone, Debug)]
pub struct Popup {
    pub text: String,
    pub kind: PopupKind,
    pub at: f32,
}

/// One kind of move, for the combo's multiplier: each different one
/// counts once (the flags at +0x10 to +0xa4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Move {
    Trick(usize),
    Ground(Ground),
    /// Forward or not.
    Flip(bool),
    /// Counter-clockwise or not.
    Spin(bool),
}

/// The moves strung together since the quad last sat on all four wheels
/// (+0x124 to +0x130). It pays when the quad has been down on all four
/// for [`SETTLE`]: its points times the number of different moves, every
/// trick box (and Big Air) counting as a different one (0x74700). A crash
/// loses it (0x745c0).
#[derive(Clone, Debug, Default)]
struct Combo {
    points: u32,
    /// Moves, repeats and all.
    moves: u32,
    kinds: Vec<Move>,
    boxes: u32,
    /// Something is under way (+0x12c): a trick, a ground trick past its
    /// first second, a flip, a box.
    on: bool,
    /// Seconds tricks have been held at their peak (+0xd0). It runs on
    /// across the combo's tricks, each paying `mHoldPoints` a second of
    /// it (0x74e90).
    held: f32,
}

impl Combo {
    fn add(&mut self, kind: Option<Move>, points: u32) {
        self.points += points;
        self.moves += 1;
        match kind {
            Some(k) if !self.kinds.contains(&k) => self.kinds.push(k),
            Some(_) => {}
            None => self.boxes += 1,
        }
        self.on = true;
    }

    fn multiplier(&self) -> u32 {
        self.kinds.len() as u32 + self.boxes
    }
}

/// Rotation through a jump about two of the quad's axes, as the original
/// tracks flips and spins (0x73b90): each tick the angles the two other
/// axes turned through, signed by which way, are added up while each is
/// under half a turn.
#[derive(Clone, Copy, Debug, Default)]
struct Turning {
    a: f32,
    b: f32,
    /// A flip just paid: wait for this one to come round (1.9 turns
    /// forward of where the jump started, backwards ones only) before the
    /// next can.
    held: bool,
}

impl Turning {
    /// Spins (about up), compared from the right and forward axes; the
    /// sign is + turning right.
    fn spin(&mut self, was: Quat, now: Quat) {
        let (r0, f0) = (was * Vec3::X, was * Vec3::NEG_Z);
        let (r1, f1) = (now * Vec3::X, now * Vec3::NEG_Z);
        self.add(angle(r0, r1), angle(f0, f1), f1.dot(r0) < 0.0);
    }

    /// Flips (about right), from the forward and up axes; + nose up.
    fn flip(&mut self, was: Quat, now: Quat) {
        let (f0, u0) = (was * Vec3::NEG_Z, was * Vec3::Y);
        let (f1, u1) = (now * Vec3::NEG_Z, now * Vec3::Y);
        self.add(angle(f0, f1), angle(u0, u1), f1.dot(u0) < 0.0);
        let round = 1.9 * std::f32::consts::PI;
        if self.held && self.a > round && self.b > round {
            *self = Turning::default();
        }
    }

    fn add(&mut self, a: f32, b: f32, negative: bool) {
        let (a, b) = if negative { (-a, -b) } else { (a, b) };
        if a < std::f32::consts::PI && b < std::f32::consts::PI {
            self.a += a;
            self.b += b;
        }
    }

    /// Turned far enough to pay (0x73e10): 1.1 turns for a flip, 1.5 for a
    /// spin.
    fn done(&self, need: f32) -> bool {
        !self.held && self.a.abs() > need && self.b.abs() > need
    }
}

fn angle(a: Vec3, b: Vec3) -> f32 {
    a.dot(b).clamp(-1.0, 1.0).acos()
}

/// Half-turns a flip and a spin need (doubles at 0x5aae38 and 0x5aae30).
const FLIP_NEED: f32 = 1.1 * std::f32::consts::PI;
const SPIN_NEED: f32 = 1.5 * std::f32::consts::PI;

/// What the wheels say about a ground trick this tick (0x74030).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pose {
    Trick(Ground),
    /// On the ground, not in a trick.
    Down,
    Air,
    /// One wheel down: no change.
    Same,
}

/// The ground trick being held, or the last one (+0xac to +0xbc).
#[derive(Clone, Copy, Debug, Default)]
struct GroundRun {
    kind: Option<Ground>,
    secs: f32,
    /// Points a second, and points so far.
    rate: f32,
    points: f32,
}

/// The trick boxes this jump (+0x1ec, +0x1f0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BoxRun {
    #[default]
    None,
    /// Flown into this box; waiting for its other end, or a trick.
    In(usize),
    Paid,
}

#[derive(Resource, Clone)]
pub struct Session {
    pub freestyle: bool,
    /// Seconds, or unlimited.
    pub limit: Option<f32>,
    pub medals: Option<[u32; 3]>,
    pub tricks: Vec<TrickDef>,
    /// The rider's skill level: tricks above it don't start, as in the
    /// original (0x18439), which takes it from the profile's skill points
    /// or, for another player's rider, its `mSkillRating` (0x25590).
    pub level: u8,
    scoring: Scoring,
    boxes: Vec<TrickBox>,
    pub spots: Vec<(Vec3, bool)>,
    pub score: u32,
    /// Points banked since the speed boost last took them.
    banked: u32,
    pub active: Option<ActiveTrick>,
    combo: Combo,
    ground: GroundRun,
    /// Seconds all four wheels have been down, up to [`SETTLE`].
    settled: f32,
    /// In the air since all wheels left the ground, and the turning since.
    flying: bool,
    flips: Turning,
    spins: Turning,
    boxing: BoxRun,
    /// The track's Biggest Air record, for the Big Air bonus (none: no
    /// bonus, as in the challenges and training). A higher jump raises it.
    pub air_record: Option<f32>,
    /// The ground's distance below when last in the air, and the most this
    /// jump.
    height: f32,
    peak: f32,
    big_air_wait: f32,
    pub popups: Vec<Popup>,
    pub time: f32,
    pub best_trick: Option<(String, u32)>,
    /// The best combo's points.
    pub best_jump: u32,
    pub landed: u32,
    pub longest_air: f32,
    /// The longest wheelie and bicycle that scored, seconds, for the track
    /// records (the stunt manager's +0xbc and +0xc0, 0x614f0).
    pub longest_wheelie: f32,
    pub longest_bicycle: f32,
    /// The longest endo, seconds (+0xc4), for the freestyle results.
    pub longest_endo: f32,
    /// The combo with the most moves, repeats and all, and what it paid:
    /// the results' Best Trick Combo and Best Trick (+0xe0 and +0xe8, kept
    /// when a combo pays, 0x74700).
    pub best_combo: (u32, u32),
    /// Ended from the pause menu (unlimited sessions).
    pub ended: bool,
    mask: u8,
    steady: f32,
    armed: bool,
    last_air: f32,
    last_rot: Quat,
    /// The flip this jump paid (forward or not), for the lessons.
    flipped: Option<bool>,
}

impl Session {
    /// `medals`: Open Freestyle's Bronze, Silver and Gold targets, the
    /// time limit's medal records (see [`crate::records`]).
    pub fn new(game: &GameSource, sel: &Selection, world: &World, medals: Option<[u32; 3]>) -> Self {
        let (tricks, scoring) = load_tricks(game);
        // ATV Academy scores like unlimited Open Freestyle, without medals
        // (0xa0e60 leaves its targets unset for game type 9).
        let training = sel.mode == Mode::Training;
        let freestyle = sel.mode == Mode::Freestyle || training;
        let mut s = Self::with(tricks, scoring, if freestyle { load_boxes(game, &sel.track, world) } else { Vec::new() });
        s.freestyle = freestyle;
        if freestyle {
            debug!("trick boxes: {:?}", s.boxes.iter().map(|b| (&b.name, b.points, b.linked, b.needs_trick, b.volumes.len())).collect::<Vec<_>>());
            s.limit = sel.time_limit.filter(|_| !training).map(|m| m as f32 * 60.0);
            s.medals = medals.filter(|_| !training);
            s.spots = load_spots(world).into_iter().map(|p| (p, false)).collect();
        }
        s
    }

    fn with(tricks: Vec<TrickDef>, scoring: Scoring, boxes: Vec<TrickBox>) -> Self {
        Self {
            freestyle: false,
            limit: None,
            medals: None,
            tricks,
            level: MAX_LEVEL,
            scoring,
            boxes,
            spots: Vec::new(),
            score: 0,
            banked: 0,
            active: None,
            combo: Combo::default(),
            ground: GroundRun::default(),
            settled: 0.0,
            flying: false,
            flips: Turning::default(),
            spins: Turning::default(),
            boxing: BoxRun::None,
            air_record: None,
            height: 0.0,
            peak: 0.0,
            big_air_wait: 0.0,
            popups: Vec::new(),
            time: 0.0,
            best_trick: None,
            best_jump: 0,
            landed: 0,
            longest_air: 0.0,
            longest_wheelie: 0.0,
            longest_bicycle: 0.0,
            longest_endo: 0.0,
            best_combo: (0, 0),
            ended: false,
            mask: 0,
            steady: 0.0,
            armed: true,
            last_air: 0.0,
            last_rot: Quat::IDENTITY,
            flipped: None,
        }
    }

    #[cfg(test)]
    pub fn test_with(tricks: Vec<TrickDef>) -> Self {
        Self::with(tricks, Scoring::RETAIL, Vec::new())
    }

    pub fn time_left(&self, clock: f32) -> Option<f32> {
        self.limit.map(|l| (l - clock).max(0.0))
    }

    /// Time is up (or the session was ended).
    /// The move under way, as the stunt manager names its current trick
    /// (+0x74, which the retail lessons read): the ground trick being
    /// held, the air trick, or the flip this jump paid.
    pub fn current_move(&self) -> Option<&str> {
        if let Some(g) = self.ground.kind {
            return Some(g.label());
        }
        if let Some(a) = &self.active {
            return self.tricks.get(a.def).map(|t| t.name.as_str());
        }
        self.flipped.map(|forward| if forward { "FRONTFLIP" } else { "BACKFLIP" })
    }

    pub fn over(&self, clock: f32) -> bool {
        self.ended || self.time_left(clock) == Some(0.0)
    }

    /// The nearest spot not found yet.
    pub fn next_spot(&self, from: Vec3) -> Option<Vec3> {
        self.spots.iter().filter(|s| !s.1).map(|s| s.0).min_by(|a, b| a.distance_squared(from).total_cmp(&b.distance_squared(from)))
    }

    pub fn spots_found(&self) -> usize {
        self.spots.iter().filter(|s| s.1).count()
    }

    /// The medal the score has earned (0 = bronze): more than its record
    /// (0x18090).
    pub fn medal(&self) -> Option<usize> {
        let m = self.medals?;
        (0..3).rev().find(|&i| self.score > m[i])
    }

    pub fn popup(&mut self, text: impl Into<String>, kind: PopupKind) {
        let text = text.into();
        debug!("{:.1}s {kind:?}: {text}", self.time);
        self.popups.push(Popup { text, kind, at: self.time });
        if self.popups.len() > 6 {
            self.popups.remove(0);
        }
    }

    /// Banks points: freestyle's score, and in a race the rider's trick
    /// points for a career.
    fn award(&mut self, points: u32) {
        self.score += points;
        self.banked += points;
    }

    /// Points banked since the last call, for the speed boost.
    pub fn take_banked(&mut self) -> u32 {
        std::mem::take(&mut self.banked)
    }

    /// The trick these buttons and this direction pull, if the rider's
    /// skill level allows it.
    pub fn trick_for(&self, buttons: u8, dir: Dir) -> Option<usize> {
        self.tricks.iter().position(|t| t.buttons == buttons && t.dir == dir && t.level <= self.level)
    }

    /// Starts trick `def` now, as a computer rider does (0x63330): it runs
    /// its full length without being held at the peak.
    pub fn start_trick(&mut self, def: usize) {
        if self.active.is_none() && self.tricks.get(def).is_some_and(|t| t.level <= self.level) {
            self.active = Some(ActiveTrick { def, t: 0.0, released: true });
            self.combo.on = true;
        }
    }

    /// One physics step, as the stunt manager's (0x75bf0). `live`: the
    /// clock is running (tricks are held during the countdown and after
    /// the end); `crashed`: the rider is off the quad.
    pub fn step(&mut self, v: &Vehicle, world: &TrackCollision, (buttons, dir): (u8, Option<Dir>), dt: f32, live: bool, crashed: bool) {
        self.time += dt;
        self.big_air_wait = (self.big_air_wait - dt).max(0.0);
        let down = v.wheels.iter().filter(|w| w.contact).count();
        let all_down = !v.wheels.is_empty() && down == v.wheels.len();
        self.settled = if all_down { (self.settled + dt).min(SETTLE) } else { 0.0 };
        if crashed {
            self.crash();
            self.last_rot = v.rotation;
            self.last_air = 0.0;
            return;
        }

        // Jumps: take-off with no wheel down, landing at the first. In the
        // air the ground's distance below is measured (0x74220).
        if !self.flying && down == 0 {
            self.flying = true;
            self.flips = Turning::default();
            self.spins = Turning::default();
            self.peak = 0.0;
            self.flipped = None;
        } else if self.flying && down > 0 {
            self.flying = false;
        }
        if self.flying {
            if let Some(h) = world.raycast(v.position, Vec3::NEG_Y, GROUND_REACH) {
                self.height = h.t;
            }
            self.peak = self.peak.max(self.height);
        }
        if all_down && let Some(r) = self.air_record.as_mut() {
            *r = r.max(self.peak);
        }
        if live && self.air_record.is_some_and(|r| self.big_air_wait == 0.0 && self.height >= r - BIG_AIR_MARGIN) {
            self.big_air_wait = BIG_AIR_WAIT;
            self.pay_box("BIG AIR".into(), BIG_AIR_POINTS);
        }

        // Flips and spins (0x74b90): paid first, then this tick's turn.
        if self.flying {
            if self.flips.done(FLIP_NEED) {
                let forward = self.flips.a < 0.0;
                let pts = if forward { self.scoring.front_flip } else { self.scoring.back_flip };
                self.combo.add(Some(Move::Flip(forward)), pts);
                self.popup(format!("{}  {pts}", if forward { "FRONTFLIP" } else { "BACKFLIP" }), PopupKind::Trick);
                self.flips.held = true;
                self.flipped = Some(forward);
            }
            if self.spins.done(SPIN_NEED) {
                let ccw = self.spins.a < 0.0;
                let pts = if ccw { self.scoring.spin_ccw } else { self.scoring.spin_cw };
                self.combo.add(Some(Move::Spin(ccw)), pts);
                self.popup(format!("360  {pts}"), PopupKind::Trick);
                self.spins = Turning::default();
            }
            self.flips.flip(self.last_rot, v.rotation);
            self.spins.spin(self.last_rot, v.rotation);
        }
        self.last_rot = v.rotation;

        // Buttons: wait for them to settle, and for a release between tricks.
        if buttons != self.mask {
            self.mask = buttons;
            self.steady = 0.0;
        } else {
            self.steady += dt;
        }
        if buttons == 0 {
            self.armed = true;
        }
        let air = v.air_time;
        if live && air > TRICK_AIR && self.active.is_none() && self.armed && buttons != 0 && self.steady >= COMBO_WINDOW
            && let Some(dir) = dir
            && let Some(def) = self.trick_for(buttons, dir)
        {
            self.active = Some(ActiveTrick { def, t: 0.0, released: false });
            self.combo.on = true;
            self.armed = false;
        }
        if let Some(a) = self.active.as_mut() {
            let def = &self.tricks[a.def];
            let peak = def.total * 0.5;
            if buttons != def.buttons {
                a.released = true;
            }
            if !a.released && a.t >= peak && air > 0.0 {
                a.t = peak;
                self.combo.held += dt;
            } else {
                a.t += dt;
            }
            if a.t >= def.total {
                let done = self.active.take().unwrap();
                self.finish_trick(done);
            }
        }

        // Landing: still mid-trick is a bail.
        if self.last_air > LANDING_AIR && down > 0 {
            self.longest_air = self.longest_air.max(self.last_air);
            if let Some(a) = self.active.take() {
                let def = &self.tricks[a.def];
                if a.t < def.total - def.end {
                    self.popup("BAILED", PopupKind::Bail);
                    self.drop_combo();
                } else {
                    self.finish_trick(a);
                }
            }
        }
        self.last_air = air;

        if live {
            let pose = pose(v, down);
            self.ground_tricks(pose, v.velocity.length(), dt);
            self.trick_boxes(v.position, down);
            self.find_spots(v.position);
        }

        // All four wheels down a moment: the combo pays.
        if self.combo.on && self.settled >= SETTLE {
            self.complete();
        }
    }

    fn finish_trick(&mut self, a: ActiveTrick) {
        let def = &self.tricks[a.def];
        let pts = fives(def.points + (def.hold_points as f32 * self.combo.held) as u32);
        let name = def.name.clone();
        if self.best_trick.as_ref().is_none_or(|b| pts > b.1) {
            self.best_trick = Some((name.clone(), pts));
        }
        self.landed += 1;
        self.combo.add(Some(Move::Trick(a.def)), pts);
        self.popup(format!("{name}  {pts}"), PopupKind::Trick);
    }

    /// A trick box (or Big Air) paid: points into the combo, and a
    /// different move every time (0x75020).
    fn pay_box(&mut self, name: String, points: u32) {
        self.combo.add(None, points);
        self.popup(format!("{name}  {points}"), PopupKind::Bonus);
    }

    /// The combo pays (0x74700).
    fn complete(&mut self) {
        let combo = std::mem::take(&mut self.combo);
        let total = combo.points * combo.multiplier();
        if combo.moves > self.best_combo.0 {
            self.best_combo = (combo.moves, total);
        }
        self.best_jump = self.best_jump.max(total);
        if total > 0 {
            let multiplier = combo.multiplier();
            if multiplier > 1 {
                self.popup(format!("{multiplier}X COMBO  {total}"), PopupKind::Total);
            } else if combo.moves > 1 {
                self.popup(format!("COMPLETE  {total}"), PopupKind::Total);
            }
            self.award(total);
        }
        self.ground = GroundRun::default();
        self.boxing = BoxRun::None;
    }

    /// The combo is lost: a bail or a crash (0x745c0, "FAILED").
    fn drop_combo(&mut self) {
        self.combo = Combo::default();
        self.active = None;
        self.ground = GroundRun::default();
        self.flips = Turning::default();
        self.spins = Turning::default();
    }

    fn crash(&mut self) {
        if self.combo.on {
            self.popup("FAILED", PopupKind::Bail);
        }
        self.drop_combo();
        self.flying = false;
        self.boxing = BoxRun::None;
    }

    /// Wheelies, bicycles and endos (0x75640). A trick starts when the
    /// quad lifts two wheels on one end or side (at walking pace or more,
    /// but for an endo), and gains points as it's held: its rate a second,
    /// the rate growing by its `Accel`, the total held to its `Max`. Held
    /// past a second it pays into the combo when it ends: back on all four
    /// wheels for a moment, into another ground trick, or in the air with
    /// a trick under way. Flying off a ramp in one only pauses it; landing
    /// back in the same trick carries on with its points so far.
    fn ground_tricks(&mut self, pose: Pose, speed: f32, dt: f32) {
        let prev = self.ground.kind;
        // Back on all four only counts once settled.
        let now = match pose {
            Pose::Same => prev.map_or(Pose::Down, Pose::Trick),
            Pose::Down if self.settled < SETTLE => prev.map_or(Pose::Down, Pose::Trick),
            p => p,
        };
        match now {
            Pose::Trick(g) => match prev {
                None => {
                    if speed >= GROUND_SPEED || g == Ground::Endo {
                        self.start_ground(g, dt, 0.0);
                    }
                }
                Some(p) if p == g => {
                    if speed >= GROUND_SPEED || g == Ground::Endo {
                        self.hold_ground(dt);
                    }
                }
                Some(p) => {
                    if self.ground.secs > GROUND_MIN {
                        self.end_ground(p);
                        // The new one starts half a second in, its points
                        // going on from the last one's.
                        let points = self.ground.points;
                        self.start_ground(g, GROUND_SWITCH, points);
                    } else {
                        self.start_ground(g, dt, 0.0);
                    }
                }
            },
            Pose::Down => {
                if let Some(p) = prev {
                    self.end_ground(p);
                    self.ground.kind = None;
                }
            }
            Pose::Air => {
                if let Some(p) = prev.filter(|_| self.active.is_some()) {
                    self.end_ground(p);
                }
            }
            Pose::Same => {}
        }
    }

    fn start_ground(&mut self, g: Ground, secs: f32, points: f32) {
        self.ground = GroundRun { kind: Some(g), secs, rate: self.scoring.rate(g).score, points };
    }

    fn hold_ground(&mut self, dt: f32) {
        let Some(g) = self.ground.kind else { return };
        let r = self.scoring.rate(g);
        self.ground.secs += dt;
        self.ground.rate *= 1.0 + r.accel * dt;
        self.ground.points += self.ground.rate * dt;
        if self.ground.secs > GROUND_MIN {
            self.ground.points = self.ground.points.min(r.max);
            self.combo.on = true;
        }
    }

    /// The ground trick ends: held over a second, it pays into the combo.
    fn end_ground(&mut self, g: Ground) {
        let secs = std::mem::take(&mut self.ground.secs);
        if secs <= GROUND_MIN {
            return;
        }
        self.ground.points = self.ground.points.min(self.scoring.rate(g).max);
        let pts = fives(self.ground.points as u32);
        match g {
            Ground::Wheelie => self.longest_wheelie = self.longest_wheelie.max(secs),
            Ground::BicycleLeft | Ground::BicycleRight => self.longest_bicycle = self.longest_bicycle.max(secs),
            Ground::Endo => self.longest_endo = self.longest_endo.max(secs),
        }
        self.combo.add(Some(Move::Ground(g)), pts);
        self.popup(format!("{} {secs:.1}S  {pts}", g.label()), PopupKind::Bonus);
    }

    /// Trick boxes (0x75360): the first box flown into in a jump pays, at
    /// once or, if it needs a trick, once the combo has one. A linked box
    /// pays when its other end comes next, so each end of a gap pays its
    /// own points for the way it was jumped.
    fn trick_boxes(&mut self, at: Vec3, down: usize) {
        if down > 0 {
            self.boxing = BoxRun::None;
            return;
        }
        let inside = |b: &TrickBox| {
            b.volumes.iter().any(|(c, r, h)| {
                let l = r.inverse() * (at - *c);
                l.x.abs() <= h.x && l.y.abs() <= h.y && l.z.abs() <= h.z
            })
        };
        let here = self.boxes.iter().position(inside);
        let b = match self.boxing {
            BoxRun::Paid => return,
            BoxRun::None => match here {
                Some(b) => {
                    self.boxing = BoxRun::In(b);
                    b
                }
                None => return,
            },
            BoxRun::In(b) => b,
        };
        let tb = &self.boxes[b];
        if let Some(l) = tb.linked {
            match here {
                Some(h) if h == l => {}
                Some(h) if h != b => {
                    self.boxing = BoxRun::None;
                    return;
                }
                _ => return,
            }
        }
        if tb.needs_trick && !self.combo.on {
            return;
        }
        let (name, points) = (tb.name.clone(), tb.points);
        self.boxing = BoxRun::Paid;
        self.pay_box(name, points);
    }

    fn find_spots(&mut self, at: Vec3) {
        if let Some(i) = self.spots.iter().position(|(p, found)| !found && (*p - at).with_y(0.0).length() < SPOT_RADIUS) {
            self.spots[i].1 = true;
        }
    }
}

/// The ground-trick pose from the wheels (0x74030): two wheels down at the
/// back a wheelie, at the front an endo, on one side a bicycle. With none
/// down the quad is in the air; one changes nothing; any other way it's on
/// the ground.
fn pose(v: &Vehicle, down: usize) -> Pose {
    if v.wheels.len() != 4 {
        return Pose::Down;
    }
    let touching = |front: bool, left: bool| {
        v.wheels.iter().zip(&v.params.wheels).any(|(w, p)| w.contact && (p.offset[2] < 0.0) == front && (p.offset[0] < 0.0) == left)
    };
    let (fl, fr, rl, rr) = (touching(true, true), touching(true, false), touching(false, true), touching(false, false));
    match (fl, fr, rl, rr) {
        (false, false, true, true) => Pose::Trick(Ground::Wheelie),
        (true, true, false, false) => Pose::Trick(Ground::Endo),
        (true, false, true, false) => Pose::Trick(Ground::BicycleLeft),
        (false, true, false, true) => Pose::Trick(Ground::BicycleRight),
        _ => match down {
            0 => Pose::Air,
            1 => Pose::Same,
            _ => Pose::Down,
        },
    }
}

/// Each trick's stunt clip, loaded with the race.
#[derive(Resource, Default)]
pub struct StuntClips(pub Vec<Option<Arc<Clip>>>);

impl StuntClips {
    pub fn load(game: &GameSource, tricks: &[TrickDef]) -> Self {
        Self(
            tricks
                .iter()
                .map(|t| match Clip::load_from(game, &format!("/Animations/Stunts/Bac/{}.bac", t.clip)) {
                    Ok(c) => Some(Arc::new(c)),
                    Err(e) => {
                        warn!("stunt {}: {e:#}", t.clip);
                        None
                    }
                })
                .collect(),
        )
    }
}

pub struct FreestylePlugin;

impl Plugin for FreestylePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (spawn_hud.run_if(resource_added::<Session>), update_hud).chain().run_if(in_state(AppState::Race)));
    }
}

#[derive(Component)]
enum FsReadout {
    Score,
    Time,
}
/// A figure in the medal list, top left (0xa1930, in freestyle and the
/// challenges): row 0 Gold, 1 Silver, 2 Bronze, each beside its icon.
#[derive(Component)]
pub(crate) struct MedalFigure(pub usize);

/// The medal list's icons (`Frontend/SmallMedals`), Gold first.
const MEDAL_ICONS: [&str; 3] = ["GOLDMEDAL", "SILVERMEDAL", "BRONZEMEDAL"];

/// Spawns the medal list's three rows into a column.
pub(crate) fn spawn_medal_list(c: &mut ChildSpawnerCommands, ui: &UiAssets) {
    for (i, icon) in MEDAL_ICONS.into_iter().enumerate() {
        c.spawn(Node { align_items: AlignItems::Center, column_gap: px(8), ..default() }).with_children(|r| {
            let h = ui.small_medals.get(icon).rect.height().max(1.0);
            r.spawn(ui.small_medals.node(icon, 22.0 / h));
            r.spawn((BitmapText::new(FontId::Chrome, 15.0, "").color(LABEL), MedalFigure(i)));
        });
    }
}

/// The medal list as the original keeps it while a session runs (0x18090):
/// the Gold, Silver and Bronze records with each player's score put in the
/// first place it beats (more than the record), the best score first, the
/// records below moving down and the last dropping off. Gives each row's
/// score and the player in it.
pub(crate) fn live_medals(records: [u32; 3], scores: &[(usize, u32)]) -> [(u32, Option<usize>); 3] {
    let mut list = records.map(|r| (r, None));
    let mut order = scores.to_vec();
    order.sort_by_key(|&(player, score)| (std::cmp::Reverse(score), player));
    for (player, score) in order {
        if let Some(i) = list.iter().position(|&(r, _)| r < score) {
            list[i..].rotate_right(1);
            list[i] = (score, Some(player));
        }
    }
    list
}
#[derive(Component)]
struct PopupLine(usize);

/// The score on the HUD rolls up to the real one, as the original's
/// (0x9ff60, and the race panel's trick score at 0xa1510): each 1/30 s it
/// gains a random amount under 5% of the gap, or under 3 points when that
/// is more (0xaac50), and never passes it.
#[derive(Component)]
pub(crate) struct RollingScore {
    shown: u32,
    wait: f32,
    seed: u32,
}

impl RollingScore {
    const TICK: f32 = 1.0 / 30.0;

    pub(crate) fn new() -> Self {
        Self { shown: 0, wait: 0.0, seed: 1 }
    }

    pub(crate) fn step(&mut self, score: u32, dt: f32) -> u32 {
        self.wait += dt;
        while self.wait >= Self::TICK {
            self.wait -= Self::TICK;
            if self.shown >= score {
                break;
            }
            let range = ((score - self.shown) as f32 * 0.05).max(3.0);
            self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let unit = ((self.seed >> 8) & 0xffff) as f32 / 65536.0;
            self.shown += (unit * range) as u32;
        }
        // A restart drops the score; the shown one follows at once.
        self.shown = self.shown.min(score);
        self.shown
    }
}

const POPUP_LINES: usize = 4;
const POPUP_SECS: f32 = 2.2;

fn spawn_hud(mut commands: Commands, session: Res<Session>, sel: Res<Selection>, ui: Option<Res<UiAssets>>) {
    let academy = sel.mode == Mode::Training;
    commands
        .spawn((crate::ui::fullscreen(), crate::hud::HudRoot, DespawnOnExit(AppState::Race), Pickable::IGNORE))
        .with_children(|root| {
            if session.freestyle {
                let score_row = |c: &mut ChildSpawnerCommands| {
                    c.spawn(Node { align_items: AlignItems::FlexEnd, column_gap: px(6), ..default() }).with_children(|r| {
                        r.spawn(BitmapText::new(FontId::Small, 8.0, "SCORE").color(LABEL));
                        r.spawn((BitmapText::new(FontId::Chrome, 18.0, "0").color(FIGURE), FsReadout::Score, RollingScore::new()));
                    });
                };
                // Top left (0xa1930): ATV Academy's score, or the time
                // limit's medal records with the score among them.
                root.spawn(Node { position_type: PositionType::Absolute, left: px(24), top: px(18), flex_direction: FlexDirection::Column, row_gap: px(4), ..default() })
                    .with_children(|c| {
                        if academy {
                            score_row(c);
                        } else if let (Some(ui), Some(_)) = (ui.as_deref(), session.medals) {
                            spawn_medal_list(c, ui);
                        }
                    });
                // Top right (0x9fe50, 0xa0b40): the clock only with a time
                // limit, and the score but in ATV Academy.
                if session.limit.is_some() || !academy {
                    root.spawn(Node {
                        position_type: PositionType::Absolute,
                        right: px(24),
                        top: px(18),
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::FlexEnd,
                        row_gap: px(4),
                        ..default()
                    })
                    .with_children(|c| {
                        if session.limit.is_some() {
                            c.spawn(Node { align_items: AlignItems::FlexEnd, column_gap: px(6), ..default() }).with_children(|r| {
                                r.spawn(BitmapText::new(FontId::Small, 8.0, "TIME").color(LABEL));
                                r.spawn((BitmapText::new(FontId::Chrome, 18.0, "0:00.00").color(FIGURE), FsReadout::Time));
                            });
                        }
                        if !academy {
                            score_row(c);
                        }
                    });
                }
            }
            // Trick call-outs, above the rider.
            root.spawn(Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                bottom: percent(58),
                flex_direction: FlexDirection::ColumnReverse,
                align_items: AlignItems::Center,
                row_gap: px(4),
                ..default()
            })
            .with_children(|c| {
                for i in 0..POPUP_LINES {
                    c.spawn((BitmapText::new(FontId::Menu, if i == 0 { 18.0 } else { 14.0 }, ""), PopupLine(i)));
                }
            });
        });
}

#[allow(clippy::type_complexity)]
fn update_hud(
    session: Option<Res<Session>>,
    progress: Option<Res<crate::race::RaceProgress>>,
    opponents: Option<Res<crate::opponents::Opponents>>,
    time: Res<Time>,
    mut readouts: Query<(&FsReadout, &mut BitmapText, Option<&mut RollingScore>), (Without<PopupLine>, Without<MedalFigure>)>,
    mut medals: Query<(&MedalFigure, &mut BitmapText), (Without<FsReadout>, Without<PopupLine>)>,
    mut lines: Query<(&PopupLine, &mut BitmapText), (Without<FsReadout>, Without<MedalFigure>)>,
) {
    let (Some(s), Some(p)) = (session, progress) else { return };
    let mut shown = s.score;
    for (r, mut t, rolling) in &mut readouts {
        let text = match r {
            FsReadout::Score => {
                shown = rolling.map_or(s.score, |mut r| r.step(s.score, time.delta_secs()));
                shown.to_string()
            }
            FsReadout::Time => fmt_time(s.time_left(p.clock).unwrap_or(p.clock)),
        };
        set_text(&mut t, &text);
    }
    // The medal list (0xa1040, 0xa1510): the records with the players'
    // scores in the places they'd take, Freestyle Battle's other players'
    // too, player 1's row in the figures' colour and the rest in the
    // labels'. Their row shows the rolling score, but in Gold's place the
    // real one, as the original's does.
    if let Some(m) = s.medals {
        let mut scores = vec![(1, s.score)];
        scores.extend(opponents.iter().flat_map(|o| &o.racers).filter_map(|r| Some((r.human()?, r.session.as_ref()?.score))));
        let list = live_medals([m[2], m[1], m[0]], &scores);
        for (MedalFigure(i), mut t) in &mut medals {
            let (value, who) = list[*i];
            let mine = who == Some(1);
            let c = if mine { FIGURE } else { LABEL };
            if t.color != c {
                t.color = c;
            }
            set_text(&mut t, &if mine && *i > 0 { shown } else { value }.to_string());
        }
    }
    // Newest call-out at the bottom, older ones above, fading out.
    let recent: Vec<&Popup> = s.popups.iter().rev().filter(|p| s.time - p.at < POPUP_SECS).take(POPUP_LINES).collect();
    for (line, mut t) in &mut lines {
        match recent.get(line.0) {
            Some(p) => {
                set_text(&mut t, &p.text);
                let a = ((POPUP_SECS - (s.time - p.at)) / 0.4).clamp(0.0, 1.0);
                let c = match p.kind {
                    PopupKind::Trick => Color::srgba(1.0, 1.0, 1.0, a),
                    PopupKind::Total => Color::srgba(1.0, 0.85, 0.2, a),
                    PopupKind::Bail => Color::srgba(1.0, 0.3, 0.15, a),
                    PopupKind::Bonus => Color::srgba(0.6, 0.9, 1.0, a),
                };
                if t.color != c {
                    t.color = c;
                }
            }
            None => set_text(&mut t, ""),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_take_their_place_in_the_medal_list() {
        let records = [1450, 1200, 900];
        assert_eq!(live_medals(records, &[(1, 50)]), [(1450, None), (1200, None), (900, None)]);
        assert_eq!(live_medals(records, &[(1, 1300)]), [(1450, None), (1300, Some(1)), (1200, None)]);
        assert_eq!(live_medals(records, &[(1, 1450)]), [(1450, None), (1450, Some(1)), (1200, None)], "a record has to be beaten");
        // Freestyle Battle: the best score first, the next under it.
        assert_eq!(live_medals(records, &[(1, 1000), (2, 1500)]), [(1500, Some(2)), (1450, None), (1200, None)]);
        assert_eq!(live_medals(records, &[(2, 950), (1, 1000)]), [(1450, None), (1200, None), (1000, Some(1))]);
    }

    #[test]
    fn the_shown_score_rolls_up_to_the_real_one() {
        let mut r = RollingScore::new();
        let first = r.step(1000, RollingScore::TICK);
        assert!(first < 50, "under 5% of the gap a tick: {first}");
        let mut last = first;
        for _ in 0..300 {
            let shown = r.step(1000, RollingScore::TICK);
            assert!(shown >= last && shown <= 1000);
            last = shown;
        }
        assert_eq!(last, 1000, "it gets there and stops");
        assert_eq!(r.step(0, RollingScore::TICK), 0, "a restart drops it at once");
        assert_eq!(r.step(10, 0.0), 0, "no time, no roll");
    }

    #[test]
    fn stunt_names() {
        let stems: Vec<String> = ["seat_grab", "Seatgrab_handstand", "Y", "leg_splt", "double_can_can"].map(String::from).to_vec();
        assert_eq!(stunt_clip("Anim Stunt Seat Grab", &stems).as_deref(), Some("seat_grab"));
        assert_eq!(stunt_clip("Anim Stunt Seat Grab Hstand", &stems).as_deref(), Some("Seatgrab_handstand"));
        assert_eq!(stunt_clip("Anim Stunt Yogi", &stems).as_deref(), Some("Y"));
        assert_eq!(stunt_clip("Anim Stunt Scissors", &stems).as_deref(), Some("leg_splt"));
        assert_eq!(stunt_clip("Anim Stunt Double Cancan", &stems).as_deref(), Some("double_can_can"));
        assert_eq!(combo_mask("KeyCombo2"), Some(0b1100));
        assert_eq!(limit_label(Some(1)), "1 MINUTE");
    }

    fn trick(name: &str, points: u32) -> TrickDef {
        TrickDef { name: name.into(), level: 1, buttons: 1, dir: Dir::Up, points, hold_points: 20, total: 1.0, end: 0.4, clip: String::new() }
    }

    const DT: f32 = 1.0 / 60.0;

    fn session() -> Session {
        let boxes = vec![
            // A gap: each end pays its own points for the way it's jumped.
            TrickBox { name: "GAP JUMP".into(), points: 200, linked: Some(1), needs_trick: false, volumes: vec![(Vec3::new(0.0, 10.0, 0.0), Quat::IDENTITY, Vec3::ONE)] },
            TrickBox { name: "GAP JUMP".into(), points: 800, linked: Some(0), needs_trick: false, volumes: vec![(Vec3::new(0.0, 10.0, -20.0), Quat::IDENTITY, Vec3::ONE)] },
            TrickBox { name: "SPECIAL TRICK".into(), points: 1000, linked: None, needs_trick: true, volumes: vec![(Vec3::new(50.0, 10.0, 0.0), Quat::IDENTITY, Vec3::ONE)] },
        ];
        let mut s = Session::with(vec![trick("A", 100), trick("B", 300)], Scoring::RETAIL, boxes);
        s.freestyle = true;
        s
    }

    /// A quad with its wheels front left, front right, rear left and rear
    /// right, moving at 5 m/s.
    fn quad() -> Vehicle {
        let text = "WHEEL 0\nWHEELOFFSET -0.5 -0.17 0.58\nWHEEL 1\nWHEELOFFSET 0.5 -0.17 0.58\n\
                    WHEEL 2\nWHEELOFFSET -0.5 -0.17 -0.6\nWHEEL 3\nWHEELOFFSET 0.5 -0.17 -0.6\nEND\n";
        let mut v = Vehicle::new(atv_sim::VehicleParams::from_files(text, None), Vec3::ZERO, Quat::IDENTITY);
        v.velocity = Vec3::new(0.0, 0.0, -5.0);
        v
    }

    fn flat() -> TrackCollision {
        let e = 500.0;
        TrackCollision::new(vec![[Vec3::new(-e, 0.0, -e), Vec3::new(e, 0.0, -e), Vec3::new(0.0, 0.0, e)]], 50.0)
    }

    /// Steps `secs` with these wheels down (front left, front right, rear
    /// left, rear right).
    fn ride(s: &mut Session, v: &mut Vehicle, world: &TrackCollision, down: [bool; 4], secs: f32) {
        for _ in 0..(secs / DT).round() as usize {
            for (w, d) in v.wheels.iter_mut().zip(down) {
                w.contact = d;
            }
            v.air_time = if down.contains(&true) { 0.0 } else { v.air_time + DT };
            s.step(v, world, (0, None), DT, true, false);
        }
    }

    const ALL: [bool; 4] = [true; 4];
    const NONE: [bool; 4] = [false; 4];
    const WHEELIE: [bool; 4] = [false, false, true, true];
    const LEFT_BICYCLE: [bool; 4] = [true, false, true, false];

    #[test]
    fn reads_retail_and_prototype_ground_points() {
        let retail = setup::entries(
            r#"<Root><Wheelie Score="300" Accel="0.0001" Max="9999"/><Bicycle_Left Score="400" Accel="0.001" Max="99999"/>
            <Bicycle_Right Score="400" Accel="0.001" Max="99999"/><Endo Score="300" Accel="0.0015" Max="99999"/>
            <Forward_Flip Score="600"/><Backward_Flip Score="400"/><Clockwise_Spin Score="600"/><Counter_Clockwise_Spin Score="600"/></Root>"#,
        );
        assert_eq!(Scoring::load(&retail), Scoring::RETAIL);
        let proto = Scoring::load(&setup::entries(r#"<Root><Wheelie Wheelie="20"/><Bicycle Bicycle="20"/><Endo Endo="20"/></Root>"#));
        assert_eq!(proto.ground[0], GroundRate { score: 80.0, accel: 0.0, max: f32::MAX });
        assert_eq!(proto.ground[2], proto.ground[1]);
        assert_eq!((proto.back_flip, proto.front_flip, proto.spin_cw), (1000, 1000, 600));
    }

    #[test]
    fn a_wheelie_pays_its_rate_once_held_a_second() {
        let (mut s, mut v, w) = (session(), quad(), flat());
        ride(&mut s, &mut v, &w, ALL, 0.5);
        ride(&mut s, &mut v, &w, WHEELIE, 3.0);
        assert_eq!(s.score, 0, "nothing until it ends");
        // A moment on three wheels doesn't end it.
        ride(&mut s, &mut v, &w, [true, false, true, true], 0.05);
        ride(&mut s, &mut v, &w, WHEELIE, 1.0);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        // 300 a second for the 4 s and the moment it takes to settle.
        assert!((1225..=1250).contains(&s.score) && s.score % 5 == 0, "{}", s.score);
        assert!((s.longest_wheelie - 4.12).abs() < 0.05, "{}", s.longest_wheelie);
        // A short one pays nothing.
        let before = s.score;
        ride(&mut s, &mut v, &w, WHEELIE, 0.8);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.score, before);
        // Too slow to start one.
        v.velocity = Vec3::new(0.0, 0.0, -1.0);
        ride(&mut s, &mut v, &w, LEFT_BICYCLE, 2.0);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.score, before);
    }

    #[test]
    fn ground_tricks_string_into_a_combo() {
        let (mut s, mut v, w) = (session(), quad(), flat());
        ride(&mut s, &mut v, &w, ALL, 0.5);
        // A wheelie straight into a left bicycle: the wheelie pays, and the
        // bicycle starts half a second in with the wheelie's points.
        ride(&mut s, &mut v, &w, WHEELIE, 2.0);
        ride(&mut s, &mut v, &w, LEFT_BICYCLE, 1.0);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        // 600 + (600 + 400 a second for the 1 s and the settling), times
        // two moves.
        assert!((3220..=3260).contains(&s.score), "{}", s.score);
        assert_eq!(s.best_combo.0, 2);
    }

    #[test]
    fn combos_multiply_by_different_moves() {
        let mut s = session();
        s.combo.add(Some(Move::Trick(0)), 100);
        s.combo.add(Some(Move::Trick(1)), 300);
        s.combo.add(Some(Move::Trick(1)), 300);
        s.combo.add(Some(Move::Flip(false)), 400);
        s.combo.add(None, 200);
        s.complete();
        // Trick A, trick B, a backflip and a box: four different moves.
        assert_eq!(s.score, 1300 * 4);
        assert_eq!(s.best_combo, (5, 1300 * 4));
        assert_eq!(s.best_jump, 1300 * 4);
        // A smaller combo, however well it pays, leaves the best one.
        s.combo.add(Some(Move::Ground(Ground::Wheelie)), 9000);
        s.complete();
        assert_eq!(s.best_combo, (5, 1300 * 4));
    }

    #[test]
    fn hold_points_run_on_across_a_combo() {
        let mut s = session();
        let done = |def| ActiveTrick { def, t: 1.0, released: true };
        // Held 1.5 s: 20 a second on top of A's 100.
        s.combo.held = 1.5;
        s.finish_trick(done(0));
        assert_eq!(s.combo.points, 130);
        // B, held another quarter second, is paid for all 1.75 s.
        s.combo.held += 0.25;
        s.finish_trick(done(1));
        assert_eq!(s.combo.points, 130 + 335);
        s.complete();
        assert_eq!(s.combo.held, 0.0);
    }

    #[test]
    fn gaps_pay_the_end_they_start_from() {
        let (mut s, mut v, w) = (session(), quad(), flat());
        ride(&mut s, &mut v, &w, ALL, 0.2);
        // Through the 200 end, then the 800 one.
        v.position = Vec3::new(0.0, 10.0, 0.0);
        ride(&mut s, &mut v, &w, NONE, 0.1);
        v.position = Vec3::new(0.0, 10.0, -10.0);
        ride(&mut s, &mut v, &w, NONE, 0.1);
        v.position = Vec3::new(0.0, 10.0, -20.0);
        ride(&mut s, &mut v, &w, NONE, 0.1);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.score, 200);
        // The other way round pays the other end's.
        v.position = Vec3::new(0.0, 10.0, -20.0);
        ride(&mut s, &mut v, &w, NONE, 0.1);
        v.position = Vec3::new(0.0, 10.0, 0.0);
        ride(&mut s, &mut v, &w, NONE, 0.1);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.score, 1000);
        // One end alone pays nothing.
        v.position = Vec3::new(0.0, 10.0, 0.0);
        ride(&mut s, &mut v, &w, NONE, 0.1);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.score, 1000);
    }

    #[test]
    fn a_box_that_needs_a_trick_waits_for_one() {
        let (mut s, mut v, w) = (session(), quad(), flat());
        ride(&mut s, &mut v, &w, ALL, 0.2);
        v.position = Vec3::new(50.0, 10.0, 0.0);
        ride(&mut s, &mut v, &w, NONE, 0.2);
        assert_eq!(s.combo.moves, 0);
        // A trick later in the same jump, even outside the box.
        v.position = Vec3::new(50.0, 10.0, -30.0);
        s.start_trick(0);
        ride(&mut s, &mut v, &w, NONE, 0.1);
        assert_eq!((s.combo.moves, s.combo.points), (1, 1000));
    }

    #[test]
    fn flips_and_spins_pay_past_their_turn() {
        let (mut s, mut v, w) = (session(), quad(), flat());
        ride(&mut s, &mut v, &w, ALL, 0.2);
        v.position = Vec3::new(0.0, 3.0, 100.0);
        ride(&mut s, &mut v, &w, NONE, DT);
        // Nose up through a whole turn: one backflip, at 198 degrees.
        let turn = |s: &mut Session, v: &mut Vehicle, axis: Vec3, degrees: f32| {
            for _ in 0..(degrees / 5.0) as usize {
                v.rotation = Quat::from_axis_angle(axis, 5f32.to_radians()) * v.rotation;
                ride(s, v, &w, NONE, DT);
            }
        };
        turn(&mut s, &mut v, Vec3::X, 190.0);
        assert_eq!(s.combo.moves, 0);
        turn(&mut s, &mut v, Vec3::X, 170.0);
        assert_eq!((s.combo.moves, s.combo.points), (1, 400));
        // Turning right: a spin at 270 degrees, clockwise.
        turn(&mut s, &mut v, Vec3::NEG_Y, 300.0);
        assert_eq!((s.combo.moves, s.combo.points), (2, 1000));
        assert_eq!(s.combo.kinds, vec![Move::Flip(false), Move::Spin(false)]);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.score, 2000);
        // Nose down is a frontflip.
        v.rotation = Quat::IDENTITY;
        ride(&mut s, &mut v, &w, ALL, 0.2);
        ride(&mut s, &mut v, &w, NONE, DT);
        turn(&mut s, &mut v, Vec3::NEG_X, 210.0);
        assert_eq!(s.combo.kinds, vec![Move::Flip(true)]);
    }

    #[test]
    fn big_air_near_the_record() {
        let (mut s, mut v, w) = (session(), quad(), flat());
        s.air_record = Some(10.0);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        v.position = Vec3::new(0.0, 9.0, 100.0);
        ride(&mut s, &mut v, &w, NONE, 0.1);
        assert_eq!(s.combo.moves, 0);
        v.position.y = 9.6;
        ride(&mut s, &mut v, &w, NONE, 1.0);
        assert_eq!((s.combo.moves, s.combo.points), (1, BIG_AIR_POINTS), "once, then a wait");
        // Higher still: the jump is the new mark to get near.
        v.position.y = 12.0;
        ride(&mut s, &mut v, &w, NONE, 0.1);
        v.position.y = 0.0;
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.air_record, Some(12.0));
    }

    #[test]
    fn skill_level_gates_tricks() {
        let mut s = session();
        s.tricks[1].level = 3;
        s.tricks[1].buttons = 2;
        s.level = 2;
        assert_eq!((s.trick_for(1, Dir::Up), s.trick_for(2, Dir::Up)), (Some(0), None));
        s.level = 3;
        assert_eq!(s.trick_for(2, Dir::Up), Some(1));
    }

    #[test]
    fn bails_and_crashes_lose_the_combo() {
        let (mut s, mut v, w) = (session(), quad(), flat());
        ride(&mut s, &mut v, &w, ALL, 0.2);
        ride(&mut s, &mut v, &w, NONE, 0.5);
        s.combo.add(Some(Move::Trick(1)), 300);
        s.active = Some(ActiveTrick { def: 0, t: 0.3, released: false });
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.score, 0, "the whole combo is lost");
        assert!(s.popups.iter().any(|p| p.kind == PopupKind::Bail));
        // In the recovery part it counts.
        ride(&mut s, &mut v, &w, NONE, 0.5);
        s.active = Some(ActiveTrick { def: 0, t: 0.7, released: true });
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.score, 100);
        // A crash loses what's under way.
        s.combo.add(Some(Move::Trick(1)), 300);
        s.step(&v, &w, (0, None), DT, true, true);
        ride(&mut s, &mut v, &w, ALL, 0.2);
        assert_eq!(s.score, 100);
    }

    #[test]
    fn races_bank_trick_points_too() {
        let mut s = session();
        s.freestyle = false;
        s.combo.add(Some(Move::Trick(0)), 100);
        s.complete();
        assert_eq!(s.score, 100);
        assert_eq!(s.take_banked(), 100);
    }
}
