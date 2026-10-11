//! Time Trial's ghost: the best lap on the track, ridden again beside the
//! player by a see-through quad and rider.
//!
//! As the original's (0x9dd60 onward), every lap is recorded, and a lap
//! faster than the ghost's (or the first one, with no ghost yet) becomes
//! the ghost (0x9e120, 0x9dc50). Each lap the ghost sets off with the
//! player, in step with the lap clock (0x9e090, 0x9de20), and once its
//! lap is over it waits out of sight, 200 m down, as the original parks
//! it. The first lap's ghost starts from the grid, a later lap's from the
//! line.
//!
//! The ghost belongs to the track and is kept between sessions in
//! `ghosts/<track>.ghost` in the config folder (the original keeps it
//! with the game save, 0x9d890/0x9df80), with the rider and quad that set
//! it: a ghost set on another quad comes back on that quad, as the
//! original loads the saved ghost's rider and vehicle (0x9dcd0).
//!
//! What is kept is what was drawn, as replays keep it: the quad, its
//! wheels, the rider's seat and every joint, [`RATE`] times a second, so
//! tricks and crashes come back as they looked.

use std::path::PathBuf;

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use atv_formats::GameSource;
use atv_sim::VehicleParams;

use crate::convert::{self, Loader, WheelVisual};
use crate::opponents::OpponentRider;
use crate::race::RaceProgress;
use crate::rider::Rider;
use crate::{AppState, QuadBody};

/// Samples a second.
pub const RATE: f32 = 30.0;
/// The longest lap kept, in seconds; the original's ghost buffer (0x32000
/// bytes) runs out too.
const LONGEST: f32 = 10.0 * 60.0;
/// Where a ghost waits with nothing to ride (0x9de20 puts it 200 m down).
const PARKED: Vec3 = Vec3::new(0.0, -200.0, 0.0);
/// How opaque the ghost is drawn.
const ALPHA: f32 = 0.4;

/// A ghost's quad root, rider seat and joints, which replays record.
#[derive(Component)]
pub struct Ghostly;

/// A place and a turn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    at: Vec3,
    turn: Quat,
}

impl Pose {
    fn of(t: &Transform) -> Self {
        Self { at: t.translation, turn: t.rotation }
    }

    fn lerp(self, b: Self, f: f32) -> Self {
        Self { at: self.at.lerp(b.at, f), turn: self.turn.slerp(b.turn, f) }
    }
}

/// One moment of a lap, as it was drawn.
#[derive(Clone, Debug, PartialEq)]
struct Frame {
    /// Seconds into the lap.
    t: f32,
    /// The quad (its centre of mass).
    body: Pose,
    /// The rider's seat on the quad, which leaves it in a crash.
    seat: Pose,
    /// Each wheel's height on the quad and its turn, in the params' order
    /// (front left, front right, rear left, rear right).
    wheels: [(f32, Quat); 4],
    /// Where the pelvis is, and every joint's turn.
    pelvis: Vec3,
    joints: Vec<Quat>,
}

impl Frame {
    fn lerp(&self, b: &Frame, f: f32) -> Frame {
        Frame {
            t: self.t + (b.t - self.t) * f,
            body: self.body.lerp(b.body, f),
            seat: self.seat.lerp(b.seat, f),
            wheels: std::array::from_fn(|i| {
                let (a, b) = (self.wheels[i], b.wheels[i]);
                (a.0 + (b.0 - a.0) * f, a.1.slerp(b.1, f))
            }),
            pelvis: self.pelvis.lerp(b.pelvis, f),
            joints: self.joints.iter().zip(&b.joints).map(|(a, b)| a.slerp(*b, f)).collect(),
        }
    }
}

/// A lap the ghost rides.
#[derive(Clone, Debug, PartialEq)]
pub struct Lap {
    pub time: f32,
    /// The rider (model folder) and quad that set it.
    pub rider: String,
    pub quad: String,
    frames: Vec<Frame>,
}

