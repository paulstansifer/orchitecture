use std::collections::HashMap;

use bevy::math::{IVec3, Vec3};
use bevy::prelude::Commands;

use crate::city::{
    apply_changes, clear_proposal_entities, clear_proposed_cut_entities, desired, AssembledCity,
    Cell, ConstructedCity, Proposal, ProposalView, ProposedCity, UndoRecord, VantageEvaluation,
    ViewableWorld,
};
use crate::eorf::{EorfId, EorfList, PlacementStyle};
use crate::materials::{BuildMaterialId, Cost};
use crate::resource::UniformResource;
use crate::sparse3d::{Facing, Slot, SlotCoord, Sparse3D};

fn proposal_view(proposal: &Option<Proposal>, has_real_cell: bool) -> ProposalView {
    match proposal {
        None => ProposalView::None,
        Some(Proposal::Place(cell)) => {
            if has_real_cell {
                ProposalView::Replace
            } else {
                ProposalView::Add(cell.clone())
            }
        }
        Some(Proposal::Remove) => ProposalView::Remove,
    }
}

impl ProposedCity {
    /// Propose placing or clearing cells in a rectangular range.
    ///
    /// Writes to `proposed_changes` (not `contents`). Returns `(loc, view)` deltas
    /// describing the visual treatment each location now needs.
    fn propose(
        &mut self,
        cw: &ConstructedCity,
        dir: i32,
        (position1, position2): (IVec3, IVec3),
        slot: Slot,
        item: Option<EorfId>,
        build_material: BuildMaterialId,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let start = position1.min(position2);
        let end = position1.max(position2);
        let mut changes: Vec<(SlotCoord, ProposalView)> = Vec::new();
        let mut undo_changed: Vec<(SlotCoord, Option<Cell>)> = Vec::new();

        for x in start.x..=end.x {
            for y in start.y..=end.y {
                for z in start.z..=end.z {
                    let loc = SlotCoord {
                        cube: IVec3::new(x, y, z),
                        slot,
                    };
                    if item.is_some()
                        && cw.road_forbidden_zone
                        && crate::road::is_in_road_forbidden_zone(loc)
                    {
                        continue;
                    }
                    let real_cell = cw.contents.get(loc).cloned();
                    let prior_proposal = self.proposed_changes.get(loc).cloned();
                    // The desired state before this edit, for undo (absolute, so it
                    // survives construct()).
                    let prior_desired = desired(cw, self, loc);

                    let new_proposal: Option<Proposal> = if let Some(id) = item {
                        let facing = Facing::from_number(dir as u8);
                        let new_cell = Cell {
                            id,
                            facing,
                            evaluation: None,
                            build_material,
                        };
                        if real_cell.as_ref() == Some(&new_cell) {
                            None // Desired state already matches real — cancel any proposal
                        } else {
                            Some(Proposal::Place(new_cell))
                        }
                    } else {
                        // Erasure: only meaningful if a real cell exists
                        if real_cell.is_some() {
                            Some(Proposal::Remove)
                        } else {
                            None
                        }
                    };

                    if new_proposal == prior_proposal {
                        continue; // Nothing changed
                    }

                    // Apply the new proposal state
                    if let Some(ref proposal) = new_proposal {
                        self.proposed_changes.set(loc, proposal.clone());
                    } else {
                        self.proposed_changes.take(loc);
                    }

                    undo_changed.push((loc, prior_desired));

                    changes.push((loc, proposal_view(&new_proposal, real_cell.is_some())));
                }
            }
        }

        if !undo_changed.is_empty() {
            self.undo_record.push(UndoRecord {
                changed: undo_changed,
            });
            // A fresh edit invalidates the redo stack.
            self.redo_record.clear();
        }
        changes
    }

    pub fn wall_drag(
        &mut self,
        cw: &ConstructedCity,
        from: Vec3,
        to: Vec3,
        selected_mesh_id: Option<EorfId>,
        build_material: BuildMaterialId,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let along_x = (to.x - from.x).abs() > (to.z - from.z).abs();

        let from_i = from.round().as_ivec3();
        let mut to_i = from_i;
        if along_x {
            to_i.x = to.x.round() as i32;
        } else {
            to_i.z = to.z.round() as i32;
        }

        let start = from_i.min(to_i);
        let end = from_i.max(to_i)
            - if along_x {
                IVec3::new(1, 0, 0)
            } else {
                IVec3::new(0, 0, 1)
            };
        let slot = if along_x {
            Slot::ZLoWall
        } else {
            Slot::XLoWall
        };

        self.propose(cw, 0, (start, end), slot, selected_mesh_id, build_material)
    }

    pub fn floor_drag(
        &mut self,
        cw: &ConstructedCity,
        from: Vec3,
        to: Vec3,
        selected_mesh_id: Option<EorfId>,
        build_material: BuildMaterialId,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let from_i = from.round().as_ivec3();
        let to_i = to.round().as_ivec3();
        let start = from_i.min(to_i);
        let end = from_i.max(to_i) - IVec3::new(1, 0, 1);
        self.propose(
            cw,
            0,
            (start, end),
            Slot::Floor,
            selected_mesh_id,
            build_material,
        )
    }

    pub fn room_drag(
        &mut self,
        cw: &ConstructedCity,
        from: Vec3,
        to: Vec3,
        dir: i32,
        selected_mesh_id: Option<EorfId>,
        build_material: BuildMaterialId,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let from_i = from.round().as_ivec3();
        let to_i = to.round().as_ivec3();
        let start = from_i.min(to_i);
        let end = from_i.max(to_i) - IVec3::new(1, 0, 1);
        self.propose(
            cw,
            dir,
            (start, end),
            Slot::Room,
            selected_mesh_id,
            build_material,
        )
    }

