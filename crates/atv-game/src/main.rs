//! ATV Quad Power Racing 2 for PC: drive a quad around a track loaded from
//! the player's own copy of the game.
//!
//! Usage: atv2 --game <disc image or folder> [--track Dunes1] [--quad Bandit]
//!             [--rider Male1] [--laps 3] [--frontend [--screen rider|tricks]]
//!             [--mode race|timetrial|arcade|freestyle|career] [--minutes N]
//!             [--round N] [--opponents N] [--challenge N] [--players N]
//!             [--autodrive] [--screenshot out.png [--after SECONDS]]
//!             [--camera x,y,z,tx,ty,tz] [--orbit yaw,pitch,distance]
//!             [--resolution 1280x720] [--fullscreen] [--cheat ALLOFEM]
//!
//! Without `--track` the game opens on its front end; with it, it goes
//! straight into a race (`--frontend` shows the menus anyway, preselected).
//! `--mode` picks the mode (`--minutes` is Open Freestyle's limit, 0 for
//! none; arcade races its rounds' tracks, `--round` starts on one; career
//! with `--round` races the saved championship's next round).

mod aitricks;
mod arcade;
mod attack;
mod audio;
mod boost;
mod career;
mod challenge;
mod convert;
mod crash;
mod demo;
mod effects;
mod freestyle;
mod frontend;
mod hd;
mod mud;
mod hud;
mod intro;
mod knock;
mod water;
mod mp;
mod opponents;
mod race;
mod replay;
mod respot;
mod rider;
mod settings;
mod trickbook;
mod ui;
mod unlocks;

use std::path::PathBuf;
use std::sync::Arc;

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton, GamepadRumbleIntensity, GamepadRumbleRequest};
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::light::GlobalAmbientLight;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::{MonitorSelection, WindowMode};

use atv_formats::aid::AiPath;
use atv_formats::GameSource;
use atv_sim::ai::RacingLine;
use atv_sim::track as trk;
use atv_sim::{Input, TrackCollision, Vehicle, VehicleParams};

use convert::{SkyDome, WheelVisual};
use crash::{Cause, CrashClips, Wreck};
use freestyle::{Session, StuntClips};
use frontend::{FrontEnd, Roster, Screen};
use opponents::OpponentRider;
use race::{ArcadeClock, RaceProgress};
use rider::Rider;
use settings::{CameraView, FollowCam, Settings};
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
    /// HD texture pack folder (default: the one the launcher builds).
    hd: Option<PathBuf>,
    use_hd: bool,
    mode: Mode,
    /// Open Freestyle's limit in minutes (None = unlimited).
    minutes: Option<u32>,
    /// Arcade round to start on (0-based).
    round: usize,
    /// Computer riders in a single race.
    opponents: usize,
    /// Freestyle challenge to run (1-based).
    challenge: usize,
    /// Humans in split screen (1 to 4).
    players: usize,
    /// Cheats to enter (see [`unlocks::CHEATS`]).
    cheats: Vec<String>,
    /// Skip the race intro's camera flights (see [`intro`]) and a
    /// challenge's intro page.
    no_intro: bool,
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
    /// A round of a career championship (see [`career`]).
    Career,
    /// One of the Freestyle Challenges (see [`challenge`]).
    Challenge,
}

