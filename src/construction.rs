use bevy::math::{IVec3, Vec3};

use crate::sparse3d::{Facing, RelSlot, SlotLocation, Sparse3D};
use crate::structure::PlacementStyle;
use crate::wall_grid::{Cell, UndoRecord, VantageEvaluation, WallGrid};

impl WallGrid {
    /// Place or clear cells in a rectangular range.
    /// Returns `(loc, new_cell)` deltas — `None` means the cell was removed.
    fn set_range_item_dir(
        &mut self,
        dir: i32,
        position1: IVec3,
        position2: IVec3,
        slot: RelSlot,
        item: Option<i32>,
    ) -> Vec<(SlotLocation, Option<Cell>)> {
        let start = position1.min(position2);
        let end = position1.max(position2);
        let mut changes: Vec<(SlotLocation, Option<Cell>)> = Vec::new();
        let mut undo_changed: Vec<(SlotLocation, Option<Cell>)> = Vec::new();

        for x in start.x..=end.x {
            for y in start.y..=end.y {
                for z in start.z..=end.z {
                    let loc = SlotLocation::new(x, y, z, slot);
                    let old_cell = self.contents.take(loc);
                    undo_changed.push((loc, old_cell));

                    if let Some(id) = item {
                        let facing = Facing::from_number(dir as u8);
                        let new_cell = Cell { id, facing, evaluation: None };
                        self.contents.set(loc, new_cell.clone());
                        changes.push((loc, Some(new_cell)));
                    } else {
                        changes.push((loc, None));
                    }
                }
            }
        }

        if !changes.is_empty() {
            self.undo_record.push(UndoRecord { changed: undo_changed });
        }
        changes
    }

    pub fn wall_drag(
        &mut self,
        from: Vec3,
        to: Vec3,
        selected_mesh_id: Option<i32>,
    ) -> Vec<(SlotLocation, Option<Cell>)> {
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
            - if along_x { IVec3::new(1, 0, 0) } else { IVec3::new(0, 0, 1) };
        let slot = if along_x { RelSlot::ZLoWall } else { RelSlot::XLoWall };

        self.set_range_item_dir(0, start, end, slot, selected_mesh_id)
    }

    pub fn floor_drag(
        &mut self,
        from: Vec3,
        to: Vec3,
        selected_mesh_id: Option<i32>,
    ) -> Vec<(SlotLocation, Option<Cell>)> {
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
        selected_mesh_id: Option<i32>,
    ) -> Vec<(SlotLocation, Option<Cell>)> {
        let pos = location.round().as_ivec3();
        let changes = self.set_range_item_dir(dir, pos, pos, RelSlot::Room, selected_mesh_id);
        if selected_mesh_id == Some(0) {
            // Desk's ID number. TODO: fix this!
            let loc = SlotLocation::new(pos.x, pos.y, pos.z, RelSlot::Room);
            if let Some(cell) = self.contents.get_mut(loc) {
                cell.evaluation = Some(VantageEvaluation { coherence: 0.5, interest: 0.5 });
            }
        }
        changes
    }

    pub fn drag(
        &mut self,
        from: Vec3,
        to: Vec3,
        selected_mesh_id: i32,
        remove: bool,
    ) -> Vec<(SlotLocation, Option<Cell>)> {
        let id = (!remove).then_some(selected_mesh_id);
        match self.structures[selected_mesh_id as usize].placement_style {
            PlacementStyle::WallDrag => self.wall_drag(from, to, id),
            PlacementStyle::FloorDrag => self.floor_drag(from, to, id),
            _ => vec![],
        }
    }

    pub fn click(
        &mut self,
        position: Vec3,
        selected_mesh_id: i32,
        dir: i32,
        remove: bool,
    ) -> Vec<(SlotLocation, Option<Cell>)> {
        match self.structures[selected_mesh_id as usize].placement_style {
            PlacementStyle::RoomPlop => {
                self.room_plop(position, dir, (!remove).then_some(selected_mesh_id))
            }
            _ => vec![],
        }
    }

    /// Undo last action. Returns `(loc, cell_to_restore)` deltas — `None` means delete.
    pub fn undo(&mut self) -> Vec<(SlotLocation, Option<Cell>)> {
        let Some(record) = self.undo_record.pop() else {
            return vec![];
        };
        for (loc, old_cell) in &record.changed {
            if let Some(cell) = old_cell {
                self.contents.set(*loc, cell.clone());
            } else {
                self.contents.take(*loc);
            }
        }
        record.changed
    }

    pub fn load_from_offline(
        &mut self,
        new_contents: Sparse3D<Cell>,
    ) -> Vec<(SlotLocation, Option<Cell>)> {
        self.replace_contents(new_contents)
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
        self.undo_record.clear();
        changes
    }
}
