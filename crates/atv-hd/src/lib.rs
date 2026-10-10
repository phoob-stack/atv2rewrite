//! HD texture packs: the game's own textures, upscaled with an AI model on
//! the player's machine and kept in a local cache the game prefers when it
//! is there.
//!
//! Nothing upscaled is shipped. [`build`] reads every `.bog` texture from
//! the player's disc, runs an external upscaler on it (Real-ESRGAN's
//! ncnn-vulkan build by default, which runs on any Vulkan GPU without
//! Python), and writes the result as a `.dds` under the pack folder at the
//! texture's archive path. [`load`] reads one back.
//!
//! Menus, HUD and fonts are stored uncompressed so text and edges stay
//! crisp. Track, quad and rider textures are re-compressed to DXT1/DXT5
//! with a full mip chain so video memory stays close to the original.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use atv_formats::GameSource;
use atv_formats::bog::{TexFormat, Texture};

/// Tag in the first reserved DDS header word marking our files; the next
/// two words hold the original texture size.
const TAG: u32 = u32::from_le_bytes(*b"ATV2");
/// The models upscale 4x; smaller targets are box-filtered down from that.
const MODEL_SCALE: u32 = 4;
/// Source texels wrapped around tiling textures before upscaling, so the
/// model sees the neighbouring tile and edges match when they repeat.
const WRAP: u32 = 8;
/// Textures handed to the upscaler per run.
const CHUNK: usize = 8;
/// Back-projection passes pulling the upscaled image back towards the
/// original (see [`back_project`]).
const BACK_PROJECT: usize = 4;

/// Which part of the game a texture belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Front end, HUD, fonts and other 2D sprites.
    Ui,
    /// Tracks, sky domes, quads and riders.
    World,
}

/// Classifies a `.bog` archive path.
pub fn kind(path: &str) -> Kind {
    let top = path.trim_start_matches('/').split('/').next().unwrap_or("").to_ascii_lowercase();
    match top.as_str() {
        "frontend" | "sprites" | "fonts" | "init" => Kind::Ui,
        _ => Kind::World,
    }
}

/// Textures that hold data rather than pictures, which an upscaler trained
/// on photos would only damage.
fn skip(path: &str) -> bool {
    path.to_ascii_lowercase().contains("bump")
}

/// Where packs go unless told otherwise: the user's local data folder.
pub fn default_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "atv2").map(|d| d.data_local_dir().join("hd"))
}

/// The pack file for an archive path like `/FrontEnd/ATVLogoBIG.bog`.
pub fn file_for(dir: &Path, archive_path: &str) -> PathBuf {
    dir.join(archive_path.trim_start_matches('/').to_ascii_lowercase()).with_extension("dds")
}

/// An upscaled texture and how many of its pixels make one original texel.
pub struct HdTexture {
    pub texture: Texture,
    pub scale: f32,
}