/// File magic and version.
const MAGIC: &[u8; 8] = b"ATV2GHST";
const VERSION: u32 = 1;

impl Lap {
    /// Where everything was `t` seconds into the lap: the first moment
    /// before it starts, and nothing once it is over.
    fn at(&self, t: f32) -> Option<Frame> {
        let i = self.frames.partition_point(|f| f.t <= t);
        match (i.checked_sub(1).map(|j| &self.frames[j]), self.frames.get(i)) {
            (Some(a), Some(b)) => Some(a.lerp(b, ((t - a.t) / (b.t - a.t).max(1e-6)).clamp(0.0, 1.0))),
            (None, Some(b)) => Some(b.clone()),
            _ => None,
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&VERSION.to_le_bytes());
        b.extend_from_slice(&self.time.to_le_bytes());
        for s in [&self.rider, &self.quad] {
            b.extend_from_slice(&(s.len() as u16).to_le_bytes());
            b.extend_from_slice(s.as_bytes());
        }
        let joints = self.frames.first().map_or(0, |f| f.joints.len());
        b.extend_from_slice(&(joints as u16).to_le_bytes());
        b.extend_from_slice(&(self.frames.len() as u32).to_le_bytes());
        let vec3 = |b: &mut Vec<u8>, v: Vec3| v.to_array().iter().for_each(|c| b.extend_from_slice(&c.to_le_bytes()));
        // Turns as four 16-bit fractions: a ghost's joints are most of it.
        let quat = |b: &mut Vec<u8>, q: Quat| {
            q.to_array().iter().for_each(|c| b.extend_from_slice(&((c.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes()))
        };
        for f in &self.frames {
            b.extend_from_slice(&f.t.to_le_bytes());
            for p in [f.body, f.seat] {
                vec3(&mut b, p.at);
                quat(&mut b, p.turn);
            }
            for (h, q) in f.wheels {
                b.extend_from_slice(&h.to_le_bytes());
                quat(&mut b, q);
            }
            vec3(&mut b, f.pelvis);
            for j in 0..joints {
                quat(&mut b, f.joints.get(j).copied().unwrap_or(Quat::IDENTITY));
            }
        }
        b
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader(bytes);
        if r.take(8)? != MAGIC || r.u32()? != VERSION {
            return None;
        }
        let time = r.f32()?;
        let rider = r.string()?;
        let quad = r.string()?;
        let joints = r.u16()? as usize;
        let count = r.u32()? as usize;
        let mut frames = Vec::with_capacity(count.min(bytes.len()));
        for _ in 0..count {
            let t = r.f32()?;
            let body = Pose { at: r.vec3()?, turn: r.quat()? };
            let seat = Pose { at: r.vec3()?, turn: r.quat()? };
            let mut wheels = [(0.0, Quat::IDENTITY); 4];
            for w in &mut wheels {
                *w = (r.f32()?, r.quat()?);
            }
            let pelvis = r.vec3()?;
            let joints = (0..joints).map(|_| r.quat()).collect::<Option<Vec<_>>>()?;
            frames.push(Frame { t, body, seat, wheels, pelvis, joints });
        }
        Some(Lap { time, rider, quad, frames })
    }
}

/// Reads a ghost file.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (a, b) = self.0.split_at_checked(n)?;
        self.0 = b;
        Some(a)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn string(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }

    fn vec3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    fn quat(&mut self) -> Option<Quat> {
        let mut c = [0.0; 4];
        for c in &mut c {
            *c = i16::from_le_bytes(self.take(2)?.try_into().ok()?) as f32 / 32767.0;
        }
        Some(Quat::from_array(c).normalize())
    }
}

/// A ghost's quad and rider in the world.
struct Model {
    rider: String,
    quad: String,
    root: Entity,
    seat: Entity,
    joints: Vec<Entity>,
}

/// Time Trial's ghost.
#[derive(Resource)]
pub struct Ghost {
    track: String,
    /// The player's rider and quad.
    rider: String,
    quad: String,
    /// The lap the ghost rides, once there is one.
    pub best: Option<Lap>,
    /// The lap being driven, so far.
    tape: Vec<Frame>,
    /// Laps done when it started.
    laps: usize,
    /// The player's rider on their quad first, then the saved ghost's
    /// when it was set on others.
    models: Vec<Model>,
}

impl Ghost {
    fn path(track: &str) -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "atv2").map(|d| d.config_dir().join("ghosts").join(format!("{}.ghost", track.to_ascii_lowercase())))
    }

    /// The track's saved ghost, if it has one.
    fn load(track: &str) -> Option<Lap> {
        let bytes = std::fs::read(Self::path(track)?).ok()?;
        let lap = Lap::from_bytes(&bytes);
        if lap.is_none() {
            warn!("ghost for {track} unreadable");
        }
        lap
    }

    fn save(&self) {
        let (Some(path), Some(lap)) = (Self::path(&self.track), &self.best) else { return };
        let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, lap.to_bytes()));
        match saved {
            Ok(()) => info!("ghost: {:.2} s lap saved to {}", lap.time, path.display()),
            Err(e) => warn!("couldn't save the ghost to {}: {e}", path.display()),
        }
    }

    /// The model to show the ghost lap on: the one with its rider and
    /// quad, else the player's.
    fn model(&self) -> Option<&Model> {
        let lap = self.best.as_ref()?;
        let same = |m: &&Model| m.rider.eq_ignore_ascii_case(&lap.rider) && m.quad.eq_ignore_ascii_case(&lap.quad);
        self.models.iter().find(same).or(self.models.first())
    }
}

