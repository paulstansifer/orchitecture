use std::f32::consts::{FRAC_PI_2, PI, TAU};

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, ScalingMode};
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use bevy::window::PrimaryWindow;
use bevy_egui::EguiContext;

use crate::camera::GameCamera;

pub const PIXELS_PER_UNIT: u32 = 60;
pub const CAMERA_DISTANCE: f32 = 30.0;

/// Render layer used exclusively by the pixel-perfect canvas sprite, so it doesn't get
/// drawn twice (once by itself, once by whichever other camera is active).
const PIXEL_CANVAS_LAYER: usize = 2;

/// Handle to the low-resolution render target that `GameCamera` draws into while in
/// walk mode, plus the entities that upscale it back onto the window with nearest-
/// neighbor sampling (2x2 blocks). See `pixel_grid_snap.rs` in the Bevy examples for
/// the technique this is based on.
#[derive(Resource)]
pub struct PixelCanvas {
    pub image: Handle<Image>,
    pub camera: Entity,
    pub sprite: Entity,
}

/// Startup system: creates the low-resolution canvas image and the camera/sprite pair
/// that upscale it. Both start inactive/hidden; `main.rs` activates them on entering
/// walk mode and deactivates them on leaving it, so build mode is unaffected.
///
/// The camera also gets its own (initially non-primary) `EguiContext`: WebGL can only
/// have one *active* camera targeting the real window at a time, so `main.rs` hands the
/// `PrimaryEguiContext` marker off to this camera whenever it (rather than `GameCamera`)
/// is the one facing the window, instead of using a separate always-on UI camera.
pub fn spawn_pixel_canvas(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    // Rgba8, not Bgra8: WebGL (used on wasm) can't unpack BGRA textures, and a failed
    // upload there was tainting the whole frame (dark gray view, stale UI artifacts).
    let mut canvas = Image::new_target_texture(1, 1, TextureFormat::Rgba8UnormSrgb, None);
    canvas.sampler = ImageSampler::nearest();
    let image = images.add(canvas);

    let sprite = commands
        .spawn((
            Sprite::from_image(image.clone()),
            Visibility::Hidden,
            RenderLayers::layer(PIXEL_CANVAS_LAYER),
        ))
        .id();

    let camera = commands
        .spawn((
            Camera2d,
            Camera {
                is_active: false,
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                // Bevy sorts same-order cameras only by `(order, target)`, with no
                // awareness that this camera's sprite samples GameCamera's Image
                // target — so without an explicit higher order than GameCamera's
                // (0), the two could be submitted in either order and this camera
                // would sometimes render before GameCamera has refreshed the canvas
                // for the frame, showing a stale (initially black) texture.
                order: 1,
                ..default()
            },
            Msaa::Off,
            RenderLayers::layer(PIXEL_CANVAS_LAYER),
            EguiContext::default(),
        ))
        .id();

    commands.insert_resource(PixelCanvas {
        image,
        camera,
        sprite,
    });
}

/// Keeps the canvas image sized at half the window's resolution (rounding down), and
/// the sprite that displays it scaled back up 2x, so each canvas texel covers a 2x2
/// block of screen pixels. Runs every frame regardless of game mode; cheap when the
/// window size hasn't changed.
pub fn resize_pixel_canvas_system(
    windows: Query<&Window, With<PrimaryWindow>>,
    pixel_canvas: Res<PixelCanvas>,
    mut images: ResMut<Assets<Image>>,
    mut sprite_q: Query<&mut Sprite>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let width = ((window.width() as u32) / 2).max(1);
    let height = ((window.height() as u32) / 2).max(1);

    if let Some(image) = images.get_mut(&pixel_canvas.image) {
        let size = image.texture_descriptor.size;
        if size.width != width || size.height != height {
            image.resize(Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            });
        }
    }

    if let Ok(mut sprite) = sprite_q.get_mut(pixel_canvas.sprite) {
        sprite.custom_size = Some(Vec2::new((width * 2) as f32, (height * 2) as f32));
    }
}

/// Trimetric camera basis vectors.
///   +X world → upper-left at 45° (1:1), +Z world → upper-right at 1:2, +Y → straight up.
pub fn trimetric_camera_basis() -> (Vec3, Vec3, Vec3) {
    let cam_r = Vec3::new(-1.0 / 3f32.sqrt(), 0.0, 2.0 / 6f32.sqrt());
    let cam_u = Vec3::new(1.0 / 3f32.sqrt(), 1.0 / 2f32.sqrt(), 1.0 / 6f32.sqrt());
    let cam_forward = cam_u.cross(cam_r);
    (cam_r, cam_u, cam_forward)
}

