//! `.kdt` track collision (`Levels/<Track>/Kdt/<Track>.kdt`).
//!
//! The original's wheels and body collide with these triangles, not with
//! the `.row` render meshes, and each one names its surface (an index into
//! `Setup/Materials.xml`, in file order: 0 `None`, 1 `Invisible`,
//! 2 `Tarmac` ... 27 `Shallow_Water`). Triangles are stored as planes:
//!
//! ```text
//! 0x00 u32 triangle_count
//! 0x04 triangle[triangle_count], 0x5C bytes each:
//!      f32[4] face plane (normal, d: normal . p + d = 0)
//!      f32[4] x3 edge planes, perpendicular to the face
//!      f32[3] bbox_min, f32[3] bbox_max (padded by 0.03)
//!      u32 surface (low byte; the wheel code masks it with 0xFF)
//! then a spatial tree over the triangles (not read).
//! ```
//!
//! The corners are where the face plane meets each pair of edge planes.

use crate::bin::Reader;
use crate::{Error, Result};

const RECORD: usize = 0x5C;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Triangle {
    /// Corners in the game frame.
    pub corners: [[f32; 3]; 3],
    pub surface: u8,
}

#[derive(Debug, Clone)]
pub struct CollisionTree {
    pub triangles: Vec<Triangle>,
}

impl CollisionTree {
    /// Parses the triangles; degenerate ones (parallel edge planes) are
    /// skipped.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data, "kdt");
        let count = r.u32()? as usize;
        if count > 4_000_000 || 4 + count * RECORD > data.len() {
            return Err(Error::Format(format!("kdt: {count} triangles don't fit in {} bytes", data.len())));
        }
        let mut triangles = Vec::with_capacity(count);
        for _ in 0..count {
            let planes: [[f32; 4]; 4] = [r.f32s()?, r.f32s()?, r.f32s()?, r.f32s()?];
            r.skip(24)?;
            let surface = (r.u32()? & 0xFF) as u8;
            let face = planes[0];
            let corner = |a: [f32; 4], b: [f32; 4]| intersect(face, a, b);
            if let (Some(p0), Some(p1), Some(p2)) =
                (corner(planes[1], planes[2]), corner(planes[2], planes[3]), corner(planes[3], planes[1]))
            {
                triangles.push(Triangle { corners: [p0, p1, p2], surface });
            }
        }
        Ok(Self { triangles })
    }
}

/// The point on all three planes, if they meet in one.
fn intersect(a: [f32; 4], b: [f32; 4], c: [f32; 4]) -> Option<[f32; 3]> {
    let (na, nb, nc) = (vec(a), vec(b), vec(c));
    let bc = cross(nb, nc);
    let det = dot(na, bc);
    if det.abs() < 1e-6 {
        return None;
    }
    let (ca, ab) = (cross(nc, na), cross(na, nb));
    Some([0, 1, 2].map(|k| -(a[3] * bc[k] + b[3] * ca[k] + c[3] * ab[k]) / det))
}

fn vec(p: [f32; 4]) -> [f32; 3] {
    [p[0], p[1], p[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_triangle() {
        // Triangle (0,0,0) (1,0,0) (0,0,1) on the floor, surface 15.
        let planes: [[f32; 4]; 4] = [
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            [0.707_106_8, 0.0, 0.707_106_8, -0.707_106_8],
            [-1.0, 0.0, 0.0, 0.0],
        ];
        let mut b = 1u32.to_le_bytes().to_vec();
        for v in planes.iter().flatten().chain(&[0.0; 6]) {
            b.extend(v.to_le_bytes());
        }
        b.extend(15u32.to_le_bytes());
        let t = CollisionTree::parse(&b).unwrap();
        let tri = t.triangles[0];
        assert_eq!(tri.surface, 15);
        let want = [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]];
        for (got, want) in tri.corners.iter().zip(want) {
            for k in 0..3 {
                assert!((got[k] - want[k]).abs() < 1e-5, "{got:?} vs {want:?}");
            }
        }
        assert!(CollisionTree::parse(&b[..40]).is_err());
    }
}
