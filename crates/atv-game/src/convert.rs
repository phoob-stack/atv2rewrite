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
    /// Low-lying mist sheets; very faint.
    Mist,
    /// Baked shadow decal, darkens what is under it.
    Shadow,
    /// Second texture layer blended over a track surface by vertex alpha.
    Overlay,
    /// Lit by the sun (vehicles).
    Lit,
    /// Lit and always opaque: rider textures keep a gloss mask in alpha.
    Rider,
    /// Sky dome: unlit and unfogged.
    Sky,
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
        } else if flags & 0x2 != 0 {
            Look::Cutout
        } else {
            Look::Baked
        }
    }

    /// Whether vertex alpha is coverage for this look (otherwise it drives
    /// effects and is ignored).
    fn uses_vertex_alpha(self) -> bool {
        matches!(self, Look::Water | Look::Foam | Look::Mist | Look::Shadow | Look::Overlay)
    }

    /// Vertex colour for this look. Lit surfaces get their light from the
    /// scene, so the baked (doubled) value is capped at white.
    fn color(self, c: u32) -> [f32; 4] {
        let mut rgba = argb(c, self.uses_vertex_alpha());
        if matches!(self, Look::Lit | Look::Rider) {
            rgba[..3].iter_mut().for_each(|v| *v = v.min(1.0));
        }
        rgba
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
    images: &'a mut Assets<Image>,
    /// Lower-case texture stem -> archive path, for name-based lookup.
    texture_paths: HashMap<String, String>,
    textures: HashMap<String, Option<(Handle<Image>, bool)>>,
    gloss_maps: HashMap<String, Option<Handle<Image>>>,
    material_cache: HashMap<(String, Look), Handle<StandardMaterial>>,
    models: HashMap<String, Option<Model>>,
    model_paths: HashMap<String, Vec<String>>,
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
    fn add_skinned(&mut self, node: &Node, look: Look) {
        self.add_tris(&node.vertices, &node.indices, look, false);
        for v in &node.vertices {
            self.joints.push([v.bone as u16, 0, 0, 0]);
            self.weights.push([1.0, 0.0, 0.0, 0.0]);
        }
        for p in &node.patches {
            self.add_patch(p, look, false);
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
/// rubber clear) and the tyre (alpha holds the tread pattern). Cutting
/// them out at alpha 0.5 punched holes through the seat, panels and tyres.
/// `GEN_components` alpha really is coverage (holed plates, cables, chain).
fn alpha_is_gloss(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("tpage") || n == "gen_tyre"
}

/// Metallic-roughness map (roughness in green) from a gloss mask in alpha.
fn gloss_rgba(tex: &Texture) -> Vec<u8> {
    let mut rgba = tex.to_rgba8();
    for p in rgba.chunks_exact_mut(4) {
        let rough = 0.8 - 0.55 * (p[3] as f32 / 255.0);
        p.copy_from_slice(&[255, (rough * 255.0) as u8, 0, 255]);
    }
    rgba
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
            images,
            texture_paths,
            textures: HashMap::new(),
            gloss_maps: HashMap::new(),
            material_cache: HashMap::new(),
            models: HashMap::new(),
            model_paths,
        }
    }

    /// Loads a texture by name; returns it and whether it has alpha. Shadow
    /// decals are converted to black-with-alpha.
    fn texture(&mut self, name: &str, shadow: bool) -> Option<(Handle<Image>, bool)> {
        let key = format!("{}{}", name.to_ascii_lowercase(), if shadow { "#shadow" } else { "" });
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
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

    /// Roughness map from a texture's gloss-mask alpha (see [`alpha_is_gloss`]).
    fn gloss_map(&mut self, name: &str) -> Option<Handle<Image>> {
        let key = name.to_ascii_lowercase();
        if let Some(t) = self.gloss_maps.get(&key) {
            return t.clone();
        }
        let loaded = self.texture_paths.get(&key).cloned().and_then(|path| {
            let (tex, _) = crate::hd::texture(self.game, &path).ok()?;
            let size = Extent3d { width: tex.width, height: tex.height, depth_or_array_layers: 1 };
            let mut img = Image::new(
                size,
                TextureDimension::D2,
                gloss_rgba(&tex),
                TextureFormat::Rgba8Unorm,
                RenderAssetUsages::RENDER_WORLD,
            );
            img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::linear()
            });
            Some(self.images.add(img))
        });
        self.gloss_maps.insert(key, loaded.clone());
        loaded
    }

    fn material(&mut self, name: Option<&str>, look: Look) -> Handle<StandardMaterial> {
        let key = (name.unwrap_or("").to_ascii_lowercase(), look);
        if let Some(m) = self.material_cache.get(&key) {
            return m.clone();
        }
        let tex = name.and_then(|n| self.texture(n, look == Look::Shadow));
        let gloss = look == Look::Lit && name.is_some_and(alpha_is_gloss);
        let gloss_map = if gloss { name.and_then(|n| self.gloss_map(n)) } else { None };
        let has_alpha = tex.as_ref().is_some_and(|t| t.1) && !gloss;
        let alpha_mode = match look {
            Look::Water | Look::Foam | Look::Mist | Look::Shadow | Look::Overlay => AlphaMode::Blend,
            Look::Cutout => AlphaMode::Mask(0.5),
            Look::Baked | Look::Lit if has_alpha => AlphaMode::Mask(0.5),
            _ => AlphaMode::Opaque,
        };
        let m = self.materials.add(StandardMaterial {
            base_color: if look == Look::Mist { Color::srgba(1.0, 1.0, 1.0, 0.3) } else { Color::WHITE },
            base_color_texture: tex.map(|t| t.0),
            unlit: !matches!(look, Look::Lit | Look::Rider),
            fog_enabled: look != Look::Sky,
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
            perceptual_roughness: if gloss_map.is_some() { 1.0 } else { 0.7 },
            // Kept low: there is no environment map, and Bevy's flat ambient
            // turns a higher F0 into bright rims at grazing angles.
            reflectance: if gloss_map.is_some() { 0.25 } else { 0.15 },
            metallic_roughness_texture: gloss_map,
            ..default()
        });
        self.material_cache.insert(key, m.clone());
        m
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
        let mut batches: HashMap<Option<usize>, Batch> = HashMap::new();
        for node in &model.nodes {
            let b = batches.entry(node.material()).or_default();
            b.add_tris(&node.vertices, &node.indices, look, false);
            for p in &node.patches {
                b.add_patch(p, look, false);
            }
        }
        let holder = commands.spawn((transform, Visibility::default())).id();
        commands.entity(parent).add_child(holder);
        for (mat, batch) in batches {
            if batch.idx.is_empty() {
                continue;
            }
            let name = mat.and_then(|i| model.materials.get(i)).map(|m| m.name.clone());
            let look = if name.as_deref().is_some_and(|n| n.to_ascii_lowercase().contains("shad")) {
                Look::Shadow
            } else {
                look
            };
            let material = self.material(name.as_deref(), look);
            let mesh = self.meshes.add(batch.into_mesh());
            let child = commands.spawn((Mesh3d(mesh), MeshMaterial3d(material))).id();
            commands.entity(holder).add_child(child);
        }
        holder
    }

    pub fn spawn_track(&mut self, commands: &mut Commands, track: &str) -> Result<Scene> {
        let path = format!("/Levels/{track}/World/{track}.row");
        let world = World::parse(&self.game.read(&path).map_err(|e| anyhow::anyhow!("reading {path}: {e}"))?)?;
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
                self.spawn_model(commands, &model, root, t, Look::Baked);
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

        Ok(Scene { world, roots, world_meshes, props })
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
        let wheels: [(&str, &str, f32, &[usize]); 4] = [
            ("road_wheel_front_left", "flwheelcentre", 1.0, &[0]),
            ("road_wheel_front_left", "frwheelcentre", -1.0, &[1]),
            ("road_wheel_rear_left", "rwheelcentre", 1.0, &[2]),
            ("road_wheel_rear_right", "rwheelcentre", 1.0, &[3]),
        ];
        for (k, (w, pt, sx, idx)) in wheels.into_iter().enumerate() {
            if let Some(model) = self.model(w, "Cars/Generic") {
                let mut pos = point(pt).unwrap_or_default();
                if let Some(x) = wheel_x.get(k) {
                    pos.x = *x;
                }
                let t = Transform::from_translation(pos).with_scale(Vec3::new(sx, 1.0, 1.0));
                let e = self.spawn_model(commands, &model, root, t, Look::Lit);
                commands.entity(e).insert(WheelVisual { wheels: idx.to_vec(), base: pos, mirror: sx < 0.0 });
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

        let mut batches: HashMap<Option<usize>, Batch> = HashMap::new();
        for node in &model.nodes {
            batches.entry(node.material()).or_default().add_skinned(node, Look::Rider);
        }
        for (mat, batch) in batches {
            if batch.idx.is_empty() {
                continue;
            }
            let name = mat.and_then(|i| model.materials.get(i)).map(|m| m.name.clone());
            let material = self.material(name.as_deref(), Look::Rider);
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
        Ok(RiderRig { joints, offsets })
    }
}
