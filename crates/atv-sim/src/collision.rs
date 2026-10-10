//! Static track collision: triangles bucketed in a uniform XZ grid for
//! fast ray casts.

use glam::{Vec2, Vec3, Vec3Swizzles};

pub struct TrackCollision {
    tris: Vec<[Vec3; 3]>,
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
        let dims = (((max.x - min.x) / cell) as usize + 1, ((max.y - min.y) / cell) as usize + 1);
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
        Self { tris, min, cell, dims, cells }
    }

    fn cell_of(min: Vec2, cell: f32, dims: (usize, usize), p: Vec2) -> (usize, usize) {
        let c = ((p - min) / cell).max(Vec2::ZERO);
        ((c.x as usize).min(dims.0 - 1), (c.y as usize).min(dims.1 - 1))
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// Closest hit along `origin + dir * t` for `t` in `0..=max_t`.
    /// Walks the grid cells under the ray's XZ footprint.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<Hit> {
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
                    if let Some(t) = ray_tri(origin, dir, a, b, c)
                        && t <= max_t
                        && best.is_none_or(|h| t < h.t)
                    {
                        let mut n = (b - a).cross(c - a).normalize_or_zero();
                        if n.dot(dir) > 0.0 {
                            n = -n;
                        }
                        best = Some(Hit { t, point: origin + dir * t, normal: n });
                    }
                }
            }
        }
        best
    }
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
        let hit = col.raycast(Vec3::new(1.0, 5.0, 2.0), Vec3::NEG_Y, 10.0).unwrap();
        assert!((hit.t - 5.0).abs() < 1e-5);
        assert!(hit.normal.y > 0.99);
        assert!(col.raycast(Vec3::new(1.0, 5.0, 2.0), Vec3::NEG_Y, 4.0).is_none());
    }
}
