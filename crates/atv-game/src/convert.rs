//! Turns parsed game data into Bevy meshes, materials and textures.
//!
//! The Xbox data is left-handed (D3D); Bevy is right-handed. Every position
//! and normal gets its Z negated and every triangle's winding reversed.

use std::collections::HashMap;

use anyhow::Result;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::DynamicSkinnedMeshBounds;
use bevy::image::{Image, ImageSampler, ImageSamplerDescriptor, ImageAddressMode};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use atv_formats::bog::{TexFormat, Texture};
use atv_formats::anim::Skeleton;
use atv_formats::rom::{Model, Node, Patch, Vertex};
use atv_formats::row::World;
use atv_formats::GameSource;

use crate::lit::{self, LitMaterial};

const PATCH_STEPS: usize = 6;

/// Radius the unit sky-dome models are scaled to. Inside the camera's far
/// plane, beyond the fog.
pub const SKY_RADIUS: f32 = 1800.0;

pub struct Scene {
    pub world: World,
    /// Top-level entities: the track (props included) and the sky dome.
    pub roots: Vec<Entity>,
    pub world_meshes: usize,
    pub props: usize,
    /// What Stadium 1's video screens show, when the track has any (see
    /// [`crate::screens`]).
    pub screens: Option<Handle<Image>>,
}

/// The sky dome; it is kept centred on the camera.
#[derive(Component)]
pub struct SkyDome;

/// How a surface is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Look {
    /// Baked-lit (unlit) surface; textures with alpha are cut out.
    Baked,
    /// Alpha-tested foliage, fences and similar.
    Cutout,
    /// Translucent water.
    Water,
    /// Translucent foam, wakes and spray.
    Foam,
    /// Additive light (Stadium 1's lamp beams): adds texture × vertex
    /// colour, scaled by their alphas, over what is behind.
    Glow,
    /// Low-lying mist sheets; very faint.
    Mist,
    /// Baked shadow decal, darkens what is under it.
    Shadow,
    /// Second texture layer blended over a track surface by vertex alpha.
    Overlay,
    /// Lit by the sun (vehicles), with the original's shader ([`lit`]).
    Lit,
    /// Lit and always opaque: rider textures keep a gloss mask in alpha.
    Rider,
    /// Sky dome: unlit and unfogged.
    Sky,
    /// Dirt over a vehicle or rider: lit, vertex alpha is how much shows.
    Dirt,
}

impl Look {
    /// From a ROW mesh block's flags.
    fn from_flags(flags: u32) -> Self {
        if flags & 0x8000 != 0 {
            Look::Shadow
        } else if flags & 0x2000 != 0 {
            Look::Water
        } else if flags & 0x4000 != 0 {
            Look::Foam
        } else if flags & 0x18 != 0 {
            // 0x8 adds (ONE, ONE), 0x10 adds by alpha (SRC_ALPHA, ONE),
            // both without depth writes (0x1060b0 / 0x1060f0).
            Look::Glow
        } else if flags & 0x2 != 0 {
            Look::Cutout
        } else {
            Look::Baked
        }
    }

    /// Whether vertex alpha is coverage for this look (otherwise it drives
    /// effects and is ignored).
    fn uses_vertex_alpha(self) -> bool {
        matches!(self, Look::Water | Look::Foam | Look::Glow | Look::Mist | Look::Shadow | Look::Overlay | Look::Dirt)
    }

    /// Vertex colour for this look. Lit surfaces keep the stored value:
    /// their shader multiplies it by the light and doubles the result.
    fn color(self, c: u32) -> [f32; 4] {
        let mut rgba = argb(c, self.uses_vertex_alpha());
        if self.is_lit() {
            rgba[..3].iter_mut().for_each(|v| *v *= 0.5);
        }
        rgba
    }

    /// Drawn with [`LitMaterial`].
    fn is_lit(self) -> bool {
        matches!(self, Look::Lit | Look::Rider | Look::Dirt)
    }
}

/// Distance from the model origin (ground under the wheels) up to the
/// physics centre of mass; WHEELOFFSET in the params is relative to it.
/// With the original suspension the front wheel centres settle 0.245 below
/// the centre of mass, where the body model has them at 0.242.
pub const COM_HEIGHT: f32 = 0.245;

/// A wheel (or rear axle half) that follows the simulation.
#[derive(Component)]
pub struct WheelVisual {
    /// Simulation wheel indices this visual averages (the rear axle is solid).
    pub wheels: Vec<usize>,
    pub base: Vec3,
    pub mirror: bool,
    /// The quad (the entity `spawn_quad` returned) this wheel belongs to.
    pub owner: Entity,
}

/// Where a quad's exhaust smokes (its body's `exhaustonsmokecentre`
/// hard-point), relative to the quad's root (the centre of mass).
#[derive(Component, Clone, Copy, Debug)]
pub struct Exhaust {
    pub tip: Vec3,
}

/// A rider's skinned mesh and the joint entities that pose it.
pub struct RiderRig {
    /// Joint entities in skeleton order; the first is the pelvis, a child
    /// of the quad's model root.
    pub joints: Vec<Entity>,
    /// Each bone's bind offset from its parent (Bevy space).
    pub offsets: Vec<Vec3>,
}

