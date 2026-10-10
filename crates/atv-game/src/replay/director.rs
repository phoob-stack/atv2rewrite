//! The replay's cameras: the original's director (`Setup/CameraDirector.xml`)
//! cutting between the track's trackside cameras and cameras that follow
//! the rider or ride on the quad.
//!
//! As in the original (0x72be0), a shot lasts at least [`MIN_SHOT`] and
//! longer than its threshold, and the cut waits until the watched rider has
//! all four wheels down and is still on the quad. Cuts alternate between
//! the style's primary camera and a secondary picked by stepping one to
//! three places along the secondaries' list (0x6ede0). Two quirks of the
//! original are kept. Every shot is held for the primary's threshold,
//! because the original never changes which camera's threshold it reads.
//! And the last secondary is never loaded, because `mNumCameras` counts one
//! short of AllCameras' list, so On Front never shows.
//!
//! Trackside cameras come from the track's `World/<Track>.xml`: each camera
//! box names the camera that films a rider inside it, and the first box
//! holding the rider wins (0x6b1b0). Outside every box, the shot drops to
//! the quad's camera until the next cut.

use std::f32::consts::{FRAC_PI_4, PI, TAU};

use bevy::prelude::*;

use atv_formats::{setup, GameSource};
use atv_sim::track as trk;
use atv_sim::TrackCollision;

use crate::settings::{CameraView, FollowCam};

/// The race camera's field of view (Bevy's default, which the race uses).
pub const FOV: f32 = FRAC_PI_4;

/// A racer's state at a moment, as [`Scene::flags`] reports it.
pub const GROUNDED: u8 = 1;
pub const ALL_DOWN: u8 = 2;
pub const CRASHED: u8 = 4;

/// The shortest shot, in seconds.
const MIN_SHOT: f32 = 3.0;
/// The general mode watches one rider at least this long (0x72d00).
const MIN_WATCH: f32 = 4.0;
/// Half the size of a quad and rider, which the zoom cameras frame. The
/// original measures the rider's model; this is a typical size.
const RIDER_HALF_SIZE: f32 = 1.4;
/// Where on the quad the trackside, orbit and two-rider cameras aim.
const AIM_HEIGHT: f32 = 0.6;

/// The original's camera types (the table at 0x1d0990).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Trackside,
    FollowHigh,
    Orbit,
    FollowFar,
    TwoRider,
    Wheel,
    Handlebars,
    OnFront,
    LowBehind,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Trackside,
        Kind::FollowHigh,
        Kind::Orbit,
        Kind::FollowFar,
        Kind::TwoRider,
        Kind::Wheel,
        Kind::Handlebars,
        Kind::OnFront,
        Kind::LowBehind,
    ];

    /// As `CameraDirector.xml` names them.
    fn parse(name: &str) -> Option<Self> {
        Some(match name.trim() {
            "Trackside" => Kind::Trackside,
            "Follow High" => Kind::FollowHigh,
            "Orbit Player" => Kind::Orbit,
            "Follow Far Behind" => Kind::FollowFar,
            "Two Rider" => Kind::TwoRider,
            "Wheel" => Kind::Wheel,
            "Handlebars" => Kind::Handlebars,
            "On Front" => Kind::OnFront,
            "Low Behind" => Kind::LowBehind,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Trackside => "TRACKSIDE",
            Kind::FollowHigh => "FOLLOW HIGH",
            Kind::Orbit => "ORBIT",
            Kind::FollowFar => "FOLLOW FAR BEHIND",
            Kind::TwoRider => "TWO RIDER",
            Kind::Wheel => "WHEEL",
            Kind::Handlebars => "HANDLEBARS",
            Kind::OnFront => "ON FRONT",
            Kind::LowBehind => "LOW BEHIND",
        }
    }
}

/// A director style: the primary camera and the secondaries, each with
/// its swap threshold in seconds.
#[derive(Clone, Debug, PartialEq)]
struct Style {
    primary: (Kind, f32),
    /// In the original's list order: each was put at the front as it was
    /// read, so the list runs backwards.
    secondaries: Vec<(Kind, f32)>,
}