/// The pack's version of a texture, if it has one.
pub fn load(dir: &Path, archive_path: &str) -> Option<HdTexture> {
    let bytes = std::fs::read(file_for(dir, archive_path)).ok()?;
    let word = |i: usize| bytes.get(32 + i * 4..36 + i * 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if word(0)? != TAG {
        return None;
    }
    let orig_w = word(1)?.max(1);
    let texture = Texture::parse_dds(&bytes).ok()?;
    let scale = texture.width as f32 / orig_w as f32;
    Some(HdTexture { texture, scale })
}

/// Number of textures in a pack folder.
pub fn count(dir: &Path) -> usize {
    fn walk(d: &Path) -> usize {
        let Ok(rd) = std::fs::read_dir(d) else { return 0 };
        rd.flatten()
            .map(|e| {
                let p = e.path();
                if p.is_dir() {
                    walk(&p)
                } else {
                    usize::from(p.extension().is_some_and(|x| x == "dds"))
                }
            })
            .sum()
    }
    walk(dir)
}

/// An external upscaler with the Real-ESRGAN ncnn-vulkan command line:
/// `-i <in dir> -o <out dir> -n <model> -s 4 -f png [-m <models dir>]`.
#[derive(Clone, Debug)]
pub struct Upscaler {
    pub exe: PathBuf,
}

pub const UPSCALER_NAME: &str = "realesrgan-ncnn-vulkan";
pub const UPSCALER_URL: &str = "https://github.com/xinntao/Real-ESRGAN/releases";

impl Upscaler {
    /// Looks for the upscaler in `ATV2_UPSCALER`, next to the running
    /// program (or one folder down, as its zip unpacks), then on the PATH.
    pub fn find() -> Option<Self> {
        if let Some(p) = std::env::var_os("ATV2_UPSCALER").map(PathBuf::from).filter(|p| p.is_file()) {
            return Some(Self { exe: p });
        }
        let name = format!("{UPSCALER_NAME}{}", std::env::consts::EXE_SUFFIX);
        let mut dirs = Vec::new();
        if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            if let Ok(rd) = std::fs::read_dir(&d) {
                dirs.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
            }
            dirs.insert(0, d);
        }
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        dirs.into_iter().map(|d| d.join(&name)).find(|p| p.is_file()).map(|exe| Self { exe })
    }

    fn run(&self, model: &str, input: &Path, output: &Path) -> Result<()> {
        let mut cmd = Command::new(&self.exe);
        cmd.arg("-i").arg(input).arg("-o").arg(output).args(["-n", model, "-s", "4", "-f", "png"]);
        if let Some(models) = self.exe.parent().map(|d| d.join("models")).filter(|m| m.is_dir()) {
            cmd.arg("-m").arg(models);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output().with_context(|| format!("could not run {}", self.exe.display()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = err.lines().rev().take(6).collect();
            bail!("upscaler failed ({}):\n{}", out.status, tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
        }
        Ok(())
    }
}

/// What to build.
#[derive(Clone, Debug)]
pub struct Options {
    pub ui: bool,
    pub world: bool,
    /// Final size relative to the original: 1, 2 or 4.
    pub ui_scale: u32,
    pub world_scale: u32,
    /// Longest side allowed after upscaling.
    pub max_size: u32,
    /// Upscaler model for each kind.
    pub ui_model: String,
    pub world_model: String,
    /// Only textures whose path contains one of these (case-insensitive).
    pub only: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            ui: true,
            world: false,
            ui_scale: 2,
            world_scale: 2,
            max_size: 2048,
            ui_model: "realesrgan-x4plus".into(),
            world_model: "realesrgan-x4plus".into(),
            only: Vec::new(),
        }
    }
}

/// Progress through a build.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: String,
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub written: usize,
    /// Already in the pack from an earlier run.
    pub kept: usize,
    pub failed: Vec<(String, String)>,
}

struct Job {
    path: String,
    kind: Kind,
    tex: Texture,
    rgba: Vec<u8>,
    scale: u32,
}

/// Upscales the game's textures into `out`. Textures already in the pack
/// are kept, so a cancelled build carries on where it stopped.
pub fn build(
    game: &GameSource,
    out: &Path,
    upscaler: &Upscaler,
    opts: &Options,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Progress),
) -> Result<Summary> {
    let mut summary = Summary::default();
    let mut todo = Vec::new();
    for p in game.archive_paths() {
        if !p.to_ascii_lowercase().ends_with(".bog") || skip(p) {
            continue;
        }
        let k = kind(p);
        if !(k == Kind::Ui && opts.ui || k == Kind::World && opts.world) {
            continue;
        }
        let lower = p.to_ascii_lowercase();
        if !opts.only.is_empty() && !opts.only.iter().any(|o| lower.contains(&o.to_ascii_lowercase())) {
            continue;
        }
        if file_for(out, p).is_file() {
            summary.kept += 1;
        } else {
            todo.push((p.to_string(), k));
        }
    }
    todo.sort();
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let work = out.join(".work");
    let mut prog = Progress { done: 0, total: todo.len(), current: String::new() };
    progress(&prog);

    for chunk in todo.chunks(CHUNK) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut jobs = Vec::new();
        for (path, k) in chunk {
            let res = game.read(path).map_err(anyhow::Error::from).and_then(|b| Ok(Texture::parse(&b)?));
            match res {
                Ok(tex) => {
                    let wanted = if *k == Kind::Ui { opts.ui_scale } else { opts.world_scale };
                    let scale = fit_scale(tex.width, tex.height, wanted, opts.max_size);
                    if scale < 2 {
                        prog.done += 1;
                        continue;
                    }
                    let rgba = tex.to_rgba8();
                    jobs.push(Job { path: path.clone(), kind: *k, tex, rgba, scale });
                }
                Err(e) => {
                    summary.failed.push((path.clone(), format!("{e:#}")));
                    prog.done += 1;
                }
            }
        }
        for kind in [Kind::Ui, Kind::World] {
            let group: Vec<&Job> = jobs.iter().filter(|j| j.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            prog.current = group[0].path.clone();
            progress(&prog);
            let model = if kind == Kind::Ui { &opts.ui_model } else { &opts.world_model };
            let results = upscale_group(upscaler, model, &group, &work);
            match results {
                Ok(images) => {
                    let encoded: Vec<Result<Vec<u8>>> = std::thread::scope(|s| {
                        let handles: Vec<_> =
                            group.iter().zip(images).map(|(j, img)| s.spawn(move || finish(j, img?))).collect();
                        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("encoder panicked")))).collect()
                    });
                    for (j, res) in group.iter().zip(encoded) {
                        match res.and_then(|bytes| {
                            let dst = file_for(out, &j.path);
                            std::fs::create_dir_all(dst.parent().unwrap())?;
                            std::fs::write(&dst, bytes)?;
                            Ok(())
                        }) {
                            Ok(()) => summary.written += 1,
                            Err(e) => summary.failed.push((j.path.clone(), format!("{e:#}"))),
                        }
                    }
                }
                Err(e) => {
                    _ = std::fs::remove_dir_all(&work);
                    return Err(e);
                }
            }
            prog.done += group.len();
            progress(&prog);
        }
    }
    _ = std::fs::remove_dir_all(&work);
    prog.current.clear();
    progress(&prog);
    Ok(summary)
}

