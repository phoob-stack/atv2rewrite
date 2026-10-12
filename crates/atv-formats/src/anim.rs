//! Rider skeletons and animations.
//!
//! - `.bas` ("BAS 1.02"): a rider's skeleton in its bind pose (a T-pose).
//! - `.bac` ("BAC 1.03"): a sampled animation of the same skeleton.
//! - `.cuban` ("CUBAN 1.02"): the same, packed. Only the retail disc's
//!   riding, crash and stunt clips are all CUBAN; the prototype has both.
//! - `atvrider.skel`: text listing the left/right bone pairs (for mirroring)
//!   and the two-bone limbs (neck, spine, elbows, knees).
//!
//! All 24 riders share one 31-bone hierarchy; their `.bas` files differ only
//! in bone lengths. Rider meshes (`.rom`) are rigidly skinned: each vertex
//! and patch control point names one bone (see [`crate::rom::Vertex::bone`]).
//!
//! ```text
//! BAS: "BAS 1.02\0" + 3 pad, u32 bone_count, then per bone (32 B):
//!      f32 offset[3], f32 rotation[4] (x, y, z, w), u32 child_count
//! BAC: "BAC 1.03\0" + 3 pad, u32 bone_count, u32 frame_count, then
//!      bone-major keys: for each bone, for each frame (28 B):
//!      f32 position[3], f32 rotation[4] (x, y, z, w)
//! CUBAN: "CUBAN 1.02\0" + pad (0x10), u8 bone_count (0x10),
//!      u16 frame_count (0x14), u32 flags (0x18, bit 0 = has a base pose),
//!      u32 position_bits, u32 rotation_bits (0x1c, 0x20: 8, 16 or 32),
//!      u32 position_count, u32 rotation_count (0x24, 0x28), then the
//!      palettes (f32 position[3] each, f32 rotation[4] each) and two index
//!      lists, each padded to 4 bytes: bone-major like BAC
//!      (`bone * frames + frame`), then with a base pose one more key per
//!      bone. Each frame key is relative to its bone's base key: positions
//!      add, rotations multiply (`key * base`). Loader 0xf1c10 and sampler
//!      0x19c00/0xea330 in the retail XBE.
//! ```
//!
//! Bones are stored depth-first, so `child_count` rebuilds the hierarchy.
//! Positions and rotations are relative to the parent bone. A BAC's root
//! key is in vehicle-model space (the riding animations put the pelvis on
//! the saddle of the shared quad layout); the other keys repeat the bone
//! offsets of a reference rider. Frames are sampled at a fixed rate.

use crate::bin::Reader;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct Bone {
    pub parent: Option<usize>,
    /// Bind-pose offset from the parent bone.
    pub offset: [f32; 3],
    /// Bind-pose rotation relative to the parent (x, y, z, w).
    pub rotation: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
}