    pub fn room_plop(
        &mut self,
        cw: &ConstructedCity,
        location: Vec3,
        dir: i32,
        selected_mesh_id: Option<EorfId>,
        build_material: BuildMaterialId,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let pos = location.round().as_ivec3();
        let changes = self.propose(
            cw,
            dir,
            (pos, pos),
            Slot::Room,
            selected_mesh_id,
            build_material,
        );
        let vantage_evaluated =
            selected_mesh_id.is_some_and(|id| cw.eorfs[id.as_usize()].vantage_evaluated);
        if vantage_evaluated {
            let loc = SlotCoord {
                cube: pos,
                slot: Slot::Room,
            };
            if let Some(Proposal::Place(cell)) = self.proposed_changes.get_mut(loc) {
                cell.evaluation = Some(VantageEvaluation {
                    order: None,
                    interest: None,
                });
            }
        }
        changes
    }

    /// Plops a `WallPlop` structure (window, doorway, column, ...) onto
    /// whichever wall is nearest `location` -- see [`crate::city::nearest_wall_slot`].
    /// Unlike `WallDrag`, this is a single-click placement onto one boundary,
    /// with no drag-to-extend behavior.
    pub fn wall_plop(
        &mut self,
        cw: &ConstructedCity,
        location: Vec3,
        dir: i32,
        selected_mesh_id: Option<EorfId>,
        build_material: BuildMaterialId,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let loc = crate::city::nearest_wall_slot(location);
        self.propose(
            cw,
            dir,
            (loc.cube, loc.cube),
            loc.slot,
            selected_mesh_id,
            build_material,
        )
    }

    pub fn drag(
        &mut self,
        cw: &ConstructedCity,
        (from, to): (Vec3, Vec3),
        dir: i32,
        selected_mesh_id: EorfId,
        remove: bool,
        build_material: BuildMaterialId,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let id = (!remove).then_some(selected_mesh_id);
        match cw.eorfs[selected_mesh_id.as_usize()].placement_style {
            PlacementStyle::WallDrag => self.wall_drag(cw, from, to, id, build_material),
            PlacementStyle::FloorDrag => self.floor_drag(cw, from, to, id, build_material),
            PlacementStyle::RoomDrag => self.room_drag(cw, from, to, dir, id, build_material),
            _ => vec![],
        }
    }

    /// Propose placing (or, with `item: None`, removing) a single structure at an
    /// absolute grid location. Unlike `click`/`drag`, which infer the slot from a
    /// mouse position in world space, this takes the slot directly -- meant for
    /// callers (e.g. the headless testing harness) that already know exactly
    /// where they want to build.
    pub fn place_at(
        &mut self,
        cw: &ConstructedCity,
        loc: SlotCoord,
        item: Option<EorfId>,
        dir: i32,
        build_material: BuildMaterialId,
    ) -> Vec<(SlotCoord, ProposalView)> {
        self.propose(
            cw,
            dir,
            (loc.cube, loc.cube),
            loc.slot,
            item,
            build_material,
        )
    }

    pub fn click(
        &mut self,
        cw: &ConstructedCity,
        position: Vec3,
        selected_mesh_id: EorfId,
        dir: i32,
        remove: bool,
        build_material: BuildMaterialId,
    ) -> Vec<(SlotCoord, ProposalView)> {
        match cw.eorfs[selected_mesh_id.as_usize()].placement_style {
            PlacementStyle::RoomPlop => self.room_plop(
                cw,
                position,
                dir,
                (!remove).then_some(selected_mesh_id),
                build_material,
            ),
            PlacementStyle::WallPlop => self.wall_plop(
                cw,
                position,
                dir,
                (!remove).then_some(selected_mesh_id),
                build_material,
            ),
            _ => vec![],
        }
    }

    /// Drive each location's desired state to `targets`, creating whatever
    /// proposal is needed given the current real cell. Returns the view deltas
    /// and the *inverse* targets (each location's desired state before this call)
    /// so undo/redo can be reversed.
    fn restore_desired(
        &mut self,
        cw: &ConstructedCity,
        targets: Vec<(SlotCoord, Option<Cell>)>,
    ) -> (
        Vec<(SlotCoord, ProposalView)>,
        Vec<(SlotCoord, Option<Cell>)>,
    ) {
        let mut changes = vec![];
        let mut inverse = vec![];
        for (loc, target) in targets {
            let prev_desired = desired(cw, self, loc);
            let real_cell = cw.contents.get(loc).cloned();
            let new_proposal: Option<Proposal> = if target == real_cell {
                None // Already matches reality; no proposal needed.
            } else {
                match &target {
                    Some(cell) => Some(Proposal::Place(cell.clone())),
                    None => Some(Proposal::Remove),
                }
            };

            if let Some(ref proposal) = new_proposal {
                self.proposed_changes.set(loc, proposal.clone());
            } else {
                self.proposed_changes.take(loc);
            }
            changes.push((loc, proposal_view(&new_proposal, real_cell.is_some())));
            inverse.push((loc, prev_desired));
        }
        (changes, inverse)
    }

    /// Undo the last action by restoring the desired state each location had
    /// before it. Pushes the inverse onto the redo stack. Returns view deltas
    /// so proposal rendering can be updated.
    pub fn undo(&mut self, cw: &ConstructedCity) -> Vec<(SlotCoord, ProposalView)> {
        let Some(record) = self.undo_record.pop() else {
            return vec![];
        };
        let (changes, inverse) = self.restore_desired(cw, record.changed);
        self.redo_record.push(UndoRecord { changed: inverse });
        changes
    }

