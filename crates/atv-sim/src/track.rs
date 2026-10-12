//! Builds simulation data (collision, start positions) from a parsed track.

use glam::{Mat3, Quat, Vec3};

use std::collections::HashMap;

use atv_formats::ink::CollisionMesh;
use atv_formats::kdt::CollisionTree;
use atv_formats::rom::Locator;
use atv_formats::row::{Instance, World};
use atv_formats::GameSource;

use crate::collision::TrackCollision;
use crate::surface::{self, Surfaces};

/// Road patches are tessellated this finely for collision.
const PATCH_STEPS: usize = 8;

/// A render triangle is "in the original's collision" when the `.kdt` comes
/// within this distance (m) of its centre.
const KDT_NEAR: f32 = 1.0;

/// A material with fewer of its triangles than this in the original's
/// collision is scenery the original never collides with; only those few
/// stay solid.
const KDT_SCENERY: f32 = 0.05;

/// A render triangle fills in for the `.kdt` only where the `.kdt` has
/// nothing this far (m) above or below it.
const OPEN_REACH: f32 = 50.0;
/// Spacing (m) of the points that test a render triangle for that.
const OPEN_SPACING: f32 = 1.0;

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
    TrackCollision::new(world_triangles(world).into_iter().map(|t| t.0).collect(), 8.0)
}

