use std::collections::{HashMap, HashSet, VecDeque};

use bevy::camera::visibility::RenderLayers;
use bevy::math::Vec3;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::autotile::display::autotile_transform;
use crate::autotile::{slot_to_unoriented, spec_stem, AutotileHandles, AutotileResult};
use crate::camera::GameCamera;
use crate::input::{cursor_world_pos, BuildState};
use crate::sparse3d::{RelSlot, RelSlotCoord, Slot, SlotCoord};
use crate::structure::{StructureId, StructureList};
use crate::util::zup_scene_transform;
use crate::world::{
    cell_transform, get_real_and_proposed, get_real_or_proposed, AssembledWorld, ConstructedWorld,
    GridCellMarker, ProposedWorld, Proposal, ProposalGhostMarker, ProposalOverlayMarker,
    ProposedCutMarker, ViewableWorld,
};

/// Resolves the cut mesh for `loc` along with the transform it should be spawned
/// with. Returns one `(handle, transform)` pair per autotile mesh that has a cut variant,
/// or a single entry for non-autotile cells. Empty when no cut meshes exist.
fn get_cuts(
    loc: SlotCoord,
    id: StructureId,
    assembled: &AssembledWorld,
    structure_list: &StructureList,
    autotile_handles: &AutotileHandles,
) -> Vec<(Handle<Scene>, Transform)> {
    if let Some(results) = assembled.autotile_results.get(&loc) {
        results
            .iter()
            .filter_map(|result| {
                if let AutotileResult::Mesh { spec, .. } = result {
                    let stem = spec_stem(spec, slot_to_unoriented(loc.slot));
                    autotile_handles
                        .handles
                        .get(&stem)
                        .and_then(|(_, cut)| cut.as_ref())
                        .map(|cut| {
                            (
                                cut.clone(),
                                zup_scene_transform(autotile_transform(loc, spec)),
                            )
                        })
                } else {
                    None
                }
            })
            .collect()
    } else {
        structure_list
            .cut_handle(id)
            .map(|h| {
                (
                    h.clone(),
                    zup_scene_transform(cell_transform(
                        loc.slot,
                        crate::sparse3d::Facing::NegX,
                        loc.cube,
                    )),
                )
            })
            .into_iter()
            .collect()
    }
}

/// Selects which cutaway algorithm is active.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum CutawayMode {
    /// Traces camera-facing walls up from the cursor's floor, hiding each wall
    /// column and the floors above it (the original algorithm).
    #[default]
    FloorEdge,
    /// Hides every cell in the octant whose corner is at the cursor grid point
    /// and which contains the camera.
    SimpleOctant,
    /// Union of FloorEdge and SimpleOctant.
    FloorEdgePlusOctant,
}

/// Marker component for y-cut visibility variant entities.
#[derive(Component)]
pub struct CutCellMarker {
    pub loc: SlotCoord,
}

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
    // `try_insert` because edits can despawn the entity.
    commands.entity(entity).try_insert(layers.clone());
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
fn descend_to_floor(
    cw: &ConstructedWorld,
    pe: &ProposedWorld,
    x: i32,
    z: i32,
    from_y: i32,
) -> Option<i32> {
    for y in (from_y - 30..=from_y).rev() {
        if get_real_or_proposed(cw, pe, RelSlotCoord::new(x, y, z, RelSlot::Floor)).is_some() {
            return Some(y);
        }
    }
    None
}

