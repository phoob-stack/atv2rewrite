//! Reads textures from the player's HD texture pack (built locally by the
//! launcher or `atv-tool hd`, see the `atv-hd` crate) when it has them, and
//! from the disc otherwise.

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Result;
use atv_formats::GameSource;
use atv_formats::bog::Texture;

static PACK: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Picks the pack folder for this run: `dir` if given, else the default
/// one if it holds anything.
pub fn init(dir: Option<PathBuf>, enabled: bool) {
    let dir = if enabled { dir.or_else(atv_hd::default_dir) } else { None };
    let dir = dir.filter(|d| d.is_dir()).and_then(|d| {
        let n = atv_hd::count(&d);
        (n > 0).then(|| {
            eprintln!("HD texture pack: {} textures in {}", n, d.display());
            d
        })
    });
    _ = PACK.set(dir);
}

/// A texture by archive path, and how many of its pixels make one texel of
/// the disc's version (1 unless it came from the pack).
pub fn texture(game: &GameSource, path: &str) -> Result<(Texture, f32)> {
    if let Some(hd) = PACK.get().and_then(Option::as_ref).and_then(|d| atv_hd::load(d, path)) {
        return Ok((hd.texture, hd.scale));
    }
    let bytes = game.read(path).map_err(|e| anyhow::anyhow!("reading {path}: {e}"))?;
    Ok((Texture::parse(&bytes)?, 1.0))
}