    /// Redo the last undone action. Pushes the inverse back onto the undo stack.
    pub fn redo(&mut self, cw: &ConstructedCity) -> Vec<(SlotCoord, ProposalView)> {
        let Some(record) = self.redo_record.pop() else {
            return vec![];
        };
        let (changes, inverse) = self.restore_desired(cw, record.changed);
        self.undo_record.push(UndoRecord { changed: inverse });
        changes
    }

    /// Clears all proposals without committing anything. Undo/redo history is
    /// left intact, so a reset is itself reversible (the stale records degrade to
    /// no-ops when a location already matches reality).
    /// Entity cleanup is the caller's responsibility.
    pub fn reset(&mut self) {
        self.proposed_changes = Sparse3D::new();
        self.resource_progress.clear();
    }
}

/// Commits all proposed changes into real contents. Returns real-cell deltas for `apply_changes`.
/// Clears `proposed_changes`; entity cleanup is the caller's responsibility.
///
/// The undo record is *preserved*: because it stores absolute prior cell states,
/// undo can still revert committed changes by proposing their inverse.
pub fn construct(
    cw: &mut ConstructedCity,
    pe: &mut ProposedCity,
    material_list: &crate::materials::MaterialList,
) -> Vec<(SlotCoord, Option<Cell>)> {
    let proposals: Vec<(SlotCoord, Proposal)> = pe
        .proposed_changes
        .iter()
        .map(|(loc, p)| (loc, p.clone()))
        .collect();

    pe.proposed_changes = Sparse3D::new();
    pe.resource_progress.clear();

    let mut real_changes = vec![];
    for (loc, proposal) in proposals {
        match proposal {
            Proposal::Place(cell) => {
                cw.set_cell(loc, cell.clone());
                real_changes.push((loc, Some(cell)));
            }
            Proposal::Remove => {
                if let Some(removed) = cw.take_cell(loc) {
                    if let Some(cost) = cell_cost(&removed, &cw.eorfs, material_list) {
                        for (res, qty) in cost {
                            if res.refundable() && qty > 0 {
                                crate::place::deposit_uniform_with_capacity(cw, res, qty as u32);
                            }
                        }
                    }
                }
                real_changes.push((loc, None));
            }
        }
    }
    real_changes
}

/// Advances one month of construction progress (grid/resource state only, no
/// ECS). Completes (commits all proposals) once every material's cost has
/// been fully paid off. Should be called once per "Advance Month" action,
/// after that month's resources have already been applied toward payment.
///
/// `fully_paid` should reflect whether
/// [`remaining_construction_need`] is empty (or `true`
/// unconditionally in sandbox mode).
///
/// Returns `Some(real_changes)` if construction completed this month (proposals
/// were committed); the caller is responsible for reflecting those into the ECS
/// via [`apply_construction_completion`]. Returns `None` otherwise.
pub fn tick_construction(
    pending: &mut ProposedCity,
    constructed: &mut ConstructedCity,
    fully_paid: bool,
    material_list: &crate::materials::MaterialList,
) -> Option<Vec<(SlotCoord, Option<Cell>)>> {
    if pending.num_changes() > 0 && fully_paid {
        return Some(construct(constructed, pending, material_list));
    }
    None
}

/// Reflects a completed construction (the `real_changes` from [`tick_construction`])
/// into the ECS: clears proposal ghosts and proposed-cut entities, then syncs the
/// newly-committed cells to real geometry.
pub fn apply_construction_completion(
    commands: &mut Commands,
    assembled: &mut AssembledCity,
    viewable: &mut ViewableWorld,
    structure_list: &EorfList,
    real_changes: Vec<(SlotCoord, Option<Cell>)>,
) {
    clear_proposal_entities(commands, assembled);
    clear_proposed_cut_entities(commands, viewable);
    apply_changes(commands, assembled, structure_list, real_changes);
}

/// Commits all pending proposals immediately (bypassing the monthly payment
/// schedule) and reflects them into the ECS — e.g. when switching into
/// sandbox mode, whose edits are always committed.
pub fn commit_pending_construction(
    commands: &mut Commands,
    constructed: &mut ConstructedCity,
    pending: &mut ProposedCity,
    assembled: &mut AssembledCity,
    viewable: &mut ViewableWorld,
    structure_list: &EorfList,
    material_list: &crate::materials::MaterialList,
) {
    let real_changes = construct(constructed, pending, material_list);
    apply_construction_completion(commands, assembled, viewable, structure_list, real_changes);
}

/// Loads a new building, replacing contents and clearing all history.
/// Entity cleanup is the caller's responsibility.
pub fn load_from_offline(
    cw: &mut ConstructedCity,
    pw: &mut ProposedCity,
    new_contents: Sparse3D<Cell>,
) -> Vec<(SlotCoord, Option<Cell>)> {
    pw.proposed_changes = Sparse3D::new();
    pw.undo_record.clear();
    pw.redo_record.clear();
    pw.resource_progress.clear();
    // Shift the building into the no-road semiplane (south of the E-W road).
    let z_shift = -new_contents.bounding_box().1.z;
    let shifted = new_contents.translate(IVec3::new(0, 0, z_shift));
    cw.replace_contents(shifted)
}

/// The pending construction project's absorption of resources this month:
/// how much of each material's cost got paid off, and how much of that came
/// from pre-existing storage (as opposed to this month's fresh market
/// inflow, which needs no physical deposit/withdrawal to claim).
#[derive(Clone, Debug, Default)]
pub struct Construction {
    pub applied: HashMap<UniformResource, u32>,
    pub from_storage: HashMap<UniformResource, u32>,
}

impl Construction {
    pub fn apply(&self, pending: &mut ProposedCity, constructed: &mut ConstructedCity) {
        for (&res, &qty) in &self.applied {
            if qty > 0 {
                *pending.resource_progress.entry(res).or_insert(0) += qty;
            }
        }
        for (&res, &qty) in &self.from_storage {
            if qty > 0 {
                crate::place::consume_uniform(constructed, res, qty);
            }
        }
    }

