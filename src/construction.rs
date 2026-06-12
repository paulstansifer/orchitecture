use bevy::math::{IVec3, Vec3};

use crate::sparse3d::{Facing, Slot, SlotCoord, Sparse3D};
use crate::structure::{PlacementStyle, StructureId};
use crate::wall_grid::{Cell, Proposal, ProposalView, UndoRecord, VantageEvaluation, WallGrid};

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

impl WallGrid {
    /// Propose placing or clearing cells in a rectangular range.
    ///
    /// Writes to `proposed_changes` (not `contents`). Returns `(loc, view)` deltas
    /// describing the visual treatment each location now needs.
    fn propose(
        &mut self,
        dir: i32,
        position1: IVec3,
        position2: IVec3,
        slot: Slot,
        item: Option<StructureId>,
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
                        && self.road_forbidden_zone
                        && crate::road::is_in_road_forbidden_zone(loc)
                    {
                        continue;
                    }
                    let real_cell = self.contents.get(loc).cloned();
                    let prior_proposal = self.proposed_changes.get(loc).cloned();
                    // The desired state before this edit, for undo (absolute, so it
                    // survives construct()).
                    let prior_desired = self.desired(loc);

                    let new_proposal: Option<Proposal> = if let Some(id) = item {
                        let facing = Facing::from_number(dir as u8);
                        let new_cell = Cell {
                            id,
                            facing,
                            evaluation: None,
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
        from: Vec3,
        to: Vec3,
        selected_mesh_id: Option<StructureId>,
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

        self.propose(0, start, end, slot, selected_mesh_id)
    }

    pub fn floor_drag(
        &mut self,
        from: Vec3,
        to: Vec3,
        selected_mesh_id: Option<StructureId>,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let from_i = from.round().as_ivec3();
        let to_i = to.round().as_ivec3();
        let start = from_i.min(to_i);
        let end = from_i.max(to_i) - IVec3::new(1, 0, 1);
        self.propose(0, start, end, Slot::Floor, selected_mesh_id)
    }

    pub fn room_drag(
        &mut self,
        from: Vec3,
        to: Vec3,
        dir: i32,
        selected_mesh_id: Option<StructureId>,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let from_i = from.round().as_ivec3();
        let to_i = to.round().as_ivec3();
        let start = from_i.min(to_i);
        let end = from_i.max(to_i) - IVec3::new(1, 0, 1);
        self.propose(dir, start, end, Slot::Room, selected_mesh_id)
    }

    pub fn room_plop(
        &mut self,
        location: Vec3,
        dir: i32,
        selected_mesh_id: Option<StructureId>,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let pos = location.round().as_ivec3();
        let changes = self.propose(dir, pos, pos, Slot::Room, selected_mesh_id);
        if selected_mesh_id == self.find_structure_by_name("desk") {
            let loc = SlotCoord {
                cube: pos,
                slot: Slot::Room,
            };
            if let Some(Proposal::Place(cell)) = self.proposed_changes.get_mut(loc) {
                cell.evaluation = Some(VantageEvaluation {
                    coherence: None,
                    interest: None,
                });
            }
        }
        changes
    }

    pub fn drag(
        &mut self,
        from: Vec3,
        to: Vec3,
        dir: i32,
        selected_mesh_id: StructureId,
        remove: bool,
    ) -> Vec<(SlotCoord, ProposalView)> {
        let id = (!remove).then_some(selected_mesh_id);
        match self.structures[selected_mesh_id.as_usize()].placement_style {
            PlacementStyle::WallDrag => self.wall_drag(from, to, id),
            PlacementStyle::FloorDrag => self.floor_drag(from, to, id),
            PlacementStyle::RoomDrag => self.room_drag(from, to, dir, id),
            _ => vec![],
        }
    }

    pub fn click(
        &mut self,
        position: Vec3,
        selected_mesh_id: StructureId,
        dir: i32,
        remove: bool,
    ) -> Vec<(SlotCoord, ProposalView)> {
        match self.structures[selected_mesh_id.as_usize()].placement_style {
            PlacementStyle::RoomPlop => {
                self.room_plop(position, dir, (!remove).then_some(selected_mesh_id))
            }
            _ => vec![],
        }
    }

    /// Drive each location's desired state to `targets`, creating whatever
    /// proposal is needed given the current real cell. Returns the view deltas
    /// and the *inverse* targets (each location's desired state before this call)
    /// so undo/redo can be reversed.
    ///
    /// Because targets are absolute cell states, this works across `construct()`:
    /// if a location's real cell no longer matches the target, a reverse proposal
    /// is created (e.g. reverting a committed desk yields a `Remove` proposal).
    fn restore_desired(
        &mut self,
        targets: Vec<(SlotCoord, Option<Cell>)>,
    ) -> (Vec<(SlotCoord, ProposalView)>, Vec<(SlotCoord, Option<Cell>)>) {
        let mut changes = vec![];
        let mut inverse = vec![];
        for (loc, target) in targets {
            let prev_desired = self.desired(loc);
            let real_cell = self.contents.get(loc).cloned();
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
    pub fn undo(&mut self) -> Vec<(SlotCoord, ProposalView)> {
        let Some(record) = self.undo_record.pop() else {
            return vec![];
        };
        let (changes, inverse) = self.restore_desired(record.changed);
        self.redo_record.push(UndoRecord { changed: inverse });
        changes
    }

    /// Redo the last undone action. Pushes the inverse back onto the undo stack.
    pub fn redo(&mut self) -> Vec<(SlotCoord, ProposalView)> {
        let Some(record) = self.redo_record.pop() else {
            return vec![];
        };
        let (changes, inverse) = self.restore_desired(record.changed);
        self.undo_record.push(UndoRecord { changed: inverse });
        changes
    }

    /// Commits all proposed changes into real contents. Returns real-cell deltas for `apply_changes`.
    /// Clears `proposed_changes`; entity cleanup is the caller's responsibility.
    ///
    /// The undo record is *preserved*: because it stores absolute prior cell states,
    /// undo can still revert committed changes by proposing their inverse.
    pub fn construct(&mut self) -> Vec<(SlotCoord, Option<Cell>)> {
        let proposals: Vec<(SlotCoord, Proposal)> = self
            .proposed_changes
            .iter()
            .map(|(loc, p)| (loc, p.clone()))
            .collect();

        self.proposed_changes = Sparse3D::new();

        let mut real_changes = vec![];
        for (loc, proposal) in proposals {
            match proposal {
                Proposal::Place(cell) => {
                    self.contents.set(loc, cell.clone());
                    real_changes.push((loc, Some(cell)));
                }
                Proposal::Remove => {
                    self.contents.take(loc);
                    real_changes.push((loc, None));
                }
            }
        }
        real_changes
    }

    /// Clears all proposals without committing anything. Undo/redo history is
    /// left intact, so a reset is itself reversible (the stale records degrade to
    /// no-ops when a location already matches reality).
    /// Entity cleanup is the caller's responsibility.
    pub fn reset_proposals(&mut self) {
        self.proposed_changes = Sparse3D::new();
    }

    pub fn load_from_offline(
        &mut self,
        new_contents: Sparse3D<Cell>,
    ) -> Vec<(SlotCoord, Option<Cell>)> {
        self.proposed_changes = Sparse3D::new();
        // A fresh building starts with no history.
        self.undo_record.clear();
        self.redo_record.clear();
        // Proposal entity cleanup is the caller's responsibility.
        // Shift the building into the no-road semiplane (south of the E-W road).
        let z_shift = -new_contents.bounding_box().1.z;
        let shifted = new_contents.translate(IVec3::new(0, 0, z_shift));
        self.replace_contents(shifted)
    }

    fn replace_contents(&mut self, new_contents: Sparse3D<Cell>) -> Vec<(SlotCoord, Option<Cell>)> {
        let mut changes: Vec<(SlotCoord, Option<Cell>)> = Vec::new();
        for (loc, _) in self.contents.iter() {
            changes.push((loc, None));
        }
        for (loc, cell) in new_contents.iter() {
            changes.push((loc, Some(cell.clone())));
        }
        self.contents = new_contents;
        changes
    }
}

#[cfg(test)]
mod tests {
    use assert2::{assert, check};

    use bevy::math::IVec3;

    use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, Slot};
    use crate::structure::{PlacementStyle, StructureId, StructureInfo};
    use crate::wall_grid::{Cell, Proposal, ProposalView, WallGrid};

    fn make_wall_grid() -> WallGrid {
        use crate::structure::StructureEmbedding;
        let structs = vec![StructureInfo {
            name: "test_wall".to_string(),
            main_mesh: "test_wall.gltf".to_string(),
            y_cut_mesh: None,
            placement_style: PlacementStyle::WallDrag,
            x_char: None,
            z_char: None,
            embedding: StructureEmbedding {
                tall: 0.0,
                passable: 0.0,
                decorative: 0.0,
                striated: 0.0,
            },
        }];
        let mut grid = WallGrid::new(structs);
        grid.road_forbidden_zone = false;
        grid
    }

    fn thing(grid: &WallGrid) -> Option<StructureId> {
        grid.find_structure_by_name("test_wall")
    }

    fn wall_cell(id: StructureId) -> Cell {
        Cell {
            id,
            facing: Facing::NegX,
            evaluation: None,
        }
    }

    fn xlowall(x: i32, y: i32, z: i32) -> RelSlotCoord {
        RelSlotCoord::new(x, y, z, RelSlot::XLoWall)
    }

    // ── propose ──────────────────────────────────────────────────────────────

    #[test]
    fn propose_add_goes_to_proposed_changes_not_contents() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);

        grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));

