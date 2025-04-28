use std::f32::consts::TAU;
use std::ops::DerefMut;

use godot::prelude::*;

struct MyExtension;

#[gdextension]
unsafe impl ExtensionLibrary for MyExtension {}

use godot::classes::{INode3D, MeshInstance3D, MeshLibrary, Node3D};
mod sparse3d;

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
    mi: Option<Gd<MeshInstance3D>>,
    replacer_mi: Option<Gd<MeshInstance3D>>,
}

struct UndoRecord {
    changed: Vec<ParticularCell>,
}

// `WallGrid` will be used to store walls, which are 1 unit long and infinitely thin, and are
// snapped to the coordinate grid. It uses one Godot `GridMap` per direction to store the models.
#[derive(GodotClass)]
#[class(base=Node3D)]
struct WallGrid {
    mesh_library: Option<Gd<MeshLibrary>>,
    contents: Sparse3D<Gd<MeshInstance3D>>,
    container: Gd<Node3D>,

    undo_record: Vec<UndoRecord>,

    base: Base<Node3D>,
}

#[godot_api]
impl WallGrid {
    #[func]
    pub fn set_mesh_library(&mut self, mesh_library: Gd<MeshLibrary>) {
        self.mesh_library = Some(mesh_library);
    }

    pub fn set_range_item_dir(
        &mut self,
        dir: Dir,
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

                    let old_mesh = self.contents.take(position, slot);

                    match old_mesh {
                        Some(ref old_mesh) => {
                            container.remove_child(old_mesh);
                        }
                        None => {}
                    }

                    let mut mesh_instance: Gd<MeshInstance3D> = MeshInstance3D::new_alloc();
                    mesh_instance.set_mesh(&mesh_library.get_item_mesh(item).unwrap());
                    let xform = Transform3D::IDENTITY
                        .rotated(Vector3::RIGHT, -TAU / 4.0)
                        .rotated(Vector3::UP, TAU / 2.0);
                    let xform = match dir {
                        Dir::X => xform,
                        Dir::Y => xform.rotated(Vector3::UP, TAU / -4.0), //xform.rotated(Vector3::FORWARD, TAU / 4.0),
                        Dir::Z => xform.rotated(Vector3::UP, TAU / -4.0),
                    };

                    let xform =
                        xform.translated(position.cast_float() + Vector3::new(1.0, 0.0, 1.0));
                    mesh_instance.set_transform(xform);

                    container.add_child(&mesh_instance);

                    changed_cells.push(ParticularCell {
                        pos: position,
                        slot,
                        mi: old_mesh,
                        replacer_mi: Some(mesh_instance.clone()),
                    });

                    self.contents.set(position, slot, mesh_instance);
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
        let to_i = to.round().cast_int();

        let start = Vector3i::coord_min(from_i, to_i) - Vector3i::new(1, 0, 1);
        let end = Vector3i::coord_max(from_i, to_i) - Vector3i::new(1, 0, 1);

        let start = start
            + if d == Dir::X {
                Vector3i::new(1, 0, 0)
            } else {
                Vector3i::new(0, 0, 1)
            };

        if d == Dir::X {
            let end = Vector3i::new(end.x, start.y, start.z);
            self.set_range_item_dir(d, start, end, Slot::XWall, selected_mesh_id);
        } else {
            let end = Vector3i::new(start.x, start.y, end.z);
            self.set_range_item_dir(d, start, end, Slot::ZWall, selected_mesh_id);
        }
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
        let pos = location.round().cast_int();
        self.set_range_item_dir(Dir::Z, pos, pos, Slot::Room, selected_mesh_id);
    }

    #[func]
    pub fn undo(&mut self) {
        if let Some(undo_record) = self.undo_record.pop() {
            for cell in undo_record.changed {
                let position = cell.pos;
                let slot = cell.slot;

                if let Some(ref new_mesh) = cell.replacer_mi {
                    self.container.remove_child(new_mesh);
                }

                if let Some(ref mesh) = cell.mi {
                    self.container.add_child(mesh);
                    self.contents.set(position, slot, mesh.clone());
                } else {
                    self.contents.take(position, slot);
                }
            }
        }
    }

    #[func]
    pub fn update_visibility(&mut self, focus_location: Vector3, camera_location: Vector3) {
        let view_direction = (focus_location.cast_int() - camera_location.cast_int()).sign();
        for (pos, _slot, mesh_instance) in self.contents.iter_mut() {
            // let diff = focus_location - pos.cast_float();
            // let dir = (focus_location - camera_location).sign();
            // let check = Vector3::new(diff.x * dir.x, diff.y * dir.y, diff.z * dir.z);

            //if check.x > 0.0 && check.y > 0.0 && check.z > 0.0 {
            if (focus_location.cast_int() - pos).sign() == view_direction {
                mesh_instance.hide();
            } else {
                mesh_instance.show();
            }
        }
    }
}

#[godot_api]
impl INode3D for WallGrid {
    fn init(base: Base<Node3D>) -> Self {
        let container = Node3D::new_alloc();
        base.to_gd().add_child(&container);
        Self {
            undo_record: Vec::new(),
            mesh_library: None,
            contents: Sparse3D::new(),
            container: container,

            base,
        }
    }
}
