//! Static track collision: triangles bucketed in a uniform XZ grid for
//! fast ray casts.

use glam::{Vec2, Vec3, Vec3Swizzles};

use crate::surface::{self, Surface, Surfaces};

pub struct TrackCollision {
    tris: Vec<[Vec3; 3]>,
    /// Surface id of each triangle ([`surface::UNKNOWN`] when untagged).
    surface_ids: Vec<u8>,
    /// Which side of each triangle is solid to bodies: 1 in front of its
    /// winding normal, -1 behind it, 0 either (see [`Self::face_contact`]).
    facing: Vec<i8>,
    surfaces: Surfaces,
    min: Vec2,
    cell: f32,
    dims: (usize, usize),
    cells: Vec<Vec<u32>>,
}

#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub t: f32,
    pub point: Vec3,
    pub normal: Vec3,
    /// Surface id of the triangle hit.
    pub surface: u8,
}

impl TrackCollision {
    pub fn new(tris: Vec<[Vec3; 3]>, cell: f32) -> Self {
        let mut min = Vec2::splat(f32::MAX);
        let mut max = Vec2::splat(f32::MIN);
        for t in &tris {
            for v in t {
                min = min.min(v.xz());
                max = max.max(v.xz());
            }
        }
        if tris.is_empty() {
            min = Vec2::ZERO;
            max = Vec2::ONE;
        }
        let dims = (
            ((max.x - min.x) / cell) as usize + 1,
            ((max.y - min.y) / cell) as usize + 1,
        );
        let mut cells = vec![Vec::new(); dims.0 * dims.1];
        for (i, t) in tris.iter().enumerate() {
            let lo = t[0].xz().min(t[1].xz()).min(t[2].xz());
            let hi = t[0].xz().max(t[1].xz()).max(t[2].xz());
            let (x0, z0) = Self::cell_of(min, cell, dims, lo);
            let (x1, z1) = Self::cell_of(min, cell, dims, hi);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    cells[z * dims.0 + x].push(i as u32);
                }
            }
        }
        Self {
            surface_ids: vec![surface::UNKNOWN; tris.len()],
            facing: vec![0; tris.len()],
            surfaces: Surfaces::default(),
            tris,
            min,
            cell,
            dims,
            cells,
        }
    }

    fn cell_of(min: Vec2, cell: f32, dims: (usize, usize), p: Vec2) -> (usize, usize) {
        let c = ((p - min) / cell).max(Vec2::ZERO);
        (
            (c.x as usize).min(dims.0 - 1),
            (c.y as usize).min(dims.1 - 1),
        )
    }

    /// Collision whose triangles carry surface ids (one per triangle).
    pub fn with_surfaces(tris: Vec<[Vec3; 3]>, ids: Vec<u8>, cell: f32) -> Self {
        let mut c = Self::new(tris, cell);
        if ids.len() == c.tris.len() {
            c.surface_ids = ids;
        }
        c
    }

    /// Tags every triangle with a surface id and sets the surface table.
    pub fn set_surfaces(&mut self, ids: Vec<u8>, surfaces: Surfaces) {
        if ids.len() == self.tris.len() {
            self.surface_ids = ids;
        }
        self.surfaces = surfaces;
    }

    /// Makes the first `facing.len()` triangles one-sided (see `facing`).
    /// The `.kdt` stores each face's outward normal; mirrored into the sim
    /// frame its triangles wind the other way, so they take -1.
    pub fn set_facing(&mut self, facing: &[i8]) {
        for (f, &s) in self.facing.iter_mut().zip(facing) {
            *f = s;
        }
    }

    pub fn triangles(&self) -> &[[Vec3; 3]] {
        &self.tris
    }

    /// Grip and drag of surface `id`.
    pub fn surface(&self, id: u8) -> Surface {
        self.surfaces.get(id)
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// Closest hit along `origin + dir * t` for `t` in `0..=max_t`.
    /// Walks the grid cells under the ray's XZ footprint.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<Hit> {
        self.cast(origin, dir, max_t, false)
    }

    /// [`Self::raycast`] that passes through faces solid on one side only
    /// when it meets them from behind, as the original's wheels do (they
    /// test faces as the body's spheres do, [`Self::face_contact`]): a
    /// wheel under a snow bank's overhang doesn't stand on its underside.
    pub fn raycast_front(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<Hit> {
        self.cast(origin, dir, max_t, true)
    }

    fn cast(&self, origin: Vec3, dir: Vec3, max_t: f32, front_only: bool) -> Option<Hit> {
        let end = origin + dir * max_t;
        let lo = origin.xz().min(end.xz());
        let hi = origin.xz().max(end.xz());
        let (x0, z0) = Self::cell_of(self.min, self.cell, self.dims, lo);
        let (x1, z1) = Self::cell_of(self.min, self.cell, self.dims, hi);
        let mut best: Option<Hit> = None;
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &i in &self.cells[z * self.dims.0 + x] {
                    let [a, b, c] = self.tris[i as usize];
                    let facing = self.facing[i as usize];
                    if front_only && facing != 0 && (b - a).cross(c - a).dot(dir) * facing as f32 > 0.0 {
                        continue;
                    }
                    if let Some(t) = ray_tri(origin, dir, a, b, c)
                        && t <= max_t
                        && best.is_none_or(|h| t < h.t)
                    {
                        let mut n = (b - a).cross(c - a).normalize_or_zero();
                        if n.dot(dir) > 0.0 {
                            n = -n;
                        }
                        best = Some(Hit {
                            t,
                            point: origin + dir * t,
                            normal: n,
                            surface: self.surface_ids[i as usize],
                        });
                    }
                }
            }
        }
        best
    }

    /// Deepest overlap of a sphere with the track: the closest surface point
    /// within `radius` of `centre`, with the normal pointing from that point
    /// towards the centre and the penetration depth.
    pub fn sphere(&self, centre: Vec3, radius: f32) -> Option<Hit> {
        let r = Vec2::splat(radius);
        let (x0, z0) = Self::cell_of(self.min, self.cell, self.dims, centre.xz() - r);
        let (x1, z1) = Self::cell_of(self.min, self.cell, self.dims, centre.xz() + r);
        let mut best: Option<(f32, Vec3, u8)> = None;
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &i in &self.cells[z * self.dims.0 + x] {
                    let [a, b, c] = self.tris[i as usize];
                    let p = closest_on_tri(centre, a, b, c);
                    let d2 = (centre - p).length_squared();
                    if d2 < radius * radius && best.is_none_or(|(b2, ..)| d2 < b2) {
                        best = Some((d2, p, self.surface_ids[i as usize]));
                    }
                }
            }
        }
        let (d2, p, surface) = best?;
        let d = d2.sqrt();
        let normal = if d > 1e-6 { (centre - p) / d } else { Vec3::Y };
        Some(Hit {
            t: radius - d,
            point: p,
            normal,
            surface,
        })
    }
}

