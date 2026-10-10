//! `.bed` sprite sheets: named regions of a `.bog` texture page, used for
//! the HUD, the front end, particles and the bitmap fonts.
//!
//! Layout (little-endian, no magic):
//!
//! ```text
//! u8 1, u8 1, source TGA name (NUL-terminated)
//! u16 width, u16 height     size of the source TGA in pixels
//! u16 count
//! count x { u16 n, n x (i32 x, i32 y), name (NUL-terminated), i16 link }
//! ```
//!
//! Coordinates are normalised to the texture: 0..100000 across its width
//! and height, so they hold for the `.bog` whatever size it was saved at.
//! Most entries are 4-point rectangles (clockwise from the top left). An
//! entry's `link` is the index of a 1-point, unnamed entry holding its
//! pivot (a needle's hub, a glyph's baseline origin), or -1. In fonts every
//! glyph is named by its character code in decimal ("65" is `A`), using
//! Latin-1 for accented letters.

use crate::bin::Reader;
use crate::{Error, Result};

const SCALE: f32 = 100_000.0;

#[derive(Debug, Clone)]
pub struct SpriteEntry {
    pub name: String,
    /// Points in texture UV space (0..1, y down).
    pub points: Vec<[f32; 2]>,
    /// Index of the entry holding this one's pivot point.
    pub link: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Sprite {
    /// Bounding rectangle in UV space: min, max.
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// Pivot in UV space, when the sheet gives one.
    pub pivot: Option<[f32; 2]>,
}

#[derive(Debug, Clone)]
pub struct SpriteSheet {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub entries: Vec<SpriteEntry>,
}

impl SpriteSheet {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.get(..2) != Some(&[1, 1]) {
            return Err(Error::Magic { what: "BED sprite sheet", expected: "\\x01\\x01" });
        }
        let mut r = Reader::at(bytes, 2, "BED sprite sheet");
        let source = r.cstr()?;
        let width = r.u16()? as u32;
        let height = r.u16()? as u32;
        let count = r.u16()? as usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let n = r.u16()? as usize;
            if n * 8 > r.remaining() {
                return Err(Error::Format(format!("BED: point count {n} exceeds data")));
            }
            let mut points = Vec::with_capacity(n);
            for _ in 0..n {
                let x = r.i32()? as f32 / SCALE;
                let y = r.i32()? as f32 / SCALE;
                points.push([x, y]);
            }
            let name = r.cstr()?;
            let link = r.u16()? as i16;
            entries.push(SpriteEntry { name, points, link: (link >= 0).then_some(link as usize) });
        }
        Ok(Self { source, width, height, entries })
    }

    fn sprite_of(&self, e: &SpriteEntry) -> Sprite {
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for p in &e.points {
            for k in 0..2 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        let pivot = e.link.and_then(|i| self.entries.get(i)).and_then(|p| p.points.first().copied());
        Sprite { min, max, pivot }
    }

    /// A named region (case-insensitive).
    pub fn sprite(&self, name: &str) -> Option<Sprite> {
        self.entries
            .iter()
            .find(|e| e.points.len() >= 2 && e.name.eq_ignore_ascii_case(name))
            .map(|e| self.sprite_of(e))
    }

    /// Every named region, in file order.
    pub fn sprites(&self) -> impl Iterator<Item = (&str, Sprite)> {
        self.entries.iter().filter(|e| e.points.len() >= 2).map(|e| (e.name.as_str(), self.sprite_of(e)))
    }

    /// The glyphs of a font sheet: character, region and baseline origin.
    pub fn glyphs(&self) -> impl Iterator<Item = (char, Sprite)> {
        self.sprites().filter_map(|(name, s)| {
            let code: u32 = name.parse().ok()?;
            Some((char::from_u32(code)?, s))
        })
    }
}
