//! The race intro: before the countdown, the camera flies over the track
//! in a shot or two and down onto the grid (the original's intro state,
//! 0x7dd80 and 0x7e750).
//!
//! The shots are the track's intro cameras in `World/<Track>.xml`: each
//! `Camera` with `CameraType` 4 ("P: Intro", 0x6d290; loaded by 0x73180),
//! played in `ID` order from 0 (0x6ac70 cuts to the next ID once a shot is
//! done; Glacier 1 has no ID 1, and is cut from 0 to 2). Each sets off from
//! where its camera stands, curves onto its `Path Node` (`NextGUID`) and
//! follows the path to the end, looking at the `Camera Focus Point` whose
//! `mIndex` is the camera's `FocusIndex` (with no focus, `FocusIndex` 0, it
//! keeps its own `Matrix`'s heading). It picks up speed as it goes
//! (0x6cf00): 6.7 m/s every second from a standstill, up to 44.7 m/s
//! (100 mph). The lead-in is a cubic from the camera to the path's start
//! that meets the path along its own direction (0x6c590, 0x6c890); the path
//! is then followed by distance (0x6ca30). A camera without a path holds
//! still for 10 seconds.
//!
//! While it plays, the race is held on the grid, the HUD is hidden and in
//! split screen player 1's view fills the screen (0x83010); the countdown
//! starts after the last shot (0x7e750, 0x83730). There is no skipping it.
//! Tracks without intro cameras (Swamp 3 and the Stadium 2 arena) go
//! straight to the countdown.
//!
//! The original resamples each path every `mFixedNodeSep` metres and fits
//! a spline through those points (0x77e00, 0x77fb0); here a Catmull-Rom
//! curve through the nodes stands in for it.

use bevy::camera::visibility::VisibilitySystems;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use atv_sim::track as trk;

use crate::opponents::Opponents;
use crate::race::RaceProgress;
use crate::rider::Rider;
use crate::ui::TextLayout;
use crate::{AppState, Args, ChaseCam, Game, Selection};

/// How fast the camera picks up speed, m/s².
const ACCEL: f32 = 6.704_167;
/// Its top speed, m/s.
const TOP_SPEED: f32 = 44.694;
/// How long a camera without a path holds its shot, seconds.
const STILL: f32 = 10.0;
/// The intro cameras' IDs run below this (0x72fb0's table).
const SLOTS: u32 = 10;
/// Points a path segment is cut into.
const STEPS: usize = 16;

/// A path as a run of short straight pieces, and the distance along it to
/// each point.
#[derive(Clone, Debug, PartialEq)]
struct Path {
    points: Vec<Vec3>,
    along: Vec<f32>,
}

impl Path {
    /// A Catmull-Rom curve through `nodes`.
    fn through(nodes: &[Vec3]) -> Self {
        let mut points = vec![nodes[0]];
        for i in 0..nodes.len() - 1 {
            let p = |j: isize| nodes[(i as isize + j).clamp(0, nodes.len() as isize - 1) as usize];
            let (p0, p1, p2, p3) = (p(-1), p(0), p(1), p(2));
            for k in 1..=STEPS {
                let t = k as f32 / STEPS as f32;
                let (t2, t3) = (t * t, t * t * t);
                points.push(0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3));
            }
        }
        let mut along = vec![0.0];
        for w in points.windows(2) {
            along.push(along.last().copied().unwrap_or(0.0) + w[0].distance(w[1]));
        }
        Self { points, along }
    }

    fn length(&self) -> f32 {
        self.along.last().copied().unwrap_or(0.0)
    }

    /// The point `s` metres along.
    fn at(&self, s: f32) -> Vec3 {
        let i = self.along.partition_point(|&a| a <= s).clamp(1, self.points.len().max(2) - 1);
        let Some(&b) = self.points.get(i) else { return self.points[0] };
        let (a, span) = (self.points[i - 1], self.along[i] - self.along[i - 1]);
        if span <= 0.0 { b } else { a.lerp(b, ((s - self.along[i - 1]) / span).clamp(0.0, 1.0)) }
    }

    /// The way it sets off.
    fn heading(&self) -> Vec3 {
        let start = self.points[0];
        self.points.iter().find(|p| p.distance_squared(start) > 1e-6).map_or(Vec3::ZERO, |p| (*p - start).normalize())
    }
}

