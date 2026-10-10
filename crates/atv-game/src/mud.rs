//! Dirt building up on the quads and riders.
//!
//! The original draws a dirt layer over each vehicle and rider (the second
//! texture set: `GEN_quadmud_01` and friends, picked per track by
//! `mVehicleDirtTexture`). Its pixel shader (`ShinyThingWithDirt`) mixes it
//! in by dirt texture alpha × vertex alpha × the lighting alpha, which the
//! game feeds with the vehicle's dirt level; dirt also dulls the shine.
//! Here each quad's level grows with distance driven on the ground, and
//! the layers fade in with it.

use bevy::prelude::*;

use atv_sim::Vehicle;

use crate::convert::{Mud, MudLayer};
use crate::opponents::Opponents;
use crate::{AppState, Player, QuadBody};

pub struct MudPlugin;

impl Plugin for MudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (get_dirty, show_dirt).chain().run_if(in_state(AppState::Race)));
    }
}

/// Metres on the ground to get fully dirty.
const FULL_AFTER: f32 = 1500.0;

/// How much dirtier a quad gets over `dt`.
pub fn dirtier(level: f32, v: &Vehicle, dt: f32) -> f32 {
    let on_ground = v.wheels.iter().filter(|w| w.contact).count() as f32 / v.wheels.len().max(1) as f32;
    (level + v.velocity.length() * on_ground * dt / FULL_AFTER).min(1.0)
}

fn get_dirty(
    time: Res<Time>,
    player: Option<Res<Player>>,
    opponents: Option<Res<Opponents>>,
    mut player_mud: Query<&mut Mud, With<QuadBody>>,
    mut muds: Query<&mut Mud, Without<QuadBody>>,
) {
    let dt = time.delta_secs();
    if let (Some(p), Ok(mut mud)) = (player, player_mud.single_mut()) {
        mud.0 = dirtier(mud.0, &p.0, dt);
    }
    for r in opponents.iter().flat_map(|o| &o.racers) {
        if let Ok(mut mud) = muds.get_mut(r.body) {
            mud.0 = dirtier(mud.0, &r.vehicle, dt);
        }
    }
}

fn show_dirt(muds: Query<&Mud>, mut layers: Query<&mut MudLayer>, mut materials: ResMut<Assets<StandardMaterial>>) {
    for mut layer in &mut layers {
        let Ok(mud) = muds.get(layer.owner) else { continue };
        // Touch the material only when the change shows.
        if (mud.0 - layer.shown).abs() < 0.01 {
            continue;
        }
        layer.shown = mud.0;
        if let Some(mut m) = materials.get_mut(&layer.material) {
            m.base_color.set_alpha(mud.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_on_the_ground_only() {
        let params = atv_sim::VehicleParams::from_files("", None);
        let mut v = Vehicle::new(params, Vec3::ZERO, Quat::IDENTITY);
        v.wheels = vec![Default::default(); 4];
        v.velocity = Vec3::new(0.0, 0.0, -15.0);
        assert_eq!(dirtier(0.0, &v, 1.0), 0.0);
        v.wheels.iter_mut().for_each(|w| w.contact = true);
        assert!((dirtier(0.0, &v, 1.0) - 0.01).abs() < 1e-6);
        assert_eq!(dirtier(0.999, &v, 1.0), 1.0);
    }
}