impl Mode {
    /// Watermark shown on the select screens.
    fn label(self) -> &'static str {
        match self {
            Mode::SingleRace => "SINGLE RACE",
            Mode::TimeTrial => "TIME TRIAL",
            Mode::Arcade => "ARCADE",
            Mode::Freestyle => "FREESTYLE",
            Mode::Career => "CAREER",
            Mode::Challenge => "FREESTYLE",
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
    /// Arcade round being raced (index into the roster's rounds), and the
    /// tries at it that didn't win (three and Arcade is over).
    round: usize,
    failed: usize,
    /// Open Freestyle's time limit in minutes (None = unlimited).
    time_limit: Option<u32>,
    /// The computer riders' models, when the field is set (career and
    /// Arcade), front of the grid first.
    field: Vec<String>,
    /// Arcade: the player's slot on the grid (1 at the front).
    grid: usize,
    /// The Freestyle Challenge (1-based).
    challenge: usize,
    /// Humans in a split-screen race (1 alone), and the rider and quad of
    /// each after the first.
    players: usize,
    others: Vec<(String, String)>,
}

impl Selection {
    /// Player 1's slot on the grid: Arcade's from the last round, else
    /// the front.
    fn player_slot(&self) -> usize {
        if self.mode == Mode::Arcade { self.grid.max(1) } else { 1 }
    }
}

#[derive(Resource, Clone)]
struct Game(Arc<GameSource>);

#[derive(Resource)]
struct Race {
    collision: TrackCollision,
    checkpoints: Vec<Vec3>,
    /// Each checkpoint's gate (empty in a challenge, whose course has its
    /// own).
    gates: Vec<race::Gate>,
    next_checkpoint: usize,
    /// The checkpoints are a challenge course, kept by [`challenge`].
    course: bool,
    /// Humans racing, in split screen when more than one.
    players: usize,
    start: (Vec3, Quat),
    /// Last pose where the quad was upright on the ground (for respots).
    safe: (Vec3, Quat),
    safe_timer: f32,
    upside_down: f32,
    /// Times player 1 was put back on the track (a challenge fails on the
    /// first).
    respots: u32,
    /// The original's automatic respots and warnings (races only).
    marshal: Option<respot::Marshal>,
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
    /// Smoothed offset from the quad, before pulling in for terrain.
    offset: Vec3,
    /// Smoothed facing.
    facing: Quat,
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
        hd: None,
        use_hd: true,
        mode: Mode::SingleRace,
        minutes: Some(3),
        round: 0,
        // The original's default field (`mNumAIPlayers` in user.xml).
        opponents: 5,
        challenge: 1,
        players: 1,
        cheats: Vec::new(),
        no_intro: false,
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
                    "options" => Screen::Options,
                    "audio" => Screen::Audio,
                    "camera" => Screen::Camera,
                    "controller" => Screen::Controller,
                    "career" => Screen::CareerChamp,
                    "status" => Screen::CareerStatus,
                    "freestyle" => Screen::FreestyleType,
                    "challenges" => Screen::Challenges,
                    "multiplayer" => Screen::MpMode,
                    "players" => Screen::MpPlayers,
                    "tricks" | "trickbook" => Screen::TrickBook,
                    "profiles" => Screen::ProfileSelect,
                    "arcade" => Screen::ArcadeStatus,
                    other => anyhow::bail!("unknown screen {other} (title, main, rider, atv, track)"),
                });
            }
            "--mode" => {
                a.mode = match val().to_ascii_lowercase().as_str() {
                    "race" | "single" => Mode::SingleRace,
                    "timetrial" | "time" => Mode::TimeTrial,
                    "arcade" => Mode::Arcade,
                    "freestyle" => Mode::Freestyle,
                    "career" => Mode::Career,
                    "challenge" => Mode::Challenge,
                    other => anyhow::bail!("unknown mode {other} (race, timetrial, arcade, freestyle, career, challenge)"),
                }
            }
            "--minutes" => a.minutes = Some(val().parse::<u32>()?).filter(|&m| m > 0),
            "--round" => {
                a.round = val().parse()?;
                a.direct = true;
            }
            "--challenge" => {
                a.challenge = val().parse::<usize>()?.clamp(1, challenge::COUNT);
                a.mode = Mode::Challenge;
                a.direct = true;
            }
            "--players" => a.players = val().parse::<usize>()?.clamp(1, mp::MAX_PLAYERS),
            "--cheat" => {
                let name = val();
                anyhow::ensure!(unlocks::cheat_bits(&name).is_some(), "unknown cheat {name} (ALLOFEM, ALLRIDERS, ALLBIKES, ALLTRACKS, ALLTRICKS, ALLCHAMPS, ALLCHALS)");
                a.cheats.push(name);
            }
            "--opponents" => a.opponents = val().parse::<usize>()?.min(7),
            "--quad" => a.quad = val(),
            "--rider" => a.rider = val(),
            "--screenshot" => a.screenshot = Some(PathBuf::from(val())),
            "--after" => a.screenshot_after = val().parse()?,
            "--autodrive" => a.autodrive = true,
            "--hd" => a.hd = Some(PathBuf::from(val())),
            "--no-hd" => a.use_hd = false,
            "--no-intro" => a.no_intro = true,
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
    hd::init(args.hd.clone(), args.use_hd);
    let roster = Roster::load(&game.0);
    let career_rules = career::Rules::load(&game.0);
    let challenges = challenge::Defs::load(&game.0);
    let mut selection = Selection {
        track: args.track.clone(),
        quad: args.quad.clone(),
        rider: args.rider.clone(),
        laps: args.laps.clamp(1, 9),
        mode: args.mode,
        round: args.round,
        failed: 0,
        time_limit: args.minutes,
        field: Vec::new(),
        grid: 1,
        challenge: args.challenge,
        players: args.players,
        others: Vec::new(),
    };
    // Split screen is Single Race and Freestyle (Battle) only.
    if selection.players > 1 && !matches!(args.mode, Mode::SingleRace | Mode::Freestyle) {
        eprintln!("--players is for single races and freestyle; playing alone");
        selection.players = 1;
    }
    if selection.players > 1 {
        selection.others = mp::default_picks(&roster, &selection.rider, &selection.quad, selection.players);
    }
    if args.mode == Mode::Challenge {
        selection.track = challenge::TRACK.into();
        selection.laps = 1;
    }
    // Arcade races its rounds' tracks (`--round N` starts on one), a lap
    // each.
    if args.mode == Mode::Arcade
        && let Some(t) = roster.arcade.get(args.round)
    {
        selection.track = t.clone();
        selection.laps = 1;
    }
    // The profile last loaded, or the progress from before profiles.
    let settings = Settings::load();
    let mut career = settings.profile.and_then(career::Career::load_profile).unwrap_or_else(career::Career::load);
    career.upgrade(&career_rules);
    unlocks::upgrade(&mut career, &roster);
    if !args.cheats.is_empty() {
        for c in &args.cheats {
            unlocks::apply_cheat(&mut career, &career_rules, c);
        }
        career.save();
    }
    // Career races the saved championship's next round.
    if args.mode == Mode::Career
        && let Some(run) = career.run.as_ref().filter(|r| !r.over())
        && let Some((t, laps)) = run.next_round()
    {
        selection.track = t.to_string();
        selection.round = run.round;
        selection.laps = laps;
        selection.rider = run.rider.clone();
        selection.quad = run.quad.clone();
        selection.field = run.opponents();
    }
    // Arcade races the saved field (a new one at first).
    if args.mode == Mode::Arcade {
        let rider = match &career.profile {
            Some(p) if !args.direct => p.rider.clone(),
            _ => selection.rider.clone(),
        };
        let field = career.arcade_field.get_or_insert_with(|| arcade::ArcadeField::new(&roster, &rider, selection.round));
        if args.direct {
            field.without(&roster, &rider, selection.round);
            selection.field = field.grid_order();
            selection.grid = field.player_slot();
        }
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
        .insert_resource(settings)
        .insert_resource(career_rules)
        .insert_resource(career)
        .insert_resource(challenges)
        .add_plugins((ui::UiPlugin, hud::HudPlugin, frontend::FrontEndPlugin, freestyle::FreestylePlugin, opponents::OpponentsPlugin, audio::AudioPlugin, mud::MudPlugin, challenge::ChallengePlugin, mp::MpPlugin, knock::KnockPlugin, water::WaterPlugin, unlocks::UnlocksPlugin))
        .add_plugins(replay::ReplayPlugin)
        .add_plugins(demo::DemoPlugin)
        .add_plugins(intro::IntroPlugin)
        .add_plugins(effects::EffectsPlugin)
        .insert_state(first)
        .add_systems(Startup, (load_ui, ui_camera))
        .add_systems(OnEnter(AppState::Race), setup)
        .add_systems(OnExit(AppState::Race), teardown)
        .add_systems(FixedUpdate, (drive, career::dnf_cutoff, challenge::follow_course).chain().run_if(in_state(AppState::Race)))
        .add_systems(
            Update,
            (sync_quad, animate_rider, stunts, crash::rider_follows, rider::pose_riders, cycle_camera, chase_camera, fly_camera, follow_sky, rumble)
                .chain()
                .run_if(in_state(AppState::Race)),
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
    commands.remove_resource::<boost::Boost>();
    commands.remove_resource::<boost::BoostRules>();
    commands.remove_resource::<attack::Fighter>();
    commands.remove_resource::<attack::FightRules>();
    commands.remove_resource::<attack::FightClips>();
    commands.remove_resource::<StuntClips>();
    commands.remove_resource::<Wreck>();
    commands.remove_resource::<CrashClips>();
    commands.remove_resource::<challenge::Course>();
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
    (career, rules, demo): (Res<career::Career>, Res<career::Rules>, Option<Res<demo::Demo>>),
) {
    set_ui_camera(&mut commands, &mut ui_cams, false);
    let scoped = DespawnOnExit(AppState::Race);
    let mut loader = convert::Loader::new(&game.0, &mut meshes, &mut materials, &mut images);
    loader.dirt = Some(roster.info(&sel.track).ground);
    let in_challenge = sel.mode == Mode::Challenge;
    loader.challenge = in_challenge.then_some(sel.challenge);
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
    let collision = trk::collision(&scene.world, &game.0, &sel.track);
    // A challenge starts at `start1` too, wherever its course is (0x86e00).
    let start = trk::start_pose(&scene.world, &collision, sel.player_slot());
    let course = in_challenge.then(|| challenge::Course::new(&scene.world, sel.challenge));
    let checkpoints = match &course {
        Some(c) => c.gates.iter().map(|g| g.center()).collect(),
        None => trk::checkpoints(&scene.world),
    };

    let generic = game.0.read("/Cars/Generic/Generic.params").map(|b| String::from_utf8_lossy(&b).into_owned());
    let own = game.0.read(&format!("/Cars/{q}/{q}.params", q = sel.quad)).ok();
    let own = own.map(|b| String::from_utf8_lossy(&b).into_owned());
    let mut params = VehicleParams::from_files(&generic.unwrap_or_default(), own.as_deref());
    if demo.is_some() {
        // The demo's computer rider gets a computer rider's handling.
        let ability = roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&sel.rider)).map_or(0.6, |r| r.stats[0] / 5.0);
        atv_sim::ai::tune_for_ai(&mut params, ability.clamp(0.0, 1.0));
    }
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
                    commands.insert_resource(CrashClips::load(&game.0, &r.mirror_map));
                    if let Some(c) = attack::FightClips::load(&game.0, &r.mirror_map) {
                        commands.insert_resource(c);
                    }
                    commands.entity(seat).insert(r);
                }
                Err(e) => error!("failed to load rider {}: {e:#}", sel.rider),
            }
        }
        Err(e) => error!("failed to load quad {}: {e:#}", sel.quad),
    }
    let field = match sel.mode {
        // Split screen: the other humans, and computer riders to make four.
        Mode::SingleRace if sel.players > 1 => mp::FIELD - 1,
        // The demo fills the grid (the track's mMaxPlayers, at most 8).
        Mode::SingleRace if demo.is_some() => 7,
        Mode::SingleRace => args.opponents,
        Mode::Career => sel.field.len(),
        // The original's arcade state machine seats 1 player and 7 AI.
        Mode::Arcade => 7,
        // Freestyle Battle: the players only.
        Mode::Freestyle => sel.players - 1,
        _ => 0,
    };
    // Tricks and freestyle scoring; split-screen players get one each.
    let mut session = Session::new(&game.0, &sel, &scene.world);
    // Player 1's tricks are the profile's: its skill level from career and
    // challenge skill points.
    session.level = rules.skill_level(career.skill_points) as u8;
    opponents::spawn(&mut commands, &mut loader, &mut bindposes, &game.0, &roster, &sel, (&scene.world, &collision), &session, field);
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
            let e = commands.spawn((view.clone(), cam, ChaseCam { offset: eye - start.0, facing: cam.rotation }, IsDefaultUiCamera, scoped.clone())).id();
            if sel.players > 1 {
                commands.entity(e).insert(mp::Split(0));
                mp::spawn_views(&mut commands, view, sel.players, &scoped);
            }
        }
    }
    // Whether the grid is on the start/finish line or still has to cross it.
    let on_line = checkpoints.first().is_some_and(|&c| (c - start.0).with_y(0.0).length() < 25.0);
    let mut progress = RaceProgress::new(if in_challenge { 1 } else { sel.laps }, on_line);
    if sel.mode == Mode::Arcade {
        let info = roster.info(&sel.track);
        progress = progress.with_arcade(ArcadeClock::new(info.arcade_start, race::arcade_gates(&scene.world)));
    }
    commands.insert_resource(progress);
    commands.insert_resource(StuntClips::load(&game.0, &session.tricks));
    commands.insert_resource(Wreck::default());
    commands.insert_resource(session);
    commands.insert_resource(boost::BoostRules::load(&game.0));
    commands.insert_resource(boost::Boost::default());
    commands.insert_resource(attack::FightRules::load(&game.0));
    let stats = roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(&sel.rider)).map_or([3.0; 4], |r| r.stats);
    commands.insert_resource(attack::Fighter::new(stats[3], stats[2]));
    commands.insert_resource(water::Water(trk::water_surface(&scene.world), water::boxes(&scene.world)));
    let gates = if in_challenge { Vec::new() } else { race::lap_gates(&scene.world) };
    // Races keep watch for riders off the course, along the AI line; the
    // attract demo's computer rider has its own respots.
    let racing = matches!(sel.mode, Mode::SingleRace | Mode::TimeTrial | Mode::Arcade | Mode::Career) && demo.is_none();
    let marshal = racing
        .then(|| game.0.read(&format!("/Levels/{t}/World/{t}.aid", t = sel.track)).ok())
        .flatten()
        .and_then(|b| AiPath::parse(&b).ok())
        .and_then(|path| respot::Marshal::new(RacingLine::from_path(&path), &race::lap_gate_faces(&scene.world), respot::zones(&scene.world), roster.info(&sel.track).forced_respots, start.0));
    commands.insert_resource(Race {
        collision,
        next_checkpoint: if in_challenge { 0 } else { usize::from(on_line && checkpoints.len() > 1) },
        checkpoints,
        gates,
        course: in_challenge,
        players: sel.players,
        start,
        safe: start,
        safe_timer: 0.0,
        upside_down: 0.0,
        respots: 0,
        marshal,
    });
    commands.insert_resource(Player(vehicle));
    commands.init_resource::<Controls>();
    if let Some(c) = course {
        if let Some(arrow) = loader.spawn_loose(&mut commands, "arrow", "Generic") {
            commands.entity(arrow).insert((challenge::GuideArrow, scoped.clone()));
        }
        commands.insert_resource(c);
    }
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

