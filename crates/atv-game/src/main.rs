//! ATV Quad Power Racing 2 for PC: drive a quad around a track loaded from
//! the player's own copy of the game.
//!
//! Usage: atv2 --game <disc image or folder> [--track Dunes1] [--quad Bandit]
//!             [--rider Male1] [--laps 3] [--frontend [--screen rider]]
//!             [--mode race|timetrial|arcade|freestyle] [--minutes N]
//!             [--round N] [--opponents N]
//!             [--autodrive] [--screenshot out.png [--after SECONDS]]
//!             [--camera x,y,z,tx,ty,tz] [--orbit yaw,pitch,distance]
//!             [--resolution 1280x720] [--fullscreen]
//!
//! Without `--track` the game opens on its front end; with it, it goes
//! straight into a race (`--frontend` shows the menus anyway, preselected).
//! `--mode` picks the mode (`--minutes` is Open Freestyle's limit, 0 for
//! none; arcade races its rounds' tracks, `--round` starts on one).

mod convert;
mod freestyle;
mod frontend;
mod hud;
mod opponents;
mod race;
mod rider;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::light::GlobalAmbientLight;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::{MonitorSelection, WindowMode};

use atv_formats::GameSource;
use atv_sim::track as trk;
use atv_sim::{Input, TrackCollision, Vehicle, VehicleParams};

use convert::{SkyDome, WheelVisual};
use freestyle::{Session, StuntClips};
use frontend::{FrontEnd, Roster, Screen};
use opponents::OpponentRider;
use race::{ArcadeClock, RaceProgress};
use rider::Rider;
use ui::UiAssets;

#[derive(Resource, Clone)]
struct Args {
    game: PathBuf,
    track: String,
    quad: String,
    rider: String,
    screenshot: Option<PathBuf>,
    screenshot_after: f32,
    camera: Option<[f32; 6]>,
    /// Fixed close-up of the quad on the grid: degrees around it (0 = in
    /// front), degrees above it, metres away.
    orbit: Option<[f32; 3]>,
    autodrive: bool,
    resolution: (u32, u32),
    fullscreen: bool,
    laps: u32,
    /// Go straight into a race (a track was given) instead of the menus.
    direct: bool,
    /// Front-end screen to open on.
    screen: Option<Screen>,
    mode: Mode,
    /// Open Freestyle's limit in minutes (None = unlimited).
    minutes: Option<u32>,
    /// Arcade round to start on (0-based).
    round: usize,
    /// Computer riders in a single race.
    opponents: usize,
}

/// The game modes on the main menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Mode {
    #[default]
    SingleRace,
    TimeTrial,
    /// The rounds of `Setup/Arcade.xml` in a row, against a checkpoint
    /// clock.
    Arcade,
    /// Open Freestyle: roam a track scoring tricks, trick boxes and spots.
    Freestyle,
}

impl Mode {
    /// Watermark shown on the select screens.
    fn label(self) -> &'static str {
        match self {
            Mode::SingleRace => "SINGLE RACE",
            Mode::TimeTrial => "TIME TRIAL",
            Mode::Arcade => "ARCADE",
            Mode::Freestyle => "FREESTYLE",
        }
    }
}

#[derive(States, Default, Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum AppState {
    #[default]
    FrontEnd,
    /// The loading screen, shown for a few frames before a race loads.
    Loading,
    Race,
}

/// What the next race uses: picked in the front end or given on the
/// command line.
#[derive(Resource, Clone, Debug)]
struct Selection {
    track: String,
    quad: String,
    /// Rider model folder under `Characters`.
    rider: String,
    laps: u32,
    mode: Mode,
    /// Arcade round being raced (index into the roster's rounds).
    round: usize,
    /// Open Freestyle's time limit in minutes (None = unlimited).
    time_limit: Option<u32>,
}

#[derive(Resource, Clone)]
struct Game(Arc<GameSource>);

#[derive(Resource)]
struct Race {
    collision: TrackCollision,
    checkpoints: Vec<Vec3>,
    next_checkpoint: usize,
    start: (Vec3, Quat),
    /// Last pose where the quad was upright on the ground (for respots).
    safe: (Vec3, Quat),
    safe_timer: f32,
    upside_down: f32,
}