/// What a shot looks at.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Aim {
    At(Vec3),
    /// No focus point: the camera's own heading.
    Fixed(Quat),
}

/// One intro camera: where it flies and what it looks at, in the race's
/// coordinates.
#[derive(Clone, Debug, PartialEq)]
struct Flight {
    start: Vec3,
    path: Option<Path>,
    aim: Aim,
}

impl Flight {
    /// The intro cameras of a `World/<Track>.xml`, in the order they play.
    fn parse_all(xml: &str) -> Vec<Self> {
        let attr = |tag: &str, key: &str| tag.split(&format!(" {key}=\"")).nth(1).and_then(|r| r.split('"').next()).map(str::trim).map(str::to_owned);
        let floats = |body: &str, tag: &str, keys: &[&str]| -> Option<Vec<f32>> {
            let at = body.find(&format!("<{tag} "))?;
            let t = &body[at..];
            let t = &t[..t.find('>')?];
            keys.iter().map(|k| attr(t, k)?.parse::<f32>().ok()).collect()
        };
        let vec = |body: &str, tag: &str| floats(body, tag, &["X", "Y", "Z"]).map(|v| trk::to_sim([v[0], v[1], v[2]]));
        let starts: Vec<usize> =
            xml.match_indices("<Object").map(|(i, _)| i).filter(|&i| xml[i + 7..].starts_with(|c: char| c.is_ascii_digit())).collect();
        let objects: Vec<(&str, &str)> = starts
            .iter()
            .enumerate()
            .filter_map(|(n, &start)| {
                let body = &xml[start..starts.get(n + 1).copied().unwrap_or(xml.len())];
                Some((&body[..body.find('>')?], body))
            })
            .collect();
        let kind = |head: &str, k: &str| attr(head, "ManipulatorType").as_deref() == Some(k);
        let mut cams: Vec<(u32, Flight)> = objects
            .iter()
            .filter(|(h, _)| kind(h, "Camera") && matches!(attr(h, "CameraType").as_deref(), Some("4" | "5" | "6")))
            .filter_map(|(cam, body)| {
                let id = attr(cam, "ID")?.parse::<u32>().ok().filter(|id| *id < SLOTS)?;
                let start = vec(body, "Position")?;
                let path = match attr(cam, "NextGUID").as_deref() {
                    None | Some("0") => None,
                    // A path that isn't there ends the shot at once (0x6c590).
                    Some(next) => {
                        let (head, body) = objects.iter().find(|(h, _)| kind(h, "Path Node") && attr(h, "GUID").as_deref() == Some(next))?;
                        let count = attr(head, "NumNodes").and_then(|n| n.parse::<usize>().ok()).unwrap_or(1).max(1);
                        let nodes: Vec<Vec3> =
                            std::iter::once(vec(body, "Position")).chain((1..count).map(|i| vec(body, &format!("NodePos{i}")))).collect::<Option<_>>()?;
                        Some(Path::through(&nodes))
                    }
                };
                let index = attr(cam, "FocusIndex").filter(|i| i != "0");
                let focus = index.and_then(|i| {
                    objects.iter().find(|(h, _)| kind(h, "Camera Focus Point") && attr(h, "mIndex").as_deref() == Some(i.as_str())).and_then(|(_, b)| vec(b, "Position"))
                });
                let aim = match focus {
                    Some(f) => Aim::At(f),
                    None => {
                        let m = ["Row0", "Row1", "Row2"].map(|r| floats(body, r, &["X", "Y", "Z"]).unwrap_or_default());
                        let m: Vec<f32> = m.concat();
                        Aim::Fixed(<[f32; 9]>::try_from(m).map_or(Quat::IDENTITY, |m| trk::rotation_to_sim(&m)))
                    }
                };
                Some((id, Flight { start, path, aim }))
            })
            .collect();
        cams.sort_by_key(|c| c.0);
        cams.dedup_by_key(|c| c.0);
        cams.into_iter().map(|c| c.1).collect()
    }

    /// The camera at `eye`.
    fn view(&self, eye: Vec3) -> Transform {
        match self.aim {
            Aim::At(focus) => Transform::from_translation(eye).looking_at(focus, Vec3::Y),
            Aim::Fixed(rotation) => Transform { translation: eye, rotation, ..default() },
        }
    }
}