/// Largest of 1, 2, 4 up to `wanted` that keeps both sides within `max`.
fn fit_scale(w: u32, h: u32, wanted: u32, max: u32) -> u32 {
    let mut s = wanted.clamp(1, MODEL_SCALE).next_power_of_two().min(MODEL_SCALE);
    while s > 1 && (w * s > max || h * s > max) {
        s /= 2;
    }
    s
}

/// One upscaled image: width, height, RGBA.
type Rgba = (u32, u32, Vec<u8>);

/// Writes the group's textures as PNGs, runs the upscaler over them once and
/// reads back the 4x results (unwrapped, for tiling textures).
fn upscale_group(up: &Upscaler, model: &str, group: &[&Job], work: &Path) -> Result<Vec<Result<Rgba>>> {
    _ = std::fs::remove_dir_all(work);
    let (din, dout) = (work.join("in"), work.join("out"));
    std::fs::create_dir_all(&din)?;
    std::fs::create_dir_all(&dout)?;
    let sources: Vec<Rgba> = group
        .iter()
        .map(|j| {
            if j.kind == Kind::World {
                wrap_pad(&j.rgba, j.tex.width, j.tex.height, WRAP)
            } else {
                (j.tex.width, j.tex.height, j.rgba.clone())
            }
        })
        .collect();
    for (i, (w, h, px)) in sources.iter().enumerate() {
        write_png(&din.join(format!("{i:03}.png")), *w, *h, px)?;
    }
    up.run(model, &din, &dout)?;
    let images = group
        .iter()
        .enumerate()
        .map(|(i, j)| {
            let (w, h, px) = read_png(&dout.join(format!("{i:03}.png")))
                .with_context(|| format!("the upscaler wrote no image for {}", j.path))?;
            let (sw, sh, src) = &sources[i];
            if (w, h) != (sw * MODEL_SCALE, sh * MODEL_SCALE) {
                bail!("upscaler returned {w}x{h} for a {sw}x{sh} image");
            }
            let px = back_project(src, *sw, *sh, &px, BACK_PROJECT);
            let margin = if j.kind == Kind::World { WRAP * MODEL_SCALE } else { 0 };
            let (tw, th) = (j.tex.width * MODEL_SCALE, j.tex.height * MODEL_SCALE);
            Ok((tw, th, crop(&px, w, margin, margin, tw, th)))
        })
        .collect();
    Ok(images)
}

/// Brings an upscaled image to its final size, restores the original's
/// alpha behaviour and encodes the pack file.
fn finish(j: &Job, (w, h, mut px): Rgba) -> Result<Vec<u8>> {
    let mut f = MODEL_SCALE / j.scale;
    let (mut w, mut h) = (w, h);
    while f > 1 {
        (w, h, px) = half(&px, w, h);
        f /= 2;
    }
    let opaque = j.rgba.chunks_exact(4).all(|p| p[3] == 255);
    let punch = j.tex.format == TexFormat::Dxt1;
    for p in px.chunks_exact_mut(4) {
        if opaque {
            p[3] = 255;
        } else if punch {
            p[3] = if p[3] >= 128 { 255 } else { 0 };
        }
    }
    let tex = match j.kind {
        // Opaque pages (backgrounds, track pictures) compress well.
        Kind::Ui if opaque => compress(w, h, px, TexFormat::Dxt1),
        Kind::Ui => {
            let data = px.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
            Texture { width: w, height: h, mip_count: 1, format: TexFormat::Bgra8, data }
        }
        Kind::World => compress(w, h, px, if punch { TexFormat::Dxt1 } else { TexFormat::Dxt5 }),
    };
    let mut reserved = [0; 11];
    reserved[..3].copy_from_slice(&[TAG, j.tex.width, j.tex.height]);
    Ok(tex.to_dds(reserved))
}

