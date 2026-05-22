use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::camera::GameCamera;
use crate::structure::{PlacementStyle, StructureList};
use crate::wall_grid::{apply_changes, WallGrid};

/// Marker for the wall/floor build cursor (pin shape).
#[derive(Component)]
pub struct WallCursorMarker;

/// Marker for the room-plop build cursor (flat disc).
#[derive(Component)]
pub struct RoomCursorMarker;

/// Marker for the drag-preview rectangle (flat semi-transparent slab).
#[derive(Component)]
pub struct DragPreviewMarker;

pub fn cursor_system(
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    build_state: Res<BuildState>,
    wall_grid: Res<WallGrid>,
    mut wall_q: Query<
        (&mut Transform, &mut Visibility),
        (
            With<WallCursorMarker>,
            Without<RoomCursorMarker>,
            Without<DragPreviewMarker>,
        ),
    >,
    mut room_q: Query<
        (&mut Transform, &mut Visibility),
        (
            With<RoomCursorMarker>,
            Without<WallCursorMarker>,
            Without<DragPreviewMarker>,
        ),
    >,
    mut preview_q: Query<
        (&mut Transform, &mut Visibility),
        (
            With<DragPreviewMarker>,
            Without<WallCursorMarker>,
            Without<RoomCursorMarker>,
        ),
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

    if let Ok((mut t, mut vis)) = preview_q.single_mut() {
        let style = wall_grid.structures[id as usize].placement_style;
        let show = build_state
            .drag_start
            .zip(maybe_pos)
            .and_then(|(start, end)| drag_preview_rect(start, end, style, y));
        match show {
            Some((center, size)) => {
                t.translation = center;
                t.scale = size;
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

/// Compute the center and scale of a flat preview slab for the given drag.
/// Returns `None` if the drag has zero extent along the relevant axes.
fn drag_preview_rect(
    start: Vec3,
    end: Vec3,
    style: PlacementStyle,
    y: f32,
) -> Option<(Vec3, Vec3)> {
    const H: f32 = 0.15;
    const WALL_H: f32 = 0.8;
    const WALL_W: f32 = 0.201; // You can't Z-fight in here! It's the 3D room!
    match style {
        PlacementStyle::WallDrag => {
            let from_r = start.round();
            if (end.x - start.x).abs() > (end.z - start.z).abs() {
                let end_x = end.x.round();
                let (min_x, max_x) = (from_r.x.min(end_x), from_r.x.max(end_x));
                if min_x >= max_x {
                    return None;
                }
                let center = Vec3::new((min_x + max_x) * 0.5, y + WALL_H * 0.5, from_r.z);
                Some((center, Vec3::new(max_x - min_x, WALL_H, WALL_W)))
            } else {
                let end_z = end.z.round();
                let (min_z, max_z) = (from_r.z.min(end_z), from_r.z.max(end_z));
                if min_z >= max_z {
                    return None;
                }
                let center = Vec3::new(from_r.x, y + WALL_H * 0.5, (min_z + max_z) * 0.5);
                Some((center, Vec3::new(WALL_W, WALL_H, max_z - min_z)))
            }
        }
        PlacementStyle::FloorDrag => {
            let min = start.round().min(end.round());
            let max = start.round().max(end.round());
            if max.x <= min.x || max.z <= min.z {
                return None;
            }
            let center = Vec3::new((min.x + max.x) * 0.5, y + H * 0.5, (min.z + max.z) * 0.5);
            Some((center, Vec3::new(max.x - min.x, H, max.z - min.z)))
        }
        _ => None,
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
    mouse_scroll: Res<AccumulatedMouseScroll>,
    model_state: Res<crate::qnn::ModelState>,
) {
    // --- Layer up/down ---
    if keyboard.just_pressed(KeyCode::ArrowUp) {
        build_state.cur_y = (build_state.cur_y + 1).min(10);
    }
    if keyboard.just_pressed(KeyCode::ArrowDown) {
        build_state.cur_y = (build_state.cur_y - 1).max(0);
    }

    let shift = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);
    if shift && mouse_scroll.delta.y > 0.5 {
        build_state.cur_y = (build_state.cur_y + 1).min(10);
    } else if shift && mouse_scroll.delta.y < -0.5 {
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
            if let Some(holder) = &model_state.holder {
                let holder = holder.lock().unwrap();
                let metrics = crate::qnn::compute_metrics(
                    &holder,
                    &wall_grid.contents,
                    &wall_grid.structures,
                    world_pos,
                );
                if metrics.len() >= 2 {
                    build_state.evaluation = Some((metrics[0], metrics[1]));
                }
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
                wall_grid.click(start, id, dir, remove)
            } else {
                wall_grid.drag(start, end, id, remove)
            };

            apply_changes(&mut commands, &mut wall_grid, &structure_list, changes);
        }
    }

    let _ = window;
}

/// Cast a ray from the cursor through the camera to a horizontal plane at height `y`.
pub(crate) fn cursor_world_pos(
    windows: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    y: f32,
) -> Option<Vec3> {
    let window = windows.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, camera_transform) = camera_q.single().ok()?;
    let ray = camera.viewport_to_world(camera_transform, cursor).ok()?;

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

/// Startup system: spawns the wall and room cursor entities.
pub fn spawn_cursors(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let cursor_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.8, 1.0),
        unlit: true,
        ..default()
    });

    // Wall/floor cursor: tall pin (cylinder + sphere on top).
    commands
        .spawn((Transform::default(), Visibility::Hidden, WallCursorMarker))
        .with_children(|p| {
            p.spawn((
                Mesh3d(meshes.add(Cylinder::new(0.04, 0.5))),
                MeshMaterial3d(cursor_mat.clone()),
                Transform::from_xyz(0.0, 0.5, 0.0),
            ));
            p.spawn((
                Mesh3d(meshes.add(Sphere::new(0.12))),
                MeshMaterial3d(cursor_mat.clone()),
                Transform::from_xyz(0.0, 1.12, 0.0),
            ));
        });

    // Room cursor: flat disc.
    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(0.45, 0.025))),
        MeshMaterial3d(cursor_mat),
        Transform::default(),
        Visibility::Hidden,
        RoomCursorMarker,
    ));

    // Drag preview: unit box scaled at runtime to represent the affected area.
    let preview_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.2, 0.9, 1.0, 0.35),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(preview_mat),
        Transform::default(),
        Visibility::Hidden,
        DragPreviewMarker,
    ));
}
