//! Turns parsed game data into Bevy meshes, materials and textures.
//!
//! The Xbox data is left-handed (D3D); Bevy is right-handed. Every position
//! and normal gets its Z negated and every triangle's winding reversed.

use std::collections::HashMap;

use anyhow::Result;
use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler, ImageSamplerDescriptor, ImageAddressMode};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use atv_formats::bog::{TexFormat, Texture};
use atv_formats::rom::{Model, Patch, Vertex};
use atv_formats::row::World;
use atv_formats::GameSource;

const PATCH_STEPS: usize = 6;

pub struct Scene {
    pub world: World,
    pub world_meshes: usize,
    pub props: usize,
}

/// Distance from the model origin (ground under the wheels) up to the
/// physics centre of mass; WHEELOFFSET in the params is relative to it.
pub const COM_HEIGHT: f32 = 0.43;

/// A wheel (or rear axle half) that follows the simulation.
#[derive(Component)]
pub struct WheelVisual {
    /// Simulation wheel indices this visual averages (the rear axle is solid).
    pub wheels: Vec<usize>,
    pub base: Vec3,
    pub mirror: bool,
}

pub struct Loader<'a> {
    game: &'a GameSource,
    meshes: &'a mut Assets<Mesh>,
    materials: &'a mut Assets<StandardMaterial>,
    images: &'a mut Assets<Image>,
    /// Lower-case texture stem -> archive path, for name-based lookup.
    texture_paths: HashMap<String, String>,
    textures: HashMap<String, Option<(Handle<Image>, bool)>>,
    material_cache: HashMap<String, Handle<StandardMaterial>>,
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
}

impl Batch {
    fn add_tris(&mut self, verts: &[Vertex], indices: &[u32]) {
        let base = self.pos.len() as u32;
        for v in verts {
            self.pos.push(flip(v.pos));
            self.normal.push(flip(v.normal));
            self.uv.push(v.uv);
            self.color.push(argb(v.color));
        }
        for t in indices.chunks_exact(3) {
            self.idx.extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
        }
    }

    fn add_patch(&mut self, p: &Patch) {
        let (pos, uv, idx) = p.tessellate(PATCH_STEPS);
        let base = self.pos.len() as u32;
        let n = PATCH_STEPS + 1;
        let cols = p.corner_color.map(argb);
        for (k, (pp, tt)) in pos.iter().zip(&uv).enumerate() {
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

    fn into_mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normal)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.color)
            .with_inserted_indices(Indices::U32(self.idx))
    }
}

/// Baked shadow decals store darkness as low values against a light
/// background; turn that into a white-is-unshadowed multiply texture.
fn shadow_to_multiply(bgra: &mut [u8]) {
    let max = bgra.chunks_exact(4).map(|p| p[0].max(p[1]).max(p[2])).max().unwrap_or(0).max(1) as f32;
    for p in bgra.chunks_exact_mut(4) {
        let s = 1.0 - p[0].max(p[1]).max(p[2]) as f32 / max;
        let v = (255.0 * (1.0 - 0.6 * s)) as u8;
        p[..3].fill(v);
        p[3] = 255;
    }
}

fn flip(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[1], -p[2]]
}