    pub fn describe(&self) -> String {
        let total: u32 = self.applied.values().sum();
        if total == 0 {
            "No resources were available for construction this month.".to_string()
        } else {
            format!("Construction absorbs {total} resource unit(s) this month.")
        }
    }
}

/// Claims resources toward `remaining_need`, up to each resource's
/// `construct_per_month()` rate, drawing from `inflow_available` first (so
/// construction can "rescue" resources that would otherwise be lost to
/// storage-fill/loss) and topping up from `storage_available` second.
/// Mutates both maps to subtract what's claimed.
pub fn compute_construction_absorption(
    remaining_need: &HashMap<UniformResource, u32>,
    inflow_available: &mut HashMap<UniformResource, u32>,
    storage_available: &mut HashMap<UniformResource, u32>,
) -> Construction {
    let mut applied = HashMap::new();
    let mut from_storage = HashMap::new();
    for (&res, &need) in remaining_need {
        let rate = res.construct_per_month();
        let want = need.min(rate);

        let have_inflow = inflow_available.get(&res).copied().unwrap_or(0);
        let from_inflow = want.min(have_inflow);
        if from_inflow > 0 {
            *inflow_available.get_mut(&res).unwrap() -= from_inflow;
        }

        let have_storage = storage_available.get(&res).copied().unwrap_or(0);
        let from_store = (want - from_inflow).min(have_storage);
        if from_store > 0 {
            *storage_available.get_mut(&res).unwrap() -= from_store;
            from_storage.insert(res, from_store);
        }

        let total = from_inflow + from_store;
        if total > 0 {
            applied.insert(res, total);
        }
    }
    Construction {
        applied,
        from_storage,
    }
}

/// The material cost of a single placed cell: its structure's fixed
/// furniture cost, or its element's cost for the cell's chosen build
/// material. `None` if the cost can't be determined (unknown material or
/// element type).
fn cell_cost(
    cell: &Cell,
    structure_infos: &[crate::eorf::EorfInfo],
    material_list: &crate::materials::MaterialList,
) -> Option<Cost> {
    let info = &structure_infos[cell.id.as_usize()];
    if let Some(furniture_cost) = info.furniture_cost() {
        return Some(furniture_cost.clone());
    }
    let build_mat = material_list
        .materials
        .get(cell.build_material.0 as usize)?;
    let element_type = info.element_type()?;
    build_mat.costs.get(&element_type).cloned()
}

/// Total resource cost to complete the current proposed construction.
/// Only counts `Proposal::Place` entries; removals are free. Furniture has a
/// fixed cost independent of the selected build material.
/// Returns sorted `(resource, quantity)` pairs; empty when cost is zero.
pub fn construction_cost(
    proposed: &Sparse3D<Proposal>,
    structure_infos: &[crate::eorf::EorfInfo],
    material_list: &crate::materials::MaterialList,
) -> Vec<(UniformResource, u32)> {
    let mut totals: HashMap<UniformResource, u32> = HashMap::new();

    for (_, proposal) in proposed.iter() {
        let Proposal::Place(cell) = proposal else {
            continue;
        };
        let Some(cost) = cell_cost(cell, structure_infos, material_list) else {
            continue;
        };
        for (res, qty) in cost {
            *totals.entry(res).or_insert(0) += qty as u32;
        }
    }

    let mut result: Vec<_> = totals.into_iter().collect();
    result.sort_by_key(|(r, _)| *r);
    result
}

/// Resource cost still owed to complete the current proposed construction:
/// `construction_cost(...)` (unchanged, total cost) minus
/// `pending.resource_progress` already applied, floored at zero. Drops zero
/// entries. Empty when there's nothing pending.
pub fn remaining_construction_need(
    pending: &ProposedCity,
    structure_infos: &[crate::eorf::EorfInfo],
    material_list: &crate::materials::MaterialList,
) -> Vec<(UniformResource, u32)> {
    construction_cost(&pending.proposed_changes, structure_infos, material_list)
        .into_iter()
        .filter_map(|(res, total)| {
            let progress = pending.resource_progress.get(&res).copied().unwrap_or(0);
            let remaining = total.saturating_sub(progress.min(total));
            (remaining > 0).then_some((res, remaining))
        })
        .collect()
}

/// The most a single resource's unpaid cost can reach before proposing more
/// construction is blocked (the player must cancel some proposals first).
pub const MAX_UNPAID_CONSTRUCTION: u32 = 100;

/// Whether the current proposal's unpaid cost exceeds
/// [`MAX_UNPAID_CONSTRUCTION`] for any resource, hard-blocking the month
/// advance until some proposed construction is cancelled.
pub fn construction_blocked(remaining_need: &[(UniformResource, u32)]) -> bool {
    remaining_need
        .iter()
        .any(|(_, qty)| *qty > MAX_UNPAID_CONSTRUCTION)
}