/// Loads the track's ghost and puts out its models, parked until they
/// ride.
pub fn spawn(
    commands: &mut Commands,
    loader: &mut Loader,
    bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    game: &GameSource,
    (track, rider, quad): (&str, &str, &str),
) {
    let best = Ghost::load(track);
    let mut wanted = vec![(rider.to_string(), quad.to_string())];
    if let Some(lap) = best.as_ref().filter(|l| !(l.rider.eq_ignore_ascii_case(rider) && l.quad.eq_ignore_ascii_case(quad))) {
        wanted.push((lap.rider.clone(), lap.quad.clone()));
    }
    loader.ghost = Some(ALPHA);
    let models = wanted.into_iter().filter_map(|(r, q)| spawn_model(commands, loader, bindposes, game, r, q)).collect();
    loader.ghost = None;
    if let Some(lap) = &best {
        info!("ghost: {:.2} s lap by {} on {}", lap.time, lap.rider, lap.quad);
    }
    commands.insert_resource(Ghost { track: track.to_string(), rider: rider.to_string(), quad: quad.to_string(), best, tape: Vec::new(), laps: 0, models });
}

fn spawn_model(commands: &mut Commands, loader: &mut Loader, bindposes: &mut Assets<SkinnedMeshInverseBindposes>, game: &GameSource, rider: String, quad: String) -> Option<Model> {
    // The wheels sit as wide as the quad's own params put them.
    let generic = game.read("/Cars/Generic/Generic.params").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let own = game.read(&format!("/Cars/{quad}/{quad}.params")).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
    let wheel_x: Vec<f32> = VehicleParams::from_files(&generic, own.as_deref()).wheels.iter().map(|w| w.offset[0]).collect();
    let root = match loader.spawn_quad(commands, &quad, Transform::from_translation(PARKED), &wheel_x) {
        Ok(e) => e,
        Err(e) => {
            warn!("ghost: no quad {quad}: {e:#}");
            return None;
        }
    };
    commands.entity(root).insert((Ghostly, Name::new("ghost"), DespawnOnExit(AppState::Race)));
    let seat = commands.spawn((Transform::from_xyz(0.0, -convert::COM_HEIGHT, 0.0), Visibility::default(), Ghostly)).id();
    commands.entity(root).add_child(seat);
    let joints = match loader.spawn_rider(commands, bindposes, &rider, seat) {
        Ok(rig) => rig.joints,
        Err(e) => {
            warn!("ghost: no rider {rider}: {e:#}");
            Vec::new()
        }
    };
    for &j in &joints {
        commands.entity(j).insert(Ghostly);
    }
    Some(Model { rider, quad, root, seat, joints })
}

