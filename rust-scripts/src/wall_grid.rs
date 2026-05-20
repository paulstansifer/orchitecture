use std::collections::HashMap;
use std::f32::consts::TAU;

use bevy::math::{IVec3, Quat, Vec3};
use bevy::prelude::{Commands, Component, Entity, Resource, SceneRoot, Transform};
use serde::{Deserialize, Serialize};

use crate::sparse3d::{Facing, RelSlot, SlotLocation, Sparse3D};
use crate::structure::{PlacementStyle, StructureInfo, StructureList};
use crate::serialization;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Dir {  // TODO: this should probably be replaced with `Facing`
    X,
    Z,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct VantageEvaluation {
    pub coherence: f32,
    pub interest: f32,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Cell {
    pub id: i32,
    #[serde(default)]
    pub facing: Facing,
    pub evaluation: Option<VantageEvaluation>,
}

impl crate::sparse3d::Rotateable for Cell {
    fn rotate(self, rotation: crate::sparse3d::Rotation) -> Self {
        let mut new = self.clone();
        new.facing = new.facing.rotate(rotation);
        new
    }
}

// Compatibility alias used by build_helpers, example_structures, qnn_translate
// TODO: remove
pub type OfflineCell = Cell;

const DESK_ID: i32 = 0;

struct UndoRecord {
    // (location, what_was_there_before)
    changed: Vec<(SlotLocation, Option<Cell>)>,
}

/// Marker component for entities that represent placed grid cells.
#[derive(Component)]
pub struct GridCellMarker {
    pub loc: SlotLocation,
}

/// Marker component for y-cut visibility variant entities.
#[derive(Component)]
pub struct CutCellMarker;

#[derive(Resource)]
pub struct WallGrid {
    pub structures: Vec<StructureInfo>,
    pub contents: Sparse3D<Cell>,
    /// Entity spawned for each placed cell.
    pub cell_entities: HashMap<SlotLocation, Entity>,
    /// Entities spawned for the y-cut visibility layer (cleared each visibility update).
    pub cut_entities: Vec<Entity>,
    undo_record: Vec<UndoRecord>,
}

impl WallGrid {
    pub fn new(structures: Vec<StructureInfo>) -> Self {
        WallGrid {
            structures,
            contents: Sparse3D::new(),
            cell_entities: HashMap::new(),
            cut_entities: Vec::new(),
            undo_record: Vec::new(),
        }
    }

    pub fn get_structure_names(&self) -> Vec<String> {
        self.structures
            .iter()
            .map(|s| {
                let mut name = s.main_mesh.clone();
                if let Some(dot) = name.find('.') {
                    name.truncate(dot);
                }
                name
            })
            .collect()
    }

    pub fn structure_is_room_plop(&self, id: i32) -> bool {
        self.structures[id as usize].placement_style == PlacementStyle::RoomPlop
    }

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
        let x_diff = to.x - from.x;
        let z_diff = to.z - from.z;
        let d = if x_diff.abs() > z_diff.abs() { Dir::X } else { Dir::Z };

        let from_i = from.round().as_ivec3();
        let mut to_i = from_i;
        if d == Dir::X {
            to_i.x = to.x.round() as i32;
        } else {
            to_i.z = to.z.round() as i32;
        }

        let start = from_i.min(to_i);
        let end = from_i.max(to_i)
            - if d == Dir::X {
                IVec3::new(1, 0, 0)
            } else {
                IVec3::new(0, 0, 1)
            };
        let slot = if d == Dir::X { RelSlot::ZLoWall } else { RelSlot::XLoWall };

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
        if selected_mesh_id == Some(DESK_ID) {
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

    pub fn save(&self, path: &std::path::PathBuf) {
        let mut structures_by_id = HashMap::new();
        for (id, info) in self.structures.iter().enumerate() {
            structures_by_id.insert(id as i32, info.clone());
        }
        let serialized = serialization::serialize_sparse3d(
            &self.contents,
            |cell, slot, structures| serialization::serialize_slot(cell.id, slot, structures),
            &structures_by_id,
        );
        std::fs::write(path, serialized).unwrap();
    }

    /// Load from a text file. Returns full replacement deltas (clear all, set all new).
    pub fn load(&mut self, path: &std::path::PathBuf) -> Vec<(SlotLocation, Option<Cell>)> {
        let serialized = std::fs::read_to_string(path).unwrap();
        let mut structures_by_char = HashMap::new();
        for (id, info) in self.structures.iter().enumerate() {
            if let Some(c) = info.x_char {
                structures_by_char.insert(c, id as i32);
            }
            if let Some(c) = info.z_char {
                structures_by_char.insert(c, id as i32);
            }
        }

        let new_contents = serialization::deserialize_sparse3d(
            &serialized,
            |c, _slot, map| {
                let id = serialization::deserialize(c, map);
                Ok::<Cell, ()>(Cell { id, facing: Facing::NegX, evaluation: None })
            },
            &structures_by_char,
        )
        .unwrap();

        self.replace_contents(new_contents)
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

/// Computes the Bevy Transform for a cell at the given grid position.
pub fn cell_transform(slot: RelSlot, facing: Facing, cube: IVec3) -> Transform {
    let rx = Quat::from_rotation_x(-TAU / 4.0);
    let ry_neg90 = Quat::from_rotation_y(-TAU / 4.0);

    let rotation = match slot {
        RelSlot::Room => {
            let facing_angle = (1.0 - facing as u8 as f32) * (-TAU / 4.0);
            Quat::from_rotation_y(-TAU / 4.0 + facing_angle) * rx
        }
        RelSlot::XLoWall | RelSlot::XHiWall | RelSlot::Floor | RelSlot::Ceiling => ry_neg90 * rx,
        RelSlot::ZLoWall | RelSlot::ZHiWall => rx,
    };

    Transform {
        translation: cube.as_vec3(),
        rotation,
        scale: Vec3::ONE,
    }
}

/// Applies a list of cell changes to the world: despawns old entities, spawns new ones.
pub fn apply_changes(
    commands: &mut Commands,
    wall_grid: &mut WallGrid,
    structure_list: &StructureList,
    changes: Vec<(SlotLocation, Option<Cell>)>,
) {
    for (loc, new_cell) in changes {
        if let Some(old_entity) = wall_grid.cell_entities.remove(&loc) {
            commands.entity(old_entity).despawn();
        }
        if let Some(cell) = new_cell {
            let transform = cell_transform(loc.rel_slot, cell.facing, loc.cube);
            let handle = structure_list.scene_handle(cell.id).clone();
            let entity = commands
                .spawn((SceneRoot(handle), transform, GridCellMarker { loc }))
                .id();
            wall_grid.cell_entities.insert(loc, entity);
        }
    }
}

/// Startup system: creates the WallGrid resource from the already-populated StructureList.
pub fn spawn_grid(mut commands: Commands, structure_list: bevy::prelude::Res<StructureList>) {
    let infos = structure_list.structures.iter().map(|s| s.info.clone()).collect();
    commands.insert_resource(WallGrid::new(infos));
}
