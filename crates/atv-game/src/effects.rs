//! In-race particle effects, ported from the original's particle manager
//! (set up at 0x8e440, run each frame by 0x8e380): what each wheel throws
//! up off the ground, and what comes out of the exhaust.
//!
//! The manager keeps five pools drawn from `Sprites/Particles`:
//! - chunks (`Dirt0..2`, `Gravel0..3`, `Dust0`), which bounce off the
//!   ground they were thrown from;
//! - dust puffs and the big slow smoke clouds behind skidding wheels;
//! - exhaust puffs;
//! - sparks (`Spark0`), which the exhaust spits while boosting.
//!
//! Quads and riders coming down into deep water splash (0xe6520), and the
//! water's ripples (`Ripple`, 0x106df0) lie flat on its surface.
//!
//! Each frame (at 60 a second) the emitters roll a chance per wheel and
//! per exhaust. Each wheel follows its own surface's line in
//! `Setup/Materials.xml` (from the track collision); wheels off the track's
//! own ground (on props) use the environment's main ground. Wheels in
//! water, wading or sunk in deep water, throw nothing.

use std::collections::{HashMap, VecDeque};

use atv_formats::bed::SpriteSheet;
use atv_formats::materials::Surface;
use atv_sim::Vehicle;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::boost::Boost;
use crate::convert::{Exhaust, WheelVisual};
use crate::crash::{WaterFx, Wreck};
use crate::opponents::Opponents;
use crate::water::{depth, Water, UNDER};
use crate::{AppState, Game, Player, QuadBody, Selection};

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Race), load)
            .add_systems(Update, (emit, step, draw).chain().run_if(in_state(AppState::Race)));
    }
}

/// The original's frame rate: its emitters roll their chances once a frame.
const FPS: f32 = 60.0;
/// 5 and 30 mph: a wheel's spray grows from nothing to full between them.
const SPRAY_MIN: f32 = 2.234_722;
const SPRAY_FULL: f32 = 13.408_33;
/// 50 mph, the speed at which skidding wheels raise the most dust.
const DUST_FULL: f32 = 22.347_22;
/// How fast a spinning rear tyre's tread can run, m/s (the rewrite's: the
/// simulation has no wheel speeds of its own).
const MAX_TREAD: f32 = 22.347_22;
/// Past this far from every camera nothing is thrown up.
const FAR: f32 = 50.0;
/// Coming down into deep water faster than this (0.08 m a frame), a quad
/// splashes (0xd7f60).
const SPLASH_SPEED: f32 = 4.8;
/// The most ripples on the water at once (0x1d360c).
const MAX_RIPPLES: usize = 256;
/// The brightest a particle's colour gets: the original packs colours
/// 128 per 1.0 into a byte (0x15cd0), and draws them doubled.
const BRIGHTEST: f32 = 255.0 / 128.0;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Pool {
    Chunks,
    Dust,
    Smoke,
    Exhaust,
    Sparks,
    Splash,
}

const POOLS: [Pool; 6] = [Pool::Chunks, Pool::Dust, Pool::Smoke, Pool::Exhaust, Pool::Sparks, Pool::Splash];

/// A pool's settings, from the original's set-up calls.
struct Spec {
    max: usize,
    life: f32,
    /// Sprite size per unit of particle size (width, height).
    scale: Vec2,
    gravity: f32,
    /// Velocity kept each 1/60 s (the sparks use `drag` instead).
    damping: f32,
    /// Sparks: velocity lost per second, as a fraction.
    drag: f32,
    /// Chunks bounce off the ground they were thrown from.
    bounce: bool,
}

impl Pool {
    fn spec(self) -> Spec {
        let base = Spec { max: 0, life: 0.0, scale: Vec2::ONE, gravity: 0.0, damping: 1.0, drag: 0.0, bounce: false };
        match self {
            Pool::Chunks => Spec { max: 600, life: 1.5, gravity: -10.0, damping: 0.9999, bounce: true, ..base },
            Pool::Dust => Spec { max: 80, life: 1.0, scale: Vec2::new(1.0, 0.5), damping: 0.99, ..base },
            Pool::Smoke => Spec { max: 240, life: 4.0, scale: Vec2::new(1.0, 0.5), damping: 0.99, ..base },
            Pool::Exhaust => Spec { max: 40, life: 0.3, scale: Vec2::splat(0.25), damping: 0.7, ..base },
            // The spark pool draws every spark the same size.
            Pool::Sparks => Spec { max: 50, life: 0.3, scale: Vec2::splat(0.02), gravity: -6.0, drag: 0.97, ..base },
            Pool::Splash => Spec { max: 64, life: 2.0, gravity: -6.0, ..base },
        }
    }
}

