use std::f32::consts::{FRAC_PI_2, PI, TAU};

use bevy::camera::ScalingMode;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::camera::GameCamera;

pub const PIXELS_PER_UNIT: u32 = 60;
pub const CAMERA_DISTANCE: f32 = 30.0;

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
