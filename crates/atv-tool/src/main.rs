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
    println!("{ok} parsed, {failed} failed; tracks total {tris} tris, {patches} patches");
    Ok(())
}

fn sim(game: &GameSource, track: &str, quad: &str, seconds: f32) -> Result<()> {
    use atv_sim::{track as trk, Input, Vehicle, VehicleParams};
    let world = World::parse(&game.read(&format!("/Levels/{track}/World/{track}.row"))?)?;
    let col = trk::collision_from_world(&world);
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
