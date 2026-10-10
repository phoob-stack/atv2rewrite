//! ATV Quad Power Racing 2 for PC: drive a quad around a track loaded from
//! the player's own copy of the game.
//!
//! Usage: atv2 --game <disc image or folder> [--track Dunes1] [--quad Bandit]
//!             [--autodrive] [--screenshot out.png [--after SECONDS]]
//!             [--camera x,y,z,tx,ty,tz]

mod convert;

use std::path::PathBuf;
use std::sync::Arc;

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use atv_formats::GameSource;
use atv_sim::track as trk;
use atv_sim::{Input, TrackCollision, Vehicle, VehicleParams};

use convert::WheelVisual;

#[derive(Resource, Clone)]
struct Args {
    game: PathBuf,
    track: String,
    quad: String,
    screenshot: Option<PathBuf>,
    screenshot_after: f32,
    camera: Option<[f32; 6]>,
    autodrive: bool,
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

#[derive(Component)]
struct QuadBody;

#[derive(Component)]
struct ChaseCam;

#[derive(Component)]
struct FlyCam {
    yaw: f32,
    pitch: f32,
}

#[derive(Component)]
struct Hud;

fn parse_args() -> anyhow::Result<Args> {
    let mut a = Args {
        game: std::env::var_os("ATV2_GAME").map(PathBuf::from).unwrap_or_default(),
        track: "Dunes1".into(),
        quad: "Bandit".into(),
        screenshot: None,
        screenshot_after: 8.0,
        camera: None,
        autodrive: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().unwrap_or_default();
        match arg.as_str() {
            "--game" => a.game = PathBuf::from(val()),
            "--track" => a.track = val(),
            "--quad" => a.quad = val(),
            "--screenshot" => a.screenshot = Some(PathBuf::from(val())),
            "--after" => a.screenshot_after = val().parse()?,
            "--autodrive" => a.autodrive = true,
            "--camera" => {
                let v: Vec<f32> = val().split(',').filter_map(|s| s.trim().parse().ok()).collect();
                a.camera = v.try_into().ok();
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    if a.game.as_os_str().is_empty() {
        anyhow::bail!("pass --game <your ATV 2 disc image or extracted folder> (or set ATV2_GAME)");
    }
    Ok(a)
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let game = Game(Arc::new(GameSource::open(&args.game)?));
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "ATV Quad Power Racing 2".into(),
                resolution: (1280, 720).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.55, 0.7, 0.9)))
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(args)
        .insert_resource(game)
        .add_systems(Startup, setup)
        .add_systems(FixedUpdate, drive)
        .add_systems(Update, (sync_quad, chase_camera, fly_camera, hud, auto_screenshot).chain())
        .run();
    Ok(())
}

fn setup(
    mut commands: Commands,
    args: Res<Args>,
    game: Res<Game>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let mut loader = convert::Loader::new(&game.0, &mut meshes, &mut materials, &mut images);
    let scene = match loader.spawn_track(&mut commands, &args.track) {
        Ok(s) => s,
        Err(e) => {
            error!("failed to load track {}: {e:#}", args.track);
            return;
        }
    };
    let collision = trk::collision_from_world(&scene.world);
    let start = trk::start_pose(&scene.world, &collision, 1);
    let checkpoints = trk::checkpoints(&scene.world);

    let generic = game.0.read("/Cars/Generic/Generic.params").map(|b| String::from_utf8_lossy(&b).into_owned());
    let own = game.0.read(&format!("/Cars/{q}/{q}.params", q = args.quad)).ok();
    let own = own.map(|b| String::from_utf8_lossy(&b).into_owned());
    let params = VehicleParams::from_files(&generic.unwrap_or_default(), own.as_deref());
    let wheel_x: Vec<f32> = params.wheels.iter().map(|w| w.offset[0]).collect();
    let vehicle = Vehicle::new(params, start.0, start.1);

    let at = Transform::from_translation(start.0).with_rotation(start.1);
    match loader.spawn_quad(&mut commands, &args.quad, at, &wheel_x) {
        Ok(e) => {
            commands.entity(e).insert(QuadBody);
        }
        Err(e) => error!("failed to load quad {}: {e:#}", args.quad),
    }
    info!(
        "loaded {}: {} world meshes, {} props, {} collision tris, {} checkpoints",
        args.track,
        scene.world_meshes,
        scene.props,
        collision.triangle_count(),
        checkpoints.len()
    );

    let eye = start.0 - (start.1 * Vec3::NEG_Z) * 6.0 + Vec3::Y * 2.5;
    let cam = Transform::from_translation(eye).looking_at(start.0, Vec3::Y);
    match args.camera {
        Some([x, y, z, tx, ty, tz]) => {
            let t = Transform::from_xyz(x, y, -z).looking_at(Vec3::new(tx, ty, -tz), Vec3::Y);
            let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
            commands.spawn((Camera3d::default(), t, FlyCam { yaw, pitch }));
        }
        None => {
            commands.spawn((Camera3d::default(), cam, ChaseCam));
        }
    }
    commands.spawn((
        Text::new(""),
        TextFont { font_size: FontSize::Px(28.0), ..default() },
        TextColor(Color::WHITE),
        Node { position_type: PositionType::Absolute, bottom: px(16), right: px(24), ..default() },
        Hud,
    ));
    commands.insert_resource(Race {
        collision,
        checkpoints,
        next_checkpoint: 0,
        start,
        safe: start,
        safe_timer: 0.0,
        upside_down: 0.0,
    });
    commands.insert_resource(Player(vehicle));
}

fn read_input(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>) -> Input {
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
        i.brake = i.brake.max(if pad.pressed(GamepadButton::West) { 1.0 } else { 0.0 });
        i.jump |= pad.pressed(GamepadButton::RightTrigger) || pad.pressed(GamepadButton::East);
    }
    i
}

