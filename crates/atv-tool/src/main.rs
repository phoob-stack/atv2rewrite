//! Command-line tool for inspecting the player's own copy of the game.

use std::path::PathBuf;

use anyhow::{Context, Result};
use atv_formats::anim::{Animation, Skeleton};
use atv_formats::{bed::SpriteSheet, bog::Texture, rom::Model, row::World, GameSource};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Inspect ATV Quad Power Racing 2 game data from your own disc")]
struct Cli {
    /// Disc image (.iso/.bin) or the folder you extracted the disc to.
    #[arg(long, env = "ATV2_GAME")]
    game: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List every file in the game archive.
    List,
    /// Unpack the archive into a folder (for your own use only).
    Extract { out: PathBuf },
    /// Parse every texture, model and track and report failures.
    Check,
    /// Drive a quad around a track without graphics and print telemetry.
    Sim {
        #[arg(long, default_value = "Dunes1")]
        track: String,
        #[arg(long, default_value = "Bandit")]
        quad: String,
        #[arg(long, default_value_t = 12.0)]
        seconds: f32,
    },
    /// Build an HD texture pack: upscale the game's textures with an AI
    /// upscaler (Real-ESRGAN ncnn-vulkan) into a local folder the game
    /// prefers when present. Nothing leaves your machine.
    Hd {
        /// Pack folder (default: your local data folder, where the game looks).
        #[arg(long)]
        out: Option<PathBuf>,
        /// realesrgan-ncnn-vulkan executable (default: next to this tool, or on the PATH).
        #[arg(long, env = "ATV2_UPSCALER")]
        upscaler: Option<PathBuf>,
        /// Upscale menus, HUD and fonts.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        ui: bool,
        /// Also upscale track, quad and rider textures.
        #[arg(long)]
        world: bool,
        #[arg(long, default_value_t = 2)]
        ui_scale: u32,
        #[arg(long, default_value_t = 2)]
        world_scale: u32,
        /// Upscaler model name (from its models folder).
        #[arg(long, default_value = "realesrgan-x4plus")]
        model: String,
        /// Only textures whose path contains one of these words.
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
    },
    /// Race computer riders round a track without graphics and print their
    /// lap times.
    Race {
        #[arg(long, default_value = "Dunes1")]
        track: String,
        /// Number of quads on the grid (1-8).
        #[arg(long, default_value_t = 4)]
        riders: usize,
        #[arg(long, default_value_t = 2)]
        laps: u32,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let game = GameSource::open(&cli.game)
        .with_context(|| format!("opening game data at {}", cli.game.display()))?;
    match cli.cmd {
        Cmd::List => {
            for e in game.archive.entries.values() {
                println!("{:>10} {}", e.raw_size, e.path);
            }
        }
        Cmd::Extract { out } => {
            let paths: Vec<String> = game.archive_paths().map(str::to_owned).collect();
            for p in &paths {
                let data = game.read(p)?;
                let dst = out.join(p.trim_start_matches('/'));
                std::fs::create_dir_all(dst.parent().unwrap())?;
                std::fs::write(dst, data)?;
            }
            println!("extracted {} files to {}", paths.len(), out.display());
        }
        Cmd::Check => check(&game)?,
        Cmd::Sim { track, quad, seconds } => sim(&game, &track, &quad, seconds)?,
        Cmd::Hd { out, upscaler, ui, world, ui_scale, world_scale, model, only } => {
            let out = out.or_else(atv_hd::default_dir).context("no local data folder; pass --out")?;
            let upscaler = match upscaler {
                Some(exe) => atv_hd::Upscaler { exe },
                None => atv_hd::Upscaler::find().with_context(|| {
                    format!("{} not found; download it from {} and pass --upscaler", atv_hd::UPSCALER_NAME, atv_hd::UPSCALER_URL)
                })?,
            };
            let opts = atv_hd::Options { ui, world, ui_scale, world_scale, ui_model: model.clone(), world_model: model, only, ..Default::default() };
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let s = atv_hd::build(&game, &out, &upscaler, &opts, &cancel, |p| {
                if !p.current.is_empty() {
                    eprintln!("[{}/{}] {}", p.done, p.total, p.current);
                }
            })?;
            for (path, err) in &s.failed {
                eprintln!("failed: {path}: {err}");
            }
            println!("{} textures upscaled, {} already there, {} failed: {}", s.written, s.kept, s.failed.len(), out.display());
        }
        Cmd::Race { track, riders, laps } => race(&game, &track, riders, laps)?,
    }
    Ok(())
}

fn check(game: &GameSource) -> Result<()> {
    let (mut ok, mut failed) = (0, 0);
    let mut tris = 0usize;
    let mut patches = 0usize;
    let paths: Vec<String> = game.archive_paths().map(str::to_owned).collect();
    for p in &paths {
        let lower = p.to_ascii_lowercase();
        let data = game.read(p)?;
        let res: Result<String> = if lower.ends_with(".rom") || lower.ends_with(".shm") {
            Model::parse(&data).map(|m| format!("{} nodes", m.nodes.len())).map_err(Into::into)
        } else if lower.ends_with(".row") {
            World::parse(&data)
                .map(|w| {
                    let t: usize = w.meshes.iter().map(|m| m.indices.len() / 3).sum();
                    let pc: usize = w.meshes.iter().map(|m| m.patches.len()).sum();
                    tris += t;
                    patches += pc;
                    format!("{} meshes, {t} tris, {pc} patches, {} props", w.meshes.len(), w.instances.len())
                })
                .map_err(Into::into)
        } else if lower.ends_with(".bas") {
            Skeleton::parse(&data).map(|s| format!("{} bones", s.bones.len())).map_err(Into::into)
        } else if lower.ends_with(".bac") {
            match Animation::parse(&data) {
                Ok(a) => Ok(format!("{} bones x {} frames", a.bone_count, a.frame_count)),
                // The bike stunt clips and punch_floor use another layout.
                Err(_) if lower.contains("_bike.") || lower.contains("punch_floor") => continue,
                Err(e) => Err(e.into()),
            }
        } else if lower.ends_with(".aid") {
            atv_formats::aid::AiPath::parse(&data)
                .map(|a| format!("{} racing line nodes", a.nodes.len()))
                .map_err(Into::into)
        } else if lower.ends_with(".ink") {
            atv_formats::ink::CollisionMesh::parse(&data)
                .map(|m| format!("{} collision triangles", m.triangles.len()))
                .map_err(Into::into)
        } else if lower.ends_with(".xwb") {
            atv_formats::xwb::WaveBank::parse(&data).map(|b| format!("{} sounds", b.waves.len())).map_err(Into::into)
        } else if lower.ends_with(".bed") {
            SpriteSheet::parse(&data).map(|s| format!("{} sprites", s.entries.len())).map_err(Into::into)
        } else if lower.ends_with(".bog") {
            match Texture::parse(&data) {
                Ok(t) => Ok(format!("{}x{} {:?}", t.width, t.height, t.format)),
                Err(e) => Err(e.into()),
            }
        } else {
            continue;
        };
        match res {
            Ok(info) => {
                ok += 1;
                if lower.ends_with(".row") {
                    println!("{p}: {info}");
                }
            }
            Err(e) => {
                failed += 1;
                println!("FAIL {p}: {e}");
            }
        }
    }
    // The music lives on the disc beside the archive; Sound.xml names it.
    let sound = atv_formats::sound::SoundSetup::parse(&String::from_utf8_lossy(&game.read("/Setup/Sound.xml")?));
    let mut music: Vec<String> = Vec::new();
    for env in ["Dunes", "Forest", "Glacier", "Industrial", "Swamp", "Stadium"] {
        if let Some(m) = sound.music(env) {
            music.extend(m.intro.iter().chain(&m.sections).filter(|n| !music.contains(n)).cloned().collect::<Vec<_>>());
        }
    }
    for name in music {
        let path = format!("/{}.wav", name.replace('\\', "/"));
        match game.read(&path).map_err(anyhow::Error::from).and_then(|b| Ok(atv_formats::wav::decode(&b)?)) {
            Ok(_) => ok += 1,
            Err(e) => {
                failed += 1;
                println!("FAIL {path}: {e}");
            }
        }
    }
    println!("{ok} parsed, {failed} failed; tracks total {tris} tris, {patches} patches");
    Ok(())
}

fn sim(game: &GameSource, track: &str, quad: &str, seconds: f32) -> Result<()> {
    use atv_sim::{track as trk, Input, Vehicle, VehicleParams};
    let world = World::parse(&game.read(&format!("/Levels/{track}/World/{track}.row"))?)?;
    let col = trk::collision(&world, game, track);
    let generic = String::from_utf8_lossy(&game.read("/Cars/Generic/Generic.params")?).into_owned();
    let own = game.read(&format!("/Cars/{quad}/{quad}.params")).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
    let params = VehicleParams::from_files(&generic, own.as_deref());
    let (pos, rot) = trk::start_pose(&world, &col, 1);
    println!("{} collision triangles; start {pos:?}", col.triangle_count());
    let cps = trk::checkpoints(&world);
    let mut next = 0usize;
    let mut v = Vehicle::new(params, pos, rot);
    let dt = 1.0 / 60.0;
    let steps = (seconds / dt) as usize;
    for i in 0..steps {
        let t = i as f32 * dt;
        // Settle for a second, then chase the checkpoints in order.
        let mut input = Input::default();
        if t > 1.0 && !cps.is_empty() {
            let target = cps[next % cps.len()];
            if (target - v.position).with_y(0.0).length() < 25.0 {
                println!("  reached checkpoint {} at t={t:.1}", next % cps.len() + 2);
                next += 1;
            }
            input = trk::steer_towards(&v, cps[next % cps.len()]);
        }
        v.step(&input, dt, &col);
        if i % 30 == 0 {
            let wheels: String = v.wheels.iter().map(|w| if w.contact { 'o' } else { '.' }).collect();
            println!(
                "t={t:5.2} pos=({:7.1},{:6.1},{:7.1}) mph={:5.1} gear={} revs={:5.0} up.y={:.2} wheels={wheels}",
                v.position.x, v.position.y, v.position.z, v.speed_mph(), v.gear, v.revs, v.up().y
            );
        }
    }
    Ok(())
}

/// Headless AI race: every quad is computer-driven; reports lap times,
/// respots and the slowest lap against the track's own AI lap-time window.
fn race(game: &GameSource, track: &str, riders: usize, laps: u32) -> Result<()> {
    use atv_formats::aid::AiPath;
    use atv_formats::setup;
    use atv_sim::ai::{AiDriver, RacingLine, Traffic, bump, tune_for_ai};
    use atv_sim::{Vehicle, VehicleParams, track as trk};
    let quads: Vec<String> = std::env::var("ATV2_QUADS")
        .map(|q| q.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    const QUADS: [&str; 8] = [
        "Bandit",
        "Blaze",
        "Dimension",
        "Nitro",
        "Skyline",
        "Charger",
        "Rocket",
        "Storm",
    ];
    let world = World::parse(&game.read(&format!("/Levels/{track}/World/{track}.row"))?)?;
    let col = trk::collision(&world, game, track);
    let line = RacingLine::from_path(&AiPath::parse(
        &game.read(&format!("/Levels/{track}/World/{track}.aid"))?,
    )?);
    let cps = trk::checkpoints(&world);
    let info = setup::entries(&String::from_utf8_lossy(&game.read("/Setup/Tracks.xml")?))
        .into_iter()
        .find(|e| e.name.eq_ignore_ascii_case(track));
    println!(
        "{track}: {} line nodes, {} edges, main loop {:.0} m, {} checkpoints; original AI laps {:?}-{:?} s",
        line.nodes.len(),
        line.edges.len(),
        line.lap_length(),
        cps.len(),
        info.as_ref().and_then(|e| e.num("mAIBestLapTime")),
        info.as_ref().and_then(|e| e.num("mAIWorstLapTime")),
    );
    if std::env::var_os("ATV2_TRACE").is_some() {
        for (i, &c) in cps.iter().enumerate() {
            let (e, t) = line.locate(c);
            println!(
                "  checkpoint {i} at {c:.0?}: line edge {e} t={t:.2}, {:.1} m off it",
                (line.point(e, t) - c).with_y(0.0).length()
            );
        }
    }
    let generic = String::from_utf8_lossy(&game.read("/Cars/Generic/Generic.params")?).into_owned();
    let mut cars = Vec::new();
    for (i, default) in QUADS.iter().enumerate().take(riders.clamp(1, 8)) {
        let q = quads.get(i).map_or(*default, String::as_str);
        let own = game
            .read(&format!("/Cars/{q}/{q}.params"))
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        let (pos, rot) = trk::start_pose(&world, &col, i + 1);
        let mut params = VehicleParams::from_files(&generic, own.as_deref());
        if std::env::var_os("ATV2_STOCK").is_none() {
            tune_for_ai(&mut params, 0.6);
        }
        let v = Vehicle::new(params, pos, rot);
        let d = AiDriver::new(&line, pos, i as u32 + 1);
        let on_line = cps
            .first()
            .is_some_and(|&c| (c - pos).with_y(0.0).length() < 25.0);
        // (next checkpoint, lap start time, laps done, armed, respots)
        cars.push((
            q,
            v,
            d,
            usize::from(on_line && cps.len() > 1),
            0.0f32,
            Vec::<f32>::new(),
            on_line,
            0u32,
        ));
    }
    let dt = 1.0 / 60.0;
    let mut t = 0.0f32;
    let limit = laps as f32
        * info
            .as_ref()
            .and_then(|e| e.num("mAIWorstLapTime"))
            .filter(|&w| w > 0.0)
            .unwrap_or(150.0)
        * 3.0;
    while t < limit && cars.iter().any(|c| (c.5.len() as u32) < laps) {
        let traffic: Vec<Traffic> = cars
            .iter()
            .map(|c| Traffic {
                position: c.1.position,
                velocity: c.1.velocity,
            })
            .collect();
        for (i, (_, v, d, next, lap_start, times, armed, respots)) in cars.iter_mut().enumerate() {
            let others: Vec<Traffic> = traffic
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, t)| *t)
                .collect();
            let input = if t < 1.0 {
                Default::default()
            } else {
                d.drive(&line, v, &others, dt)
            };
            v.step(&input, dt, &col);
            if std::env::var_os("ATV2_TRACE").is_some() && i == 0 && (t * 60.0) as u32 % 120 == 0 {
                println!(
                    "  t={t:6.1} edge={} t={:.2} pos={:.0?} mph={:.0} target_mph={:.0} up={:.2} gear={} revs={:.0} thr={:.2} brk={:.2} wheels={}",
                    d.edge,
                    d.t,
                    v.position,
                    v.speed_mph(),
                    d.target_speed * 2.237,
                    v.up().y,
                    v.gear,
                    v.revs,
                    v.throttle,
                    v.brake,
                    v.wheels
                        .iter()
                        .map(|w| if w.contact { 'o' } else { '.' })
                        .collect::<String>()
                );
            }
            if d.wants_respot() || v.up().y < 0.2 && v.speed_mph().abs() < 2.0 {
                if std::env::var_os("ATV2_TRACE").is_some() {
                    println!(
                        "  respot #{i} t={t:.1} edge={} pos={:.0?} up={:.2} driver={}",
                        d.edge,
                        v.position,
                        v.up().y,
                        d.wants_respot()
                    );
                }
                let (p, r) = d.respot(&line);
                let ground = col
                    .raycast(p + glam::Vec3::Y * 3.0, glam::Vec3::NEG_Y, 20.0)
                    .map_or(p.y, |h| h.point.y);
                v.reset(p.with_y(ground + 0.6), r);
                *respots += 1;
            }
            if !cps.is_empty() && (cps[*next] - v.position).with_y(0.0).length() < 25.0 {
                if *next == 0 {
                    if *armed {
                        times.push(t - *lap_start);
                    }
                    *armed = true;
                    *lap_start = t;
                }
                *next = (*next + 1) % cps.len();
            }
        }
        for i in 0..cars.len() {
            for j in i + 1..cars.len() {
                let (a, b) = cars.split_at_mut(j);
                bump(&mut a[i].1, &mut b[0].1);
            }
        }
        t += dt;
    }
    for (q, _, _, _, _, times, _, respots) in &cars {
        let s: Vec<String> = times.iter().map(|x| format!("{x:.1}")).collect();
        let best = times
            .iter()
            .copied()
            .reduce(f32::min)
            .map_or("-".into(), |b| format!("{b:.1}"));
        println!(
            "{q:>10}: laps [{}] s, best {best}, {respots} respots",
            s.join(", ")
        );
    }
    Ok(())
}