#[derive(Resource)]
struct Player(Vehicle);

/// The controls applied on the last physics step (the rider follows them).
#[derive(Resource, Default)]
struct Controls(Input);

#[derive(Component)]
struct QuadBody;

#[derive(Component)]
struct ChaseCam {
    /// Smoothed position before pulling in for terrain.
    ideal: Vec3,
}

#[derive(Component)]
struct FlyCam {
    yaw: f32,
    pitch: f32,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut a = Args {
        game: std::env::var_os("ATV2_GAME").map(PathBuf::from).unwrap_or_default(),
        track: "Dunes1".into(),
        quad: "Bandit".into(),
        rider: "Male1".into(),
        screenshot: None,
        screenshot_after: 8.0,
        camera: None,
        orbit: None,
        autodrive: false,
        resolution: (1280, 720),
        fullscreen: false,
        laps: 3,
        direct: false,
        screen: None,
        mode: Mode::SingleRace,
        minutes: Some(3),
        round: 0,
        // The original's default field (`mNumAIPlayers` in user.xml).
        opponents: 5,
    };
    let mut frontend = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().unwrap_or_default();
        match arg.as_str() {
            "--game" => a.game = PathBuf::from(val()),
            "--track" => {
                a.track = val();
                a.direct = true;
            }
            "--laps" => a.laps = val().parse()?,
            "--frontend" => frontend = true,
            "--screen" => {
                frontend = true;
                a.screen = Some(match val().to_ascii_lowercase().as_str() {
                    "title" => Screen::Title,
                    "main" => Screen::Main,
                    "rider" => Screen::Rider,
                    "atv" | "quad" => Screen::Quad,
                    "track" => Screen::Track,
                    other => anyhow::bail!("unknown screen {other} (title, main, rider, atv, track)"),
                });
            }
            "--mode" => {
                a.mode = match val().to_ascii_lowercase().as_str() {
                    "race" | "single" => Mode::SingleRace,
                    "timetrial" | "time" => Mode::TimeTrial,
                    "arcade" => Mode::Arcade,
                    "freestyle" => Mode::Freestyle,
                    other => anyhow::bail!("unknown mode {other} (race, timetrial, arcade, freestyle)"),
                }
            }
            "--minutes" => a.minutes = Some(val().parse::<u32>()?).filter(|&m| m > 0),
            "--round" => {
                a.round = val().parse()?;
                a.direct = true;
            }
            "--opponents" => a.opponents = val().parse::<usize>()?.min(7),
            "--quad" => a.quad = val(),
            "--rider" => a.rider = val(),
            "--screenshot" => a.screenshot = Some(PathBuf::from(val())),
            "--after" => a.screenshot_after = val().parse()?,
            "--autodrive" => a.autodrive = true,
            "--fullscreen" => a.fullscreen = true,
            "--resolution" => {
                let v = val();
                let (w, h) = v.split_once('x').ok_or_else(|| anyhow::anyhow!("--resolution wants WIDTHxHEIGHT, got {v}"))?;
                a.resolution = (w.parse()?, h.parse()?);
            }
            "--camera" => {
                let v: Vec<f32> = val().split(',').filter_map(|s| s.trim().parse().ok()).collect();
                a.camera = v.try_into().ok();
            }
            "--orbit" => {
                let v: Vec<f32> = val().split(',').filter_map(|s| s.trim().parse().ok()).collect();
                a.orbit = v.try_into().ok();
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    if frontend {
        a.direct = false;
    }
    if a.game.as_os_str().is_empty() {
        anyhow::bail!("pass --game <your ATV 2 disc image or extracted folder> (or set ATV2_GAME)");
    }
    Ok(a)
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let game = Game(Arc::new(GameSource::open(&args.game)?));
    let roster = Roster::load(&game.0);
    let mut selection = Selection {
        track: args.track.clone(),
        quad: args.quad.clone(),
        rider: args.rider.clone(),
        laps: args.laps.clamp(1, 9),
        mode: args.mode,
        round: args.round,
        time_limit: args.minutes,
    };
    // Arcade races its rounds' tracks (`--round N` starts on one).
    if args.mode == Mode::Arcade
        && let Some(t) = roster.arcade.get(args.round)
    {
        selection.track = t.clone();
    }
    let first = if args.direct { AppState::Loading } else { AppState::FrontEnd };
    let front_end = FrontEnd::at(args.screen.unwrap_or_default());
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "ATV Quad Power Racing 2".into(),
                resolution: args.resolution.into(),
                mode: if args.fullscreen {
                    WindowMode::BorderlessFullscreen(MonitorSelection::Current)
                } else {
                    WindowMode::Windowed
                },
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.55, 0.7, 0.9)))
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(args)
        .insert_resource(game)
        .insert_resource(roster)
        .insert_resource(selection)
        .insert_resource(front_end)
        .add_plugins((ui::UiPlugin, hud::HudPlugin, frontend::FrontEndPlugin, freestyle::FreestylePlugin, opponents::OpponentsPlugin))
        .insert_state(first)
        .add_systems(Startup, (load_ui, ui_camera))
        .add_systems(OnEnter(AppState::Race), setup)
        .add_systems(OnExit(AppState::Race), teardown)
        .add_systems(FixedUpdate, drive.run_if(in_state(AppState::Race)))
        .add_systems(
            Update,
            (sync_quad, animate_rider, stunts, rider::pose_riders, chase_camera, fly_camera, follow_sky).chain().run_if(in_state(AppState::Race)),
        )
        .add_systems(Update, auto_screenshot)
        .run();
    Ok(())
}