/// One particle. Colours are the original's: 1.0 leaves the sprite as it
/// is, more brightens it (up to `BRIGHTEST`).
#[derive(Clone, Debug)]
struct Particle {
    pos: Vec3,
    vel: Vec3,
    /// Turn about the view direction, and its rate.
    rot: f32,
    spin: f32,
    size: f32,
    grow: f32,
    age: f32,
    fade_in: f32,
    fade_out: f32,
    alpha: f32,
    start: Vec3,
    end: Vec3,
    sprite: usize,
    /// The ground a chunk bounces off: normal and a point on it.
    ground: Option<(Vec3, Vec3)>,
    /// Lives this many times the pool's life (splashes, 0xe6bc0).
    life_scale: f32,
}

impl Particle {
    fn new(sprite: usize, pos: Vec3, vel: Vec3) -> Self {
        Particle {
            pos,
            vel,
            rot: 0.0,
            spin: 0.0,
            size: 1.0,
            grow: 0.0,
            age: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            alpha: 1.0,
            start: Vec3::ONE,
            end: Vec3::ONE,
            sprite,
            ground: None,
            life_scale: 1.0,
        }
    }

    /// One step (0xf4b50, with the chunks' bounce from 0xf77c0 and the
    /// sparks' drag from 0xf7c20). False once it has lived its life.
    fn step(&mut self, spec: &Spec, dt: f32) -> bool {
        self.age += dt / self.life_scale;
        if self.age > spec.life {
            return false;
        }
        if spec.drag > 0.0 {
            self.vel += (-spec.drag * self.vel + Vec3::Y * spec.gravity) * dt;
        } else {
            self.vel *= spec.damping.powf(dt * FPS);
            self.vel.y += spec.gravity * dt;
        }
        self.pos += self.vel * dt;
        self.rot += self.spin * dt;
        self.size += self.grow * dt;
        if spec.bounce
            && let Some((n, at)) = self.ground
        {
            let under = (self.pos - at).dot(n);
            if under <= 0.0 {
                self.pos -= n * under;
                let vn = self.vel.dot(n);
                self.vel = -n * vn * 0.9 + (self.vel - n * vn) * 0.9;
            }
        }
        true
    }

    /// Colour and opacity now: fading in, holding, then fading out.
    fn colour(&self, life: f32) -> Vec4 {
        let t = (self.age / life).clamp(0.0, 1.0);
        let rgb = self.start.lerp(self.end, t);
        let a = if self.fade_in > 0.0 && self.age < self.fade_in {
            self.alpha * self.age / self.fade_in
        } else if self.fade_out > 0.0 && self.age > life - self.fade_out {
            self.alpha * (life - self.age) / self.fade_out
        } else {
            self.alpha
        };
        rgb.extend(a.clamp(0.0, 1.0))
    }
}

/// A ripple lying flat on the water (0x1072c0): it waits out its delay,
/// then grows and fades over its life.
#[derive(Clone, Debug, PartialEq)]
struct Ripple {
    pos: Vec3,
    /// Half its width, and how fast that grows, m/s.
    size: f32,
    grow: f32,
    alpha: f32,
    delay: f32,
    age: f32,
    life: f32,
}

impl Ripple {
    fn new(pos: Vec3, size: f32, grow: f32, alpha: f32, delay: f32) -> Self {
        Ripple { pos, size, grow, alpha, delay, age: 0.0, life: 2.0 }
    }

    /// One step (0x106f00). False once it has lived its life.
    fn step(&mut self, dt: f32) -> bool {
        if self.delay > 0.0 {
            self.delay = (self.delay - dt).max(0.0);
        } else {
            self.age += dt;
            self.size += self.grow * dt;
        }
        self.age < self.life
    }

    /// How opaque it is now (the original's fade isn't worked out: here
    /// it fades evenly over its life). None while it waits.
    fn opacity(&self) -> Option<f32> {
        (self.delay <= 0.0 && self.age < self.life).then(|| self.alpha * (1.0 - self.age / self.life))
    }
}