/// BFS over Floor cells at `floor_y`, ignoring walls. Returns (x, z) of all reachable cells.
fn ground_floor_fill(
    cw: &ConstructedWorld,
    pe: &ProposedWorld,
    sx: i32,
    floor_y: i32,
    sz: i32,
) -> HashSet<(i32, i32)> {
    let mut visited: HashSet<(i32, i32)> = HashSet::new();
    let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
    if get_real_or_proposed(cw, pe, RelSlotCoord::new(sx, floor_y, sz, RelSlot::Floor)).is_none() {
        return visited;
    }
    visited.insert((sx, sz));
    queue.push_back((sx, sz));
    while let Some((fx, fz)) = queue.pop_front() {
        for (nx, nz) in [(fx + 1, fz), (fx - 1, fz), (fx, fz + 1), (fx, fz - 1)] {
            if !visited.contains(&(nx, nz))
                && get_real_or_proposed(cw, pe, RelSlotCoord::new(nx, floor_y, nz, RelSlot::Floor))
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
    cw: &ConstructedWorld,
    pe: &ProposedWorld,
    sx: i32,
    sy: i32,
    sz: i32,
    floor_visited: &mut HashSet<(i32, i32, i32)>,
    hidden: &mut Vec<SlotCoord>,
) -> Vec<(i32, i32)> {
    use bevy::math::IVec3;
    let mut cells: Vec<(i32, i32)> = Vec::new();
    if floor_visited.contains(&(sx, sy, sz)) {
        return cells;
    }
    if get_real_or_proposed(cw, pe, RelSlotCoord::new(sx, sy, sz, RelSlot::Floor)).is_none() {
        return cells;
    }
    floor_visited.insert((sx, sy, sz));
    hidden.push(SlotCoord {
        cube: IVec3::new(sx, sy, sz),
        slot: Slot::Floor,
    });
    cells.push((sx, sz));
    let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
    queue.push_back((sx, sz));
    while let Some((fx, fz)) = queue.pop_front() {
        let neighbors: [(i32, i32, SlotCoord); 4] = [
            (
                fx + 1,
                fz,
                SlotCoord {
                    cube: IVec3::new(fx + 1, sy, fz),
                    slot: Slot::XLoWall,
                },
            ),
            (
                fx - 1,
                fz,
                SlotCoord {
                    cube: IVec3::new(fx, sy, fz),
                    slot: Slot::XLoWall,
                },
            ),
            (
                fx,
                fz + 1,
                SlotCoord {
                    cube: IVec3::new(fx, sy, fz + 1),
                    slot: Slot::ZLoWall,
                },
            ),
            (
                fx,
                fz - 1,
                SlotCoord {
                    cube: IVec3::new(fx, sy, fz),
                    slot: Slot::ZLoWall,
                },
            ),
        ];
        for (nx, nz, wall_loc) in neighbors {
            if floor_visited.contains(&(nx, sy, nz)) {
                continue;
            }
            if get_real_or_proposed(cw, pe, wall_loc).is_some() {
                continue;
            }
            if get_real_or_proposed(cw, pe, RelSlotCoord::new(nx, sy, nz, RelSlot::Floor)).is_some()
            {
                floor_visited.insert((nx, sy, nz));
                hidden.push(SlotCoord {
                    cube: IVec3::new(nx, sy, nz),
                    slot: Slot::Floor,
                });
                cells.push((nx, nz));
                queue.push_back((nx, nz));
            }
        }
    }
    cells
}

/// For each cell in `floor_cells` (at `floor_y`), finds camera-facing exterior walls.
fn find_wall_seeds(
    cw: &ConstructedWorld,
    pe: &ProposedWorld,
    floor_cells: &HashSet<(i32, i32)>,
    floor_y: i32,
    x_dir: i32,
    z_dir: i32,
    inverted: bool,
) -> Vec<SlotCoord> {
    use bevy::math::IVec3;
    let (x_dir, z_dir) = if inverted {
        (-x_dir, -z_dir)
    } else {
        (x_dir, z_dir)
    };
    let mut walls: Vec<SlotCoord> = Vec::new();
    for &(fx, fz) in floor_cells {
        if x_dir != 0 && !floor_cells.contains(&(fx + x_dir, fz)) {
            let wx = if x_dir > 0 { fx + 1 } else { fx };
            let loc = SlotCoord {
                cube: IVec3::new(wx, floor_y, fz),
                slot: Slot::XLoWall,
            };
            if get_real_or_proposed(cw, pe, loc).is_some() {
                walls.push(loc);
            }
        }
        if z_dir != 0 && !floor_cells.contains(&(fx, fz + z_dir)) {
            let wz = if z_dir > 0 { fz + 1 } else { fz };
            let loc = SlotCoord {
                cube: IVec3::new(fx, floor_y, wz),
                slot: Slot::ZLoWall,
            };
            if get_real_or_proposed(cw, pe, loc).is_some() {
                walls.push(loc);
            }
        }
    }
    walls
}

/// Climbs a wall column upward from `bottom_loc`, hiding all walls in it.
fn climb_wall_column(
    cw: &ConstructedWorld,
    pe: &ProposedWorld,
    bottom_loc: SlotCoord,
    x_dir: i32,
    z_dir: i32,
    visited_walls: &mut HashSet<(i32, i32, i32, bool)>,
    hidden: &mut Vec<SlotCoord>,
    mut cut: Option<&mut Vec<(SlotCoord, StructureId, bool)>>,
    floor_seeds: &mut Vec<(i32, i32, i32, bool)>,
) {
    use bevy::math::IVec3;
    let is_x = bottom_loc.slot == Slot::XLoWall;
    let mut y = bottom_loc.cube.y;
    let mut first = true;
    loop {
        let key = (bottom_loc.cube.x, y, bottom_loc.cube.z, is_x);
        if !visited_walls.insert(key) {
            break;
        }
        let cur_loc = SlotCoord {
            cube: IVec3::new(bottom_loc.cube.x, y, bottom_loc.cube.z),
            slot: bottom_loc.slot,
        };
        let (real, proposed) = get_real_and_proposed(cw, pe, cur_loc);
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
        let next_loc = SlotCoord {
            cube: IVec3::new(bottom_loc.cube.x, y + 1, bottom_loc.cube.z),
            slot: bottom_loc.slot,
        };
        if get_real_or_proposed(cw, pe, next_loc).is_none() {
            let y_above = y + 1;
            match bottom_loc.slot {
                Slot::XLoWall => {
                    let wx = bottom_loc.cube.x;
                    floor_seeds.push((wx - 1, y_above, bottom_loc.cube.z, x_dir > 0));
                    floor_seeds.push((wx, y_above, bottom_loc.cube.z, x_dir < 0));
                }
                Slot::ZLoWall => {
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

pub fn compute_floor_edge(
    cw: &ConstructedWorld,
    pe: &ProposedWorld,
    (focus_location, is_room_plop): (Vec3, bool),
    camera_location: Vec3,
    cur_y: i32,
) -> (Vec<SlotCoord>, Vec<(SlotCoord, StructureId, bool)>) {
    use bevy::math::IVec3;
    let mut hidden: Vec<SlotCoord> = Vec::new();
    let mut cut: Vec<(SlotCoord, StructureId, bool)> = Vec::new();

    let (x_dir, z_dir) = camera_facing_dirs(focus_location, camera_location);
    let (sx, sz) = cursor_cube(focus_location, camera_location, is_room_plop);
    let Some(floor_y) = descend_to_floor(cw, pe, sx, sz, cur_y) else {
        return (hidden, cut);
    };

    let ground_cells = ground_floor_fill(cw, pe, sx, floor_y, sz);
    if ground_cells.is_empty() {
        return (hidden, cut);
    }

    let mut visited_walls: HashSet<(i32, i32, i32, bool)> = HashSet::new();
    let mut floor_visited: HashSet<(i32, i32, i32)> = HashSet::new();
    let mut pending_walls: VecDeque<SlotCoord> = VecDeque::new();
    let mut pending_floors: VecDeque<(i32, i32, i32, bool)> = VecDeque::new();

    for wall_loc in find_wall_seeds(cw, pe, &ground_cells, floor_y, x_dir, z_dir, false) {
        pending_walls.push_back(wall_loc);
    }

    loop {
        while let Some(wall_loc) = pending_walls.pop_front() {
            let mut floor_seeds: Vec<(i32, i32, i32, bool)> = Vec::new();
            climb_wall_column(
                cw,
                pe,
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

        let upper_cells = upper_floor_fill(cw, pe, fx, fy, fz, &mut floor_visited, &mut hidden);
        if upper_cells.is_empty() {
            continue;
        }

        let upper_set: HashSet<(i32, i32)> = upper_cells.into_iter().collect();
        for wall_loc in find_wall_seeds(cw, pe, &upper_set, fy, x_dir, z_dir, inverted) {
            pending_walls.push_back(wall_loc);
        }
    }

    // For each hidden floor, also hide Room objects above it until the next floor.
    let hidden_floors: Vec<SlotCoord> = hidden
        .iter()
        .filter(|loc| loc.slot == Slot::Floor)
        .copied()
        .collect();
    for floor_loc in hidden_floors {
        let (x, z) = (floor_loc.cube.x, floor_loc.cube.z);
        let mut y = floor_loc.cube.y;
        loop {
            let room_loc = SlotCoord {
                cube: IVec3::new(x, y, z),
                slot: Slot::Room,
            };
            if get_real_or_proposed(cw, pe, room_loc).is_some() {
                hidden.push(room_loc);
            }
            if get_real_or_proposed(
                cw,
                pe,
                SlotCoord {
                    cube: IVec3::new(x, y + 1, z),
                    slot: Slot::Floor,
                },
            )
            .is_some()
            {
                break;
            }
            y += 1;
            if y > floor_loc.cube.y + 30 {
                break;
            }
        }
    }

    // The floors visited by the initial traverse are replaced with the "cut" version.
    for &(gx, gz) in &ground_cells {
        let floor_loc = SlotCoord {
            cube: IVec3::new(gx, floor_y, gz),
            slot: Slot::Floor,
        };
        let (real, proposed) = get_real_and_proposed(cw, pe, floor_loc);
        if let Some(cell) = real.or(proposed) {
            hidden.push(floor_loc);
            cut.push((floor_loc, cell.id, real.is_none()));
        }
    }

    (hidden, cut)
}

/// Returns true if `loc` falls inside the SimpleOctant hidden region.
fn octant_hidden(loc: SlotCoord, sx: i32, sz: i32, cur_y: i32, x_neg: bool, z_neg: bool) -> bool {
    if loc.cube.y < cur_y || (loc.cube.y == cur_y && loc.slot == Slot::Floor) {
        return false;
    }
    let x_ok = if x_neg {
        loc.cube.x < sx
    } else {
        loc.cube.x >= sx
    };
    let z_ok = if z_neg {
        loc.cube.z < sz
    } else {
        loc.cube.z >= sz
    };
    if x_ok && z_ok {
        return true;
    }
    false
}

/// Collects cut-face entries for the SimpleOctant algorithm.
fn simple_octant_cuts(
    cw: &ConstructedWorld,
    pe: &ProposedWorld,
    sx: i32,
    sz: i32,
    cut_y: i32,
    x_neg: bool,
    z_neg: bool,
) -> Vec<(SlotCoord, StructureId, bool)> {
    let is_cut_face = |loc: SlotCoord| {
        let x_ok = if x_neg {
            loc.cube.x < sx
        } else {
            loc.cube.x >= sx
        };
        let z_ok = if z_neg {
            loc.cube.z < sz
        } else {
            loc.cube.z >= sz
        };
        x_ok && z_ok && loc.cube.y == cut_y && loc.slot != Slot::Floor
    };
    let mut cuts = vec![];
    for (loc, cell) in cw.contents.iter() {
        if is_cut_face(loc) {
            cuts.push((loc, cell.id, false));
        }
    }
    for (loc, proposal) in pe.proposed_changes.iter() {
        if is_cut_face(loc) && cw.contents.get(loc).is_none() {
            if let Proposal::Place(cell) = proposal {
                cuts.push((loc, cell.id, true));
            }
        }
    }
    cuts
}

/// Per-frame hidden-cell membership test, abstracted over algorithm.
enum HiddenPredicate {
    Set(HashSet<SlotCoord>),
    /// `x_neg`/`z_neg`: true means the camera-side is the *negative* half-space.
    Octant {
        sx: i32,
        sz: i32,
        cur_y: i32,
        x_neg: bool,
        z_neg: bool,
    },
    /// Union of a FloorEdge set and a SimpleOctant predicate.
    Combined {
        set: HashSet<SlotCoord>,
        sx: i32,
        sz: i32,
        cur_y: i32,
        x_neg: bool,
        z_neg: bool,
    },
}

impl HiddenPredicate {
    fn contains(&self, loc: SlotCoord) -> bool {
        match self {
            HiddenPredicate::Set(s) => s.contains(&loc),
            HiddenPredicate::Octant {
                sx,
                sz,
                cur_y,
                x_neg,
                z_neg,
            } => octant_hidden(loc, *sx, *sz, *cur_y, *x_neg, *z_neg),
            HiddenPredicate::Combined {
                set,
                sx,
                sz,
                cur_y,
                x_neg,
                z_neg,
            } => set.contains(&loc) || octant_hidden(loc, *sx, *sz, *cur_y, *x_neg, *z_neg),
        }
    }
}

/// Propagates `RenderLayers` from scene-root entities to newly-spawned children.
pub fn propagate_render_layers_system(
    changed_q: Query<
        (Entity, Option<&RenderLayers>),
        (
            Or<(With<GridCellMarker>, With<ProposalGhostMarker>)>,
            Changed<Children>,
        ),
    >,
    children_q: Query<&Children>,
    mut commands: Commands,
) {
    for (entity, layers) in changed_q.iter() {
        let effective = layers.cloned().unwrap_or_default();
        if effective == RenderLayers::default() {
            continue; // layer 0 is the default; newly-spawned children already have it
        }
        if let Ok(entity_children) = children_q.get(entity) {
            for child in entity_children.iter() {
                apply_render_layers_to_tree(child, &effective, &children_q, &mut commands);
            }
        }
    }
}

pub fn update_cutaway_system(
    mut commands: Commands,
    constructed: Res<ConstructedWorld>,
    pending: Res<ProposedWorld>,
    assembled: Res<AssembledWorld>,
    mut viewable: ResMut<ViewableWorld>,
    structure_list: Res<StructureList>,
    autotile_handles: Res<AutotileHandles>,
    build_state: Res<BuildState>,
    cutaway_mode: Res<CutawayMode>,
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
) {
    let Ok((_, cam_gt)) = camera_q.single() else {
        return;
    };
    let camera_pos = cam_gt.translation();
    let focus_pos = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32)
        .unwrap_or_else(|| Vec3::new(0.0, build_state.cur_y as f32, 0.0));

    let (hidden, cut_entries): (HiddenPredicate, Vec<(SlotCoord, StructureId, bool)>) =
        match *cutaway_mode {
            CutawayMode::FloorEdge => {
                let is_room_plop = constructed
                    .structure_is_room_plop(StructureId(build_state.selected_structure as u32));
                let (locs, cuts) = compute_floor_edge(
                    &constructed,
                    &pending,
                    (focus_pos, is_room_plop),
                    camera_pos,
                    build_state.cur_y,
                );
                (HiddenPredicate::Set(locs.into_iter().collect()), cuts)
            }
            CutawayMode::SimpleOctant => {
                let sx = focus_pos.x.round() as i32;
                let sz = focus_pos.z.round() as i32;
                let cut_y = build_state.cur_y;
                let x_neg = camera_pos.x < focus_pos.x;
                let z_neg = camera_pos.z < focus_pos.z;
                let cuts = simple_octant_cuts(&constructed, &pending, sx, sz, cut_y, x_neg, z_neg);
                let pred = HiddenPredicate::Octant {
                    sx,
                    sz,
                    cur_y: cut_y,
                    x_neg,
                    z_neg,
                };
                (pred, cuts)
            }
            CutawayMode::FloorEdgePlusOctant => {
                let sx = focus_pos.x.round() as i32;
                let sz = focus_pos.z.round() as i32;
                let cut_y = build_state.cur_y;
                let x_neg = camera_pos.x < focus_pos.x;
                let z_neg = camera_pos.z < focus_pos.z;
                let is_room_plop = constructed
                    .structure_is_room_plop(StructureId(build_state.selected_structure as u32));
                let (locs, mut cuts) = compute_floor_edge(
                    &constructed,
                    &pending,
                    (focus_pos, is_room_plop),
                    camera_pos,
                    build_state.cur_y,
                );
                // Merge cut entries, deduplicating by location (real beats proposed-only).
                let mut cut_map: HashMap<SlotCoord, (StructureId, bool)> = cuts
                    .drain(..)
                    .map(|(loc, id, po)| (loc, (id, po)))
                    .collect();
                for (loc, id, po) in
                    simple_octant_cuts(&constructed, &pending, sx, sz, cut_y, x_neg, z_neg)
                {
                    cut_map
                        .entry(loc)
                        .and_modify(|e| {
                            if !po {
                                e.1 = false;
                            }
                        })
                        .or_insert((id, po));
                }
                let cuts = cut_map
                    .into_iter()
                    .map(|(loc, (id, po))| (loc, id, po))
                    .collect();
                let pred = HiddenPredicate::Combined {
                    set: locs.into_iter().collect(),
                    sx,
                    sz,
                    cur_y: cut_y,
                    x_neg,
                    z_neg,
                };
                (pred, cuts)
            }
        };

    for (entity, marker, mut vis, current_layers) in vis_q.iter_mut() {
        if *vis != Visibility::Inherited {
            *vis = Visibility::Inherited;
        }

        let desired = if hidden.contains(marker.loc) {
            RenderLayers::layer(SHADOW_ONLY_LAYER)
        } else {
            RenderLayers::default()
        };

        if current_layers.map_or(true, |l| l != &desired) {
            apply_render_layers_to_tree(entity, &desired, &children_q, &mut commands);
        }
    }

    for (entity, marker, current_layers) in ghost_q.iter_mut() {
        let desired = if hidden.contains(marker.loc) {
            RenderLayers::layer(SHADOW_ONLY_LAYER)
        } else {
            RenderLayers::default()
        };
        if current_layers.map_or(true, |l| l != &desired) {
            apply_render_layers_to_tree(entity, &desired, &children_q, &mut commands);
        }
    }

    for (entity, marker, current_layers) in overlay_q.iter_mut() {
        let desired = if hidden.contains(marker.loc) {
            RenderLayers::layer(SHADOW_ONLY_LAYER)
        } else {
            RenderLayers::default()
        };
        if current_layers.map_or(true, |l| l != &desired) {
            apply_render_layers_to_tree(entity, &desired, &children_q, &mut commands);
        }
    }

    // Separate proposed-only cuts from regular cuts.
    let mut desired_proposed: HashMap<SlotCoord, StructureId> = HashMap::new();
    let mut desired_regular: HashMap<SlotCoord, StructureId> = HashMap::new();
    for (loc, id, is_proposed_only) in cut_entries {
        if is_proposed_only {
            desired_proposed.insert(loc, id);
        } else {
            desired_regular.insert(loc, id);
        }
    }

    // Diff regular cut entities so unchanged cuts persist across frames.
    viewable.cut_entities.retain(|loc, (id, entities)| {
        if desired_regular.get(loc) == Some(id) {
            true
        } else {
            for e in entities.iter() {
                commands.entity(*e).despawn();
            }
            false
        }
    });

    // Spawn regular cut entities for locations newly entering the cut zone.
    for (loc, id) in desired_regular {
        if viewable.cut_entities.get(&loc).map(|(i, _)| *i) == Some(id) {
            continue;
        }
        let cuts = get_cuts(loc, id, &assembled, &structure_list, &autotile_handles);
        if !cuts.is_empty() {
            let entities: Vec<Entity> = cuts
                .into_iter()
                .map(|(cut_handle, transform)| {
                    commands
                        .spawn((SceneRoot(cut_handle), transform, CutCellMarker { loc }))
                        .id()
                })
                .collect();
            viewable.cut_entities.insert(loc, (id, entities));
        }
    }

    // Despawn proposed cut entities whose locations left the cut zone.
    viewable.proposed_cut_entities.retain(|loc, entities| {
        if desired_proposed.contains_key(loc) {
            true
        } else {
            for e in entities.iter() {
                commands.entity(*e).despawn();
            }
            false
        }
    });

    // Spawn proposed cut entities for locations newly entering the cut zone.
    for (loc, id) in desired_proposed {
        let cuts = get_cuts(loc, id, &assembled, &structure_list, &autotile_handles);
        if !cuts.is_empty() {
            if let std::collections::hash_map::Entry::Vacant(v) =
                viewable.proposed_cut_entities.entry(loc)
            {
                let entities: Vec<Entity> = cuts
                    .into_iter()
                    .map(|(cut_handle, transform)| {
                        commands
                            .spawn((
                                SceneRoot(cut_handle),
                                transform,
                                CutCellMarker { loc },
                                ProposedCutMarker,
                            ))
                            .id()
                    })
                    .collect();
                v.insert(entities);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use assert2::check;

    use super::*;
    use std::collections::HashSet;

    use bevy::math::IVec3;

    use crate::build_helpers::Builder;
    use crate::sparse3d::Facing;
    use crate::structure::load_structure_info;
    use crate::world::{AssembledWorld, Cell, ConstructedWorld, ProposedWorld, ViewableWorld};

    /// Builds a 3×1×3-cube room and returns the contents plus the loaded structures.
    fn two_level_room() -> (
        crate::sparse3d::Sparse3D<crate::world::Cell>,
        Vec<crate::structure::StructureInfo>,
    ) {
        let structures = load_structure_info();
        let mut builder = Builder::new(&structures);
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(2, 0, 2),
            RelSlot::Floor,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 1, 0),
            IVec3::new(2, 1, 2),
            RelSlot::Floor,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 0, 2),
            RelSlot::XLoWall,
            None,
        );
        builder.build_plane(
            IVec3::new(3, 0, 0),
            IVec3::new(3, 0, 2),
            RelSlot::XLoWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(2, 0, 0),
            RelSlot::ZLoWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 3),
            IVec3::new(2, 0, 3),
            RelSlot::ZLoWall,
            None,
        );
        (builder.get(), structures)
    }

    #[test]
    fn test_visibility_cursor_inside_hides_right_wall_and_roof() {
        let (contents, structures) = two_level_room();
        let mut cw = ConstructedWorld::new(structures);
        let pe = ProposedWorld::new();
        cw.contents = contents;

        let camera_pos = Vec3::new(10.0, 5.0, 1.5);
        let focus_pos = Vec3::new(1.5, 0.0, 1.5);

        let (hidden_locs, _cut) = compute_floor_edge(&cw, &pe, (focus_pos, false), camera_pos, 0);
        let hidden_set: HashSet<SlotCoord> = hidden_locs.into_iter().collect();

        check!(!hidden_set.is_empty());

        let hidden_right_wall: Vec<SlotCoord> = hidden_set
            .iter()
            .filter(|l| l.slot == Slot::XLoWall && l.cube.x == 3)
            .copied()
            .collect();
        check!(hidden_right_wall.len() == 3);

        let hidden_roof_tiles: Vec<SlotCoord> = hidden_set
            .iter()
            .filter(|l| l.slot == Slot::Floor && l.cube.y == 1)
            .copied()
            .collect();
        check!(hidden_roof_tiles.len() == 9);

        check!(SHADOW_ONLY_LAYER == 1);
    }

    fn shadow_layer_test_app() -> (App, SlotCoord) {
        let loc = SlotCoord {
            cube: IVec3::new(0, 1, 0),
            slot: Slot::Floor,
        };

        let mut app = App::new();
        app.add_systems(
            Update,
            (
                update_cutaway_system,
                propagate_render_layers_system.after(update_cutaway_system),
            ),
        );

        let structures = load_structure_info();
        let mut cw = ConstructedWorld::new(structures);
        cw.contents.set(
            loc,
            Cell {
                id: StructureId(0),
                facing: Facing::NegX,
                evaluation: None,
                material: crate::world::Material::default(),
            },
        );
        app.insert_resource(cw);
        app.insert_resource(ProposedWorld::new());
        app.insert_resource(AssembledWorld::new());
        app.insert_resource(ViewableWorld::new());
        app.insert_resource(StructureList::default());
        app.insert_resource(AutotileHandles {
            handles: std::collections::HashMap::new(),
        });
        app.insert_resource(BuildState::default());
        app.insert_resource(CutawayMode::SimpleOctant);

        app.world_mut().spawn((
            Camera::default(),
            GlobalTransform::from(Transform::from_xyz(10.0, 5.0, 10.0)),
            GameCamera,
        ));

        (app, loc)
    }

    #[test]
    fn test_cutaway_shadow_layer_set_on_root_and_loaded_children() {
        let (mut app, loc) = shadow_layer_test_app();

        let grandchild = app.world_mut().spawn_empty().id();
        let child = app.world_mut().spawn(Visibility::default()).id();
        let root = app
            .world_mut()
            .spawn((GridCellMarker { loc }, Visibility::default()))
            .id();
        app.world_mut().entity_mut(child).add_child(grandchild);
        app.world_mut().entity_mut(root).add_child(child);

        app.update();

        let desired = RenderLayers::layer(SHADOW_ONLY_LAYER);
        check!(app.world().get::<RenderLayers>(root).cloned() == Some(desired.clone()));
        check!(app.world().get::<RenderLayers>(child).cloned() == Some(desired.clone()));
        check!(app.world().get::<RenderLayers>(grandchild).cloned() == Some(desired));
    }

    #[test]
    fn test_shadow_layer_propagates_to_late_loaded_children() {
        let (mut app, loc) = shadow_layer_test_app();

        let root = app
            .world_mut()
            .spawn((GridCellMarker { loc }, Visibility::default()))
            .id();

        app.update();

        let desired = RenderLayers::layer(SHADOW_ONLY_LAYER);
        check!(app.world().get::<RenderLayers>(root).cloned() == Some(desired.clone()));

        let grandchild = app.world_mut().spawn_empty().id();
        let child = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(child).add_child(grandchild);
        app.world_mut().entity_mut(root).add_child(child);

        app.update();

        check!(app.world().get::<RenderLayers>(child).cloned() == Some(desired.clone()));
        check!(app.world().get::<RenderLayers>(grandchild).cloned() == Some(desired));
    }
}