impl TrackCollision {
    /// The triangles that may lie within `radius` of `centre` (each once),
    /// for [`Self::face_contact`].
    pub fn near(&self, centre: Vec3, radius: f32) -> Vec<u32> {
        let r = Vec2::splat(radius);
        let (x0, z0) = Self::cell_of(self.min, self.cell, self.dims, centre.xz() - r);
        let (x1, z1) = Self::cell_of(self.min, self.cell, self.dims, centre.xz() + r);
        let mut out = Vec::new();
        for z in z0..=z1 {
            for x in x0..=x1 {
                out.extend(self.cells[z * self.dims.0 + x].iter().copied().filter(|&i| {
                    let t = &self.tris[i as usize];
                    let lo = t[0].min(t[1]).min(t[2]) - radius;
                    let hi = t[0].max(t[1]).max(t[2]) + radius;
                    centre.cmpge(lo).all() && centre.cmple(hi).all()
                }));
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// A body sphere against triangle `i`, as the original tests one
    /// (0xd6490, edge contacts off): it touches when its centre is over the
    /// face, within the radius (plus 0.03) in front of it, and was not
    /// right through it last step (`prev` no more than the radius plus
    /// 0.03 behind it; `centre` stands in for a `prev` that isn't finite).
    /// Returns the depth as `t` (0 within the 0.03 margin), the point on
    /// the sphere's surface and the face's normal. A triangle solid on
    /// either side takes the side `prev` was on.
    pub fn face_contact(&self, i: u32, centre: Vec3, prev: Vec3, radius: f32) -> Option<Hit> {
        const MARGIN: f32 = 0.03;
        let [a, b, c] = self.tris[i as usize];
        let wind = (b - a).cross(c - a);
        let w = wind.try_normalize()?;
        // Without a last step (just placed), where it is now stands in.
        let prev = if prev.is_finite() { prev } else { centre };
        let n = match self.facing[i as usize] {
            0 => {
                if w.dot(prev - a) < 0.0 { -w } else { w }
            }
            f => w * f as f32,
        };
        let d = n.dot(centre - a);
        if d > radius + MARGIN || n.dot(prev - a) < -radius - MARGIN {
            return None;
        }
        // Over the face: inside all three edges.
        let inside = [(a, b), (b, c), (c, a)].iter().all(|&(p, q)| (q - p).cross(centre - p).dot(wind) >= 0.0);
        if !inside {
            return None;
        }
        Some(Hit {
            t: (radius - d).max(0.0),
            point: centre - n * radius,
            normal: n,
            surface: self.surface_ids[i as usize],
        })
    }
}

/// Closest point on triangle `abc` to `p` (Ericson, Real-Time Collision
/// Detection 5.1.5).
fn closest_on_tri(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Moller-Trumbore, double-sided.
fn ray_tri(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_floor_quad() {
        let a = Vec3::new(-10.0, 0.0, -10.0);
        let b = Vec3::new(10.0, 0.0, -10.0);
        let c = Vec3::new(10.0, 0.0, 10.0);
        let d = Vec3::new(-10.0, 0.0, 10.0);
        let col = TrackCollision::new(vec![[a, b, c], [a, c, d]], 4.0);
        let hit = col
            .raycast(Vec3::new(1.0, 5.0, 2.0), Vec3::NEG_Y, 10.0)
            .unwrap();
        assert!((hit.t - 5.0).abs() < 1e-5);
        assert!(hit.normal.y > 0.99);
        assert!(
            col.raycast(Vec3::new(1.0, 5.0, 2.0), Vec3::NEG_Y, 4.0)
                .is_none()
        );
        let s = col.sphere(Vec3::new(1.0, 0.2, 2.0), 0.5).unwrap();
        assert!((s.t - 0.3).abs() < 1e-5 && s.normal.y > 0.99);
        assert!(col.sphere(Vec3::new(1.0, 0.6, 2.0), 0.5).is_none());
    }

    #[test]
    fn one_sided_faces() {
        // Wound downwards, solid on top as a `.kdt` face is in the sim frame.
        let (a, b, c) = (Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 0.0, -10.0), Vec3::new(10.0, 0.0, 10.0));
        let mut col = TrackCollision::new(vec![[a, b, c]], 4.0);
        col.set_facing(&[-1]);
        let p = Vec3::new(5.0, 0.3, -2.0);
        let hit = col.face_contact(0, p, p, 0.4).unwrap();
        assert!(hit.normal.y > 0.99 && (hit.t - 0.1).abs() < 1e-5);
        // Far under it, just placed: no contact to drag it down.
        let under = Vec3::new(5.0, -2.0, -2.0);
        assert!(col.face_contact(0, under, Vec3::NAN, 0.4).is_none());
        // Sunk through it within a radius since the last step: pushed back up.
        assert!(col.face_contact(0, Vec3::new(5.0, -0.2, -2.0), p, 0.4).is_some());
        // Rays meet it from the top only with `raycast_front`.
        assert!(col.raycast_front(Vec3::new(5.0, 1.0, -2.0), Vec3::NEG_Y, 2.0).is_some());
        assert!(col.raycast_front(under, Vec3::Y, 3.0).is_none());
        assert!(col.raycast(under, Vec3::Y, 3.0).is_some());
    }
}
