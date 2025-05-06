use std::f32::consts::TAU;
use std::ops::DerefMut;

use godot::classes::file_access::ModeFlags;
use godot::prelude::*;

struct MyExtension;

#[gdextension]
unsafe impl ExtensionLibrary for MyExtension {}

use godot::classes::{INode3D, MeshInstance3D, MeshLibrary, Node3D};
mod qnn;
mod qnn_translate;
mod serialization;
mod sparse3d;
mod structure;

use serde::{Deserialize, Serialize};
use serialization::{deserialize, deserialize_sparse3d, serialize_slot, serialize_sparse3d};
use sparse3d::{Slot, Sparse3D};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Dir {
    X,
    Y,
    Z,
}

struct ParticularCell {
    pos: Vector3i,
    slot: Slot,
    mi: Option<Cell>,
    replacer_mi: Option<Cell>,
}

struct UndoRecord {
    changed: Vec<ParticularCell>,
}

// HACK: we should be passed this information
const DESK_ID: i32 = 0;
const DOORWAY_ID: i32 = 1;
const _FLOOR_ID: i32 = 2;
const WALL_ID: i32 = 3;
const _RAILING_ID: i32 = 4;
const CUT_WALL_ID: i32 = 5;
const CUT_DOORWAY_ID: i32 = 6;
const STAIRS_ID: i32 = 7;
const CUT_STAIRS_ID: i32 = 8;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct VantageEvaluation {
    symmetry: f32,
    interest: f32,
}