impl Style {
    fn parse(xml: &str) -> Option<Self> {
        let root = setup::root(xml)?;
        let name = root.get("mDefaultStyle").map_or("AllCameras", String::as_str);
        let count = setup::entries(xml).into_iter().find(|e| e.name == name)?.num("mNumCameras")? as usize;
        let cams = setup::children(xml, name);
        let cam = |tag: &str| {
            let c = cams.iter().find(|c| c.name == tag)?;
            Some((Kind::parse(c.get("mCameraType")?)?, c.num("mSwapThreshold")?))
        };
        let primary = cam("PrimaryCamera")?;
        // The count includes the primary.
        let mut secondaries: Vec<(Kind, f32)> = (0..count.saturating_sub(1)).filter_map(|i| cam(&format!("SecondaryCamera{i}"))).collect();
        secondaries.reverse();
        Some(Self { primary, secondaries })
    }

    fn fallback() -> Self {
        Self { primary: (Kind::Trackside, 8.0), secondaries: vec![(Kind::FollowFar, 5.0)] }
    }
}

/// A trackside camera: where it stands and how it frames the rider.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Trackside {
    pos: Vec3,
    /// 0 still, 1 zooms to frame the rider, 2 zooms with a hand-held
    /// wobble, 3 the wobble at a set zoom.
    style: u8,
    zoom: f32,
}

/// A box on the track: a rider inside is filmed by camera `cam`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CamBox {
    centre: Vec3,
    rows: [Vec3; 3],
    half: Vec3,
    cam: usize,
}

impl CamBox {
    fn contains(&self, p: Vec3) -> bool {
        let d = p - self.centre;
        (0..3).all(|i| self.rows[i].dot(d).abs() < self.half[i])
    }
}

/// The trackside cameras and their boxes from a `World/<Track>.xml`.
fn parse_world(xml: &str) -> (Vec<Trackside>, Vec<CamBox>) {
    let attr = |tag: &str, key: &str| tag.split(&format!(" {key}=\"")).nth(1).and_then(|r| r.split('"').next()).map(str::trim).map(str::to_owned);
    let vec = |body: &str, tag: &str| {
        let at = body.find(&format!("<{tag} "))?;
        let t = &body[at..];
        let t = &t[..t.find('>')?];
        let n = |k: &str| attr(t, k)?.parse::<f32>().ok();
        Some([n("X")?, n("Y")?, n("Z")?])
    };
    let starts: Vec<usize> =
        xml.match_indices("<Object").map(|(i, _)| i).filter(|&i| xml[i + 7..].starts_with(|c: char| c.is_ascii_digit())).collect();
    let mut cams: Vec<(String, Trackside)> = Vec::new();
    let mut boxes: Vec<(String, CamBox)> = Vec::new();
    for (n, &start) in starts.iter().enumerate() {
        let body = &xml[start..starts.get(n + 1).copied().unwrap_or(xml.len())];
        let Some(head) = body.find('>').map(|e| &body[..e]) else { continue };
        let Some(pos) = vec(body, "Position") else { continue };
        match attr(head, "ManipulatorType").as_deref() {
            Some("Camera") => {
                let Some(style) = attr(head, "CameraType").and_then(|t| t.parse::<u8>().ok()).filter(|t| *t <= 3) else { continue };
                let zoom = attr(head, "CameraZoom").and_then(|z| z.parse().ok()).unwrap_or(1.0);
                cams.push((attr(head, "GUID").unwrap_or_default(), Trackside { pos: trk::to_sim(pos), style, zoom }));
            }
            Some("Camera Box") => {
                let (Some(r0), Some(r1), Some(r2), Some(scale)) = (vec(body, "Row0"), vec(body, "Row1"), vec(body, "Row2"), vec(body, "Scale")) else { continue };
                // Each row dotted with an offset is unchanged by the Z flip
                // when the row's Z flips too.
                let rows = [r0, r1, r2].map(trk::to_sim);
                let b = CamBox { centre: trk::to_sim(pos), rows, half: Vec3::from(scale).abs(), cam: 0 };
                boxes.push((attr(head, "NextGUID").unwrap_or_default(), b));
            }
            _ => {}
        }
    }
    let boxes = boxes
        .into_iter()
        .filter_map(|(next, b)| Some(CamBox { cam: cams.iter().position(|(guid, _)| *guid == next)?, ..b }))
        .collect();
    (cams.into_iter().map(|(_, c)| c).collect(), boxes)
}

/// A camera fixed to the quad (`VehicleCameras.xml`'s `Mount*`), in the
/// quad's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mount {
    offset: Vec3,
    rotation: Quat,
}

impl Mount {
    fn parse(xml: &str, entry: &str, fallback: Vec3) -> Self {
        let vec = |child: &str| {
            let m = setup::child(xml, entry, child)?;
            let n = |k: &str| m.get(k)?.trim().parse::<f32>().ok();
            Some([n("X")?, n("Y")?, n("Z")?])
        };
        let offset = vec("PosOffset").map_or(fallback, trk::to_sim);
        let rotation = match (vec("Row0"), vec("Row1"), vec("Row2")) {
            (Some(a), Some(b), Some(c)) => trk::rotation_to_sim(&[a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]]),
            _ => Quat::IDENTITY,
        };
        Self { offset, rotation }
    }
}

