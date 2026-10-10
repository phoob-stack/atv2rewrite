//! Command-line tool for inspecting the player's own copy of the game.

use std::path::PathBuf;

use anyhow::{Context, Result};
use atv_formats::{bog::Texture, rom::Model, row::World, GameSource};
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
