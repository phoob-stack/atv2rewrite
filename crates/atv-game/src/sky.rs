//! The sky dome and how far the race views see.
//!
//! The original stops drawing the track at its `mDrawDistance`
//! (`Tracks.xml`; the camera's far plane, 0x4d4310) and fogs it between
//! `mFogStartDistance` and `mFogEndDistance`, fractions of that distance
//! (vertex constant c90, set by 0x4d3fc0). The sky is drawn first with a
//! far plane of 5000 (0x481ed0), so it shows past the track. The dome is
//! scaled to the draw distance and centred `mSkyYOffset` below the camera
//! (0x481ed0), so its horizon sits under the view's. Here the sky dome has a
//! camera of its own behind each race view: it sits as far above the
//! dome's centre, turns with the view, and draws only the dome's layer.

use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use atv_formats::{setup, GameSource};

use crate::convert::{SkyDome, SKY_RADIUS};
use crate::screens::ScreenCam;

/// The render layer the sky dome is on.
const SKY_LAYER: usize = 1;

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (layer_sky, add_sky_cams)).add_systems(PostUpdate, sync_sky_cams.before(TransformSystems::Propagate));
    }
}

/// How far a track's views see: the far plane and the fog's start and end,
/// in metres, and how high above the sky dome's centre the views sit, as a
/// fraction of its radius.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct ViewDistance {
    pub far: f32,
    pub fog_start: f32,
    pub fog_end: f32,
    pub sky_lift: f32,
}

impl ViewDistance {
    pub fn for_track(game: &GameSource, track: &str) -> Self {
        let xml = game.read("/Setup/Tracks.xml").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let entry = setup::entries(&xml).into_iter().find(|e| e.name.eq_ignore_ascii_case(track));
        let num = |key: &str| entry.as_ref().and_then(|e| e.num(key));
        Self::new(num("mDrawDistance"), num("mFogStartDistance"), num("mFogEndDistance"), num("mSkyYOffset"))
    }

    fn new(draw: Option<f32>, start: Option<f32>, end: Option<f32>, sky_y: Option<f32>) -> Self {
        let far = draw.filter(|d| *d > 1.0).unwrap_or(600.0);
        let fog_end = end.unwrap_or(1.0) * far;
        let sky_lift = (-sky_y.unwrap_or(-120.0) / far).clamp(0.0, 0.9);
        Self { far, fog_start: (start.unwrap_or(0.5) * far).min(fog_end - 1.0), fog_end, sky_lift }
    }
}

/// A track's fog colour, which its race views also clear to: `Tracks.xml`
/// `mFogColour`, which the race camera packs into its `rFogColour` (0x46e020)
/// and sets as the fog and clear colour every frame (0x4d90c0). The world
/// file's header carries a colour too, but the race camera doesn't take it.
pub fn fog_colour(game: &GameSource, track: &str) -> Option<Color> {
    fog_colour_in(&String::from_utf8_lossy(&game.read("/Setup/Tracks.xml").ok()?), track)
}

fn fog_colour_in(xml: &str, track: &str) -> Option<Color> {
    let c = setup::child(xml, track, "mFogColour")?;
    let ch = |k: &str| c.get(k).and_then(|v| v.trim().parse::<f32>().ok()).map(|v| v.clamp(0.0, 1.0));
    Some(Color::srgb(ch("R")?, ch("G")?, ch("B")?))
}

/// A race view's sky camera.
#[derive(Component)]
struct SkyCam(Entity);

/// A race view that has its sky camera.
#[derive(Component)]
struct HasSkyCam;

/// Puts the sky dome's meshes on the sky layer.
fn layer_sky(mut commands: Commands, domes: Query<Entity, With<SkyDome>>, children: Query<&Children>, layered: Query<(), With<RenderLayers>>) {
    for dome in &domes {
        for e in std::iter::once(dome).chain(children.iter_descendants(dome)) {
            if !layered.contains(e) {
                commands.entity(e).insert(RenderLayers::layer(SKY_LAYER));
            }
        }
    }
}

