use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::camera::track_cursor_delta;
use crate::ortho_camera::{
    cam_fwd_xz_base, snap_to_pixel, trimetric_camera_basis, WalkCameraState,
};

pub fn walk_input_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
    mut walk_state: ResMut<WalkCameraState>,
    mut last_cursor: Local<Option<Vec2>>,
) {
    let dt = time.delta_secs();

    // Track cursor position for drag delta.
    let cursor_delta = track_cursor_delta(&windows, &mut last_cursor);

    // WASD movement, screen-relative (W = up-on-screen = into the trimetric view).
    let (cam_r_base, _, _) = trimetric_camera_basis();
    let cam_fwd = cam_fwd_xz_base();
    let rot = Quat::from_rotation_y(walk_state.camera_direction as f32 * FRAC_PI_2);
    let cam_r = rot * cam_r_base;
    let cam_fwd_xz = rot * cam_fwd;

    let speed = 8.0;
    if keyboard.pressed(KeyCode::KeyW) {
        walk_state.target_position += cam_fwd_xz * speed * dt;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        walk_state.target_position -= cam_fwd_xz * speed * dt;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        walk_state.target_position -= cam_r * speed * dt;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        walk_state.target_position += cam_r * speed * dt;
    }

    walk_state.target_position =
        snap_to_pixel(walk_state.target_position, walk_state.camera_direction);

    // Right-drag rotation.
    if mouse_button.just_pressed(MouseButton::Right) {
        walk_state.is_right_dragging = true;
        walk_state.requested_direction = walk_state.camera_direction as f32 * FRAC_PI_2;
        walk_state.drag_delta = Vec2::ZERO;
    }

    if walk_state.is_right_dragging && mouse_button.pressed(MouseButton::Right) {
        walk_state.requested_direction -= cursor_delta.x * 0.005;
        walk_state.drag_delta += cursor_delta;
    }

    if mouse_button.just_released(MouseButton::Right) && walk_state.is_right_dragging {
        let snapped = (walk_state.requested_direction / FRAC_PI_2).round() as i32;
        walk_state.camera_direction = snapped.rem_euclid(4) as u8;
        walk_state.is_right_dragging = false;
        walk_state.drag_delta = Vec2::ZERO;
    }
}
