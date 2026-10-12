//! `.hsc` body collision shapes: the quad's own body (`Cars/Generic/
//! Generic.hsc`) and the knock-over props (`Levels/<Env>/Models/
//! <Prop>_Collision.hsc`: cones, signs, barriers, planks, deckchairs).
//! Read by the original at 0x24c10.
//!
//! ```text
//! 0x00 u32 hull_count
//!      hull[hull_count]:
//!        u32 plane_count
//!        plane[plane_count]: f32[3] normal, f32 d
//!                            (a point p is inside where normal·p + d <= 0)
//! u32 sphere_count
//! sphere[sphere_count]: f32[3] centre, f32 radius
//! ```
//!
//! Everything is in model space, in the game frame (like the `.rom`).
//! Every file has one hull. The original keeps a hull as one convex body,
//! the space inside all of its planes, and tests the other body's spheres
//! against it (0xb3da0); spheres meet spheres in 0xb29d0. The prototype's
//! signs list a post's and a panel's box planes in one hull, so there the
//! hull is only where the boxes overlap and the spheres do the work;
//! retail rebuilt them as single hulls. (The game splits the prototype's
//! back into their boxes.)

use crate::bin::Reader;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: [f32; 3],
    pub d: f32,
}

impl Plane {
    /// Signed distance of `p` outside the plane (negative inside).
    pub fn distance(&self, p: [f32; 3]) -> f32 {
        self.normal[0] * p[0] + self.normal[1] * p[1] + self.normal[2] * p[2] + self.d
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sphere {
    pub centre: [f32; 3],
    pub radius: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Hull {
    pub planes: Vec<Plane>,
}

#[derive(Debug, Clone, Default)]
pub struct BodyShape {
    pub hulls: Vec<Hull>,
    pub spheres: Vec<Sphere>,
}

impl BodyShape {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data, "hsc");
        let hull_count = r.count(4)?;
        let mut hulls = Vec::with_capacity(hull_count);
        for _ in 0..hull_count {
            let n = r.count(16)?;
            let mut planes = Vec::with_capacity(n);
            for _ in 0..n {
                planes.push(Plane { normal: r.vec3()?, d: r.f32()? });
            }
            hulls.push(Hull { planes });
        }
        let n = r.count(16)?;
        let mut spheres = Vec::with_capacity(n);
        for _ in 0..n {
            spheres.push(Sphere { centre: r.vec3()?, radius: r.f32()? });
        }
        if r.remaining() != 0 {
            return Err(Error::Format(format!("hsc: {} bytes left over", r.remaining())));
        }
        Ok(Self { hulls, spheres })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn hull_and_spheres() {
        let f = |x: f32| x.to_bits();
        // One hull of two planes (y <= 1, y >= 0), one sphere.
        let data = le(&[1, 2, f(0.0), f(1.0), f(0.0), f(-1.0), f(0.0), f(-1.0), f(0.0), f(0.0), 1, f(0.0), f(0.5), f(0.0), f(0.25)]);
        let s = BodyShape::parse(&data).unwrap();
        assert_eq!(s.hulls.len(), 1);
        assert_eq!(s.hulls[0].planes[0], Plane { normal: [0.0, 1.0, 0.0], d: -1.0 });
        assert!(s.hulls[0].planes[1].distance([0.0, 0.5, 0.0]) < 0.0);
        assert_eq!(s.spheres, vec![Sphere { centre: [0.0, 0.5, 0.0], radius: 0.25 }]);
        // The stalactites' files are empty.
        assert!(BodyShape::parse(&le(&[0, 0])).unwrap().spheres.is_empty());
        assert!(BodyShape::parse(&le(&[1, 9])).is_err());
    }
}
