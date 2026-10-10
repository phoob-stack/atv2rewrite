//! Bevy viewer: shows a track from the player's own disc with a quad on the
//! start line. Fly with WASD/QE, look by dragging with the right mouse button.
//!
//! Usage: atv-viewer --game <disc image or folder> [--track Dunes1]
//!                   [--quad Bandit] [--screenshot out.png]

mod convert;

use std::path::PathBuf;
use std::sync::Arc;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use atv_formats::GameSource;

#[derive(Resource, Clone)]
struct Args {
    game: PathBuf,
    track: String,
    quad: String,
    screenshot: Option<PathBuf>,
    /// Optional "x,y,z,tx,ty,tz" camera eye and target in game coordinates.
    camera: Option<[f32; 6]>,
}

#[derive(Resource, Clone)]
struct Game(Arc<GameSource>);

#[derive(Component)]
struct FlyCam {
    yaw: f32,
    pitch: f32,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut game = std::env::var_os("ATV2_GAME").map(PathBuf::from);
    let mut track = "Dunes1".to_string();
    let mut quad = "Bandit".to_string();
    let mut screenshot = None;
    let mut camera = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--game" => game = it.next().map(PathBuf::from),
            "--track" => track = it.next().unwrap_or(track),
            "--quad" => quad = it.next().unwrap_or(quad),
            "--screenshot" => screenshot = it.next().map(PathBuf::from),
            "--camera" => {
                let v: Vec<f32> = it.next().unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect();
                camera = v.try_into().ok();
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let game = game.ok_or_else(|| {
        anyhow::anyhow!("pass --game <your ATV 2 disc image or extracted folder> (or set ATV2_GAME)")
    })?;
    Ok(Args { game, track, quad, screenshot, camera })
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let game = Game(Arc::new(GameSource::open(&args.game)?));
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!("ATV 2 viewer - {}", args.track),
                resolution: (1280, 720).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.55, 0.7, 0.9)))
        .insert_resource(args)
        .insert_resource(game)
        .add_systems(Startup, setup)
        .add_systems(Update, (fly_camera, auto_screenshot))
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
    let scene = match loader.spawn_track(&mut commands, &args.track, &args.quad) {
        Ok(s) => s,
        Err(e) => {
            error!("failed to load track {}: {e:#}", args.track);
            return;
        }
    };
    info!(
        "loaded {}: {} world meshes, {} props, quad at {:?}",
        args.track, scene.world_meshes, scene.props, scene.quad_pos
    );
    // Chase-style view from behind and above the quad.
    let target = scene.quad_pos + scene.quad_forward * 6.0 + Vec3::Y * 1.0;
    let eye = scene.quad_pos - scene.quad_forward * 5.0 + Vec3::Y * 2.5;
    let (eye, target) = match args.camera {
        // Game coordinates are left-handed: flip Z.
        Some([x, y, z, tx, ty, tz]) => (Vec3::new(x, y, -z), Vec3::new(tx, ty, -tz)),
        None => (eye, target),
    };
    let look = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    let (yaw, pitch, _) = look.rotation.to_euler(EulerRot::YXZ);
    commands.spawn((Camera3d::default(), look, FlyCam { yaw, pitch }));
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

/// With --screenshot, waits for textures to upload, saves one frame, exits.
fn auto_screenshot(
    mut commands: Commands,
    args: Res<Args>,
    time: Res<Time>,
    mut taken: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(path) = &args.screenshot else { return };
    // Software renderers take a while to upload everything; wait it out.
    let now = time.elapsed_secs();
    match *taken {
        None if now > 8.0 => {
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.clone()));
            *taken = Some(now);
        }
        Some(t) if now > t + 4.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
