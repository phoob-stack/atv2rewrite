//! "ROW 2.25" track worlds (`Levels/<Track>/World/<Track>.row`).
//!
//! Known layout (little-endian):
//! ```text
//! 0x00 "ROW 2.25\0" + pad, u32 fog/clear colour (0xAARRGGBB)
//! 0x10 u32 material_count, u32 0, u32 locator_count, u32 point_count,
//!      u32 light_count, u32 instance_count, u32 sprite_count, u32 0,
//!      u32 effect_count (water, mist...), u32 unknown
//! 0x38 materials[material_count]   0x8C bytes each, name in the first 32
//!      locators[locator_count]     0x7C bytes: name[64], pos, scale, 3x3
//!      points[point_count]         0x2C bytes
//!      lights[light_count]         0x4C bytes: name[32], u32 type, u32,
//!                                   pos, dir, rgb (0..255 floats)
//!      instances[instance_count]   0x80 bytes: flags[32] ("FREE" = can be
//!                                   knocked over), model[32] (".clm"),
//!                                   pos, scale, 3x3, u32 tint
//!      sprites[sprite_count]       0x68 bytes (same as ROM sprites)
//!      ...effects and a spatial tree interleaved with mesh blocks...
//! mesh block: u32 flags, i32 material, i32 material2, i32, u32 tri_count,
//!             u32 vertex_count, u32 patch_count, u32 patch_count2, i32 x3,
//!             u32 indices[tri_count*3], Vertex[vertex_count] (52 bytes),
//!             Patch[patch_count] (604 bytes)
//! ```
//! The spatial tree between mesh blocks is not decoded yet; mesh blocks are
//! found by validating every candidate header. What's left between them
//! (95-99% of each file is accounted for) is 48-byte tree nodes.

use crate::bin::Reader;
use crate::rom::{Locator, Patch, Sprite, Vertex};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct Light {
    pub name: String,
    pub kind: u32,
    pub pos: [f32; 3],
    pub dir: [f32; 3],
    /// 0..255 per channel.
    pub color: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub flags: String,
    /// Model name with a `.clm` extension; the mesh is the `.rom` with the
    /// same stem in the level or generic model folders.
    pub model: String,
    pub pos: [f32; 3],
    pub scale: [f32; 3],
    pub rotation: [f32; 9],
    pub tint: u32,
}

#[derive(Debug, Clone)]
pub struct Mesh {
    pub offset: usize,
    pub flags: u32,
    pub material: i32,
    pub material2: i32,
    pub indices: Vec<u32>,
    pub vertices: Vec<Vertex>,
    pub patches: Vec<Patch>,
}

#[derive(Debug, Clone)]
pub struct World {
    pub fog_color: u32,
    pub materials: Vec<String>,
    pub locators: Vec<Locator>,
    pub lights: Vec<Light>,
    pub instances: Vec<Instance>,
    pub sprites: Vec<Sprite>,
    pub meshes: Vec<Mesh>,
}

const VERTEX_SIZE: usize = 52;
const PATCH_SIZE: usize = 604;

impl World {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if !bytes.starts_with(b"ROW 2.25") {
            return Err(Error::Magic { what: "ROW world", expected: "ROW 2.25" });
        }
        let mut r = Reader::at(bytes, 0x0C, "ROW world");
        let fog_color = r.u32()?;
        let nmat = r.u32()? as usize;
        r.u32()?;
        let nloc = r.u32()?;
        let npts = r.u32()?;
        let nlight = r.u32()?;
        let ninst = r.u32()?;
        let nspr = r.u32()?;
        r.seek(0x38);
        let mut materials = Vec::new();
        for _ in 0..nmat {
            materials.push(crate::bin::cstr(r.bytes(0x8C)?));
        }
        let mut locators = Vec::new();
        for _ in 0..nloc {
            let name = r.name(64)?;
            let pos = r.vec3()?;
            let scale = r.vec3()?;
            let rotation = r.f32s::<9>()?;
            locators.push(Locator { name, pos, scale, rotation });
        }
        r.skip(npts as usize * 0x2C)?;
        let mut lights = Vec::new();
        for _ in 0..nlight {
            let name = r.name(32)?;
            let kind = r.u32()?;
            r.u32()?;
            lights.push(Light { name, kind, pos: r.vec3()?, dir: r.vec3()?, color: r.vec3()? });
        }
        let mut instances = Vec::new();
        for _ in 0..ninst {
            let flags = r.name(32)?;
            let model = r.name(32)?;
            let pos = r.vec3()?;
            let scale = r.vec3()?;
            let rotation = r.f32s::<9>()?;
            let tint = r.u32()?;
            instances.push(Instance { flags, model, pos, scale, rotation, tint });
        }
        let mut sprites = Vec::new();
        for _ in 0..nspr {
            let kind = r.name(32)?;
            let material = r.name(32)?;
            sprites.push(Sprite { kind, material, params: r.f32s::<10>()? });
        }
        let meshes = scan_meshes(bytes, r.pos(), nmat as i32)?;
        Ok(World { fog_color, materials, locators, lights, instances, sprites, meshes })
    }
}