/// Keys for each player in split screen: throttle, brake, left, right,
/// lean back, lean forward, hop.
const PLAYER_KEYS: [[KeyCode; 7]; 2] = [
    [KeyCode::KeyW, KeyCode::KeyS, KeyCode::KeyA, KeyCode::KeyD, KeyCode::KeyR, KeyCode::KeyF, KeyCode::Space],
    [KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::Numpad8, KeyCode::Numpad5, KeyCode::ShiftRight],
];

/// `airborne`: the pad's X and RB are trick buttons in the air, so they
/// don't brake or pre-load there. `slot`: in split screen, the player
/// (0-based), who has their own keys (players 1 and 2) and their own pad
/// (the slot-th one); alone, every key and pad drives.
fn read_input(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>, airborne: bool, slot: Option<usize>) -> Input {
    let key = |k: KeyCode| -> f32 { if keys.pressed(k) { 1.0 } else { 0.0 } };
    let mut i = match slot {
        None => Input {
            throttle: key(KeyCode::ArrowUp).max(key(KeyCode::KeyW)),
            brake: key(KeyCode::ArrowDown).max(key(KeyCode::KeyS)),
            steer: key(KeyCode::ArrowRight).max(key(KeyCode::KeyD)) - key(KeyCode::ArrowLeft).max(key(KeyCode::KeyA)),
            lean: key(KeyCode::KeyI) - key(KeyCode::KeyK),
            jump: keys.pressed(KeyCode::Space),
        },
        Some(n) => match PLAYER_KEYS.get(n) {
            Some(k) => Input {
                throttle: key(k[0]),
                brake: key(k[1]),
                steer: key(k[3]) - key(k[2]),
                lean: key(k[4]) - key(k[5]),
                jump: keys.pressed(k[6]),
            },
            None => Input::default(),
        },
    };
    for (_, pad) in pads.iter().enumerate().filter(|(n, _)| slot.is_none_or(|s| s == *n)) {
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

/// The speed boost button: Y on the pad (on the ground; it's a trick
/// button in the air), Shift on the keyboard, or for player 2 in split
/// screen the number pad's 0.
pub(crate) fn read_boost(keys: &ButtonInput<KeyCode>, pads: &Query<&Gamepad>, airborne: bool, slot: Option<usize>) -> bool {
    let key = match slot {
        None => keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]),
        Some(0) => keys.pressed(KeyCode::ShiftLeft),
        Some(1) => keys.pressed(KeyCode::Numpad0),
        Some(_) => false,
    };
    key || (!airborne && pads.iter().enumerate().any(|(n, pad)| slot.is_none_or(|s| s == n) && pad.pressed(GamepadButton::North)))
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
    mut wreck: Option<ResMut<Wreck>>,
    crash_clips: Option<Res<CrashClips>>,
    mut restart: ResMut<NextState<AppState>>,
    (mut boost, boost_rules): (Option<ResMut<boost::Boost>>, Option<Res<boost::BoostRules>>),
    (demo, mut opponents): (Option<Res<demo::Demo>>, Option<ResMut<opponents::Opponents>>),
    water: Option<Res<water::Water>>,
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
    let crashed = wreck.as_ref().is_some_and(|w| w.fall.is_some());
    let mut held = false;
    let mut input = if crashed {
        // Thrown off: the quad rolls on by itself.
        Input::default()
    } else if !progress.started() || (over && (freestyle || race.course || progress.out_of_time() || progress.dnf)) {
        // Brake to a stop (braking at a standstill would reverse).
        Input { brake: if over && v.speed_mph() > 2.0 { 0.3 } else { 0.0 }, ..default() }
    } else if let Some(opp) = opponents.as_mut().filter(|_| demo.is_some()) {
        // The attract demo: a computer rider has player 1's seat.
        held = progress.finished.is_none() && boost.as_ref().is_some_and(|b| b.on || b.meter >= 1.0);
        opp.drive_player(v, dt)
    } else if let Some(spot) = session.next_spot(v.position).filter(|_| args.autodrive && freestyle) {
        // Freestyle's autopilot goes exploring.
        trk::steer_towards(v, spot)
    } else if (args.autodrive || progress.finished.is_some()) && n > 0 {
        // The autopilot spends a full boost meter.
        held = progress.finished.is_none() && boost.as_ref().is_some_and(|b| b.on || b.meter >= 1.0);
        trk::steer_towards(v, race.checkpoints[next])
    } else {
        held = read_boost(&keys, &pads, !v.grounded(), (race.players > 1).then_some(0));
        read_input(&keys, &pads, !v.grounded(), (race.players > 1).then_some(0))
    };
    if held {
        input.throttle = 1.0;
    }
    if let (Some(b), Some(rules)) = (boost.as_mut(), boost_rules.as_deref()) {
        v.drive_scale = b.step(rules, held, session.take_banked(), dt);
    }
    if race.course {
        // The challenge's gates are kept by `challenge::follow_course`.
    } else if let Some(&cp) = race.checkpoints.get(next).filter(|_| freestyle) {
        // No laps in freestyle; the checkpoints only lead the autopilot.
        if (cp - v.position).with_y(0.0).length() < 25.0 {
            race.next_checkpoint = (next + 1) % n;
        }
    } else if let Some(&cp) = race.checkpoints.get(next) {
        let to_cp = (cp - v.position).with_y(0.0);
        // Touching the gate, as the original passes one, or near enough
        // its middle for a route that skirts it.
        if race.gates.get(next).is_some_and(|g| g.touched(&race::hull(v))) || to_cp.length() < 25.0 {
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

    let before = (v.velocity, v.angular_velocity);
    let air = v.air_time;
    v.step(&input, dt, &race.collision);
    let surface = water.as_deref().and_then(|w| w.surface(v.position));
    if let Some(w) = wreck.as_mut() {
        let wet = w.over_water(v, surface, dt);
        v.in_water = wet;
        if crashed {
            if w.advance_in(crash_clips.as_deref(), &race.collision, water.as_deref(), wet, dt) {
                w.fall = None;
                put_back(&mut race, v, opponents.as_deref());
            }
        } else if progress.started() {
            let crash = if wet { Some((Cause::Water, crash::way_over(v, None))) } else { w.check(v, before, &race.collision, dt) };
            if let Some((cause, way)) = crash {
                // Which getting-up clip: any number that varies crash to crash.
                let pick = ((v.position.x * 12.9898 + v.position.z * 78.233).sin() * 43758.547).fract().abs();
                w.start(v, cause, way, pick);
                if let (Some(b), Some(rules)) = (boost.as_mut(), boost_rules.as_deref()) {
                    b.crashed(rules);
                }
            } else if air > 0.5 && v.air_time == 0.0 {
                w.landed = true;
            }
        }
    }
    if let Some(c) = controls.as_mut() {
        c.0 = input;
    }
    let (mut buttons, mut dir) = freestyle::read_tricks(&keys, &pads, (race.players > 1).then_some(0));
    if args.autodrive && session.active.is_none() && !v.grounded() {
        // The autopilot shows off a different trick every so often.
        let k = (session.time * 0.7) as usize;
        buttons = 1 << (k % 4);
        dir = Some([freestyle::Dir::Up, freestyle::Dir::Right, freestyle::Dir::Down, freestyle::Dir::Left][k / 4 % 4]);
    }
    let live = progress.started() && !over;
    session.step(v, buttons, dir, dt, live);

    // The original's automatic respots, with the rider on the quad.
    let crashed = wreck.as_ref().is_some_and(|w| w.fall.is_some());
    let gates = race.gates.len();
    let next = race.next_checkpoint % gates.max(1);
    let racing = progress.racing();
    if race.marshal.as_mut().is_some_and(|m| racing && !crashed && m.watch(v.position, v.forward(), next, gates, dt)) {
        put_back(&mut race, v, opponents.as_deref());
    }
    if let Some(m) = race.marshal.as_ref() {
        if racing {
            progress.wrong_way = m.backwards;
        }
        progress.warning = m.warning().filter(|_| racing);
    }

    // Remember a safe spot; respot on R, when stuck upside down, or after
    // falling out of the world.
    race.safe_timer += dt;
    if v.grounded() && v.up().y > 0.9 && race.safe_timer > 1.0 && !v.in_water {
        race.safe = (v.position, v.rotation);
        race.safe_timer = 0.0;
    }
    race.upside_down = if v.up().y < 0.2 { race.upside_down + dt } else { 0.0 };
    let fell = v.position.y < race.safe.0.y - 40.0;
    let crashed = wreck.as_ref().is_some_and(|w| w.fall.is_some());
    if keys.just_pressed(KeyCode::KeyR) || (!crashed && race.upside_down > 2.5) || fell {
        if let Some(w) = wreck.as_mut() {
            w.fall = None;
        }
        put_back(&mut race, v, opponents.as_deref());
    } else if let Some((p, r)) = opponents.as_mut().filter(|_| demo.is_some() && !crashed).and_then(|o| o.respot_player()) {
        // The demo's computer rider gives up where the others would.
        let ground = race.collision.raycast(p + Vec3::Y * 3.0, Vec3::NEG_Y, 20.0).map_or(p.y, |h| h.point.y);
        v.reset(p.with_y(ground + 0.6), r);
        race.upside_down = 0.0;
    }
    if keys.just_pressed(KeyCode::Backspace) && demo.is_none() {
        // The quick restart: as Restart on the pause menu, so the computer
        // riders, the score and the clocks start over too.
        restart.set(AppState::Loading);
    }
}

/// Puts player 1 back: in a race on the racing line, as the original puts
/// back a racer whatever the reason (0x90ac0), else at the last spot where
/// the quad was upright on the ground.
fn put_back(race: &mut Race, v: &mut Vehicle, opponents: Option<&opponents::Opponents>) {
    if let Some(m) = race.marshal.as_mut() {
        let others: Vec<Vec3> = opponents.iter().flat_map(|o| &o.racers).map(|r| r.vehicle.position).collect();
        race.safe = m.respot(&race.collision, &others);
        race.safe_timer = 0.0;
        v.reset(race.safe.0, race.safe.1);
    } else {
        let (p, r) = race.safe;
        v.reset(p + Vec3::Y * 0.5, r);
    }
    race.upside_down = 0.0;
    race.respots += 1;
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

/// The rider kicks, flinches from a kick, or performs the trick in
/// progress.
#[allow(clippy::type_complexity)]
fn stunts(
    time: Res<Time>,
    session: Option<Res<Session>>,
    (clips, fighter, fight_clips): (Option<Res<StuntClips>>, Option<Res<attack::Fighter>>, Option<Res<attack::FightClips>>),
    (wreck, crash_clips): (Option<Res<Wreck>>, Option<Res<CrashClips>>),
    mut riders: Query<&mut Rider, Without<OpponentRider>>,
) {
    let (Some(s), Some(clips)) = (session, clips) else { return };
    let fight = fighter.zip(fight_clips).and_then(|(f, c)| f.pose(&c));
    // Bracing for the water.
    let plunge = wreck.zip(crash_clips).and_then(|(w, c)| w.plunge(&c));
    let stunt = fight.or(plunge).or_else(|| {
        s.active.as_ref().and_then(|a| {
            let clip = clips.0.get(a.def)?.clone()?;
            Some((clip, a.t / s.tricks[a.def].total))
        })
    });
    for mut r in &mut riders {
        r.set_stunt(stunt.clone(), time.delta_secs());
    }
}

/// The cameras look this far above the quad's centre of mass, about the
/// rider's chest, so the quad sits low in the picture.
const LOOK_RAISE: f32 = 1.0;

/// The player's follow camera, as picked in Options: it sits at the
/// camera's offset behind the quad's heading and looks ahead of it. The
/// offset swings round and the view turns at the camera's stiffnesses
/// (softer in the air), so the camera lags turns but not speed.
#[allow(clippy::too_many_arguments)]
fn chase_camera(
    time: Res<Time>,
    game: Res<Game>,
    settings: Res<Settings>,
    player: Option<Res<Player>>,
    race: Option<Res<Race>>,
    opponents: Option<Res<opponents::Opponents>>,
    (wreck, water): (Option<Res<Wreck>>, Option<Res<water::Water>>),
    mut cams: Query<(&mut Transform, &mut ChaseCam, Option<&mp::Split>)>,
    mut riders: Query<(&Rider, &mut Visibility), Without<OpponentRider>>,
    mut rig: Local<Option<(CameraView, FollowCam)>>,
) {
    let (Some(player), Some(race)) = (player, race) else { return };
    if rig.map(|r| r.0) != Some(settings.camera) {
        *rig = Some((settings.camera, FollowCam::load(&game.0, settings.camera)));
    }
    let Some((_, follow)) = *rig else { return };
    for (mut t, mut cam, split) in &mut cams {
        // Split screen: each view follows its own player's quad.
        let v = match split.map_or(0, |s| s.0) {
            0 if let Some(body) = wreck.as_deref().and_then(Wreck::floating) => {
                // The quad sinks; the camera stays over the water and
                // watches the rider tread water.
                let floor = water.as_deref().and_then(|w| w.level(t.translation)).map_or(f32::MIN, |s| s + 0.5);
                t.translation.y = t.translation.y.max(floor);
                let want = Transform::from_translation(t.translation).looking_at(body + Vec3::Y * 0.5, Vec3::Y).rotation;
                cam.facing = cam.facing.slerp(want, 1.0 - (-follow.rot_stiffness[1] * time.delta_secs()).exp()).normalize();
                t.rotation = cam.facing;
                continue;
            }
            0 => &player.0,
            slot => match opponents.as_deref().and_then(|o| o.human(slot + 1)) {
                Some(r) => &r.vehicle,
                None => continue,
            },
        };
        follow_quad(&time, &race, &follow, v, &mut t, &mut cam);
    }
    // Very close puts the camera at the rider's head: ride without them.
    for (r, mut vis) in &mut riders {
        let hide = settings.camera == CameraView::VeryClose && !r.crashed() && race.players == 1;
        vis.set_if_neq(if hide { Visibility::Hidden } else { Visibility::Inherited });
    }
}

fn follow_quad(time: &Time, race: &Race, follow: &FollowCam, v: &Vehicle, t: &mut Transform, cam: &mut ChaseCam) {
    let fwd = v.forward().with_y(0.0).normalize_or(Vec3::NEG_Z);
    let right = fwd.cross(Vec3::Y);
    let at = |o: Vec3| v.position + right * o.x + Vec3::Y * o.y + fwd * o.z;
    let air = usize::from(!v.grounded());
    let ease = |stiffness: f32| 1.0 - (-stiffness * time.delta_secs()).exp();
    cam.offset = cam.offset.lerp(at(follow.offset) - v.position, ease(follow.pos_stiffness[air]));
    // Never let the camera sink below the quad's level.
    cam.offset.y = cam.offset.y.max(0.5);
    let ideal = v.position + cam.offset;

    // Pull in when terrain or a wall is between the rider and the camera,
    // and keep clear of the ground under it.
    let pivot = v.position + Vec3::Y * 1.2;
    let to_cam = ideal - pivot;
    let dist = to_cam.length();
    let mut eye = ideal;
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
    let look = at(follow.look + Vec3::Y * LOOK_RAISE);
    if (look - eye).length_squared() > 1e-4 {
        let want = Transform::from_translation(eye).looking_at(look, Vec3::Y).rotation;
        cam.facing = cam.facing.slerp(want, ease(follow.rot_stiffness[air])).normalize();
    }
    t.rotation = cam.facing;
}

/// C or the pad's Back button steps through the follow cameras; the pick
/// is kept, as if made in Options.
fn cycle_camera(keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>, virt: Res<Time<Virtual>>, mut settings: ResMut<Settings>) {
    if virt.is_paused() {
        return;
    }
    if keys.just_pressed(KeyCode::KeyC) || pads.iter().any(|p| p.just_pressed(GamepadButton::Select)) {
        settings.camera = settings.camera.next();
        settings.save();
    }
}

/// Vibration (when it's on in Options): a hard knock when the rider is
/// thrown off and a thump on landing from a big jump. In split screen each
/// player's own pad shakes (the pad order `read_input` uses); otherwise
/// every pad does, since any of them drives.
#[allow(clippy::too_many_arguments)]
fn rumble(
    settings: Res<Settings>,
    player: Option<Res<Player>>,
    wreck: Option<Res<Wreck>>,
    sel: Res<Selection>,
    opponents: Option<Res<opponents::Opponents>>,
    pads: Query<Entity, With<Gamepad>>,
    mut requests: MessageWriter<GamepadRumbleRequest>,
    mut last: Local<[(bool, f32); mp::MAX_PLAYERS]>,
) {
    let (Some(player), Some(wreck)) = (player, wreck) else { return };
    // Crashed and air time for each player; only player 1 crashes.
    let mut now = [(false, 0.0); mp::MAX_PLAYERS];
    now[0] = (wreck.fall.is_some(), player.0.air_time);
    for r in opponents.iter().flat_map(|o| &o.racers) {
        // Player numbers count from 1.
        if let Some(slot) = r.human.map(|n| n - 1).filter(|s| *s < mp::MAX_PLAYERS) {
            now[slot] = (false, r.vehicle.air_time);
        }
    }
    let before = std::mem::replace(&mut *last, now);
    if !settings.vibration {
        return;
    }
    for (slot, (&(crashed, air), &(was_crashed, was_air))) in now.iter().zip(&before).enumerate() {
        let (strength, secs) = if crashed && !was_crashed {
            (1.0, 0.45)
        } else if !crashed && was_air > 0.5 && air == 0.0 {
            ((was_air / 2.0).clamp(0.25, 0.8), 0.2)
        } else {
            continue;
        };
        for (_, gamepad) in pads.iter().enumerate().filter(|(n, _)| sel.players <= 1 || *n == slot) {
            requests.write(GamepadRumbleRequest::Add {
                gamepad,
                duration: std::time::Duration::from_secs_f32(secs),
                intensity: GamepadRumbleIntensity { strong_motor: strength, weak_motor: strength * 0.6 },
            });
        }
    }
}

fn follow_sky(cams: Query<(&Transform, Option<&mp::Split>), With<Camera3d>>, mut sky: Query<&mut Transform, (With<SkyDome>, Without<Camera3d>)>) {
    // Player 1's view in split screen.
    let cam = cams.iter().find(|(_, s)| s.is_none_or(|s| s.0 == 0)).map(|c| c.0);
    let (Some(cam), Ok(mut sky)) = (cam, sky.single_mut()) else { return };
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
