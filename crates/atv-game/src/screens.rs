//! Stadium 1's video screens. Their material is `FrontBuffer`, which the
//! original binds to the console's front buffer (mesh setup 0x106390
//! clamps it and scales its UVs to the 640x480 buffer), so the screens show
//! the race as the player sees it, a frame late. Here a second camera
//! follows player 1's view and draws into the image the screens use.

use bevy::camera::RenderTarget;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::transform::TransformSystems;
use bevy::ui::IsDefaultUiCamera;

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, follow_view.after(crate::CameraTakeover).before(TransformSystems::Propagate));
    }
}

/// The front buffer's size: the original's 640x480 at half resolution.
const SIZE: (u32, u32) = (320, 240);

/// The camera drawing what the screens show.
#[derive(Component)]
pub struct ScreenCam;

/// The image the screens show, drawn into by a [`ScreenCam`].
pub fn image() -> Image {
    let mut img = Image::new_target_texture(SIZE.0, SIZE.1, TextureFormat::Rgba8UnormSrgb, None);
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        ..ImageSamplerDescriptor::linear()
    });
    img
}

/// A camera like player 1's `view`, drawing into `target` before the
/// views on the window.
pub fn spawn(commands: &mut Commands, view: impl Bundle, target: Handle<Image>, at: Transform, scoped: impl Bundle) {
    commands.spawn((view, Camera { order: -1, ..default() }, RenderTarget::Image(target.into()), at, ScreenCam, scoped));
}

/// Keeps the screens' camera where player 1's view is.
fn follow_view(main: Query<&Transform, (With<Camera3d>, With<IsDefaultUiCamera>, Without<ScreenCam>)>, mut screens: Query<&mut Transform, With<ScreenCam>>) {
    let Ok(view) = main.single() else { return };
    for mut t in &mut screens {
        if *t != *view {
            *t = *view;
        }
    }
}