/// Everything the cameras need from the disc.
struct Rigs {
    style: Style,
    cams: Vec<Trackside>,
    boxes: Vec<CamBox>,
    /// The player's own race camera, which the trackside camera falls back
    /// on until a vehicle camera has been cut to.
    chase: FollowCam,
    far: FollowCam,
    high: FollowCam,
    front: Mount,
    handlebars: Mount,
    wheel: Mount,
    low: Mount,
}

impl Rigs {
    fn load(game: &GameSource, track: &str, view: CameraView) -> Self {
        let read = |p: &str| game.read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let style = Style::parse(&read("/Setup/CameraDirector.xml")).unwrap_or_else(Style::fallback);
        let (cams, boxes) = parse_world(&read(&format!("/Levels/{track}/World/{track}.xml")));
        let vehicle = read("/Setup/VehicleCameras.xml");
        // Follow High's own values (0x6ae80): high up and to the side, and slow.
        let high = FollowCam { offset: Vec3::new(5.0, 15.0, 5.0), look: Vec3::ZERO, pos_stiffness: [0.5; 2], rot_stiffness: [0.5; 2] };
        Self {
            style,
            cams,
            boxes,
            chase: FollowCam::load(game, view),
            far: FollowCam::load(game, CameraView::Far),
            high,
            front: Mount::parse(&vehicle, "MountOnFront", Vec3::new(0.0, 0.4, -1.0)),
            handlebars: Mount::parse(&vehicle, "MountOnHandlebars", Vec3::new(0.0, 0.7, -0.2)),
            wheel: Mount::parse(&vehicle, "MountOnWheel", Vec3::new(-0.6, 0.0, 0.3)),
            low: Mount::parse(&vehicle, "MountLowBehind", Vec3::new(-1.09, 0.14, 2.01)),
        }
    }
}

/// What the cameras can see: each racer's quad at a time in the replay.
pub struct Scene<'a> {
    pub quad: &'a dyn Fn(usize, f32) -> Option<(Vec3, Quat)>,
    /// [`GROUNDED`], [`ALL_DOWN`] and [`CRASHED`] for a racer at a time.
    pub flags: &'a dyn Fn(usize, f32) -> u8,
    pub racers: usize,
    pub target: usize,
    pub time: f32,
    pub collision: Option<&'a TrackCollision>,
    /// The right stick, which turns the orbit camera.
    pub stick: Vec2,
    /// Real seconds this frame: the orbit camera turns while paused too.
    pub real_dt: f32,
}

/// Where the camera is and where it looks.
#[derive(Clone, Copy)]
pub struct Shot {
    pub eye: Vec3,
    pub rotation: Quat,
    pub fov: f32,
}

/// The vehicle camera last cut to, which a trackside shot falls back on.
#[derive(Clone, Copy, PartialEq)]
enum Ride {
    Chase,
    Mount(Kind),
}

/// One shot's moving parts, started afresh at each cut.
#[derive(Default)]
struct Take {
    eye: Option<Vec3>,
    facing: Option<Quat>,
    /// A follow camera's offset from the quad.
    offset: Option<Vec3>,
    /// The orbit camera's offset from the quad.
    orbit: Option<Vec3>,
    /// The two-rider camera's aim, and how fast it moves.
    aim: Option<(Vec3, Vec3)>,
    /// Which trackside camera is filming, or dropped to the quad's camera.
    cam: Option<usize>,
    fallback: bool,
    /// How much of the frame the rider fills, picked at each camera.
    fill: f32,
    /// The hand-held wobble: aim error (yaw, pitch) and zoom kick (log),
    /// each springing back.
    wobble: (Vec2, Vec2),
    kick: (f32, f32),
}

pub struct Director {
    rigs: Option<Rigs>,
    seed: u32,
    /// A camera picked by hand, or the director's cuts.
    held: Option<Kind>,
    kind: Kind,
    on_primary: bool,
    /// Where the walk along the secondaries stands.
    cursor: Option<usize>,
    since_cut: f32,
    /// Seconds on the rider being watched.
    since_switch: f32,
    ride: Ride,
    take: Take,
    last: Option<Shot>,
    started: bool,
}