/// 0xAARRGGBB -> linear RGBA. Colours are baked lighting where 0x80 means
/// "unlit", so they are doubled (the Xbox used MODULATE2X).
fn argb(c: u32) -> [f32; 4] {
    let ch = |s: u32| Color::srgb_u8(((c >> s) & 255) as u8, 0, 0).to_linear().red;
    let a = ((c >> 24) & 255) as f32 / 255.0;
    // Vertex alpha drives effects (tyre dirt blend etc.), not coverage.
    let _ = a;
    [ch(16) * 2.0, ch(8) * 2.0, ch(0) * 2.0, 1.0]
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
            material_cache: HashMap::new(),
            models: HashMap::new(),
            model_paths,
        }
    }

    fn texture(&mut self, name: &str) -> Option<(Handle<Image>, bool)> {
        let key = name.to_ascii_lowercase();
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let loaded = self.texture_paths.get(&key).cloned().and_then(|path| {
            let tex = Texture::parse(&self.game.read(&path).ok()?).ok()?;
            let mut tex = tex;
            let shadow = key.ends_with("_shad");
            if shadow && tex.format == TexFormat::Bgra8 {
                shadow_to_multiply(&mut tex.data);
            }
            let (format, alpha) = match tex.format {
                TexFormat::Dxt1 => (TextureFormat::Bc1RgbaUnormSrgb, false),
                TexFormat::Dxt5 => (TextureFormat::Bc3RgbaUnormSrgb, true),
                TexFormat::Bgra8 => (TextureFormat::Bgra8UnormSrgb, true),
            };
            // Block-compressed mips must be at least 4x4 for wgpu validation.
            let mut mips = 0;
            let mut size = 0;
            while mips < tex.mip_count {
                let (w, h) = (tex.width >> mips, tex.height >> mips);
                // Bevy only takes a single level for uncompressed images.
                if (tex.format == TexFormat::Bgra8 && mips == 1) || w < 4 || h < 4 {
                    break;
                }
                size += tex.level_size(mips);
                mips += 1;
            }
            let mut img = Image::new(
                Extent3d { width: tex.width, height: tex.height, depth_or_array_layers: 1 },
                TextureDimension::D2,
                tex.data[..size].to_vec(),
                format,
                RenderAssetUsages::RENDER_WORLD,
            );
            img.texture_descriptor.mip_level_count = mips.max(1);
            img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::linear()
            });
            Some((self.images.add(img), if shadow { false } else { alpha }))
        });
        if loaded.is_none() {
            warn!("texture not found: {name}");
        }
        self.textures.insert(key, loaded.clone());
        loaded
    }

    fn material(&mut self, name: Option<&str>) -> Handle<StandardMaterial> {
        let key = name.unwrap_or("").to_ascii_lowercase();
        if let Some(m) = self.material_cache.get(&key) {
            return m.clone();
        }
        let tex = name.and_then(|n| self.texture(n));
        let m = self.materials.add(StandardMaterial {
            base_color_texture: tex.as_ref().map(|t| t.0.clone()),
            unlit: true,
            cull_mode: None,
            double_sided: true,
            alpha_mode: if key.ends_with("_shad") {
                AlphaMode::Multiply
            } else if tex.as_ref().is_some_and(|t| t.1) {
                AlphaMode::Mask(0.5)
            } else {
                AlphaMode::Opaque
            },
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
    fn spawn_model(&mut self, commands: &mut Commands, model: &Model, parent: Entity, transform: Transform) -> Entity {
        let mut batches: HashMap<Option<usize>, Batch> = HashMap::new();
        for node in &model.nodes {
            let b = batches.entry(node.material()).or_default();
            b.add_tris(&node.vertices, &node.indices);
            for p in &node.patches {
                b.add_patch(p);
            }
        }
        let holder = commands.spawn((transform, Visibility::default())).id();
        commands.entity(parent).add_child(holder);
        for (mat, batch) in batches {
            if batch.idx.is_empty() {
                continue;
            }
            let name = mat.and_then(|i| model.materials.get(i)).map(|m| m.name.clone());
            let material = self.material(name.as_deref());
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

        let mut batches: HashMap<i32, Batch> = HashMap::new();
        for m in &world.meshes {
            let b = batches.entry(m.material).or_default();
            b.add_tris(&m.vertices, &m.indices);
            for p in &m.patches {
                b.add_patch(p);
            }
        }
        let world_meshes = batches.len();

        for (mat, batch) in batches {
            let name = usize::try_from(mat).ok().and_then(|i| world.materials.get(i)).cloned();
            let material = self.material(name.as_deref());
            let mesh = self.meshes.add(batch.into_mesh());
            let e = commands.spawn((Mesh3d(mesh), MeshMaterial3d(material))).id();
            commands.entity(root).add_child(e);
        }

        let mut props = 0;
        for inst in &world.instances {
            let stem = inst.model.trim_end_matches(".clm").trim_end_matches(".CLM");
            if let Some(model) = self.model(stem, &env) {
                let t = mat_from_xbox(&inst.rotation, inst.scale, inst.pos);
                self.spawn_model(commands, &model, root, t);
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
        if let Some(model) = self.model(sky, "SkyDomes") {
            self.spawn_model(commands, &model, root, Transform::from_xyz(0.0, -200.0, 0.0));
        }

        Ok(Scene { world, world_meshes, props })
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
        self.spawn_model(commands, &body, root, Transform::default());
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
            self.spawn_model(commands, &model, root, Transform::from_translation(pos));
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
                let e = self.spawn_model(commands, &model, root, t);
                commands.entity(e).insert(WheelVisual { wheels: idx.to_vec(), base: pos, mirror: sx < 0.0 });
            }
        }
        Ok(root_entity)
    }
}
