//! Rivers, ponds and the sea: how deep a quad is wading. The track's water
//! meshes aren't solid (the quad drives on the bed under them), so their
//! level surfaces are kept apart to measure against. Quads wading through
//! throw up spray. Deep water is the original's own: the track's
//! `waterbox` locators.

use bevy::prelude::*;

use atv_formats::rom::Locator;
use atv_formats::row::World;
use atv_sim::track::to_sim;
use atv_sim::TrackCollision;

use crate::{AppState, Player};

pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Race), load_spray)
            .add_systems(Update, (throw_spray, fly_spray).chain().run_if(in_state(AppState::Race)));
    }
}

/// The mesh and material every droplet shares.
#[derive(Resource)]
struct SprayLook {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// One droplet of spray: velocity and seconds left.
#[derive(Component)]
struct Droplet {
    velocity: Vec3,
    life: f32,
}

/// Seconds a droplet lives, and the most there can be at once.
const DROPLET_LIFE: f32 = 0.7;
const MAX_DROPLETS: usize = 400;

/// The track's water surfaces, and its water boxes.
#[derive(Resource)]
pub struct Water(pub TrackCollision, pub Vec<WaterBox>);

/// One of the original's water volumes (built at 0x101030 from a
/// `waterbox` locator): an upright box whose top is the water's surface.
/// Anything over its footprint, however far down, is measured against it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterBox {
    centre: Vec3,
    /// The box's sideways and lengthways axes, and half its size along each.
    axes: [Vec3; 2],
    half: [f32; 2],
    top: f32,
}

impl WaterBox {
    pub fn from_locator(l: &Locator) -> Self {
        let r = &l.rotation;
        WaterBox {
            centre: to_sim(l.pos),
            axes: [to_sim([r[0], r[1], r[2]]), to_sim([r[6], r[7], r[8]])],
            half: [l.scale[0] * 0.5, l.scale[2] * 0.5],
            top: l.pos[1] + l.scale[1] * 0.5,
        }
    }

    fn covers(&self, at: Vec3) -> bool {
        let d = at - self.centre;
        (0..2).all(|i| self.axes[i].dot(d).abs() < self.half[i])
    }
}

/// The track's water boxes.
pub fn boxes(world: &World) -> Vec<WaterBox> {
    world.locators.iter().filter(|l| l.name.to_ascii_lowercase().starts_with("waterbox")).map(WaterBox::from_locator).collect()
}

/// Height of the quad's centre of mass over the bottoms of its wheels.
const RIDE_HEIGHT: f32 = 0.5;
/// Deeper than this (metres) the wheels throw up deep-water sounds.
pub const DEEP: f32 = 0.4;
/// A quad or a thrown rider whose centre is this far under the surface is
/// in the water (the original's water sensors, 0xd7f60, set up with -1 m).
pub const UNDER: f32 = 1.0;

impl Water {
    /// How far under the surface a quad centred at `at` has its wheels
    /// (0 when it's dry).
    pub fn depth(&self, at: Vec3) -> f32 {
        let bottom = at.y - RIDE_HEIGHT;
        self.0
            .raycast(at + Vec3::Y * 3.0, Vec3::NEG_Y, 6.0)
            .map_or(0.0, |h| (h.point.y - bottom).clamp(0.0, 3.0))
    }

    /// The height of the deep water's surface over or under `at`: the
    /// highest top of the water boxes over it (0xd7f60 counts a quad in the
    /// water if it's under any of them).
    pub fn surface(&self, at: Vec3) -> Option<f32> {
        self.1.iter().filter(|b| b.covers(at)).map(|b| b.top).reduce(f32::max)
    }

    /// The height of the water you can see over or under `at`, if there's
    /// any; else the deep water's [`Water::surface`]. (The boxes are placed
    /// by hand, and their tops can sit metres off the water's mesh.)
    pub fn level(&self, at: Vec3) -> Option<f32> {
        self.0.raycast(at + Vec3::Y * 20.0, Vec3::NEG_Y, 60.0).map(|h| h.point.y).or_else(|| self.surface(at))
    }
}

/// [`Water::depth`] when the track has been loaded.
pub fn depth(water: Option<&Water>, at: Vec3) -> f32 {
    water.map_or(0.0, |w| w.depth(at))
}

fn load_spray(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(SprayLook {
        mesh: meshes.add(Sphere::new(0.06).mesh().ico(1).unwrap_or_else(|_| Sphere::new(0.06).mesh().uv(6, 4))),
        material: materials.add(StandardMaterial {
            base_color: Color::srgba(0.85, 0.9, 0.95, 0.55),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        }),
    });
}