impl Default for Director {
    fn default() -> Self {
        Self {
            rigs: None,
            seed: 0x2545_f491,
            held: None,
            kind: Kind::Trackside,
            on_primary: true,
            cursor: None,
            since_cut: 0.0,
            since_switch: 0.0,
            ride: Ride::Chase,
            take: Take::default(),
            last: None,
            started: false,
        }
    }
}

impl Director {
    /// Reads the cameras for `track`; `view` is the player's race camera.
    pub fn load(&mut self, game: &GameSource, track: &str, view: CameraView) {
        let rigs = Rigs::load(game, track, view);
        info!("replay cameras: {} trackside, {} boxes, {:?} then {:?}", rigs.cams.len(), rigs.boxes.len(), rigs.style.primary, rigs.style.secondaries);
        self.rigs = Some(rigs);
    }

    /// The watched rider changed: the same camera, freshly placed.
    pub fn cut(&mut self) {
        self.take = Take::default();
        self.since_cut = 0.0;
    }

    /// Who the general mode watches next (0x72d00): after [`MIN_WATCH`] on
    /// one rider, a computer rider picked at random, unless either of them
    /// is thrown off. The first `players` racers are the players. The
    /// original first asks a scored table for a rider (0x729f0); that table
    /// isn't decoded, so the pick is always the random one it falls back on.
    pub fn pick_rider(&mut self, scene: &Scene, players: usize, dt: f32) -> usize {
        self.since_switch += dt;
        if scene.racers <= players || self.since_switch < MIN_WATCH {
            return scene.target;
        }
        let pick = players + self.rand(scene.racers - players);
        let thrown = |i: usize| (scene.flags)(i, scene.time) & CRASHED != 0;
        if pick == scene.target || thrown(scene.target) || thrown(pick) {
            return scene.target;
        }
        // The same camera picks the new rider up, and holds them a while.
        self.since_switch = 0.0;
        self.cut();
        pick
    }

    /// Steps through holding each camera, then back to the director's cuts.
    pub fn next_camera(&mut self) {
        self.held = match self.held {
            None => Some(Kind::ALL[0]),
            Some(k) => Kind::ALL.iter().position(|&a| a == k).and_then(|i| Kind::ALL.get(i + 1)).copied(),
        };
        match self.held {
            Some(k) => self.activate(k),
            None => {
                self.on_primary = true;
                self.activate(self.primary().0);
            }
        }
    }

    /// The camera on screen, and whether it was picked by hand.
    pub fn label(&self) -> String {
        let shown = match (self.kind, self.take.fallback, self.ride) {
            (Kind::Trackside, true, Ride::Chase) => "FOLLOW",
            (Kind::Trackside, true, Ride::Mount(k)) => k.label(),
            (k, _, _) => k.label(),
        };
        match self.held {
            Some(_) => format!("CAMERA  {shown}"),
            None => format!("DIRECTOR  {shown}"),
        }
    }

    fn primary(&self) -> (Kind, f32) {
        self.rigs.as_ref().map_or(Style::fallback().primary, |r| r.style.primary)
    }

    fn rand(&mut self, n: usize) -> usize {
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (((self.seed >> 16) & 0x7fff) as usize * n) >> 15
    }

    /// -1..1.
    fn pm(&mut self) -> f32 {
        self.rand(0x8000) as f32 / 16384.0 - 1.0
    }

    fn activate(&mut self, kind: Kind) {
        self.kind = kind;
        self.since_cut = 0.0;
        self.take = Take::default();
        match kind {
            Kind::FollowFar | Kind::Wheel | Kind::Handlebars | Kind::OnFront | Kind::LowBehind => self.ride = Ride::Mount(kind),
            _ => {}
        }
    }

    /// The next secondary: one to three steps along the list, wrapping.
    fn pick_secondary(&mut self) -> Option<Kind> {
        let list = self.rigs.as_ref().map_or_else(|| Style::fallback().secondaries, |r| r.style.secondaries.clone());
        if list.is_empty() {
            return None;
        }
        let mut at = self.cursor.unwrap_or(0);
        let mut steps = 0;
        loop {
            at = (at + 1) % list.len();
            steps += 1;
            if steps > self.rand(3) {
                break;
            }
        }
        self.cursor = Some(at);
        Some(list[at].0)
    }

