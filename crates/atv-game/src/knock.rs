//! Knock-over props: the instances a track flags `FREE` (corner signs,
//! deckchairs, umbrellas, surfboards, Stadium 2's cones). A quad that
//! drives into one sends it flying; it tumbles under gravity, bounces off
//! the track and settles where it lands. The original's `.hsc` hulls aren't
//! needed for this: each prop is an upright cylinder from its model's
//! bounding box, which is what a quad brushing past it sees.

use bevy::prelude::*;

use atv_sim::TrackCollision;

use crate::{AppState, Player, Race};

pub struct KnockPlugin;

impl Plugin for KnockPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (hit_props, tumble).chain().run_if(in_state(AppState::Race)));
    }
}

/// A prop that can be knocked over, with its upright size (world units).
#[derive(Component, Clone, Copy, Debug)]
pub struct Knockable {
    /// Bounding box centre over the prop's origin, in its model space.
    pub centre: Vec3,
    pub radius: f32,
    pub height: f32,
}

impl Knockable {
    /// The cylinder of a model whose box is `min..max`, placed with `t`.
    pub fn from_box(min: Vec3, max: Vec3, t: &Transform) -> Self {
        let half = (max - min) * 0.5 * t.scale.abs();
        Self { centre: (min + max) * 0.5, radius: half.x.max(half.z).max(0.1), height: (half.y * 2.0).max(0.2) }
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

/// Half a quad's width plus a little: how close its centre gets to a prop.
const QUAD_RADIUS: f32 = 0.8;
/// Slower than this (m/s) a quad pushes past without knocking anything.
const MIN_SPEED: f32 = 1.5;
const GRAVITY: f32 = 9.81;
/// Longest physics step for a tumbling prop, seconds.
const MAX_STEP: f32 = 1.0 / 60.0;

/// What a quad moving at `v` past a prop at `dir` (unit, horizontal,
/// quad to prop) gives it: velocity and spin.
fn launch(v: Vec3, dir: Vec3) -> (Vec3, Vec3) {
    let speed = v.length();
    // Slower than the quad, so it is thrown up and aside and the quad
    // drives on under it rather than pushing it along.
    let velocity = v * 0.8 + dir * (2.0 + speed * 0.15) + Vec3::Y * (3.0 + speed * 0.25);
    // Topples away from the quad, spinning about the axis across its path.
    let axis = Vec3::Y.cross(velocity.with_y(0.0)).normalize_or_zero();
    (velocity, axis * (3.0 + speed * 0.4))
}

fn hit_props(
    mut commands: Commands,
    player: Option<Res<Player>>,
    opponents: Option<Res<crate::opponents::Opponents>>,
    mut props: Query<(Entity, &Transform, &Knockable, Option<&mut Tumble>)>,
    mut last: Local<Vec<Vec3>>,
) {
    let Some(player) = player else { return };
    // Each quad's path since last frame, so a fast quad at a low frame
    // rate can't step over a prop.
    let now: Vec<(Vec3, Vec3)> = std::iter::once(&player.0)
        .chain(opponents.iter().flat_map(|o| o.racers.iter().map(|r| &r.vehicle)))
        .map(|v| (v.position, v.velocity))
        .collect();
    let quads: Vec<(Vec3, Vec3, Vec3)> = now
        .iter()
        .enumerate()
        .map(|(i, &(at, v))| (last.get(i).copied().filter(|p| p.distance(at) < 10.0).unwrap_or(at), at, v))
        .filter(|(.., v)| v.length() > MIN_SPEED)
        .collect();
    *last = now.iter().map(|q| q.0).collect();
    for (e, t, k, tumble) in &mut props {
        let centre = t.transform_point(k.centre);
        let base = t.translation.y.min(centre.y - k.height * 0.5);
        for &(from, to, v) in &quads {
            let at = closest_on_segment(from, to, centre);
            let off = (centre - at).with_y(0.0);
            let reach = k.radius + QUAD_RADIUS;
            if off.length() > reach || at.y < base - 1.0 || at.y > base + k.height + 1.0 {
                continue;
            }
            // Only a quad that was closing on the prop hits it.
            if v.dot(centre - from) <= 0.0 {
                continue;
            }
            // A prop already thrown clear isn't hit again by the same pass.
            if tumble.as_ref().is_some_and(|t| t.velocity.dot(v) >= v.length_squared() * 0.7) {
                continue;
            }
            let dir = off.normalize_or(v.with_y(0.0).normalize_or(Vec3::X));
            let (velocity, spin) = launch(v, dir);
            debug!("knocked a prop at {centre:.1} (quad at {:.1} m/s)", v.length());
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
    let dt = time.delta_secs().min(0.25);
    for (e, mut t, k, mut tb) in &mut props {
        // Short steps, so a slow frame doesn't drop a prop through a slope.
        let steps = (dt / MAX_STEP).ceil().max(1.0);
        let mut fell = false;
        for _ in 0..steps as usize {
            fell |= !step(&mut t, k, &mut tb, &race.collision, dt / steps);
        }
        if fell {
            commands.entity(e).despawn();
        } else if tb.still > 0.5 {
            commands.entity(e).remove::<Tumble>();
        }
    }
}

/// One step of a tumbling prop; false once it has fallen off the world.
fn step(t: &mut Transform, k: &Knockable, tb: &mut Tumble, world: &TrackCollision, dt: f32) -> bool {
    tb.velocity.y -= GRAVITY * dt;
    t.translation += tb.velocity * dt;
    let turn = tb.spin * dt;
    if turn.length_squared() > 0.0 {
        t.rotation = (Quat::from_scaled_axis(turn) * t.rotation).normalize();
    }
    // Lying down, a prop's origin sits about half its width up.
    let lying = 1.0 - (t.rotation * Vec3::Y).y.abs();
    let lift = k.radius.min(k.height * 0.5) * 0.5 * lying;
    match world.raycast(t.translation + Vec3::Y * 2.0, Vec3::NEG_Y, 8.0).map(|h| h.point.y + lift) {
        Some(g) if t.translation.y < g => {
            t.translation.y = g;
            if tb.velocity.y < 0.0 {
                tb.velocity.y *= -0.25;
            }
            let rub = 0.6_f32.powf(dt * 10.0);
            tb.velocity.x *= rub;
            tb.velocity.z *= rub;
            tb.spin *= 0.5_f32.powf(dt * 10.0);
            tb.still = if tb.velocity.length() < 0.4 { tb.still + dt } else { 0.0 };
        }
        // Into the sea or off a cliff.
        None if tb.velocity.y < -30.0 => return false,
        _ => tb.still = 0.0,
    }
    true
}

/// The point of `a..b` nearest `p`, measured across the ground.
fn closest_on_segment(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = (b - a).with_y(0.0);
    let len2 = ab.length_squared();
    if len2 < 1e-6 {
        return b;
    }
    a + (b - a) * ((p - a).with_y(0.0).dot(ab) / len2).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launched_away_and_up() {
        let (v, spin) = launch(Vec3::new(0.0, 0.0, -10.0), Vec3::NEG_Z);
        assert!(v.z < -8.0 && v.y > 3.0);
        // Topples forwards: spinning about the quad's sideways axis.
        assert!(spin.x.abs() > 1.0 && spin.y.abs() < 1e-6);
    }

    #[test]
    fn fast_quad_cannot_step_over() {
        let p = Vec3::new(0.0, 0.0, -5.0);
        let at = closest_on_segment(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0), p + Vec3::X * 0.5);
        assert!((at - p).length() < 1e-5);
    }

    #[test]
    fn cylinder_from_box() {
        let t = Transform::from_scale(Vec3::new(2.0, 1.0, 2.0));
        let k = Knockable::from_box(Vec3::new(-0.25, 0.0, -0.1), Vec3::new(0.25, 1.5, 0.1), &t);
        assert!((k.radius - 0.5).abs() < 1e-6 && (k.height - 1.5).abs() < 1e-6);
        assert_eq!(k.centre, Vec3::new(0.0, 0.75, 0.0));
    }
}