/// A splash where something came down into the water at `at` (on the
/// surface), moving `motion` a 60 Hz frame, `radius` across (0xe6520):
/// spray thrown up and out in a ring, four big ripples one after
/// another, and small ripples spreading out further the harder it hit.
fn splash(rng: &mut Rng, sprite: usize, at: Vec3, motion: Vec3, radius: f32) -> (Vec<Particle>, Vec<Ripple>) {
    let tau = std::f32::consts::TAU;
    let strength = (-5.0 * motion.y).min(1.0);
    let mut spray = Vec::with_capacity(32);
    for i in 0..32 {
        let (s, c) = (i as f32 / 32.0 * tau).sin_cos();
        let out = rng.unit() * 0.75 * radius;
        let pos = at + Vec3::new(out * s, 0.0, out * c);
        let vel = Vec3::new(1.2 * s + 3.0 * motion.x, (3.0 + rng.unit() * 4.0) * strength, 1.2 * c + 3.0 * motion.z);
        spray.push(Particle {
            rot: rng.unit() * tau,
            spin: rng.pm(0.3),
            size: 0.3 + rng.unit() * 0.1,
            grow: 1.5 + rng.unit() * 0.5,
            fade_out: 2.0,
            alpha: 0.5,
            life_scale: (strength + 1.0) * 0.5,
            ..Particle::new(sprite, pos, vel)
        });
    }
    let mut ripples: Vec<Ripple> =
        [(0.0, 0.25), (0.11, 0.5), (0.22, 0.375), (0.33, 0.25)].iter().map(|&(delay, alpha)| Ripple::new(at, 0.25 * radius, 1.5, alpha, delay)).collect();
    for i in 0..16 {
        let (s, c) = (i as f32 / 16.0 * tau).sin_cos();
        let out = rng.unit() * 8.0 * strength * radius;
        ripples.push(Ripple::new(at + Vec3::new(out * s, 0.0, out * c), 0.0, 0.6, 0.4, 0.3 + rng.unit()));
    }
    (spray, ripples)
}

/// The rings a rider treading water sends out (0xe69e0, kind 1).
fn stir(at: Vec3) -> [Ripple; 3] {
    [0.0, 0.05, 0.1].map(|delay| Ripple::new(at, 0.0, 1.5, 0.4, delay))
}

/// How far a quad reaches from its centre to the outsides of its wheels:
/// its size to the water (0x1c1b0 keeps it for the water sensor).
fn reach(v: &Vehicle) -> f32 {
    v.params.wheels.iter().map(|w| Vec3::from(w.offset).length() + w.radius).fold(0.0, f32::max)
}

#[derive(Default)]
struct Rng(u32);

impl Rng {
    /// 0..1.
    fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        ((self.0 >> 8) & 0xffff) as f32 / 65536.0
    }
    /// -x..x, as the original's 0xaacb0.
    fn pm(&mut self, x: f32) -> f32 {
        (self.unit() * 2.0 - 1.0) * x
    }
    fn pick(&mut self, n: usize) -> usize {
        ((self.unit() * n as f32) as usize).min(n.saturating_sub(1))
    }
    fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
    /// How many of something that happens `per_frame` times a 60 Hz frame
    /// (a chance when under 1) happen over `dt`.
    fn count(&mut self, per_frame: f32, dt: f32) -> usize {
        let n = (per_frame * dt * FPS).max(0.0);
        n as usize + usize::from(self.chance(n.fract()))
    }
}

/// Everything the effects draw with, and the live particles.
#[derive(Resource)]
struct Effects {
    /// The environment's main ground, and each surface by collision id.
    surface: Surface,
    by_id: Vec<Option<Surface>>,
    /// Sprite UV rectangles (min, max) and their names.
    sprites: Vec<(String, Vec2, Vec2)>,
    pools: HashMap<Pool, VecDeque<Particle>>,
    meshes: HashMap<Pool, Handle<Mesh>>,
    rng: Rng,
    /// Per quad (player 1 first, then each racer): the boost smoke left
    /// to puff (0x8ada0 refills it over a second off the boost).
    puff: Vec<f32>,
    /// Per quad: whether it was in deep water last frame.
    wet: Vec<bool>,
    ripples: VecDeque<Ripple>,
    ripple_mesh: Handle<Mesh>,
}

impl Effects {
    fn sprite(&self, name: &str) -> usize {
        self.sprites.iter().position(|(n, ..)| n.eq_ignore_ascii_case(name)).unwrap_or(0)
    }

    fn add(&mut self, pool: Pool, p: Particle) {
        let max = pool.spec().max;
        let list = self.pools.entry(pool).or_default();
        if list.len() >= max {
            list.pop_front();
        }
        list.push_back(p);
    }

    fn add_ripples(&mut self, ripples: impl IntoIterator<Item = Ripple>) {
        for r in ripples {
            if self.ripples.len() >= MAX_RIPPLES {
                self.ripples.pop_front();
            }
            self.ripples.push_back(r);
        }
    }

    fn splash(&mut self, at: Vec3, motion: Vec3, radius: f32) {
        let sprite = self.sprite("Splash");
        let (spray, ripples) = splash(&mut self.rng, sprite, at, motion, radius);
        for p in spray {
            self.add(Pool::Splash, p);
        }
        self.add_ripples(ripples);
    }
}

/// The ground each environment mostly is (as `audio::main_surface`).
fn main_surface(env: &str) -> &'static str {
    match env {
        "Dunes" => "Sand",
        "Glacier" => "Snow",
        "Swamp" => "Mud",
        _ => "Dirt",
    }
}