    /// Cuts when the shot has run its time and the rider is settled.
    fn direct(&mut self, scene: &Scene, dt: f32) {
        self.since_cut += dt;
        if self.held.is_some() {
            return;
        }
        let flags = (scene.flags)(scene.target, scene.time);
        let settled = flags & ALL_DOWN != 0 && flags & CRASHED == 0;
        // The primary's threshold for every shot, as the original reads it.
        let threshold = self.primary().1;
        if self.since_cut < MIN_SHOT || self.since_cut <= threshold || !settled {
            return;
        }
        if self.on_primary
            && let Some(k) = self.pick_secondary()
        {
            self.on_primary = false;
            self.activate(k);
            return;
        }
        self.on_primary = true;
        self.activate(self.primary().0);
    }

    pub fn shot(&mut self, scene: &Scene, dt: f32) -> Shot {
        if !self.started {
            self.started = true;
            self.activate(self.held.unwrap_or(self.primary().0));
        }
        self.direct(scene, dt);
        let Some((p, r)) = (scene.quad)(scene.target, scene.time) else {
            return self.last.unwrap_or(Shot { eye: Vec3::Y, rotation: Quat::IDENTITY, fov: FOV });
        };
        let grounded = (scene.flags)(scene.target, scene.time) & GROUNDED != 0;
        let shot = match self.kind {
            Kind::Trackside => self.trackside(scene, p, r, grounded, dt),
            Kind::FollowHigh => {
                let high = self.rigs.as_ref().map(|r| r.high);
                self.follow(scene, high, p, r, grounded, dt)
            }
            Kind::Orbit => self.orbit(scene, p, r),
            Kind::TwoRider => self.two_rider(scene, p, r, dt),
            _ => self.ride(scene, p, r, grounded, dt),
        };
        self.last = Some(shot);
        shot
    }

    /// The vehicle camera last cut to.
    fn ride(&mut self, scene: &Scene, p: Vec3, r: Quat, grounded: bool, dt: f32) -> Shot {
        let rigs = self.rigs.as_ref();
        let mount = match self.ride {
            Ride::Mount(Kind::Wheel) => rigs.map(|r| r.wheel),
            Ride::Mount(Kind::Handlebars) => rigs.map(|r| r.handlebars),
            Ride::Mount(Kind::OnFront) => rigs.map(|r| r.front),
            Ride::Mount(Kind::LowBehind) => rigs.map(|r| r.low),
            _ => None,
        };
        if let Some(m) = mount {
            return Shot { eye: p + r * m.offset, rotation: r * m.rotation, fov: FOV };
        }
        let follow = match self.ride {
            Ride::Mount(Kind::FollowFar) => rigs.map(|r| r.far),
            _ => rigs.map(|r| r.chase),
        };
        self.follow(scene, follow, p, r, grounded, dt)
    }

    /// A follow camera, as the race's own.
    fn follow(&mut self, scene: &Scene, follow: Option<FollowCam>, p: Vec3, r: Quat, grounded: bool, dt: f32) -> Shot {
        let f = follow.unwrap_or(FollowCam { offset: Vec3::new(0.0, 2.7, -4.3), look: Vec3::new(0.0, 0.0, 1.0), pos_stiffness: [6.0, 4.0], rot_stiffness: [4.0, 4.0] });
        let fwd = (r * Vec3::NEG_Z).with_y(0.0).normalize_or(Vec3::NEG_Z);
        let right = fwd.cross(Vec3::Y);
        let at = |o: Vec3| p + right * o.x + Vec3::Y * o.y + fwd * o.z;
        let air = usize::from(!grounded);
        let ease = |stiffness: f32| 1.0 - (-stiffness * dt).exp();
        let want = at(f.offset) - p;
        let mut offset = self.take.offset.map_or(want, |o| o.lerp(want, ease(f.pos_stiffness[air])));
        offset.y = offset.y.max(0.5);
        self.take.offset = Some(offset);
        let eye = in_view(scene.collision, p, p + offset);
        let look = at(f.look + Vec3::Y * crate::LOOK_RAISE);
        let mut facing = self.take.facing.unwrap_or(Quat::IDENTITY);
        if (look - eye).length_squared() > 1e-4 {
            let want = Transform::from_translation(eye).looking_at(look, Vec3::Y).rotation;
            facing = match self.take.facing {
                Some(f0) => f0.slerp(want, ease(f.rot_stiffness[air])).normalize(),
                None => want,
            };
        }
        self.take.facing = Some(facing);
        self.take.eye = Some(eye);
        Shot { eye, rotation: facing, fov: FOV }
    }

