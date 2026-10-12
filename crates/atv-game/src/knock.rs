//! Knock-over props: the instances a track flags `FREE` (corner signs,
//! deckchairs, umbrellas, surfboards, cones). A quad that drives into one
//! sends it flying; it tumbles under gravity, bounces off the track and
//! settles where it lands.
//!
//! Quad and prop meet by their `.hsc` shapes, as the original's bodies do:
//! the quad's is `Cars/Generic/Generic.hsc`, a prop's is the
//! `<model>_Collision.hsc` beside its model (0x2e100), each a convex hull
//! and a set of spheres. A sphere of one body inside the other's hull
//! (0xb3da0) or touching one of its spheres (0xb29d0) is a contact. A prop
//! without a `.hsc` (retail's Stadium floor signs) is its model's box.
//! Hitting one knocks the quad, which plays its knock sounds, and costs it
//! the speed the original's contact between the two bodies takes (see
//! [`slowdown`]). Contacts and tumbling run in the fixed physics tick, after
//! every quad has moved, so they don't depend on the frame rate.

use std::sync::Arc;

use bevy::prelude::*;

use atv_formats::hsc::BodyShape;

use crate::{AppState, Game, Player, Race};

pub struct KnockPlugin;

impl Plugin for KnockPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Race), load_quad_shape).add_systems(
            FixedUpdate,
            (hit_props, tumble).chain().after(crate::attack::kicks).run_if(in_state(AppState::Race)),
        );
    }
}

/// A body's `.hsc` shape in its model space (Bevy frame).
#[derive(Debug, Clone, Default)]
pub struct Shape {
    /// Convex hulls, each inside where `n·p + d <= 0` for all its planes.
    hulls: Vec<Vec<(Vec3, f32)>>,
    /// Centre and radius.
    spheres: Vec<(Vec3, f32)>,
    /// The hull's corners.
    corners: Vec<Vec3>,
}

impl Shape {
    pub fn from_hsc(hsc: &BodyShape) -> Self {
        let flip = |v: [f32; 3]| Vec3::new(v[0], v[1], -v[2]);
        // The original keeps one hull (0x24c10); no file has more.
        let planes: Vec<(Vec3, f32)> = hsc.hulls.last().map(|h| h.planes.iter().map(|p| (flip(p.normal), p.d)).collect()).unwrap_or_default();
        // The prototype's signs list a post's and a panel's boxes, six
        // planes each, in their one hull. The original takes the hull
        // whole, which leaves only where the boxes overlap (nothing, for
        // the red signs), so a quad could drive through a post between its
        // spheres. Here they are split back into their boxes, as retail's
        // rebuilt single hulls cover them.
        let boxes: Vec<Vec<(Vec3, f32)>> = planes.chunks(6).map(<[_]>::to_vec).collect();
        let hulls = if planes.len() > 6 && planes.len() % 6 == 0 && boxes.iter().all(|b| corners(b).len() == 8) { boxes } else { vec![planes] };
        Self::new(hulls, hsc.spheres.iter().map(|s| (flip(s.centre), s.radius)).collect())
    }

    /// The box `min..max`.
    pub fn cuboid(min: Vec3, max: Vec3) -> Self {
        let planes = vec![
            (Vec3::X, -max.x),
            (Vec3::NEG_X, min.x),
            (Vec3::Y, -max.y),
            (Vec3::NEG_Y, min.y),
            (Vec3::Z, -max.z),
            (Vec3::NEG_Z, min.z),
        ];
        Self::new(vec![planes], Vec::new())
    }

    fn new(mut hulls: Vec<Vec<(Vec3, f32)>>, spheres: Vec<(Vec3, f32)>) -> Self {
        let corners: Vec<Vec<Vec3>> = hulls.iter().map(|h| corners(h)).collect();
        // A hull with no inside holds nothing.
        let mut c = corners.iter();
        hulls.retain(|_| c.next().is_some_and(|c| !c.is_empty()));
        Self { hulls, spheres, corners: corners.concat() }
    }