fn load(
    mut commands: Commands,
    game: Res<Game>,
    sel: Res<Selection>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let env = sel.track.trim_end_matches(|c: char| c.is_ascii_digit());
    let surfaces = game.0.read("/Setup/Materials.xml").map(|b| Surface::parse_all(&String::from_utf8_lossy(&b))).unwrap_or_default();
    let by_id = atv_sim::surface::NAMES.iter().map(|n| surfaces.iter().find(|s| s.name.eq_ignore_ascii_case(n)).cloned()).collect();
    let Some(surface) = surfaces.into_iter().find(|s| s.name == main_surface(env)) else {
        warn!("no {} surface in Materials.xml: no wheel effects", main_surface(env));
        return;
    };
    let page = crate::hd::texture(&game.0, "/Sprites/Particles.bog");
    let sheet = game.0.read("/Sprites/Particles.bed").map_err(anyhow::Error::from).and_then(|b| Ok(SpriteSheet::parse(&b)?));
    let (Ok((tex, _)), Ok(sheet)) = (page, sheet) else {
        warn!("no Sprites/Particles: no in-race effects");
        return;
    };
    let image = images.add(Image::new(
        Extent3d { width: tex.width, height: tex.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        tex.to_rgba8(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    ));
    let sprites = sheet.sprites().map(|(n, s)| (n.to_string(), Vec2::from(s.min), Vec2::from(s.max))).collect();
    let mut handles = HashMap::new();
    for pool in POOLS {
        let material = materials.add(StandardMaterial {
            base_color_texture: Some(image.clone()),
            alpha_mode: if pool == Pool::Sparks { AlphaMode::Add } else { AlphaMode::Blend },
            unlit: true,
            cull_mode: None,
            ..default()
        });
        let mesh = meshes.add(Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default()));
        commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material), Transform::default(), NoFrustumCulling, DespawnOnExit(AppState::Race)));
        handles.insert(pool, mesh);
    }
    let ripple_material = materials.add(StandardMaterial {
        base_color_texture: Some(image.clone()),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let ripple_mesh = meshes.add(Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default()));
    commands.spawn((Mesh3d(ripple_mesh.clone()), MeshMaterial3d(ripple_material), Transform::default(), NoFrustumCulling, DespawnOnExit(AppState::Race)));
    commands.insert_resource(Effects {
        surface,
        by_id,
        sprites,
        pools: HashMap::new(),
        meshes: handles,
        rng: Rng(0x2545_f491),
        puff: Vec::new(),
        wet: Vec::new(),
        ripples: VecDeque::new(),
        ripple_mesh,
    });
}

/// One quad's part in the effects this frame.
struct Quad<'a> {
    v: &'a Vehicle,
    body: Option<Entity>,
    boosting: bool,
    /// Player-driven quads throw twice what computer riders' do.
    human: bool,
}

#[allow(clippy::too_many_arguments)]
fn emit(
    time: Res<Time>,
    fx: Option<ResMut<Effects>>,
    player: Option<Res<Player>>,
    boost: Option<Res<Boost>>,
    opponents: Option<Res<Opponents>>,
    water: Option<Res<Water>>,
    wreck: Option<ResMut<Wreck>>,
    body: Query<Entity, With<QuadBody>>,
    exhausts: Query<&Exhaust>,
    wheels: Query<(&WheelVisual, &GlobalTransform)>,
    cams: Query<&GlobalTransform, With<Camera3d>>,
) {
    let rider_fx = wreck.map(|mut w| std::mem::take(&mut w.water_fx)).unwrap_or_default();
    let (Some(mut fx), Some(player)) = (fx, player) else { return };
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for e in rider_fx {
        match e {
            WaterFx::Splash { at, motion, radius } => fx.splash(at, motion, radius),
            WaterFx::Stir(at) => fx.add_ripples(stir(at)),
        }
    }
    let mut quads = vec![Quad { v: &player.0, body: body.single().ok(), boosting: boost.is_some_and(|b| b.on), human: true }];
    for r in opponents.iter().flat_map(|o| &o.racers) {
        quads.push(Quad { v: &r.vehicle, body: Some(r.body), boosting: r.boost.on, human: r.human.is_some() });
    }
    fx.puff.resize(quads.len(), 1.0);
    fx.wet.resize(quads.len(), false);
    // Each quad's water sensor (0xd7f60): coming down fast into deep water,
    // it splashes where it went in.
    for (k, q) in quads.iter().enumerate() {
        let v = q.v;
        let wet = water.as_deref().and_then(|w| w.surface(v.position)).is_some_and(|s| v.position.y - s <= -UNDER);
        if wet
            && !fx.wet[k]
            && -v.velocity.y > SPLASH_SPEED
            && let Some(level) = water.as_deref().and_then(|w| w.level(v.position))
        {
            fx.splash(v.position.with_y(level), v.velocity / FPS, reach(v));
        }
        fx.wet[k] = wet;
    }
    let centres: HashMap<(Entity, usize), Vec3> =
        wheels.iter().flat_map(|(w, t)| w.wheels.iter().map(move |&i| ((w.owner, i), t.translation()))).collect();
    let cams: Vec<Vec3> = cams.iter().map(GlobalTransform::translation).collect();
    for (k, q) in quads.iter().enumerate() {
        // 0x8e380: computer riders' quads throw half as much, and it all
        // thins out to nothing 50 m from the camera.
        let near = cams.iter().map(|c| c.distance(q.v.position)).fold(f32::MAX, f32::min);
        let rate = if q.human { 1.0 } else { 0.5 } * ((FAR - near) / FAR).clamp(0.0, 1.0);
        let exhaust = q.body.and_then(|e| exhausts.get(e).ok());
        if let Some(ex) = exhaust {
            exhaust_smoke(&mut fx, k, q, ex, rate, dt);
        }
        if rate <= 0.0 || fx.wet[k] || depth(water.as_deref(), q.v.position) > 0.05 {
            continue;
        }
        if let Some(e) = q.body {
            wheel_spray(&mut fx, q.v, e, &centres, rate, dt);
        }
    }
}