/// The XZ projection of cam_u; the screen-up direction on the ground plane.
/// Orthogonal to cam_r; |cam_fwd_xz|² = 1/2.
pub fn cam_fwd_xz_base() -> Vec3 {
    Vec3::new(1.0 / 3f32.sqrt(), 0.0, 1.0 / 6f32.sqrt())
}

/// Snap a world position so its screen-space projection lands on whole pixels.
///
/// Since cam_r and cam_fwd_xz are orthogonal with |cam_r|²=1 and |cam_fwd_xz|²=1/2,
/// decomposition uses: a = dot(p, cam_r), b = dot(p, cam_fwd_xz) * 2.
pub fn snap_to_pixel(pos: Vec3, camera_direction: u8) -> Vec3 {
    let (cam_r_base, _, _) = trimetric_camera_basis();
    let rot = Quat::from_rotation_y(camera_direction as f32 * FRAC_PI_2);
    let cam_r = rot * cam_r_base;
    let cam_fwd_xz = rot * cam_fwd_xz_base();
    let p = Vec3::new(pos.x, 0.0, pos.z);
    let ppu = PIXELS_PER_UNIT as f32;
    let a = (p.dot(cam_r) * ppu).round() / ppu;
    let b = (p.dot(cam_fwd_xz) * 2.0 * ppu).round() / ppu;
    let snapped = a * cam_r + b * cam_fwd_xz;
    Vec3::new(snapped.x, pos.y, snapped.z)
}

#[derive(Resource)]
pub struct WalkCameraState {
    pub target_position: Vec3,
    /// Which of the 4 cardinal faces the camera shows (0–3, each ×90°).
    pub camera_direction: u8,
    /// Current rendered angle (radians); animates linearly toward `camera_direction * FRAC_PI_2`.
    pub current_display_angle: f32,
    /// Accumulated rotation from the current right-drag gesture (radians).
    pub requested_direction: f32,
    pub is_right_dragging: bool,
    /// Total screen-space drag delta for the current gesture; used by walk_ui.
    pub drag_delta: Vec2,
}

impl Default for WalkCameraState {
    fn default() -> Self {
        WalkCameraState {
            target_position: Vec3::ZERO,
            camera_direction: 0,
            current_display_angle: 0.0,
            requested_direction: 0.0,
            is_right_dragging: false,
            drag_delta: Vec2::ZERO,
        }
    }
}

pub fn walk_camera_system(
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut walk_state: ResMut<WalkCameraState>,
    mut camera_q: Query<(&mut Transform, &mut Projection), With<GameCamera>>,
) {
    let Ok((mut transform, mut projection)) = camera_q.single_mut() else {
        return;
    };

    // Animate current_display_angle linearly toward the committed camera_direction.
    let target_angle = walk_state.camera_direction as f32 * FRAC_PI_2;
    let mut diff = (target_angle - walk_state.current_display_angle).rem_euclid(TAU);
    if diff > PI {
        diff -= TAU; // take the shorter arc
    }
    const ROTATION_SPEED: f32 = FRAC_PI_2 / 0.1; // 90° in 0.1s, linear
    let step = diff.signum() * (ROTATION_SPEED * time.delta_secs()).min(diff.abs());
    walk_state.current_display_angle += step;

    let window_height = windows.single().map(|w| w.height()).unwrap_or(600.0);
    let viewport_height = window_height / PIXELS_PER_UNIT as f32;

    *projection = Projection::Orthographic(OrthographicProjection {
        scaling_mode: ScalingMode::FixedVertical { viewport_height },
        ..OrthographicProjection::default_3d()
    });

    let (cam_r_base, cam_u_base, _) = trimetric_camera_basis();
    let rot = Quat::from_rotation_y(walk_state.current_display_angle);
    let cam_r = rot * cam_r_base;
    let cam_u = rot * cam_u_base;
    let cam_forward = cam_u.cross(cam_r);

    let snapped = snap_to_pixel(walk_state.target_position, walk_state.camera_direction);
    let translation = snapped - cam_forward * CAMERA_DISTANCE;
    transform.translation = translation;
    transform.rotation = Quat::from_mat3(&Mat3::from_cols(cam_r, cam_u, cam_r.cross(cam_u)));
}
