use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::autotile::{spec_stem, AutotileHandles, AutotileResult, AutotileRules};
use crate::camera::GameCamera;
use crate::cutaway::CutawayMode;
use crate::sparse3d::{Facing, RelSlot};
use crate::structure::{PlacementStyle, StructureId, StructureList};
use crate::wall_grid::{
    apply_proposal_changes, cell_transform, ProposalGhostMarker, ProposalOverlayAssets,
    ProposedCutMarker, WallGrid,
};

#[derive(Resource)]
pub struct CursorEntities {
    pub wall: Entity,
    pub room: Entity,
    pub preview: Entity,
    pub cyan_mat: Handle<StandardMaterial>,
}

pub fn cursor_system(
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    build_state: Res<BuildState>,
    wall_grid: Res<WallGrid>,
    cursor_entities: Res<CursorEntities>,
    mut cursors: Query<(&mut Transform, &mut Visibility)>,
) {
    let id = StructureId(build_state.selected_structure as u32);
    let is_room = wall_grid.structure_is_room_plop(id);
    let maybe_pos = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32);
    let y = build_state.cur_y as f32;

    if let Ok((mut t, mut vis)) = cursors.get_mut(cursor_entities.wall) {
        match (!is_room).then_some(maybe_pos).flatten() {
            Some(pos) => {
                let s = pos.round();
                t.translation = Vec3::new(s.x, y, s.z);
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }

    if let Ok((mut t, mut vis)) = cursors.get_mut(cursor_entities.room) {
        match is_room.then_some(maybe_pos).flatten() {
            Some(pos) => {
                let s = pos.round();
                let cube = IVec3::new(s.x as i32, build_state.cur_y, s.z as i32);
                let facing = Facing::from_number(build_state.cur_dir);
                let tr = cell_transform(RelSlot::Room, facing, cube);
                t.translation = tr.translation;
                t.rotation = tr.rotation;
                t.scale = Vec3::splat(0.999);
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }

    if let Ok((mut t, mut vis)) = cursors.get_mut(cursor_entities.preview) {
        let style = wall_grid.structures[id.as_usize()].placement_style;
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
        PlacementStyle::RoomDrag => {
            let min = start.round().min(end.round());
            let max = start.round().max(end.round());
            if max.x <= min.x || max.z <= min.z {
                return None;
            }
            let center = Vec3::new((min.x + max.x) * 0.5, y + 0.5, (min.z + max.z) * 0.5);
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
    egui_wants_input: Res<bevy_egui::input::EguiWantsInput>,
    model_state: Res<crate::qnn::ModelState>,
    overlay_assets: Res<ProposalOverlayAssets>,
    mut cutaway_mode: ResMut<CutawayMode>,
) {
    // --- Layer up/down ---
    if keyboard.just_pressed(KeyCode::ArrowUp) {
        build_state.cur_y = (build_state.cur_y + 1).min(10);
    }
    if keyboard.just_pressed(KeyCode::ArrowDown) {
        build_state.cur_y = (build_state.cur_y - 1).max(0);
    }

    let shift = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);
    if shift && !egui_wants_input.wants_any_pointer_input() {
        if mouse_scroll.delta.y > 0.5 {
            build_state.cur_y = (build_state.cur_y + 1).min(10);
        } else if mouse_scroll.delta.y < -0.5 {
            build_state.cur_y = (build_state.cur_y - 1).max(0);
        }
    }

    // --- Rotation ---
    if keyboard.just_pressed(KeyCode::KeyR) {
        build_state.cur_dir = (build_state.cur_dir + 3) % 4;
    }

    // --- Cutaway mode cycle ---
    if keyboard.just_pressed(KeyCode::KeyC) {
        *cutaway_mode = match *cutaway_mode {
            CutawayMode::FloorEdge => CutawayMode::SimpleOctant,
            CutawayMode::SimpleOctant => CutawayMode::FloorEdgePlusOctant,
            CutawayMode::FloorEdgePlusOctant => CutawayMode::FloorEdge,
        };
    }

    // --- Structure selection by digit key ---
    if !egui_wants_input.wants_keyboard_input() {
        const DIGIT_KEYS: [KeyCode; 9] = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        let num_structures = wall_grid.get_structure_names().len();
        for (i, key) in DIGIT_KEYS.iter().enumerate() {
            if keyboard.just_pressed(*key) && i < num_structures {
                build_state.selected_structure = i;
            }
        }
    }

    // --- Undo ---
    if keyboard.just_pressed(KeyCode::KeyZ) {
        // Use bypass_change_detection so proposal edits don't trigger ceiling-light recomputation.
        let changes = wall_grid.bypass_change_detection().undo();
        if !changes.is_empty() {
            let wg = wall_grid.bypass_change_detection();
            apply_proposal_changes(
                &mut commands,
                &mut *wg,
                &structure_list,
                &overlay_assets,
                changes,
            );
        }
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
            let id = StructureId(build_state.selected_structure as u32);
            let dir = build_state.cur_dir as i32;

            let dist_sq = (end - start).length_squared();

            // All proposal edits bypass change detection so ceiling lights don't recompute.
            let changes = if dist_sq < 0.25 {
                wall_grid
                    .bypass_change_detection()
                    .click(start, id, dir, remove)
            } else {
                wall_grid
                    .bypass_change_detection()
                    .drag(start, end, dir, id, remove)
            };

            if !changes.is_empty() {
                let wg = wall_grid.bypass_change_detection();
                apply_proposal_changes(
                    &mut commands,
                    &mut *wg,
                    &structure_list,
                    &overlay_assets,
                    changes,
                );
            }
        }
    }

    let _ = window;
}

/// Keeps the room cursor's SceneRoot in sync with the selected structure.
pub fn update_room_cursor_mesh(
    build_state: Res<BuildState>,
    cursor_entities: Res<CursorEntities>,
    structure_list: Res<StructureList>,
    wall_grid: Res<WallGrid>,
    autotile_rules: Res<AutotileRules>,
    autotile_handles: Res<AutotileHandles>,
    mut commands: Commands,
    mut last_id: Local<Option<usize>>,
) {
    let id = build_state.selected_structure;
    if Some(id) == *last_id {
        return;
    }
    *last_id = Some(id);
    let struct_id = StructureId(id as u32);
    // The last case of the first rule is used as the preview
    if wall_grid.structure_is_room_plop(struct_id) {
        let name = &structure_list.structures[id].info.name;
        let autotile_handle = autotile_rules
            .0
            .iter()
            .find(|rule| &rule.structure_name == name)
            .and_then(|rule| {
                rule.cases.last().and_then(|case| {
                    if let AutotileResult::Mesh { spec, .. } = &case.result {
                        let stem = spec_stem(spec, rule.slot);
                        autotile_handles.handles.get(&stem).map(|(h, _)| h.clone())
                    } else {
                        None
                    }
                })
            });
        let handle =
            autotile_handle.unwrap_or_else(|| structure_list.scene_handle(struct_id).clone());
        commands
            .entity(cursor_entities.room)
            .insert(SceneRoot(handle));
    }
}

/// Recolors newly spawned mesh children of the room cursor (cyan), proposal ghosts (translucent),
/// and proposed-cut entities (translucent, same material as ghosts).
///
/// Uses `ParamSet` to avoid a conflict between the `Added<T>` filter and `&mut T` access,
/// which Bevy treats as incompatible within a single system.
pub fn recolor_new_mesh_children(
    cursor_entities: Res<CursorEntities>,
    overlay_assets: Res<ProposalOverlayAssets>,
    ghost_markers_q: Query<Entity, With<ProposalGhostMarker>>,
    proposed_cut_q: Query<Entity, With<ProposedCutMarker>>,
    child_of_q: Query<&ChildOf>,
    mut param_set: ParamSet<(
        Query<Entity, Added<MeshMaterial3d<StandardMaterial>>>,
        Query<&mut MeshMaterial3d<StandardMaterial>>,
    )>,
) {
    let new_entities: Vec<Entity> = param_set.p0().iter().collect();
    for entity in new_entities {
        if is_descendant_of(entity, cursor_entities.room, &child_of_q) {
            if let Ok(mut m) = param_set.p1().get_mut(entity) {
                *m = MeshMaterial3d(cursor_entities.cyan_mat.clone());
            }
        } else {
            for ghost_entity in ghost_markers_q.iter() {
                if is_descendant_of(entity, ghost_entity, &child_of_q) {
                    if let Ok(mut m) = param_set.p1().get_mut(entity) {
                        *m = MeshMaterial3d(overlay_assets.ghost_mat.clone());
                    }
                    break;
                }
            }
            for cut_entity in proposed_cut_q.iter() {
                if is_descendant_of(entity, cut_entity, &child_of_q) {
                    if let Ok(mut m) = param_set.p1().get_mut(entity) {
                        *m = MeshMaterial3d(overlay_assets.ghost_mat.clone());
                    }
                    break;
                }
            }
        }
    }
}

fn is_descendant_of(mut entity: Entity, ancestor: Entity, child_of_q: &Query<&ChildOf>) -> bool {
    loop {
        if entity == ancestor {
            return true;
        }
        match child_of_q.get(entity) {
            Ok(child_of) => entity = child_of.0,
            Err(_) => return false,
        }
    }
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
    let wall = commands
        .spawn((Transform::default(), Visibility::Hidden))
        .with_children(|p| {
            p.spawn((
                Mesh3d(meshes.add(Cylinder::new(0.02, 1.0))),
                MeshMaterial3d(cursor_mat.clone()),
                Transform::from_xyz(0.0, 0.5, 0.0),
            ));
            p.spawn((
                Mesh3d(meshes.add(Cylinder::new(0.04, 0.5))),
                MeshMaterial3d(cursor_mat.clone()),
                Transform::from_xyz(0.0, 0.5, 0.0),
            ));
            p.spawn((
                Mesh3d(meshes.add(Sphere::new(0.1))),
                MeshMaterial3d(cursor_mat.clone()),
                Transform::from_xyz(0.0, 1.12, 0.0),
            ));
        })
        .id();

    // Room cursor: starts empty; SceneRoot is inserted by update_room_cursor_mesh.
    let cyan_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.2, 0.8, 1.0, 0.7),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let room = commands
        .spawn((Transform::default(), Visibility::Hidden))
        .id();

    // Drag preview: unit box scaled at runtime to represent the affected area.
    let preview_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.2, 0.9, 1.0, 0.35),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let preview = commands
        .spawn((
            Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
            MeshMaterial3d(preview_mat),
            Transform::default(),
            Visibility::Hidden,
        ))
        .id();

    commands.insert_resource(CursorEntities {
        wall,
        room,
        preview,
        cyan_mat,
    });
}