pub struct Loader<'a> {
    game: &'a GameSource,
    meshes: &'a mut Assets<Mesh>,
    materials: &'a mut Assets<StandardMaterial>,
    lit: &'a mut Assets<LitMaterial>,
    images: &'a mut Assets<Image>,
    /// Lower-case texture stem -> archive path, for name-based lookup.
    texture_paths: HashMap<String, String>,
    textures: HashMap<String, Option<(Handle<Image>, bool)>>,
    material_cache: HashMap<(String, Look), Handle<StandardMaterial>>,
    /// Lit materials by texture, look and whether they shine.
    lit_cache: HashMap<(String, Look, bool), Handle<LitMaterial>>,
    models: HashMap<String, Option<Model>>,
    model_paths: HashMap<String, Vec<String>>,
    /// The race's ground: picks the tyres and, when set, gives vehicles
    /// and riders a dirt layer that builds up (see [`Mud`]).
    pub dirt: Option<Ground>,
    /// The freestyle challenge being run, whose props are put out.
    pub challenge: Option<usize>,
    /// The quad spawned last; its rider's dirt follows it.
    last_quad: Option<Entity>,
    /// While spawning Time Trial's ghost: how opaque it is drawn. The
    /// ghost gets its own see-through copies of the materials (as the
    /// original loads the ghost its own models and textures, 0x26240),
    /// and no dirt, shadow or exhaust.
    pub ghost: Option<f32>,
    ghost_materials: HashMap<AssetId<LitMaterial>, Handle<LitMaterial>>,
    /// The image standing in for the original's front buffer.
    front_buffer: Option<Handle<Image>>,
}

/// What a track's ground does to the quads (`Tracks.xml` `mTyreType`,
/// which also matches each track's `mVehicleDirtTexture`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ground {
    #[default]
    Road,
    Sand,
    Snow,
}

impl Ground {
    pub fn from_tyre_type(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "sand" => Ground::Sand,
            "snow" => Ground::Snow,
            _ => Ground::Road,
        }
    }

    /// Wheel model prefix (`sand_wheel_front_left`).
    fn wheels(self) -> &'static str {
        match self {
            Ground::Road => "road",
            Ground::Sand => "sand",
            Ground::Snow => "snow",
        }
    }

    /// Dirt texture stem (`GEN_quadmud_01`, `CH_quadsand_01`).
    fn dirt(self) -> &'static str {
        match self {
            Ground::Road => "mud",
            Ground::Sand => "sand",
            Ground::Snow => "snow",
        }
    }

    /// Tyre dirt colour (`UNI_TyreOverlay_DustPalette`).
    fn tyre_palette(self) -> &'static str {
        match self {
            Ground::Road => "Mud",
            Ground::Sand => "Dust",
            Ground::Snow => "Snow",
        }
    }
}

/// How dirty a quad and its rider are, 0 (clean) to 1.
#[derive(Component, Default)]
pub struct Mud(pub f32);

/// A dirt layer mesh, faded in with its quad's [`Mud`].
#[derive(Component)]
pub struct MudLayer {
    pub owner: Entity,
    pub material: Handle<LitMaterial>,
    pub shown: f32,
}

/// Geometry gathered per material before upload.
#[derive(Default)]
struct Batch {
    pos: Vec<[f32; 3]>,
    normal: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    color: Vec<[f32; 4]>,
    idx: Vec<u32>,
    /// Skinning, for riders only: up to four bones per vertex.
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
}

impl Batch {
    /// `overlay` takes the second UV set (track meshes keep it in the first
    /// two "extra" floats).
    fn add_tris(&mut self, verts: &[Vertex], indices: &[u32], look: Look, overlay: bool) {
        let base = self.pos.len() as u32;
        for v in verts {
            self.pos.push(flip(v.pos));
            self.normal.push(flip(v.normal));
            self.uv.push(if overlay { [v.extra[0], v.extra[1]] } else { v.uv });
            self.color.push(look.color(v.color));
        }
        for t in indices.chunks_exact(3) {
            self.idx.extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
        }
    }

    fn add_patch(&mut self, p: &Patch, look: Look, overlay: bool) {
        let tess = p.tessellate(PATCH_STEPS);
        let (pos, idx) = (&tess.pos, &tess.indices);
        let uv = if overlay { &tess.uv2 } else { &tess.uv };
        let base = self.pos.len() as u32;
        let n = PATCH_STEPS + 1;
        let cols = p.corner_color.map(|c| look.color(c));
        for (k, (pp, tt)) in pos.iter().zip(uv).enumerate() {
            let (u, v) = ((k % n) as f32 / PATCH_STEPS as f32, (k / n) as f32 / PATCH_STEPS as f32);
            // Normal from finite differences across the grid.
            let at = |i: usize, j: usize| Vec3::from(pos[j.min(n - 1) * n + i.min(n - 1)]);
            let (i, j) = (k % n, k / n);
            let du = at(i + 1, j) - at(i.saturating_sub(1), j);
            let dv = at(i, j + 1) - at(i, j.saturating_sub(1));
            let nrm = du.cross(dv).normalize_or(Vec3::Y);
            self.pos.push(flip(*pp));
            self.normal.push(flip(nrm.into()));
            self.uv.push(*tt);
            let mut c = [0.0; 4];
            for ch in 0..4 {
                c[ch] = (cols[0][ch] * (1.0 - u) + cols[1][ch] * u) * (1.0 - v)
                    + (cols[3][ch] * (1.0 - u) + cols[2][ch] * u) * v;
            }
            self.color.push(c);
        }
        for t in idx.chunks_exact(3) {
            self.idx.extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
        }
    }

