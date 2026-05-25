use std::collections::{HashSet, VecDeque};

use bevy::camera::visibility::RenderLayers;
use bevy::math::Vec3;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::camera::GameCamera;
use crate::input::{cursor_world_pos, BuildState};
use crate::sparse3d::{RelSlot, SlotLocation};
use crate::structure::StructureList;
use crate::wall_grid::{
    cell_transform, GridCellMarker, ProposalGhostMarker, ProposalOverlayMarker, ProposedCutMarker,
    WallGrid,
};

/// Marker component for y-cut visibility variant entities.
#[derive(Component)]
pub struct CutCellMarker;

/// Layer used for geometry that should cast shadows but not be seen by the main camera.
const SHADOW_ONLY_LAYER: usize = 1;

/// Sets `layers` on `entity` and every descendant, so that `RenderLayers` is consistent
/// through the whole scene-spawned hierarchy (Bevy does not auto-propagate `RenderLayers`).
fn apply_render_layers_to_tree(
    entity: Entity,
    layers: &RenderLayers,
    children_q: &Query<&Children>,
    commands: &mut Commands,
) {
    commands.entity(entity).insert(layers.clone());
    if let Ok(children) = children_q.get(entity) {
        for child in children.iter() {
            apply_render_layers_to_tree(child, layers, children_q, commands);
        }
    }
}

/// Returns (x_dir, z_dir), each in {-1, 0, 1}, indicating which cardinal directions
/// face toward the camera. Within 5° of a single cardinal, returns only that one.
fn camera_facing_dirs(focus: Vec3, camera: Vec3) -> (i32, i32) {
    let dx = camera.x - focus.x;
    let dz = camera.z - focus.z;
    let len = (dx * dx + dz * dz).sqrt();
    if len < 1e-6 {
        return (1, 0);
    }
    let ndx = dx / len;
    let ndz = dz / len;
    let threshold = 5.0f32.to_radians().cos(); // ~0.9962
    if ndx.abs() >= threshold {
        (ndx.signum() as i32, 0)
    } else if ndz.abs() >= threshold {
        (0, ndz.signum() as i32)
    } else {
        (ndx.signum() as i32, ndz.signum() as i32)
    }
}

/// Returns the (x, z) of the cube the cursor is in or pointing toward (in camera direction).
fn cursor_cube(focus: Vec3, camera: Vec3, is_room_plop: bool) -> (i32, i32) {
    if is_room_plop {
        (focus.x.floor() as i32, focus.z.floor() as i32)
    } else {
        let cx = focus.x.round() as i32;
        let cz = focus.z.round() as i32;
        let dx = camera.x - focus.x;
        let dz = camera.z - focus.z;
        (
            if dx >= 0.0 { cx } else { cx - 1 },
            if dz >= 0.0 { cz } else { cz - 1 },
        )
    }
}

/// Searches downward from `from_y` for the first Floor cell at (x, z).
fn descend_to_floor(wall_grid: &WallGrid, x: i32, z: i32, from_y: i32) -> Option<i32> {
    for y in (from_y - 30..=from_y).rev() {
        if wall_grid
            .get_real_or_proposed(SlotLocation::new(x, y, z, RelSlot::Floor))
            .is_some()
        {
            return Some(y);
        }
    }
    None
}

/// BFS over Floor cells at `floor_y`, ignoring walls. Returns (x, z) of all reachable cells.
fn ground_floor_fill(
    wall_grid: &WallGrid,
    sx: i32,
    floor_y: i32,
    sz: i32,
) -> HashSet<(i32, i32)> {
    let mut visited: HashSet<(i32, i32)> = HashSet::new();
    let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
    if wall_grid
        .get_real_or_proposed(SlotLocation::new(sx, floor_y, sz, RelSlot::Floor))
        .is_none()
    {
        return visited;
    }
    visited.insert((sx, sz));
    queue.push_back((sx, sz));
    while let Some((fx, fz)) = queue.pop_front() {
        for (nx, nz) in [(fx + 1, fz), (fx - 1, fz), (fx, fz + 1), (fx, fz - 1)] {
            if !visited.contains(&(nx, nz))
                && wall_grid
                    .get_real_or_proposed(SlotLocation::new(nx, floor_y, nz, RelSlot::Floor))
                    .is_some()
            {
                visited.insert((nx, nz));
                queue.push_back((nx, nz));
            }
        }
    }
    visited
}

