use std::f32::consts::TAU;

use bevy::input::mouse::MouseButton;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

#[derive(Resource)]
pub struct CameraState {
    pub target_position: Vec3,
    pub target_yaw: f32,
    pub target_pitch: f32,
    pub target_dist: f32,
}

impl Default for CameraState {
    fn default() -> Self {
        CameraState {
            target_position: Vec3::ZERO,
            target_yaw: 0.0,
            target_pitch: 0.7,
            target_dist: 30.0,
        }
    }
}

#[derive(Component)]
pub struct GameCamera;

pub fn camera_input_system(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut last_cursor: Local<Option<Vec2>>,
    mut state: ResMut<CameraState>,
    mut camera_q: Query<&mut Transform, With<GameCamera>>,
) {
    let dt = time.delta_secs();

    // Track cursor delta for middle-mouse drag.
    let cursor = windows.single().map(|w| w.cursor_position()).ok().flatten();
    let cursor_delta = cursor
        .zip(*last_cursor)
        .map(|(now, prev)| now - prev)
        .unwrap_or(Vec2::ZERO);
    *last_cursor = cursor;

    if mouse_button.pressed(MouseButton::Middle) {
        state.target_yaw -= cursor_delta.x * 0.005;
        state.target_pitch = (state.target_pitch + cursor_delta.y * 0.005)
            .clamp(0.05, TAU / 4.0 - 0.05);
        state.target_dist = (state.target_dist + cursor_delta.y * 0.1).clamp(5.0, 200.0);
    }

    // WASD pan.
    let speed = 8.0;
    let forward = Vec3::new(-state.target_yaw.sin(), 0.0, -state.target_yaw.cos());
    let right = Vec3::new(state.target_yaw.cos(), 0.0, -state.target_yaw.sin());

    if keyboard.pressed(KeyCode::KeyW) {
        state.target_position += forward * speed * dt;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        state.target_position -= forward * speed * dt;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        state.target_position -= right * speed * dt;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        state.target_position += right * speed * dt;
    }
    if keyboard.pressed(KeyCode::Space) {
        state.target_position = Vec3::ZERO;
    }

    let Ok(mut transform) = camera_q.single_mut() else {
        return;
    };

    let pitch = state.target_pitch;
    let yaw = state.target_yaw;
    let dist = state.target_dist;
    let offset = Vec3::new(
        yaw.sin() * pitch.cos() * dist,
        pitch.sin() * dist,
        yaw.cos() * pitch.cos() * dist,
    );
    let desired_pos = state.target_position + offset;
    let desired = Transform::from_translation(desired_pos)
        .looking_at(state.target_position, Vec3::Y);

    let lerp_pos = (8.0 * dt).min(1.0);
    let lerp_rot = (6.0 * dt).min(1.0);
    transform.translation = transform.translation.lerp(desired.translation, lerp_pos);
    transform.rotation = transform.rotation.slerp(desired.rotation, lerp_rot);
}