    /// The trackside camera whose box holds the rider, zoomed to frame them.
    fn trackside(&mut self, scene: &Scene, p: Vec3, r: Quat, grounded: bool, dt: f32) -> Shot {
        if !self.take.fallback {
            let found = self.rigs.as_ref().and_then(|rigs| rigs.boxes.iter().find(|b| b.contains(p)).map(|b| b.cam));
            match found {
                Some(c) if self.take.cam != Some(c) => {
                    self.take.cam = Some(c);
                    // Each camera picks how tight to frame (0x6f770) and
                    // starts with a wobble (0x70970, 0x6fa30).
                    self.take.fill = 0.4 + (self.rand(0x8000) as f32 / 32768.0) * 0.35;
                    let angle = self.rand(0x8000) as f32 / 32768.0 * TAU;
                    let size = (self.pm().abs() * 0.1 * PI).min(PI);
                    self.take.wobble = (Vec2::from_angle(angle) * size, Vec2::ZERO);
                    self.take.kick = (self.pm() * 0.2, 0.0);
                }
                Some(_) => {}
                None => {
                    self.take.fallback = true;
                    self.take.eye = None;
                    self.take.facing = None;
                    self.take.offset = None;
                }
            }
        }
        let cam = self.take.cam.and_then(|c| self.rigs.as_ref()?.cams.get(c).copied());
        let Some(cam) = cam.filter(|_| !self.take.fallback) else { return self.ride(scene, p, r, grounded, dt) };
        let aim = p + Vec3::Y * AIM_HEIGHT;
        let tan = (FOV * 0.5).tan();
        let dist = cam.pos.distance(aim);
        let mut zoom = match cam.style {
            0 => 1.0,
            1 | 2 => (tan / 0.7 * self.take.fill * dist / (2.0 * RIDER_HALF_SIZE)).max(1.0),
            _ => cam.zoom.max(0.1),
        };
        if cam.style == 2 {
            self.take.kick = spring(self.take.kick, 20.0, 20f32.sqrt() * 1.4, dt);
            zoom = (zoom.ln() + self.take.kick.0).max(0.0).exp();
        }
        let mut rotation = Transform::from_translation(cam.pos).looking_at(aim, Vec3::Y).rotation;
        if cam.style >= 2 {
            let (e, v) = self.take.wobble;
            let (ex, vx) = spring((e.x, v.x), 25.0, 9.0, dt);
            let (ey, vy) = spring((e.y, v.y), 25.0, 9.0, dt);
            self.take.wobble = (Vec2::new(ex, ey), Vec2::new(vx, vy));
            rotation = rotation * Quat::from_rotation_y(ex / zoom) * Quat::from_rotation_x(ey / zoom);
        }
        Shot { eye: cam.pos, rotation, fov: 2.0 * (tan / zoom).atan() }
    }

    /// Close behind, staying put as the rider turns; the right stick
    /// swings it round (0x6de10).
    fn orbit(&mut self, scene: &Scene, p: Vec3, r: Quat) -> Shot {
        let offset = match (self.take.orbit, self.last) {
            (Some(o), _) => o,
            // Picks up from the last camera when it was close.
            (None, Some(last)) if last.eye.distance(p) < 10.0 => last.eye - p,
            _ => {
                let fwd = (r * Vec3::NEG_Z).with_y(0.0).normalize_or(Vec3::NEG_Z);
                Vec3::Y * 1.5 - fwd * 1.5
            }
        };
        let turn = scene.stick * 2.0 * scene.real_dt;
        let side = offset.cross(Vec3::Y).normalize_or(Vec3::X);
        let mut offset = Quat::from_rotation_y(-turn.x) * Quat::from_axis_angle(side, turn.y) * offset;
        if offset.y < 0.3 {
            offset.y = 0.3;
        }
        self.take.orbit = Some(offset);
        let want = p + offset;
        let eye = self.take.eye.map_or(want, |e| e.lerp(want, 1.0 - (-20.0 * scene.real_dt).exp()));
        self.take.eye = Some(eye);
        let eye = in_view(scene.collision, p, eye);
        let rotation = Transform::from_translation(eye).looking_at(p + Vec3::Y * AIM_HEIGHT, Vec3::Y).rotation;
        Shot { eye, rotation, fov: FOV }
    }