fn load_ui(mut commands: Commands, game: Res<Game>, mut images: ResMut<Assets<Image>>) {
    match UiAssets::load(&game.0, &mut images) {
        Ok(ui) => commands.insert_resource(ui),
        Err(e) => error!("failed to load the game's 2D art: {e:#}"),
    }
}

/// Draws the menus and loading screen; the race brings its own camera.
#[derive(Component)]
struct UiCamera;

fn ui_camera(mut commands: Commands) {
    commands.spawn((Camera2d, UiCamera, IsDefaultUiCamera));
}

/// The race camera takes over drawing the UI (the HUD) while racing.
fn set_ui_camera(commands: &mut Commands, cams: &mut Query<(Entity, &mut Camera), With<UiCamera>>, on: bool) {
    for (e, mut c) in cams {
        c.is_active = on;
        if on {
            commands.entity(e).insert(IsDefaultUiCamera);
        } else {
            commands.entity(e).remove::<IsDefaultUiCamera>();
        }
    }
}

fn teardown(mut commands: Commands, mut cams: Query<(Entity, &mut Camera), With<UiCamera>>) {
    commands.remove_resource::<Race>();
    commands.remove_resource::<Player>();
    commands.remove_resource::<Controls>();
    commands.remove_resource::<RaceProgress>();
    commands.remove_resource::<Session>();
    commands.remove_resource::<StuntClips>();
    set_ui_camera(&mut commands, &mut cams, true);
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    args: Res<Args>,
    sel: Res<Selection>,
    roster: Res<Roster>,
    mut ui_cams: Query<(Entity, &mut Camera), With<UiCamera>>,
    game: Res<Game>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut clear: ResMut<ClearColor>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    set_ui_camera(&mut commands, &mut ui_cams, false);
    let scoped = DespawnOnExit(AppState::Race);
    let mut loader = convert::Loader::new(&game.0, &mut meshes, &mut materials, &mut images);
    let scene = match loader.spawn_track(&mut commands, &sel.track) {
        Ok(s) => s,
        Err(e) => {
            error!("failed to load track {}: {e:#}", sel.track);
            commands.queue(|w: &mut World| w.resource_mut::<NextState<AppState>>().set(AppState::FrontEnd));
            return;
        }
    };
    for &e in &scene.roots {
        commands.entity(e).insert(scoped.clone());
    }
    let collision = trk::collision_from_world(&scene.world);
    let start = trk::start_pose(&scene.world, &collision, 1);
    let checkpoints = trk::checkpoints(&scene.world);

    let generic = game.0.read("/Cars/Generic/Generic.params").map(|b| String::from_utf8_lossy(&b).into_owned());
    let own = game.0.read(&format!("/Cars/{q}/{q}.params", q = sel.quad)).ok();
    let own = own.map(|b| String::from_utf8_lossy(&b).into_owned());
    let params = VehicleParams::from_files(&generic.unwrap_or_default(), own.as_deref());
    let wheel_x: Vec<f32> = params.wheels.iter().map(|w| w.offset[0]).collect();
    let vehicle = Vehicle::new(params, start.0, start.1);

    let at = Transform::from_translation(start.0).with_rotation(start.1);
    match loader.spawn_quad(&mut commands, &sel.quad, at, &wheel_x) {
        Ok(e) => {
            commands.entity(e).insert((QuadBody, scoped.clone()));
            // The riding animations are in the quad model's space, which
            // hangs below the physics body like the quad's own model.
            let seat = commands.spawn((Transform::from_xyz(0.0, -convert::COM_HEIGHT, 0.0), Visibility::default())).id();
            commands.entity(e).add_child(seat);
            let rider = loader
                .spawn_rider(&mut commands, &mut bindposes, &sel.rider, seat)
                .and_then(|rig| Rider::new(&game.0, rig));
            match rider {
                Ok(r) => {
                    commands.entity(seat).insert(r);
                }
                Err(e) => error!("failed to load rider {}: {e:#}", sel.rider),
            }
        }
        Err(e) => error!("failed to load quad {}: {e:#}", sel.quad),
    }
    if sel.mode == Mode::SingleRace {
        opponents::spawn(&mut commands, &mut loader, &mut bindposes, &game.0, &roster, &sel, &scene.world, &collision, args.opponents);
    }
    info!(
        "loaded {}: {} world meshes, {} props, {} collision tris, {} checkpoints",
        sel.track,
        scene.world_meshes,
        scene.props,
        collision.triangle_count(),
        checkpoints.len()
    );

    // The world's lighting is baked into vertex colours; only the quad and
    // rider are lit, by the track's sun and ambient lights. Light colours use
    // the same 0x80 = 1.0 scale as vertex colours (the Xbox lit vehicles
    // with fixed-function lights and MODULATE2X), so a surface facing a
    // 0x80 sun shows its texture as authored. Bevy lights are physical:
    // divide out the camera exposure (and pi for the diffuse BRDF) to land
    // on that scale.
    let exposure = Exposure::default().exposure();
    let rgb = |c: [f32; 3]| Color::linear_rgb(c[0] / 128.0, c[1] / 128.0, c[2] / 128.0);
    // Tracks without a sun (Stadium, Swamp, Industrial, Forest) get one
    // scaled to the baked light around the start, so the quad doesn't
    // glow against a dark world.
    let level = baked_level(&scene.world, start.0);
    let mut sun = (Vec3::new(0.3, -0.8, -0.5), Color::linear_rgb(0.6 * level, 0.6 * level, 0.6 * level));
    let mut have_ambient = false;
    ambient.brightness = 1.0 / exposure;
    for l in &scene.world.lights {
        match l.name.to_ascii_lowercase().as_str() {
            "sun" => sun = (trk::to_sim(l.dir), rgb(l.color)),
            "ambient" => {
                ambient.color = rgb(l.color);
                have_ambient = true;
            }
            _ => {}
        }
    }
    if !have_ambient {
        ambient.color = Color::linear_rgb(0.45 * level, 0.45 * level, 0.45 * level);
    }
    commands.spawn((
        DirectionalLight { color: sun.1, illuminance: std::f32::consts::PI / exposure, ..default() },
        Transform::default().looking_to(sun.0.normalize_or(Vec3::NEG_Y), Vec3::Y),
        scoped.clone(),
    ));
    let fog_color = {
        let c = scene.world.fog_color;
        Color::srgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
    };
    clear.0 = fog_color;
    let view = (
        Camera3d::default(),
        // Colours are baked; show them as authored.
        Tonemapping::None,
        Projection::Perspective(PerspectiveProjection { far: convert::SKY_RADIUS * 2.0, ..default() }),
        DistanceFog {
            color: fog_color,
            falloff: FogFalloff::Linear { start: 120.0, end: 900.0 },
            ..default()
        },
    );
    let eye = start.0 - (start.1 * Vec3::NEG_Z) * 6.0 + Vec3::Y * 2.5;
    let cam = Transform::from_translation(eye).looking_at(start.0, Vec3::Y);
    let fixed = match (args.camera, args.orbit) {
        (Some([x, y, z, tx, ty, tz]), _) => Some(Transform::from_xyz(x, y, -z).looking_at(Vec3::new(tx, ty, -tz), Vec3::Y)),
        (None, Some([yaw, pitch, dist])) => {
            let (yaw, pitch) = (yaw.to_radians(), pitch.to_radians());
            let dir = start.1 * Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos());
            let target = start.0 + Vec3::Y * 0.2;
            Some(Transform::from_translation(target + dir * dist).looking_at(target, Vec3::Y))
        }
        _ => None,
    };
    match fixed {
        Some(t) => {
            let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
            commands.spawn((view, t, FlyCam { yaw, pitch }, IsDefaultUiCamera, scoped.clone()));
        }
        None => {
            commands.spawn((view, cam, ChaseCam { ideal: eye }, IsDefaultUiCamera, scoped.clone()));
        }
    }
    // Whether the grid is on the start/finish line or still has to cross it.
    let on_line = checkpoints.first().is_some_and(|&c| (c - start.0).with_y(0.0).length() < 25.0);
    let mut progress = RaceProgress::new(sel.laps, on_line);
    if sel.mode == Mode::Arcade {
        let info = roster.info(&sel.track);
        progress = progress.with_arcade(ArcadeClock::new(info.arcade_start, info.worst_lap, checkpoints.len(), &race::arcade_gates(&scene.world)));
    }
    commands.insert_resource(progress);
    let session = Session::new(&game.0, &sel, &scene.world);
    commands.insert_resource(StuntClips::load(&game.0, &session.tricks));
    commands.insert_resource(session);
    commands.insert_resource(Race {
        collision,
        next_checkpoint: usize::from(on_line && checkpoints.len() > 1),
        checkpoints,
        start,
        safe: start,
        safe_timer: 0.0,
        upside_down: 0.0,
    });
    commands.insert_resource(Player(vehicle));
    commands.init_resource::<Controls>();
}

