//! Render resolution: the 3D scene (world + weapon) draws into an offscreen
//! image of the chosen height (e.g. 720p, window aspect ratio) and is stretched
//! to the window with nearest-neighbour filtering.
//! The UI stays crisp because egui lives on a separate camera that draws to the
//! window itself.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ImageRenderTarget, RenderTarget};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::window::{PrimaryWindow, WindowRef};
use bevy_egui::PrimaryEguiContext;

use crate::settings::Settings;
use crate::viewmodel::ViewModelCam;
use crate::world::MainCamera;

const BLIT_LAYER: usize = 2;

#[derive(Resource)]
pub struct SceneImage(pub Handle<Image>);

#[derive(Component)]
pub struct Blit;

/// Spawns the window-facing camera (hosts egui) and the sprite showing the scene.
pub fn setup_blit(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut img = Image::new_target_texture(16, 16, TextureFormat::Rgba8UnormSrgb, None);
    img.sampler = bevy::image::ImageSampler::nearest();
    let handle = images.add(img);
    commands.insert_resource(SceneImage(handle.clone()));
    commands.spawn((
        Camera2d,
        Camera { order: 10, clear_color: ClearColorConfig::None, ..default() },
        RenderLayers::layer(BLIT_LAYER),
        PrimaryEguiContext,
    ));
    commands.spawn((
        Sprite::from_image(handle),
        RenderLayers::layer(BLIT_LAYER),
        Visibility::Hidden,
        Blit,
    ));
}

pub fn apply_render_scale(
    settings: Res<Settings>,
    scene: Res<SceneImage>,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut cams: Query<&mut RenderTarget, Or<(With<MainCamera>, With<ViewModelCam>)>>,
    mut blit: Single<(&mut Visibility, &mut Sprite), With<Blit>>,
) {
    let (pw, ph) = (window.physical_width() as f32, window.physical_height() as f32);
    let want_image = settings.render_height != 0 && (settings.render_height as f32) < ph;
    for mut t in &mut cams {
        let is_image = matches!(*t, RenderTarget::Image(_));
        if want_image != is_image {
            *t = if want_image {
                RenderTarget::Image(ImageRenderTarget { handle: scene.0.clone(), scale_factor: 1.0 })
            } else {
                RenderTarget::Window(WindowRef::Primary)
            };
        }
    }
    let (vis, sprite) = &mut *blit;
    let v = if want_image { Visibility::Inherited } else { Visibility::Hidden };
    if **vis != v {
        **vis = v;
    }
    if !want_image {
        return;
    }
    let rh = settings.render_height as f32;
    let size = UVec2::new((rh * pw / ph).round().max(16.0) as u32, rh as u32);
    if let Some(img) = images.get(&scene.0) {
        if img.size() != size {
            if let Some(mut img) = images.get_mut(&scene.0) {
                img.resize(bevy::render::render_resource::Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 });
            }
        }
    }
    let logical = Vec2::new(window.width(), window.height());
    if sprite.custom_size != Some(logical) {
        sprite.custom_size = Some(logical);
    }
}