    /// Adds a rider node: its triangles follow one bone per vertex; patch
    /// vertices blend the bones of the control points by their Bezier
    /// weights, which is exact for rigidly skinned control points.
    fn add_skinned(&mut self, node: &Node, look: Look, overlay: bool) {
        self.add_tris(&node.vertices, &node.indices, look, overlay);
        for v in &node.vertices {
            self.joints.push([v.bone as u16, 0, 0, 0]);
            self.weights.push([1.0, 0.0, 0.0, 0.0]);
        }
        for p in &node.patches {
            self.add_patch(p, look, overlay);
            for j in 0..=PATCH_STEPS {
                let bv = bernstein(j as f32 / PATCH_STEPS as f32);
                for i in 0..=PATCH_STEPS {
                    let bu = bernstein(i as f32 / PATCH_STEPS as f32);
                    let mut per_bone: Vec<(u16, f32)> = Vec::new();
                    for (k, &bone) in p.bones.iter().enumerate() {
                        let w = bu[k % 4] * bv[k / 4];
                        let bone = bone.max(0) as u16;
                        match per_bone.iter_mut().find(|(b, _)| *b == bone) {
                            Some(e) => e.1 += w,
                            None => per_bone.push((bone, w)),
                        }
                    }
                    per_bone.sort_by(|a, b| b.1.total_cmp(&a.1));
                    per_bone.truncate(4);
                    let total: f32 = per_bone.iter().map(|e| e.1).sum::<f32>().max(1e-6);
                    let (mut j4, mut w4) = ([0u16; 4], [0f32; 4]);
                    for (n, (b, w)) in per_bone.into_iter().enumerate() {
                        j4[n] = b;
                        w4[n] = w / total;
                    }
                    self.joints.push(j4);
                    self.weights.push(w4);
                }
            }
        }
    }

    fn into_mesh(self) -> Mesh {
        let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normal)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.color)
            .with_inserted_indices(Indices::U32(self.idx));
        if self.joints.is_empty() {
            return mesh;
        }
        mesh.with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(self.joints))
            .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, self.weights)
    }
}

fn bernstein(t: f32) -> [f32; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

/// Vehicle textures whose alpha is a gloss mask rather than coverage: each
/// quad's own paint atlas (`Tpage*`: shiny plastic opaque, matte seat and
/// rubber clear in the prototype; all opaque in retail) and the tyre
/// (alpha holds the tread pattern). Cutting them out at alpha 0.5 punched
/// holes through the seat, panels and tyres. `GEN_components` alpha really
/// is coverage (holed plates, cables, chain).
fn alpha_is_gloss(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("tpage") || n == "gen_tyre"
}

/// Baked shadow decals mark shadow as bright texels on black (or, in a few,
/// as alpha). Turns one into black with the shadow strength as alpha.
fn shadow_rgba(tex: &Texture) -> Vec<u8> {
    let mut rgba = tex.to_rgba8();
    let in_alpha = rgba.chunks_exact(4).any(|p| p[3] < 250);
    let level = |p: &[u8]| if in_alpha { p[3] } else { p[0].max(p[1]).max(p[2]) };
    let max = rgba.chunks_exact(4).map(level).max().unwrap_or(0).max(1) as f32;
    for p in rgba.chunks_exact_mut(4) {
        let s = level(p) as f32 / max;
        p.copy_from_slice(&[0, 0, 0, (255.0 * 0.55 * s) as u8]);
    }
    rgba
}

fn flip(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[1], -p[2]]
}

/// 0xAARRGGBB -> linear RGBA. Colours are baked lighting where 0x80 means
/// "unlit", so they are doubled (the Xbox used MODULATE2X). Alpha is kept
/// only where it is coverage or a layer weight.
fn argb(c: u32, alpha: bool) -> [f32; 4] {
    let ch = |s: u32| Color::srgb_u8(((c >> s) & 255) as u8, 0, 0).to_linear().red;
    let a = if alpha { ((c >> 24) & 255) as f32 / 255.0 } else { 1.0 };
    [ch(16) * 2.0, ch(8) * 2.0, ch(0) * 2.0, a]
}

fn mat_from_xbox(rot: &[f32; 9], scale: [f32; 3], pos: [f32; 3]) -> Transform {
    // Rows of the D3D matrix are the basis vectors; conjugate by Z-flip.
    let m = Mat3::from_cols(
        Vec3::new(rot[0], rot[1], -rot[2]),
        Vec3::new(rot[3], rot[4], -rot[5]),
        Vec3::new(-rot[6], -rot[7], rot[8]),
    );
    let rotation = Quat::from_mat3(&ortho_basis(m));
    Transform { translation: Vec3::from(flip(pos)), rotation, scale: Vec3::from(scale) }
}

/// Removes scale/shear so the matrix converts cleanly to a quaternion.
fn ortho_basis(m: Mat3) -> Mat3 {
    let x = m.x_axis.normalize_or(Vec3::X);
    let z = m.z_axis.normalize_or(Vec3::Z);
    let y = z.cross(x).normalize_or(Vec3::Y);
    let z = x.cross(y);
    Mat3::from_cols(x, y, z)
}