/// What the wheels throw up: chunks and dust off the spray (0x8bf40), and
/// dust and smoke behind wheels turning fast on dusty ground (0x8b900).
fn wheel_spray(fx: &mut Effects, v: &Vehicle, body: Entity, centres: &HashMap<(Entity, usize), Vec3>, rate: f32, dt: f32) {
    let (fwd, up, right) = (v.forward(), v.up(), v.right());
    let ahead = v.velocity.dot(fwd);
    let speed = v.velocity.length();
    let dust = fx.sprite("Dust0");
    for (i, (w, wp)) in v.wheels.iter().zip(&v.params.wheels).enumerate() {
        let Some(&centre) = centres.get(&(body, i)) else { continue };
        let s = w.contact.then(|| fx.by_id.get(usize::from(w.surface)).cloned().flatten()).flatten().unwrap_or_else(|| fx.surface.clone());
        let normal = if w.contact { w.contact_normal } else { up };
        let contact = centre - normal * wp.radius;
        // The tread's speed: rolling with the ground, and spinning past it
        // on the driven (rear) wheels.
        let tread = if wp.front { ahead } else { ahead + v.wheelspin() * (MAX_TREAD - ahead).max(0.0) };
        let spray = 1.8 * v.velocity - tread * fwd;
        let mut amount = (spray.length_squared() - SPRAY_MIN * SPRAY_MIN) / (SPRAY_FULL * SPRAY_FULL - SPRAY_MIN * SPRAY_MIN);
        if wp.front {
            amount *= (v.steer.abs() + 1.0) * 0.5;
        }
        let amount = amount.clamp(0.0, 1.0);
        if let Some(chunk) = s.chunk {
            for _ in 0..fx.rng.count(amount * s.chunk_prob * rate, dt) {
                let frames = chunk.frames();
                let pick = fx.rng.pick(frames.len());
                let sprite = fx.sprite(frames[pick]);
                let r = &mut fx.rng;
                let pos = contact + right * r.pm(0.3) + up * 0.01;
                let lift = r.pm(0.75) + f32::from(u8::from(w.contact)) + 0.5;
                let vel = spray * 0.1 * (1.0 + r.pm(0.25)) + right * SPRAY_MIN * r.pm(0.5) + up * SPRAY_MIN * lift + v.velocity * 0.5;
                let colour = tint(s.chunk_colour);
                let p = Particle {
                    rot: r.pm(std::f32::consts::PI),
                    spin: r.pm(4.0 * std::f32::consts::PI),
                    size: (0.12 + r.pm(0.08)) * s.chunk_size,
                    fade_out: 0.4 + r.pm(0.1),
                    start: colour,
                    end: colour,
                    ground: Some((normal, contact - normal * 0.01)),
                    ..Particle::new(sprite, pos, vel)
                };
                fx.add(Pool::Chunks, p);
            }
        }
        // Dust off the spray, behind the rear wheels on the ground.
        if s.dust && w.contact && !wp.front {
            for _ in 0..fx.rng.count(amount * s.chunk_dust_prob * rate, dt) {
                let r = &mut fx.rng;
                let colour = tint(s.dust_colour);
                let p = Particle {
                    rot: r.pm(std::f32::consts::PI),
                    spin: 0.5 + r.pm(0.5),
                    size: 0.2 + r.pm(0.1),
                    grow: 2.0 + r.pm(0.3),
                    fade_in: 0.05,
                    fade_out: 0.4 + r.pm(0.1),
                    alpha: 0.2,
                    start: colour,
                    end: colour,
                    ..Particle::new(dust, contact + right * r.pm(0.5), spray * 0.2 + Vec3::Y)
                };
                fx.add(Pool::Dust, p);
            }
        }
        // Dust behind a wheel turning fast on the ground.
        if s.dust && w.contact {
            let moving = if speed > 0.447 { (speed / DUST_FULL).min(1.0) } else { 0.0 };
            let turning = (tread.abs() / wp.radius / 100.0).min(1.0);
            let k = moving.min(turning);
            for _ in 0..fx.rng.count(k * k * s.dust_prob * rate, dt) {
                let r = &mut fx.rng;
                let start = tint(s.dust_colour) * (1.0 + r.pm(0.5));
                let end = tint(s.dust_colour) * (1.0 + r.pm(0.5));
                let p = Particle {
                    rot: r.pm(std::f32::consts::PI),
                    spin: 0.5 + r.pm(0.5),
                    size: 0.3 + r.pm(0.1),
                    grow: 2.0 + r.pm(0.5),
                    fade_in: 0.05,
                    fade_out: 0.4 + r.pm(0.1),
                    alpha: 0.2,
                    start,
                    end,
                    ..Particle::new(dust, centre - Vec3::Y * wp.radius, v.velocity * 0.15 + Vec3::Y)
                };
                fx.add(Pool::Dust, p);
                // Four times in ten, a big slow cloud too.
                if fx.rng.pick(10) > 5 {
                    let r = &mut fx.rng;
                    let p = Particle {
                        size: 0.6 + r.pm(0.2),
                        grow: 1.4 + r.pm(0.5),
                        fade_in: 0.5,
                        fade_out: 1.5 + r.pm(0.5),
                        alpha: 0.3,
                        start,
                        end,
                        ..Particle::new(dust, centre, v.velocity * 0.15 + Vec3::Y * 1.3)
                    };
                    fx.add(Pool::Smoke, p);
                }
            }
        }
    }
}

