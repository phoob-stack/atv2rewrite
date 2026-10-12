//! Poses the rider on the quad from the game's own riding animations.
//!
//! The riding clips (`Animations/Generic/Bac`) each run from the neutral
//! seated pose to a full lean. The rider leans into turns (`turn_right1`,
//! mirrored for left turns), shifts weight forward and back with the lean
//! control (`leen_forward`, `leen_back`) and crouches while the suspension
//! is pre-loaded for a hop (`landing`, whose middle is the deepest crouch).
//! Forward/back lean and crouch are layered on the turn pose as offsets from
//! each clip's first frame. Tricks blend a stunt clip
//! (`Animations/Stunts/Bac`) over all of it. A hard landing plays the
//! landing clip once on top, and a crash replaces the whole pose with the
//! crash clips (`Animations/Crashes/Bac`) while the rider tumbles free of
//! the quad.
//!
//! The retail disc has no `Bac` folders: every clip is packed as a `.cuban`
//! one folder up, with its root key relative to the seated pelvis, which
//! the `.bac` clips bake in ([`SEAT`]).

use std::sync::Arc;

use anyhow::Result;
use bevy::prelude::*;

use atv_formats::GameSource;
use atv_formats::anim::{Animation, SkelInfo};

use crate::convert::RiderRig;

const CLIP_DIR: &str = "/Animations/Generic/Bac/";

/// The seated pelvis in quad-model space (game frame): every prototype
/// `.bac` riding, crash and stunt clip's root key is its `.cuban` twin's
/// plus this.
const SEAT: [f32; 3] = [-0.001_279_1, 1.024_095_4, -0.289_261_25];

/// Where a clip is on this disc: the `.bac` path if it is there, else its
/// `.cuban` twin (`/Animations/X/Bac/name.bac` -> `/Animations/X/name.cuban`).
fn clip_path(game: &GameSource, path: &str) -> String {
    if game.exists(path) {
        return path.to_string();
    }
    match path.strip_suffix(".bac").and_then(|p| p.rsplit_once("/Bac/")) {
        Some((dir, name)) => format!("{dir}/{name}.cuban"),
        None => path.to_string(),
    }
}

/// The stunt clips on this disc, by file stem.
pub fn stunt_stems(game: &GameSource) -> Vec<String> {
    let mut stems: Vec<String> = game
        .archive_paths()
        .filter_map(|p| p.strip_prefix("/Animations/Stunts/Bac/")?.strip_suffix(".bac").map(str::to_owned))
        .collect();
    if stems.is_empty() {
        stems = game
            .archive_paths()
            .filter_map(|p| p.strip_prefix("/Animations/Stunts/")?.strip_suffix(".cuban").map(str::to_owned))
            .collect();
    }
    stems
}

/// One bone's local transform (Bevy space).
#[derive(Clone, Copy)]
struct BonePose {
    position: Vec3,
    rotation: Quat,
}

/// A clip resampled into Bevy space: `frames[frame][bone]`.
pub struct Clip {
    frames: Vec<Vec<BonePose>>,
}

impl Clip {
    fn load(game: &GameSource, name: &str) -> Result<Self> {
        Self::load_from(game, &format!("{CLIP_DIR}{name}.bac"))
    }

    pub fn load_from(game: &GameSource, path: &str) -> Result<Self> {
        let path = clip_path(game, path);
        let mut anim = Animation::parse_any(&game.read(&path)?)?;
        anyhow::ensure!(anim.frame_count > 0, "{path}: no frames");
        if path.ends_with(".cuban") {
            for k in &mut anim.keys[..anim.frame_count] {
                k.position = [0, 1, 2].map(|a| k.position[a] + SEAT[a]);
            }
        }
        let frames = (0..anim.frame_count)
            .map(|f| {
                (0..anim.bone_count)
                    .map(|b| {
                        let k = anim.key(b, f);
                        let [x, y, z, w] = k.rotation;
                        // Z-mirror into Bevy's right-handed space.
                        BonePose {
                            position: Vec3::new(k.position[0], k.position[1], -k.position[2]),
                            rotation: Quat::from_xyzw(-x, -y, z, w).normalize(),
                        }
                    })
                    .collect()
            })
            .collect();
        Ok(Clip { frames })
    }

    pub fn frames(&self) -> usize {
        self.frames.len()
    }