/// A camera on its way.
#[derive(Clone, Debug)]
struct Motion {
    speed: f32,
    clock: f32,
    /// How far through the lead-in (0 to 1), then metres along the path.
    lead: f32,
    along: Option<f32>,
    done: bool,
    /// The lead-in, a cubic in `lead`: start + a·u + b·u² + c·u³; and the
    /// straight distance it covers.
    cubic: [Vec3; 3],
    span: f32,
}

impl Motion {
    fn new(f: &Flight) -> Self {
        let mut m = Self { speed: 0.0, clock: 0.0, lead: 0.0, along: None, done: false, cubic: [Vec3::ZERO; 3], span: 0.0 };
        if let Some(path) = &f.path {
            let (p0, p1) = (f.start, path.points[0]);
            m.span = p0.distance(p1);
            // Both ends leave along the path's own way, as long as the gap.
            let t = path.heading() * m.span;
            m.cubic = [t, 3.0 * (p1 - p0) - 3.0 * t, 2.0 * (p0 - p1) + 2.0 * t];
            m.along = (m.span < 0.01).then_some(0.0);
        }
        m
    }

    /// Moves the camera on by `dt` seconds; where it is now.
    fn step(&mut self, f: &Flight, dt: f32) -> Vec3 {
        self.clock += dt;
        let Some(path) = &f.path else {
            self.done = self.clock >= STILL;
            return f.start;
        };
        self.speed = (self.speed + ACCEL * dt).min(TOP_SPEED);
        match self.along.as_mut() {
            None => {
                self.lead += self.speed / self.span * dt;
                if self.lead >= 1.0 {
                    // Over the end: the rest of the step carries on along
                    // the path.
                    let s = ((self.lead - 1.0) * self.span).min(path.length());
                    self.along = Some(s);
                    return path.at(s);
                }
                let u = self.lead;
                let [a, b, c] = self.cubic;
                f.start + a * u + b * u * u + c * u * u * u
            }
            Some(s) => {
                *s = (*s + self.speed * dt).min(path.length());
                self.done = *s >= path.length();
                path.at(*s)
            }
        }
    }
}

/// The intro is playing: the race waits for it.
#[derive(Resource)]
pub struct Intro {
    shots: Vec<Flight>,
    shot: usize,
    motion: Motion,
    /// The screens hidden for it, and how they were.
    hidden: Vec<(Entity, Visibility)>,
}

impl Intro {
    fn new(shots: Vec<Flight>) -> Option<Self> {
        let motion = Motion::new(shots.first()?);
        Some(Self { shots, shot: 0, motion, hidden: Vec::new() })
    }

    /// Plays on by `dt` seconds: the camera now, or `None` after the last
    /// shot.
    fn step(&mut self, dt: f32) -> Option<Transform> {
        let flight = self.shots.get(self.shot)?;
        let eye = self.motion.step(flight, dt);
        let view = flight.view(eye);
        if self.motion.done {
            // Cut to the next camera, which starts from rest (0x6ac70).
            self.shot += 1;
            if let Some(next) = self.shots.get(self.shot) {
                self.motion = Motion::new(next);
            }
        }
        Some(view)
    }
}

/// Shows during the intro (the attract demo's page).
#[derive(Component)]
pub struct ShowInIntro;

pub struct IntroPlugin;

impl Plugin for IntroPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Race), start)
            .add_systems(FixedUpdate, hold.before(crate::drive).run_if(resource_exists::<Intro>).run_if(in_state(AppState::Race)))
            .add_systems(
                PostUpdate,
                fly.before(TransformSystems::Propagate)
                    .before(VisibilitySystems::VisibilityPropagate)
                    .before(TextLayout)
                    .run_if(resource_exists::<Intro>)
                    .run_if(in_state(AppState::Race)),
            )
            .add_systems(OnExit(AppState::Race), |mut commands: Commands| commands.remove_resource::<Intro>());
    }
}