pub struct GhostPlugin;

impl Plugin for GhostPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnExit(AppState::Race), |mut commands: Commands| commands.remove_resource::<Ghost>()).add_systems(
            Update,
            ride.after(crate::rider::pose_riders)
                .run_if(resource_exists::<Ghost>)
                .run_if(not(resource_exists::<crate::replay::Playback>))
                .run_if(in_state(AppState::Race)),
        );
    }
}

/// Records the player's lap, keeps the fastest as the ghost, and puts
/// the ghost where it was at this point of its lap.
#[allow(clippy::type_complexity)]
fn ride(
    progress: Option<Res<RaceProgress>>,
    mut ghost: ResMut<Ghost>,
    quads: Query<Entity, With<QuadBody>>,
    riders: Query<(Entity, &Rider), Without<OpponentRider>>,
    wheels: Query<(Entity, &WheelVisual)>,
    mut transforms: Query<&mut Transform>,
) {
    let Some(p) = progress else { return };
    let g = ghost.as_mut();
    if p.lap_times.len() < g.laps {
        g.laps = 0;
        g.tape.clear();
    }
    // A lap done: faster than the ghost's, it is the ghost now.
    while g.laps < p.lap_times.len() {
        let time = p.lap_times[g.laps];
        g.laps += 1;
        let frames = std::mem::take(&mut g.tape);
        if !frames.is_empty() && g.best.as_ref().is_none_or(|b| time < b.time) {
            g.best = Some(Lap { time, rider: g.rider.clone(), quad: g.quad.clone(), frames });
            g.save();
        }
    }
    let t = p.lap_time();
    let due = g.tape.last().is_none_or(|f| t - f.t >= 1.0 / RATE - 1e-4);
    if !p.over() && due && t <= LONGEST {
        let quad = quads.single().ok();
        if let Some(f) = quad.and_then(|q| capture(t, q, riders.iter().next(), &wheels, &transforms)) {
            g.tape.push(f);
        }
    }
    let frame = if p.over() { None } else { g.best.as_ref().and_then(|b| b.at(t)) };
    let shown = g.model().map(|m| m.root);
    for m in &g.models {
        match frame.as_ref().filter(|_| Some(m.root) == shown) {
            Some(f) => pose(f, m, &wheels, &mut transforms),
            None => {
                if let Ok(mut t) = transforms.get_mut(m.root) {
                    *t = Transform::from_translation(PARKED);
                }
            }
        }
    }
}

/// The player's quad and rider as drawn now, `t` seconds into the lap.
fn capture(t: f32, quad: Entity, rider: Option<(Entity, &Rider)>, wheels: &Query<(Entity, &WheelVisual)>, transforms: &Query<&mut Transform>) -> Option<Frame> {
    let body = Pose::of(transforms.get(quad).ok()?);
    let mut frame = Frame {
        t,
        body,
        seat: Pose { at: Vec3::new(0.0, -convert::COM_HEIGHT, 0.0), turn: Quat::IDENTITY },
        wheels: [(0.0, Quat::IDENTITY); 4],
        pelvis: Vec3::ZERO,
        joints: Vec::new(),
    };
    for (e, w) in wheels.iter().filter(|(_, w)| w.owner == quad) {
        if let (Some(slot), Ok(wt)) = (w.wheels.first().and_then(|&i| frame.wheels.get_mut(i)), transforms.get(e)) {
            *slot = (wt.translation.y, wt.rotation);
        }
    }
    if let Some((seat, r)) = rider {
        if let Ok(st) = transforms.get(seat) {
            frame.seat = Pose::of(st);
        }
        let joints: Vec<&Transform> = r.joints().iter().filter_map(|&j| transforms.get(j).ok()).collect();
        frame.pelvis = joints.first().map_or(Vec3::ZERO, |j| j.translation);
        frame.joints = joints.iter().map(|j| j.rotation).collect();
    }
    Some(frame)
}

