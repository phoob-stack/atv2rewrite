//! Tricks and Open Freestyle.
//!
//! Tricks come from the disc's `Setup/Tricks.xml`: each is a button (or a
//! pair of buttons) plus a stick direction, pulled while airborne. The rider
//! plays the trick's stunt animation; landing before its recovery part has
//! started is a bail and loses the jump. Several tricks in one jump
//! multiply, holding the buttons at a trick's peak adds its hold points,
//! and wheelies, endos and bicycles (two wheels on one side) score on the
//! ground. Tricks work in every mode; only freestyle shows the score.
//!
//! Open Freestyle is free roaming of a track against an optional time
//! limit, with Bronze/Silver/Gold targets from `Setup/Freestyle.xml`.
//! Besides tricks it scores flying through the stadiums' trick boxes
//! (`Setup/TrickBoxes.xml` and the `TBOXn` locators) and finding the
//! track's spots (checkpoints, respots and photo boxes) for the first time.

use std::sync::Arc;

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::prelude::*;

use atv_formats::row::World;
use atv_formats::setup;
use atv_formats::GameSource;
use atv_sim::track as trk;
use atv_sim::Vehicle;

use crate::race::fmt_time;
use crate::rider::Clip;
use crate::ui::{set_text, BitmapText, FontId};
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

/// Trick buttons, as a mask: the disc's D, A, B and E buttons in order of
/// the tricks' level (1 to 4). The three two-button combos are levels 5-7.
pub const TRICK_KEYS: [[KeyCode; 2]; 4] =
    [[KeyCode::Digit1, KeyCode::Numpad1], [KeyCode::Digit2, KeyCode::Numpad2], [KeyCode::Digit3, KeyCode::Numpad3], [KeyCode::Digit4, KeyCode::Numpad4]];
pub const TRICK_PAD: [GamepadButton; 4] = [GamepadButton::West, GamepadButton::North, GamepadButton::LeftTrigger, GamepadButton::RightTrigger];

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

/// Points per quarter second of wheelie, bicycle and endo.
#[derive(Clone, Copy, Debug)]
pub struct GroundPoints {
    pub wheelie: u32,
    pub bicycle: u32,
    pub endo: u32,
}