    pub fn is_empty(&self) -> bool {
        self.corners.is_empty() && self.spheres.is_empty()
    }

    /// The corners and spheres, as (centre, radius).
    fn points(&self) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.corners.iter().map(|&c| (c, 0.0)).chain(self.spheres.iter().copied())
    }

    fn bounds(&self) -> (Vec3, Vec3) {
        self.points().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), (c, r)| (lo.min(c - r), hi.max(c + r)))
    }

    /// The shape placed by `t`.
    fn posed(&self, t: &Transform) -> Posed {
        let grow = t.scale.abs().max_element();
        let hulls = self
            .hulls
            .iter()
            .map(|h| {
                h.iter()
                    .map(|&(n, d)| {
                        let on = t.transform_point(-n * d);
                        let n = (t.rotation * (n / t.scale)).normalize();
                        (n, -n.dot(on))
                    })
                    .collect()
            })
            .collect();
        let spheres = self.spheres.iter().map(|&(c, r)| (t.transform_point(c), r * grow)).collect();
        Posed { hulls, spheres }
    }
}

/// The corners of the hull `planes`: where three of them meet inside the
/// rest.
fn corners(planes: &[(Vec3, f32)]) -> Vec<Vec3> {
    let mut out: Vec<Vec3> = Vec::new();
    for (i, &(a, da)) in planes.iter().enumerate() {
        for (j, &(b, db)) in planes.iter().enumerate().skip(i + 1) {
            for &(c, dc) in &planes[j + 1..] {
                let m = Mat3::from_cols(a, b, c).transpose();
                if m.determinant().abs() < 1e-6 {
                    continue;
                }
                let p = m.inverse() * -Vec3::new(da, db, dc);
                if planes.iter().all(|&(n, d)| n.dot(p) + d <= 1e-4) && !out.iter().any(|q| q.distance_squared(p) < 1e-8) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// A shape in the world.
struct Posed {
    hulls: Vec<Vec<(Vec3, f32)>>,
    spheres: Vec<(Vec3, f32)>,
}

/// Where two bodies touch.
#[derive(Debug, Clone, Copy)]
struct Contact {
    /// From the first body into the second.
    normal: Vec3,
    depth: f32,
    point: Vec3,
}

/// How far the sphere `c`, `r` is into `hull`, and the hull's face it is
/// nearest to leaving by (0xb3da0): a sphere is in when it is within `r`
/// of the inside of every plane.
fn in_hull(c: Vec3, r: f32, hull: &[(Vec3, f32)]) -> Option<(Vec3, f32)> {
    let (n, out) = hull.iter().map(|&(n, d)| (n, n.dot(c) + d)).max_by(|a, b| a.1.total_cmp(&b.1))?;
    (out <= r).then_some((n, r - out))
}

/// The deepest contact between `a` and `b`: `a`'s spheres in `b`'s hull,
/// `b`'s in `a`'s, and sphere on sphere (0xb29d0).
fn contact(a: &Posed, b: &Posed) -> Option<Contact> {
    let mut best: Option<Contact> = None;
    let mut keep = |c: Contact| {
        if best.is_none_or(|b| c.depth > b.depth) {
            best = Some(c);
        }
    };
    for &(c, r) in &a.spheres {
        for (n, depth) in b.hulls.iter().filter_map(|h| in_hull(c, r, h)) {
            keep(Contact { normal: -n, depth, point: c - n * r });
        }
        for &(cb, rb) in &b.spheres {
            let off = cb - c;
            let gap = off.length();
            if gap < r + rb {
                let normal = off.normalize_or(Vec3::Y);
                keep(Contact { normal, depth: r + rb - gap, point: c + normal * r });
            }
        }
    }
    for &(c, r) in &b.spheres {
        for (n, depth) in a.hulls.iter().filter_map(|h| in_hull(c, r, h)) {
            keep(Contact { normal: n, depth, point: c - n * r });
        }
    }
    best
}

/// The quads' shape, `Cars/Generic/Generic.hsc` (every quad loads the
/// same one), with how far it reaches from the centre of mass.
#[derive(Resource)]
struct QuadShape {
    shape: Shape,
    reach: f32,
}

fn load_quad_shape(mut commands: Commands, game: Res<Game>) {
    match game.0.read("/Cars/Generic/Generic.hsc").map_err(|e| e.to_string()).and_then(|b| BodyShape::parse(&b).map_err(|e| e.to_string())) {
        Ok(hsc) => {
            let shape = Shape::from_hsc(&hsc);
            let reach = shape.points().map(|(c, r)| (c - Vec3::Y * atv_sim::body::MODEL_DROP).length() + r).fold(0.0, f32::max);
            commands.insert_resource(QuadShape { shape, reach });
        }
        Err(e) => {
            warn!("no quad collision shape, props can't be knocked over: {e}");
            commands.remove_resource::<QuadShape>();
        }
    }
}

/// A prop that can be knocked over.
#[derive(Component, Clone, Debug)]
pub struct Knockable {
    pub shape: Arc<Shape>,
    /// Middle of the shape (model space), taken as its centre of mass.
    centre: Vec3,
    /// How far the shape reaches from `centre`, model space.
    radius: f32,
    /// Moment of inertia per unit mass, model space (taken the same about
    /// every axis).
    inertia: f32,
}

impl Knockable {
    pub fn new(shape: Arc<Shape>) -> Self {
        let (lo, hi) = shape.bounds();
        let centre = (lo + hi) * 0.5;
        let radius = shape.points().map(|(c, r)| c.distance(centre) + r).fold(0.1, f32::max);
        // A solid box's moments, averaged over its three axes.
        let size = (hi - lo).max(Vec3::splat(0.1));
        Self { shape, centre, radius, inertia: size.length_squared() / 18.0 }
    }
}

/// A prop in flight or rolling to a stop.
#[derive(Component, Debug)]
struct Tumble {
    velocity: Vec3,
    spin: Vec3,
    /// Seconds spent slow on the ground.
    still: f32,
}

/// Slower than this (m/s) a quad pushes past without knocking anything.
const MIN_SPEED: f32 = 1.5;
/// 5 mph: slower than this a quad's knocks are quiet (0x94480).
const KNOCK_SPEED: f32 = 2.234_722;
/// The longest a quad moves between contact tests, so a fast one can't
/// pass through a post between ticks.
const SWEEP_STEP: f32 = 0.1;
const GRAVITY: f32 = 9.81;
/// Longest physics step for a tumbling prop, seconds.
const MAX_STEP: f32 = 1.0 / 60.0;
/// A tumbling prop's bounce and grip on the ground.
const BOUNCE: f32 = 0.25;
const FRICTION: f32 = 0.6;
/// A knock-over prop's body (0x2e100 over the defaults at 0xada50): mass
/// 0.05 to a quad's 1 (`MASS` in the `.params`), hardness 0.3 to its 0.2.
const PROP_MASS: f32 = 0.05;
const PROP_HARDNESS: f32 = 0.3;
/// The quad's `HARDNESS` (`Generic.params`).
const QUAD_HARDNESS: f32 = 0.2;

/// The velocity a quad of `mass` closing on a prop at `closing` m/s along
/// `dir` loses. The original's contacts between bodies (made by 0xb29d0
/// and 0xb3da0, solved by 0xb1ca0) bounce them apart with restitution the
/// product of their hardnesses, so the quad gives up `(1 + e) × m_prop /
/// (m_quad + m_prop)` of the closing speed along the contact: about 5%, a
/// little less where the original's off-centre hit sets the prop spinning.
fn slowdown(mass: f32, dir: Vec3, closing: f32) -> Vec3 {
    let e = QUAD_HARDNESS * PROP_HARDNESS;
    -dir * closing.max(0.0) * (1.0 + e) * PROP_MASS / (mass + PROP_MASS)
}

/// What a quad moving at `v` gives a prop it hits along `dir` (unit, quad
/// into prop) at `arm` from the prop's middle: velocity and spin.
fn launch(v: Vec3, dir: Vec3, arm: Vec3) -> (Vec3, Vec3) {
    let speed = v.length();
    let side = dir.with_y(0.0).normalize_or(v.with_y(0.0).normalize_or(Vec3::X));
    // Slower than the quad, so it is thrown up and aside and the quad
    // drives on under it rather than pushing it along.
    let velocity = v * 0.8 + side * (2.0 + speed * 0.15) + Vec3::Y * (3.0 + speed * 0.25);
    // Turned about the hit: a post struck low has its foot knocked out
    // from under it, a panel struck high topples away. Hit through its
    // middle, it topples away from the quad.
    let axis = arm.cross(dir).try_normalize().unwrap_or_else(|| Vec3::Y.cross(velocity.with_y(0.0)).normalize_or_zero());
    (velocity, axis * (3.0 + speed * 0.4))
}

#[allow(clippy::type_complexity)]
fn hit_props(
    mut commands: Commands,
    quad_shape: Option<Res<QuadShape>>,
    mut player: Option<ResMut<Player>>,
    mut opponents: Option<ResMut<crate::opponents::Opponents>>,
    mut wreck: Option<ResMut<crate::crash::Wreck>>,
    mut props: Query<(Entity, &Transform, &Knockable, Option<&mut Tumble>)>,
    mut last: Local<Vec<Vec3>>,
) {
    let (Some(player), Some(quad_shape)) = (player.as_mut(), quad_shape) else { return };
    // Each quad's path since the last tick, swept so a fast quad can't
    // step over a prop.
    let now: Vec<(Vec3, Quat, Vec3)> = std::iter::once(&player.0)
        .chain(opponents.iter().flat_map(|o| o.racers.iter().map(|r| &r.vehicle)))
        .map(|v| (v.position, v.rotation, v.velocity))
        .collect();
    let quads: Vec<(usize, Vec3, Vec3, Quat, Vec3)> = now
        .iter()
        .enumerate()
        .map(|(i, &(at, rot, v))| (i, last.get(i).copied().filter(|p| p.distance(at) < 10.0).unwrap_or(at), at, rot, v))
        .filter(|(.., v)| v.length() > MIN_SPEED)
        .collect();
    *last = now.iter().map(|q| q.0).collect();
    // The quad's shape hangs below its centre of mass as the original's does.
    let pose = |at: Vec3, rot: Quat| Transform::from_translation(at - rot * Vec3::Y * atv_sim::body::MODEL_DROP).with_rotation(rot);
    for (e, t, k, tumble) in &mut props {
        let centre = t.transform_point(k.centre);
        let reach = k.radius * t.scale.abs().max_element() + quad_shape.reach;
        let mut prop = None;
        for &(quad, from, to, rot, v) in &quads {
            if closest_on_segment(from, to, centre).distance(centre) > reach {
                continue;
            }
            let prop = prop.get_or_insert_with(|| k.shape.posed(t));
            let steps = ((to - from).length() / SWEEP_STEP).ceil().clamp(1.0, 16.0) as usize;
            let Some(hit) = (1..=steps).find_map(|s| contact(&quad_shape.shape.posed(&pose(from.lerp(to, s as f32 / steps as f32), rot)), prop))
            else {
                continue;
            };
            let dir = hit.normal;
            let prop_v = tumble.as_ref().map_or(Vec3::ZERO, |t| t.velocity);
            // Only a quad that was closing on the prop hits it.
            let closing = (v - prop_v).dot(dir);
            if closing <= 0.0 {
                continue;
            }
            // A prop already thrown clear isn't hit again by the same pass.
            if prop_v.dot(v) >= v.length_squared() * 0.7 {
                continue;
            }
            let (velocity, spin) = launch(v, dir, hit.point - centre);
            // The prop takes some of the quad's way.
            let quad_v = if quad == 0 { Some(&mut player.0) } else { opponents.as_mut().and_then(|o| o.racers.get_mut(quad - 1)).map(|r| &mut r.vehicle) };
            if let Some(q) = quad_v {
                let dv = slowdown(q.params.mass, dir, closing);
                q.nudge(Vec3::ZERO, dv);
            }
            // The original's props are physics bodies, so hitting one is a
            // knock on the quad like any other (0x1aeb0 sums every contact
            // into the impact 0x94480 sounds): over 5 mph, by how hard.
            if v.length() >= KNOCK_SPEED {
                let hit = v.dot(dir).max(0.0);
                let knock = if quad == 0 {
                    wreck.as_mut().map(|w| &mut w.knock)
                } else {
                    opponents.as_mut().and_then(|o| o.racers.get_mut(quad - 1)).map(|r| &mut r.wreck.knock)
                };
                if let Some(k) = knock {
                    *k = k.max(hit);
                }
            }
            debug!("knocked a prop at {centre:.1} (quad at {:.1} m/s, {:.2} m in)", v.length(), hit.depth);
            match tumble {
                Some(mut t) => {
                    t.velocity = velocity;
                    t.spin = spin;
                    t.still = 0.0;
                }
                None => {
                    commands.entity(e).insert(Tumble { velocity, spin, still: 0.0 });
                }
            }
            break;
        }
    }
}

fn tumble(
    mut commands: Commands,
    time: Res<Time>,
    race: Option<Res<Race>>,
    mut props: Query<(Entity, &mut Transform, &Knockable, &mut Tumble)>,
) {
    let Some(race) = race else { return };
    let ground = |p: Vec3| race.collision.raycast(p + Vec3::Y * 2.0, Vec3::NEG_Y, 8.0).map(|h| (h.point.y, h.normal));
    let dt = time.delta_secs().min(0.25);
    for (e, mut t, k, mut tb) in &mut props {
        let steps = (dt / MAX_STEP).ceil().max(1.0);
        let mut fell = false;
        for _ in 0..steps as usize {
            fell |= !step(&mut t, k, &mut tb, &ground, dt / steps);
        }
        if fell {
            commands.entity(e).despawn();
        } else if tb.still > 0.5 {
            commands.entity(e).remove::<Tumble>();
        }
    }
}

/// One step of a tumbling prop, a rigid body resting on its shape's
/// corners and spheres; false once it has fallen off the world. `ground`
/// gives the height and normal of the ground under a point.
fn step(t: &mut Transform, k: &Knockable, tb: &mut Tumble, ground: &impl Fn(Vec3) -> Option<(f32, Vec3)>, dt: f32) -> bool {
    tb.velocity.y -= GRAVITY * dt;
    let grow = t.scale.abs().max_element();
    let middle = t.transform_point(k.centre) + tb.velocity * dt;
    let turn = tb.spin * dt;
    if turn.length_squared() > 0.0 {
        t.rotation = (Quat::from_scaled_axis(turn) * t.rotation).normalize();
    }
    t.translation = middle - t.rotation * (t.scale * k.centre);
    let inv_inertia = 1.0 / (k.inertia * grow * grow).max(1e-3);
    let mut lift: f32 = 0.0;
    let mut grounded = false;
    for (c, r) in k.shape.points() {
        let p = t.transform_point(c) - Vec3::Y * r * grow;
        let Some((g, n)) = ground(p) else { continue };
        if p.y >= g {
            continue;
        }
        grounded = true;
        lift = lift.max(g - p.y);
        // An impulse at the point that stops it going into the ground,
        // with a little bounce and Coulomb friction along it.
        let arm = p - middle;
        let at = tb.velocity + tb.spin.cross(arm);
        let vn = at.dot(n);
        if vn >= 0.0 {
            continue;
        }
        let k_n = 1.0 + inv_inertia * arm.cross(n).length_squared();
        let jn = -(1.0 + BOUNCE) * vn / k_n;
        let mut j = n * jn;
        let slide = at - n * vn;
        if let Some(along) = slide.try_normalize() {
            let k_t = 1.0 + inv_inertia * arm.cross(along).length_squared();
            j -= along * (slide.length() / k_t).min(FRICTION * jn);
        }
        tb.velocity += j;
        tb.spin += arm.cross(j) * inv_inertia;
    }
    t.translation.y += lift;
    if grounded {
        tb.spin *= 0.3_f32.powf(dt);
        tb.still = if tb.velocity.length() < 0.3 && tb.spin.length() < 0.6 { tb.still + dt } else { 0.0 };
    } else {
        tb.still = 0.0;
        // Into the sea or off a cliff.
        if tb.velocity.y < -30.0 && ground(middle).is_none() {
            return false;
        }
    }
    true
}

/// The point of `a..b` nearest `p`.
fn closest_on_segment(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-6 {
        return b;
    }
    a + ab * ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atv_formats::hsc::{Hull, Plane, Sphere};

    /// A 2 m signpost 0.1 m thick with a 1.2 m panel on top, as retail's
    /// signs are built: one hull round it all, spheres at the corners.
    fn sign() -> Shape {
        let b = |n: [f32; 3], d: f32| Plane { normal: n, d };
        let hsc = BodyShape {
            hulls: vec![Hull {
                planes: vec![
                    b([1.0, 0.0, 0.0], -0.6),
                    b([-1.0, 0.0, 0.0], -0.6),
                    b([0.0, 1.0, 0.0], -2.0),
                    b([0.0, -1.0, 0.0], 0.0),
                    b([0.0, 0.0, 1.0], -0.05),
                    b([0.0, 0.0, -1.0], -0.05),
                ],
            }],
            spheres: vec![Sphere { centre: [0.0, 2.0, 0.0], radius: 0.06 }, Sphere { centre: [0.0, 0.06, 0.0], radius: 0.06 }],
        };
        Shape::from_hsc(&hsc)
    }

    #[test]
    fn hull_corners() {
        let s = sign();
        assert_eq!(s.corners.len(), 8);
        let (lo, hi) = s.bounds();
        assert!(lo.abs_diff_eq(Vec3::new(-0.6, 0.0, -0.06), 1e-5) && hi.abs_diff_eq(Vec3::new(0.6, 2.06, 0.06), 1e-5), "{lo} {hi}");
        // Planes that close nothing in leave no hull.
        let open = Shape::new(vec![vec![(Vec3::Y, 0.0), (Vec3::NEG_Y, -1.0)]], Vec::new());
        assert!(open.is_empty() && open.hulls.is_empty());
    }

    #[test]
    fn prototype_boxes_split_apart() {
        // A post and a panel beside it, listed in one hull as the
        // prototype's signs are: taken whole they hold nothing.
        let b = |n: [f32; 3], d: f32| Plane { normal: n, d };
        let cube = |x0: f32, x1: f32, y0: f32, y1: f32| {
            vec![b([1.0, 0.0, 0.0], -x1), b([-1.0, 0.0, 0.0], x0), b([0.0, 1.0, 0.0], -y1), b([0.0, -1.0, 0.0], y0), b([0.0, 0.0, 1.0], -0.05), b([0.0, 0.0, -1.0], -0.05)]
        };
        let planes = [cube(-0.05, 0.05, 0.0, 2.0), cube(0.1, 1.0, 1.0, 2.0)].concat();
        let s = Shape::from_hsc(&BodyShape { hulls: vec![Hull { planes }], spheres: Vec::new() });
        assert_eq!(s.hulls.len(), 2);
        let p = s.posed(&Transform::default());
        let quad = Posed { hulls: Vec::new(), spheres: vec![(Vec3::new(0.0, 0.5, 0.3), 0.3)] };
        assert!(contact(&quad, &p).is_some());
    }

    #[test]
    fn sphere_in_hull_pushes_out_of_the_nearest_face() {
        let k = sign().posed(&Transform::default());
        // A quad's sphere 0.2 m into the panel's front face.
        let quad = Posed { hulls: Vec::new(), spheres: vec![(Vec3::new(0.3, 1.5, 0.35), 0.5)] };
        let c = contact(&quad, &k).unwrap();
        assert!(c.normal.abs_diff_eq(Vec3::NEG_Z, 1e-5) && (c.depth - 0.2).abs() < 1e-5, "{c:?}");
        // Clear of the panel's edge: no contact.
        let wide = Posed { hulls: Vec::new(), spheres: vec![(Vec3::new(1.2, 1.5, 0.0), 0.5)] };
        assert!(contact(&wide, &k).is_none());
    }

    #[test]
    fn spheres_meet_hull_and_spheres() {
        let quad = sign().posed(&Transform::from_xyz(0.0, 0.0, -1.0));
        // A prop's sphere just inside the other body's hull.
        let prop = Posed { hulls: Vec::new(), spheres: vec![(Vec3::new(0.0, 1.0, -0.9), 0.1)] };
        let c = contact(&quad, &prop).unwrap();
        assert!(c.normal.abs_diff_eq(Vec3::Z, 1e-5) && (c.depth - 0.05).abs() < 1e-5, "{c:?}");
        // Sphere on sphere.
        let a = Posed { hulls: Vec::new(), spheres: vec![(Vec3::ZERO, 0.5)] };
        let b = Posed { hulls: Vec::new(), spheres: vec![(Vec3::new(0.8, 0.0, 0.0), 0.5)] };
        let c = contact(&a, &b).unwrap();
        assert!(c.normal.abs_diff_eq(Vec3::X, 1e-5) && (c.depth - 0.2).abs() < 1e-5);
    }

    #[test]
    fn posed_planes_follow_scale_and_turn() {
        let t = Transform::from_xyz(5.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)).with_scale(Vec3::new(2.0, 1.0, 1.0));
        let p = sign().posed(&t);
        // The panel, twice as wide, now runs along z.
        assert!(in_hull(Vec3::new(5.0, 1.0, 1.1), 0.0, &p.hulls[0]).is_some());
        assert!(in_hull(Vec3::new(5.0, 1.0, 1.3), 0.0, &p.hulls[0]).is_none());
        assert!(in_hull(Vec3::new(5.2, 1.0, 0.0), 0.0, &p.hulls[0]).is_none());
    }

    #[test]
    fn launched_away_and_up() {
        let (v, spin) = launch(Vec3::new(0.0, 0.0, -10.0), Vec3::NEG_Z, Vec3::ZERO);
        assert!(v.z < -8.0 && v.y > 3.0);
        // Hit through the middle it topples forwards: spinning about the
        // quad's sideways axis.
        assert!(spin.x.abs() > 1.0 && spin.y.abs() < 1e-6);
        // Hit low, the top falls back towards the quad.
        let (_, low) = launch(Vec3::new(0.0, 0.0, -10.0), Vec3::NEG_Z, Vec3::NEG_Y);
        let (_, high) = launch(Vec3::new(0.0, 0.0, -10.0), Vec3::NEG_Z, Vec3::Y);
        assert!(low.x * high.x < 0.0);
    }

    #[test]
    fn a_prop_takes_a_little_speed() {
        // Head on at 20 m/s: about 1 m/s lost, all along the contact.
        let dv = slowdown(1.0, Vec3::NEG_Z, 20.0);
        assert!((dv.z - 1.0097).abs() < 1e-3 && dv.x == 0.0 && dv.y == 0.0, "{dv}");
        // Already moving apart: nothing.
        assert_eq!(slowdown(1.0, Vec3::X, -3.0), Vec3::ZERO);
    }

    #[test]
    fn closest_point() {
        let p = Vec3::new(0.0, 0.0, -5.0);
        let at = closest_on_segment(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0), p + Vec3::X * 0.5);
        assert!((at - p).length() < 1e-5);
    }

    #[test]
    fn box_without_a_hull_file() {
        let k = Knockable::new(Arc::new(Shape::cuboid(Vec3::new(-0.25, 0.0, -0.1), Vec3::new(0.25, 1.5, 0.1))));
        assert_eq!(k.shape.corners.len(), 8);
        assert!(k.centre.abs_diff_eq(Vec3::new(0.0, 0.75, 0.0), 1e-6));
    }

    /// With `ATV2_GAME` set: a quad driven past each knock-over prop at a
    /// spread of sideways offsets hits it wherever it overlaps the prop's
    /// model below the quad's top, and nowhere well clear of its shape.
    #[test]
    fn quads_hit_the_real_props() {
        let Some(path) = std::env::var_os("ATV2_GAME") else { return };
        let game = atv_formats::GameSource::open(path).unwrap();
        let read = |p: &str| BodyShape::parse(&game.read(p).unwrap()).map(|h| Shape::from_hsc(&h)).unwrap();
        let quad = read("/Cars/Generic/Generic.hsc");
        assert!(!quad.hulls.is_empty() && !quad.spheres.is_empty());
        for prop in [
            "Generic/Models/GEN_redsign",
            "Generic/Models/GEN_redsignleft",
            "Generic/Models/GEN_yellowsign",
            "Generic/Models/GEN_yellowsignleft",
            "Generic/Models/GEN_Cone",
            "Dunes/Models/DU_deckchair",
            "Dunes/Models/DU_umbrella",
            "Dunes/Models/DU_surfboard",
        ] {
            let model = atv_formats::rom::Model::parse(&game.read(&format!("/Levels/{prop}.rom")).unwrap()).unwrap();
            let shape = read(&format!("/Levels/{prop}_Collision.hsc"));
            let k = Knockable::new(Arc::new(shape));
            let posed = k.shape.posed(&Transform::default());
            // The model's width across the quad's path (x) up to the
            // quad's height (an umbrella's canopy is over it).
            let (lo, hi) = model
                .nodes
                .iter()
                .flat_map(|n| &n.vertices)
                .filter(|v| v.pos[1] < 0.9)
                .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v.pos[0]), hi.max(v.pos[0])));
            let mut row = String::new();
            let mut missed = Vec::new();
            for i in -20..=20 {
                let x = i as f32 * 0.1;
                // Level, driving down -z past the prop, swept as hit_props does.
                let hit = (0..=60).any(|s| {
                    let z = 3.0 - s as f32 * 0.1;
                    contact(&quad.posed(&Transform::from_xyz(x, 0.0, z)), &posed).is_some()
                });
                row.push(if hit { '#' } else { '.' });
                // The quad (0.55 m each side of its middle) overlaps the
                // prop's model by 0.2 m or more.
                if x + 0.35 > lo && x - 0.35 < hi && !hit {
                    missed.push(x);
                }
                // Never clear of the shape itself (retail's sign hulls
                // widen under their panels).
                let (slo, shi) = k.shape.bounds();
                assert!(!hit || (x + 0.75 > slo.x && x - 0.75 < shi.x), "{prop}: hit at {x:.1} clear of {slo}..{shi}");
            }
            println!("{prop:36} {lo:5.2}..{hi:5.2} {row}");
            assert!(missed.is_empty(), "{prop}: drove through at {missed:?} ({lo:.2}..{hi:.2})");
        }
    }

    #[test]
    fn knocked_sign_falls_flat_and_settles() {
        let k = Knockable::new(Arc::new(sign()));
        let flat = |_: Vec3| Some((0.0, Vec3::Y));
        let mut t = Transform::default();
        // Knocked forwards from upright.
        let (velocity, spin) = launch(Vec3::new(0.0, 0.0, -12.0), Vec3::NEG_Z, Vec3::ZERO);
        let mut tb = Tumble { velocity, spin, still: 0.0 };
        let mut settled = None;
        for i in 0..1200 {
            assert!(step(&mut t, &k, &mut tb, &flat, 1.0 / 120.0));
            if tb.still > 0.5 {
                settled = Some(i);
                break;
            }
        }
        assert!(settled.is_some(), "still moving: {tb:?} at {t:?}");
        // Lying on its face or back, flat on the ground, not sunk into it.
        let up = t.rotation * Vec3::Y;
        assert!(up.y.abs() < 0.2, "{up}");
        let low = k.shape.points().map(|(c, r)| t.transform_point(c).y - r).fold(f32::MAX, f32::min);
        assert!(low > -0.05 && low < 0.05, "{low}");
        assert!(t.translation.z < -3.0, "{}", t.translation);
    }
}
