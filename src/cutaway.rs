use std::collections::{HashMap, HashSet, VecDeque};

use bevy::camera::visibility::RenderLayers;
use bevy::math::Vec3;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::autotile::display::autotile_transform;
use crate::autotile::{slot_to_unoriented, spec_stem, AutotileHandles, AutotiledMeshes};
use crate::camera::GameCamera;
use crate::city::{
    cell_transform, get_real_and_proposed, get_real_or_proposed, AssembledCity, City,
    ConstructedCity, GridCellMarker, MaterialAssets, Proposal, ProposalGhostMarker,
    ProposalOverlayMarker, ProposedCity, ProposedCutMarker, ViewableWorld,
};
use crate::eorf::{EorfId, EorfList};
use crate::gi_material::{GiMaterial, ShadowOnlyMaterial};
use crate::input::{cursor_world_pos, BuildState};
use crate::materials::MaterialList;
use crate::sparse3d::{RelSlot, RelSlotCoord, Slot, SlotCoord};
use bevy::pbr::Material;

/// Resolves the cut mesh for `loc` along with the transform it should be spawned
/// with. Returns one `(handle, transform)` pair per autotile mesh that has a cut variant,
/// or a single entry for non-autotile cells. Empty when no cut meshes exist.
fn get_cuts(
    loc: SlotCoord,
    id: EorfId,
    assembled: &AssembledCity,
    structure_list: &EorfList,
    autotile_handles: &AutotileHandles,
) -> Vec<(Handle<Scene>, Transform)> {
    if let Some(results) = assembled.autotile_results.get(&loc) {
        results
            .iter()
            .filter_map(|result| {
                if let AutotiledMeshes::Mesh { spec, .. } = result {
                    let stem = spec_stem(spec, slot_to_unoriented(loc.slot));
                    autotile_handles
                        .handles
                        .get(&stem)
                        .and_then(|(_, cut)| cut.as_ref())
                        .map(|cut| {
                            (
                                cut.clone(),
                                // The cutaway view's cut meshes don't track a
                                // `WallPlop` cell's actual `facing` (that would
                                // need threading it through `cut_entries`
                                // alongside `EorfId`); they always render
                                // unflipped, same as before `WallPlop` existed.
                                autotile_transform(loc, crate::sparse3d::Facing::NegX, spec),
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
                    cell_transform(loc.slot, crate::sparse3d::Facing::NegX, loc.cube),
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

/// Layer used for geometry that should be hidden from the main camera. Ghosts and
/// overlays (which don't need to cast shadows when hidden) still use this; real
/// cells instead swap to `ShadowOnlyMaterial` so they keep casting — see
/// `sync_cutaway_shadow_material` and the note on `queue_shadows` there.
const SHADOW_ONLY_LAYER: usize = 1;

/// Marks a real-cell (`GridCellMarker`) root that is currently cutaway-hidden, i.e.
/// its mesh leaves have been swapped to `ShadowOnlyMaterial` (invisible to the
/// camera, still casting a shadow). Maintained by `update_cutaway_system`; acted on
/// by `sync_cutaway_shadow_material`.
#[derive(Component)]
pub struct CutawayHidden;

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
    cw: &ConstructedCity,
    pe: &ProposedCity,
    x: i32,
    z: i32,
    from_y: i32,
) -> Option<i32> {
    (from_y - 30..=from_y).rev().find(|&y| {
        get_real_or_proposed(cw, pe, RelSlotCoord::new(x, y, z, RelSlot::Floor)).is_some()
    })
}

/// BFS over Floor cells at `floor_y`, ignoring walls. Returns (x, z) of all reachable cells.
fn ground_floor_fill(
    cw: &ConstructedCity,
    pe: &ProposedCity,
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
    cw: &ConstructedCity,
    pe: &ProposedCity,
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
    cw: &ConstructedCity,
    pe: &ProposedCity,
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

/// Where `climb_wall_column` records what it found, all gathered by the caller's loop
/// across many wall columns.
struct ClimbOutputs<'a> {
    visited_walls: &'a mut HashSet<(i32, i32, i32, bool)>,
    hidden: &'a mut Vec<SlotCoord>,
    cut: Option<&'a mut Vec<(SlotCoord, EorfId, bool)>>,
    floor_seeds: &'a mut Vec<(i32, i32, i32, bool)>,
}

/// Climbs a wall column upward from `bottom_loc`, hiding all walls in it.
fn climb_wall_column(
    cw: &ConstructedCity,
    pe: &ProposedCity,
    bottom_loc: SlotCoord,
    (x_dir, z_dir): (i32, i32),
    out: ClimbOutputs,
) {
    use bevy::math::IVec3;
    let ClimbOutputs {
        visited_walls,
        hidden,
        mut cut,
        floor_seeds,
    } = out;
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
    cw: &ConstructedCity,
    pe: &ProposedCity,
    (focus_location, is_room_plop): (Vec3, bool),
    camera_location: Vec3,
    cur_y: i32,
) -> (Vec<SlotCoord>, Vec<(SlotCoord, EorfId, bool)>) {
    use bevy::math::IVec3;
    let mut hidden: Vec<SlotCoord> = Vec::new();
    let mut cut: Vec<(SlotCoord, EorfId, bool)> = Vec::new();

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
                (x_dir, z_dir),
                ClimbOutputs {
                    visited_walls: &mut visited_walls,
                    hidden: &mut hidden,
                    cut: if wall_loc.cube.y == floor_y {
                        Some(&mut cut)
                    } else {
                        None // Cuts are only at the bottom level
                    },
                    floor_seeds: &mut floor_seeds,
                },
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

/// Whether `loc`'s (x, z) falls within the SimpleOctant's horizontal half-space.
/// `x_neg`/`z_neg`: true means the camera-side is the *negative* half-space.
fn in_octant_xz(loc: SlotCoord, sx: i32, sz: i32, x_neg: bool, z_neg: bool) -> bool {
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
    x_ok && z_ok
}

/// Returns true if `loc` falls inside the SimpleOctant hidden region.
fn octant_hidden(loc: SlotCoord, sx: i32, sz: i32, cur_y: i32, x_neg: bool, z_neg: bool) -> bool {
    if loc.cube.y < cur_y || (loc.cube.y == cur_y && loc.slot == Slot::Floor) {
        return false;
    }
    in_octant_xz(loc, sx, sz, x_neg, z_neg)
}

/// Collects cut-face entries for the SimpleOctant algorithm.
fn simple_octant_cuts(
    cw: &ConstructedCity,
    pe: &ProposedCity,
    sx: i32,
    sz: i32,
    cut_y: i32,
    x_neg: bool,
    z_neg: bool,
) -> Vec<(SlotCoord, EorfId, bool)> {
    let is_cut_face = |loc: SlotCoord| {
        in_octant_xz(loc, sx, sz, x_neg, z_neg) && loc.cube.y == cut_y && loc.slot != Slot::Floor
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
/// Real cells (`GridCellMarker`) no longer use `RenderLayers` for cutaway hiding
/// (they swap materials instead — see `sync_cutaway_shadow_material`), so only
/// ghosts still need this.
pub fn propagate_render_layers_system(
    changed_q: Query<
        (Entity, Option<&RenderLayers>),
        (With<ProposalGhostMarker>, Changed<Children>),
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
    world: City,
    mut viewable: ResMut<ViewableWorld>,
    structure_list: Res<EorfList>,
    autotile_handles: Res<AutotileHandles>,
    build_state: Res<BuildState>,
    cutaway_mode: Res<CutawayMode>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    vis_q: Query<(Entity, &GridCellMarker, Has<CutawayHidden>)>,
    ghost_q: Query<(Entity, &ProposalGhostMarker, Option<&RenderLayers>)>,
    overlay_q: Query<(Entity, &ProposalOverlayMarker, Option<&RenderLayers>)>,
    children_q: Query<&Children>,
) {
    let City {
        constructed,
        pending,
        assembled,
    } = world;
    let Ok((_, cam_gt)) = camera_q.single() else {
        return;
    };
    let camera_pos = cam_gt.translation();
    let focus_pos = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32)
        .unwrap_or_else(|| Vec3::new(0.0, build_state.cur_y as f32, 0.0));

    let (hidden, cut_entries): (HiddenPredicate, Vec<(SlotCoord, EorfId, bool)>) =
        match *cutaway_mode {
            CutawayMode::FloorEdge => {
                let is_room_plop = constructed
                    .structure_is_room_plop(EorfId(build_state.selected_structure as u32));
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
                    .structure_is_room_plop(EorfId(build_state.selected_structure as u32));
                let (locs, mut cuts) = compute_floor_edge(
                    &constructed,
                    &pending,
                    (focus_pos, is_room_plop),
                    camera_pos,
                    build_state.cur_y,
                );
                // Merge cut entries, deduplicating by location (real beats proposed-only).
                let mut cut_map: HashMap<SlotCoord, (EorfId, bool)> = cuts
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

    // Real cells: toggle the `CutawayHidden` marker on transitions only (so
    // `Added`/`RemovedComponents` in `sync_cutaway_shadow_material` fire once per
    // change). The actual material swap that makes them invisible-but-casting lives
    // there — render layers can't do it (see that system's note).
    for (entity, marker, is_hidden_now) in vis_q.iter() {
        let want_hidden = hidden.contains(marker.loc);
        if want_hidden && !is_hidden_now {
            commands.entity(entity).insert(CutawayHidden);
        } else if !want_hidden && is_hidden_now {
            commands.entity(entity).remove::<CutawayHidden>();
        }
    }

    for (entity, marker, current_layers) in ghost_q.iter() {
        sync_proposal_render_layers(
            entity,
            current_layers,
            hidden.contains(marker.loc),
            &children_q,
            &mut commands,
        );
    }

    for (entity, marker, current_layers) in overlay_q.iter() {
        sync_proposal_render_layers(
            entity,
            current_layers,
            hidden.contains(marker.loc),
            &children_q,
            &mut commands,
        );
    }

    // Separate proposed-only cuts from regular cuts.
    let mut desired_proposed: HashMap<SlotCoord, EorfId> = HashMap::new();
    let mut desired_regular: HashMap<SlotCoord, EorfId> = HashMap::new();
    for (loc, id, is_proposed_only) in cut_entries {
        if is_proposed_only {
            desired_proposed.insert(loc, id);
        } else {
            desired_regular.insert(loc, id);
        }
    }

    sync_cut_entities(
        &mut viewable.cut_entities,
        desired_regular,
        false,
        &assembled,
        &structure_list,
        &autotile_handles,
        &mut commands,
    );
    sync_cut_entities(
        &mut viewable.proposed_cut_entities,
        desired_proposed,
        true,
        &assembled,
        &structure_list,
        &autotile_handles,
        &mut commands,
    );
}

/// Updates a proposal-overlay entity's (and its subtree's) render layers to reflect
/// whether its cell is currently cutaway-hidden.
fn sync_proposal_render_layers(
    entity: Entity,
    current_layers: Option<&RenderLayers>,
    want_hidden: bool,
    children_q: &Query<&Children>,
    commands: &mut Commands,
) {
    let desired = if want_hidden {
        RenderLayers::layer(SHADOW_ONLY_LAYER)
    } else {
        RenderLayers::default()
    };
    if current_layers.map_or(desired != RenderLayers::default(), |l| l != &desired) {
        apply_render_layers_to_tree(entity, &desired, children_q, commands);
    }
}

/// Diffs `entities` against `desired` (loc -> structure id): despawns entities for
/// locations that dropped out or changed structure, then spawns entities for locations
/// newly entering the cut zone. Locations whose desired id is unchanged are left alone
/// so unchanged cuts persist across frames. `is_proposed` controls whether spawned
/// entities also get a `ProposedCutMarker`.
fn sync_cut_entities(
    entities: &mut HashMap<SlotCoord, (EorfId, Vec<Entity>)>,
    desired: HashMap<SlotCoord, EorfId>,
    is_proposed: bool,
    assembled: &AssembledCity,
    structure_list: &EorfList,
    autotile_handles: &AutotileHandles,
    commands: &mut Commands,
) {
    entities.retain(|loc, (id, ents)| {
        if desired.get(loc) == Some(id) {
            true
        } else {
            for e in ents.iter() {
                commands.entity(*e).despawn();
            }
            false
        }
    });

    for (loc, id) in desired {
        if entities.get(&loc).map(|(i, _)| *i) == Some(id) {
            continue;
        }
        let cuts = get_cuts(loc, id, assembled, structure_list, autotile_handles);
        if !cuts.is_empty() {
            let new_entities: Vec<Entity> = cuts
                .into_iter()
                .map(|(cut_handle, transform)| {
                    let mut ec =
                        commands.spawn((SceneRoot(cut_handle), transform, CutCellMarker { loc }));
                    if is_proposed {
                        ec.insert(ProposedCutMarker);
                    }
                    ec.id()
                })
                .collect();
            entities.insert(loc, (id, new_entities));
        }
    }
}

/// Collects `root` and all of its descendants (the whole scene subtree).
fn collect_subtree(root: Entity, children_q: &Query<&Children>) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        out.push(e);
        if let Ok(children) = children_q.get(e) {
            for child in children.iter() {
                stack.push(child);
            }
        }
    }
    out
}

/// Queues a swap of every `MeshMaterial3d<From>` leaf in `entities` to
/// `MeshMaterial3d<To>`. Done in a queued closure so it's safe against entities
/// despawned before the flush (edits respawn cells constantly).
fn swap_leaf_material<From: Material, To: Material>(
    commands: &mut Commands,
    entities: Vec<Entity>,
    to: Handle<To>,
) {
    commands.queue(move |world: &mut World| {
        for e in entities {
            if let Ok(mut em) = world.get_entity_mut(e) {
                if em.contains::<MeshMaterial3d<From>>() {
                    em.remove::<MeshMaterial3d<From>>();
                    em.insert(MeshMaterial3d(to.clone()));
                }
            }
        }
    });
}

/// Applies the `CutawayHidden` marker (maintained by `update_cutaway_system`) to a
/// real cell's mesh leaves by swapping their material.
///
/// Why a material swap rather than render layers: Bevy's `queue_shadows` filters
/// shadow casters by the *camera's* render layers, so a caster must share a layer
/// with the camera to cast a shadow into its view — which is also exactly what
/// makes it visible. There's no layer that is "invisible to the camera but still
/// casts for it". `ShadowOnlyMaterial` instead stays on the camera's layer and
/// discards in the color pass, so the cell is invisible but keeps casting.
pub fn sync_cutaway_shadow_material(
    mut commands: Commands,
    world: City,
    material_list: Res<MaterialList>,
    material_assets: Res<MaterialAssets>,
    children_q: Query<&Children>,
    child_of_q: Query<&ChildOf>,
    cell_root_q: Query<&GridCellMarker>,
    hidden_root_q: Query<(), With<CutawayHidden>>,
    newly_hidden: Query<Entity, Added<CutawayHidden>>,
    mut unhidden: RemovedComponents<CutawayHidden>,
    new_gi_leaves: Query<Entity, Added<MeshMaterial3d<GiMaterial>>>,
) {
    let shadow = material_assets.shadow_only();

    // Cells that just became hidden: hide every current GI leaf in their subtree.
    for root in newly_hidden.iter() {
        let subtree = collect_subtree(root, &children_q);
        swap_leaf_material::<GiMaterial, ShadowOnlyMaterial>(
            &mut commands,
            subtree,
            shadow.clone(),
        );
    }

    // Cells that just became visible again: restore the cell's real material.
    for root in unhidden.read() {
        let Ok(marker) = cell_root_q.get(root) else {
            continue; // root was despawned along with the removal — nothing to restore
        };
        let Some(cell) = get_real_or_proposed(&world.constructed, &world.pending, marker.loc)
        else {
            continue;
        };
        let material = cell.material(&world.constructed.eorfs, &material_list);
        let gi = material_assets.get(material);
        let subtree = collect_subtree(root, &children_q);
        swap_leaf_material::<ShadowOnlyMaterial, GiMaterial>(&mut commands, subtree, gi);
    }

    // GI leaves that appear while their cell is already hidden (a cell respawned by
    // an edit, or scene children that loaded late): hide them too. This is the
    // material-swap analogue of `propagate_render_layers_system` for ghosts, and it
    // closes the same ordering race between cell (re)spawn and the cutaway state.
    for leaf in new_gi_leaves.iter() {
        let mut node = leaf;
        loop {
            if cell_root_q.contains(node) {
                if hidden_root_q.contains(node) {
                    swap_leaf_material::<GiMaterial, ShadowOnlyMaterial>(
                        &mut commands,
                        vec![leaf],
                        shadow.clone(),
                    );
                }
                break;
            }
            match child_of_q.get(node) {
                Ok(child_of) => node = child_of.0,
                Err(_) => break,
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
    use crate::city::{AssembledCity, Cell, ConstructedCity, ProposedCity, ViewableWorld};
    use crate::eorf::load_structure_info;
    use crate::sparse3d::Facing;

    /// Builds a 3×1×3-cube room and returns the contents plus the loaded structures.
    fn two_level_room() -> (
        crate::sparse3d::Sparse3D<crate::city::Cell>,
        Vec<crate::eorf::EorfInfo>,
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

    // ── camera_facing_dirs ────────────────────────────────────────────────

    #[test]
    fn camera_facing_dirs_pure_x() {
        // Camera directly to the +X side
        let (xd, zd) = camera_facing_dirs(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0));
        check!(xd == 1);
        check!(zd == 0);
    }

    #[test]
    fn camera_facing_dirs_pure_neg_z() {
        let (xd, zd) = camera_facing_dirs(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0));
        check!(xd == 0);
        check!(zd == -1);
    }

    #[test]
    fn camera_facing_dirs_diagonal_gives_both_signs() {
        // 45° diagonal: both components should be ±1
        let (xd, zd) = camera_facing_dirs(Vec3::ZERO, Vec3::new(5.0, 0.0, 5.0));
        check!(xd == 1);
        check!(zd == 1);
    }

    #[test]
    fn camera_facing_dirs_near_cardinal_snaps() {
        // Within 5° of +X axis — should snap to (1, 0)
        let small_z = 10.0f32 * 3.0f32.to_radians().tan(); // ~3° off +X
        let (xd, zd) = camera_facing_dirs(Vec3::ZERO, Vec3::new(10.0, 0.0, small_z));
        check!(xd == 1);
        check!(zd == 0);
    }

    // ── cursor_cube ───────────────────────────────────────────────────────

    #[test]
    fn cursor_cube_room_plop_floors_focus() {
        // RoomPlop mode: floor(focus) gives the containing cube
        let focus = Vec3::new(2.7, 0.0, 3.9);
        let (cx, cz) = cursor_cube(focus, Vec3::new(10.0, 5.0, 10.0), true);
        check!(cx == 2);
        check!(cz == 3);
    }

    #[test]
    fn cursor_cube_wall_mode_biases_toward_camera() {
        // Camera is to +X of focus; cursor should pick the cube on the camera side
        let focus = Vec3::new(2.5, 0.0, 2.5); // exactly on a grid corner
        let camera = Vec3::new(10.0, 5.0, 2.5); // +X of focus (dx > 0)
        let (cx, _cz) = cursor_cube(focus, camera, false);
        // dx = 10 - 2.5 = 7.5 >= 0 → cx = round(2.5) = 2 (or 3), bias cx (no -1)
        check!(cx == 2 || cx == 3); // round(2.5) is 2 or 3 depending on tie-break
    }

    #[test]
    fn cursor_cube_wall_mode_neg_x_biases_away() {
        // Camera is to -X side; should subtract 1 from rounded x
        let focus = Vec3::new(3.0, 0.0, 1.0);
        let camera = Vec3::new(-5.0, 5.0, 1.0); // dx < 0
        let (cx, _cz) = cursor_cube(focus, camera, false);
        // round(3.0) = 3, dx < 0 → cx = 3 - 1 = 2
        check!(cx == 2);
    }

    // ── compute_floor_edge: camera from +Z hides front Z-wall ─────────────

    #[test]
    fn test_visibility_camera_from_z_hides_front_zwall() {
        let (contents, structures) = two_level_room();
        let mut cw = ConstructedCity::new(structures);
        let pe = ProposedCity::new();
        cw.contents = contents;

        // Camera from +Z side, cursor inside the room
        let camera_pos = Vec3::new(1.5, 5.0, 10.0);
        let focus_pos = Vec3::new(1.5, 0.0, 1.5);

        let (hidden_locs, _cut) = compute_floor_edge(&cw, &pe, (focus_pos, false), camera_pos, 0);
        let hidden_set: HashSet<SlotCoord> = hidden_locs.into_iter().collect();

        check!(!hidden_set.is_empty());

        // The far-Z wall (z=3) should be hidden
        let hidden_z_walls: Vec<SlotCoord> = hidden_set
            .iter()
            .filter(|l| l.slot == Slot::ZLoWall && l.cube.z == 3)
            .copied()
            .collect();
        check!(!hidden_z_walls.is_empty());
    }

    /// With an empty world, compute_floor_edge should hide nothing.
    #[test]
    fn test_floor_edge_empty_world_hides_nothing() {
        let structures = load_structure_info();
        let cw = ConstructedCity::new(structures);
        let pe = ProposedCity::new();

        let (hidden, cut) = compute_floor_edge(
            &cw,
            &pe,
            (Vec3::new(1.5, 0.0, 1.5), false),
            Vec3::new(10.0, 5.0, 1.5),
            0,
        );
        check!(hidden.is_empty());
        check!(cut.is_empty());
    }

    #[test]
    fn test_visibility_cursor_inside_hides_right_wall_and_roof() {
        let (contents, structures) = two_level_room();
        let mut cw = ConstructedCity::new(structures);
        let pe = ProposedCity::new();
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

    // ── cutaway + shadows interaction ─────────────────────────────────────────

    /// Returns a Bevy app with a 3×1×3 room loaded into `ConstructedWorld`.
    /// The room is `build_box(0,0,0 → 2,0,2)`.  Camera is at (10, 5, 10) so
    /// both the +X wall column (stored as XLoWall at x=3) and the +Z wall
    /// column (ZLoWall at z=3) fall in the camera-facing hidden zone.
    fn room_shadow_test_app() -> (App, SlotCoord, SlotCoord) {
        let structures = load_structure_info();
        let mut builder = Builder::new(&structures);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 0, 2));
        let contents = builder.get();

        // Representative wall locs that should be hidden.
        let x_wall_loc = SlotCoord {
            cube: IVec3::new(3, 0, 1),
            slot: Slot::XLoWall,
        };
        let z_wall_loc = SlotCoord {
            cube: IVec3::new(1, 0, 3),
            slot: Slot::ZLoWall,
        };

        let mut app = App::new();
        app.add_systems(
            Update,
            (
                update_cutaway_system,
                propagate_render_layers_system.after(update_cutaway_system),
            ),
        );

        let mut cw = ConstructedCity::new(structures);
        cw.contents = contents;

        // Pre-populate autotile_results with empty vecs so that get_cuts never
        // tries to access EorfList mesh handles (which aren't loaded in tests).
        let mut assembled = AssembledCity::new();
        for (loc, _) in cw.contents.iter() {
            assembled.autotile_results.insert(loc, vec![]);
        }

        app.insert_resource(cw);
        app.insert_resource(ProposedCity::new());
        app.insert_resource(assembled);
        app.insert_resource(ViewableWorld::new());
        app.insert_resource(EorfList::default());
        app.insert_resource(AutotileHandles {
            handles: std::collections::HashMap::new(),
        });
        app.insert_resource(BuildState::default());
        app.insert_resource(CutawayMode::FloorEdge);

        // Camera at (10, 5, 10): camera_facing_dirs → (+1, +1), so both the
        // +X and +Z exterior walls are in the hidden zone.
        app.world_mut().spawn((
            Camera::default(),
            GlobalTransform::from(Transform::from_xyz(10.0, 5.0, 10.0)),
            GameCamera,
        ));

        (app, x_wall_loc, z_wall_loc)
    }

    /// Verifies that `compute_floor_edge` still includes the camera-facing walls
    /// in the hidden set after a table is proposed in the centre of the room.
    #[test]
    fn test_compute_floor_edge_walls_hidden_after_table_proposal() {
        let structures = load_structure_info();
        let table_id = crate::eorf::find_structure_by_name(&structures, "table").unwrap();

        let mut builder = Builder::new(&structures);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 0, 2));
        let contents = builder.get();

        let mut cw = ConstructedCity::new(structures);
        cw.contents = contents;

        // Camera at (10,5,10), no PrimaryWindow → focus defaults to (0,0,0).
        let camera_pos = Vec3::new(10.0, 5.0, 10.0);
        let focus_pos = Vec3::new(0.0, 0.0, 0.0);

        let x_wall_loc = SlotCoord {
            cube: IVec3::new(3, 0, 1),
            slot: Slot::XLoWall,
        };

        // Without any proposal.
        let pe_empty = ProposedCity::new();
        let (hidden_before, _) =
            compute_floor_edge(&cw, &pe_empty, (focus_pos, false), camera_pos, 0);
        let set_before: HashSet<SlotCoord> = hidden_before.into_iter().collect();
        check!(
            set_before.contains(&x_wall_loc),
            "wall should be in hidden set before any proposal"
        );

        // With a table proposed at the centre of the room.
        let mut pe_with_table = ProposedCity::new();
        pe_with_table.proposed_changes.set(
            SlotCoord {
                cube: IVec3::new(1, 0, 1),
                slot: Slot::Room,
            },
            crate::city::Proposal::Place(Cell {
                id: table_id,
                facing: Facing::NegX,
                evaluation: None,
                build_material: crate::materials::BuildMaterialId::default(),
            }),
        );

        let (hidden_after, _) =
            compute_floor_edge(&cw, &pe_with_table, (focus_pos, false), camera_pos, 0);
        let set_after: HashSet<SlotCoord> = hidden_after.into_iter().collect();

        // The wall should still be hidden — the bug is that it falls out of the
        // hidden set when a table proposal is present.
        check!(
            set_after.contains(&x_wall_loc),
            "wall should still be in hidden set after table proposal"
        );
    }

    /// Verifies that a hidden wall keeps its `CutawayHidden` marker (and therefore
    /// its shadow-only material) after a table is proposed in the cut zone. The
    /// original bug: `update_cutaway_system` dropped previously-hidden walls out of
    /// the hidden set on the frame after the proposal was added.
    #[test]
    fn test_hidden_wall_keeps_cutaway_marker_after_table_proposal() {
        let (mut app, x_wall_loc, _z_wall_loc) = room_shadow_test_app();

        let table_id = {
            let structures = load_structure_info();
            crate::eorf::find_structure_by_name(&structures, "table").unwrap()
        };

        // Spawn a GridCellMarker entity for the +X wall that should be hidden.
        let wall_entity = app
            .world_mut()
            .spawn((GridCellMarker { loc: x_wall_loc }, Visibility::default()))
            .id();

        // Frame 1: cutaway runs; the wall should be marked CutawayHidden.
        app.update();

        check!(
            app.world().get::<CutawayHidden>(wall_entity).is_some(),
            "wall should be CutawayHidden after first frame"
        );

        // Now propose a table in the middle of the room.
        {
            let mut pe = app.world_mut().resource_mut::<ProposedCity>();
            pe.proposed_changes.set(
                SlotCoord {
                    cube: IVec3::new(1, 0, 1),
                    slot: Slot::Room,
                },
                crate::city::Proposal::Place(Cell {
                    id: table_id,
                    facing: Facing::NegX,
                    evaluation: None,
                    build_material: crate::materials::BuildMaterialId::default(),
                }),
            );
        }

        // Frame 2: cutaway runs again after the proposal.
        app.update();

        // The wall is still in the cut zone, so it must still be CutawayHidden.
        check!(
            app.world().get::<CutawayHidden>(wall_entity).is_some(),
            "wall should still be CutawayHidden after table proposal is added"
        );
    }

    /// Demonstrates that a proposal ghost in the hidden zone loses `SHADOW_ONLY_LAYER`
    /// when `autotile_update_system` respawns it after `update_cutaway_system` has
    /// already set the layer.
    ///
    /// Real-world trigger: `apply_proposal_changes` clears `proposal_autotile_results`
    /// for a location whenever the proposal view changes (cursor move, proposal edit).
    /// On the next frame the execution order is:
    ///
    ///   1. `update_cutaway_system`  – sets `SHADOW_ONLY_LAYER` on the *old* ghost
    ///                                 (command queued, not yet applied)
    ///   2. `autotile_update_system` – sees a cache miss, despawns the old ghost and
    ///                                 spawns a fresh one without any `RenderLayers`
    ///   3. end-of-frame deferred flush – SHADOW_ONLY applied to old ghost; old ghost
    ///                                 despawned; new ghost emerges on layer 0
    ///
    /// Expected: the respawned ghost should have `SHADOW_ONLY_LAYER`.
    /// Actual (bug): the new ghost has no `RenderLayers`, so it is visible to the
    /// camera and does not cast shadows correctly.
    ///
    /// This test is expected to FAIL until the bug is fixed.
    #[test]
    fn test_ghost_loses_shadow_layer_after_proposal_cache_clear() {
        use crate::autotile::{autotile_update_system, compile, parse, AutotileRules};
        use crate::eorf::Eorf;

        let structures = load_structure_info();
        let table_id = crate::eorf::find_structure_by_name(&structures, "table").unwrap();

        let mut builder = Builder::new(&structures);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 0, 2));
        let contents = builder.get();

        let autotile_src = include_str!("../buildables/structures.autotile");
        let autotile_file = parse(autotile_src).expect("structures.autotile parse error");
        let autotile_rules = AutotileRules(compile(&autotile_file));

        let structure_list = EorfList {
            structures: structures
                .iter()
                .map(|info| Eorf {
                    info: info.clone(),
                    mesh_handle: Handle::default(),
                    cut_handle: None,
                    is_wings: false,
                })
                .collect(),
        };

        let mut app = App::new();
        // Fixed ordering: autotile runs first (spawning/respawning entities), then
        // Bevy's auto-apply_deferred flushes those commands so the new entities exist
        // in the ECS, then cutaway assigns SHADOW_ONLY_LAYER to hidden entities.
        app.add_systems(
            Update,
            (
                autotile_update_system,
                update_cutaway_system.after(autotile_update_system),
                propagate_render_layers_system.after(update_cutaway_system),
            ),
        );

        let mut cw = ConstructedCity::new(structures);
        cw.contents = contents;
        app.insert_resource(cw);

        // Propose a table at Room(1,0,1). With SimpleOctant (focus=(0,0,0),
        // camera=(10,5,10)), octant_hidden returns true for x≥0 and z≥0,
        // so the ghost lands in the hidden zone.
        let table_loc = SlotCoord {
            cube: IVec3::new(1, 0, 1),
            slot: Slot::Room,
        };
        let mut pe = ProposedCity::new();
        pe.proposed_changes.set(
            table_loc,
            crate::city::Proposal::Place(crate::city::Cell {
                id: table_id,
                facing: Facing::NegX,
                evaluation: None,
                build_material: crate::materials::BuildMaterialId::default(),
            }),
        );
        app.insert_resource(pe);
        app.insert_resource(AssembledCity::new());
        app.insert_resource(ViewableWorld::new());
        app.insert_resource(structure_list);
        app.insert_resource(AutotileHandles {
            handles: std::collections::HashMap::new(),
        });
        app.insert_resource(autotile_rules);
        app.insert_resource(BuildState::default());
        app.insert_resource(CutawayMode::SimpleOctant);

        app.world_mut().spawn((
            Camera::default(),
            GlobalTransform::from(Transform::from_xyz(10.0, 5.0, 10.0)),
            GameCamera,
        ));

        // Frame 1: autotile spawns the ghost; cutaway has no ghost to act on yet.
        app.update();

        // Frame 2: cutaway finds the ghost and sets SHADOW_ONLY_LAYER; autotile
        // sees a cache hit and leaves the ghost alone.
        app.update();

        // Verify the ghost correctly has SHADOW_ONLY_LAYER after frame 2.
        let shadow_layer = RenderLayers::layer(SHADOW_ONLY_LAYER);
        let ghost_after_frame2 = app
            .world()
            .resource::<AssembledCity>()
            .proposal_entities
            .get(&table_loc)
            .and_then(|v| v.first().copied());
        check!(
            ghost_after_frame2.is_some(),
            "ghost entity for table should exist after frame 2"
        );
        check!(
            app.world()
                .get::<RenderLayers>(ghost_after_frame2.unwrap())
                .cloned()
                == Some(shadow_layer.clone()),
            "ghost should have SHADOW_ONLY_LAYER after frame 2 cutaway"
        );

        // Simulate what `apply_proposal_changes` does when the proposal view changes
        // (e.g. user moves cursor): clear the cached autotile result for this location.
        // On the next frame, autotile will despawn the old ghost and spawn a new one.
        #[cfg(autotile_matching)]
        {
            app.world_mut()
                .resource_mut::<AssembledCity>()
                .proposal_autotile_results
                .remove(&table_loc);
        }

        // Frame 3: execution order:
        //   cutaway  → queues SHADOW_ONLY on *old* ghost
        //   autotile → cache miss → despawns old ghost, spawns new ghost (no RenderLayers)
        //   flush    → old ghost gets SHADOW_ONLY then is despawned;
        //              new ghost exists on layer 0
        app.update();

        let ghost_entities_after = app
            .world()
            .resource::<AssembledCity>()
            .proposal_entities
            .get(&table_loc)
            .cloned()
            .unwrap_or_default();
        check!(
            !ghost_entities_after.is_empty(),
            "a new ghost entity should have been spawned after the cache clear"
        );
        for entity in ghost_entities_after {
            check!(
                app.world().get::<RenderLayers>(entity).cloned() == Some(shadow_layer.clone()),
                "the respawned ghost in the hidden zone should have SHADOW_ONLY_LAYER, \
                 but autotile_update_system respawns it without RenderLayers after \
                 update_cutaway_system already set the layer (bug)"
            );
        }
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
        let mut cw = ConstructedCity::new(structures);
        cw.contents.set(
            loc,
            Cell {
                id: EorfId(0),
                facing: Facing::NegX,
                evaluation: None,
                build_material: crate::materials::BuildMaterialId::default(),
            },
        );
        app.insert_resource(cw);
        app.insert_resource(ProposedCity::new());
        app.insert_resource(AssembledCity::new());
        app.insert_resource(ViewableWorld::new());
        app.insert_resource(EorfList::default());
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

    /// Like `shadow_layer_test_app`, but wired for the material-swap mechanism:
    /// includes `sync_cutaway_shadow_material` and the `MaterialAssets`/`MaterialList`
    /// resources it needs, using bare (assetless) handles.
    fn shadow_material_test_app() -> (App, SlotCoord) {
        let (mut app, loc) = shadow_layer_test_app();
        app.add_systems(
            Update,
            sync_cutaway_shadow_material.after(update_cutaway_system),
        );
        app.insert_resource(MaterialAssets::for_test(
            Handle::default(),
            Handle::default(),
        ));
        app.insert_resource(MaterialList::default());
        (app, loc)
    }

    /// Spawns a `GridCellMarker` root with one `GiMaterial` leaf; the cell is in the
    /// hidden octant, so the leaf's material should be swapped to `ShadowOnlyMaterial`
    /// (invisible to the camera, still casting) and the root marked `CutawayHidden`.
    #[test]
    fn test_hidden_cell_swaps_leaf_to_shadow_only() {
        let (mut app, loc) = shadow_material_test_app();

        let leaf = app
            .world_mut()
            .spawn((MeshMaterial3d::<GiMaterial>(Handle::default()),))
            .id();
        let root = app
            .world_mut()
            .spawn((GridCellMarker { loc }, Visibility::default()))
            .id();
        app.world_mut().entity_mut(root).add_child(leaf);

        app.update();

        check!(app.world().get::<CutawayHidden>(root).is_some());
        check!(app
            .world()
            .get::<MeshMaterial3d<ShadowOnlyMaterial>>(leaf)
            .is_some());
        check!(app
            .world()
            .get::<MeshMaterial3d<GiMaterial>>(leaf)
            .is_none());
    }

    /// After a cell is hidden and its leaf swapped to shadow-only, moving the camera
    /// so the cell leaves the hidden octant should restore the real `GiMaterial`.
    #[test]
    fn test_unhidden_cell_restores_leaf_material() {
        let (mut app, loc) = shadow_material_test_app();

        let leaf = app
            .world_mut()
            .spawn((MeshMaterial3d::<GiMaterial>(Handle::default()),))
            .id();
        let root = app
            .world_mut()
            .spawn((GridCellMarker { loc }, Visibility::default()))
            .id();
        app.world_mut().entity_mut(root).add_child(leaf);

        // Frame 1: hidden — leaf becomes shadow-only.
        app.update();
        check!(app
            .world()
            .get::<MeshMaterial3d<ShadowOnlyMaterial>>(leaf)
            .is_some());

        // Move the camera to the opposite octant so `loc` is no longer hidden.
        let cam_entity = {
            let mut q = app.world_mut().query_filtered::<Entity, With<GameCamera>>();
            q.single(app.world()).unwrap()
        };
        app.world_mut()
            .entity_mut(cam_entity)
            .insert(GlobalTransform::from(Transform::from_xyz(
                -10.0, 5.0, -10.0,
            )));

        // Frame 2: no longer hidden — the real material is restored.
        app.update();
        check!(app.world().get::<CutawayHidden>(root).is_none());
        check!(app
            .world()
            .get::<MeshMaterial3d<GiMaterial>>(leaf)
            .is_some());
        check!(app
            .world()
            .get::<MeshMaterial3d<ShadowOnlyMaterial>>(leaf)
            .is_none());
    }
}
