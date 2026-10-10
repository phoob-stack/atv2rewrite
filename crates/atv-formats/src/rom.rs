//! "ROM 1.26" models: vehicles, riders, props and sky domes (`.rom`, and the
//! `.shm` shadow-mesh twins, which use the same layout).
//!
//! Layout (little-endian):
//! ```text
//! 0x00 "ROM 1.26\0" + 3 pad bytes
//! 0x0C u32 material_count, u32 locator_count, u32 point_count,
//!      u32 sprite_count, u32 unknown (always 0), u32 node_count
//! 0x24 i32 -1, i32 -1, f32 -1.0, f32 bbox_min[3], f32 bbox_max[3]
//! 0x48 materials[material_count]  0x8C bytes, name in the first 32
//!      locators[locator_count]    0x7C bytes: name[64], pos, scale, 3x3 matrix
//!      points[point_count]        0x2C bytes: name[32], pos (wheel centres...)
//!      sprites[sprite_count]      0x68 bytes: name[32], material[32], 10 floats
//!      nodes[node_count]
//! node: i32 x5 header, u32 tri_count, u32 patch_count, u32 0, u32 vertex_count,
//!       i32 -1 x3, u32 indices[tri_count*3], Vertex[vertex_count],
//!       Patch[patch_count]
//! vertex (56 B): pos, normal, uv, 4 floats, u32 ARGB, u32 bone
//! patch (672 B): u32 mode, 0, 0; X[16], Y[16], Z[16]; UVs; u32 ARGB[4]
//!       at word 148; i32 bone[16] at word 152 (-1 when unskinned)
//! ```

use crate::bin::Reader;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, Default)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Four floats: a second (overlay) UV pair, then two unused. Zero in
    /// most models; riders use the pair for their mud layer.
    pub extra: [f32; 4],
    /// Vertex colour as stored (0xAARRGGBB).
    pub color: u32,
    /// Skeleton bone the vertex follows (riders; 0 in unskinned models).
    pub bone: u32,
}

/// Bicubic Bezier patch (curved bodywork, and most of each track's road).
#[derive(Debug, Clone)]
pub struct Patch {
    pub header: [u32; 3],
    /// 4x4 control points, row-major.
    pub control: [[f32; 3]; 16],
    /// Texture coordinate per control point (row-major). Patches with
    /// header[0] == 1 store all 16; others store only the four corners,
    /// which are spread bilinearly here.
    pub uv: [[f32; 2]; 16],
    /// Second (overlay) layer texture coordinates, laid out like `uv`.
    /// Only track patches have one; models leave it zero.
    pub uv2: [[f32; 2]; 16],
    /// Corner colours (0xAARRGGBB) at control points 0, 3, 15 and 12, i.e.
    /// (u,v) = (0,0), (1,0), (1,1), (0,1) on the tessellated grid.
    pub corner_color: [u32; 4],
    /// Skeleton bone per control point (row-major) for riders; -1 in
    /// unskinned models.
    pub bones: [i32; 16],
}

/// A tessellated patch: an (n+1) x (n+1) vertex grid, row-major.
pub struct Tessellation {
    pub pos: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub uv2: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Node {
    /// Five header ints. Field 1 looks like the main material index (-1 for
    /// none); fields 0 and 2..4 are not yet understood.
    pub header: [i32; 5],
    pub indices: Vec<u32>,
    pub vertices: Vec<Vertex>,
    pub patches: Vec<Patch>,
}

impl Node {
    pub fn material(&self) -> Option<usize> {
        usize::try_from(self.header[1]).ok()
    }
}

#[derive(Debug, Clone)]
pub struct Material {
    pub name: String,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Locator {
    pub name: String,
    pub pos: [f32; 3],
    pub scale: [f32; 3],
    pub rotation: [f32; 9],
}

#[derive(Debug, Clone)]
pub struct Sprite {
    pub kind: String,
    pub material: String,
    pub params: [f32; 10],
}

#[derive(Debug, Clone)]
pub struct Model {
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    pub materials: Vec<Material>,
    pub locators: Vec<Locator>,
    pub points: Vec<(String, [f32; 3])>,
    pub sprites: Vec<Sprite>,
    pub nodes: Vec<Node>,
}

const PATCH_SIZE: usize = 672;

impl Model {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if !bytes.starts_with(b"ROM 1.26") {
            return Err(Error::Magic { what: "ROM model", expected: "ROM 1.26" });
        }
        let mut r = Reader::at(bytes, 0x0C, "ROM model");
        let nmat = r.u32()?;
        let nloc = r.u32()?;
        let npts = r.u32()?;
        let nspr = r.u32()?;
        let _unk = r.u32()?;
        let nnode = r.u32()?;
        r.seek(0x30);
        let bbox_min = r.vec3()?;
        let bbox_max = r.vec3()?;