    /// Beyond the rider, looking back past them at the nearest other
    /// racer (0x6da60, 0x71a20). Alone, it looks back along the way they
    /// came.
    fn two_rider(&mut self, scene: &Scene, p: Vec3, r: Quat, dt: f32) -> Shot {
        let other = (0..scene.racers)
            .filter(|&i| i != scene.target)
            .filter_map(|i| (scene.quad)(i, scene.time).map(|q| q.0))
            .min_by(|a, b| a.distance_squared(p).total_cmp(&b.distance_squared(p)));
        let from = other.or_else(|| (scene.quad)(scene.target, (scene.time - 1.0).max(0.0)).map(|q| q.0));
        let away = from.map(|o| (p - o).with_y(0.0)).filter(|d| d.length_squared() > 0.25).map(Vec3::normalize);
        let away = away.unwrap_or_else(|| (r * Vec3::NEG_Z).with_y(0.0).normalize_or(Vec3::NEG_Z));
        let want = p + away * 10.0 + Vec3::Y * 2.5;
        let eye = self.take.eye.map_or(want, |e| e.lerp(want, 1.0 - (-8.0 * dt).exp()));
        self.take.eye = Some(eye);
        let eye = in_view(scene.collision, p, eye);
        let target = p + Vec3::Y * AIM_HEIGHT;
        let (mut aim, mut vel) = self.take.aim.unwrap_or((target, Vec3::ZERO));
        // A stiff, critically damped spring.
        let steps = (dt * 120.0).ceil().max(1.0);
        let h = dt / steps;
        for _ in 0..steps as usize {
            vel += (50.0 * (target - aim) - 2.0 * 50f32.sqrt() * vel) * h;
            aim += vel * h;
        }
        self.take.aim = Some((aim, vel));
        let rotation = Transform::from_translation(eye).looking_at(aim, Vec3::Y).rotation;
        Shot { eye, rotation, fov: FOV }
    }
}

/// `eye`, pulled in when terrain or a wall is between it and the rider at
/// `p`, and kept clear of the ground under it, as the race camera does.
fn in_view(world: Option<&TrackCollision>, p: Vec3, eye: Vec3) -> Vec3 {
    let Some(world) = world else { return eye };
    let pivot = p + Vec3::Y * 1.2;
    let to_cam = eye - pivot;
    let dist = to_cam.length();
    let mut eye = eye;
    if dist > 0.01 {
        let dir = to_cam / dist;
        if let Some(hit) = world.raycast(pivot, dir, dist + 0.4) {
            eye = pivot + dir * (hit.t - 0.4).max(0.3);
        }
    }
    if let Some(ground) = world.raycast(eye + Vec3::Y, Vec3::NEG_Y, 1.5) {
        eye.y = eye.y.max(ground.point.y + 0.5);
    }
    eye
}

