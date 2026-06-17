use std::f32::consts::TAU;

use bevy::input::mouse::{AccumulatedMouseScroll, MouseButton, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::input::EguiWantsInput;

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
            target_position: Vec3::new(0.0, 0.0, -5.0),
            target_yaw: 30.0,
            target_pitch: 0.7,
            target_dist: 22.0,
        }
    }
}

#[derive(Component)]
pub struct GameCamera;

/// Startup system: spawns the main 3D camera.
pub fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(55.0, 55.0, 25.0).looking_at(Vec3::ZERO, Vec3::Y),
        GameCamera,
    ));
}

pub fn camera_input_system(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    egui_wants_input: Res<EguiWantsInput>,
    mut last_cursor: Local<Option<Vec2>>,
    mut state: ResMut<CameraState>,
    mut camera_q: Query<(&mut Transform, &mut Projection), With<GameCamera>>,
) {
    let dt = time.delta_secs();

    // Track cursor delta for right-mouse drag.
    let cursor = windows.single().map(|w| w.cursor_position()).ok().flatten();
    let cursor_delta = cursor
        .zip(*last_cursor)
        .map(|(now, prev)| now - prev)
        .unwrap_or(Vec2::ZERO);
    *last_cursor = cursor;

    if mouse_button.pressed(MouseButton::Right) {
        state.target_yaw -= cursor_delta.x * 0.005;
        state.target_pitch =
            (state.target_pitch + cursor_delta.y * 0.005).clamp(0.05, TAU / 4.0 - 0.05);
    }

    let shift = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);
    // `absorb_bevy_input_system` doesn't seem to work in all cases for mouse wheels
    if !shift && mouse_scroll.delta.y != 0.0 && !egui_wants_input.wants_any_pointer_input() {
        // Browsers report wheel deltas in pixels (~100/notch); native reports lines (~1/notch).
        let scroll_y = match mouse_scroll.unit {
            MouseScrollUnit::Line => mouse_scroll.delta.y,
            MouseScrollUnit::Pixel => mouse_scroll.delta.y / 100.0,
        };
        state.target_dist = (state.target_dist * 0.9_f32.powf(scroll_y)).clamp(5.0, 200.0);
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

    let Ok((mut transform, mut projection)) = camera_q.single_mut() else {
        return;
    };

    // Ensure the build camera stays perspective (walk mode may have switched it).
    if !matches!(*projection, Projection::Perspective(_)) {
        *projection = Projection::Perspective(PerspectiveProjection::default());
    }

    let pitch = state.target_pitch;
    let yaw = state.target_yaw;
    let dist = state.target_dist;
    let offset = Vec3::new(
        yaw.sin() * pitch.cos() * dist,
        pitch.sin() * dist,
        yaw.cos() * pitch.cos() * dist,
    );
    let desired_pos = state.target_position + offset;
    let desired =
        Transform::from_translation(desired_pos).looking_at(state.target_position, Vec3::Y);

    let lerp_pos = (8.0 * dt).min(1.0);
    let lerp_rot = (6.0 * dt).min(1.0);
    transform.translation = transform.translation.lerp(desired.translation, lerp_pos);
    transform.rotation = transform.rotation.slerp(desired.rotation, lerp_rot);
}