        let mut materials = Vec::new();
        for _ in 0..nmat {
            let raw = r.bytes(0x8C)?;
            materials.push(Material { name: crate::bin::cstr(&raw[..32]), raw: raw.to_vec() });
        }
        let mut locators = Vec::new();
        for _ in 0..nloc {
            let name = r.name(64)?;
            let pos = r.vec3()?;
            let scale = r.vec3()?;
            let rotation = r.f32s::<9>()?;
            locators.push(Locator { name, pos, scale, rotation });
        }
        let mut points = Vec::new();
        for _ in 0..npts {
            points.push((r.name(32)?, r.vec3()?));
        }
        let mut sprites = Vec::new();
        for _ in 0..nspr {
            let kind = r.name(32)?;
            let material = r.name(32)?;
            sprites.push(Sprite { kind, material, params: r.f32s::<10>()? });
        }
        let mut nodes = Vec::with_capacity(nnode as usize);
        for _ in 0..nnode {
            let mut header = [0i32; 5];
            for h in &mut header {
                *h = r.i32()?;
            }
            let ntri = r.u32()? as usize;
            let npatch = r.u32()? as usize;
            let _zero = r.u32()?;
            let nv = r.u32()? as usize;
            r.skip(12)?;
            if ntri * 12 + nv * 56 + npatch * PATCH_SIZE > r.remaining() {
                return Err(Error::Format("ROM node sizes exceed file".into()));
            }
            let mut indices = Vec::with_capacity(ntri * 3);
            for _ in 0..ntri * 3 {
                let i = r.u32()?;
                if i as usize >= nv {
                    return Err(Error::Format(format!("ROM index {i} >= {nv} vertices")));
                }
                indices.push(i);
            }
            let mut vertices = Vec::with_capacity(nv);
            for _ in 0..nv {
                let v = Vertex {
                    pos: r.vec3()?,
                    normal: r.vec3()?,
                    uv: r.vec2()?,
                    extra: r.f32s::<4>()?,
                    color: r.u32()?,
                    bone: r.u32()?,
                };
                vertices.push(v);
            }
            let mut patches = Vec::with_capacity(npatch);
            for _ in 0..npatch {
                let start = r.pos();
                let header = [r.u32()?, r.u32()?, r.u32()?];
                let xs = r.f32s::<16>()?;
                let ys = r.f32s::<16>()?;
                let zs = r.f32s::<16>()?;
                let uv = r.f32s::<32>()?;
                r.seek(start + 148 * 4);
                let corner_color = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
                let mut bones = [-1i32; 16];
                for b in &mut bones {
                    *b = r.i32()?;
                }
                r.seek(start + PATCH_SIZE);
                let control = std::array::from_fn(|i| [xs[i], ys[i], zs[i]]);
                patches.push(Patch { header, control, uv: Patch::uvs(header[0], &uv), uv2: [[0.0; 2]; 16], corner_color, bones });
            }
            nodes.push(Node { header, indices, vertices, patches });
        }
        if r.remaining() != 0 {
            return Err(Error::Format(format!("ROM has {} trailing bytes", r.remaining())));
        }
        Ok(Model { bbox_min, bbox_max, materials, locators, points, sprites, nodes })
    }
}

impl Patch {
    pub(crate) fn uvs(mode: u32, raw: &[f32; 32]) -> [[f32; 2]; 16] {
        if mode == 1 {
            return std::array::from_fn(|i| [raw[i * 2], raw[i * 2 + 1]]);
        }
        let c = |i: usize| [raw[i * 2], raw[i * 2 + 1]];
        let (a, b, cc, d) = (c(0), c(1), c(2), c(3));
        std::array::from_fn(|i| {
            let (u, v) = ((i % 4) as f32 / 3.0, (i / 4) as f32 / 3.0);
            let lerp = |k: usize| (a[k] * (1.0 - u) + b[k] * u) * (1.0 - v) + (d[k] * (1.0 - u) + cc[k] * u) * v;
            [lerp(0), lerp(1)]
        })
    }

    /// Evaluates the patch on an (n+1) x (n+1) grid (u along control-point
    /// columns, v along rows).
    pub fn tessellate(&self, n: usize) -> Tessellation {
        fn bern(t: f32) -> [f32; 4] {
            let s = 1.0 - t;
            [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
        }
        fn eval<const D: usize>(net: &[[f32; D]; 16], bu: &[f32; 4], bv: &[f32; 4]) -> [f32; D] {
            let mut p = [0.0f32; D];
            for (row, wv) in bv.iter().enumerate() {
                for (col, wu) in bu.iter().enumerate() {
                    let c = net[row * 4 + col];
                    for k in 0..D {
                        p[k] += c[k] * wu * wv;
                    }
                }
            }
            p
        }
        let mut t = Tessellation { pos: Vec::new(), uv: Vec::new(), uv2: Vec::new(), indices: Vec::new() };
        for j in 0..=n {
            let bv = bern(j as f32 / n as f32);
            for i in 0..=n {
                let bu = bern(i as f32 / n as f32);
                t.pos.push(eval(&self.control, &bu, &bv));
                t.uv.push(eval(&self.uv, &bu, &bv));
                t.uv2.push(eval(&self.uv2, &bu, &bv));
            }
        }
        let w = (n + 1) as u32;
        for j in 0..n as u32 {
            for i in 0..n as u32 {
                let a = j * w + i;
                t.indices.extend_from_slice(&[a, a + 1, a + w, a + 1, a + w + 1, a + w]);
            }
        }
        t
    }
}