/// Average baked light (1.0 = texture as authored) of the track surface
/// within 40 m of `at`, clamped to a sensible range.
fn baked_level(world: &atv_formats::row::World, at: Vec3) -> f32 {
    let lum = |c: u32| (((c >> 16) & 255) + ((c >> 8) & 255) + (c & 255)) as f32 / 3.0 / 128.0;
    let (mut sum, mut n) = (0.0, 0.0);
    // Visible, opaque surfaces only (no water, foam or shadow decals).
    for m in world.meshes.iter().filter(|m| m.material >= 0 && m.flags & 0xE000 == 0) {
        let near = |p: [f32; 3]| trk::to_sim(p).distance_squared(at) < 40.0 * 40.0;
        for v in m.vertices.iter().filter(|v| near(v.pos)) {
            sum += lum(v.color);
            n += 1.0;
        }
        for p in m.patches.iter().filter(|p| near(p.control[0])) {
            sum += p.corner_color.iter().map(|&c| lum(c)).sum::<f32>() / 4.0;
            n += 1.0;
        }
    }
    if n == 0.0 { 1.0 } else { (sum / n).clamp(0.35, 1.2) }
}

/// `airborne`: the pad's X and RB are trick buttons in the air, so they
/// don't brake or pre-load there.
fn read_input(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>, airborne: bool) -> Input {
    let key = |k: KeyCode| -> f32 { if keys.pressed(k) { 1.0 } else { 0.0 } };
    let mut i = Input {
        throttle: key(KeyCode::ArrowUp).max(key(KeyCode::KeyW)),
        brake: key(KeyCode::ArrowDown).max(key(KeyCode::KeyS)),
        steer: key(KeyCode::ArrowRight).max(key(KeyCode::KeyD)) - key(KeyCode::ArrowLeft).max(key(KeyCode::KeyA)),
        lean: key(KeyCode::KeyI) - key(KeyCode::KeyK),
        jump: keys.pressed(KeyCode::Space),
    };
    for pad in pads {
        let stick_x = pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0);
        let stick_y = pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        let dead = |v: f32| if v.abs() < 0.15 { 0.0 } else { v };
        i.steer = (i.steer + dead(stick_x)).clamp(-1.0, 1.0);
        i.lean = (i.lean + dead(stick_y)).clamp(-1.0, 1.0);
        i.throttle = i.throttle.max(pad.get(GamepadButton::RightTrigger2).unwrap_or(0.0));
        i.throttle = i.throttle.max(if pad.pressed(GamepadButton::South) { 1.0 } else { 0.0 });
        i.brake = i.brake.max(pad.get(GamepadButton::LeftTrigger2).unwrap_or(0.0));
        if !airborne {
            i.brake = i.brake.max(if pad.pressed(GamepadButton::West) { 1.0 } else { 0.0 });
            i.jump |= pad.pressed(GamepadButton::RightTrigger);
        }
        i.jump |= pad.pressed(GamepadButton::East);
    }
    i
}