/// Spray off every quad wading at speed: a few droplets a frame, more the
/// faster it goes, thrown up and out behind its wheels.
#[allow(clippy::too_many_arguments)]
fn throw_spray(
    mut commands: Commands,
    time: Res<Time>,
    water: Option<Res<Water>>,
    look: Option<Res<SprayLook>>,
    player: Option<Res<Player>>,
    opponents: Option<Res<crate::opponents::Opponents>>,
    droplets: Query<(), With<Droplet>>,
    mut seed: Local<u32>,
) {
    let (Some(water), Some(look), Some(player)) = (water, look, player) else { return };
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let mut count = droplets.iter().count();
    let mut rand = || {
        *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        ((*seed >> 16) & 0x7fff) as f32 / 32767.0
    };
    let quads = std::iter::once(&player.0).chain(opponents.iter().flat_map(|o| o.racers.iter().map(|r| &r.vehicle)));
    for v in quads {
        let speed = v.velocity.with_y(0.0).length();
        let depth = water.depth(v.position);
        if depth <= 0.05 || speed < 2.0 {
            continue;
        }
        let surface = v.position.y - RIDE_HEIGHT + depth;
        let back = -v.velocity.with_y(0.0) / speed;
        let side = Vec3::Y.cross(back);
        let n = ((speed * 2.5 * dt * 10.0) as usize).clamp(1, 12);
        for _ in 0..n {
            if count >= MAX_DROPLETS {
                return;
            }
            count += 1;
            // Off either side of the quad, up and back.
            let s = if rand() < 0.5 { -1.0 } else { 1.0 };
            let at = Vec3::new(v.position.x, surface, v.position.z) + side * s * (0.5 + rand() * 0.3) + back * (rand() * 0.8 - 0.2);
            let velocity = side * s * (0.5 + rand() * 1.5) + back * speed * (0.1 + rand() * 0.2) + Vec3::Y * (1.5 + rand() * 2.0 + speed * 0.08);
            commands.spawn((
                Mesh3d(look.mesh.clone()),
                MeshMaterial3d(look.material.clone()),
                Transform::from_translation(at).with_scale(Vec3::splat(0.6 + rand() * 0.8)),
                Droplet { velocity, life: DROPLET_LIFE * (0.6 + rand() * 0.4) },
                DespawnOnExit(AppState::Race),
            ));
        }
    }
}

fn fly_spray(mut commands: Commands, time: Res<Time>, mut droplets: Query<(Entity, &mut Transform, &mut Droplet)>) {
    let dt = time.delta_secs();
    for (e, mut t, mut d) in &mut droplets {
        d.life -= dt;
        if d.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        d.velocity.y -= 9.81 * dt;
        t.translation += d.velocity * dt;
        // Grows a little as it spreads out.
        t.scale *= 1.0 + dt * 0.8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wading_depth() {
        let s = 50.0;
        let pond = Water(
            TrackCollision::new(vec![[Vec3::new(-s, 1.0, -s), Vec3::new(s, 1.0, -s), Vec3::new(0.0, 1.0, s)]], 8.0),
            Vec::new(),
        );
        // Wheels 0.3 m under the surface; above it; and far off the pond.
        assert!((pond.depth(Vec3::new(0.0, 1.2, 0.0)) - 0.3).abs() < 1e-5);
        assert_eq!(pond.depth(Vec3::new(0.0, 2.0, 0.0)), 0.0);
        assert_eq!(pond.depth(Vec3::new(200.0, 0.0, 0.0)), 0.0);
        assert_eq!(pond.level(Vec3::new(0.0, -10.0, 0.0)), Some(1.0));
    }

    #[test]
    fn water_boxes() {
        // A box 10 m wide and 40 m long turned a quarter round, its top at
        // 2 m; a smaller one inside it, its top at 3 m.
        let quarter = [0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0];
        let lake = Locator { name: "waterbox".into(), pos: [0.0, -8.0, 0.0], scale: [10.0, 20.0, 40.0], rotation: quarter };
        let pool = Locator { name: "WaterBox".into(), pos: [15.0, 0.0, 0.0], scale: [2.0, 6.0, 2.0], rotation: quarter };
        let other = Locator { name: "Respot".into(), ..lake.clone() };
        let world = World { fog_color: 0, materials: vec![], material_wraps: vec![], locators: vec![lake, pool, other], lights: vec![], instances: vec![], sprites: vec![], meshes: vec![] };
        let water = Water(TrackCollision::new(Vec::new(), 8.0), boxes(&world));
        assert_eq!(water.1.len(), 2);
        // Lengthways along X now; however deep, the top's the surface.
        assert_eq!(water.surface(Vec3::new(18.0, -30.0, 0.0)), Some(2.0));
        assert_eq!(water.surface(Vec3::new(15.0, 0.0, 0.0)), Some(3.0));
        assert_eq!(water.surface(Vec3::new(0.0, 0.0, 6.0)), None);
    }
}