/// BFS over Floor cells at `sy` starting from `(sx, sz)`, stopping at walls.
/// Marks all found cells as visited, pushes them to `hidden`, returns their (x, z).
fn upper_floor_fill(
    wall_grid: &WallGrid,
    sx: i32,
    sy: i32,
    sz: i32,
    floor_visited: &mut HashSet<(i32, i32, i32)>,
    hidden: &mut Vec<SlotLocation>,
) -> Vec<(i32, i32)> {
    let mut cells: Vec<(i32, i32)> = Vec::new();
    if floor_visited.contains(&(sx, sy, sz)) {
        return cells;
    }
    if wall_grid
        .get_real_or_proposed(SlotLocation::new(sx, sy, sz, RelSlot::Floor))
        .is_none()
    {
        return cells;
    }
    floor_visited.insert((sx, sy, sz));
    hidden.push(SlotLocation::new(sx, sy, sz, RelSlot::Floor));
    cells.push((sx, sz));
    let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
    queue.push_back((sx, sz));
    while let Some((fx, fz)) = queue.pop_front() {
        let neighbors: [(i32, i32, SlotLocation); 4] = [
            (
                fx + 1,
                fz,
                SlotLocation::new(fx + 1, sy, fz, RelSlot::XLoWall),
            ),
            (fx - 1, fz, SlotLocation::new(fx, sy, fz, RelSlot::XLoWall)),
            (
                fx,
                fz + 1,
                SlotLocation::new(fx, sy, fz + 1, RelSlot::ZLoWall),
            ),
            (fx, fz - 1, SlotLocation::new(fx, sy, fz, RelSlot::ZLoWall)),
        ];
        for (nx, nz, wall_loc) in neighbors {
            if floor_visited.contains(&(nx, sy, nz)) {
                continue;
            }
            if wall_grid.get_real_or_proposed(wall_loc).is_some() {
                continue;
            }
            if wall_grid
                .get_real_or_proposed(SlotLocation::new(nx, sy, nz, RelSlot::Floor))
                .is_some()
            {
                floor_visited.insert((nx, sy, nz));
                hidden.push(SlotLocation::new(nx, sy, nz, RelSlot::Floor));
                cells.push((nx, nz));
                queue.push_back((nx, nz));
            }
        }
    }
    cells
}

/// For each cell in `floor_cells` (at `floor_y`), finds camera-facing exterior walls:
/// edges where there is no adjacent floor cell in `x_dir`/`z_dir` direction.
fn find_wall_seeds(
    wall_grid: &WallGrid,
    floor_cells: &HashSet<(i32, i32)>,
    floor_y: i32,
    x_dir: i32,
    z_dir: i32,
    inverted: bool,
) -> Vec<SlotLocation> {
    let (x_dir, z_dir) = if inverted {
        (-x_dir, -z_dir)
    } else {
        (x_dir, z_dir)
    };
    let mut walls: Vec<SlotLocation> = Vec::new();
    for &(fx, fz) in floor_cells {
        if x_dir != 0 && !floor_cells.contains(&(fx + x_dir, fz)) {
            let wx = if x_dir > 0 { fx + 1 } else { fx };
            let loc = SlotLocation::new(wx, floor_y, fz, RelSlot::XLoWall);
            if wall_grid.get_real_or_proposed(loc).is_some() {
                walls.push(loc);
            }
        }
        if z_dir != 0 && !floor_cells.contains(&(fx, fz + z_dir)) {
            let wz = if z_dir > 0 { fz + 1 } else { fz };
            let loc = SlotLocation::new(fx, floor_y, wz, RelSlot::ZLoWall);
            if wall_grid.get_real_or_proposed(loc).is_some() {
                walls.push(loc);
            }
        }
    }
    walls
}