impl Skeleton {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if !bytes.starts_with(b"BAS 1.0") {
            return Err(Error::Magic { what: "BAS skeleton", expected: "BAS 1.02" });
        }
        let mut r = Reader::at(bytes, 0x0C, "BAS skeleton");
        let n = r.count(32)?;
        let mut bones = Vec::with_capacity(n);
        // Bones whose children are still to come, with how many remain.
        let mut open: Vec<(usize, u32)> = Vec::new();
        for i in 0..n {
            let offset = r.vec3()?;
            let rotation = r.f32s::<4>()?;
            let children = r.u32()?;
            let parent = open.last().map(|&(p, _)| p);
            if let Some(top) = open.last_mut() {
                top.1 -= 1;
                if top.1 == 0 {
                    open.pop();
                }
            } else if i != 0 {
                return Err(Error::Format("BAS skeleton has more than one root".into()));
            }
            if children > 0 {
                open.push((i, children));
            }
            bones.push(Bone { parent, offset, rotation });
        }
        Ok(Skeleton { bones })
    }

    /// Bind-pose position of every bone in model space. Bind rotations are
    /// identity in every rider, so this is the sum of offsets down the tree.
    pub fn bind_positions(&self) -> Vec<[f32; 3]> {
        let mut out: Vec<[f32; 3]> = Vec::with_capacity(self.bones.len());
        for b in &self.bones {
            let base = b.parent.map_or([0.0; 3], |p| out[p]);
            out.push([base[0] + b.offset[0], base[1] + b.offset[1], base[2] + b.offset[2]]);
        }
        out
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Key {
    pub position: [f32; 3],
    /// (x, y, z, w)
    pub rotation: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct Animation {
    pub bone_count: usize,
    pub frame_count: usize,
    /// `keys[bone * frame_count + frame]`.
    pub keys: Vec<Key>,
}

impl Animation {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if !bytes.starts_with(b"BAC 1.0") {
            return Err(Error::Magic { what: "BAC animation", expected: "BAC 1.03" });
        }
        let mut r = Reader::at(bytes, 0x0C, "BAC animation");
        let bone_count = r.u32()? as usize;
        let frame_count = r.u32()? as usize;
        let total = bone_count.saturating_mul(frame_count);
        if total.saturating_mul(28) != r.remaining() {
            return Err(Error::Format(format!(
                "BAC: {bone_count} bones x {frame_count} frames does not match {} bytes",
                r.remaining()
            )));
        }
        let mut keys = Vec::with_capacity(total);
        for _ in 0..total {
            keys.push(Key { position: r.vec3()?, rotation: r.f32s::<4>()? });
        }
        Ok(Animation { bone_count, frame_count, keys })
    }

    /// A BAC or CUBAN clip, told apart by its magic.
    pub fn parse_any(bytes: &[u8]) -> Result<Self> {
        if bytes.starts_with(b"CUBAN") { Self::parse_cuban(bytes) } else { Self::parse(bytes) }
    }

    pub fn parse_cuban(bytes: &[u8]) -> Result<Self> {
        if !bytes.starts_with(b"CUBAN 1.0") {
            return Err(Error::Magic { what: "CUBAN animation", expected: "CUBAN 1.02" });
        }
        let mut r = Reader::at(bytes, 0x10, "CUBAN animation");
        let bone_count = r.u8()? as usize;
        r.seek(0x14);
        let frame_count = r.u16()? as usize;
        r.seek(0x18);
        let based = r.u32()? & 1 != 0;
        let (pos_bits, rot_bits) = (r.u32()?, r.u32()?);
        let (pos_count, rot_count) = (r.u32()? as usize, r.u32()? as usize);
        if pos_count > r.remaining() / 12 || rot_count > r.remaining() / 16 {
            return Err(Error::Format("CUBAN: palette runs past the end".into()));
        }
        let positions = (0..pos_count).map(|_| r.vec3()).collect::<Result<Vec<_>>>()?;
        let rotations = (0..rot_count).map(|_| r.f32s::<4>()).collect::<Result<Vec<_>>>()?;
        let keys = bone_count * frame_count;
        let n = keys + if based { bone_count } else { 0 };
        let mut indices = |bits: u32| -> Result<Vec<usize>> {
            let list = (0..n)
                .map(|_| match bits {
                    8 => r.u8().map(usize::from),
                    16 => r.u16().map(usize::from),
                    32 => r.u32().map(|v| v as usize),
                    _ => Err(Error::Format(format!("CUBAN: {bits}-bit indices"))),
                })
                .collect::<Result<Vec<_>>>()?;
            // Each list is padded to a multiple of 4 bytes.
            let pad = (4 - n * bits as usize / 8 % 4) % 4;
            r.skip(pad.min(r.remaining()))?;
            Ok(list)
        };
        let pos_index = indices(pos_bits)?;
        let rot_index = indices(rot_bits)?;
        let get = |i: usize| -> Result<Key> {
            match (positions.get(pos_index[i]), rotations.get(rot_index[i])) {
                (Some(&position), Some(&rotation)) => Ok(Key { position, rotation }),
                _ => Err(Error::Format("CUBAN: index outside its palette".into())),
            }
        };
        let mut out = Vec::with_capacity(keys);
        for i in 0..keys {
            let mut k = get(i)?;
            if based {
                let base = get(keys + i / frame_count.max(1))?;
                k.position = [0, 1, 2].map(|a| k.position[a] + base.position[a]);
                k.rotation = quat_mul(k.rotation, base.rotation);
            }
            out.push(k);
        }
        Ok(Animation { bone_count, frame_count, keys: out })
    }

    pub fn key(&self, bone: usize, frame: usize) -> Key {
        self.keys[bone * self.frame_count + frame.min(self.frame_count - 1)]
    }
}

/// Hamilton product `a * b` of (x, y, z, w) quaternions.
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// The parts of `atvrider.skel` the game needs.
#[derive(Debug, Clone, Default)]
pub struct SkelInfo {
    /// Left/right bone pairs, used to mirror a pose.
    pub mirror: Vec<(usize, usize)>,
}

impl SkelInfo {
    pub fn parse(text: &str) -> Self {
        let mut mirror = Vec::new();
        for line in text.lines() {
            let Some(rest) = line.trim().strip_prefix("mirror") else { continue };
            let mut it = rest.split(',').map(|s| s.trim().parse::<usize>());
            if let (Some(Ok(a)), Some(Ok(b))) = (it.next(), it.next()) {
                mirror.push((a, b));
            }
        }
        SkelInfo { mirror }
    }

    /// Bone index of each bone's mirror twin (itself for centre bones).
    pub fn mirror_map(&self, bone_count: usize) -> Vec<usize> {
        let mut map: Vec<usize> = (0..bone_count).collect();
        for &(a, b) in &self.mirror {
            if a < bone_count && b < bone_count {
                map[a] = b;
                map[b] = a;
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One bone, two frames, a base pose: 8-bit position and 16-bit
    /// rotation indices (each list padded to 4 bytes).
    #[test]
    fn cuban_keys_are_relative_to_the_base_pose() {
        let mut b = b"CUBAN 1.02\0\0\0\0\0\0".to_vec();
        b.extend([1, 0, 0, 0]); // bones
        b.extend([2, 0, 0, 0]); // frames
        b.extend(1u32.to_le_bytes()); // has a base pose
        for v in [8u32, 16, 2, 2] {
            b.extend(v.to_le_bytes());
        }
        for f in [0.0f32, 1.0, 0.0, 10.0, 20.0, 30.0] {
            b.extend(f.to_le_bytes());
        }
        let half = std::f32::consts::FRAC_1_SQRT_2;
        for f in [0.0f32, 0.0, 0.0, 1.0, 0.0, half, 0.0, half] {
            b.extend(f.to_le_bytes());
        }
        b.extend([0, 0, 1, 0]); // positions: frame 0, frame 1, base, pad
        b.extend([0, 0, 1, 0, 1, 0, 0, 0]); // rotations, then 2 pad bytes
        let a = Animation::parse_any(&b).unwrap();
        assert_eq!((a.bone_count, a.frame_count), (1, 2));
        assert_eq!(a.key(0, 1).position, [10.0, 21.0, 30.0]);
        // A quarter turn about Y on top of another: a half turn.
        let r = a.key(0, 1).rotation;
        assert!(r[1].abs() > 0.999 && r[3].abs() < 1e-6, "{r:?}");
        assert_eq!(a.key(0, 0).rotation, [0.0, half, 0.0, half]);
    }
}
