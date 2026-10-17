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
//! vertex (56 B): pos, normal, uv, overlay uv, 2 floats, u32 ARGB, u32 bone
//! patch (672 B): u32 uv_full, uv2_full, uv3_full; X[16], Y[16], Z[16];
//!       u32 0; uv, uv2, uv3 nets (16 pairs each, see `NET_ORDER`; only the
//!       4 corners are valid when the flag is 0); u32 ARGB[4] at word 148;
//!       i32 bone[16] at word 152, column-major (-1 when unskinned)
//! ```
//! Node header: [kind, material, overlay material, -1, ?]. The overlay
//! (quad mud, tyre dust) uses the second UV set: vertex `extra[0..2]` and
//! the patch uv2 net.

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
    /// which are spread bilinearly here. See `NET_ORDER` for the stored
    /// order.
    pub uv: [[f32; 2]; 16],
    /// Second (overlay) layer texture coordinates, laid out like `uv`
    /// (full when header[1] == 1).
    pub uv2: [[f32; 2]; 16],
    /// Corner colours (0xAARRGGBB) at control points 0, 3, 15 and 12, i.e.
    /// (u,v) = (0,0), (1,0), (1,1), (0,1) on the tessellated grid.
    pub corner_color: [u32; 4],
    /// Skeleton bone per control point (row-major) for riders; -1 in
    /// unskinned models.
    pub bones: [i32; 16],
}

/// A tessellated patch: a `cols` x rows vertex grid, row-major.
pub struct Tessellation {
    pub cols: usize,
    pub pos: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub uv2: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Node {
    /// Five header ints. Field 1 is the material index (-1 for none) and
    /// field 2 the overlay material drawn with the second UV set; field 0
    /// is 0/2 for triangles and 1/3 for patches; field 4 is not understood.
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

impl Material {
    /// Whether the texture repeats across U and V (see [`wraps`]).
    pub fn wraps(&self) -> [bool; 2] {
        wraps(&self.raw)
    }
}

/// Whether a 0x8C-byte material record's texture repeats across U and V:
/// bytes +0x82 and +0x83, 0 = clamp (the original's model and world
/// loaders, 0x4eec00 and 0x4f3cc0). Backdrop cards such as Dunes' islands
/// clamp V, so their texture's clear top isn't repeated over them.
pub fn wraps(raw: &[u8]) -> [bool; 2] {
    [raw.get(0x82).is_none_or(|b| *b != 0), raw.get(0x83).is_none_or(|b| *b != 0)]
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
                // One word (always 0 so far) that track patches don't have.
                r.skip(4)?;
                let uv = r.f32s::<32>()?;
                let uv2 = r.f32s::<32>()?;
                r.seek(start + 148 * 4);
                let c = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
                // Stored column by column, unlike the control points: the
                // original transposes the table as it loads it (0x123be0).
                let mut stored = [-1i32; 16];
                for b in &mut stored {
                    *b = r.i32()?;
                }
                let bones = std::array::from_fn(|k| stored[(k % 4) * 4 + k / 4]);
                r.seek(start + PATCH_SIZE);
                patches.push(Patch {
                    header,
                    control: std::array::from_fn(|i| [xs[i], ys[i], zs[i]]),
                    uv: patch_uvs(header[0] == 1, &uv),
                    uv2: patch_uvs(header[1] == 1, &uv2),
                    // Stored in the same corner order as the UV nets.
                    corner_color: [c[0], c[3], c[2], c[1]],
                    bones,
                });
            }
            nodes.push(Node { header, indices, vertices, patches });
        }
        if r.remaining() != 0 {
            return Err(Error::Format(format!("ROM has {} trailing bytes", r.remaining())));
        }
        Ok(Model { bbox_min, bbox_max, materials, locators, points, sprites, nodes })
    }
}

/// Control-point index (row-major) -> position in a stored UV net. Nets
/// list the four corners first (control points 0, 12, 15, 3), then the
/// inner points of the 0-12 edge, the two inner columns, and the inner
/// points of the 3-15 edge.
pub(crate) const NET_ORDER: [usize; 16] = [0, 6, 10, 3, 4, 7, 11, 14, 5, 8, 12, 15, 1, 9, 13, 2];

/// One layer of patch texture coordinates: a full 16-point net, or only the
/// four corners (the rest of the record is then stale memory) spread
/// bilinearly.
pub(crate) fn patch_uvs(full: bool, raw: &[f32; 32]) -> [[f32; 2]; 16] {
    let at = |i: usize| [raw[i * 2], raw[i * 2 + 1]];
    if full {
        return std::array::from_fn(|k| at(NET_ORDER[k]));
    }
    let (c0, c1, c2, c3) = (at(0), at(1), at(2), at(3));
    std::array::from_fn(|k| {
        let (s, t) = ((k / 4) as f32 / 3.0, (k % 4) as f32 / 3.0);
        std::array::from_fn(|d| {
            c0[d] * (1.0 - s) * (1.0 - t) + c1[d] * s * (1.0 - t) + c2[d] * s * t + c3[d] * (1.0 - s) * t
        })
    })
}

impl Patch {
    /// Evaluates the patch on an (n+1) x (n+1) grid (u along control-point
    /// columns, v along rows).
    pub fn tessellate(&self, n: usize) -> Tessellation {
        self.tessellate_grid(n, n)
    }

    /// Evaluates the patch on an (nu+1) x (nv+1) grid. `nu` = 1 gives a
    /// strip: the first and last control columns, joined straight across.
    pub fn tessellate_grid(&self, nu: usize, nv: usize) -> Tessellation {
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
        let mut t = Tessellation { cols: nu + 1, pos: Vec::new(), uv: Vec::new(), uv2: Vec::new(), indices: Vec::new() };
        for j in 0..=nv {
            let bv = bern(j as f32 / nv as f32);
            for i in 0..=nu {
                let bu = bern(i as f32 / nu as f32);
                t.pos.push(eval(&self.control, &bu, &bv));
                t.uv.push(eval(&self.uv, &bu, &bv));
                t.uv2.push(eval(&self.uv2, &bu, &bv));
            }
        }
        let w = (nu + 1) as u32;
        for j in 0..nv as u32 {
            for i in 0..nu as u32 {
                let a = j * w + i;
                t.indices.extend_from_slice(&[a, a + 1, a + w, a + 1, a + w + 1, a + w]);
            }
        }
        t
    }
}