pub fn load_tricks(game: &GameSource) -> (Vec<TrickDef>, GroundPoints) {
    let xml = game.read("/Setup/Tricks.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let stems: Vec<String> = game
        .archive_paths()
        .filter_map(|p| p.strip_prefix("/Animations/Stunts/Bac/")?.strip_suffix(".bac").map(str::to_owned))
        .collect();
    let entries = setup::entries(&xml);
    let num = |name: &str, key: &str, default: u32| {
        entries.iter().find(|e| e.name == name).and_then(|e| e.num(key)).map_or(default, |v| v as u32)
    };
    let ground = GroundPoints { wheelie: num("Wheelie", "Wheelie", 20), bicycle: num("Bicycle", "Bicycle", 20), endo: num("Endo", "Endo", 20) };
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
    (tricks, ground)
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

/// Bronze, silver and gold scores for an Open Freestyle time limit.
pub fn medal_targets(game: &GameSource, limit: Option<u32>) -> Option<[u32; 3]> {
    let xml = game.read("/Setup/Freestyle.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).ok()?;
    let tag = match limit {
        Some(1) => "Time1Minute".to_string(),
        Some(m) => format!("Time{m}Minutes"),
        None => "TimeUnlimited".to_string(),
    };
    let block = xml.split(&format!("<{tag}>")).nth(1)?.split(&format!("</{tag}>")).next()?;
    let mut out = [0; 3];
    for (i, medal) in ["Bronze", "Silver", "Gold"].iter().enumerate() {
        let rec = block.split(&format!("<{medal}Record")).nth(1)?;
        out[i] = rec.split("Record=\"").nth(1)?.split('"').next()?.trim().parse().ok()?;
    }
    Some(out)
}

pub const MEDALS: [&str; 3] = ["BRONZE", "SILVER", "GOLD"];

/// A trick box: fly through any of its volumes (and its linked box's, if
/// it has one, in the same jump) for its points.
#[derive(Clone, Debug)]
struct TrickBox {
    name: String,
    points: u32,
    linked: Option<usize>,
    volumes: Vec<(Vec3, Quat, Vec3)>,
}

fn load_boxes(game: &GameSource, track: &str, world: &World) -> Vec<TrickBox> {
    let xml = game.read("/Setup/TrickBoxes.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let Some(block) = xml.split(&format!("<{track}>")).nth(1).and_then(|b| b.split(&format!("</{track}>")).next()) else { return Vec::new() };
    let attr = |tag: &str, key: &str| tag.split(&format!("{key}=\"")).nth(1).and_then(|r| r.split('"').next()).map(str::trim).map(str::to_owned);
    let mut defs: Vec<(String, String, u32, String)> = Vec::new();
    for tag in block.split('<').skip(1) {
        let Some(id) = tag.split_whitespace().next().filter(|t| t.starts_with("TBOX")) else { continue };
        let points: u32 = attr(tag, "Points").and_then(|p| p.parse().ok()).unwrap_or(0);
        if points == 0 {
            continue;
        }
        defs.push((id.to_string(), attr(tag, "Name").unwrap_or_default().replace('_', " ").to_uppercase(), points, attr(tag, "LinkedBox").unwrap_or_default()));
    }
    let mut boxes: Vec<TrickBox> = defs
        .iter()
        .map(|(id, name, points, _)| TrickBox {
            name: name.clone(),
            points: *points,
            linked: None,
            volumes: world
                .locators
                .iter()
                .filter(|l| l.name.trim().eq_ignore_ascii_case(id))
                .map(|l| (trk::to_sim(l.pos), trk::rotation_to_sim(&l.rotation), Vec3::from(l.scale).abs()))
                .collect(),
        })
        .collect();
    for (i, (_, _, _, link)) in defs.iter().enumerate() {
        boxes[i].linked = defs.iter().position(|d| d.0.eq_ignore_ascii_case(link));
    }
    boxes.retain(|b| !b.volumes.is_empty());
    boxes
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

pub const SPOT_POINTS: u32 = 250;
const SPOT_RADIUS: f32 = 14.0;
/// Seconds airborne before trick buttons count (skips bumps), and before
/// touching down counts as a landing.
const TRICK_AIR: f32 = 0.3;
const LANDING_AIR: f32 = 0.2;
/// Buttons must be steady this long, so pressing a combo's two buttons a
/// frame apart doesn't start the one-button trick.
const COMBO_WINDOW: f32 = 0.06;
const HOLD_TICK: f32 = 0.2;

#[derive(Clone, Debug)]
pub struct ActiveTrick {
    pub def: usize,
    /// Seconds into the trick (paused at the peak while held).
    pub t: f32,
    held: f32,
    /// The buttons were let go, so the trick can't be held any more.
    released: bool,
}

impl ActiveTrick {
    fn hold_points(&self, def: &TrickDef) -> u32 {
        (self.held / HOLD_TICK) as u32 * def.hold_points
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

#[derive(Default, Clone, Debug)]
struct Jump {
    tricks: Vec<(usize, u32)>,
    boxes: Vec<usize>,
    /// Rotation turned through in the air, about the quad's own pitch, yaw
    /// and roll axes (radians).
    turned: Vec3,
    from: Vec3,
    top: f32,
}

/// A ground trick: two wheels off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ground {
    Wheelie,
    Endo,
    Bicycle,
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
    ground_points: GroundPoints,
    boxes: Vec<TrickBox>,
    pub spots: Vec<(Vec3, bool)>,
    pub score: u32,
    /// Points banked since the speed boost last took them.
    banked: u32,
    pub active: Option<ActiveTrick>,
    jump: Jump,
    ground: Option<(Ground, f32)>,
    pub popups: Vec<Popup>,
    pub time: f32,
    pub best_trick: Option<(String, u32)>,
    pub best_jump: u32,
    pub landed: u32,
    pub longest_air: f32,
    /// Ended from the pause menu (unlimited sessions).
    pub ended: bool,
    mask: u8,
    steady: f32,
    armed: bool,
    last_air: f32,
    last_rot: Quat,
}

impl Session {
    pub fn new(game: &GameSource, sel: &Selection, world: &World) -> Self {
        let (tricks, ground_points) = load_tricks(game);
        let freestyle = sel.mode == Mode::Freestyle;
        let mut s = Self::with(tricks, ground_points, if freestyle { load_boxes(game, &sel.track, world) } else { Vec::new() });
        s.freestyle = freestyle;
        if freestyle {
            s.limit = sel.time_limit.map(|m| m as f32 * 60.0);
            s.medals = medal_targets(game, sel.time_limit);
            s.spots = load_spots(world).into_iter().map(|p| (p, false)).collect();
        }
        s
    }

    fn with(tricks: Vec<TrickDef>, ground_points: GroundPoints, boxes: Vec<TrickBox>) -> Self {
        Self {
            freestyle: false,
            limit: None,
            medals: None,
            tricks,
            level: MAX_LEVEL,
            ground_points,
            boxes,
            spots: Vec::new(),
            score: 0,
            banked: 0,
            active: None,
            jump: Jump::default(),
            ground: None,
            popups: Vec::new(),
            time: 0.0,
            best_trick: None,
            best_jump: 0,
            landed: 0,
            longest_air: 0.0,
            ended: false,
            mask: 0,
            steady: 0.0,
            armed: true,
            last_air: 0.0,
            last_rot: Quat::IDENTITY,
        }
    }

    #[cfg(test)]
    pub fn test_with(tricks: Vec<TrickDef>) -> Self {
        Self::with(tricks, GroundPoints { wheelie: 20, bicycle: 20, endo: 20 }, Vec::new())
    }

    pub fn time_left(&self, clock: f32) -> Option<f32> {
        self.limit.map(|l| (l - clock).max(0.0))
    }

    /// Time is up (or the session was ended).
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

    /// The ground trick being held right now.
    pub fn ground_trick(&self) -> Option<Ground> {
        self.ground.map(|g| g.0)
    }

    /// The medal the score has earned (0 = bronze).
    pub fn medal(&self) -> Option<usize> {
        let m = self.medals?;
        (0..3).rev().find(|&i| self.score >= m[i])
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
            self.active = Some(ActiveTrick { def, t: 0.0, held: 0.0, released: true });
        }
    }

    /// One physics step. `live`: the clock is running (tricks are held
    /// during the countdown and after the end).
    pub fn step(&mut self, v: &Vehicle, buttons: u8, dir: Option<Dir>, dt: f32, live: bool) {
        self.time += dt;
        let air = v.air_time;
        let grounded = v.grounded();

        // Rotation in the air, in the quad's own frame.
        if air > 0.0 {
            let (axis, angle) = (self.last_rot.inverse() * v.rotation).to_axis_angle();
            if angle.is_finite() && angle < 1.0 {
                self.jump.turned += axis * angle;
            }
            self.jump.top = self.jump.top.max(v.position.y - self.jump.from.y);
        } else {
            self.jump.from = v.position;
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
        if live && air > TRICK_AIR && self.active.is_none() && self.armed && buttons != 0 && self.steady >= COMBO_WINDOW
            && let Some(dir) = dir
            && let Some(def) = self.trick_for(buttons, dir)
        {
            self.active = Some(ActiveTrick { def, t: 0.0, held: 0.0, released: false });
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
                a.held += dt;
            } else {
                a.t += dt;
            }
            if a.t >= def.total {
                let done = self.active.take().unwrap();
                self.finish_trick(done);
            }
        }

        // Trick boxes, flown through.
        if air > TRICK_AIR {
            for (i, b) in self.boxes.iter().enumerate() {
                let inside = b.volumes.iter().any(|(c, r, h)| {
                    let l = r.inverse() * (v.position - *c);
                    l.x.abs() <= h.x && l.y.abs() <= h.y && l.z.abs() <= h.z
                });
                if inside && !self.jump.boxes.contains(&i) {
                    self.jump.boxes.push(i);
                }
            }
        }

        // Landing.
        if self.last_air > LANDING_AIR && grounded {
            self.land(self.last_air, v.position);
        }
        self.last_air = air;

        if live {
            self.ground_tricks(v, dt);
            self.find_spots(v.position);
        }
    }

    fn finish_trick(&mut self, a: ActiveTrick) {
        let def = &self.tricks[a.def];
        let pts = def.points + a.hold_points(def);
        let name = def.name.clone();
        self.jump.tricks.push((a.def, a.hold_points(def)));
        self.popup(format!("{name}  {pts}"), PopupKind::Trick);
    }

    fn land(&mut self, air: f32, at: Vec3) {
        if let Some(a) = self.active.take() {
            let def = &self.tricks[a.def];
            if a.t < def.total - def.end {
                // Still mid-trick: bailed, the jump scores nothing.
                self.popup("BAILED", PopupKind::Bail);
                self.jump = Jump { from: at, ..default() };
                return;
            }
            self.finish_trick(a);
        }
        let jump = std::mem::take(&mut self.jump);
        self.jump.from = at;
        self.longest_air = self.longest_air.max(air);

        let mut total = 0;
        let mut names: Vec<usize> = jump.tricks.iter().map(|t| t.0).collect();
        names.sort_unstable();
        names.dedup();
        let trick_points: u32 = jump.tricks.iter().map(|(d, hold)| self.tricks[*d].points + hold).sum();
        // Different tricks in one jump multiply.
        let multiplier = names.len().max(1) as u32;
        total += trick_points * multiplier;
        for (d, hold) in &jump.tricks {
            let def = &self.tricks[*d];
            if self.best_trick.as_ref().is_none_or(|b| def.points + hold > b.1) {
                self.best_trick = Some((def.name.clone(), def.points + hold));
            }
        }
        self.landed += jump.tricks.len() as u32;

        let mut bonuses: Vec<(String, u32)> = Vec::new();
        // Whole turns about each axis.
        let turns = |a: f32| ((a.abs() + 0.5) / std::f32::consts::TAU) as u32;
        let flips = turns(jump.turned.x);
        if flips > 0 {
            bonuses.push((if jump.turned.x > 0.0 { "BACKFLIP" } else { "FRONTFLIP" }.into(), 1000 * flips));
        }
        let spins = turns(jump.turned.y);
        if spins > 0 {
            bonuses.push((format!("{}", 360 * spins), 600 * spins));
        }
        let rolls = turns(jump.turned.z);
        if rolls > 0 {
            bonuses.push(("BARREL ROLL".into(), 800 * rolls));
        }
        for &b in &jump.boxes {
            let tb = &self.boxes[b];
            // A linked pair (a gap) needs both ends in the same jump, and
            // pays once.
            match tb.linked {
                Some(l) if !jump.boxes.contains(&l) => continue,
                Some(l) if l < b => continue,
                _ => {}
            }
            bonuses.push((tb.name.clone(), tb.points));
        }
        if air >= 1.5 {
            bonuses.push(("BIG AIR".into(), (air * 100.0) as u32));
        }
        for (name, pts) in &bonuses {
            total += pts;
            self.popup(format!("{name}  {pts}"), PopupKind::Bonus);
        }
        if total > 0 {
            if multiplier > 1 {
                self.popup(format!("{multiplier}X COMBO  {total}"), PopupKind::Total);
            } else if !jump.tricks.is_empty() || bonuses.len() > 1 {
                self.popup(format!("LANDED  {total}"), PopupKind::Total);
            }
            self.best_jump = self.best_jump.max(total);
            self.award(total);
        }
    }

    fn ground_tricks(&mut self, v: &Vehicle, dt: f32) {
        let contact = |front: bool, left: Option<bool>| {
            v.wheels.iter().zip(&v.params.wheels).filter(|(_, p)| (p.offset[2] < 0.0) == front && left.is_none_or(|l| (p.offset[0] < 0.0) == l)).all(|(w, _)| w.contact)
        };
        let none = |front: bool, left: Option<bool>| {
            v.wheels.iter().zip(&v.params.wheels).filter(|(_, p)| (p.offset[2] < 0.0) == front && left.is_none_or(|l| (p.offset[0] < 0.0) == l)).all(|(w, _)| !w.contact)
        };
        let side = |left: bool| {
            v.wheels.iter().zip(&v.params.wheels).all(|(w, p)| w.contact == ((p.offset[0] < 0.0) == left))
        };
        let moving = v.velocity.length() > 4.0;
        let now = if !moving || v.wheels.len() < 4 {
            None
        } else if contact(false, None) && none(true, None) {
            Some(Ground::Wheelie)
        } else if contact(true, None) && none(false, None) {
            Some(Ground::Endo)
        } else if side(true) || side(false) {
            Some(Ground::Bicycle)
        } else {
            None
        };
        match (self.ground, now) {
            (Some((g, secs)), Some(n)) if g == n => self.ground = Some((g, secs + dt)),
            (prev, now) => {
                if let Some((g, secs)) = prev
                    && secs >= 1.0
                {
                    let (name, per) = match g {
                        Ground::Wheelie => ("WHEELIE", self.ground_points.wheelie),
                        Ground::Endo => ("ENDO", self.ground_points.endo),
                        Ground::Bicycle => ("BICYCLE", self.ground_points.bicycle),
                    };
                    let pts = per * (secs / 0.25) as u32;
                    self.popup(format!("{name} {secs:.1}S  {pts}"), PopupKind::Bonus);
                    self.award(pts);
                }
                self.ground = now.map(|n| (n, 0.0));
            }
        }
    }

    fn find_spots(&mut self, at: Vec3) {
        let Some(i) = self.spots.iter().position(|(p, found)| !found && (*p - at).with_y(0.0).length() < SPOT_RADIUS) else { return };
        self.spots[i].1 = true;
        let (found, n) = (self.spots_found(), self.spots.len());
        self.popup(format!("NEW SPOT {found}/{n}  {SPOT_POINTS}"), PopupKind::Bonus);
        self.award(SPOT_POINTS);
    }
}

/// Held trick buttons and the stick direction, from keyboard and pads.
/// `slot`: in split screen, the player (0-based): player 1 has the number
/// row and WASD, player 2 the number pad and the arrows, and each has their
/// own pad; alone, every key and pad counts.
pub fn read_tricks(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>, slot: Option<usize>) -> (u8, Option<Dir>) {
    let (buttons, [right, left, up, down]): (Vec<KeyCode>, [&[KeyCode]; 4]) = match slot {
        None => (TRICK_KEYS.iter().flatten().copied().collect(), [
            &[KeyCode::ArrowRight, KeyCode::KeyD],
            &[KeyCode::ArrowLeft, KeyCode::KeyA],
            &[KeyCode::ArrowUp, KeyCode::KeyW, KeyCode::KeyI],
            &[KeyCode::ArrowDown, KeyCode::KeyS, KeyCode::KeyK],
        ]),
        Some(0) => (TRICK_KEYS.iter().map(|k| k[0]).collect(), [&[KeyCode::KeyD], &[KeyCode::KeyA], &[KeyCode::KeyW], &[KeyCode::KeyS]]),
        Some(1) => (TRICK_KEYS.iter().map(|k| k[1]).collect(), [&[KeyCode::ArrowRight], &[KeyCode::ArrowLeft], &[KeyCode::ArrowUp], &[KeyCode::ArrowDown]]),
        Some(_) => (Vec::new(), [&[], &[], &[], &[]]),
    };
    let pads = || pads.iter().enumerate().filter(|(n, _)| slot.is_none_or(|s| s == *n)).map(|(_, p)| p);
    let mut mask = 0u8;
    for (i, pad_button) in TRICK_PAD.iter().enumerate() {
        let key = TRICK_KEYS[i].iter().any(|k| buttons.contains(k) && keys.pressed(*k));
        if key || pads().any(|p| p.pressed(*pad_button)) {
            mask |= 1 << i;
        }
    }
    let key = |k: &[KeyCode]| if k.iter().any(|k| keys.pressed(*k)) { 1.0 } else { 0.0 };
    let mut x: f32 = key(right) - key(left);
    let mut y: f32 = key(up) - key(down);
    for pad in pads() {
        // The stick, or the d-pad the Trick Book's pad picture lights.
        let d = |b: GamepadButton| if pad.pressed(b) { 1.0 } else { 0.0 };
        x += pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0) + d(GamepadButton::DPadRight) - d(GamepadButton::DPadLeft);
        y += pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0) + d(GamepadButton::DPadUp) - d(GamepadButton::DPadDown);
    }
    let dir = if x.abs().max(y.abs()) < 0.5 {
        None
    } else if x.abs() > y.abs() {
        Some(if x > 0.0 { Dir::Right } else { Dir::Left })
    } else {
        Some(if y > 0.0 { Dir::Up } else { Dir::Down })
    };
    (mask, dir)
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
    Spots,
    Target,
}
#[derive(Component)]
struct PopupLine(usize);

const POPUP_LINES: usize = 4;
const POPUP_SECS: f32 = 2.2;

fn spawn_hud(mut commands: Commands, session: Res<Session>) {
    commands
        .spawn((crate::ui::fullscreen(), DespawnOnExit(AppState::Race), Pickable::IGNORE))
        .with_children(|root| {
            if session.freestyle {
                root.spawn(Node { position_type: PositionType::Absolute, left: px(24), top: px(18), flex_direction: FlexDirection::Column, row_gap: px(4), ..default() })
                    .with_children(|c| {
                        c.spawn(Node { align_items: AlignItems::FlexEnd, column_gap: px(6), ..default() }).with_children(|r| {
                            r.spawn(BitmapText::new(FontId::Small, 9.0, "SCORE"));
                            r.spawn((BitmapText::new(FontId::Chrome, 22.0, "0"), FsReadout::Score));
                        });
                        c.spawn((BitmapText::new(FontId::Small, 8.0, "").color(Color::srgb(1.0, 0.85, 0.3)), FsReadout::Target));
                        c.spawn((BitmapText::new(FontId::Small, 8.0, "").color(Color::srgb(0.75, 0.85, 1.0)), FsReadout::Spots));
                    });
                root.spawn(Node {
                    position_type: PositionType::Absolute,
                    right: px(24),
                    top: px(18),
                    align_items: AlignItems::FlexEnd,
                    column_gap: px(6),
                    ..default()
                })
                .with_children(|r| {
                    r.spawn(BitmapText::new(FontId::Small, 8.0, if session.limit.is_some() { "TIME LEFT" } else { "TIME" }));
                    r.spawn((BitmapText::new(FontId::Chrome, 18.0, "0:00.00"), FsReadout::Time));
                });
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

fn update_hud(
    session: Option<Res<Session>>,
    progress: Option<Res<crate::race::RaceProgress>>,
    mut readouts: Query<(&FsReadout, &mut BitmapText), Without<PopupLine>>,
    mut lines: Query<(&PopupLine, &mut BitmapText), Without<FsReadout>>,
) {
    let (Some(s), Some(p)) = (session, progress) else { return };
    for (r, mut t) in &mut readouts {
        let text = match r {
            FsReadout::Score => s.score.to_string(),
            FsReadout::Time => fmt_time(s.time_left(p.clock).unwrap_or(p.clock)),
            FsReadout::Spots => format!("SPOTS {}/{}", s.spots_found(), s.spots.len()),
            FsReadout::Target => match s.medals {
                Some(m) => match (0..3).find(|&i| s.score < m[i]) {
                    Some(i) => format!("{} {}", MEDALS[i], m[i]),
                    None => "GOLD MEDAL!".into(),
                },
                None => String::new(),
            },
        };
        set_text(&mut t, &text);
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

    fn session() -> Session {
        let boxes = vec![
            TrickBox { name: "GAP".into(), points: 1000, linked: Some(1), volumes: vec![] },
            TrickBox { name: "GAP".into(), points: 1000, linked: Some(0), volumes: vec![] },
            TrickBox { name: "HOOP".into(), points: 500, linked: None, volumes: vec![] },
        ];
        let mut s = Session::with(vec![trick("A", 100), trick("B", 300)], GroundPoints { wheelie: 20, bicycle: 20, endo: 20 }, boxes);
        s.freestyle = true;
        s
    }

    #[test]
    fn combos_multiply_and_gaps_pay_once() {
        let mut s = session();
        s.jump.tricks = vec![(0, 0), (1, 40), (1, 0)];
        s.jump.boxes = vec![0, 1, 2];
        s.land(1.0, Vec3::ZERO);
        // (100 + 340 + 300) x 2 different tricks, the gap once, the hoop.
        assert_eq!(s.score, 740 * 2 + 1000 + 500);
        assert_eq!(s.best_trick, Some(("B".into(), 340)));
        // Half a gap pays nothing.
        s.jump.boxes = vec![1];
        s.land(1.0, Vec3::ZERO);
        assert_eq!(s.score, 740 * 2 + 1500);
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
    fn landing_mid_trick_bails() {
        let mut s = session();
        s.jump.tricks = vec![(1, 0)];
        s.active = Some(ActiveTrick { def: 0, t: 0.3, held: 0.0, released: false });
        s.land(1.0, Vec3::ZERO);
        assert_eq!(s.score, 0, "the whole jump is lost");
        assert_eq!(s.popups.last().map(|p| p.kind), Some(PopupKind::Bail));
        // In the recovery part it counts.
        s.active = Some(ActiveTrick { def: 0, t: 0.7, held: 0.0, released: true });
        s.land(1.0, Vec3::ZERO);
        assert_eq!(s.score, 100);
    }

    #[test]
    fn races_bank_trick_points_too() {
        let mut s = session();
        s.freestyle = false;
        s.jump.tricks = vec![(0, 0)];
        s.land(1.0, Vec3::ZERO);
        assert_eq!(s.score, 100);
    }
}
