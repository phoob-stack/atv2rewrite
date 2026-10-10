//! "BOG 1.01" textures: a small header wrapping a DDS image.

use crate::bin::Reader;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    Dxt1,
    Dxt5,
    /// Uncompressed 32-bit BGRA (DDS RGB with alpha masks).
    Bgra8,
}

#[derive(Debug, Clone)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub format: TexFormat,
    /// All mip levels, largest first, tightly packed as in DDS.
    pub data: Vec<u8>,
}

impl Texture {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if !bytes.starts_with(b"BOG ") {
            return Err(Error::Magic { what: "BOG texture", expected: "BOG 1.01" });
        }
        let Some(dds) = find(bytes, b"DDS ") else {
            return Self::parse_raw(bytes);
        };
        let mut r = Reader::at(bytes, dds + 4, "DDS header");
        let _size = r.u32()?;
        let _flags = r.u32()?;
        let height = r.u32()?;
        let width = r.u32()?;
        let _pitch = r.u32()?;
        let _depth = r.u32()?;
        let mip_count = r.u32()?.max(1);
        r.seek(dds + 76);
        let _pf_size = r.u32()?;
        let pf_flags = r.u32()?;
        let fourcc = r.bytes(4)?;
        let bits = r.u32()?;
        let format = if pf_flags & 4 != 0 {
            match fourcc {
                b"DXT1" => TexFormat::Dxt1,
                b"DXT5" | b"DXT4" => TexFormat::Dxt5,
                other => return Err(Error::Format(format!("unsupported DDS fourcc {other:?}"))),
            }
        } else if bits == 32 {
            TexFormat::Bgra8
        } else {
            return Err(Error::Format(format!("unsupported DDS pixel format ({bits} bpp)")));
        };
        let data = bytes[dds + 128..].to_vec();
        let mut tex = Texture { width, height, mip_count, format, data };
        // Trust only the mips that are actually present.
        let mut total = 0;
        let mut n = 0;
        while n < tex.mip_count {
            let s = tex.level_size(n);
            if total + s > tex.data.len() {
                break;
            }
            total += s;
            n += 1;
        }
        tex.mip_count = n.max(1);
        tex.data.truncate(total.max(tex.level_size(0).min(tex.data.len())));
        Ok(tex)
    }

    /// Uncompressed textures: a 40-byte header (u32 format, width, height,
    /// mips, 1, pixel data size, 0, 0), then Xbox-swizzled mips. Format 32 is
    /// ARGB8 (shadow and bump maps); format 64 is 8-bit palettized with a
    /// 256-entry ARGB palette before the pixels (sky domes, tyre overlays).
    /// Both are expanded to BGRA8.
    fn parse_raw(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::at(bytes, 0x0C, "raw BOG");
        let bpp = r.u32()?;
        let width = r.u32()?;
        let height = r.u32()?;
        let mips = r.u32()?.max(1);
        if !matches!(bpp, 32 | 64) || !width.is_power_of_two() || !height.is_power_of_two() {
            return Err(Error::Format(format!("unsupported raw BOG (format {bpp}, {width}x{height})")));
        }
        let mut off = 0x28;
        let palette = if bpp == 64 {
            off += 1024;
            Some(bytes.get(0x28..0x28 + 1024).ok_or(Error::Eof("BOG palette"))?)
        } else {
            None
        };
        let texel = if palette.is_some() { 1 } else { 4 };
        let mut data = Vec::new();
        let mut count = 0;
        for level in 0..mips {
            let (w, h) = ((width >> level).max(1), (height >> level).max(1));
            let size = (w * h) as usize * texel;
            let Some(src) = bytes.get(off..off + size) else { break };
            let expanded;
            let src = match palette {
                Some(pal) => {
                    expanded = src.iter().flat_map(|&i| pal[i as usize * 4..i as usize * 4 + 4].to_vec()).collect::<Vec<u8>>();
                    &expanded[..]
                }
                None => src,
            };
            data.extend_from_slice(&unswizzle(src, w, h));
            off += size;
            count += 1;
        }
        if count == 0 {
            return Err(Error::Eof("raw BOG pixels"));
        }
        Ok(Texture { width, height, mip_count: count, format: TexFormat::Bgra8, data })
    }

    /// The top mip level decoded to straight RGBA8.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; w * h * 4];
        let top = &self.data[..self.level_size(0).min(self.data.len())];
        match self.format {
            TexFormat::Dxt1 => texpresso::Format::Bc1.decompress(top, w, h, &mut out),
            TexFormat::Dxt5 => texpresso::Format::Bc3.decompress(top, w, h, &mut out),
            TexFormat::Bgra8 => {
                for (o, i) in out.chunks_exact_mut(4).zip(top.chunks_exact(4)) {
                    o.copy_from_slice(&[i[2], i[1], i[0], i[3]]);
                }
            }
        }
        out
    }

    pub fn level_size(&self, level: u32) -> usize {
        let w = (self.width >> level).max(1) as usize;
        let h = (self.height >> level).max(1) as usize;
        match self.format {
            TexFormat::Dxt1 => w.div_ceil(4) * h.div_ceil(4) * 8,
            TexFormat::Dxt5 => w.div_ceil(4) * h.div_ceil(4) * 16,
            TexFormat::Bgra8 => w * h * 4,
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Undoes Xbox texture swizzling (Morton order over the shared power-of-two
/// square, extra length laid out linearly) for 4-byte texels.
fn unswizzle(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; src.len()];
    let min = w.min(h);
    let bits = min.trailing_zeros();
    for y in 0..h {
        for x in 0..w {
            let mut s = 0u32;
            for b in 0..bits {
                s |= ((x >> b) & 1) << (2 * b) | ((y >> b) & 1) << (2 * b + 1);
            }
            // Rectangles: the larger axis' high bits select whole squares.
            let block = if w > h { x >> bits } else { y >> bits };
            let si = (block * min * min + s) as usize * 4;
            let di = (y * w + x) as usize * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}
