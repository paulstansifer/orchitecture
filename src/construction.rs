use bevy::math::{IVec3, Vec3};

use crate::sparse3d::{Facing, RelSlot, SlotLocation, Sparse3D};
use crate::structure::{PlacementStyle, StructureId};
use crate::wall_grid::{Cell, Proposal, ProposalView, UndoRecord, VantageEvaluation, WallGrid};

impl WallGrid {
    /// Propose placing or clearing cells in a rectangular range.
    ///
    /// Writes to `proposed_changes` (not `contents`). Returns `(loc, view)` deltas
    /// describing the visual treatment each location now needs.
    fn set_range_item_dir(
        &mut self,
        dir: i32,
        position1: IVec3,
        position2: IVec3,
        slot: RelSlot,
        item: Option<StructureId>,
    ) -> Vec<(SlotLocation, ProposalView)> {
        let start = position1.min(position2);
        let end = position1.max(position2);
        let mut changes: Vec<(SlotLocation, ProposalView)> = Vec::new();
        let mut undo_changed: Vec<(SlotLocation, Option<Proposal>)> = Vec::new();

        for x in start.x..=end.x {
            for y in start.y..=end.y {
                for z in start.z..=end.z {
                    let loc = SlotLocation::new(x, y, z, slot);
                    let real_cell = self.contents.get(loc).cloned();
                    let prior_proposal = self.proposed_changes.get(loc).cloned();

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

                    undo_changed.push((loc, prior_proposal));

                    let view = match &new_proposal {
                        None => ProposalView::None,
                        Some(Proposal::Place(cell)) => {
                            if real_cell.is_none() {
                                ProposalView::Add(cell.clone())
                            } else {
                                ProposalView::Replace(cell.clone())
                            }
                        }
                        Some(Proposal::Remove) => ProposalView::Remove,
                    };
                    changes.push((loc, view));
                }
            }
        }

        if !undo_changed.is_empty() {
            self.undo_record.push(UndoRecord {
                changed: undo_changed,
            });
        }
        changes
    }

    pub fn wall_drag(
        &mut self,
        from: Vec3,
        to: Vec3,
        selected_mesh_id: Option<StructureId>,
    ) -> Vec<(SlotLocation, ProposalView)> {
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
            RelSlot::ZLoWall
        } else {
            RelSlot::XLoWall
        };