fn scan_meshes(b: &[u8], start: usize, nmat: i32) -> Result<Vec<Mesh>> {
    let words = b.len() / 4;
    let w = |i: usize| i32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
    let mut out = Vec::new();
    let mut i = start.div_ceil(4);
    while i + 11 <= words {
        if let Some((mesh, end)) = try_mesh(b, i, nmat, &w)? {
            out.push(mesh);
            i = end / 4;
        } else {
            i += 1;
        }
    }
    Ok(out)
}

fn try_mesh(
    b: &[u8],
    i: usize,
    nmat: i32,
    w: &impl Fn(usize) -> i32,
) -> Result<Option<(Mesh, usize)>> {
    let (flags, mat, mat2, x) = (w(i), w(i + 1), w(i + 2), w(i + 3));
    let (ntri, nv, npatch, npatch2) = (w(i + 4), w(i + 5), w(i + 6), w(i + 7));
    let small = |q: i32| (-1..64).contains(&q);
    let plausible = (0..5000).contains(&npatch2)
        && (0..65536).contains(&flags)
        && (-1..nmat).contains(&mat)
        && (-1..nmat).contains(&mat2)
        && (0..200_000).contains(&ntri)
        && (0..200_000).contains(&nv)
        && (0..5000).contains(&npatch)
        && (ntri > 0) == (nv > 0)
        && (ntri > 0 || npatch > 0 || npatch2 > 0)
        && small(x)
        && small(w(i + 8))
        && small(w(i + 9))
        && small(w(i + 10));
    if !plausible {
        return Ok(None);
    }
    // Two patch counts; both use the same record (the second kind's
    // meaning is not known yet).
    let (ntri, nv, npatch) = (ntri as usize, nv as usize, (npatch + npatch2) as usize);
    let idx_start = (i + 11) * 4;
    let end = idx_start + ntri * 12 + nv * VERTEX_SIZE + npatch * PATCH_SIZE;
    if end > b.len() {
        return Ok(None);
    }
    let mut r = Reader::at(b, idx_start, "ROW mesh");
    let mut indices = Vec::with_capacity(ntri * 3);
    for _ in 0..ntri * 3 {
        let v = r.u32()?;
        if v as usize >= nv {
            return Ok(None);
        }
        indices.push(v);
    }
    let mut vertices = Vec::with_capacity(nv);
    for _ in 0..nv {
        vertices.push(Vertex {
            pos: r.vec3()?,
            normal: r.vec3()?,
            uv: r.vec2()?,
            extra: r.f32s::<4>()?,
            color: r.u32()?,
        });
    }
    let mut patches = Vec::with_capacity(npatch);
    for _ in 0..npatch {
        let start = r.pos();
        let header = [r.u32()?, r.u32()?, r.u32()?];
        if header.iter().any(|&h| h >= 16) {
            return Ok(None);
        }
        let xs = r.f32s::<16>()?;
        let ys = r.f32s::<16>()?;
        let zs = r.f32s::<16>()?;
        let uv = r.f32s::<32>()?;
        r.seek(start + 147 * 4);
        let corner_color = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
        r.seek(start + PATCH_SIZE);
        patches.push(Patch {
            header,
            control: std::array::from_fn(|k| [xs[k], ys[k], zs[k]]),
            uv: Patch::uvs(header[0], &uv),
            corner_color,
        });
    }
    Ok(Some((
        Mesh { offset: i * 4, flags: flags as u32, material: mat, material2: mat2, indices, vertices, patches },
        end,
    )))
}