    /// Left-right mirror image, swapping paired bones.
    pub fn mirrored(&self, map: &[usize]) -> Self {
        let frames = self
            .frames
            .iter()
            .map(|pose| {
                (0..pose.len())
                    .map(|b| {
                        let p = pose[map.get(b).copied().unwrap_or(b).min(pose.len() - 1)];
                        let q = p.rotation;
                        BonePose {
                            position: p.position * Vec3::new(-1.0, 1.0, 1.0),
                            rotation: Quat::from_xyzw(q.x, -q.y, -q.z, q.w),
                        }
                    })
                    .collect()
            })
            .collect();
        Clip { frames }
    }

    /// Pose at `t` (0 = first frame, 1 = last), interpolating frames.
    fn sample(&self, t: f32) -> Vec<BonePose> {
        let last = self.frames.len() - 1;
        let f = t.clamp(0.0, 1.0) * last as f32;
        let (i, s) = (f.floor() as usize, f.fract());
        let (a, b) = (&self.frames[i], &self.frames[(i + 1).min(last)]);
        a.iter()
            .zip(b)
            .map(|(a, b)| BonePose { position: a.position.lerp(b.position, s), rotation: a.rotation.slerp(b.rotation, s) })
            .collect()
    }
}

#[derive(Component)]
pub struct Rider {
    joints: Vec<Entity>,
    offsets: Vec<Vec3>,
    turn_right: Clip,
    turn_left: Clip,
    forward: Clip,
    back: Clip,
    crouch: Clip,
    /// Smoothed controls driving the pose.
    steer: f32,
    lean: f32,
    preload: f32,
    /// A stunt being performed (clip, 0..1 through it) and how far the
    /// pose has blended into it; the last stunt is kept to blend out of.
    stunt: Option<(Arc<Clip>, f32)>,
    stunt_weight: f32,
    /// Seconds into a landing, while one plays.
    landing: Option<f32>,
    /// While crashed: the crash clip and how far through it (0..1). The
    /// pose eases towards it from the last one shown.
    crash: Option<(Arc<Clip>, f32)>,
    shown: Vec<Transform>,
    /// Left-right bone pairs, for mirroring clips.
    pub mirror_map: Vec<usize>,
}

/// Rate the one-shot clips play at, frames per second.
pub const ONE_SHOT_FPS: f32 = 30.0;

/// How far into each clip the controls go. The clips end in extreme poses
/// (hanging off the side, standing on the pegs); a full input reaches this.
const TURN_REACH: f32 = 0.6;
const LEAN_REACH: f32 = 0.8;
/// The deepest crouch is the middle of the landing clip.
const CROUCH_REACH: f32 = 0.5;

impl Rider {
    pub fn new(game: &GameSource, rig: RiderRig) -> Result<Self> {
        let skel = game.read("/Characters/Skeletons/atvrider.skel").map(|b| SkelInfo::parse(&String::from_utf8_lossy(&b)));
        let map = skel.unwrap_or_default().mirror_map(rig.joints.len());
        let turn_right = Clip::load(game, "turn_right1")?;
        let turn_left = turn_right.mirrored(&map);
        let bones = rig.joints.len();
        let rider = Rider {
            joints: rig.joints,
            offsets: rig.offsets,
            turn_right,
            turn_left,
            forward: Clip::load(game, "leen_forward")?,
            back: Clip::load(game, "leen_back")?,
            crouch: Clip::load(game, "landing")?,
            steer: 0.0,
            lean: 0.0,
            preload: 0.0,
            stunt: None,
            stunt_weight: 0.0,
            landing: None,
            crash: None,
            shown: Vec::new(),
            mirror_map: map,
        };
        for c in [&rider.turn_right, &rider.forward, &rider.back, &rider.crouch] {
            anyhow::ensure!(c.frames[0].len() == bones, "riding clip has {} bones, skeleton {bones}", c.frames[0].len());
        }
        Ok(rider)
    }

    /// Eases the controls towards their targets.
    pub fn update_controls(&mut self, steer: f32, lean: f32, preload: f32, dt: f32) {
        let k = 1.0 - (-8.0 * dt).exp();
        self.steer += (steer.clamp(-1.0, 1.0) - self.steer) * k;
        self.lean += (lean.clamp(-1.0, 1.0) - self.lean) * k;
        self.preload += (preload.clamp(0.0, 1.0) - self.preload) * k;
        if let Some(t) = &mut self.landing {
            *t += dt;
            if *t * ONE_SHOT_FPS >= (self.crouch.frames.len() - 1) as f32 {
                self.landing = None;
            }
        }
    }

    /// Starts the landing clip from the top.
    pub fn land(&mut self) {
        self.landing = Some(0.0);
    }

    /// Shows a crash clip at `t` (0..1) instead of riding, or rides again
    /// with `None`.
    pub fn set_crash(&mut self, crash: Option<(Arc<Clip>, f32)>) {
        self.crash = crash;
    }