/// The track's collision including its static props: each placed prop
/// whose model has an `.ink` collision mesh (rocks, buildings, trees, tyre
/// stacks, the stadium gates) adds that mesh where the prop stands.
///
/// The ground is the original's own collision (`.kdt`), as the original
/// drives on (0x5d030), with its surfaces; props keep full grip and no
/// drag. The render meshes follow it closely but not everywhere: Industrial
/// 2 and 3 have a 2-3 m trench across the track that the `.kdt` bridges,
/// and Glacier 1 and Forest 2 have spots metres apart. A track without a
/// `.kdt` uses its render meshes.
///
/// Where the `.kdt` has no ground at all (river beds under deep water) the
/// render meshes fill in, less the materials the `.kdt` all but leaves out:
/// scenery the original lets quads pass through (Stadium 1's roof cables,
/// neon strips, hanging arrows and blimp, Swamp's plane wreck, Dunes'
/// yachts).
pub fn collision(world: &World, game: &GameSource, track: &str) -> TrackCollision {
    let render = world_triangles(world);
    let (mut tris, mut ids) = match kdt_triangles(game, track) {
        Some((mut tris, mut ids)) => {
            // Under deep water the `.kdt` has no floor at all (the original
            // takes the quad out of the water first); the render meshes'
            // river beds stand in there, nowhere the `.kdt` covers.
            let kdt = TrackCollision::new(tris.clone(), 8.0);
            // A hole can be smaller than the render triangle over it, so the
            // test runs on a grid across the whole triangle.
            let open = |t: &[Vec3; 3]| {
                let c = (t[0] + t[1] + t[2]) / 3.0;
                let edge = (t[0] - t[1]).length().max((t[1] - t[2]).length()).max((t[2] - t[0]).length());
                let n = ((edge / OPEN_SPACING).ceil() as u32).clamp(1, 400);
                (0..=n).any(|i| {
                    (0..=n - i).any(|j| {
                        let k = n - i - j;
                        let p = (t[0] * i as f32 + t[1] * j as f32 + t[2] * k as f32) / n as f32;
                        let p = p.lerp(c, 0.01);
                        kdt.raycast(p + Vec3::Y * OPEN_REACH, Vec3::NEG_Y, 2.0 * OPEN_REACH).is_none()
                    })
                })
            };
            tris.extend(without_scenery(render, &kdt).into_iter().map(|t| t.0).filter(open));
            ids.resize(tris.len(), surface::UNKNOWN);
            (tris, ids)
        }
        None => {
            let tris: Vec<_> = render.into_iter().map(|t| t.0).collect();
            let ids = vec![surface::UNKNOWN; tris.len()];
            (tris, ids)
        }
    };
    tris.extend(prop_triangles(world, game, track));
    ids.resize(tris.len(), surface::UNKNOWN);
    let surfaces = game
        .read("/Setup/Materials.xml")
        .map(|b| Surfaces::parse(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    let mut c = TrackCollision::new(tris, 8.0);
    c.set_surfaces(ids, surfaces);
    c
}

/// The original's own collision triangles (`.kdt`) with their surface ids,
/// in the sim frame.
pub fn kdt_collision(game: &GameSource, track: &str) -> Option<TrackCollision> {
    let (tris, ids) = kdt_triangles(game, track)?;
    Some(TrackCollision::with_surfaces(tris, ids, 8.0))
}

fn kdt_triangles(game: &GameSource, track: &str) -> Option<(Vec<[Vec3; 3]>, Vec<u8>)> {
    let tree = CollisionTree::parse(&game.read(&format!("/Levels/{track}/Kdt/{track}.kdt")).ok()?).ok()?;
    Some(tree.triangles.iter().map(|t| (t.corners.map(to_sim), t.surface)).unzip())
}

/// The track's water surfaces (meshes with a water material; the 0x2000
/// flag also marks shadow decals), level parts only, for finding how deep
/// a quad is wading.
pub fn water_surface(world: &World) -> TrackCollision {
    let mut tris = Vec::new();
    for m in &world.meshes {
        let named = usize::try_from(m.material)
            .ok()
            .and_then(|i| world.materials.get(i))
            .is_some_and(|n| n.to_ascii_lowercase().contains("water"));
        if !named {
            continue;
        }
        for t in m.indices.as_chunks::<3>().0 {
            tris.push([0, 1, 2].map(|k| to_sim(m.vertices[t[k] as usize].pos)));
        }
        for p in &m.patches {
            let t = p.tessellate(PATCH_STEPS);
            for tri in t.indices.as_chunks::<3>().0 {
                tris.push([0, 1, 2].map(|k| to_sim(t.pos[tri[k] as usize])));
            }
        }
    }
    // Falls and spray sheets stand up; only the level surface is water to wade in.
    tris.retain(|[a, b, c]| (*b - *a).cross(*c - *a).normalize_or_zero().y.abs() > 0.7);
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

/// Drops the triangles of materials the original's collision leaves out
/// (see [`collision`]), keeping the ones it covers.
fn without_scenery(tris: Vec<([Vec3; 3], i32)>, kdt: &TrackCollision) -> Vec<([Vec3; 3], i32)> {
    let near: Vec<bool> = tris.iter().map(|([a, b, c], _)| kdt.sphere((*a + *b + *c) / 3.0, KDT_NEAR).is_some()).collect();
    let mut counts: HashMap<i32, (usize, usize)> = HashMap::new();
    for ((_, mat), &n) in tris.iter().zip(&near) {
        let c = counts.entry(*mat).or_default();
        c.0 += 1;
        c.1 += usize::from(n);
    }
    let scenery = |mat: i32| counts.get(&mat).is_some_and(|&(all, covered)| (covered as f32) < KDT_SCENERY * all as f32);
    tris.into_iter().zip(near).filter(|((_, mat), n)| *n || !scenery(*mat)).map(|(t, _)| t).collect()
}

/// The track's solid render triangles, each with its material index.
fn world_triangles(world: &World) -> Vec<([Vec3; 3], i32)> {
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
            tris.push(([0, 1, 2].map(|k| to_sim(m.vertices[t[k] as usize].pos)), m.material));
        }
        for p in &m.patches {
            let t = p.tessellate(PATCH_STEPS);
            for tri in t.indices.chunks_exact(3) {
                tris.push(([0, 1, 2].map(|k| to_sim(t.pos[tri[k] as usize])), m.material));
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

/// A lap checkpoint's number: `Checkpoint N` (any case, words may follow),
/// or 1 for the start line, `Startpoint 1`. The original reads both the
/// same way, the number after the first ten letters (0x65c50).
pub fn checkpoint_number(name: &str) -> Option<u32> {
    let name = name.trim().to_ascii_lowercase();
    let rest = name.strip_prefix("checkpoint ").or_else(|| name.strip_prefix("startpoint "))?;
    rest.split_whitespace().next()?.parse().ok()
}

/// Checkpoint gate centres in lap order: the start/finish line
/// ("Startpoint 1"), then "Checkpoint 2" .. "Checkpoint N".
pub fn checkpoints(world: &World) -> Vec<Vec3> {
    let mut cps: Vec<(u32, Vec3)> = world.locators.iter().filter_map(|l| Some((checkpoint_number(&l.name)?, to_sim(l.pos)))).collect();
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

    #[test]
    fn scenery_the_original_leaves_out_is_not_solid() {
        let floor = |x: f32| [Vec3::new(x, 0.0, 0.0), Vec3::new(x + 1.0, 0.0, 0.0), Vec3::new(x, 0.0, 1.0)];
        let kdt = TrackCollision::new((0..40).map(|i| floor(i as f32)).collect(), 8.0);
        let up = |t: [Vec3; 3], y: f32| t.map(|p| p + Vec3::Y * y);
        let mut tris: Vec<_> = (0..40).map(|i| (floor(i as f32), 0)).collect();
        // A cable: one triangle at its anchor on the floor, the rest high up.
        tris.push((floor(0.0), 1));
        tris.extend((1..40).map(|i| (up(floor(i as f32), 10.0), 1)));
        // A roof the original covers well, with a few overhanging triangles.
        tris.extend((0..30).map(|i| (floor(i as f32), 2)));
        tris.extend((0..2).map(|i| (up(floor(i as f32), 10.0), 2)));
        let kept = without_scenery(tris, &kdt);
        let count = |mat: i32| kept.iter().filter(|t| t.1 == mat).count();
        assert_eq!((count(0), count(1), count(2)), (40, 1, 32));
    }
}