/// Fraction (0.0–1.0) of the current pending construction's total material
/// cost that's already been paid off, weighted by each material's time cost
/// (`1 / UniformResource::construct_per_month()`) so materials that are
/// slower to deliver count for more of the bar. `None` when there's no
/// pending construction (or its cost is zero).
pub fn construction_progress_fraction(
    pending: &ProposedCity,
    structure_infos: &[crate::eorf::EorfInfo],
    material_list: &crate::materials::MaterialList,
) -> Option<f32> {
    let total_cost = construction_cost(&pending.proposed_changes, structure_infos, material_list);
    if total_cost.is_empty() {
        return None;
    }

    let time_cost =
        |res: UniformResource, qty: u32| -> f32 { qty as f32 / res.construct_per_month() as f32 };

    let total_time: f32 = total_cost
        .iter()
        .map(|(res, qty)| time_cost(*res, *qty))
        .sum();
    if total_time <= 0.0 {
        return Some(1.0);
    }
    let paid_time: f32 = total_cost
        .iter()
        .map(|(res, qty)| {
            let progress = pending
                .resource_progress
                .get(res)
                .copied()
                .unwrap_or(0)
                .min(*qty);
            time_cost(*res, progress)
        })
        .sum();
    Some((paid_time / total_time).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use assert2::{assert, check};

    use bevy::math::IVec3;

    use crate::city::{Cell, ConstructedCity, Proposal, ProposalView, ProposedCity};
    use crate::eorf::{EorfId, EorfInfo, PlacementStyle};
    use crate::materials::BuildMaterialId;
    use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, Slot, SlotCoord};

    use super::{compute_construction_absorption, construct, load_from_offline, tick_construction};

    fn make_world() -> (ConstructedCity, ProposedCity) {
        use crate::eorf::StructureEmbedding;
        let structs = vec![EorfInfo {
            name: "test_wall".to_string(),
            placement_style: PlacementStyle::WallDrag,
            x_char: None,
            z_char: None,
            embedding: StructureEmbedding {
                tall: 0.0,
                passable: 0.0,
                decorative: 0.0,
                striated: 0.0,
                temporary: 0.0,
            },
            kind: crate::eorf::FurnitureOrElement::Element(crate::materials::ElementType::WallLike),
            vantage_evaluated: false,
            storage_capacity: Vec::new(),
            placeable: true,
            slots: Vec::new(),
        }];
        let mut cw = ConstructedCity::new(structs);
        cw.road_forbidden_zone = false;
        (cw, ProposedCity::new())
    }

    fn thing(cw: &ConstructedCity) -> Option<EorfId> {
        cw.find_structure_by_name("test_wall")
    }

    fn wall_cell(id: EorfId) -> Cell {
        Cell {
            id,
            facing: Facing::NegX,
            evaluation: None,
            build_material: BuildMaterialId::default(),
        }
    }

    fn xlowall(x: i32, y: i32, z: i32) -> RelSlotCoord {
        RelSlotCoord::new(x, y, z, RelSlot::XLoWall)
    }

    // ── propose ──────────────────────────────────────────────────────────────

    #[test]
    fn propose_add_goes_to_proposed_changes_not_contents() {
        let (cw, mut pw) = make_world();
        let loc = xlowall(0, 0, 0);

        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );

        check!(cw.contents.get(loc).is_none());
        check!(matches!(
            pw.proposed_changes.get(loc),
            Some(Proposal::Place(_))
        ));
    }

    #[test]
    fn propose_returns_add_view_for_empty_slot() {
        let (cw, mut pw) = make_world();
        let deltas = pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::Add(_)));
    }

    #[test]
    fn propose_returns_replace_view_for_occupied_slot() {
        let (mut cw, mut pw) = make_world();
        let loc = xlowall(0, 0, 0);
        // Place a real cell first
        cw.contents.set(loc, wall_cell(thing(&cw).unwrap()));

        // propose removal to get a Remove view
        let deltas = pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            None,
            BuildMaterialId::default(),
        );
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::Remove));
    }

    #[test]
    fn propose_same_as_real_produces_no_proposal() {
        let (mut cw, mut pw) = make_world();
        let loc = xlowall(0, 0, 0);
        // Put the same cell in real contents
        cw.contents.set(loc, wall_cell(thing(&cw).unwrap()));

        // Propose placing the exact same cell
        let deltas = pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );

        check!(deltas.is_empty(), "identical proposal should be a no-op");
        check!(pw.proposed_changes.get(loc).is_none());
    }

    #[test]
    fn propose_remove_on_empty_slot_is_no_op() {
        let (cw, mut pw) = make_world();
        let deltas = pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            None,
            BuildMaterialId::default(),
        );
        check!(deltas.is_empty());
        check!(pw.proposed_changes.iter().count() == 0);
    }

    // ── undo ─────────────────────────────────────────────────────────────────

    #[test]
    fn undo_restores_prior_proposal_state() {
        let (cw, mut pw) = make_world();
        let loc = xlowall(0, 0, 0);

        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        check!(pw.proposed_changes.get(loc).is_some());

        let deltas = pw.undo(&cw);
        check!(pw.proposed_changes.get(loc).is_none());
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::None));
    }

    #[test]
    fn undo_on_empty_stack_returns_empty() {
        let (cw, mut pe) = make_world();
        let deltas = pe.undo(&cw);
        check!(deltas.is_empty());
    }

    #[test]
    fn undo_clears_undo_record_entry() {
        let (cw, mut pw) = make_world();
        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        check!(pw.undo_record.len() == 1);
        pw.undo(&cw);
        check!(pw.undo_record.len() == 0);
    }

    // ── construct ─────────────────────────────────────────────────────────────

    #[test]
    fn construct_moves_proposals_to_contents() {
        let (mut cw, mut pw) = make_world();
        let loc = xlowall(1, 0, 0);

        pw.propose(
            &cw,
            0,
            (IVec3::new(1, 0, 0), IVec3::new(1, 0, 0)),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        check!(cw.contents.get(loc).is_none());

        let real_changes = construct(&mut cw, &mut pw, &crate::materials::MaterialList::default());

        check!(pw.proposed_changes.get(loc).is_none());
        check!(cw.contents.get(loc).is_some());
        check!(real_changes.len() == 1);
        check!(real_changes[0].1.is_some());
    }

    #[test]
    fn construct_remove_proposal_deletes_from_contents() {
        let (mut cw, mut pw) = make_world();
        let loc = xlowall(0, 0, 0);
        cw.contents.set(loc, wall_cell(thing(&cw).unwrap()));

        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            None,
            BuildMaterialId::default(),
        );
        let real_changes = construct(&mut cw, &mut pw, &crate::materials::MaterialList::default());

        check!(cw.contents.get(loc).is_none());
        check!(real_changes.len() == 1);
        check!(real_changes[0].1.is_none());
    }

    #[test]
    fn construct_remove_refunds_refundable_resources_but_not_lime() {
        use crate::eorf::StructureEmbedding;
        use crate::materials::{BuildMaterial, ElementType, MaterialList};
        use crate::place::{ParentRestriction, ParticularPlace, Place};
        use crate::resource::{Approximation, Inventory, StorageKind, UniformResource};
        use std::collections::BTreeMap;

        let (mut cw, mut pw) = make_world();

        // A storage bin (with ample capacity) so the refund has somewhere to
        // land.
        let bin_id = cw.eorfs.len() as u32;
        cw.eorfs.push(EorfInfo {
            name: "test_bin".to_string(),
            placement_style: PlacementStyle::RoomPlop,
            x_char: None,
            z_char: None,
            embedding: StructureEmbedding {
                tall: 0.0,
                passable: 0.0,
                decorative: 0.0,
                striated: 0.0,
                temporary: 1.0,
            },
            kind: crate::eorf::FurnitureOrElement::Furniture(vec![]),
            vantage_evaluated: false,
            storage_capacity: vec![(StorageKind::Bulk, 999.0)],
            placeable: true,
            slots: Vec::new(),
        });
        cw.contents.set(
            SlotCoord {
                cube: IVec3::new(5, 0, 5),
                slot: Slot::Room,
            },
            crate::city::Cell {
                id: crate::eorf::EorfId(bin_id),
                facing: Facing::default(),
                evaluation: None,
                build_material: BuildMaterialId::default(),
            },
        );

        cw.places = vec![Place {
            name: "storage room".to_string(),
            requirements: vec![],
            public_storage: true,
            accounting: Some(Approximation {
                digits: 2,
                max: 999,
            }),
            quality_factors: vec![],
            assignable_for: None,
        }];
        cw.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![crate::place::FulfilledPorf::Furniture(SlotCoord {
                cube: IVec3::new(5, 0, 5),
                slot: Slot::Room,
            })],
            contents: Inventory::new(100.0),
            restriction: ParentRestriction::Unrestricted,
        });

        let mut costs = BTreeMap::new();
        costs.insert(
            ElementType::WallLike,
            vec![(UniformResource::Fieldstone, 8), (UniformResource::Lime, 2)],
        );
        let material_list = MaterialList {
            materials: vec![BuildMaterial {
                name: "Fieldstone".to_string(),
                costs,
                fanciness: 0.3,
            }],
        };

        let loc = xlowall(0, 0, 0);
        cw.contents.set(
            loc,
            Cell {
                id: thing(&cw).unwrap(),
                facing: Facing::NegX,
                evaluation: None,
                build_material: BuildMaterialId(0),
            },
        );

        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            None,
            BuildMaterialId::default(),
        );
        construct(&mut cw, &mut pw, &material_list);

        let totals = crate::place::storage_totals(&cw);
        check!(totals.get(&UniformResource::Fieldstone) == Some(&8));
        check!(!totals.contains_key(&UniformResource::Lime));
    }

    #[test]
    fn construct_preserves_undo_record() {
        let (mut cw, mut pw) = make_world();
        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        construct(&mut cw, &mut pw, &crate::materials::MaterialList::default());
        // Undo history survives construct so committed changes remain undoable.
        check!(pw.undo_record.len() == 1);
    }

    #[test]
    fn construct_clears_resource_progress() {
        let (mut cw, mut pw) = make_world();
        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        pw.resource_progress
            .insert(crate::resource::UniformResource::Timber, 30);
        construct(&mut cw, &mut pw, &crate::materials::MaterialList::default());
        check!(pw.resource_progress.is_empty());
    }

    #[test]
    fn undo_after_construct_creates_reverse_proposal() {
        let (mut cw, mut pw) = make_world();
        let loc = xlowall(1, 0, 0);
        pw.propose(
            &cw,
            0,
            (IVec3::new(1, 0, 0), IVec3::new(1, 0, 0)),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        construct(&mut cw, &mut pw, &crate::materials::MaterialList::default());
        check!(cw.contents.get(loc).is_some());

        // Undoing the committed placement proposes its removal (real cell stays put).
        let deltas = pw.undo(&cw);
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::Remove));
        check!(matches!(
            pw.proposed_changes.get(loc),
            Some(Proposal::Remove)
        ));
        check!(cw.contents.get(loc).is_some());
    }

    // ── reset ─────────────────────────────────────────────────────────────────

    #[test]
    fn reset_clears_proposals_but_keeps_undo_record() {
        let (cw, mut pw) = make_world();
        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        check!(!pw.undo_record.is_empty());

        pw.reset();

        check!(pw.proposed_changes.iter().count() == 0);
        // Undo history survives a reset.
        check!(!pw.undo_record.is_empty());
        // Real contents untouched
        check!(cw.contents.get(xlowall(0, 0, 0)).is_none());
    }

    #[test]
    fn reset_clears_resource_progress() {
        let (_cw, mut pw) = make_world();
        pw.resource_progress
            .insert(crate::resource::UniformResource::Timber, 30);
        pw.reset();
        check!(pw.resource_progress.is_empty());
    }

    // ── tick_construction ────────────────────────────────────────────────────

    #[test]
    fn tick_construction_completes_when_fully_paid() {
        let (mut cw, mut pw) = make_world();
        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        let result = tick_construction(
            &mut pw,
            &mut cw,
            true,
            &crate::materials::MaterialList::default(),
        );
        check!(result.is_some());
        check!(pw.num_changes() == 0);
    }

    #[test]
    fn tick_construction_waits_when_not_fully_paid() {
        let (mut cw, mut pw) = make_world();
        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        let result = tick_construction(
            &mut pw,
            &mut cw,
            false,
            &crate::materials::MaterialList::default(),
        );
        check!(result.is_none());
        check!(pw.num_changes() == 1);
    }

    #[test]
    fn tick_construction_no_project_is_a_noop() {
        let (mut cw, mut pw) = make_world();
        let result = tick_construction(
            &mut pw,
            &mut cw,
            true,
            &crate::materials::MaterialList::default(),
        );
        check!(result.is_none());
    }

    // ── compute_construction_absorption ─────────────────────────────────────

    use crate::resource::UniformResource;
    use std::collections::HashMap;

    fn m(pairs: &[(UniformResource, u32)]) -> HashMap<UniformResource, u32> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn fully_applied_from_inflow_when_need_and_rate_allow() {
        let mut inflow = m(&[(UniformResource::Timber, 10)]);
        let mut storage = m(&[]);
        let c = compute_construction_absorption(
            &m(&[(UniformResource::Timber, 30)]),
            &mut inflow,
            &mut storage,
        );
        check!(c.applied[&UniformResource::Timber] == 10);
        check!(!c.from_storage.contains_key(&UniformResource::Timber));
        check!(inflow[&UniformResource::Timber] == 0);
    }

    #[test]
    fn construction_rate_caps_absorption_independently_per_resource() {
        // construct_per_month() is 50 for everything currently; demand above
        // that rate can't be applied even if fully needed and available.
        let mut inflow = m(&[(UniformResource::Timber, 80)]);
        let mut storage = m(&[]);
        let c = compute_construction_absorption(
            &m(&[(UniformResource::Timber, 200)]),
            &mut inflow,
            &mut storage,
        );
        check!(c.applied[&UniformResource::Timber] == 50);
        check!(inflow[&UniformResource::Timber] == 30);
    }

    #[test]
    fn draws_from_inflow_before_storage() {
        let mut inflow = m(&[(UniformResource::Timber, 4)]);
        let mut storage = m(&[(UniformResource::Timber, 100)]);
        let c = compute_construction_absorption(
            &m(&[(UniformResource::Timber, 10)]),
            &mut inflow,
            &mut storage,
        );
        check!(c.applied[&UniformResource::Timber] == 10);
        check!(c.from_storage[&UniformResource::Timber] == 6);
        check!(inflow[&UniformResource::Timber] == 0);
        check!(storage[&UniformResource::Timber] == 94);
    }

    #[test]
    fn never_draws_more_storage_than_needed_after_inflow() {
        let mut inflow = m(&[(UniformResource::Timber, 10)]);
        let mut storage = m(&[(UniformResource::Timber, 100)]);
        let c = compute_construction_absorption(
            &m(&[(UniformResource::Timber, 10)]),
            &mut inflow,
            &mut storage,
        );
        check!(c.applied[&UniformResource::Timber] == 10);
        check!(!c.from_storage.contains_key(&UniformResource::Timber));
        check!(storage[&UniformResource::Timber] == 100);
    }

    // ── redo ──────────────────────────────────────────────────────────────────

    #[test]
    fn redo_reapplies_undone_proposal() {
        let (cw, mut pw) = make_world();
        let loc = xlowall(0, 0, 0);

        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        pw.undo(&cw);
        check!(pw.proposed_changes.get(loc).is_none());

        let deltas = pw.redo(&cw);
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::Add(_)));
        check!(matches!(
            pw.proposed_changes.get(loc),
            Some(Proposal::Place(_))
        ));
    }

    #[test]
    fn redo_on_empty_stack_returns_empty() {
        let (cw, mut pw) = make_world();
        check!(pw.redo(&cw).is_empty());
    }

    #[test]
    fn new_edit_clears_redo_stack() {
        let (cw, mut pw) = make_world();
        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        pw.undo(&cw);
        check!(!pw.redo_record.is_empty());

        // A fresh edit invalidates redo.
        pw.propose(
            &cw,
            0,
            (IVec3::new(1, 0, 0), IVec3::new(1, 0, 0)),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );
        check!(pw.redo_record.is_empty());
        check!(pw.redo(&cw).is_empty());
    }

    // ── get_real_or_proposed ──────────────────────────────────────────────────

    #[test]
    fn get_real_or_proposed_removal() {
        use crate::city::get_real_or_proposed;
        let (mut cw, mut pw) = make_world();
        let loc = xlowall(0, 0, 0);
        // Real cell with id=0
        cw.contents.set(loc, wall_cell(thing(&cw).unwrap()));
        // Proposed removal
        pw.proposed_changes.set(loc, Proposal::Remove);

        check!(get_real_or_proposed(&cw, &pw, loc).is_some());
    }

    #[test]
    fn get_real_or_proposed_falls_back_to_real() {
        use crate::city::get_real_or_proposed;
        let (mut cw, pw) = make_world();
        let loc = xlowall(0, 0, 0);
        cw.contents.set(loc, wall_cell(thing(&cw).unwrap()));

        check!(get_real_or_proposed(&cw, &pw, loc).is_some());
    }

    // ── load_from_offline ─────────────────────────────────────────────────────

    #[test]
    fn load_clears_proposals_and_undo() {
        let (mut cw, mut pw) = make_world();
        pw.propose(
            &cw,
            0,
            (IVec3::ZERO, IVec3::ZERO),
            Slot::XLoWall,
            thing(&cw),
            BuildMaterialId::default(),
        );

        use crate::sparse3d::Sparse3D;
        load_from_offline(&mut cw, &mut pw, Sparse3D::new());

        check!(pw.proposed_changes.iter().count() == 0);
        check!(pw.undo_record.is_empty());
    }

    // ── smoke ─────────────────────────────────────────────────────────────────

    #[test]
    fn smoke_load_propose_undo_construct() {
        use crate::eorf::{find_structure_by_name, load_structure_info};
        use crate::serialization::load_from_str;
        use bevy::math::Vec3;

        let structures = load_structure_info();
        let table_id = find_structure_by_name(&structures, "table").unwrap();
        let wall_id = find_structure_by_name(&structures, "wall").unwrap();

        let mut cw = ConstructedCity::new(structures.clone());
        cw.road_forbidden_zone = false;
        let mut pe = ProposedCity::new();

        // 1. Load a saved building: one z-wall ('-') at (0,0,0).
        let saved = " -\n  \n~~~~~\n~*~*~\n";
        let loaded = load_from_str(saved, &structures).unwrap();
        let load_changes = load_from_offline(&mut cw, &mut pe, loaded);

        assert!(let [(loaded_loc, loaded_cell)] = load_changes.as_slice());
        check!(loaded_loc.slot == Slot::ZLoWall);
        check!(loaded_cell.as_ref().unwrap().id == wall_id);

        // 2. Propose two z-walls via drag (x=1..=2, z=0).
        let drag_deltas = pe.drag(
            &cw,
            (Vec3::new(1.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.0)),
            0,
            wall_id,
            false,
            BuildMaterialId::default(),
        );
        check!(drag_deltas.len() == 2);
        check!(pe.proposed_changes.iter().count() == 2);

        // 3. Propose a desk at (2, 0, 2) via click.
        let click_deltas = pe.click(
            &cw,
            Vec3::new(2.0, 0.0, 2.0),
            table_id,
            0,
            false,
            BuildMaterialId::default(),
        );
        check!(click_deltas.len() == 1);
        check!(matches!(click_deltas[0].1, ProposalView::Add(_)));
        check!(pe.proposed_changes.iter().count() == 3);
        // The desk is vantage-evaluated (per furniture.ron), so plopping it
        // attaches an empty VantageEvaluation for the QNN to fill in.
        let table_loc = RelSlotCoord::new(2, 0, 2, RelSlot::Room);
        assert!(let Some(Proposal::Place(table_cell)) = pe.proposed_changes.get(table_loc));
        check!(table_cell.evaluation.is_some());

        // 4. Undo the desk; the two wall proposals remain.
        let undo_deltas = pe.undo(&cw);
        check!(undo_deltas.len() == 1);
        check!(matches!(undo_deltas[0].1, ProposalView::None));
        check!(pe.proposed_changes.iter().count() == 2);

        // 5. Construct: two new walls land in real contents; proposals clear.
        let real_changes = construct(&mut cw, &mut pe, &crate::materials::MaterialList::load());
        check!(real_changes.len() == 2);
        check!(pe.proposed_changes.iter().count() == 0);
        // Undo history survives construct (the wall-drag record remains).
        check!(pe.undo_record.len() == 1);
        // Original loaded wall + 2 newly constructed walls
        check!(cw.contents.iter().count() == 3);
    }

    // ── WallPlop ─────────────────────────────────────────────────────────────

    #[test]
    fn click_on_wall_plop_structure_snaps_to_nearest_wall() {
        use crate::eorf::{find_structure_by_name, load_structure_info};
        use crate::sparse3d::Facing;
        use bevy::math::Vec3;

        let structures = load_structure_info();
        let window_id = find_structure_by_name(&structures, "window").unwrap();

        let mut cw = ConstructedCity::new(structures.clone());
        cw.road_forbidden_zone = false;
        let mut pe = ProposedCity::new();

        // A click near an X-boundary (x close to an integer, z mid-cell) snaps
        // to the XLoWall there, not the ZLoWall of the current room.
        let deltas = pe.click(
            &cw,
            Vec3::new(3.02, 0.0, 3.6),
            window_id,
            Facing::PosX as i32,
            false,
            BuildMaterialId::default(),
        );

        check!(deltas.len() == 1);
        let (loc, view) = &deltas[0];
        check!(loc.slot == Slot::XLoWall);
        check!(loc.cube == IVec3::new(3, 0, 4));
        assert!(let ProposalView::Add(cell) = view);
        check!(cell.id == window_id);
        check!(cell.facing == Facing::PosX);
    }

    #[test]
    fn click_on_wall_plop_structure_removes_existing_cell() {
        use crate::eorf::{find_structure_by_name, load_structure_info};
        use crate::sparse3d::Facing;
        use bevy::math::Vec3;

        let structures = load_structure_info();
        let column_id = find_structure_by_name(&structures, "column").unwrap();

        let mut cw = ConstructedCity::new(structures.clone());
        cw.road_forbidden_zone = false;
        cw.contents.set(
            RelSlotCoord::new(4, 0, 3, RelSlot::ZLoWall),
            Cell {
                id: column_id,
                facing: Facing::NegZ,
                evaluation: None,
                build_material: BuildMaterialId::default(),
            },
        );
        let mut pe = ProposedCity::new();

        // A click near a Z-boundary with `remove: true` proposes removing
        // whatever real cell is there.
        let deltas = pe.click(
            &cw,
            Vec3::new(3.6, 0.0, 3.02),
            column_id,
            0,
            true,
            BuildMaterialId::default(),
        );

        check!(deltas.len() == 1);
        let (loc, view) = &deltas[0];
        check!(loc.slot == Slot::ZLoWall);
        check!(loc.cube == IVec3::new(4, 0, 3));
        check!(matches!(view, ProposalView::Remove));
    }
}