#[allow(clippy::too_many_arguments)]
fn drive(
    time: Res<Time>,
    args: Res<Args>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    race: Option<ResMut<Race>>,
    player: Option<ResMut<Player>>,
    progress: Option<ResMut<RaceProgress>>,
    session: Option<ResMut<Session>>,
    mut controls: Option<ResMut<Controls>>,
) {
    let (Some(mut race), Some(mut player), Some(mut progress), Some(mut session)) = (race, player, progress, session) else { return };
    let dt = time.delta_secs();
    let v = &mut player.0;
    progress.tick(dt);
    let freestyle = session.freestyle;

    let n = race.checkpoints.len();
    let next = race.next_checkpoint % n.max(1);
    // Held on the grid for the countdown; after the flag the autopilot
    // brings the quad home; out of time, it rolls to a stop.
    let over = if freestyle { session.over(progress.clock) } else { progress.over() };
    let input = if !progress.started() || (over && (freestyle || progress.out_of_time())) {
        // Brake to a stop (braking at a standstill would reverse).
        Input { brake: if over && v.speed_mph() > 2.0 { 0.3 } else { 0.0 }, ..default() }
    } else if let Some(spot) = session.next_spot(v.position).filter(|_| args.autodrive && freestyle) {
        // Freestyle's autopilot goes exploring.
        trk::steer_towards(v, spot)
    } else if (args.autodrive || progress.finished.is_some()) && n > 0 {
        trk::steer_towards(v, race.checkpoints[next])
    } else {
        read_input(&keys, &pads, !v.grounded())
    };
    if let Some(&cp) = race.checkpoints.get(next).filter(|_| freestyle) {
        // No laps in freestyle; the checkpoints only lead the autopilot.
        if (cp - v.position).with_y(0.0).length() < 25.0 {
            race.next_checkpoint = (next + 1) % n;
        }
    } else if let Some(&cp) = race.checkpoints.get(next) {
        let to_cp = (cp - v.position).with_y(0.0);
        if to_cp.length() < 25.0 {
            progress.reach(next);
            race.next_checkpoint = (next + 1) % n;
        }
        // Lap progress for the meter: checkpoints passed plus the share of
        // the way to the next one.
        let prev = race.checkpoints[(next + n - 1) % n];
        let leg = (cp - prev).with_y(0.0).length().max(1.0);
        let along = (1.0 - to_cp.length() / leg).clamp(0.0, 1.0);
        progress.lap_fraction = if next == 0 && !progress.armed() { 0.0 } else { (((next + n - 1) % n) as f32 + along) / n as f32 };
        let vel = v.velocity.with_y(0.0);
        let away = vel.length() > 4.0 && vel.normalize().dot(to_cp.normalize_or_zero()) < -0.3;
        progress.wrong_way = if away { progress.wrong_way + dt } else { 0.0 };
    }

    v.step(&input, dt, &race.collision);
    if let Some(c) = controls.as_mut() {
        c.0 = input;
    }
    let (mut buttons, mut dir) = freestyle::read_tricks(&keys, &pads);
    if args.autodrive && session.active.is_none() && !v.grounded() {
        // The autopilot shows off a different trick every so often.
        let k = (session.time * 0.7) as usize;
        buttons = 1 << (k % 4);
        dir = Some([freestyle::Dir::Up, freestyle::Dir::Right, freestyle::Dir::Down, freestyle::Dir::Left][k / 4 % 4]);
    }
    let live = progress.started() && !over;
    session.step(v, buttons, dir, dt, live);

    // Remember a safe spot; respot there on R, when stuck upside down, or
    // after falling out of the world.
    race.safe_timer += dt;
    if v.grounded() && v.up().y > 0.9 && race.safe_timer > 1.0 {
        race.safe = (v.position, v.rotation);
        race.safe_timer = 0.0;
    }
    race.upside_down = if v.up().y < 0.2 { race.upside_down + dt } else { 0.0 };
    let fell = v.position.y < race.safe.0.y - 40.0;
    if keys.just_pressed(KeyCode::KeyR) || race.upside_down > 2.5 || fell {
        let (p, r) = race.safe;
        v.reset(p + Vec3::Y * 0.5, r);
        race.upside_down = 0.0;
    }
    if keys.just_pressed(KeyCode::Backspace) {
        let (p, r) = race.start;
        v.reset(p, r);
        let on_line = race.checkpoints.first().is_some_and(|&c| (c - p).with_y(0.0).length() < 25.0);
        race.next_checkpoint = usize::from(on_line && n > 1);
        let arcade = progress.arcade.take();
        *progress = RaceProgress::new(progress.laps, on_line);
        progress.arcade = arcade;
    }
}