    /// The skeleton's joint entities.
    pub fn joints(&self) -> &[Entity] {
        &self.joints
    }

    pub fn crashed(&self) -> bool {
        self.crash.is_some()
    }

    /// Plays a stunt at `t` (0..1) over the riding pose, or blends back to
    /// riding with `None`.
    pub fn set_stunt(&mut self, stunt: Option<(Arc<Clip>, f32)>, dt: f32) {
        let on = stunt.is_some();
        if let Some(s) = stunt {
            if self.stunt.as_ref().is_some_and(|(c, _)| !Arc::ptr_eq(c, &s.0)) {
                self.stunt_weight = self.stunt_weight.min(0.3);
            }
            self.stunt = Some(s);
        }
        // Quick in, a little slower out.
        let rate = if on { 12.0 } else { 7.0 };
        let target = if on { 1.0 } else { 0.0 };
        self.stunt_weight += (target - self.stunt_weight) * (1.0 - (-rate * dt).exp());
        if !on && self.stunt_weight < 0.01 {
            self.stunt = None;
            self.stunt_weight = 0.0;
        }
    }

    /// Local transform of every joint for the current controls, eased
    /// over `dt` while crashed.
    fn next_pose(&mut self, dt: f32) -> Vec<Transform> {
        let target = self.pose();
        if self.crash.is_none() || self.shown.len() != target.len() {
            self.shown = target;
            return self.shown.clone();
        }
        let k = 1.0 - (-15.0 * dt).exp();
        for (s, t) in self.shown.iter_mut().zip(&target) {
            s.translation = s.translation.lerp(t.translation, k);
            s.rotation = s.rotation.slerp(t.rotation, k).normalize();
        }
        self.shown.clone()
    }

    /// Local transform of every joint for the current controls.
    fn pose(&self) -> Vec<Transform> {
        if let Some((clip, t)) = self.crash.as_ref().filter(|(c, _)| c.frames[0].len() == self.joints.len()) {
            // The pelvis sits on the tumbling body; the clip turns it.
            return clip
                .sample(*t)
                .iter()
                .enumerate()
                .map(|(b, p)| Transform {
                    translation: if b == 0 { Vec3::ZERO } else { self.offsets[b] },
                    rotation: p.rotation,
                    scale: Vec3::ONE,
                })
                .collect();
        }
        let turn = if self.steer >= 0.0 { &self.turn_right } else { &self.turn_left };
        let base = turn.sample(self.steer.abs() * TURN_REACH);
        let lean = if self.lean >= 0.0 { &self.forward } else { &self.back };
        let mut layers = vec![
            (lean, self.lean.abs() * LEAN_REACH),
            (&self.crouch, self.preload * CROUCH_REACH),
        ];
        if let Some(t) = self.landing {
            layers.push((&self.crouch, t * ONE_SHOT_FPS / (self.crouch.frames.len() - 1) as f32));
        }
        let layers: Vec<(&Vec<BonePose>, Vec<BonePose>)> =
            layers.iter().map(|(c, t)| (&c.frames[0], c.sample(*t))).collect();
        let stunt = self.stunt.as_ref().filter(|(c, _)| c.frames[0].len() == base.len()).map(|(c, t)| c.sample(*t));
        base.iter()
            .enumerate()
            .map(|(b, p)| {
                let mut rotation = p.rotation;
                let mut position = if b == 0 { p.position } else { self.offsets[b] };
                for (rest, pose) in &layers {
                    rotation = rotation * (rest[b].rotation.inverse() * pose[b].rotation);
                    if b == 0 {
                        position += pose[b].position - rest[b].position;
                    }
                }
                if let Some(s) = &stunt {
                    let w = self.stunt_weight;
                    rotation = rotation.slerp(s[b].rotation, w);
                    if b == 0 {
                        position = position.lerp(s[b].position, w);
                    }
                }
                Transform { translation: position, rotation: rotation.normalize(), scale: Vec3::ONE }
            })
            .collect()
    }
}

pub fn pose_riders(time: Res<Time>, mut riders: Query<&mut Rider>, mut joints: Query<&mut Transform>) {
    for mut rider in &mut riders {
        rider.apply(time.delta_secs(), &mut joints);
    }
}

impl Rider {
    /// Moves the joints to the current pose.
    pub fn apply(&mut self, dt: f32, joints: &mut Query<&mut Transform>) {
        let pose = self.next_pose(dt);
        for (e, t) in self.joints.iter().zip(pose) {
            if let Ok(mut jt) = joints.get_mut(*e) {
                *jt = t;
            }
        }
    }
}
