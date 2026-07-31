use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::autotile::{spec_stem, AutotileHandles, AutotileRules, AutotiledMeshes};
use crate::camera::GameCamera;
use crate::city::{
    apply_changes, apply_proposal_changes, cell_transform, get_real_or_proposed, City, CityMut,
    ConstructedCity, GridCellMarker, MaterialAssets, ProposalGhostMarker, ProposalOverlayAssets,
    ProposedCutMarker,
};
use crate::construction::construct;
use crate::cutaway::{CutCellMarker, CutawayMode};
use crate::eorf::{EorfId, EorfList, PlacementStyle};
use crate::game_mode::SandboxMode;
use crate::materials::BuildMaterialId;
use crate::sparse3d::{Facing, Slot, SlotCoord};

/// Bundles read-only resources used by `building_input_system` so its
/// parameter count stays under Bevy's system-function arity limit (16).
#[derive(bevy::ecs::system::SystemParam)]
pub struct BuildAssets<'w> {
    pub structure_list: Res<'w, EorfList>,
    pub overlay_assets: Res<'w, ProposalOverlayAssets>,
    pub model_state: Res<'w, crate::qnn::ModelState>,
    pub material_list: Res<'w, crate::materials::MaterialList>,
}

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
    constructed: Res<ConstructedCity>,
    structure_list: Res<EorfList>,
    cursor_entities: Res<CursorEntities>,
    mut cursors: Query<(&mut Transform, &mut Visibility)>,
) {
    let id = EorfId(build_state.selected_structure as u32);
    let is_room = constructed.structure_is_room_plop(id);
    let is_wall_plop = constructed.structure_is_wall_plop(id);
    // Both `RoomPlop` and `WallPlop` preview the actual mesh (via the "room"
    // cursor entity, snapped to a room cube or a wall boundary respectively)
    // instead of the plain pin used by drag-based placement styles.
    let uses_object_cursor = is_room || is_wall_plop;
    let maybe_pos = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32);
    let y = build_state.cur_y as f32;

    if let Ok((mut t, mut vis)) = cursors.get_mut(cursor_entities.wall) {
        match (!uses_object_cursor).then_some(maybe_pos).flatten() {
            Some(pos) => {
                let s = pos.round();
                t.translation = Vec3::new(s.x, y, s.z);
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }

    if let Ok((mut t, mut vis)) = cursors.get_mut(cursor_entities.room) {
        match uses_object_cursor.then_some(maybe_pos).flatten() {
            Some(pos) => {
                let (slot, cube, facing) = if is_room {
                    let s = pos.round();
                    let cube = IVec3::new(s.x as i32, build_state.cur_y, s.z as i32);
                    (Slot::Room, cube, Facing::from_number(build_state.cur_dir))
                } else {
                    let loc = crate::city::nearest_wall_slot(pos);
                    let facing = Facing::from_number(build_state.wall_plop_dir(loc.slot) as u8);
                    (loc.slot, loc.cube, facing)
                };
                let tr = cell_transform(slot, facing, cube);
                t.translation = tr.translation;
                if structure_list.is_wings(id) {
                    t.translation += crate::city::wings_offset(tr.rotation);
                }
                t.rotation = tr.rotation;
                t.scale = Vec3::splat(0.999);
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }

    if let Ok((mut t, mut vis)) = cursors.get_mut(cursor_entities.preview) {
        let style = constructed.eorfs[id.as_usize()].placement_style;
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
    /// `WallPlop`'s rotation state on `XLoWall`s: unflipped (`NegX`) or
    /// flipped 180° (`PosX`). Kept separate from `wall_plop_flip_z` so that
    /// rotating the preview while it's snapped to an X-wall doesn't affect
    /// its remembered orientation on Z-walls (which sit 90° apart), and
    /// vice versa.
    pub wall_plop_flip_x: bool,
    /// `WallPlop`'s rotation state on `ZLoWall`s: unflipped (`NegZ`) or
    /// flipped 180° (`PosZ`). See `wall_plop_flip_x`.
    pub wall_plop_flip_z: bool,
    /// Latest evaluation results (order, interest).
    pub evaluation: Option<(f32, f32)>,
    /// Selected material (`BuildMaterialId`, index into `MaterialList::materials`) per structure type.
    pub material_per_type:
        std::collections::HashMap<crate::materials::ElementType, BuildMaterialId>,
}

impl BuildState {
    /// The selected material for `stype`, defaulting to (and recording) its
    /// first available material if none has been chosen yet.
    pub fn material_for_type(
        &mut self,
        stype: crate::materials::ElementType,
        material_list: &crate::materials::MaterialList,
    ) -> BuildMaterialId {
        *self.material_per_type.entry(stype).or_insert_with(|| {
            material_list
                .for_type(stype)
                .first()
                .map(|&(id, _)| id)
                .unwrap_or_default()
        })
    }

    /// The `Facing` (as a `Cell::facing` number, 0-3) `WallPlop` should place
    /// with when snapping onto `slot`, per the remembered per-axis rotation
    /// state. `slot` must be `XLoWall` or `ZLoWall`.
    pub fn wall_plop_dir(&self, slot: Slot) -> i32 {
        match slot {
            Slot::XLoWall => {
                if self.wall_plop_flip_x {
                    Facing::PosX as i32
                } else {
                    Facing::NegX as i32
                }
            }
            Slot::ZLoWall => {
                if self.wall_plop_flip_z {
                    Facing::PosZ as i32
                } else {
                    Facing::NegZ as i32
                }
            }
            Slot::Room | Slot::Floor => 0,
        }
    }
}

/// Reflects a set of proposal-view `changes` into the world and ECS. In sandbox
/// mode the proposals are committed immediately (built for real); otherwise they
/// stay as proposal ghosts. No-op if `changes` is empty.
#[allow(clippy::too_many_arguments)]
fn apply_edit(
    commands: &mut Commands,
    constructed: &mut ConstructedCity,
    pending: &mut crate::city::ProposedCity,
    assembled: &mut crate::city::AssembledCity,
    structure_list: &EorfList,
    overlay_assets: &ProposalOverlayAssets,
    material_list: &crate::materials::MaterialList,
    sandbox_enabled: bool,
    changes: Vec<(SlotCoord, crate::city::ProposalView)>,
) {
    if changes.is_empty() {
        return;
    }
    if sandbox_enabled {
        let real_changes = construct(constructed, pending, material_list);
        apply_changes(commands, assembled, structure_list, real_changes);
    } else {
        apply_proposal_changes(commands, assembled, structure_list, overlay_assets, changes);
    }
}

/// "Layer up/down" arrow keys and shift+scroll-wheel, both driving `build_state.cur_y`.
fn handle_layer_controls(
    keyboard: &ButtonInput<KeyCode>,
    mouse_scroll: &AccumulatedMouseScroll,
    egui_wants_input: &bevy_egui::input::EguiWantsInput,
    typing: bool,
    build_state: &mut BuildState,
) {
    if !typing && keyboard.just_pressed(KeyCode::ArrowUp) {
        build_state.cur_y = (build_state.cur_y + 1).min(10);
    }
    if !typing && keyboard.just_pressed(KeyCode::ArrowDown) {
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
}

/// `R` key: cycles the selected structure's rotation, or for a `WallPlop`, flips its
/// orientation on whichever wall axis the cursor is currently nearest to.
fn handle_rotation(
    keyboard: &ButtonInput<KeyCode>,
    typing: bool,
    windows: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    constructed: &ConstructedCity,
    build_state: &mut BuildState,
) {
    if typing || !keyboard.just_pressed(KeyCode::KeyR) {
        return;
    }
    let id = EorfId(build_state.selected_structure as u32);
    if constructed.structure_is_wall_plop(id) {
        // WallPlop only rotates 180°, and the X-wall/Z-wall rotation
        // states are independent (see `BuildState::wall_plop_flip_x`).
        // Which one flips depends on whichever wall the cursor is
        // currently nearest to.
        if let Some(pos) = cursor_world_pos(windows, camera_q, build_state.cur_y as f32) {
            match crate::city::nearest_wall_slot(pos).slot {
                Slot::XLoWall => build_state.wall_plop_flip_x = !build_state.wall_plop_flip_x,
                Slot::ZLoWall => build_state.wall_plop_flip_z = !build_state.wall_plop_flip_z,
                Slot::Room | Slot::Floor => {}
            }
        }
    } else {
        build_state.cur_dir = (build_state.cur_dir + 3) % 4;
    }
}

/// `C` key: cycles through the cutaway-view algorithms.
fn handle_cutaway_cycle(
    keyboard: &ButtonInput<KeyCode>,
    typing: bool,
    cutaway_mode: &mut CutawayMode,
) {
    if !typing && keyboard.just_pressed(KeyCode::KeyC) {
        *cutaway_mode = match *cutaway_mode {
            CutawayMode::FloorEdge => CutawayMode::SimpleOctant,
            CutawayMode::SimpleOctant => CutawayMode::FloorEdgePlusOctant,
            CutawayMode::FloorEdgePlusOctant => CutawayMode::FloorEdge,
        };
    }
}

/// F1/F2/F3: switches the left-panel tab.
fn handle_left_tab_keys(
    keyboard: &ButtonInput<KeyCode>,
    typing: bool,
    ui_state: &mut crate::build_ui::UiState,
) {
    if !typing && keyboard.just_pressed(KeyCode::F1) {
        ui_state.left_tab = crate::build_ui::LeftTab::Elements;
    }
    if !typing && keyboard.just_pressed(KeyCode::F2) {
        ui_state.left_tab = crate::build_ui::LeftTab::Furniture;
    }
    if !typing && keyboard.just_pressed(KeyCode::F3) {
        ui_state.left_tab = crate::build_ui::LeftTab::Places;
    }
}

/// Eorf selection by digit key (in sorted display order, filtered to the active tab -- so
/// digits 1-9 reach the same 9 structures the Elements/Furniture tab currently numbers 1-9).
fn handle_digit_selection(
    keyboard: &ButtonInput<KeyCode>,
    typing: bool,
    ui_state: &crate::build_ui::UiState,
    constructed: &ConstructedCity,
    build_state: &mut BuildState,
) {
    if typing {
        return;
    }
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
    let want_furniture = match ui_state.left_tab {
        crate::build_ui::LeftTab::Elements => Some(false),
        crate::build_ui::LeftTab::Furniture => Some(true),
        crate::build_ui::LeftTab::Places => None,
    };
    let Some(want_furniture) = want_furniture else {
        return;
    };
    let filtered: Vec<usize> = crate::eorf::sorted_structure_indices(&constructed.eorfs)
        .into_iter()
        .filter(|&i| {
            constructed.eorfs[i].is_furniture() == want_furniture && constructed.eorfs[i].placeable
        })
        .collect();
    for (display_idx, key) in DIGIT_KEYS.iter().enumerate() {
        if keyboard.just_pressed(*key) {
            if let Some(&struct_idx) = filtered.get(display_idx) {
                build_state.selected_structure = struct_idx;
            }
        }
    }
}

/// Z/Y: undo/redo the last proposal edit.
#[allow(clippy::too_many_arguments)]
fn handle_undo_redo(
    commands: &mut Commands,
    keyboard: &ButtonInput<KeyCode>,
    typing: bool,
    constructed: &mut ConstructedCity,
    pending: &mut crate::city::ProposedCity,
    assembled: &mut crate::city::AssembledCity,
    structure_list: &EorfList,
    overlay_assets: &ProposalOverlayAssets,
    material_list: &crate::materials::MaterialList,
    sandbox_enabled: bool,
) {
    let undo_redo_changes = if typing {
        None
    } else if keyboard.just_pressed(KeyCode::KeyZ) {
        Some(pending.undo(constructed))
    } else if keyboard.just_pressed(KeyCode::KeyY) {
        Some(pending.redo(constructed))
    } else {
        None
    };
    if let Some(changes) = undo_redo_changes {
        apply_edit(
            commands,
            constructed,
            pending,
            assembled,
            structure_list,
            overlay_assets,
            material_list,
            sandbox_enabled,
            changes,
        );
    }
}

/// `V` key: evaluates the QNN model at the cursor and records the result for the UI.
fn handle_evaluate(
    keyboard: &ButtonInput<KeyCode>,
    typing: bool,
    windows: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    constructed: &ConstructedCity,
    model_state: &crate::qnn::ModelState,
    build_state: &mut BuildState,
) {
    if typing || !keyboard.just_pressed(KeyCode::KeyV) {
        return;
    }
    let Some(world_pos) = cursor_world_pos(windows, camera_q, build_state.cur_y as f32) else {
        return;
    };
    let Some(holder) = &model_state.holder else {
        return;
    };
    let holder = holder.lock().unwrap();
    let metrics = crate::qnn::compute_metrics(
        &holder,
        &constructed.contents,
        &constructed.eorfs,
        world_pos,
    );
    if metrics.len() >= 2 {
        build_state.evaluation = Some((metrics[0], metrics[1]));
    }
}

/// Left-click/drag: places or removes structures along the drag, then commits the edit.
#[allow(clippy::too_many_arguments)]
fn handle_drag_building(
    commands: &mut Commands,
    keyboard: &ButtonInput<KeyCode>,
    mouse_button: &ButtonInput<MouseButton>,
    windows: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    constructed: &mut ConstructedCity,
    pending: &mut crate::city::ProposedCity,
    assembled: &mut crate::city::AssembledCity,
    structure_list: &EorfList,
    overlay_assets: &ProposalOverlayAssets,
    material_list: &crate::materials::MaterialList,
    sandbox_enabled: bool,
    build_state: &mut BuildState,
) {
    let ctrl = keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight);
    let remove = ctrl;

    if mouse_button.just_pressed(MouseButton::Left) {
        if let Some(pos) = cursor_world_pos(windows, camera_q, build_state.cur_y as f32) {
            build_state.drag_start = Some(pos);
        }
    }

    if mouse_button.just_released(MouseButton::Left) {
        if let (Some(start), Some(end)) = (
            build_state.drag_start.take(),
            cursor_world_pos(windows, camera_q, build_state.cur_y as f32),
        ) {
            let id = EorfId(build_state.selected_structure as u32);
            let dir = if constructed.structure_is_wall_plop(id) {
                build_state.wall_plop_dir(crate::city::nearest_wall_slot(start).slot)
            } else {
                build_state.cur_dir as i32
            };
            let build_material = {
                let info = &constructed.eorfs[id.as_usize()];
                if info.is_furniture() {
                    // Meaningless for furniture (which always displays as
                    // planks), but `propose` wants one.
                    BuildMaterialId::default()
                } else {
                    let stype = info.element_type().unwrap_or_default();
                    build_state.material_for_type(stype, material_list)
                }
            };

            let dist_sq = (end - start).length_squared();

            let changes = if dist_sq < 0.25 {
                pending.click(constructed, start, id, dir, remove, build_material)
            } else {
                pending.drag(constructed, (start, end), dir, id, remove, build_material)
            };

            apply_edit(
                commands,
                constructed,
                pending,
                assembled,
                structure_list,
                overlay_assets,
                material_list,
                sandbox_enabled,
                changes,
            );
        }
    }
}

/// Right-click (as opposed to right-drag, which rotates the camera): picks a real furniture
/// cell under the cursor for the furniture-editing popup.
fn handle_furniture_pick(
    mouse_button: &ButtonInput<MouseButton>,
    window: &Window,
    windows: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    egui_wants_input: &bevy_egui::input::EguiWantsInput,
    grid_raycast: &mut crate::selection::GridRaycast,
    constructed: &ConstructedCity,
    right_press_pos: &mut Option<Vec2>,
    furniture_right_click: &mut crate::build_ui::FurnitureRightClick,
) {
    if mouse_button.just_pressed(MouseButton::Right) {
        *right_press_pos = window.cursor_position();
    }
    if mouse_button.just_released(MouseButton::Right) {
        let pressed = right_press_pos.take();
        let moved = pressed
            .zip(window.cursor_position())
            .map(|(a, b)| a.distance(b))
            .unwrap_or(f32::INFINITY);
        // A click barely moves the cursor; a rotate-drag moves it a lot.
        if moved < 4.0 && !egui_wants_input.wants_any_pointer_input() {
            if let Some(ray) = crate::selection::cursor_ray(windows, camera_q) {
                if let Some(loc) = grid_raycast.cast(ray) {
                    if let Some(cell) = constructed.contents.get(loc) {
                        if constructed.eorfs[cell.id.as_usize()].is_furniture() {
                            furniture_right_click.0 = Some(loc);
                        }
                    }
                }
            }
        }
    }
}

pub fn building_input_system(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    mut world: CityMut,
    assets: BuildAssets,
    mut build_state: ResMut<BuildState>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    egui_wants_input: Res<bevy_egui::input::EguiWantsInput>,
    mut cutaway_mode: ResMut<CutawayMode>,
    sandbox: Res<SandboxMode>,
    mut furniture_right_click: ResMut<crate::build_ui::FurnitureRightClick>,
    mut right_press_pos: Local<Option<Vec2>>,
    mut ui_state: ResMut<crate::build_ui::UiState>,
    mut grid_raycast: crate::selection::GridRaycast,
) {
    let BuildAssets {
        structure_list,
        overlay_assets,
        model_state,
        material_list,
    } = assets;
    let (constructed, pending, assembled) = (
        &mut world.constructed,
        &mut world.pending,
        &mut world.assembled,
    );
    // While egui has keyboard focus (e.g. typing in a text field), swallow all
    // build-tool keyboard shortcuts so they don't fire behind the widget.
    let typing = egui_wants_input.wants_keyboard_input();

    handle_layer_controls(
        &keyboard,
        &mouse_scroll,
        &egui_wants_input,
        typing,
        &mut build_state,
    );
    handle_rotation(
        &keyboard,
        typing,
        &windows,
        &camera_q,
        constructed,
        &mut build_state,
    );
    handle_cutaway_cycle(&keyboard, typing, &mut cutaway_mode);
    handle_left_tab_keys(&keyboard, typing, &mut ui_state);
    handle_digit_selection(&keyboard, typing, &ui_state, constructed, &mut build_state);
    handle_undo_redo(
        &mut commands,
        &keyboard,
        typing,
        constructed,
        pending,
        assembled,
        &structure_list,
        &overlay_assets,
        &material_list,
        sandbox.enabled,
    );
    handle_evaluate(
        &keyboard,
        typing,
        &windows,
        &camera_q,
        constructed,
        &model_state,
        &mut build_state,
    );

    // Left-drag building and right-click picking share this gate (and `window`): if there's
    // no primary window this frame, neither can do anything.
    let Ok(window) = windows.single() else {
        return;
    };

    handle_drag_building(
        &mut commands,
        &keyboard,
        &mouse_button,
        &windows,
        &camera_q,
        constructed,
        pending,
        assembled,
        &structure_list,
        &overlay_assets,
        &material_list,
        sandbox.enabled,
        &mut build_state,
    );
    handle_furniture_pick(
        &mouse_button,
        window,
        &windows,
        &camera_q,
        &egui_wants_input,
        &mut grid_raycast,
        constructed,
        &mut right_press_pos,
        &mut furniture_right_click,
    );
}

/// Keeps the room cursor's SceneRoot in sync with the selected structure.
pub fn update_room_cursor_mesh(
    build_state: Res<BuildState>,
    cursor_entities: Res<CursorEntities>,
    structure_list: Res<EorfList>,
    constructed: Res<ConstructedCity>,
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
    let struct_id = EorfId(id as u32);
    // The last case of the first rule is used as the preview
    if constructed.structure_is_room_plop(struct_id)
        || constructed.structure_is_wall_plop(struct_id)
    {
        let name = &structure_list.structures[id].info.name;
        let autotile_handle = autotile_rules
            .0
            .iter()
            .find(|rule| rule.subject.structure_name() == Some(name.as_str()))
            .and_then(|rule| {
                rule.cases.last().and_then(|case| {
                    if let AutotiledMeshes::Mesh { spec, .. } = &case.result {
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

/// The material a newly spawned mesh child should be recolored with, determined by
/// walking up its ancestors to the entity that "owns" it.
enum Recolor {
    /// Descendant of the room cursor: translucent cyan.
    Cursor,
    /// Descendant of a proposal ghost or proposed-only cut: translucent ghost.
    Ghost,
    /// Descendant of a real placed cell (or its y-cut variant): the cell's material color.
    Material(SlotCoord),
}

/// Recolors newly spawned mesh children of the room cursor (cyan), proposal ghosts
/// (translucent), proposed-cut entities (translucent, same material as ghosts), and
/// real placed cells / their y-cut variants (their structure's `Material` color).
///
/// Uses `ParamSet` to avoid a conflict between the `Added<T>` filter and `&mut T` access,
/// which Bevy treats as incompatible within a single system.
pub fn recolor_new_mesh_children(
    cursor_entities: Res<CursorEntities>,
    overlay_assets: Res<ProposalOverlayAssets>,
    material_assets: Res<MaterialAssets>,
    material_list: Res<crate::materials::MaterialList>,
    world: City,
    ghost_markers_q: Query<(), With<ProposalGhostMarker>>,
    proposed_cut_q: Query<(), With<ProposedCutMarker>>,
    cell_markers_q: Query<&GridCellMarker>,
    cut_markers_q: Query<&CutCellMarker, Without<ProposedCutMarker>>,
    child_of_q: Query<&ChildOf>,
    mut commands: Commands,
    mut param_set: ParamSet<(
        Query<Entity, Added<MeshMaterial3d<StandardMaterial>>>,
        Query<&mut MeshMaterial3d<StandardMaterial>>,
    )>,
) {
    let City {
        constructed,
        pending,
        ..
    } = world;
    let new_entities: Vec<Entity> = param_set.p0().iter().collect();
    for entity in new_entities {
        // Walk up the scene tree to find the entity that owns this mesh child.
        let mut node = entity;
        let recolor = loop {
            if node == cursor_entities.room {
                break Some(Recolor::Cursor);
            }
            if ghost_markers_q.contains(node) || proposed_cut_q.contains(node) {
                break Some(Recolor::Ghost);
            }
            if let Ok(marker) = cell_markers_q.get(node) {
                break Some(Recolor::Material(marker.loc));
            }
            if let Ok(marker) = cut_markers_q.get(node) {
                break Some(Recolor::Material(marker.loc));
            }
            match child_of_q.get(node) {
                Ok(child_of) => node = child_of.0,
                Err(_) => break None,
            }
        };

        match recolor {
            // Cursor/ghost overlays keep their plain `StandardMaterial`.
            Some(Recolor::Cursor) => {
                if let Ok(mut m) = param_set.p1().get_mut(entity) {
                    *m = MeshMaterial3d(cursor_entities.cyan_mat.clone());
                }
            }
            Some(Recolor::Ghost) => {
                if let Ok(mut m) = param_set.p1().get_mut(entity) {
                    *m = MeshMaterial3d(overlay_assets.ghost_mat.clone());
                }
            }
            // Real placed cells (and furniture) get the GI extended material, so we
            // swap out the GLTF-supplied `StandardMaterial` for `GiMaterial`. The mesh
            // child may be despawned (autotile/cutaway respawn) before this flushes, so
            // re-check the entity at apply time rather than letting the command panic.
            Some(Recolor::Material(loc)) => {
                let material = get_real_or_proposed(&constructed, &pending, loc)
                    .map(|c| c.material(&constructed.eorfs, &material_list))
                    .unwrap_or_default();
                let handle = material_assets.get(material);
                commands.queue(move |world: &mut bevy::prelude::World| {
                    if let Ok(mut e) = world.get_entity_mut(entity) {
                        e.remove::<MeshMaterial3d<StandardMaterial>>();
                        e.insert(MeshMaterial3d(handle));
                    }
                });
            }
            None => continue,
        }
    }
}

/// Cast a ray from the cursor through the camera to a horizontal plane at height `y`.
pub(crate) fn cursor_world_pos(
    windows: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    y: f32,
) -> Option<Vec3> {
    let ray = crate::selection::cursor_ray(windows, camera_q)?;

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