impl<'a> Loader<'a> {
    pub fn new(
        game: &'a GameSource,
        meshes: &'a mut Assets<Mesh>,
        materials: &'a mut Assets<StandardMaterial>,
        lit: &'a mut Assets<LitMaterial>,
        images: &'a mut Assets<Image>,
    ) -> Self {
        let mut texture_paths = HashMap::new();
        let mut model_paths: HashMap<String, Vec<String>> = HashMap::new();
        for p in game.archive_paths() {
            let lower = p.to_ascii_lowercase();
            let stem = lower.rsplit('/').next().unwrap();
            if let Some(s) = stem.strip_suffix(".bog") {
                texture_paths.entry(s.to_string()).or_insert_with(|| p.to_string());
            } else if let Some(s) = stem.strip_suffix(".rom") {
                model_paths.entry(s.to_string()).or_default().push(p.to_string());
            }
        }
        Self {
            game,
            meshes,
            materials,
            lit,
            images,
            texture_paths,
            textures: HashMap::new(),
            material_cache: HashMap::new(),
            lit_cache: HashMap::new(),
            models: HashMap::new(),
            model_paths,
            dirt: None,
            challenge: None,
            last_quad: None,
            ghost: None,
            ghost_materials: HashMap::new(),
            front_buffer: None,
        }
    }