/// Gives each race view a sky camera, which clears the view's area in its
/// place.
#[allow(clippy::type_complexity)]
fn add_sky_cams(
    mut commands: Commands,
    domes: Query<(), With<SkyDome>>,
    mut views: Query<(Entity, &mut Camera), (With<Camera3d>, Without<SkyCam>, Without<HasSkyCam>, Without<ScreenCam>)>,
) {
    if domes.is_empty() {
        return;
    }
    for (view, mut cam) in &mut views {
        let clear = std::mem::replace(&mut cam.clear_color, ClearColorConfig::None);
        commands.spawn((
            Camera3d::default(),
            Camera { order: cam.order - 100, clear_color: clear, viewport: cam.viewport.clone(), is_active: cam.is_active, ..default() },
            Tonemapping::None,
            Projection::Perspective(PerspectiveProjection { far: SKY_RADIUS * 2.0, ..default() }),
            Transform::default(),
            RenderLayers::layer(SKY_LAYER),
            SkyCam(view),
        ));
        commands.entity(view).insert(HasSkyCam);
    }
}

/// Keeps each sky camera at the dome's centre, turned and framed like its
/// view; drops it with its view.
#[allow(clippy::type_complexity)]
fn sync_sky_cams(
    mut commands: Commands,
    distance: Option<Res<ViewDistance>>,
    views: Query<(&Camera, &Transform, &Projection), (With<HasSkyCam>, Without<SkyCam>)>,
    mut skies: Query<(Entity, &SkyCam, &mut Camera, &mut Transform, &mut Projection)>,
) {
    let lift = distance.map_or(0.2, |d| d.sky_lift);
    for (e, sky, mut cam, mut t, mut proj) in &mut skies {
        let Ok((view, view_t, view_proj)) = views.get(sky.0) else {
            commands.entity(e).despawn();
            continue;
        };
        let rect = |v: &Option<bevy::camera::Viewport>| v.as_ref().map(|v| (v.physical_position, v.physical_size));
        if rect(&cam.viewport) != rect(&view.viewport) {
            cam.viewport = view.viewport.clone();
        }
        if cam.is_active != view.is_active {
            cam.is_active = view.is_active;
        }
        *t = Transform::from_translation(Vec3::Y * SKY_RADIUS * lift).with_rotation(view_t.rotation);
        if let (Projection::Perspective(p), Projection::Perspective(v)) = (proj.as_mut(), view_proj) {
            if p.fov != v.fov {
                p.fov = v.fov;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fog_is_a_fraction_of_the_draw_distance() {
        // Dunes 1: 700, 0.5, 1.0, sky 200 below.
        let d = ViewDistance::new(Some(700.0), Some(0.5), Some(1.0), Some(-200.0));
        assert_eq!((d.far, d.fog_start, d.fog_end), (700.0, 350.0, 700.0));
        assert!((d.sky_lift - 200.0 / 700.0).abs() < 1e-6);
        // Glacier: fog from behind the camera.
        let g = ViewDistance::new(Some(600.0), Some(-0.1), Some(0.45), Some(-120.0));
        assert!((g.fog_start + 60.0).abs() < 1e-3 && (g.fog_end - 270.0).abs() < 1e-3);
        assert!((g.sky_lift - 0.2).abs() < 1e-6);
        assert_eq!(ViewDistance::new(None, None, None, None).far, 600.0);
    }

    #[test]
    fn fog_colour_is_the_tracks_xml_one() {
        let xml = r#"<Tracks><Swamp1 mDrawDistance="450.0">
      <mAmbientColour A="1.0" R="0.25" G="0.25" B="0.25"/>
      <mFogColour A="1.0" R="0.7" G="0.7" B="0.75"/>
   </Swamp1><Swamp2 mDrawDistance="450.0"></Swamp2></Tracks>"#;
        assert_eq!(fog_colour_in(xml, "Swamp1"), Some(Color::srgb(0.7, 0.7, 0.75)));
        assert_eq!(fog_colour_in(xml, "Swamp2"), None);
    }
}