/// Climbs a wall column upward from `bottom_loc`, hiding all walls in it.
/// The first (lowest) wall gets a cut entry. Pushes seeds for upper floor fills
/// adjacent to the top of the column. Uses `visited_walls` to avoid re-processing.
fn climb_wall_column(
    wall_grid: &WallGrid,
    bottom_loc: SlotLocation,
    x_dir: i32,
    z_dir: i32,
    visited_walls: &mut HashSet<(i32, i32, i32, bool)>,
    hidden: &mut Vec<SlotLocation>,
    mut cut: Option<&mut Vec<(SlotLocation, i32, bool)>>,
    floor_seeds: &mut Vec<(i32, i32, i32, bool)>,
) {
    let is_x = bottom_loc.rel_slot == RelSlot::XLoWall;
    let mut y = bottom_loc.cube.y;
    let mut first = true;
    loop {
        let key = (bottom_loc.cube.x, y, bottom_loc.cube.z, is_x);
        if !visited_walls.insert(key) {
            break;
        }
        let cur_loc =
            SlotLocation::new(bottom_loc.cube.x, y, bottom_loc.cube.z, bottom_loc.rel_slot);
        let (real, proposed) = wall_grid.get_real_and_proposed(cur_loc);
        let Some(cell) = real.or(proposed) else {
            break;
        };
        hidden.push(cur_loc);
        if first {
            if let Some(ref mut cut) = cut {
                let is_proposed_only = real.is_none();
                cut.push((cur_loc, cell.id, is_proposed_only));
            }
            first = false;
        }
        let next_loc = SlotLocation::new(
            bottom_loc.cube.x,
            y + 1,
            bottom_loc.cube.z,
            bottom_loc.rel_slot,
        );
        if wall_grid.get_real_or_proposed(next_loc).is_none() {
            let y_above = y + 1;
            match bottom_loc.rel_slot {
                RelSlot::XLoWall => {
                    let wx = bottom_loc.cube.x;
                    floor_seeds.push((wx - 1, y_above, bottom_loc.cube.z, x_dir > 0));
                    floor_seeds.push((wx, y_above, bottom_loc.cube.z, x_dir < 0));
                }
                RelSlot::ZLoWall => {
                    let wz = bottom_loc.cube.z;
                    floor_seeds.push((bottom_loc.cube.x, y_above, wz - 1, z_dir > 0));
                    floor_seeds.push((bottom_loc.cube.x, y_above, wz, z_dir < 0));
                }
                _ => {}
            }
            break;
        }
        y += 1;
    }
}

pub fn compute_visibility(
    wall_grid: &WallGrid,
    (focus_location, is_room_plop): (Vec3, bool),
    camera_location: Vec3,
    cur_y: i32,
) -> (Vec<SlotLocation>, Vec<(SlotLocation, i32, bool)>) {
    let mut hidden: Vec<SlotLocation> = Vec::new();
    let mut cut: Vec<(SlotLocation, i32, bool)> = Vec::new();

    let (x_dir, z_dir) = camera_facing_dirs(focus_location, camera_location);
    let (sx, sz) = cursor_cube(focus_location, camera_location, is_room_plop);
    let Some(floor_y) = descend_to_floor(wall_grid, sx, sz, cur_y) else {
        return (hidden, cut);
    };

    let ground_cells = ground_floor_fill(wall_grid, sx, floor_y, sz);
    if ground_cells.is_empty() {
        return (hidden, cut);
    }

    let mut visited_walls: HashSet<(i32, i32, i32, bool)> = HashSet::new();
    let mut floor_visited: HashSet<(i32, i32, i32)> = HashSet::new();
    let mut pending_walls: VecDeque<SlotLocation> = VecDeque::new();
    let mut pending_floors: VecDeque<(i32, i32, i32, bool)> = VecDeque::new();

    for wall_loc in find_wall_seeds(wall_grid, &ground_cells, floor_y, x_dir, z_dir, false) {
        pending_walls.push_back(wall_loc);
    }

    loop {
        while let Some(wall_loc) = pending_walls.pop_front() {
            let mut floor_seeds: Vec<(i32, i32, i32, bool)> = Vec::new();
            climb_wall_column(
                wall_grid,
                wall_loc,
                x_dir,
                z_dir,
                &mut visited_walls,
                &mut hidden,
                if wall_loc.cube.y == floor_y {
                    Some(&mut cut)
                } else {
                    None // Cuts are only at the bottom level
                },
                &mut floor_seeds,
            );
            for seed in floor_seeds {
                pending_floors.push_back(seed);
            }
        }

        let Some((fx, fy, fz, inverted)) = pending_floors.pop_front() else {
            break;
        };

        if floor_visited.contains(&(fx, fy, fz)) {
            continue;
        }

        let upper_cells =
            upper_floor_fill(wall_grid, fx, fy, fz, &mut floor_visited, &mut hidden);
        if upper_cells.is_empty() {
            continue;
        }

        let upper_set: HashSet<(i32, i32)> = upper_cells.into_iter().collect();
        for wall_loc in find_wall_seeds(wall_grid, &upper_set, fy, x_dir, z_dir, inverted) {
            pending_walls.push_back(wall_loc);
        }
    }

    (hidden, cut)
}