// Must manually set this up while assembling the scene
fn unset_mesh() -> Gd<MeshInstance3D> {
    MeshInstance3D::new_alloc()
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
struct Cell {
    id: i32,
    #[serde(skip, default = "unset_mesh")]
    mesh: Gd<MeshInstance3D>,
    evaluation: Option<VantageEvaluation>,
}

// Safe to use outside of Godot
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
struct OfflineCell {
    id: i32,
    evaluation: Option<VantageEvaluation>,
}

// `WallGrid` will be used to store walls, which are 1 unit long and infinitely thin, and are
// snapped to the coordinate grid. It uses one Godot `GridMap` per direction to store the models.
#[derive(GodotClass)]
#[class(base=Node3D)]
struct WallGrid {
    mesh_library: Option<Gd<MeshLibrary>>,
    contents: Sparse3D<Cell>,
    container: Gd<Node3D>,
    temp_container: Gd<Node3D>,

    undo_record: Vec<UndoRecord>,

    base: Base<Node3D>,
}

// `Room` allows multiple orientations, but all other slots only allow a single orientation.
fn slot_transform(slot: Slot) -> Transform3D {
    let xform = Transform3D::IDENTITY
        .rotated(Vector3::RIGHT, -TAU / 4.0)
        .rotated(Vector3::UP, TAU / 2.0);
    (match slot {
        Slot::Room => xform,
        Slot::XWall => xform,
        Slot::YFloor => xform.rotated(Vector3::UP, TAU / -4.0),
        Slot::ZWall => xform.rotated(Vector3::UP, -TAU / 4.0),
    })
    .translated(Vector3::new(1.0, 0.0, 1.0))
}

#[godot_api]
impl WallGrid {
    #[func]
    pub fn set_mesh_library(&mut self, mesh_library: Gd<MeshLibrary>) {
        self.mesh_library = Some(mesh_library);
    }

    pub fn set_range_item_dir(
        &mut self,
        _dir: Dir,
        position1: Vector3i,
        position2: Vector3i,
        slot: Slot,
        item: i32,
    ) {
        let start_x = i32::min(position1.x, position2.x);
        let end_x = i32::max(position1.x, position2.x);
        let start_y = i32::min(position1.y, position2.y);
        let end_y = i32::max(position1.y, position2.y);
        let start_z = i32::min(position1.z, position2.z);
        let end_z = i32::max(position1.z, position2.z);

        let mut changed_cells: Vec<ParticularCell> = Vec::new();

        let container: &mut Node3D = self.container.deref_mut();
        let mesh_library = self.mesh_library.as_ref().unwrap();

        for x in start_x..=end_x {
            for y in start_y..=end_y {
                for z in start_z..=end_z {
                    let position = Vector3i::new(x, y, z);

                    let old_cell = self.contents.take(position, slot);

                    match old_cell {
                        Some(ref old_cell) => {
                            container.remove_child(&old_cell.mesh);
                        }
                        None => {}
                    }

                    if item == -1 {
                        changed_cells.push(ParticularCell {
                            pos: position,
                            slot,
                            mi: old_cell,
                            replacer_mi: None,
                        });
                        self.contents.take(position, slot);
                    } else {
                        let mut mesh_instance: Gd<MeshInstance3D> = MeshInstance3D::new_alloc();
                        mesh_instance.set_mesh(&mesh_library.get_item_mesh(item).unwrap());

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

                        self.contents.set(position, slot, new_cell);
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
    // The user has dragged something wall-like bewteen `from` and `to`
    pub fn wall_drag(&mut self, from: Vector3, to: Vector3, selected_mesh_id: i32) {
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

        let start = Vector3i::coord_min(from_i, to_i) - Vector3i::new(1, 0, 1);
        let end = Vector3i::coord_max(from_i, to_i) - Vector3i::new(1, 0, 1);

        let start = start
            + if d == Dir::X {
                Vector3i::new(1, 0, 0)
            } else {
                Vector3i::new(0, 0, 1)
            };

        let slot = if d == Dir::X {
            Slot::XWall
        } else {
            Slot::ZWall
        };

        self.set_range_item_dir(d, start, end, slot, selected_mesh_id);
    }

    #[func]
    pub fn floor_drag(&mut self, from: Vector3, to: Vector3, selected_mesh_id: i32) {
        let from_i = from.round().cast_int();
        let to_i = to.round().cast_int();

        let start = Vector3i::coord_min(from_i, to_i);
        let end = Vector3i::coord_max(from_i, to_i) - Vector3i::new(1, 0, 1);

        self.set_range_item_dir(Dir::Y, start, end, Slot::YFloor, selected_mesh_id);
    }

    #[func]
    pub fn room_plop(&mut self, location: Vector3, selected_mesh_id: i32) {
        let pos = location.round().cast_int() - Vector3i::new(1, 0, 1);
        self.set_range_item_dir(Dir::Z, pos, pos, Slot::Room, selected_mesh_id);
        if selected_mesh_id == DESK_ID {
            self.contents.get_mut(pos, Slot::Room).unwrap().evaluation = Some(VantageEvaluation {
                symmetry: 0.5,
                interest: 0.5,
            });
        }
    }

    #[func]
    pub fn save(&self, filename: GString) {
        let serialized = serialize_sparse3d(
            &self.contents,
            |cell, slot| serialize_slot(cell.id, slot),
            serialization::cell_needs_extended,
        );
        let mut file = GFile::open(&filename, ModeFlags::WRITE).unwrap();
        file.write_gstring(&serialized).unwrap();
        godot_print!("Saved to {}", file.path_absolute());
    }

    #[func]
    pub fn load(&mut self, filename: GString) {
        let mut file = GFile::open(&filename, ModeFlags::READ).unwrap();
        let serialized = file.read_as_gstring_entire(false).unwrap().to_string();

        self.contents = deserialize_sparse3d(&serialized, |c, _slot| {
            let id = deserialize(c);
            let mesh_library = self.mesh_library.as_ref().unwrap();
            let mut mesh_instance: Gd<MeshInstance3D> = MeshInstance3D::new_alloc();
            mesh_instance.set_mesh(&mesh_library.get_item_mesh(id).unwrap());

            Ok::<Cell, ()>(Cell {
                id,
                mesh: mesh_instance,
                evaluation: None,
            })
        })
        .unwrap();

        self.container.propagate_call("queue_free");
        let container = Node3D::new_alloc();
        self.base_mut().add_child(&container);
        self.container = container;

        for (pos, slot, cell) in self.contents.iter_mut() {
            cell.mesh
                .set_transform(slot_transform(slot).translated(pos.cast_float()));
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
                    self.container.add_child(&old_cell.mesh);
                    self.contents.set(position, slot, old_cell.clone());
                } else {
                    self.contents.take(position, slot);
                }
            }
        }
    }

    #[func]
    pub fn dont_actually_call_me() {
        if false {
            qnn::train(); // Otherwise, we get warnings for things not used in the library.
        }
    }

    #[func]
    pub fn update_visibility(&mut self, focus_location: Vector3, camera_location: Vector3) {
        let cut_wall = self
            .mesh_library
            .as_ref()
            .unwrap()
            .get_item_mesh(CUT_WALL_ID)
            .unwrap();
        let cut_doorway = self
            .mesh_library
            .as_ref()
            .unwrap()
            .get_item_mesh(CUT_DOORWAY_ID)
            .unwrap();
        let cut_stairs = self
            .mesh_library
            .as_ref()
            .unwrap()
            .get_item_mesh(CUT_STAIRS_ID)
            .unwrap();

        // Clear the old cut walls:
        self.temp_container.propagate_call("queue_free");
        let new_temp_container = Node3D::new_alloc();
        self.base_mut().add_child(&new_temp_container);
        self.temp_container = new_temp_container;

        let view_direction = (focus_location - camera_location).sign().round().cast_int();
        let effective_focus_location = focus_location.round().cast_int()
            + Vector3i::new(view_direction.x * 2, 0, view_direction.z * 2);

        let last_y_layer = Vector3i::new(view_direction.x, 0, view_direction.z);
        for (pos, _slot, cell) in self.contents.iter_mut() {
            if (effective_focus_location - pos).sign() == view_direction {
                cell.mesh.hide();
            } else if (cell.id == WALL_ID || cell.id == DOORWAY_ID || cell.id == STAIRS_ID)
                && (effective_focus_location - pos).sign() == last_y_layer
            {
                cell.mesh.hide();

                let mut cut_instance: Gd<MeshInstance3D> = MeshInstance3D::new_alloc();

                cut_instance.set_transform(cell.mesh.get_transform());
                if cell.id == DOORWAY_ID {
                    cut_instance.set_mesh(&cut_doorway);
                } else if cell.id == STAIRS_ID {
                    cut_instance.set_mesh(&cut_stairs);
                } else {
                    cut_instance.set_mesh(&cut_wall);
                }
                self.temp_container.add_child(&cut_instance);
            } else {
                cell.mesh.show();
            }
        }
    }
}

#[godot_api]
impl INode3D for WallGrid {
    fn init(base: Base<Node3D>) -> Self {
        let container = Node3D::new_alloc();
        base.to_gd().add_child(&container);
        let temp_container = Node3D::new_alloc();
        base.to_gd().add_child(&temp_container);

        Self {
            undo_record: Vec::new(),
            mesh_library: None,
            contents: Sparse3D::new(),
            container: container,
            temp_container: temp_container,

            base,
        }
    }
}

#[allow(dead_code)]
fn main() {
    qnn::train();
}