/// Loads the track's intro cameras, if it has any.
fn start(mut commands: Commands, game: Res<Game>, sel: Res<Selection>, args: Res<Args>) {
    commands.remove_resource::<Intro>();
    if args.no_intro || args.camera.is_some() || args.orbit.is_some() {
        return;
    }
    let xml = game.0.read(&format!("/Levels/{0}/World/{0}.xml", sel.track)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let Some(intro) = Intro::new(Flight::parse_all(&xml)) else { return };
    info!("intro: {} shots", intro.shots.len());
    commands.insert_resource(intro);
}

/// Keeps everyone on the grid, the countdown not yet started.
pub(crate) fn hold(mut progress: Option<ResMut<RaceProgress>>, mut opponents: Option<ResMut<Opponents>>) {
    if let Some(p) = progress.as_mut() {
        p.countdown = RaceProgress::COUNTDOWN;
    }
    for r in opponents.iter_mut().flat_map(|o| o.racers.iter_mut()) {
        r.progress.countdown = RaceProgress::COUNTDOWN;
    }
}

/// Flies the camera; after the last shot puts the race camera, the HUD and
/// the split-screen views back.
#[allow(clippy::type_complexity)]
pub(crate) fn fly(
    mut commands: Commands,
    time: Res<Time>,
    mut intro: ResMut<Intro>,
    mut cams: Query<(&mut Transform, &mut Camera, Option<&crate::mp::Split>), With<ChaseCam>>,
    mut screens: Query<(Entity, &mut Visibility), (With<Node>, Without<ChildOf>, With<DespawnOnExit<AppState>>, Without<ShowInIntro>)>,
    mut riders: Query<&mut Visibility, (With<Rider>, Without<Node>)>,
) {
    let intro = &mut *intro;
    // Split screen: the other players' views.
    let slot = |split: Option<&crate::mp::Split>| split.map_or(0, |s| s.0);
    let view = cams.iter().any(|(_, _, s)| slot(s) == 0).then(|| intro.step(time.delta_secs())).flatten();
    let Some(view) = view else {
        for (e, was) in intro.hidden.drain(..) {
            if let Ok((_, mut v)) = screens.get_mut(e) {
                *v = was;
            }
        }
        for (_, mut cam, split) in &mut cams {
            if slot(split) > 0 {
                cam.is_active = true;
            }
        }
        info!("intro: over");
        commands.remove_resource::<Intro>();
        return;
    };
    for (mut t, mut cam, split) in &mut cams {
        if slot(split) > 0 {
            cam.is_active = false;
            continue;
        }
        *t = view;
        // Player 1's view fills the screen while the others wait.
        if split.is_some() {
            cam.viewport = None;
        }
    }
    for (e, mut v) in &mut screens {
        if !intro.hidden.iter().any(|h| h.0 == e) {
            intro.hidden.push((e, *v));
            *v = Visibility::Hidden;
        }
    }
    // Every rider shows, whatever the race camera hid.
    for mut v in &mut riders {
        v.set_if_neq(Visibility::Inherited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: &str = r#"<Root><Objects NObjects="6">
    <Object0 NextGUID="0" ObjectType="33" GUID="0" ManipulatorType="Camera Focus Point" PlatformMask="7" mTrackVelFrac="0.99" mIndex="2">
      <Position X="500.0" Y="0.0" Z="0.0" /></Object0>
    <Object1 NextGUID="33" ObjectType="1" GUID="32" ManipulatorType="Camera" PlatformMask="7" CameraType="4" TestVar="0" CameraZoom="1.0" HeatHaze="0" ID="2" FocusIndex="0">
      <Position X="0.0" Y="10.0" Z="-20.0" />
      <Matrix>
        <Row0 X="1.0" Y="0.0" Z="0.0" />
        <Row1 X="0.0" Y="1.0" Z="0.0" />
        <Row2 X="0.0" Y="0.0" Z="1.0" /></Matrix>
      <Scale X="1.0" Y="1.0" Z="1.0" /></Object1>
    <Object2 mType="0" NumNodes="3" mFixedNodeSep="5.0" NextGUID="0" ManipulatorType="Path Node" PlatformMask="7" ObjectType="5" GUID="33">
      <Position X="0.0" Y="10.0" Z="0.0" />
      <Scale X="1.0" Y="1.0" Z="1.0" />
      <NodePos1 X="0.0" Y="5.0" Z="30.0" />
      <NodePos2 X="0.0" Y="2.0" Z="60.0" /></Object2>
    <Object3 NextGUID="0" ObjectType="33" GUID="0" ManipulatorType="Camera Focus Point" PlatformMask="7" mTrackVelFrac="0.99" mIndex="1">
      <Position X="0.0" Y="1.0" Z="70.0" /></Object3>
    <Object4 NextGUID="0" ObjectType="1" GUID="40" ManipulatorType="Camera" PlatformMask="7" CameraType="4" TestVar="0" CameraZoom="1.0" HeatHaze="0" ID="0" FocusIndex="1">
      <Position X="5.0" Y="6.0" Z="7.0" /></Object4>
    <Object5 NextGUID="0" ObjectType="1" GUID="41" ManipulatorType="Camera" PlatformMask="7" CameraType="2" TestVar="0" CameraZoom="1.0" HeatHaze="0" ID="1" FocusIndex="1">
      <Position X="5.0" Y="6.0" Z="7.0" /></Object5>
</Objects></Root>"#;

    #[test]
    fn reads_the_intro_cameras_in_order() {
        let shots = Flight::parse_all(WORLD);
        // The trackside camera (type 2) isn't one; ID 0 plays before ID 2.
        assert_eq!(shots.len(), 2);
        // ID 0 has no path: a still shot of its focus. The game's Z is flipped.
        assert_eq!(shots[0].start, Vec3::new(5.0, 6.0, -7.0));
        assert!(shots[0].path.is_none());
        assert_eq!(shots[0].aim, Aim::At(Vec3::new(0.0, 1.0, -70.0)));
        // ID 2 flies its path with no focus, facing its own +Z.
        let f = &shots[1];
        assert_eq!(f.start, Vec3::new(0.0, 10.0, 20.0));
        let path = f.path.as_ref().unwrap();
        assert_eq!(path.points[0], Vec3::new(0.0, 10.0, 0.0));
        assert!(path.points.last().unwrap().distance(Vec3::new(0.0, 2.0, -60.0)) < 1e-4);
        // The curve passes through the middle node.
        assert!(path.points.iter().any(|p| p.distance(Vec3::new(0.0, 5.0, -30.0)) < 1e-4));
        assert!((path.length() - 60.0).abs() < 1.0, "{}", path.length());
        let facing = f.view(Vec3::ZERO).rotation * Vec3::NEG_Z;
        assert!(facing.distance(Vec3::NEG_Z) < 1e-4, "{facing}");
    }

    #[test]
    fn flies_onto_the_path_and_to_its_end() {
        let path = Path::through(&[Vec3::new(0.0, 10.0, 0.0), Vec3::new(0.0, 10.0, -50.0)]);
        let f = Flight { start: Vec3::new(10.0, 10.0, 0.0), path: Some(path.clone()), aim: Aim::At(Vec3::ZERO) };
        let mut m = Motion::new(&f);
        let dt = 1.0 / 60.0;
        let mut last = f.start;
        let mut before = f.start;
        let mut t = 0.0;
        let mut joined = None;
        while !m.done && t < 30.0 {
            let p = m.step(&f, dt);
            // Smooth all the way: no step much longer than the speed allows.
            assert!(p.distance(last) <= TOP_SPEED * dt * 1.6 + 1e-3, "jump at {t}");
            if joined.is_none() && m.along.is_some() {
                joined = Some(p);
                before = last;
            }
            last = p;
            t += dt;
        }
        assert!(joined.unwrap().distance(path.points[0]) < 0.5);
        // The lead-in arrives along the path's way, heading down -Z.
        assert!((path.points[0] - before).normalize().z < -0.9);
        assert!(last.distance(Vec3::new(0.0, 10.0, -50.0)) < 1e-3);
        // 10 m of lead-in and 50 m of path from a standstill at 6.7 m/s².
        let expect = (2.0 * 60.0 / ACCEL).sqrt();
        assert!((t - expect).abs() < 0.1, "{t} vs {expect}");
    }

    #[test]
    fn plays_each_shot_then_ends() {
        let mut intro = Intro::new(Flight::parse_all(WORLD)).unwrap();
        let dt = 0.1;
        let first = intro.step(dt).unwrap();
        assert_eq!(first.translation, Vec3::new(5.0, 6.0, -7.0));
        // The still shot holds for ten seconds, then the second starts
        // from its own camera, at rest.
        let mut n = 1;
        while intro.shot == 0 {
            intro.step(dt).unwrap();
            n += 1;
        }
        assert!((n as f32 * dt - STILL).abs() < 0.15, "{n}");
        let second = intro.step(dt).unwrap();
        assert!(second.translation.distance(Vec3::new(0.0, 10.0, 20.0)) < 0.1);
        let mut steps = 0;
        while intro.step(dt).is_some() {
            steps += 1;
            assert!(steps < 1000);
        }
        assert!(Intro::new(Vec::new()).is_none());
    }
}