/// Puts a ghost model where the frame has everything.
fn pose(f: &Frame, m: &Model, wheels: &Query<(Entity, &WheelVisual)>, transforms: &mut Query<&mut Transform>) {
    if let Ok(mut t) = transforms.get_mut(m.root) {
        *t = Transform::from_translation(f.body.at).with_rotation(f.body.turn);
    }
    if let Ok(mut t) = transforms.get_mut(m.seat) {
        *t = Transform::from_translation(f.seat.at).with_rotation(f.seat.turn);
    }
    for (e, w) in wheels.iter().filter(|(_, w)| w.owner == m.root) {
        if let (Some(&(h, q)), Ok(mut t)) = (w.wheels.first().and_then(|&i| f.wheels.get(i)), transforms.get_mut(e)) {
            t.translation.y = h;
            t.rotation = q;
        }
    }
    // Other riders share the skeleton; each keeps its own bone lengths.
    for (i, (&e, q)) in m.joints.iter().zip(&f.joints).enumerate() {
        if let Ok(mut t) = transforms.get_mut(e) {
            t.rotation = *q;
            if i == 0 {
                t.translation = f.pelvis;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(t: f32, x: f32) -> Frame {
        Frame {
            t,
            body: Pose { at: Vec3::new(x, 1.0, -2.0), turn: Quat::from_rotation_y(x * 0.1) },
            seat: Pose { at: Vec3::new(0.0, -convert::COM_HEIGHT, 0.0), turn: Quat::IDENTITY },
            wheels: [(0.1, Quat::from_rotation_x(x)); 4],
            pelvis: Vec3::new(0.0, 0.5, x * 0.01),
            joints: vec![Quat::from_rotation_z(x * 0.2); 3],
        }
    }

    fn lap() -> Lap {
        Lap { time: 1.0, rider: "Bandit".into(), quad: "Honda".into(), frames: (0..31).map(|i| frame(i as f32 / RATE, i as f32)).collect() }
    }

    #[test]
    fn rides_in_step_with_the_lap() {
        let lap = lap();
        // Before the start it waits at the first moment, and part way
        // between samples it is part way.
        assert_eq!(lap.at(-1.0).unwrap().body.at.x, 0.0);
        let f = lap.at(2.5 / RATE).unwrap();
        assert!((f.body.at.x - 2.5).abs() < 1e-4);
        assert!((f.t - 2.5 / RATE).abs() < 1e-6);
        // Its lap over, it has nowhere to be.
        assert!(lap.at(1.5).is_none());
    }

    #[test]
    fn saved_ghost_reads_back() {
        let lap = lap();
        let back = Lap::from_bytes(&lap.to_bytes()).unwrap();
        assert_eq!((back.time, back.rider.as_str(), back.quad.as_str(), back.frames.len()), (1.0, "Bandit", "Honda", 31));
        for (a, b) in lap.frames.iter().zip(&back.frames) {
            assert_eq!((a.t, a.body.at, a.pelvis), (b.t, b.body.at, b.pelvis));
            assert!(a.body.turn.dot(b.body.turn).abs() > 0.9999);
            assert!(a.joints.iter().zip(&b.joints).all(|(x, y)| x.dot(*y).abs() > 0.9999));
        }
        // Anything else isn't a ghost.
        assert!(Lap::from_bytes(b"ATV2GHST").is_none());
        assert!(Lap::from_bytes(&lap.to_bytes()[..100]).is_none());
    }
}