    /// Loads a texture by name; returns it and whether it has alpha. Shadow
    /// decals are converted to black-with-alpha.
    fn texture(&mut self, name: &str, shadow: bool) -> Option<(Handle<Image>, bool)> {
        let key = format!("{}{}", name.to_ascii_lowercase(), if shadow { "#shadow" } else { "" });
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        // The disc's `FrontBuffer` texture is a placeholder for the screen.
        if name.eq_ignore_ascii_case("frontbuffer") && !shadow {
            let img = self.front_buffer.get_or_insert_with(|| self.images.add(crate::screens::image())).clone();
            return Some((img, false));
        }
        let loaded = self.texture_paths.get(&name.to_ascii_lowercase()).cloned().and_then(|path| {
            let (tex, _) = crate::hd::texture(self.game, &path).ok()?;
            let size = Extent3d { width: tex.width, height: tex.height, depth_or_array_layers: 1 };
            let (img, alpha) = if shadow {
                let mut img = Image::new(
                    size,
                    TextureDimension::D2,
                    shadow_rgba(&tex),
                    TextureFormat::Rgba8UnormSrgb,
                    RenderAssetUsages::RENDER_WORLD,
                );
                img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
                (img, true)
            } else {
                let (format, alpha) = match tex.format {
                    TexFormat::Dxt1 => (TextureFormat::Bc1RgbaUnormSrgb, false),
                    TexFormat::Dxt5 => (TextureFormat::Bc3RgbaUnormSrgb, true),
                    TexFormat::Bgra8 => (TextureFormat::Bgra8UnormSrgb, true),
                };
                // Block-compressed mips must be at least 4x4 for wgpu validation.
                let mut mips = 0;
                let mut bytes = 0;
                while mips < tex.mip_count {
                    let (w, h) = (tex.width >> mips, tex.height >> mips);
                    // Bevy only takes a single level for uncompressed images.
                    if (tex.format == TexFormat::Bgra8 && mips == 1) || w < 4 || h < 4 {
                        break;
                    }
                    bytes += tex.level_size(mips);
                    mips += 1;
                }
                let mut img = Image::new(
                    size,
                    TextureDimension::D2,
                    tex.data[..bytes].to_vec(),
                    format,
                    RenderAssetUsages::RENDER_WORLD,
                );
                img.texture_descriptor.mip_level_count = mips.max(1);
                img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                    address_mode_u: ImageAddressMode::Repeat,
                    address_mode_v: ImageAddressMode::Repeat,
                    anisotropy_clamp: 8,
                    ..ImageSamplerDescriptor::linear()
                });
                (img, alpha)
            };
            Some((self.images.add(img), alpha))
        });
        if loaded.is_none() {
            warn!("texture not found: {name}");
        }
        self.textures.insert(key, loaded.clone());
        loaded
    }

    fn material(&mut self, name: Option<&str>, look: Look) -> Handle<StandardMaterial> {
        let key = (name.unwrap_or("").to_ascii_lowercase(), look);
        if let Some(m) = self.material_cache.get(&key) {
            return m.clone();
        }
        let tex = name.and_then(|n| self.texture(n, look == Look::Shadow));
        let has_alpha = tex.as_ref().is_some_and(|t| t.1);
        let alpha_mode = match look {
            Look::Water | Look::Foam | Look::Mist | Look::Shadow | Look::Overlay => AlphaMode::Blend,
            Look::Cutout => AlphaMode::Mask(0.5),
            Look::Glow => AlphaMode::Add,
            Look::Baked if has_alpha => AlphaMode::Mask(0.5),
            _ => AlphaMode::Opaque,
        };
        let m = self.materials.add(StandardMaterial {
            base_color: if look == Look::Mist { Color::srgba(1.0, 1.0, 1.0, 0.3) } else { Color::WHITE },
            base_color_texture: tex.map(|t| t.0),
            unlit: true,
            fog_enabled: !matches!(look, Look::Sky | Look::Glow),
            cull_mode: None,
            double_sided: true,
            alpha_mode,
            // Layers drawn on top of a surface win the depth test.
            depth_bias: match look {
                Look::Overlay => 2.0,
                Look::Foam | Look::Mist => 4.0,
                Look::Shadow => 8.0,
                _ => 0.0,
            },
            ..default()
        });
        self.material_cache.insert(key, m.clone());
        m
    }

    /// A quad or rider surface. `shiny`: the mesh has a dirt layer, so the
    /// original draws it with `ShinyThingWithDirt`, whose specular the
    /// texture's alpha masks; without one it is plain `TextureColVertCol`.
    fn lit_material(&mut self, name: Option<&str>, look: Look, shiny: bool) -> Handle<LitMaterial> {
        let key = (name.unwrap_or("").to_ascii_lowercase(), look, shiny);
        let m = match self.lit_cache.get(&key) {
            Some(m) => m.clone(),
            None => {
                let tex = name.and_then(|n| self.texture(n, false));
                // Rider textures and the quads' gloss-masked ones are opaque.
                let cutout = look == Look::Lit && tex.as_ref().is_some_and(|t| t.1) && !name.is_some_and(alpha_is_gloss);
                let flags = if shiny { lit::GLOSS } else { 0 } | if cutout { lit::CUTOUT } else { 0 };
                let alpha_mode = if cutout { AlphaMode::Mask(0.5) } else { AlphaMode::Opaque };
                let m = self.lit.add(LitMaterial::new(tex.map(|t| t.0), flags, alpha_mode));
                self.lit_cache.insert(key, m.clone());
                m
            }
        };
        match self.ghost {
            Some(alpha) => self.ghostly(m, alpha),
            None => m,
        }
    }

    /// A see-through copy of `m` for the ghost.
    fn ghostly(&mut self, m: Handle<LitMaterial>, alpha: f32) -> Handle<LitMaterial> {
        if let Some(g) = self.ghost_materials.get(&m.id()) {
            return g.clone();
        }
        let Some(mut g) = self.lit.get(&m).cloned() else { return m };
        g.alpha_mode = AlphaMode::Blend;
        g.uniform.opacity *= alpha;
        let h = self.lit.add(g);
        self.ghost_materials.insert(m.id(), h.clone());
        h
    }

    /// Spawns a dirt layer mesh. The model names a generic dirt texture
    /// (`GEN_quadmud_01`, `CH_quadmud_01`, `UNI_TyreOverlay`); the track's
    /// ground picks the variant. Each layer gets its own material so its
    /// quad's dirt can fade it in.
    fn spawn_dirt(&mut self, commands: &mut Commands, batch: Batch, name: &str, parent: Entity, owner: Entity, skin: Option<SkinnedMesh>) {
        if batch.idx.is_empty() || self.ghost.is_some() {
            return;
        }
        let ground = self.dirt.unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        let tex = if lower.starts_with("gen_quad") {
            self.texture(&format!("GEN_quad{}_01", ground.dirt()), false).map(|t| t.0)
        } else if lower.starts_with("ch_quad") {
            // The rider's snow has no alpha: borrow the mud's coverage.
            self.composite(&format!("CH_quad{}_01", ground.dirt()), "CH_quadmud_01")
        } else if lower.starts_with("uni_tyreoverlay") {
            self.composite(&format!("UNI_TyreOverlay_{}Palette", ground.tyre_palette()), "UNI_TyreOverlay")
        } else {
            self.texture(name, false).map(|t| t.0)
        };
        let mut dirt = LitMaterial::new(tex, lit::DIRT, AlphaMode::Blend);
        dirt.uniform.opacity = 0.0;
        dirt.depth_bias = 2.0;
        let material = self.lit.add(dirt);
        let mut mesh = batch.into_mesh();
        if skin.is_some() {
            mesh = match mesh.with_generated_skinned_mesh_bounds() {
                Ok(m) => m,
                Err(_) => return,
            };
        }
        let layer = MudLayer { owner, material: material.clone(), shown: 0.0 };
        let mut e = commands.spawn((Mesh3d(self.meshes.add(mesh)), MeshMaterial3d(material), layer));
        if let Some(skin) = skin {
            e.insert((skin, DynamicSkinnedMeshBounds));
        }
        let child = e.id();
        commands.entity(parent).add_child(child);
    }

    /// A texture with the colour of one and the alpha of another: tyre
    /// dirt is a palette colour under `UNI_TyreOverlay`'s alpha (how much
    /// dirt sits where on the tread).
    fn composite(&mut self, colour: &str, alpha: &str) -> Option<Handle<Image>> {
        let key = format!("#{colour}#{alpha}").to_ascii_lowercase();
        if let Some(t) = self.textures.get(&key) {
            return t.clone().map(|t| t.0);
        }
        let load = |name: &str| {
            let path = self.texture_paths.get(&name.to_ascii_lowercase())?;
            let (tex, _) = crate::hd::texture(self.game, path).ok()?;
            let rgba = tex.to_rgba8();
            Some((tex.width, tex.height, rgba))
        };
        let made = load(alpha).zip(load(colour)).map(|(mask, colour)| {
            let (w, h) = (mask.0, mask.1);
            let mut out = vec![0u8; (w * h * 4) as usize];
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    let (cx, cy) = (x * colour.0 / w, y * colour.1 / h);
                    let j = ((cy * colour.0 + cx) * 4) as usize;
                    out[i..i + 3].copy_from_slice(&colour.2[j..j + 3]);
                    out[i + 3] = mask.2[i + 3];
                }
            }
            let size = Extent3d { width: w, height: h, depth_or_array_layers: 1 };
            let mut img = Image::new(size, TextureDimension::D2, out, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
            img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::linear()
            });
            (self.images.add(img), true)
        });
        self.textures.insert(key, made.clone());
        made.map(|t| t.0)
    }

    fn model(&mut self, stem: &str, prefer_dir: &str) -> Option<Model> {
        let key = stem.to_ascii_lowercase();
        if let Some(m) = self.models.get(&key) {
            return m.clone();
        }
        let paths = self.model_paths.get(&key).cloned().unwrap_or_default();
        let pref = prefer_dir.to_ascii_lowercase();
        let path = paths
            .iter()
            .find(|p| p.to_ascii_lowercase().contains(&pref))
            .or_else(|| paths.iter().find(|p| !p.to_ascii_lowercase().contains("frontend")))
            .or(paths.first());
        let m = path.and_then(|p| Model::parse(&self.game.read(p).ok()?).ok());
        if m.is_none() {
            warn!("model not found: {stem}");
        }
        self.models.insert(key, m.clone());
        m
    }

    /// Spawns one model as children of `parent`, one entity per material.
    fn spawn_model(
        &mut self,
        commands: &mut Commands,
        model: &Model,
        parent: Entity,
        transform: Transform,
        look: Look,
    ) -> Entity {
        // Lit meshes with a dirt layer shine (see `lit_material`).
        let mut batches: HashMap<(Option<usize>, bool), Batch> = HashMap::new();
        for node in &model.nodes {
            let b = batches.entry((node.material(), look.is_lit() && node.header[2] >= 0)).or_default();
            b.add_tris(&node.vertices, &node.indices, look, false);
            for p in &node.patches {
                b.add_patch(p, look, false);
            }
        }
        let holder = commands.spawn((transform, Visibility::default())).id();
        commands.entity(parent).add_child(holder);
        if let (Some(_), Some(owner)) = (self.dirt, self.last_quad.filter(|_| look == Look::Lit)) {
            let mut dirt: HashMap<usize, Batch> = HashMap::new();
            for node in &model.nodes {
                let Ok(ov) = usize::try_from(node.header[2]) else { continue };
                let b = dirt.entry(ov).or_default();
                b.add_tris(&node.vertices, &node.indices, Look::Dirt, true);
                for p in &node.patches {
                    b.add_patch(p, Look::Dirt, true);
                }
            }
            for (ov, batch) in dirt {
                let Some(name) = model.materials.get(ov).map(|m| m.name.clone()) else { continue };
                self.spawn_dirt(commands, batch, &name, holder, owner, None);
            }
        }
        for ((mat, shiny), batch) in batches {
            if batch.idx.is_empty() {
                continue;
            }
            let name = mat.and_then(|i| model.materials.get(i)).map(|m| m.name.clone());
            let look = if name.as_deref().is_some_and(|n| n.to_ascii_lowercase().contains("shad")) {
                Look::Shadow
            } else {
                look
            };
            if look == Look::Shadow && self.ghost.is_some() {
                continue;
            }
            let mesh = Mesh3d(self.meshes.add(batch.into_mesh()));
            let child = if look.is_lit() {
                commands.spawn((mesh, MeshMaterial3d(self.lit_material(name.as_deref(), look, shiny)))).id()
            } else {
                commands.spawn((mesh, MeshMaterial3d(self.material(name.as_deref(), look)))).id()
            };
            commands.entity(holder).add_child(child);
        }
        holder
    }

    pub fn spawn_track(&mut self, commands: &mut Commands, track: &str) -> Result<Scene> {
        let path = format!("/Levels/{track}/World/{track}.row");
        let mut world = World::parse(&self.game.read(&path).map_err(|e| anyhow::anyhow!("reading {path}: {e}"))?)?;
        // Stadium 2's cones, gates and floor arrows belong to challenges.
        world.instances.retain(|i| i.in_play(self.challenge));
        let env: String = track.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
        let root = commands.spawn((Transform::default(), Visibility::default(), Name::new(track.to_string()))).id();

        // One batch per material and look; a mesh with a second material
        // also adds its overlay layer to that material's overlay batch.
        let mut batches: HashMap<(i32, Look), Batch> = HashMap::new();
        for m in &world.meshes {
            // No material: invisible geometry that only blocks the quad.
            if m.material < 0 {
                continue;
            }
            let name = |i: i32| usize::try_from(i).ok().and_then(|i| world.materials.get(i)).map(|n| n.to_ascii_lowercase());
            // Mist sheets pair two mist textures that modulate each other;
            // the base alone gives the haze.
            let mist = name(m.material).is_some_and(|n| n.contains("mist"));
            let look = if mist { Look::Mist } else { Look::from_flags(m.flags) };
            let mut layers = vec![(m.material, look, false)];
            if m.material2 >= 0 && look != Look::Shadow && !mist {
                layers.push((m.material2, Look::Overlay, true));
            }
            for (mat, look, overlay) in layers {
                let b = batches.entry((mat, look)).or_default();
                b.add_tris(&m.vertices, &m.indices, look, overlay);
                for p in &m.patches {
                    b.add_patch(p, look, overlay);
                }
            }
        }
        let world_meshes = batches.len();

        for ((mat, look), batch) in batches {
            let name = usize::try_from(mat).ok().and_then(|i| world.materials.get(i)).cloned();
            let material = self.material(name.as_deref(), look);
            let mesh = self.meshes.add(batch.into_mesh());
            let e = commands.spawn((Mesh3d(mesh), MeshMaterial3d(material))).id();
            commands.entity(root).add_child(e);
        }

        let mut props = 0;
        for inst in &world.instances {
            let stem = inst.model.trim_end_matches(".clm").trim_end_matches(".CLM");
            if let Some(model) = self.model(stem, &env) {
                let t = mat_from_xbox(&inst.rotation, inst.scale, inst.pos);
                let holder = self.spawn_model(commands, &model, root, t, Look::Baked);
                if inst.flags.split_whitespace().next().is_some_and(|f| f.eq_ignore_ascii_case("free")) {
                    let (a, b) = (Vec3::from(flip(model.bbox_min)), Vec3::from(flip(model.bbox_max)));
                    commands.entity(holder).insert(crate::knock::Knockable::from_box(a.min(b), a.max(b), &t));
                }
                props += 1;
            }
        }

        // Sky dome, following the track's environment naming.
        let sky = match env.as_str() {
            "Dunes" => "DU_sky",
            "Glacier" => "GL_sky",
            "Industrial" => "IND_sky",
            "Swamp" => "SW_sky",
            "Stadium" => "ST_sky",
            _ => "skydomeday",
        };
        // The domes are unit hemispheres; scaled up and kept on the camera.
        let mut roots = vec![root];
        if let Some(model) = self.model(sky, "SkyDomes") {
            let holder = commands.spawn((Transform::default(), Visibility::default(), SkyDome)).id();
            roots.push(holder);
            let t = Transform::from_scale(Vec3::splat(SKY_RADIUS));
            self.spawn_model(commands, &model, holder, t, Look::Sky);
            // Mirrored below the horizon: translucent water shows it as a
            // reflection, as the mirrored scenery under the water expects.
            let t = Transform::from_scale(Vec3::new(SKY_RADIUS, -SKY_RADIUS, SKY_RADIUS));
            self.spawn_model(commands, &model, holder, t, Look::Sky);
        }

        Ok(Scene { world, roots, world_meshes, props, screens: self.front_buffer.clone() })
    }

    /// Spawns a loose model (the challenges' guide arrow) under its own
    /// root, at the world origin.
    pub fn spawn_loose(&mut self, commands: &mut Commands, stem: &str, dir: &str) -> Option<Entity> {
        let model = self.model(stem, dir)?;
        let root = commands.spawn((Transform::default(), Visibility::default(), Name::new(stem.to_string()))).id();
        self.spawn_model(commands, &model, root, Transform::default(), Look::Baked);
        Some(root)
    }

    /// Assembles a quad from its parts at the hard-points in its body model.
    /// Returns the root entity, whose transform is the physics body (centre
    /// of mass); the model hangs below it by [`COM_HEIGHT`].
    pub fn spawn_quad(&mut self, commands: &mut Commands, quad: &str, at: Transform, wheel_x: &[f32]) -> Result<Entity> {
        let dir = format!("/Cars/{quad}/");
        let body = Model::parse(&self.game.read(&format!("{dir}Body.rom"))?)?;
        let point = |name: &str| {
            body.points.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, p)| Vec3::from(flip(*p)))
        };
        let root = commands.spawn((at, Visibility::default(), Name::new(quad.to_string()))).id();
        let model_root = commands.spawn((Transform::from_xyz(0.0, -COM_HEIGHT, 0.0), Visibility::default())).id();
        commands.entity(root).add_child(model_root);
        let root_entity = root;
        let root = model_root;
        if self.dirt.is_some() && self.ghost.is_none() {
            commands.entity(root_entity).insert(Mud::default());
        }
        if let Some(tip) = point("exhaustonsmokecentre").filter(|_| self.ghost.is_none()) {
            commands.entity(root_entity).insert(Exhaust { tip: tip - Vec3::new(0.0, COM_HEIGHT, 0.0) });
        }
        self.last_quad = Some(root_entity);
        self.spawn_model(commands, &body, root, Transform::default(), Look::Lit);
        let parts = [
            ("Handlebars", "handlebarscentre"),
            ("FLWishbone", "flsuspcentre"),
            ("FRWishbone", "frsuspcentre"),
            ("FLMudguard", "flmudguardcentre"),
            ("FRMudguard", "frmudguardcentre"),
            ("RLMudguard", "rlmudguardcentre"),
            ("RRMudguard", "rrmudguardcentre"),
            ("RearTrailArm", "rsuspcentre"),
            ("RearShock", "rshockpivot"),
            ("Exhaust", "exhaustpivot"),
        ];
        for (part, pt) in parts {
            let Ok(bytes) = self.game.read(&format!("{dir}{part}.rom")) else { continue };
            let Ok(model) = Model::parse(&bytes) else { continue };
            let pos = point(pt).unwrap_or_default();
            self.spawn_model(commands, &model, root, Transform::from_translation(pos), Look::Lit);
        }
        // Params wheel order: 0 FL, 1 FR, 2 RL, 3 RR. Wheel models are
        // centred on the tyre: height and length come from the body's
        // hard-points, the track width from WHEELOFFSET.
        // Sand and snow tracks fit their own tyres.
        let wheels: [(&str, &str, f32, &[usize]); 4] = [
            ("wheel_front_left", "flwheelcentre", 1.0, &[0]),
            ("wheel_front_left", "frwheelcentre", -1.0, &[1]),
            ("wheel_rear_left", "rwheelcentre", 1.0, &[2]),
            ("wheel_rear_right", "rwheelcentre", 1.0, &[3]),
        ];
        let tyres = self.dirt.unwrap_or_default().wheels();
        for (k, (w, pt, sx, idx)) in wheels.into_iter().enumerate() {
            let model = self.model(&format!("{tyres}_{w}"), "Cars/Generic").or_else(|| self.model(&format!("road_{w}"), "Cars/Generic"));
            if let Some(model) = model {
                let mut pos = point(pt).unwrap_or_default();
                if let Some(x) = wheel_x.get(k) {
                    pos.x = *x;
                }
                let t = Transform::from_translation(pos).with_scale(Vec3::new(sx, 1.0, 1.0));
                let e = self.spawn_model(commands, &model, root, t, Look::Lit);
                commands.entity(e).insert(WheelVisual { wheels: idx.to_vec(), base: pos, mirror: sx < 0.0, owner: root_entity });
            }
        }
        Ok(root_entity)
    }

    /// Spawns a rider (`Characters/<name>`) under a quad's model root. The
    /// mesh is skinned to one joint entity per bone, left in the bind pose
    /// (a T-pose around the model origin) for the caller to pose.
    pub fn spawn_rider(
        &mut self,
        commands: &mut Commands,
        bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
        name: &str,
        parent: Entity,
    ) -> Result<RiderRig> {
        let dir = format!("/Characters/{name}/");
        let model = Model::parse(&self.game.read(&format!("{dir}{name}.rom"))?)?;
        let skeleton = Skeleton::parse(&self.game.read(&format!("{dir}{name}.bas"))?)?;
        let bind: Vec<Vec3> = skeleton.bind_positions().into_iter().map(|p| Vec3::from(flip(p))).collect();
        let offsets: Vec<Vec3> = skeleton.bones.iter().map(|b| Vec3::from(flip(b.offset))).collect();

        let mut joints: Vec<Entity> = Vec::with_capacity(skeleton.bones.len());
        for (i, b) in skeleton.bones.iter().enumerate() {
            let e = commands.spawn((Transform::from_translation(offsets[i]), Visibility::default())).id();
            commands.entity(b.parent.map_or(parent, |p| joints[p])).add_child(e);
            joints.push(e);
        }
        let inverse = bindposes.add(bind.iter().map(|p| Mat4::from_translation(-*p)).collect::<Vec<_>>());

        let mut batches: HashMap<(Option<usize>, bool), Batch> = HashMap::new();
        for node in &model.nodes {
            batches.entry((node.material(), node.header[2] >= 0)).or_default().add_skinned(node, Look::Rider, false);
        }
        for ((mat, shiny), batch) in batches {
            if batch.idx.is_empty() {
                continue;
            }
            let name = mat.and_then(|i| model.materials.get(i)).map(|m| m.name.clone());
            let material = self.lit_material(name.as_deref(), Look::Rider, shiny);
            let mesh = batch.into_mesh().with_generated_skinned_mesh_bounds()?;
            let child = commands
                .spawn((
                    Mesh3d(self.meshes.add(mesh)),
                    MeshMaterial3d(material),
                    SkinnedMesh { inverse_bindposes: inverse.clone(), joints: joints.clone() },
                    DynamicSkinnedMeshBounds,
                ))
                .id();
            commands.entity(parent).add_child(child);
        }
        if let (Some(_), Some(owner)) = (self.dirt, self.last_quad) {
            let mut dirt: HashMap<usize, Batch> = HashMap::new();
            for node in &model.nodes {
                if let Ok(ov) = usize::try_from(node.header[2]) {
                    dirt.entry(ov).or_default().add_skinned(node, Look::Dirt, true);
                }
            }
            for (ov, mut batch) in dirt {
                let Some(name) = model.materials.get(ov).map(|m| m.name.clone()) else { continue };
                // Rider vertex alpha is all zero: the mask is in the texture.
                batch.color.iter_mut().for_each(|c| c[3] = 1.0);
                let skin = SkinnedMesh { inverse_bindposes: inverse.clone(), joints: joints.clone() };
                self.spawn_dirt(commands, batch, &name, parent, owner, Some(skin));
            }
        }
        Ok(RiderRig { joints, offsets })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stadium_light_beams_glow() {
        // ST_Rays (0x412): additive by alpha, though double-sided (0x2).
        assert_eq!(Look::from_flags(0x412), Look::Glow);
        assert_eq!(Look::from_flags(0x8), Look::Glow);
        // Neon strips and hanging arrows stay cut out.
        assert_eq!(Look::from_flags(0x82), Look::Cutout);
        assert_eq!(Look::from_flags(0x486), Look::Cutout);
    }
}