/// DXT-compresses an image with a full mip chain.
fn compress(w: u32, h: u32, rgba: Vec<u8>, format: TexFormat) -> Texture {
    let fmt = if format == TexFormat::Dxt1 { texpresso::Format::Bc1 } else { texpresso::Format::Bc3 };
    let params = texpresso::Params { algorithm: texpresso::Algorithm::ClusterFit, ..Default::default() };
    let mut data = Vec::new();
    let (mut lw, mut lh, mut level) = (w, h, rgba);
    let mut mips = 0;
    loop {
        let start = data.len();
        data.resize(start + fmt.compressed_size(lw as usize, lh as usize), 0);
        fmt.compress(&level, lw as usize, lh as usize, params, &mut data[start..]);
        mips += 1;
        if lw == 1 && lh == 1 {
            break;
        }
        (lw, lh, level) = half(&level, lw, lh);
    }
    Texture { width: w, height: h, mip_count: mips, format, data }
}

/// Iterative back-projection: nudges the 4x image until shrinking it again
/// gives back the original. GAN upscalers invent detail, and on tiny text
/// that means wrong glyphs (an 8 on the rev gauge came out as a 0); this
/// keeps their sharp edges but restores what the original actually shows.
fn back_project(src: &[u8], w: u32, h: u32, hr: &[u8], passes: usize) -> Vec<u8> {
    let s = MODEL_SCALE as usize;
    let (w, h) = (w as usize, h as usize);
    let hw = w * s;
    let mut out: Vec<f32> = hr.iter().map(|&v| v as f32).collect();
    let mut diff = vec![0f32; w * h * 4];
    for _ in 0..passes {
        // Original minus the box-shrunk estimate.
        for y in 0..h {
            for x in 0..w {
                for c in 0..4 {
                    let mut sum = 0.0;
                    for dy in 0..s {
                        let row = (y * s + dy) * hw;
                        for dx in 0..s {
                            sum += out[(row + x * s + dx) * 4 + c];
                        }
                    }
                    let i = (y * w + x) * 4 + c;
                    diff[i] = src[i] as f32 - sum / (s * s) as f32;
                }
            }
        }
        // Added back, bilinearly enlarged.
        for y in 0..h * s {
            let fy = ((y as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, (h - 1) as f32);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(h - 1);
            for x in 0..hw {
                let fx = ((x as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, (w - 1) as f32);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(w - 1);
                for c in 0..4 {
                    let d = |xx: usize, yy: usize| diff[(yy * w + xx) * 4 + c];
                    let top = d(x0, y0) + (d(x1, y0) - d(x0, y0)) * tx;
                    let bot = d(x0, y1) + (d(x1, y1) - d(x0, y1)) * tx;
                    let o = &mut out[(y * hw + x) * 4 + c];
                    *o = (*o + top + (bot - top) * ty).clamp(0.0, 255.0);
                }
            }
        }
    }
    out.iter().map(|&v| v.round() as u8).collect()
}

/// Box-filters an image to half size (a side of 1 stays 1).
fn half(px: &[u8], w: u32, h: u32) -> Rgba {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let mut sum = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(w - 1);
                    let sy = (y * 2 + dy).min(h - 1);
                    sum += px[((sy * w + sx) * 4 + c) as usize] as u32;
                }
                out[((y * nw + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
            }
        }
    }
    (nw, nh, out)
}

/// Surrounds a tiling image with `m` texels wrapped from its opposite edges.
fn wrap_pad(px: &[u8], w: u32, h: u32, m: u32) -> Rgba {
    let (pw, ph) = (w + 2 * m, h + 2 * m);
    let mut out = Vec::with_capacity((pw * ph * 4) as usize);
    for y in 0..ph {
        let sy = (y + h * m - m) % h;
        for x in 0..pw {
            let sx = (x + w * m - m) % w;
            let i = ((sy * w + sx) * 4) as usize;
            out.extend_from_slice(&px[i..i + 4]);
        }
    }
    (pw, ph, out)
}

fn crop(px: &[u8], w: u32, x0: u32, y0: u32, cw: u32, ch: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((cw * ch * 4) as usize);
    for y in y0..y0 + ch {
        let i = ((y * w + x0) * 4) as usize;
        out.extend_from_slice(&px[i..i + (cw * 4) as usize]);
    }
    out
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn read_png(path: &Path) -> Result<Rgba> {
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut dec = png::Decoder::new(file);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("png too large")?];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let px = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => bail!("unexpanded palette png"),
    };
    Ok((info.width, info.height, px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_kinds() {
        let d = Path::new("pack");
        assert_eq!(file_for(d, "/FrontEnd/BG/Main.bog"), Path::new("pack/frontend/bg/main.dds"));
        assert_eq!(kind("/Sprites/HUDTPAGE.bog"), Kind::Ui);
        assert_eq!(kind("/Fonts/kim36.bog"), Kind::Ui);
        assert_eq!(kind("/Levels/Dunes1/Textures/sand.bog"), Kind::World);
        assert_eq!(kind("/Cars/Bandit/TpageBandit.bog"), Kind::World);
        assert!(skip("/Sprites/DropletBumpMap.bog"));
    }

    #[test]
    fn scales_fit_the_limit() {
        assert_eq!(fit_scale(512, 256, 4, 2048), 4);
        assert_eq!(fit_scale(1024, 512, 4, 2048), 2);
        assert_eq!(fit_scale(2048, 64, 4, 2048), 1);
        assert_eq!(fit_scale(64, 64, 2, 2048), 2);
        assert_eq!(fit_scale(64, 64, 3, 2048), 4);
    }

    #[test]
    fn wrap_then_crop_is_identity() {
        let (w, h) = (4, 2);
        let px: Vec<u8> = (0..w * h * 4).map(|i| i as u8).collect();
        let (pw, ph, padded) = wrap_pad(&px, w, h, 3);
        assert_eq!((pw, ph), (10, 8));
        assert_eq!(crop(&padded, pw, 3, 3, w, h), px);
        // The texel left of the image is the right edge of the same row.
        let at = |x: u32, y: u32| &padded[((y * pw + x) * 4) as usize..][..4];
        assert_eq!(at(2, 3), &px[((w - 1) * 4) as usize..][..4]);
    }

    #[test]
    fn back_projection_restores_the_original() {
        // A 2x1 source and a 4x upscale that got the right texel wrong.
        let src = [10, 20, 30, 255, 200, 100, 50, 255];
        let mut hr = Vec::new();
        for _ in 0..4 {
            for x in 0..8 {
                hr.extend_from_slice(if x < 4 { &src[..4] } else { &[0, 0, 0, 255] });
            }
        }
        let fixed = back_project(&src, 2, 1, &hr, 8);
        let right: u32 = (0..4).flat_map(|y| (4..8).map(move |x| (y * 8 + x) * 4)).map(|i| fixed[i] as u32).sum();
        assert!((right as f32 / 16.0 - 200.0).abs() < 8.0, "{}", right / 16);
    }

    #[test]
    fn half_averages() {
        let px = [0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0];
        let (w, h, out) = half(&px, 2, 2);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![128, 128, 128, 128]);
    }

    #[test]
    fn png_round_trip_and_pack_file() {
        let dir = std::env::temp_dir().join(format!("atv-hd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let px: Vec<u8> = (0..8 * 8 * 4).map(|i| (i * 7) as u8).collect();
        write_png(&dir.join("a.png"), 8, 8, &px).unwrap();
        assert_eq!(read_png(&dir.join("a.png")).unwrap(), (8, 8, px.clone()));

        let tex = Texture { width: 4, height: 4, mip_count: 1, format: TexFormat::Bgra8, data: vec![0; 64] };
        let job = Job { path: "/Fonts/x.bog".into(), kind: Kind::Ui, rgba: tex.to_rgba8(), tex, scale: 2 };
        let bytes = finish(&job, (16, 16, vec![9; 16 * 16 * 4])).unwrap();
        let dst = file_for(&dir, &job.path);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(&dst, bytes).unwrap();
        let hd = load(&dir, "/Fonts/X.bog").unwrap();
        assert_eq!((hd.texture.width, hd.scale), (8, 2.0));
        assert_eq!(count(&dir), 1);

        let world = Texture { width: 4, height: 4, mip_count: 1, format: TexFormat::Dxt1, data: vec![0; 8] };
        let job = Job { path: "/Levels/a/b.bog".into(), kind: Kind::World, rgba: world.to_rgba8(), tex: world, scale: 4 };
        let bytes = finish(&job, (16, 16, vec![200; 16 * 16 * 4])).unwrap();
        let t = Texture::parse_dds(&bytes).unwrap();
        assert_eq!((t.width, t.format, t.mip_count), (16, TexFormat::Dxt1, 5));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