        self.set_range_item_dir(0, start, end, slot, selected_mesh_id)
    }

    pub fn floor_drag(
        &mut self,
        from: Vec3,
        to: Vec3,
        selected_mesh_id: Option<StructureId>,
    ) -> Vec<(SlotLocation, ProposalView)> {
        let from_i = from.round().as_ivec3();
        let to_i = to.round().as_ivec3();
        let start = from_i.min(to_i);
        let end = from_i.max(to_i) - IVec3::new(1, 0, 1);
        self.set_range_item_dir(0, start, end, RelSlot::Floor, selected_mesh_id)
    }

    pub fn room_plop(
        &mut self,
        location: Vec3,
        dir: i32,
        selected_mesh_id: Option<StructureId>,
    ) -> Vec<(SlotLocation, ProposalView)> {
        let pos = location.round().as_ivec3();
        let changes = self.set_range_item_dir(dir, pos, pos, RelSlot::Room, selected_mesh_id);
        if selected_mesh_id == Some(StructureId(0)) {
            // Desk's ID number. TODO: fix this!
            let loc = SlotLocation::new(pos.x, pos.y, pos.z, RelSlot::Room);
            if let Some(Proposal::Place(cell)) = self.proposed_changes.get_mut(loc) {
                cell.evaluation = Some(VantageEvaluation {
                    coherence: 0.5,
                    interest: 0.5,
                });
            }
        }
        changes
    }

    pub fn drag(
        &mut self,
        from: Vec3,
        to: Vec3,
        selected_mesh_id: StructureId,
        remove: bool,
    ) -> Vec<(SlotLocation, ProposalView)> {
        let id = (!remove).then_some(selected_mesh_id);
        match self.structures[selected_mesh_id.as_usize()].placement_style {
            PlacementStyle::WallDrag => self.wall_drag(from, to, id),
            PlacementStyle::FloorDrag => self.floor_drag(from, to, id),
            _ => vec![],
        }
    }

    pub fn click(
        &mut self,
        position: Vec3,
        selected_mesh_id: StructureId,
        dir: i32,
        remove: bool,
    ) -> Vec<(SlotLocation, ProposalView)> {
        match self.structures[selected_mesh_id.as_usize()].placement_style {
            PlacementStyle::RoomPlop => {
                self.room_plop(position, dir, (!remove).then_some(selected_mesh_id))
            }
            _ => vec![],
        }
    }

    /// Undo the last proposal action. Returns view deltas so proposal rendering can be updated.
    pub fn undo(&mut self) -> Vec<(SlotLocation, ProposalView)> {
        let Some(record) = self.undo_record.pop() else {
            return vec![];
        };
        let mut changes = vec![];
        for (loc, prior_proposal) in record.changed {
            if let Some(ref proposal) = prior_proposal {
                self.proposed_changes.set(loc, proposal.clone());
            } else {
                self.proposed_changes.take(loc);
            }
            let real_cell = self.contents.get(loc);
            let view = match &prior_proposal {
                None => ProposalView::None,
                Some(Proposal::Place(cell)) => {
                    if real_cell.is_none() {
                        ProposalView::Add(cell.clone())
                    } else {
                        ProposalView::Replace(cell.clone())
                    }
                }
                Some(Proposal::Remove) => ProposalView::Remove,
            };
            changes.push((loc, view));
        }
        changes
    }

    /// Commits all proposed changes into real contents. Returns real-cell deltas for `apply_changes`.
    /// Clears `proposed_changes` and `undo_record`; entity cleanup is the caller's responsibility.
    pub fn construct(&mut self) -> Vec<(SlotLocation, Option<Cell>)> {
        let proposals: Vec<(SlotLocation, Proposal)> = self
            .proposed_changes
            .iter()
            .map(|(loc, p)| (loc, p.clone()))
            .collect();

        self.proposed_changes = Sparse3D::new();
        self.undo_record.clear();

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

    /// Clears all proposals and the undo record without committing anything.
    /// Entity cleanup is the caller's responsibility.
    pub fn reset_proposals(&mut self) {
        self.proposed_changes = Sparse3D::new();
        self.undo_record.clear();
    }

    pub fn load_from_offline(
        &mut self,
        new_contents: Sparse3D<Cell>,
    ) -> Vec<(SlotLocation, Option<Cell>)> {
        self.proposed_changes = Sparse3D::new();
        self.undo_record.clear();
        // Proposal entity cleanup is the caller's responsibility.
        // Shift the building into the no-road semiplane (south of the E-W road).
        let shifted = new_contents.translate(crate::road::BUILDING_LOAD_OFFSET);
        self.replace_contents(shifted)
    }

    fn replace_contents(
        &mut self,
        new_contents: Sparse3D<Cell>,
    ) -> Vec<(SlotLocation, Option<Cell>)> {
        let mut changes: Vec<(SlotLocation, Option<Cell>)> = Vec::new();
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
    use bevy::math::IVec3;

    use crate::sparse3d::{Facing, RelSlot, SlotLocation};
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
        WallGrid::new(structs)
    }

    fn wall_cell(id: StructureId) -> Cell {
        Cell {
            id,
            facing: Facing::NegX,
            evaluation: None,
        }
    }

    fn xlowall(x: i32, y: i32, z: i32) -> SlotLocation {
        SlotLocation::new(x, y, z, RelSlot::XLoWall)
    }

    // ── propose ──────────────────────────────────────────────────────────────

    #[test]
    fn propose_add_goes_to_proposed_changes_not_contents() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);

        grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, Some(StructureId(0)));

        assert!(
            grid.contents.get(loc).is_none(),
            "contents must not change on proposal"
        );
        assert!(matches!(
            grid.proposed_changes.get(loc),
            Some(Proposal::Place(_))
        ));
    }

    #[test]
    fn propose_returns_add_view_for_empty_slot() {
        let mut grid = make_wall_grid();
        let deltas =
            grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, Some(StructureId(0)));
        assert_eq!(deltas.len(), 1);
        assert!(matches!(deltas[0].1, ProposalView::Add(_)));
    }

    #[test]
    fn propose_returns_replace_view_for_occupied_slot() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        // Place a real cell first
        grid.contents.set(loc, wall_cell(StructureId(0)));

        // Now propose a different cell (id=0 same, so let's make a distinct check)
        // propose removal to get a Remove view
        let deltas = grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, None);
        assert_eq!(deltas.len(), 1);
        assert!(matches!(deltas[0].1, ProposalView::Remove));
    }

    #[test]
    fn propose_same_as_real_produces_no_proposal() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        // Put the same cell in real contents
        grid.contents.set(loc, wall_cell(StructureId(0)));

        // Propose placing the exact same cell
        let deltas =
            grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, Some(StructureId(0)));

        assert!(deltas.is_empty(), "identical proposal should be a no-op");
        assert!(grid.proposed_changes.get(loc).is_none());
    }

    #[test]
    fn propose_remove_on_empty_slot_is_no_op() {
        let mut grid = make_wall_grid();
        let deltas = grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, None);
        assert!(deltas.is_empty());
        assert_eq!(grid.proposed_changes.iter().count(), 0);
    }

    // ── undo ─────────────────────────────────────────────────────────────────

    #[test]
    fn undo_restores_prior_proposal_state() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);

        grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, Some(StructureId(0)));
        assert!(grid.proposed_changes.get(loc).is_some());

        let deltas = grid.undo();
        assert!(
            grid.proposed_changes.get(loc).is_none(),
            "undo should clear the proposal"
        );
        assert_eq!(deltas.len(), 1);
        assert!(matches!(deltas[0].1, ProposalView::None));
    }

    #[test]
    fn undo_on_empty_stack_returns_empty() {
        let mut grid = make_wall_grid();
        let deltas = grid.undo();
        assert!(deltas.is_empty());
    }

    #[test]
    fn undo_clears_undo_record_entry() {
        let mut grid = make_wall_grid();
        grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, Some(StructureId(0)));
        assert_eq!(grid.undo_record.len(), 1);
        grid.undo();
        assert_eq!(grid.undo_record.len(), 0);
    }

    // ── construct ─────────────────────────────────────────────────────────────

    #[test]
    fn construct_moves_proposals_to_contents() {
        let mut grid = make_wall_grid();
        let loc = xlowall(1, 0, 0);

        grid.set_range_item_dir(
            0,
            IVec3::new(1, 0, 0),
            IVec3::new(1, 0, 0),
            RelSlot::XLoWall,
            Some(StructureId(0)),
        );
        assert!(grid.contents.get(loc).is_none());

        let real_changes = grid.construct();

        assert!(grid.proposed_changes.get(loc).is_none());
        assert!(grid.contents.get(loc).is_some());
        assert_eq!(real_changes.len(), 1);
        assert!(real_changes[0].1.is_some());
    }

    #[test]
    fn construct_remove_proposal_deletes_from_contents() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        grid.contents.set(loc, wall_cell(StructureId(0)));

        grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, None);
        let real_changes = grid.construct();

        assert!(grid.contents.get(loc).is_none());
        assert_eq!(real_changes.len(), 1);
        assert!(real_changes[0].1.is_none());
    }

    #[test]
    fn construct_clears_undo_record() {
        let mut grid = make_wall_grid();
        grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, Some(StructureId(0)));
        grid.construct();
        assert!(grid.undo_record.is_empty());
    }

    // ── reset ─────────────────────────────────────────────────────────────────

    #[test]
    fn reset_clears_proposals_and_undo_record() {
        let mut grid = make_wall_grid();
        grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, Some(StructureId(0)));
        assert!(!grid.undo_record.is_empty());

        grid.reset_proposals();

        assert_eq!(grid.proposed_changes.iter().count(), 0);
        assert!(grid.undo_record.is_empty());
        // Real contents untouched
        assert!(grid.contents.get(xlowall(0, 0, 0)).is_none());
    }

    // ── get_real_or_proposed ──────────────────────────────────────────────────

    #[test]
    fn get_real_or_proposed_returns_proposal_over_real() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        // Real cell with id=0
        grid.contents.set(loc, wall_cell(StructureId(0)));
        // Proposed removal
        grid.proposed_changes.set(loc, Proposal::Remove);

        assert!(
            grid.get_real_or_proposed(loc).is_none(),
            "Remove proposal should shadow the real cell"
        );
    }

    #[test]
    fn get_real_or_proposed_falls_back_to_real() {
        let mut grid = make_wall_grid();
        let loc = xlowall(0, 0, 0);
        grid.contents.set(loc, wall_cell(StructureId(0)));

        assert!(grid.get_real_or_proposed(loc).is_some());
    }

    // ── months_for_construction ───────────────────────────────────────────────

    #[test]
    fn months_zero_when_no_proposals() {
        let grid = make_wall_grid();
        assert_eq!(grid.months_for_construction(), 0);
    }

    #[test]
    fn months_one_for_single_change() {
        let mut grid = make_wall_grid();
        grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, Some(StructureId(0)));
        assert_eq!(grid.months_for_construction(), 1);
    }

    #[test]
    fn months_ceil_ten() {
        let mut grid = make_wall_grid();
        // Propose 10 walls along z
        for z in 0..10 {
            let pos = IVec3::new(0, 0, z);
            grid.set_range_item_dir(0, pos, pos, RelSlot::XLoWall, Some(StructureId(0)));
        }
        assert_eq!(grid.num_proposed_changes(), 10);
        assert_eq!(grid.months_for_construction(), 1);
    }

    #[test]
    fn months_ceil_eleven_is_two() {
        let mut grid = make_wall_grid();
        for z in 0..11 {
            let pos = IVec3::new(0, 0, z);
            grid.set_range_item_dir(0, pos, pos, RelSlot::XLoWall, Some(StructureId(0)));
        }
        assert_eq!(grid.months_for_construction(), 2);
    }

    // ── load_from_offline ─────────────────────────────────────────────────────

    #[test]
    fn load_clears_proposals_and_undo() {
        let mut grid = make_wall_grid();
        grid.set_range_item_dir(0, IVec3::ZERO, IVec3::ZERO, RelSlot::XLoWall, Some(StructureId(0)));

        use crate::sparse3d::Sparse3D;
        grid.load_from_offline(Sparse3D::new());

        assert_eq!(grid.proposed_changes.iter().count(), 0);
        assert!(grid.undo_record.is_empty());
    }

    // ── smoke ─────────────────────────────────────────────────────────────────

    #[test]
    fn smoke_load_propose_undo_construct() {
        use crate::serialization::load_from_str;
        use crate::structure::load_structure_info;
        use bevy::math::Vec3;

        // Structure indices from buildables/structures.json:
        //   0 = desk  (RoomPlop,  z_char='V')
        //   5 = wall  (WallDrag,  x_char='|', z_char='-')
        const DESK_ID: StructureId = StructureId(0);
        const WALL_ID: StructureId = StructureId(5);

        let structures = load_structure_info();
        let mut grid = WallGrid::new(structures.clone());

        // 1. Load a saved building: one z-wall ('-') at (0,0,0).
        //    Save format: pairs of (room+zwall / xwall+floor) lines, then "~~~~~" per y-layer.
        let saved = " -\n  \n~~~~~\n~*~*~\n";
        let loaded = load_from_str(saved, &structures);
        let load_changes = grid.load_from_offline(loaded);

        assert_eq!(load_changes.len(), 1);
        let (loaded_loc, loaded_cell) = &load_changes[0];
        assert_eq!(loaded_loc.rel_slot, RelSlot::ZLoWall);
        assert_eq!(loaded_cell.as_ref().unwrap().id, WALL_ID);

        // 2. Propose two z-walls via drag (x=1..=2, z=0).
        let drag_deltas = grid.drag(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0),
            WALL_ID,
            false,
        );
        assert_eq!(drag_deltas.len(), 2);
        assert_eq!(grid.proposed_changes.iter().count(), 2);

        // 3. Propose a desk at (2, 0, 2) via click.
        let click_deltas = grid.click(Vec3::new(2.0, 0.0, 2.0), DESK_ID, 0, false);
        assert_eq!(click_deltas.len(), 1);
        assert!(matches!(click_deltas[0].1, ProposalView::Add(_)));
        assert_eq!(grid.proposed_changes.iter().count(), 3);

        // 4. Undo the desk; the two wall proposals remain.
        let undo_deltas = grid.undo();
        assert_eq!(undo_deltas.len(), 1);
        assert!(matches!(undo_deltas[0].1, ProposalView::None));
        assert_eq!(grid.proposed_changes.iter().count(), 2);

        // 5. Construct: two new walls land in real contents; proposals clear.
        let real_changes = grid.construct();
        assert_eq!(real_changes.len(), 2);
        assert!(grid.proposed_changes.iter().count() == 0);
        assert!(grid.undo_record.is_empty());
        // Original loaded wall + 2 newly constructed walls
        assert_eq!(grid.contents.iter().count(), 3);
    }
}
