//! Builds simulation data (collision, start positions) from a parsed track.

use glam::{Mat3, Quat, Vec3};

use std::collections::HashMap;

use atv_formats::ink::CollisionMesh;
use atv_formats::rom::Locator;
use atv_formats::row::{Instance, World};
use atv_formats::GameSource;

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

/// The track's own collision: its meshes and road patches. Props are
/// added by [`collision`].
pub fn collision_from_world(world: &World) -> TrackCollision {
    TrackCollision::new(world_triangles(world), 8.0)
}

/// The track's collision including its static props: each placed prop
/// whose model has an `.ink` collision mesh (rocks, buildings, trees, tyre
/// stacks, the stadium gates) adds that mesh where the prop stands.
pub fn collision(world: &World, game: &GameSource, track: &str) -> TrackCollision {
    let mut tris = world_triangles(world);
    tris.extend(prop_triangles(world, game, track));
    TrackCollision::new(tris, 8.0)
}

/// Where a prop's model-space point (game frame) ends up in the sim frame,
/// placed the same way the renderer places the model.
pub fn instance_point(inst: &Instance, p: [f32; 3]) -> Vec3 {
    let r = &inst.rotation;
    let m = Mat3::from_cols(Vec3::new(r[0], r[1], -r[2]), Vec3::new(r[3], r[4], -r[5]), Vec3::new(-r[6], -r[7], r[8]));
    let x = m.x_axis.normalize_or(Vec3::X);
    let z = m.z_axis.normalize_or(Vec3::Z);
    let y = z.cross(x).normalize_or(Vec3::Y);
    let rot = Mat3::from_cols(x, y, x.cross(y));
    rot * (to_sim(p) * Vec3::from(inst.scale)) + to_sim(inst.pos)
}

/// Collision triangles of the track's props, in the sim frame.
pub fn prop_triangles(world: &World, game: &GameSource, track: &str) -> Vec<[Vec3; 3]> {
    // `.ink` files by lower-case stem; the track's own environment's
    // folder wins over Generic and the others.
    let env = track.trim_end_matches(|c: char| c.is_ascii_digit()).to_ascii_lowercase();
    let mut paths: HashMap<String, Vec<String>> = HashMap::new();
    for p in game.archive_paths() {
        let lower = p.to_ascii_lowercase();
        if let Some(stem) = lower.strip_suffix(".ink") {
            let stem = stem.rsplit(['/', '\\']).next().unwrap_or(stem).to_string();
            paths.entry(stem).or_default().push(p.to_string());
        }
    }
    let mut meshes: HashMap<String, Option<CollisionMesh>> = HashMap::new();
    let mut tris = Vec::new();
    for inst in &world.instances {
        let stem = inst.model.to_ascii_lowercase();
        let stem = stem.trim_end_matches(".clm").to_string();
        let mesh = meshes.entry(stem.clone()).or_insert_with(|| {
            let list = paths.get(&stem)?;
            let pick = |dir: &str| list.iter().find(|p| p.to_ascii_lowercase().contains(&format!("/{dir}/")));
            let path = pick(&env).or_else(|| pick("generic")).or(list.first())?;
            CollisionMesh::parse(&game.read(path).ok()?).ok()
        });
        if let Some(mesh) = mesh {
            tris.extend(mesh.triangles.iter().map(|t| t.map(|p| instance_point(inst, p))));
        }
    }
    tris
}

fn world_triangles(world: &World) -> Vec<[Vec3; 3]> {
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
            let t = p.tessellate(PATCH_STEPS);
            for tri in t.indices.chunks_exact(3) {
                tris.push([0, 1, 2].map(|k| to_sim(t.pos[tri[k] as usize])));
            }
        }
    }
    tris
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

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(rotation: [f32; 9], scale: [f32; 3], pos: [f32; 3]) -> Instance {
        Instance { flags: String::new(), model: "x.clm".into(), pos, scale, rotation, tint: 0 }
    }

    #[test]
    fn props_are_placed_like_the_renderer() {
        let id = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        // Model point (1, 2, 3) on a prop at (10, 0, 20): the sim negates Z.
        let p = instance_point(&inst(id, [1.0; 3], [10.0, 0.0, 20.0]), [1.0, 2.0, 3.0]);
        assert!((p - Vec3::new(11.0, 2.0, -23.0)).length() < 1e-5, "{p}");
        // Scale applies along the prop's own axes before it turns.
        let p = instance_point(&inst(id, [2.0, 3.0, 1.0], [0.0; 3]), [1.0, 1.0, 0.0]);
        assert!((p - Vec3::new(2.0, 3.0, 0.0)).length() < 1e-5, "{p}");
        // A quarter turn about Y (game rows x = (0,0,-1), z = (1,0,0)): the
        // model's +X ends up along game -Z, which is sim +Z.
        let quarter = [0.0, 0.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0];
        let p = instance_point(&inst(quarter, [1.0; 3], [0.0; 3]), [1.0, 0.0, 0.0]);
        assert!((p - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-5, "{p}");
    }
}
