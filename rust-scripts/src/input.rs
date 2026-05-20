use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::camera::GameCamera;
use crate::structure::StructureList;
use crate::wall_grid::{cell_transform, GridCellMarker, WallGrid};

/// Marker for the wall/floor build cursor (pin shape).
#[derive(Component)]
pub struct WallCursorMarker;

/// Marker for the room-plop build cursor (flat disc).
#[derive(Component)]
pub struct RoomCursorMarker;

pub fn cursor_system(
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    build_state: Res<BuildState>,
    wall_grid: Res<WallGrid>,
    mut wall_q: Query<
        (&mut Transform, &mut Visibility),
        (With<WallCursorMarker>, Without<RoomCursorMarker>),
    >,
    mut room_q: Query<
        (&mut Transform, &mut Visibility),
        (With<RoomCursorMarker>, Without<WallCursorMarker>),
    >,
) {
    let id = build_state.selected_structure as i32;
    let is_room = wall_grid.structure_is_room_plop(id);
    let maybe_pos = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32);
    let y = build_state.cur_y as f32;

    if let Ok((mut t, mut vis)) = wall_q.single_mut() {
        match (!is_room).then_some(maybe_pos).flatten() {
            Some(pos) => {
                let s = pos.round();
                t.translation = Vec3::new(s.x, y, s.z);
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }

    if let Ok((mut t, mut vis)) = room_q.single_mut() {
        match is_room.then_some(maybe_pos).flatten() {
            Some(pos) => {
                let s = pos.round();
                t.translation = Vec3::new(s.x + 0.5, y, s.z + 0.5);
                t.rotation =
                    Quat::from_rotation_y(build_state.cur_dir as f32 * std::f32::consts::TAU / 4.0);
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

/// Shared game state for the build tool.
#[derive(Resource, Default)]
pub struct BuildState {
    pub selected_structure: usize,
    pub cur_dir: u8,
    pub cur_y: i32,
    pub drag_start: Option<Vec3>,
    /// Latest evaluation results (coherence, interest).
    pub evaluation: Option<(f32, f32)>,
    /// Cursor world position on the cur_y plane; updated each frame.
    pub focus_pos: Vec3,
}

/// Applies a list of cell changes to the world: despawns old entities, spawns new ones.
pub fn apply_changes(
    commands: &mut Commands,
    wall_grid: &mut WallGrid,
    structure_list: &StructureList,
    changes: Vec<(
        crate::sparse3d::SlotLocation,
        Option<crate::wall_grid::Cell>,
    )>,
) {
    for (loc, new_cell) in changes {
        // Despawn existing entity at this location (if any).
        if let Some(old_entity) = wall_grid.cell_entities.remove(&loc) {
            commands.entity(old_entity).despawn();
        }
        // Spawn new entity if placing a cell.
        if let Some(cell) = new_cell {
            let transform = cell_transform(loc.rel_slot, cell.facing, loc.cube);
            let handle = structure_list.scene_handle(cell.id).clone();
            let entity = commands
                .spawn((SceneRoot(handle), transform, GridCellMarker { loc }))
                .id();
            wall_grid.cell_entities.insert(loc, entity);
        }
    }
}

pub fn building_input_system(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    mut wall_grid: ResMut<WallGrid>,
    structure_list: Res<StructureList>,
    mut build_state: ResMut<BuildState>,
) {
    // Keep focus_pos current so update_visibility_system uses the right spot.
    let y = build_state.cur_y as f32;
    if let Some(pos) = cursor_world_pos(&windows, &camera_q, y) {
        build_state.focus_pos = pos;
    } else {
        build_state.focus_pos.y = y;
    }

    // --- Layer up/down ---
    if keyboard.just_pressed(KeyCode::ArrowUp) {
        build_state.cur_y = (build_state.cur_y + 1).min(10);
    }
    if keyboard.just_pressed(KeyCode::ArrowDown) {
        build_state.cur_y = (build_state.cur_y - 1).max(0);
    }

    // --- Rotation ---
    if keyboard.just_pressed(KeyCode::KeyR) {
        build_state.cur_dir = (build_state.cur_dir + 1) % 4;
    }

    // --- Undo ---
    if keyboard.just_pressed(KeyCode::KeyZ) {
        let changes = wall_grid.undo();
        apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
    }

    // --- Evaluate (V key) ---
    if keyboard.just_pressed(KeyCode::KeyV) {
        if let Some(world_pos) = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32) {
            let metrics = wall_grid.metrics_at(world_pos);
            if metrics.len() >= 2 {
                build_state.evaluation = Some((metrics[0], metrics[1]));
            }
        }
    }

    // --- Mouse drag building ---
    let ctrl = keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight);
    let remove = ctrl;
    let Ok(window) = windows.single() else {
        return;
    };

    if mouse_button.just_pressed(MouseButton::Left) {
        if let Some(pos) = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32) {
            build_state.drag_start = Some(pos);
        }
    }

    if mouse_button.just_released(MouseButton::Left) {
        if let (Some(start), Some(end)) = (
            build_state.drag_start.take(),
            cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32),
        ) {
            let id = build_state.selected_structure as i32;
            let dir = build_state.cur_dir as i32;

            let dist_sq = (end - start).length_squared();

            let changes = if dist_sq < 0.25 {
                // Click (no drag)
                wall_grid.click(start, id, dir, remove)
            } else {
                // Drag
                wall_grid.drag(start, end, id, remove)
            };

            apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
        }
    }

    let _ = window;
}

/// Cast a ray from the cursor through the camera to a horizontal plane at height `y`.
fn cursor_world_pos(
    windows: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    y: f32,
) -> Option<Vec3> {
    let window = windows.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, camera_transform) = camera_q.single().ok()?;
    let ray = camera.viewport_to_world(camera_transform, cursor).ok()?;

    // Intersect with the horizontal plane at y
    let denom = ray.direction.y;
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (y - ray.origin.y) / denom;
    if t < 0.0 {
        return None;
    }
    Some(ray.origin + ray.direction * t)
}