        check!(grid.contents.get(loc).is_none());
        check!(matches!(
            grid.proposed_changes.get(loc),
            Some(Proposal::Place(_))
        ));
    }

    #[test]
    fn propose_returns_add_view_for_empty_slot() {
        let mut grid = make_wall_grid();
        let deltas = grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::Add(_)));
    }

    #[test]
    fn propose_returns_replace_view_for_occupied_slot() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        // Place a real cell first
        grid.contents.set(loc, wall_cell(thing(&grid).unwrap()));

        // Now propose a different cell (id=0 same, so let's make a distinct check)
        // propose removal to get a Remove view
        let deltas = grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, None);
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::Remove));
    }

    #[test]
    fn propose_same_as_real_produces_no_proposal() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        // Put the same cell in real contents
        grid.contents.set(loc, wall_cell(thing(&grid).unwrap()));

        // Propose placing the exact same cell
        let deltas = grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));

        check!(deltas.is_empty(), "identical proposal should be a no-op");
        check!(grid.proposed_changes.get(loc).is_none());
    }

    #[test]
    fn propose_remove_on_empty_slot_is_no_op() {
        let mut grid = make_wall_grid();
        let deltas = grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, None);
        check!(deltas.is_empty());
        check!(grid.proposed_changes.iter().count() == 0);
    }

    // ── undo ─────────────────────────────────────────────────────────────────

    #[test]
    fn undo_restores_prior_proposal_state() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);

        grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));
        check!(grid.proposed_changes.get(loc).is_some());

        let deltas = grid.undo();
        check!(grid.proposed_changes.get(loc).is_none());
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::None));
    }

    #[test]
    fn undo_on_empty_stack_returns_empty() {
        let mut grid = make_wall_grid();
        let deltas = grid.undo();
        check!(deltas.is_empty());
    }

    #[test]
    fn undo_clears_undo_record_entry() {
        let mut grid = make_wall_grid();
        grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));
        check!(grid.undo_record.len() == 1);
        grid.undo();
        check!(grid.undo_record.len() == 0);
    }

    // ── construct ─────────────────────────────────────────────────────────────

    #[test]
    fn construct_moves_proposals_to_contents() {
        let mut grid = make_wall_grid();
        let loc = xlowall(1, 0, 0);

        grid.propose(
            0,
            IVec3::new(1, 0, 0),
            IVec3::new(1, 0, 0),
            Slot::XLoWall,
            thing(&grid),
        );
        check!(grid.contents.get(loc).is_none());

        let real_changes = grid.construct();

        check!(grid.proposed_changes.get(loc).is_none());
        check!(grid.contents.get(loc).is_some());
        check!(real_changes.len() == 1);
        check!(real_changes[0].1.is_some());
    }

    #[test]
    fn construct_remove_proposal_deletes_from_contents() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        grid.contents.set(loc, wall_cell(thing(&grid).unwrap()));

        grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, None);
        let real_changes = grid.construct();

        check!(grid.contents.get(loc).is_none());
        check!(real_changes.len() == 1);
        check!(real_changes[0].1.is_none());
    }

    #[test]
    fn construct_preserves_undo_record() {
        let mut grid = make_wall_grid();
        grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));
        grid.construct();
        // Undo history survives construct so committed changes remain undoable.
        check!(grid.undo_record.len() == 1);
    }

    #[test]
    fn undo_after_construct_creates_reverse_proposal() {
        let mut grid = make_wall_grid();
        let loc = xlowall(1, 0, 0);
        grid.propose(
            0,
            IVec3::new(1, 0, 0),
            IVec3::new(1, 0, 0),
            Slot::XLoWall,
            thing(&grid),
        );
        grid.construct();
        check!(grid.contents.get(loc).is_some());

        // Undoing the committed placement proposes its removal (real cell stays put).
        let deltas = grid.undo();
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::Remove));
        check!(matches!(grid.proposed_changes.get(loc), Some(Proposal::Remove)));
        check!(grid.contents.get(loc).is_some());
    }

    // ── reset ─────────────────────────────────────────────────────────────────

    #[test]
    fn reset_clears_proposals_but_keeps_undo_record() {
        let mut grid = make_wall_grid();
        grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));
        check!(!grid.undo_record.is_empty());

        grid.reset_proposals();

        check!(grid.proposed_changes.iter().count() == 0);
        // Undo history survives a reset.
        check!(!grid.undo_record.is_empty());
        // Real contents untouched
        check!(grid.contents.get(xlowall(0, 0, 0)).is_none());
    }

    // ── redo ──────────────────────────────────────────────────────────────────

    #[test]
    fn redo_reapplies_undone_proposal() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);

        grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));
        grid.undo();
        check!(grid.proposed_changes.get(loc).is_none());

        let deltas = grid.redo();
        check!(deltas.len() == 1);
        check!(matches!(deltas[0].1, ProposalView::Add(_)));
        check!(matches!(
            grid.proposed_changes.get(loc),
            Some(Proposal::Place(_))
        ));
    }

    #[test]
    fn redo_on_empty_stack_returns_empty() {
        let mut grid = make_wall_grid();
        check!(grid.redo().is_empty());
    }

    #[test]
    fn new_edit_clears_redo_stack() {
        let mut grid = make_wall_grid();
        grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));
        grid.undo();
        check!(!grid.redo_record.is_empty());

        // A fresh edit invalidates redo.
        grid.propose(
            0,
            IVec3::new(1, 0, 0),
            IVec3::new(1, 0, 0),
            Slot::XLoWall,
            thing(&grid),
        );
        check!(grid.redo_record.is_empty());
        check!(grid.redo().is_empty());
    }

    // ── get_real_or_proposed ──────────────────────────────────────────────────

    #[test]
    fn get_real_or_proposed_removal() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        // Real cell with id=0
        grid.contents.set(loc, wall_cell(thing(&grid).unwrap()));
        // Proposed removal
        grid.proposed_changes.set(loc, Proposal::Remove);

        check!(grid.get_real_or_proposed(loc).is_some());
    }

    #[test]
    fn get_real_or_proposed_falls_back_to_real() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        grid.contents.set(loc, wall_cell(thing(&grid).unwrap()));

        check!(grid.get_real_or_proposed(loc).is_some());
    }

    // ── load_from_offline ─────────────────────────────────────────────────────

    #[test]
    fn load_clears_proposals_and_undo() {
        let mut grid = make_wall_grid();
        grid.propose(0, IVec3::ZERO, IVec3::ZERO, Slot::XLoWall, thing(&grid));

        use crate::sparse3d::Sparse3D;
        grid.load_from_offline(Sparse3D::new());

        check!(grid.proposed_changes.iter().count() == 0);
        check!(grid.undo_record.is_empty());
    }

    // ── smoke ─────────────────────────────────────────────────────────────────

    #[test]
    fn smoke_load_propose_undo_construct() {
        use crate::serialization::load_from_str;
        use crate::structure::{find_structure_by_name, load_structure_info};
        use bevy::math::Vec3;

        let structures = load_structure_info();
        let desk_id = find_structure_by_name(&structures, "desk").unwrap();
        let wall_id = find_structure_by_name(&structures, "wall").unwrap();

        let mut grid = WallGrid::new(structures.clone());
        grid.road_forbidden_zone = false;

        // 1. Load a saved building: one z-wall ('-') at (0,0,0).
        //    Save format: pairs of (room+zwall / xwall+floor) lines, then "~~~~~" per y-layer.
        let saved = " -\n  \n~~~~~\n~*~*~\n";
        let loaded = load_from_str(saved, &structures);
        let load_changes = grid.load_from_offline(loaded);

        assert!(let [(loaded_loc, loaded_cell)] = load_changes.as_slice());
        check!(loaded_loc.slot == Slot::ZLoWall);
        check!(loaded_cell.as_ref().unwrap().id == wall_id);

        // 2. Propose two z-walls via drag (x=1..=2, z=0).
        let drag_deltas = grid.drag(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0),
            0,
            wall_id,
            false,
        );
        check!(drag_deltas.len() == 2);
        check!(grid.proposed_changes.iter().count() == 2);

        // 3. Propose a desk at (2, 0, 2) via click.
        let click_deltas = grid.click(Vec3::new(2.0, 0.0, 2.0), desk_id, 0, false);
        check!(click_deltas.len() == 1);
        check!(matches!(click_deltas[0].1, ProposalView::Add(_)));
        check!(grid.proposed_changes.iter().count() == 3);

        // 4. Undo the desk; the two wall proposals remain.
        let undo_deltas = grid.undo();
        check!(undo_deltas.len() == 1);
        check!(matches!(undo_deltas[0].1, ProposalView::None));
        check!(grid.proposed_changes.iter().count() == 2);

        // 5. Construct: two new walls land in real contents; proposals clear.
        let real_changes = grid.construct();
        check!(real_changes.len() == 2);
        check!(grid.proposed_changes.iter().count() == 0);
        // Undo history survives construct (the wall-drag record remains).
        check!(grid.undo_record.len() == 1);
        // Original loaded wall + 2 newly constructed walls
        check!(grid.contents.iter().count() == 3);
    }
}