/// A damped spring pulling `(value, speed)` back to zero.
fn spring((mut x, mut v): (f32, f32), k: f32, c: f32, dt: f32) -> (f32, f32) {
    let steps = (dt * 120.0).ceil().max(1.0);
    let h = dt / steps;
    for _ in 0..steps as usize {
        v += (-k * x - c * v) * h;
        x += v * h;
    }
    (x, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_leaves_out_the_last_secondary() {
        let xml = r#"<Root mDefaultStyle="AllCameras">
   <Realistic mNumCameras="2">
      <PrimaryCamera mCameraType="Trackside" mSwapThreshold="10.0"/>
      <SecondaryCamera0 mCameraType="Follow High" mSwapThreshold="5.0"/>
   </Realistic>
   <AllCameras mNumCameras="3">
      <PrimaryCamera mCameraType="Trackside" mSwapThreshold="8.0"/>
      <SecondaryCamera0 mCameraType="Orbit Player" mSwapThreshold="5.0"/>
      <SecondaryCamera1 mCameraType="Two Rider" mSwapThreshold="10.0"/>
      <SecondaryCamera2 mCameraType="On Front" mSwapThreshold="5.0"/>
   </AllCameras>
</Root>"#;
        let s = Style::parse(xml).unwrap();
        assert_eq!(s.primary, (Kind::Trackside, 8.0));
        assert_eq!(s.secondaries, vec![(Kind::TwoRider, 10.0), (Kind::Orbit, 5.0)]);
    }

    #[test]
    fn boxes_pick_their_cameras() {
        let xml = r#"<Root><Objects NObjects="3">
    <Object0 NextGUID="7" ObjectType="2" GUID="0" ManipulatorType="Camera Box" PlatformMask="7">
      <Position X="10.0" Y="0.0" Z="20.0" />
      <Matrix>
        <Row0 X="1.0" Y="0.0" Z="0.0" />
        <Row1 X="0.0" Y="1.0" Z="0.0" />
        <Row2 X="0.0" Y="0.0" Z="1.0" /></Matrix>
      <Scale X="5.0" Y="5.0" Z="2.0" /></Object0>
    <Object1 NextGUID="0" ObjectType="1" GUID="7" ManipulatorType="Camera" PlatformMask="7" CameraType="2" TestVar="0" CameraZoom="1.5" HeatHaze="1" ID="0" FocusIndex="0">
      <Position X="1.0" Y="2.0" Z="3.0" />
      <Matrix>
        <Row0 X="1.0" Y="0.0" Z="0.0" />
        <Row1 X="0.0" Y="1.0" Z="0.0" />
        <Row2 X="0.0" Y="0.0" Z="1.0" /></Matrix>
      <Scale X="1.0" Y="1.0" Z="1.0" /></Object1>
    <Object2 NextGUID="9" ObjectType="1" GUID="8" ManipulatorType="Camera" PlatformMask="7" CameraType="4" CameraZoom="1.0">
      <Position X="0.0" Y="0.0" Z="0.0" /></Object2>
</Objects></Root>"#;
        let (cams, boxes) = parse_world(xml);
        assert_eq!(cams, vec![Trackside { pos: Vec3::new(1.0, 2.0, -3.0), style: 2, zoom: 1.5 }]);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].cam, 0);
        // The game's Z is flipped.
        assert!(boxes[0].contains(Vec3::new(12.0, 1.0, -21.5)));
        assert!(!boxes[0].contains(Vec3::new(12.0, 1.0, -17.5)));
        assert!(!boxes[0].contains(Vec3::new(12.0, 1.0, 21.0)));
    }

    #[test]
    fn mounts_face_the_quads_way() {
        let xml = r#"<Root>
   <MountOnBack>
      <PosOffset X="0.0" Y="1.25" Z="0.2"/>
      <Orientation>
         <Row0 X="-1.0" Y="0.0" Z="0.0"/>
         <Row1 X="0.0" Y="1.0" Z="0.0"/>
         <Row2 X="0.0" Y="0.0" Z="-1.0"/>
      </Orientation>
   </MountOnBack>
</Root>"#;
        let m = Mount::parse(xml, "MountOnBack", Vec3::ZERO);
        // Forward is -Z for the quad and the camera alike.
        assert_eq!(m.offset, Vec3::new(0.0, 1.25, -0.2));
        assert!((m.rotation * Vec3::NEG_Z).distance(Vec3::Z) < 1e-5);
    }

    #[test]
    fn general_mode_moves_between_computer_riders() {
        let mut d = Director::default();
        let quad = |_: usize, _: f32| Some((Vec3::ZERO, Quat::IDENTITY));
        let fine = |_: usize, _: f32| GROUNDED | ALL_DOWN;
        let scene = Scene { quad: &quad, flags: &fine, racers: 4, target: 0, time: 0.0, collision: None, stick: Vec2::ZERO, real_dt: 0.1 };
        assert_eq!(d.pick_rider(&scene, 1, 3.9), 0);
        let mut target = 0;
        for _ in 0..20 {
            target = d.pick_rider(&Scene { target, ..scene }, 1, 0.5);
            if target != 0 {
                break;
            }
        }
        assert!((1..4).contains(&target));
        // Not again for a while.
        assert_eq!(d.pick_rider(&Scene { target, ..scene }, 1, 3.0), target);
        // Alone, nobody else to watch.
        let alone = Scene { racers: 1, ..scene };
        assert_eq!(d.pick_rider(&alone, 1, 10.0), 0);
    }

    #[test]
    fn cuts_alternate_after_the_primarys_time() {
        let mut d = Director::default();
        let quad = |_: usize, _: f32| Some((Vec3::ZERO, Quat::IDENTITY));
        let settled = |_: usize, _: f32| GROUNDED | ALL_DOWN;
        let scene = Scene { quad: &quad, flags: &settled, racers: 1, target: 0, time: 0.0, collision: None, stick: Vec2::ZERO, real_dt: 0.1 };
        d.shot(&scene, 0.0);
        assert_eq!(d.kind, Kind::Trackside);
        for _ in 0..79 {
            d.shot(&scene, 0.1);
        }
        assert_eq!(d.kind, Kind::Trackside);
        for _ in 0..3 {
            d.shot(&scene, 0.1);
        }
        assert_eq!(d.kind, Kind::FollowFar);
        // Thrown off: no cut, however long.
        let crashed = |_: usize, _: f32| CRASHED;
        let scene = Scene { flags: &crashed, ..scene };
        for _ in 0..200 {
            d.shot(&scene, 0.1);
        }
        assert_eq!(d.kind, Kind::FollowFar);
        let scene = Scene { flags: &settled, ..scene };
        d.shot(&scene, 0.1);
        assert_eq!(d.kind, Kind::Trackside);
    }
}
