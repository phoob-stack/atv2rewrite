//! Builds simulation data (collision, start positions) from a parsed track.

use glam::{Mat3, Quat, Vec3};

use atv_formats::rom::Locator;
use atv_formats::row::World;

use crate::collision::TrackCollision;

/// Road patches are tessellated this finely for collision.
const PATCH_STEPS: usize = 8;

/// Converts a left-handed game position to the simulation's frame.
pub fn to_sim(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], -p[2])
}

/// Rotation of a game 3x3 matrix (rows = basis vectors) in the sim frame.
pub fn rotation_to_sim(rot: &[f32; 9]) -> Quat {
    let x = Vec3::new(rot[0], rot[1], -rot[2]).normalize_or(Vec3::X);
    let z = Vec3::new(-rot[6], -rot[7], rot[8]).normalize_or(Vec3::Z);
    let y = z.cross(x).normalize_or(Vec3::Y);
    let z = x.cross(y);
    Quat::from_mat3(&Mat3::from_cols(x, y, z))
}

/// Whether a material is solid ground (not water, decals, sky or light rays).
pub fn is_solid_material(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !["water", "_shad", "spume", "wake", "rays", "sky", "mist", "distant"].iter().any(|k| n.contains(k))
}

pub fn collision_from_world(world: &World) -> TrackCollision {
    let mut tris = Vec::new();
    for m in &world.meshes {
        let solid = usize::try_from(m.material)
            .ok()
            .and_then(|i| world.materials.get(i))
            .is_none_or(|n| is_solid_material(n));
        if !solid {
            continue;
        }
        for t in m.indices.chunks_exact(3) {
            tris.push([0, 1, 2].map(|k| to_sim(m.vertices[t[k] as usize].pos)));
        }
        for p in &m.patches {
            let (pos, _, idx) = p.tessellate(PATCH_STEPS);
            for t in idx.chunks_exact(3) {
                tris.push([0, 1, 2].map(|k| to_sim(pos[t[k] as usize])));
            }
        }
    }
    TrackCollision::new(tris, 8.0)
}

/// Grid slot `n` (1-based): the `StartN` locator box, or the first
/// checkpoint if the track has no grid.
pub fn start_locator(world: &World, n: usize) -> Option<&Locator> {
    let want = format!("start{n}");
    world
        .locators
        .iter()
        .find(|l| l.name.trim().eq_ignore_ascii_case(&want))
        .or_else(|| world.locators.iter().find(|l| l.name.to_ascii_lowercase().starts_with("checkpoint")))
}

/// Spawn pose for grid slot `n`: on the ground under the slot box, facing
/// along the box's forward axis.
pub fn start_pose(world: &World, collision: &TrackCollision, n: usize) -> (Vec3, Quat) {
    let Some(l) = start_locator(world, n) else { return (Vec3::new(0.0, 10.0, 0.0), Quat::IDENTITY) };
    let pos = to_sim(l.pos);
    let rot = rotation_to_sim(&l.rotation);
    let ground = collision.raycast(pos + Vec3::Y * 3.0, Vec3::NEG_Y, 20.0).map_or(pos.y, |h| h.point.y);
    (pos.with_y(ground + 0.6), rot)
}

/// Checkpoint gate centres in lap order ("Checkpoint 2" .. "Checkpoint N").
pub fn checkpoints(world: &World) -> Vec<Vec3> {
    let mut cps: Vec<(u32, Vec3)> = world
        .locators
        .iter()
        .filter_map(|l| {
            let name = l.name.trim().to_ascii_lowercase();
            let rest = name.strip_prefix("checkpoint ")?;
            let n: u32 = rest.split_whitespace().next()?.parse().ok()?;
            Some((n, to_sim(l.pos)))
        })
        .collect();
    cps.sort_by_key(|c| c.0);
    cps.dedup_by_key(|c| c.0);
    cps.into_iter().map(|c| c.1).collect()
}

/// Very simple driver used for headless tests: aims at `target`, eases
/// off for sharp turns.
pub fn steer_towards(v: &crate::Vehicle, target: Vec3) -> crate::Input {
    let to = (target - v.position).with_y(0.0).normalize_or_zero();
    let fwd = v.forward().with_y(0.0).normalize_or_zero();
    let angle = fwd.cross(to).y.atan2(fwd.dot(to));
    crate::Input {
        throttle: if angle.abs() > 1.0 { 0.4 } else { 1.0 },
        steer: (-angle * 2.0).clamp(-1.0, 1.0),
        ..Default::default()
    }
}