fn sync_quad(
    player: Option<Res<Player>>,
    mut body: Query<(Entity, &mut Transform), (With<QuadBody>, Without<WheelVisual>)>,
    mut wheels: Query<(&WheelVisual, &mut Transform), Without<QuadBody>>,
) {
    let Some(player) = player else { return };
    let v = &player.0;
    let Ok((quad, mut t)) = body.single_mut() else { return };
    t.translation = v.position;
    t.rotation = v.rotation;
    for (w, mut t) in &mut wheels {
        if w.owner == quad {
            place_wheel(w, &mut t, v);
        }
    }
}

/// Moves a wheel model to where the simulation has its wheel(s).
fn place_wheel(w: &WheelVisual, t: &mut Transform, v: &Vehicle) {
    let n = w.wheels.len() as f32;
    let (mut height, mut spin, mut steer) = (0.0, 0.0, 0.0);
    for &i in &w.wheels {
        let (Some(ws), Some(wp)) = (v.wheels.get(i), v.params.wheels.get(i)) else { continue };
        // Wheel centre = attachment + travel along the strut.
        height += wp.offset[1] + wp.min_pos + ws.compression;
        spin += ws.spin;
        steer += ws.steer;
    }
    let (height, spin, steer) = (height / n, spin / n, steer / n);
    t.translation = w.base.with_y(height + convert::COM_HEIGHT);
    let spin = if w.mirror { spin } else { -spin };
    t.rotation = Quat::from_rotation_y(steer) * Quat::from_rotation_x(spin);
}

