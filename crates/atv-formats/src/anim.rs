//! Rider skeletons and animations.
//!
//! - `.bas` ("BAS 1.02"): a rider's skeleton in its bind pose (a T-pose).
//! - `.bac` ("BAC 1.03"): a sampled animation of the same skeleton.
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

    pub fn key(&self, bone: usize, frame: usize) -> Key {
        self.keys[bone * self.frame_count + frame.min(self.frame_count - 1)]
    }
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