pub fn update_visibility_system(
    mut commands: Commands,
    mut wall_grid: ResMut<WallGrid>,
    structure_list: Res<StructureList>,
    build_state: Res<BuildState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    mut vis_q: Query<(
        Entity,
        &GridCellMarker,
        &mut Visibility,
        Option<&RenderLayers>,
    )>,
    mut ghost_q: Query<(Entity, &ProposalGhostMarker, Option<&RenderLayers>)>,
    mut overlay_q: Query<(Entity, &ProposalOverlayMarker, Option<&RenderLayers>)>,
    children_q: Query<&Children>,
    cut_q: Query<Entity, With<CutCellMarker>>,
) {
    let Ok((_, cam_gt)) = camera_q.single() else {
        return;
    };
    let camera_pos = cam_gt.translation();
    let focus_pos = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32)
        .unwrap_or_else(|| Vec3::new(0.0, build_state.cur_y as f32, 0.0));

    let is_room_plop = wall_grid.structure_is_room_plop(build_state.selected_structure as i32);

    for entity in cut_q.iter() {
        commands.entity(entity).despawn();
    }
    // bypass_change_detection so per-frame cut_entities writes don't mark WallGrid
    // as changed and don't trigger ceiling light rebuilds every frame.
    wall_grid.bypass_change_detection().cut_entities.clear();

    let (hidden_locs, cut_entries) = compute_visibility(
        &wall_grid,
        (focus_pos, is_room_plop),
        camera_pos,
        build_state.cur_y,
    );
    let hidden_set: HashSet<_> = hidden_locs.into_iter().collect();

    for (entity, marker, mut vis, current_layers) in vis_q.iter_mut() {
        // Camera hiding is done via RenderLayers, not Visibility, so hidden geometry
        // keeps Visibility::Inherited and still participates in shadow passes.
        if *vis != Visibility::Inherited {
            *vis = Visibility::Inherited;
        }

        let desired = if hidden_set.contains(&marker.loc) {
            RenderLayers::layer(SHADOW_ONLY_LAYER)
        } else {
            RenderLayers::default()
        };

        // Only traverse the scene tree when the layer assignment actually changes.
        if current_layers.map_or(true, |l| l != &desired) {
            apply_render_layers_to_tree(entity, &desired, &children_q, &mut commands);
        }
    }

    for (entity, marker, current_layers) in ghost_q.iter_mut() {
        let desired = if hidden_set.contains(&marker.loc) {
            RenderLayers::layer(SHADOW_ONLY_LAYER)
        } else {
            RenderLayers::default()
        };
        if current_layers.map_or(true, |l| l != &desired) {
            apply_render_layers_to_tree(entity, &desired, &children_q, &mut commands);
        }
    }

    for (entity, marker, current_layers) in overlay_q.iter_mut() {
        let desired = if hidden_set.contains(&marker.loc) {
            RenderLayers::layer(SHADOW_ONLY_LAYER)
        } else {
            RenderLayers::default()
        };
        if current_layers.map_or(true, |l| l != &desired) {
            apply_render_layers_to_tree(entity, &desired, &children_q, &mut commands);
        }
    }

    for (loc, id, is_proposed_only) in cut_entries {
        if let Some(cut_handle) = structure_list.cut_handle(id) {
            let transform = cell_transform(loc.rel_slot, crate::sparse3d::Facing::NegX, loc.cube);
            let entity = if is_proposed_only {
                commands
                    .spawn((SceneRoot(cut_handle.clone()), transform, CutCellMarker, ProposedCutMarker))
                    .id()
            } else {
                commands
                    .spawn((SceneRoot(cut_handle.clone()), transform, CutCellMarker))
                    .id()
            };
            wall_grid
                .bypass_change_detection()
                .cut_entities
                .push(entity);
        }
    }
}
