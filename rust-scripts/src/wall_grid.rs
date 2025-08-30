use std::collections::HashMap;
use std::f32::consts::TAU;
use std::ops::DerefMut;

use godot::classes::file_access::ModeFlags;
use godot::prelude::*;

use godot::classes::{INode3D, MeshInstance3D, MeshLibrary, Node3D};
use serde::{Deserialize, Serialize};

use crate::serialization;
use crate::sparse3d::{RelSlot, SlotLocation, Sparse3D};
use crate::structure::{self, Structure};
use crate::{example_structures, qnn};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Dir {
    X,
    Y,
    Z,
}

struct ParticularCell {
    pos: Vector3i,
    slot: RelSlot,
    mi: Option<Cell>,
    replacer_mi: Option<Cell>,
}

struct UndoRecord {
    changed: Vec<ParticularCell>,
}

// HACK: we should be passed this information
const DESK_ID: i32 = 0;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct VantageEvaluation {
    pub symmetry: f32,
    pub interest: f32,
}

// Must manually set this up while assembling the scene
fn unset_mesh() -> Gd<MeshInstance3D> {
    MeshInstance3D::new_alloc()
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Cell {
    pub id: i32,
    #[serde(skip, default = "unset_mesh")]
    pub mesh: Gd<MeshInstance3D>,
    pub evaluation: Option<VantageEvaluation>,
}

// Safe to use outside of Godot
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct OfflineCell {
    pub id: i32,
    pub evaluation: Option<VantageEvaluation>,
}

// `WallGrid` will be used to store walls, which are 1 unit long and infinitely thin, and are
// snapped to the coordinate grid.
#[derive(GodotClass)]
#[class(base=Node3D)]
pub struct WallGrid {
    structures: Vec<Structure>,

    mesh_library: Gd<MeshLibrary>,
    contents: Sparse3D<Cell>,
    container: Gd<Node3D>,
    temp_container: Gd<Node3D>,

    undo_record: Vec<UndoRecord>,

    base: Base<Node3D>,
}

// Fixup to get models into the right spot (necessary, since walls can be X or Y).
fn slot_transform(slot: RelSlot) -> Transform3D {
    let xform = Transform3D::IDENTITY.rotated(Vector3::RIGHT, -TAU / 4.0);
    match slot {
        RelSlot::Room => xform.rotated(Vector3::UP, -TAU / 4.0),
        RelSlot::XLoWall | RelSlot::XHiWall => xform.rotated(Vector3::UP, -TAU / 4.0),
        RelSlot::Floor | RelSlot::Ceiling => xform.rotated(Vector3::UP, -TAU / 4.0),
        RelSlot::ZLoWall | RelSlot::ZHiWall => xform,
    }
}

#[godot_api]
impl WallGrid {
    #[allow(dead_code)]
    pub fn to_offline(&self) -> Sparse3D<OfflineCell> {
        let mut offline_grid = Sparse3D::new();
        for (loc, cell) in self.contents.iter() {
            offline_grid.set(
                loc,
                OfflineCell {
                    id: cell.id,
                    evaluation: cell.evaluation.clone(),
                },
            );
        }
        offline_grid
    }

    pub fn from_offline(&mut self, offline_grid: Sparse3D<OfflineCell>) {
        self.contents = Sparse3D::new();
        for (loc, offline_cell) in offline_grid.iter() {
            let mut mesh_instance: Gd<MeshInstance3D> = MeshInstance3D::new_alloc();
            mesh_instance.set_mesh(&self.mesh_library.get_item_mesh(offline_cell.id).unwrap());

            self.contents.set(
                loc,
                Cell {
                    id: offline_cell.id,
                    mesh: mesh_instance,
                    evaluation: offline_cell.evaluation.clone(),
                },
            );
        }

        self.container.propagate_call("queue_free");
        let container = Node3D::new_alloc();
        self.base_mut().add_child(&container);
        self.container = container;

        for (loc, cell) in self.contents.iter_mut() {
            cell.mesh
                .set_transform(slot_transform(loc.rel_slot).translated(loc.cube.cast_float()));
            self.container.add_child(&cell.mesh);
        }
    }

    #[func]
    pub fn get_structures(&self) -> Array<GString> {
        let mut res = Array::new();

        for structure in &self.structures {
            let mut name = structure.info.main_mesh.clone();
            if let Some(dot_index) = name.find('.') {
                name.truncate(dot_index);
            }
            res.push(&name);
        }

        res
    }

    pub fn set_range_item_dir(
        &mut self,
        _dir: Dir,
        position1: Vector3i,
        position2: Vector3i,
        slot: RelSlot,
        item: Option<i32>,
    ) {
        let start_x = i32::min(position1.x, position2.x);
        let end_x = i32::max(position1.x, position2.x);
        let start_y = i32::min(position1.y, position2.y);
        let end_y = i32::max(position1.y, position2.y);
        let start_z = i32::min(position1.z, position2.z);
        let end_z = i32::max(position1.z, position2.z);

        let mut changed_cells: Vec<ParticularCell> = Vec::new();

        let container: &mut Node3D = self.container.deref_mut();

        for x in start_x..=end_x {
            for y in start_y..=end_y {
                for z in start_z..=end_z {
                    let position = Vector3i::new(x, y, z);
                    let loc = SlotLocation::new(x, y, z, slot);

                    let old_cell = self.contents.take(loc);

                    match old_cell {
                        Some(ref old_cell) => {
                            container.remove_child(&old_cell.mesh);
                        }
                        None => {}
                    }

                    if let Some(item) = item {
                        let mut mesh_instance: Gd<MeshInstance3D> = MeshInstance3D::new_alloc();
                        mesh_instance.set_mesh(&self.mesh_library.get_item_mesh(item).unwrap());

                        mesh_instance
                            .set_transform(slot_transform(slot).translated(position.cast_float()));

                        container.add_child(&mesh_instance);

                        let new_cell = Cell {
                            id: item,
                            mesh: mesh_instance.clone(),
                            evaluation: None,
                        };

                        changed_cells.push(ParticularCell {
                            pos: position,
                            slot,
                            mi: old_cell,
                            replacer_mi: Some(new_cell.clone()),
                        });

                        self.contents.set(loc, new_cell);
                    } else {
                        changed_cells.push(ParticularCell {
                            pos: position,
                            slot,
                            mi: old_cell,
                            replacer_mi: None,
                        });
                        self.contents.take(loc);
                    }
                }
            }
        }

        if !changed_cells.is_empty() {
            self.undo_record.push(UndoRecord {
                changed: changed_cells,
            });
        }
    }

    #[func]
    pub fn click(&mut self, position: Vector3, selected_mesh_id: i32, remove: bool) {
        match self.structures[selected_mesh_id as usize]
            .info
            .placement_style
        {
            structure::PlacementStyle::RoomPlop => {
                self.room_plop(position, (!remove).then_some(selected_mesh_id))
            }
            _ => {}
        }
    }

    #[func]
    pub fn drag(&mut self, from: Vector3, to: Vector3, selected_mesh_id: i32, remove: bool) {
        let id = (!remove).then_some(selected_mesh_id);
        match self.structures[selected_mesh_id as usize]
            .info
            .placement_style
        {
            structure::PlacementStyle::WallDrag => self.wall_drag(from, to, id),
            structure::PlacementStyle::FloorDrag => self.floor_drag(from, to, id),
            _ => {}
        }
    }

    // The user has dragged something wall-like bewteen `from` and `to`
    pub fn wall_drag(&mut self, from: Vector3, to: Vector3, selected_mesh_id: Option<i32>) {
        let x_diff = to.x - from.x;
        let z_diff = to.z - from.z;

        let d = if x_diff.abs() > z_diff.abs() {
            Dir::X
        } else {
            Dir::Z
        };

        let from_i = from.round().cast_int();
        let mut to_i = from_i;
        if d == Dir::X {
            to_i.x = to.x.round() as i32;
        } else {
            to_i.z = to.z.round() as i32;
        }

        let start = Vector3i::coord_min(from_i, to_i);
        let end = Vector3i::coord_max(from_i, to_i);

        let end = end
            - if d == Dir::X {
                Vector3i::new(1, 0, 0)
            } else {
                Vector3i::new(0, 0, 1)
            };

        let slot = if d == Dir::X {
            RelSlot::ZLoWall
        } else {
            RelSlot::XLoWall
        };

        self.set_range_item_dir(d, start, end, slot, selected_mesh_id);
    }

    pub fn floor_drag(&mut self, from: Vector3, to: Vector3, selected_mesh_id: Option<i32>) {
        let from_i = from.round().cast_int();
        let to_i = to.round().cast_int();

        let start = Vector3i::coord_min(from_i, to_i);
        let end = Vector3i::coord_max(from_i, to_i) - Vector3i::new(1, 0, 1);

        self.set_range_item_dir(Dir::Y, start, end, RelSlot::Floor, selected_mesh_id);
    }

    pub fn room_plop(&mut self, location: Vector3, selected_mesh_id: Option<i32>) {
        let pos = location.round().cast_int();
        self.set_range_item_dir(Dir::Z, pos, pos, RelSlot::Room, selected_mesh_id);
        if selected_mesh_id == Some(DESK_ID) {
            let loc = SlotLocation::new(pos.x, pos.y, pos.z, RelSlot::Room);
            self.contents.get_mut(loc).unwrap().evaluation = Some(VantageEvaluation {
                symmetry: 0.5,
                interest: 0.5,
            });
        }
    }

    #[func]
    pub fn save(&self, filename: GString) {
        let mut structures_by_id = HashMap::new();
        for (id, structure) in self.structures.iter().enumerate() {
            structures_by_id.insert(id as i32, structure.info.clone());
        }

        let serialized = serialization::serialize_sparse3d(
            &self.contents,
            |cell, slot, structures| serialization::serialize_slot(cell.id, slot, structures),
            serialization::cell_needs_extended,
            &structures_by_id,
        );
        let path = GString::from(format!("training/{filename}"));

        let mut file = GFile::open(&path, ModeFlags::WRITE).unwrap();
        file.write_gstring(&serialized).unwrap();
        godot_print!("Saved to {}", file.path_absolute());
    }

    #[func]
    pub fn load(&mut self, filename: GString) {
        if let Ok(idx) = filename.to_string().parse::<usize>() {
            let new_map = example_structures::make_structures().remove(idx);
            self.from_offline(new_map);
            return;
        }

        let path = GString::from(format!("training/{filename}"));

        let mut file = GFile::open(&path, ModeFlags::READ).unwrap();
        let serialized = file.read_as_gstring_entire(false).unwrap().to_string();

        let mut structures_by_char = HashMap::new();
        for (id, structure) in self.structures.iter().enumerate() {
            if let Some(x_char) = structure.info.x_char {
                structures_by_char.insert(x_char, id as i32);
            }
            if let Some(z_char) = structure.info.z_char {
                structures_by_char.insert(z_char, id as i32);
            }
        }

        // TODO: a lot of this duplicates `from_offline`; use that instead.
        self.contents = serialization::deserialize_sparse3d(
            &serialized,
            |c, _slot, structures_by_char| {
                let id = serialization::deserialize(c, structures_by_char);
                let mut mesh_instance: Gd<MeshInstance3D> = MeshInstance3D::new_alloc();
                mesh_instance.set_mesh(&self.mesh_library.get_item_mesh(id).unwrap());

                Ok::<Cell, ()>(Cell {
                    id,
                    mesh: mesh_instance,
                    evaluation: None,
                })
            },
            &structures_by_char,
        )
        .unwrap();

        self.container.propagate_call("queue_free");
        let container = Node3D::new_alloc();
        self.base_mut().add_child(&container);
        self.container = container;

        for (loc, cell) in self.contents.iter_mut() {
            cell.mesh
                .set_transform(slot_transform(loc.rel_slot).translated(loc.cube.cast_float()));
            self.container.add_child(&cell.mesh);
        }
        godot_print!("Loaded from {}", file.path_absolute());
    }

    #[func]
    pub fn undo(&mut self) {
        if let Some(undo_record) = self.undo_record.pop() {
            for mut cell in undo_record.changed {
                let position = cell.pos;
                let slot = cell.slot;

                if let Some(ref mut new_cell) = cell.replacer_mi {
                    self.container.remove_child(&new_cell.mesh);
                    new_cell.mesh.queue_free();
                }

                if let Some(ref old_cell) = cell.mi {
                    let loc = SlotLocation::new(position.x, position.y, position.z, slot);
                    self.contents.set(loc, old_cell.clone());
                } else {
                    let loc = SlotLocation::new(position.x, position.y, position.z, slot);
                    self.contents.take(loc);
                }
            }
        }
    }

    #[func]
    pub fn dont_actually_call_me() {
        if false {
            // Otherwise, we get warnings for things not used in the library.
            qnn::train::<burn::backend::NdArray>();
        }
    }

    #[func]
    pub fn update_visibility(&mut self, focus_location: Vector3, camera_location: Vector3) {
        // Clear the old cut walls:
        self.temp_container.propagate_call("queue_free");
        let new_temp_container = Node3D::new_alloc();
        self.base_mut().add_child(&new_temp_container);
        self.temp_container = new_temp_container;

        let view_direction = (focus_location - camera_location).sign().round().cast_int();
        let effective_focus_location = focus_location.round().cast_int()
            + Vector3i::new(view_direction.x * 2, 0, view_direction.z * 2);

        let last_y_layer = Vector3i::new(view_direction.x, 0, view_direction.z);
        for (loc, cell) in self.contents.iter_mut() {
            if (effective_focus_location - loc.cube).sign() == view_direction {
                cell.mesh.hide();
            } else if (effective_focus_location - loc.cube).sign() == last_y_layer {
                cell.mesh.hide();

                let mut cut_instance: Gd<MeshInstance3D> = MeshInstance3D::new_alloc();

                cut_instance.set_transform(cell.mesh.get_transform());
                cut_instance.set_mesh(&self.mesh_library.get_item_mesh(cell.id + 1000).unwrap());

                self.temp_container.add_child(&cut_instance);
            } else {
                cell.mesh.show();
            }
        }
    }

    #[func]
    fn get_ready_to_quit(&mut self) {
        self.structures.clear();

        for (_, cell) in self.contents.iter_mut() {
            cell.mesh.queue_free();
        }
        self.container.queue_free();
        self.temp_container.queue_free();
    }
}

#[godot_api]
impl INode3D for WallGrid {
    fn init(base: Base<Node3D>) -> Self {
        let container = Node3D::new_alloc();
        base.to_gd().add_child(&container);
        let temp_container = Node3D::new_alloc();
        base.to_gd().add_child(&temp_container);

        let structures = structure::load_structures();

        let mut mesh_library = MeshLibrary::new_gd();

        for (id, structure) in structures.iter().enumerate() {
            mesh_library.create_item(id as i32);
            mesh_library.create_item(id as i32 + 1000);
            mesh_library.set_item_mesh(id as i32, &structure.mesh);
            if let Some(ref cut_mesh) = structure.y_cut_mesh {
                // HACK; add 1000 for the cutaway versions:
                mesh_library.set_item_mesh(id as i32 + 1000, cut_mesh);
            } else {
                // HACK: instead of doing a lookup, just always replace the mesh (but sometimes
                // with the same mesh).
                mesh_library.set_item_mesh(id as i32 + 1000, &structure.mesh);
            }
        }

        Self {
            structures,
            undo_record: Vec::new(),
            mesh_library,
            contents: Sparse3D::new(),
            container: container,
            temp_container: temp_container,

            base,
        }
    }
}
