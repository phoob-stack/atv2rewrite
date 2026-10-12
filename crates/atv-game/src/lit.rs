//! How the quads and riders are lit: the original's vertex and pixel
//! shaders, ported. Both discs ship their shader sources (`VertexShader/`,
//! `PixelShader/`).
//!
//! The vertex shader (`SunLight.asm`) lights each vertex with the track's
//! ambient and sun: diffuse = saturate(vertex colour x (ambient + N.L x
//! sun)), specular = (N.H)^16 x the sun's specular colour (constants c5,
//! c6, c7, c40, c95 = (1, 2, 16, 0.5), set at 0x4e8cfe). The pixel shader
//! is `TextureColVertCol` (texture x diffuse x 2, no specular) for a mesh
//! without a dirt layer, and `ShinyThingWithDirt` for one with (generic
//! mesh draw 0x4ef690), which adds the specular masked by the texture's
//! alpha. The maths happens on the gamma-encoded values, as the Xbox did.
//!
//! The colours come from `Tracks.xml` (`mAmbientColour`, `mSunColour`,
//! `mSunSpecColour`, read at race start by 0x41f840), the sun's direction
//! from the track's `sun` light (or the original's default, 0x4f2600).

use bevy::asset::{load_internal_asset, uuid_handle};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use atv_formats::{setup, GameSource};

const SHADER: Handle<Shader> = uuid_handle!("6f0f3c1e-63a5-4f83-9d0a-6a6f0e7b1c55");

/// Specular masked by the texture's alpha (`ShinyThingWithDirt`).
pub const GLOSS: u32 = 1;
/// Texture alpha is coverage, cut out at 0.5.
pub const CUTOUT: u32 = 2;
/// A dirt layer: coverage is texture alpha x vertex alpha x `opacity`.
pub const DIRT: u32 = 4;

pub struct LitPlugin;

impl Plugin for LitPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, SHADER, "lit.wesl", Shader::from_wesl);
        app.add_plugins(MaterialPlugin::<LitMaterial>::default())
            .init_resource::<Lighting>()
            .add_systems(PostUpdate, apply_lighting);
    }
}

/// The light on the quads and riders, as the original's shader constants.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub struct Lighting {
    pub ambient: Vec3,
    pub sun: Vec3,
    /// c40: the sun's specular colour.
    pub specular: Vec3,
    /// Towards the sun.
    pub to_sun: Vec3,
}

impl Default for Lighting {
    /// The original's defaults for a scene without lights: half ambient,
    /// half sun from above (0x4f2600), and no specular.
    fn default() -> Self {
        Self { ambient: Vec3::splat(0.5), sun: Vec3::splat(0.5), specular: Vec3::ZERO, to_sun: -DEFAULT_SUN_DIR.normalize() }
    }
}

/// The way the sun shines when a track has no `sun` light (0x4f2600).
pub const DEFAULT_SUN_DIR: Vec3 = Vec3::new(0.0, -1.0, -0.5);

impl Lighting {
    /// A track's light from its `Tracks.xml` entry. `sun_dir` is the way
    /// its `sun` light shines.
    pub fn for_track(game: &GameSource, track: &str, sun_dir: Option<Vec3>) -> Self {
        let xml = game.read("/Setup/Tracks.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let colour = |name: &str, or: Vec3| {
            setup::child(&xml, track, name).map_or(or, |c| {
                let ch = |k: &str| c.get(k).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0);
                Vec3::new(ch("R"), ch("G"), ch("B"))
            })
        };
        let tint = setup::child(&xml, track, "XBox").and_then(|c| c.get("mGlobalTint")?.trim().parse::<f32>().ok()).unwrap_or(1.0);
        let d = Self::default();
        Self {
            ambient: colour("mAmbientColour", d.ambient) * tint,
            sun: colour("mSunColour", d.sun) * tint,
            // Halved, and less red and green (0x41f840).
            specular: colour("mSunSpecColour", Vec3::ZERO) * Vec3::new(0.65, 0.75, 1.0) * 0.5 * tint,
            to_sun: -sun_dir.unwrap_or(DEFAULT_SUN_DIR).normalize_or(DEFAULT_SUN_DIR.normalize()),
        }
    }

    fn uniform(&self) -> [Vec4; 4] {
        [self.ambient.extend(0.0), self.sun.extend(0.0), self.specular.extend(0.0), self.to_sun.extend(0.0)]
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct LitUniform {
    pub ambient: Vec4,
    pub sun: Vec4,
    pub specular: Vec4,
    pub to_sun: Vec4,
    pub opacity: f32,
    pub flags: u32,
}

/// A quad or rider surface (see the module docs).
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct LitMaterial {
    #[uniform(0)]
    pub uniform: LitUniform,
    #[texture(1)]
    #[sampler(2)]
    pub texture: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    pub depth_bias: f32,
}

impl LitMaterial {
    pub fn new(texture: Option<Handle<Image>>, flags: u32, alpha_mode: AlphaMode) -> Self {
        let mut uniform = LitUniform { opacity: 1.0, flags, ..default() };
        [uniform.ambient, uniform.sun, uniform.specular, uniform.to_sun] = Lighting::default().uniform();
        Self { uniform, texture, alpha_mode, depth_bias: 0.0 }
    }
}

impl Material for LitMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(SHADER)
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    fn depth_bias(&self) -> f32 {
        self.depth_bias
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // The models are drawn without back-face culling.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// Puts the current [`Lighting`] into every lit material that lacks it.
fn apply_lighting(lighting: Res<Lighting>, mut materials: ResMut<Assets<LitMaterial>>) {
    let want = lighting.uniform();
    let stale: Vec<_> = materials
        .iter()
        .filter(|(_, m)| [m.uniform.ambient, m.uniform.sun, m.uniform.specular, m.uniform.to_sun] != want)
        .map(|(id, _)| id)
        .collect();
    for id in stale {
        if let Some(mut m) = materials.get_mut(id) {
            [m.uniform.ambient, m.uniform.sun, m.uniform.specular, m.uniform.to_sun] = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specular_is_halved_and_tinted() {
        // Retail Dunes 1: mSunSpecColour 0.3 grey.
        let s = Vec3::splat(0.3) * Vec3::new(0.65, 0.75, 1.0) * 0.5;
        assert!((s - Vec3::new(0.0975, 0.1125, 0.15)).length() < 1e-6);
        assert_eq!(Lighting::default().specular, Vec3::ZERO);
        assert!((Lighting::default().to_sun.y - 0.894).abs() < 1e-3);
    }
}