fn animate_rider(
    time: Res<Time>,
    player: Option<Res<Player>>,
    controls: Option<Res<Controls>>,
    mut riders: Query<&mut Rider, Without<OpponentRider>>,
) {
    let (Some(player), Some(controls)) = (player, controls) else { return };
    let v = &player.0;
    for mut r in &mut riders {
        r.update_controls(v.steer, controls.0.lean, v.preload, time.delta_secs());
    }
}

/// The rider performs the trick in progress.
fn stunts(time: Res<Time>, session: Option<Res<Session>>, clips: Option<Res<StuntClips>>, mut riders: Query<&mut Rider, Without<OpponentRider>>) {
    let (Some(s), Some(clips)) = (session, clips) else { return };
    let stunt = s.active.as_ref().and_then(|a| {
        let clip = clips.0.get(a.def)?.clone()?;
        Some((clip, a.t / s.tricks[a.def].total))
    });
    for mut r in &mut riders {
        r.set_stunt(stunt.clone(), time.delta_secs());
    }
}

fn chase_camera(
    time: Res<Time>,
    player: Option<Res<Player>>,
    race: Option<Res<Race>>,
    mut cam: Query<(&mut Transform, &mut ChaseCam)>,
) {
    let (Some(player), Some(race), Ok((mut t, mut cam))) = (player, race, cam.single_mut()) else { return };
    let v = &player.0;
    let fwd = v.forward().with_y(0.0).normalize_or(Vec3::NEG_Z);
    let desired = v.position - fwd * 5.5 + Vec3::Y * 2.2;
    let k = 1.0 - (-6.0 * time.delta_secs()).exp();
    cam.ideal = cam.ideal.lerp(desired, k);
    // Never let the camera sink below the quad's level.
    cam.ideal.y = cam.ideal.y.max(v.position.y + 0.8);

    // Pull in when terrain or a wall is between the rider and the camera,
    // and keep clear of the ground under it.
    let pivot = v.position + Vec3::Y * 1.2;
    let to_cam = cam.ideal - pivot;
    let dist = to_cam.length();
    let mut eye = cam.ideal;
    if dist > 0.01 {
        let dir = to_cam / dist;
        if let Some(hit) = race.collision.raycast(pivot, dir, dist + 0.4) {
            eye = pivot + dir * (hit.t - 0.4).max(0.3);
        }
    }
    if let Some(ground) = race.collision.raycast(eye + Vec3::Y, Vec3::NEG_Y, 1.5) {
        eye.y = eye.y.max(ground.point.y + 0.5);
    }
    t.translation = eye;
    let look = v.position + fwd * 3.0 + Vec3::Y * 0.6;
    t.look_at(look, Vec3::Y);
}