fn drive(
    time: Res<Time>,
    args: Res<Args>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    race: Option<ResMut<Race>>,
    player: Option<ResMut<Player>>,
) {
    let (Some(mut race), Some(mut player)) = (race, player) else { return };
    let dt = time.delta_secs();
    let v = &mut player.0;

    let input = if args.autodrive && !race.checkpoints.is_empty() {
        let n = race.checkpoints.len();
        let target = race.checkpoints[race.next_checkpoint % n];
        trk::steer_towards(v, target)
    } else {
        read_input(&keys, &pads)
    };
    if let Some(&cp) = race.checkpoints.get(race.next_checkpoint % race.checkpoints.len().max(1))
        && (cp - v.position).with_y(0.0).length() < 25.0
    {
        race.next_checkpoint += 1;
    }

    v.step(&input, dt, &race.collision);

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
        race.next_checkpoint = 0;
    }
}

fn sync_quad(
    player: Option<Res<Player>>,
    mut body: Query<&mut Transform, (With<QuadBody>, Without<WheelVisual>)>,
    mut wheels: Query<(&WheelVisual, &mut Transform), Without<QuadBody>>,
) {
    let Some(player) = player else { return };
    let v = &player.0;
    if let Ok(mut t) = body.single_mut() {
        t.translation = v.position;
        t.rotation = v.rotation;
    }
    for (w, mut t) in &mut wheels {
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
}

fn chase_camera(
    time: Res<Time>,
    player: Option<Res<Player>>,
    mut cam: Query<&mut Transform, With<ChaseCam>>,
) {
    let (Some(player), Ok(mut t)) = (player, cam.single_mut()) else { return };
    let v = &player.0;
    let fwd = v.forward().with_y(0.0).normalize_or(Vec3::NEG_Z);
    let desired = v.position - fwd * 5.5 + Vec3::Y * 2.2;
    let k = 1.0 - (-6.0 * time.delta_secs()).exp();
    t.translation = t.translation.lerp(desired, k);
    // Never let the camera sink below the quad's level.
    t.translation.y = t.translation.y.max(v.position.y + 0.8);
    let look = v.position + fwd * 3.0 + Vec3::Y * 0.6;
    t.look_at(look, Vec3::Y);
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

fn hud(player: Option<Res<Player>>, race: Option<Res<Race>>, mut text: Query<&mut Text, With<Hud>>) {
    let (Some(player), Some(race), Ok(mut text)) = (player, race, text.single_mut()) else { return };
    let v = &player.0;
    let gear = if v.gear == 0 { "R".to_string() } else { v.gear.to_string() };
    let n = race.checkpoints.len().max(1);
    text.0 = format!(
        "{:>3.0} MPH  gear {gear}  {:>5.0} rpm\ncheckpoint {}/{n}",
        v.speed_mph().abs(),
        v.revs,
        race.next_checkpoint % n + 1
    );
}

/// With --screenshot: waits (software renderers are slow to upload), saves
/// one frame, then exits.
fn auto_screenshot(
    mut commands: Commands,
    args: Res<Args>,
    time: Res<Time>,
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
