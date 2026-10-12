//! The quad's collision body: `Cars/Generic/Generic.hsc`, one shape every
//! quad shares (loaded at 0x1f9f8). Its spheres meet the track (0x6fef0)
//! and the other quads' hulls; its hull meets the other quads' spheres
//! (0x6fa00). The file is in the body model's space; the original hangs the
//! model 0.42 m below the physics body (the quad setup at 0xa4f4e) and
//! moves the shape the same way when it loads it.

use std::sync::Arc;

use atv_formats::hsc::BodyShape;
use atv_formats::GameSource;
use glam::Vec3;

/// How far below the physics body the original hangs the quad's model and
/// collision shape.
pub const MODEL_DROP: f32 = 0.42;

/// Spheres and a convex hull, game frame, relative to the centre of mass.
#[derive(Debug, Clone)]
pub struct Body {
    /// Centre and radius.
    pub spheres: Vec<(Vec3, f32)>,
    /// Planes `(n, d)`: inside where `n·p + d <= 0` for all of them.
    pub hull: Vec<(Vec3, f32)>,
    /// How far the shape reaches from the centre of mass.
    pub reach: f32,
}

impl Body {
    pub fn new(spheres: Vec<(Vec3, f32)>, hull: Vec<(Vec3, f32)>) -> Self {
        let reach = spheres.iter().map(|(c, r)| c.length() + r).fold(0.0, f32::max);
        Self { spheres, hull, reach }
    }

    /// The shape from a `.hsc` file, hung [`MODEL_DROP`] below the body as
    /// the original loads it (0x25750 with the quad's offset).
    pub fn from_hsc(hsc: &BodyShape) -> Self {
        let drop = Vec3::new(0.0, -MODEL_DROP, 0.0);
        let hull = hsc
            .hulls
            .last()
            .map(|h| h.planes.iter().map(|p| (Vec3::from(p.normal), p.d - Vec3::from(p.normal).dot(drop))).collect())
            .unwrap_or_default();
        let spheres = hsc.spheres.iter().map(|s| (Vec3::from(s.centre) + drop, s.radius)).collect();
        Self::new(spheres, hull)
    }

    /// The quads' shape from the player's game.
    pub fn from_game(game: &GameSource) -> Option<Arc<Self>> {
        let hsc = BodyShape::parse(&game.read("/Cars/Generic/Generic.hsc").ok()?).ok()?;
        let body = Self::from_hsc(&hsc);
        (!body.spheres.is_empty()).then(|| Arc::new(body))
    }

    /// Stand-in for a game without the file (and for tests): spheres
    /// hand-placed along a Bandit-sized chassis, no hull.
    pub fn fallback() -> Self {
        Self::new(
            vec![
                (Vec3::new(0.0, 0.05, 0.0), 0.22),  // belly
                (Vec3::new(0.0, 0.3, 0.75), 0.25),  // front rack
                (Vec3::new(0.0, 0.7, 0.3), 0.25),   // handlebars
                (Vec3::new(0.0, 0.5, -0.35), 0.3),  // seat
                (Vec3::new(0.0, 0.4, -0.85), 0.25), // rear rack
            ],
            Vec::new(),
        )
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::fallback()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atv_formats::hsc::{Hull, Plane, Sphere};

    #[test]
    fn hung_below_the_body() {
        // A box 0.3..1.0 high and one sphere, in model space.
        let b = |n: [f32; 3], d: f32| Plane { normal: n, d };
        let hsc = BodyShape {
            hulls: vec![Hull { planes: vec![b([0.0, 1.0, 0.0], -1.0), b([0.0, -1.0, 0.0], 0.3)] }],
            spheres: vec![Sphere { centre: [0.0, 0.5, 0.2], radius: 0.25 }],
        };
        let body = Body::from_hsc(&hsc);
        assert!(body.spheres[0].0.abs_diff_eq(Vec3::new(0.0, 0.08, 0.2), 1e-6));
        let inside = |p: Vec3| body.hull.iter().all(|&(n, d)| n.dot(p) + d <= 0.0);
        assert!(inside(Vec3::new(0.0, 0.5, 0.0)) && inside(Vec3::new(0.0, -0.1, 0.0)));
        assert!(!inside(Vec3::new(0.0, 0.6, 0.0)) && !inside(Vec3::new(0.0, -0.13, 0.0)));
        assert!((body.reach - (0.08f32.hypot(0.2) + 0.25)).abs() < 1e-6);
    }
}