fn follow_sky(cam: Query<&Transform, With<Camera3d>>, mut sky: Query<&mut Transform, (With<SkyDome>, Without<Camera3d>)>) {
    let (Ok(cam), Ok(mut sky)) = (cam.single(), sky.single_mut()) else { return };
    // Sunk a little so the hazy rim sits under the horizon.
    sky.translation = cam.translation - Vec3::Y * convert::SKY_RADIUS * 0.08;
}

fn fly_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut q: Query<(&mut Transform, &mut FlyCam)>,
) {
    let Ok((mut t, mut cam)) = q.single_mut() else { return };
    if buttons.pressed(MouseButton::Right) {
        cam.yaw -= motion.delta.x * 0.003;
        cam.pitch = (cam.pitch - motion.delta.y * 0.003).clamp(-1.5, 1.5);
        t.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
    }
    let mut dir = Vec3::ZERO;
    for (k, d) in [
        (KeyCode::KeyW, t.forward().as_vec3()),
        (KeyCode::KeyS, -t.forward().as_vec3()),
        (KeyCode::KeyA, -t.right().as_vec3()),
        (KeyCode::KeyD, t.right().as_vec3()),
        (KeyCode::KeyE, Vec3::Y),
        (KeyCode::KeyQ, -Vec3::Y),
    ] {
        if keys.pressed(k) {
            dir += d;
        }
    }
    let speed = if keys.pressed(KeyCode::ShiftLeft) { 120.0 } else { 25.0 };
    t.translation += dir.normalize_or_zero() * speed * time.delta_secs();
}

/// With --screenshot: waits (software renderers are slow to upload), saves
/// one frame, then exits.
fn auto_screenshot(
    mut commands: Commands,
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut taken: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(path) = &args.screenshot else { return };
    let now = time.elapsed_secs();
    match *taken {
        None if now > args.screenshot_after => {
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.clone()));
            *taken = Some(now);
        }
        Some(t) if now > t + 4.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
