//! Fog mixed the way the Xbox mixed it.
//!
//! The original fogs in the pixel pipeline's final stage, on gamma-encoded
//! colours: out = (1 - f) x colour + f x fog colour. Bevy mixes in linear
//! light, which on the dark tracks turns the near scenery pale: Swamp 1's
//! fog starts at the camera, and 15% of its grey there showed as 40%. Bevy
//! applies its fog in one shader module, `bevy_pbr::render::fog`, for every
//! fogged material (ours in `lit.wesl` too), so this swaps that module for
//! `fog.wesl`, the same formulas with the linear fog mixed in gamma.
//!
//! The module is an embedded asset that Bevy loads on its own; whenever it
//! (re)loads, the swap is put back.

use bevy::prelude::*;

/// Bevy's fog module.
const BEVY_FOG: &str = "embedded://bevy_pbr/render/fog.wesl";

/// Marks the replacement's source.
const MARK: &str = "// atv2 gamma fog";

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        let handle: Handle<Shader> = app.world().resource::<AssetServer>().load(BEVY_FOG);
        app.insert_resource(BevyFog(handle)).add_systems(PostUpdate, swap_fog);
    }
}

/// Keeps Bevy's fog module loaded and names it.
#[derive(Resource)]
struct BevyFog(Handle<Shader>);

/// Puts `fog.wesl` in place of Bevy's fog module when that one loads.
fn swap_fog(fog: Res<BevyFog>, mut events: MessageReader<AssetEvent<Shader>>, mut shaders: ResMut<Assets<Shader>>) {
    let touched = events.read().any(|e| match e {
        AssetEvent::Added { id } | AssetEvent::Modified { id } | AssetEvent::LoadedWithDependencies { id } => *id == fog.0.id(),
        _ => false,
    });
    if !touched || shaders.get(&fog.0).is_none_or(|s| matches!(&s.source, bevy::shader::Source::Wesl(src) if src.contains(MARK))) {
        return;
    }
    let _ = shaders.insert(&fog.0, Shader::from_wesl(include_str!("fog.wesl"), BEVY_FOG));
}