/// A Materials.xml colour as a particle tint. The original's reader packs
/// it as a vertex colour, 128 per 1.0 (retail 0x113ce0 via 0x15cd0), and the
/// wheel effects unpack it with 1/128 (0xa83b0), so the file's value is
/// the tint itself: sand's dust (0.8, 0.7, 0.6) is sand coloured.
fn tint(c: [f32; 4]) -> Vec3 {
    Vec3::new(c[0], c[1], c[2]).clamp(Vec3::ZERO, Vec3::splat(BRIGHTEST))
}

/// The exhaust (0x8b330): a puff now and then at low speed, more with the
/// throttle open at low revs; boosting, a burst of dark smoke that thins
/// out over a second, and a stream of sparks (0x8ada0).
fn exhaust_smoke(fx: &mut Effects, k: usize, q: &Quad, ex: &Exhaust, rate: f32, dt: f32) {
    let v = q.v;
    let tip = v.position + v.rotation * ex.tip;
    let fwd = v.forward();
    let dust = fx.sprite("Dust0");
    let spark = fx.sprite("Spark0");
    let pi = std::f32::consts::PI;
    if q.boosting {
        let puff = fx.puff[k];
        for _ in 0..fx.rng.count((puff * 5.0).floor(), dt) {
            let r = &mut fx.rng;
            let (a, b) = (1.0 + r.pm(0.5), 1.0 + r.pm(0.5));
            let p = Particle {
                rot: r.pm(pi),
                spin: 2.5 + r.pm(2.5),
                size: 0.275 + r.pm(0.125),
                grow: 4.0 + r.pm(1.0),
                fade_in: 0.05,
                fade_out: 0.45 + r.pm(0.25),
                alpha: 0.5,
                start: Vec3::new(0.25, 0.25, 0.2) * a,
                end: Vec3::new(0.25, 0.25, 0.2) * b,
                ..Particle::new(dust, tip, -fwd * (2.0 + r.pm(0.25)) + v.velocity * 1.5)
            };
            fx.add(Pool::Exhaust, p);
        }
        fx.puff[k] = (puff - dt).max(0.0);
        for _ in 0..fx.rng.count(3.0, dt) {
            let r = &mut fx.rng;
            let jitter = Vec3::new(r.pm(0.03), r.pm(0.03), r.pm(0.03));
            let spread = Vec3::new(r.pm(0.25), r.pm(0.25), r.pm(0.25));
            let p = Particle {
                // Yellow going orange (the spark pool's colours).
                start: Vec3::new(0.75, 1.0, 0.4),
                end: Vec3::new(1.0, 0.75, 0.4),
                ..Particle::new(spark, tip + jitter, v.velocity - fwd * 2.0 + spread)
            };
            fx.add(Pool::Sparks, p);
        }
        return;
    }
    fx.puff[k] = (fx.puff[k] + dt).min(1.0);
    let speed = v.velocity.length();
    if speed > SPRAY_FULL || rate <= 0.0 {
        return;
    }
    let max = v.params.max_revs.max(1.0);
    let chance = (SPRAY_FULL - speed) / SPRAY_FULL * (v.throttle / max * (max - v.revs).max(0.0) + 0.25) * 2.0;
    for _ in 0..fx.rng.count(chance * rate, dt) {
        let r = &mut fx.rng;
        let push = (v.revs * 0.002 + v.throttle * 35.0) * (1.0 + r.pm(0.5));
        let (a, b) = (1.0 + r.pm(0.5), 1.0 + r.pm(0.5));
        let p = Particle {
            rot: r.pm(pi),
            spin: 2.5 + r.pm(2.5),
            size: 0.55 + r.pm(0.25),
            grow: 4.0 + r.pm(1.0),
            fade_in: 0.05,
            fade_out: 0.45 + r.pm(0.25),
            alpha: 0.5,
            start: Vec3::new(0.5, 0.5, 0.75) * a,
            end: Vec3::new(0.7, 0.7, 0.85) * b,
            ..Particle::new(dust, tip, -fwd * push + v.velocity)
        };
        fx.add(Pool::Exhaust, p);
    }
}

