//! `.ink` prop collision meshes (`Levels/<Env>/Models/<Prop>.ink`).
//!
//! A static prop placed in a track (rocks, buildings, trees, tyre stacks,
//! the stadium gates) collides as the triangle soup in the `.ink` with the
//! same name as its model; props without one have no collision. Knock-over
//! props (cones, signs, barriers) use `_Collision.hsc` hulls instead.
//!
//! ```text
//! 0x00 u32 magic 0x00CEFDAB
//! 0x04 u32 triangle_count
//! 0x08 triangle[triangle_count]: three f32[3] corners, model space
//!      (game frame, like the `.rom`)
//! ```

use crate::bin::Reader;
use crate::{Error, Result};

const MAGIC: u32 = 0x00CE_FDAB;

#[derive(Debug, Clone)]
pub struct CollisionMesh {
    pub triangles: Vec<[[f32; 3]; 3]>,
}

impl CollisionMesh {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data, "ink");
        if r.u32()? != MAGIC {
            return Err(Error::Magic { what: "ink", expected: "0x00CEFDAB" });
        }
        let count = r.u32()? as usize;
        if count > 1_000_000 || 8 + count * 36 > data.len() {
            return Err(Error::Format(format!("ink: {count} triangles don't fit in {} bytes", data.len())));
        }
        let mut triangles = Vec::with_capacity(count);
        for _ in 0..count {
            triangles.push([r.vec3()?, r.vec3()?, r.vec3()?]);
        }
        Ok(Self { triangles })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_triangle() {
        let mut b = MAGIC.to_le_bytes().to_vec();
        b.extend(1u32.to_le_bytes());
        for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 2.0, 0.0] {
            b.extend(v.to_le_bytes());
        }
        let m = CollisionMesh::parse(&b).unwrap();
        assert_eq!(m.triangles, vec![[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.0]]]);
        assert!(CollisionMesh::parse(&b[..20]).is_err());
    }
}