fn step(time: Res<Time>, fx: Option<ResMut<Effects>>) {
    let Some(mut fx) = fx else { return };
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (pool, list) in &mut fx.pools {
        let spec = pool.spec();
        list.retain_mut(|p| p.step(&spec, dt));
    }
    fx.ripples.retain_mut(|r| r.step(dt));
}

/// Rebuilds each pool's mesh: a sprite per particle, facing the nearest
/// camera.
fn draw(fx: Option<Res<Effects>>, cams: Query<&GlobalTransform, With<Camera3d>>, mut meshes: ResMut<Assets<Mesh>>) {
    let Some(fx) = fx else { return };
    let cams: Vec<(Vec3, Vec3, Vec3)> = cams.iter().map(|c| (c.translation(), c.right().into(), c.up().into())).collect();
    for pool in POOLS {
        let Some(mut mesh) = fx.meshes.get(&pool).and_then(|h| meshes.get_mut(h)) else { continue };
        let spec = pool.spec();
        let list = fx.pools.get(&pool);
        let n = list.map_or(0, VecDeque::len);
        let mut pos = Vec::with_capacity(n * 4);
        let mut uv = Vec::with_capacity(n * 4);
        let mut col = Vec::with_capacity(n * 4);
        let mut idx = Vec::with_capacity(n * 6);
        for p in list.into_iter().flatten() {
            let Some(&(_, right, up)) = cams.iter().min_by(|a, b| a.0.distance_squared(p.pos).total_cmp(&b.0.distance_squared(p.pos))) else {
                break;
            };
            let (s, c) = p.rot.sin_cos();
            let half = spec.scale * p.size.max(0.0) * 0.5;
            let x = (right * c + up * s) * half.x;
            let y = (up * c - right * s) * half.y;
            let (_, min, max) = fx.sprites.get(p.sprite).cloned().unwrap_or_default();
            let base = pos.len() as u32;
            for (corner, t) in [(-x + y, min), (x + y, Vec2::new(max.x, min.y)), (x - y, max), (-x - y, Vec2::new(min.x, max.y))] {
                pos.push((p.pos + corner).to_array());
                uv.push(t.to_array());
            }
            let rgba = p.colour(spec.life);
            let rgb = rgba.truncate().clamp(Vec3::ZERO, Vec3::splat(BRIGHTEST));
            let linear = Color::srgba(rgb.x, rgb.y, rgb.z, rgba.w).to_linear().to_f32_array();
            col.extend([linear; 4]);
            idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        if idx.is_empty() {
            // An empty mesh upsets the renderer: one invisible point instead.
            pos.extend([[0.0; 3]; 3]);
            uv.extend([[0.0; 2]; 3]);
            col.extend([[0.0; 4]; 3]);
            idx.extend([0, 1, 2]);
        }
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
        mesh.insert_indices(Indices::U32(idx));
    }
    draw_ripples(&fx, &mut meshes);
}

/// The ripples: each a flat square on the water (0x107010), just above it.
fn draw_ripples(fx: &Effects, meshes: &mut Assets<Mesh>) {
    let Some(mut mesh) = meshes.get_mut(&fx.ripple_mesh) else { return };
    let (_, min, max) = fx.sprites.get(fx.sprite("Ripple")).cloned().unwrap_or_default();
    let (mut pos, mut uv, mut col, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for r in &fx.ripples {
        let Some(alpha) = r.opacity() else { continue };
        let base = pos.len() as u32;
        let s = r.size.max(0.0);
        let at = r.pos + Vec3::Y * 0.03;
        for (dx, dz, t) in [(-s, -s, min), (s, -s, Vec2::new(max.x, min.y)), (s, s, max), (-s, s, Vec2::new(min.x, max.y))] {
            pos.push((at + Vec3::new(dx, 0.0, dz)).to_array());
            uv.push(t.to_array());
        }
        col.extend([[1.0, 1.0, 1.0, alpha.clamp(0.0, 1.0)]; 4]);
        idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    if idx.is_empty() {
        pos.extend([[0.0; 3]; 3]);
        uv.extend([[0.0; 2]; 3]);
        col.extend([[0.0; 4]; 3]);
        idx.extend([0, 1, 2]);
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    mesh.insert_indices(Indices::U32(idx));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_bounce_off_their_ground() {
        let spec = Pool::Chunks.spec();
        let mut p = Particle { ground: Some((Vec3::Y, Vec3::ZERO)), ..Particle::new(0, Vec3::new(0.0, 0.05, 0.0), Vec3::new(1.0, -3.0, 0.0)) };
        for _ in 0..10 {
            assert!(p.step(&spec, 1.0 / 60.0));
        }
        assert!(p.pos.y >= 0.0 && p.vel.y > 0.0, "{p:?}");
        // Gone after its life and a half second.
        assert!(!p.step(&spec, 2.0));
    }

    #[test]
    fn exhaust_smoke_hangs() {
        // Down to a crawl within a fifth of a second, whatever the frame rate.
        let spec = Pool::Exhaust.spec();
        let mut a = Particle::new(0, Vec3::ZERO, Vec3::new(0.0, 0.0, 30.0));
        let mut b = a.clone();
        for _ in 0..12 {
            a.step(&spec, 1.0 / 60.0);
        }
        for _ in 0..6 {
            b.step(&spec, 1.0 / 30.0);
        }
        assert!(a.vel.z < 0.5 && (a.vel.z - b.vel.z).abs() < 1e-3, "{} {}", a.vel.z, b.vel.z);
    }

    #[test]
    fn fades_in_and_out() {
        let p = Particle { fade_in: 0.1, fade_out: 0.5, alpha: 0.4, ..Particle::new(0, Vec3::ZERO, Vec3::ZERO) };
        let at = |age: f32| Particle { age, ..p.clone() }.colour(1.0).w;
        assert!((at(0.05) - 0.2).abs() < 1e-6);
        assert!((at(0.3) - 0.4).abs() < 1e-6);
        assert!((at(0.75) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn counts_per_frame_over_time() {
        let mut r = Rng(1);
        assert_eq!(r.count(3.0, 1.0 / 60.0), 3);
        assert_eq!(r.count(3.0, 1.0 / 30.0), 6);
        let hits: usize = (0..6000).map(|_| r.count(0.25, 1.0 / 60.0)).sum();
        assert!((1300..1700).contains(&hits), "{hits}");
    }

    #[test]
    fn splashes_harder_from_higher() {
        let mut rng = Rng(7);
        // Dropping 0.25 m a frame: a full splash.
        let (spray, ripples) = splash(&mut rng, 3, Vec3::new(10.0, 2.0, 0.0), Vec3::new(0.1, -0.25, 0.0), 1.4);
        assert_eq!((spray.len(), ripples.len()), (32, 20));
        assert!(spray.iter().all(|p| p.sprite == 3 && p.life_scale == 1.0 && p.vel.y >= 3.0 && (p.pos - Vec3::new(10.0, 2.0, 0.0)).length() <= 1.05));
        assert!(ripples.iter().all(|r| r.pos.y == 2.0 && (r.pos.x - 10.0).abs() <= 11.2));
        // 0.1 m a frame: half as strong; the spray lives three quarters as long.
        let (spray, ripples) = splash(&mut rng, 3, Vec3::ZERO, Vec3::new(0.0, -0.1, 0.0), 1.4);
        assert!(spray.iter().all(|p| p.life_scale == 0.75 && p.vel.y <= 3.5));
        assert!(ripples.iter().all(|r| r.pos.length() <= 5.6));
    }

    #[test]
    fn ripples_wait_grow_and_fade() {
        let mut r = Ripple::new(Vec3::ZERO, 0.25, 1.5, 0.5, 0.2);
        assert!(r.step(0.1) && r.opacity().is_none() && r.size == 0.25);
        assert!(r.step(0.1) && r.opacity() == Some(0.5));
        assert!(r.step(1.0));
        assert!((r.size - 1.75).abs() < 1e-5 && (r.opacity().unwrap() - 0.25).abs() < 1e-5);
        assert!(!r.step(1.0));
        assert_eq!(stir(Vec3::ONE).map(|r| r.delay), [0.0, 0.05, 0.1]);
    }

    #[test]
    fn material_tint() {
        // The file's colour is the tint: sand's dust stays sand coloured
        // instead of going white, and nothing passes the brightest.
        assert_eq!(tint([0.8, 0.7, 0.6, 1.0]), Vec3::new(0.8, 0.7, 0.6));
        assert_eq!(tint([3.0, 0.5, -1.0, 1.0]), Vec3::new(BRIGHTEST, 0.5, 0.0));
    }
}
