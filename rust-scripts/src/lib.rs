use core::f32;
use std::f32::consts::TAU;

use godot::prelude::*;

struct MyExtension;

#[gdextension]
unsafe impl ExtensionLibrary for MyExtension {}

use godot::classes::{GridMap, INode3D, MeshLibrary, Node3D};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Dir {
    X,
    Y,
    Z,
}

impl Dir {
    fn basis(self) -> Basis {
        match self {
            Dir::X => Basis::IDENTITY,
            Dir::Y => Basis::IDENTITY.rotated(Vector3::FORWARD, TAU / 4.0),
            Dir::Z => Basis::IDENTITY.rotated(Vector3::UP, TAU / 4.0),
        }
    }
}

struct ParticularCell {
    pos: Vector3i,
    mesh_id: i32,
    orientation: i32,
}

struct UndoRecord {
    grid: Option<Dir>,
    changed: Vec<ParticularCell>,
}

// `WallGrid` will be used to store walls, which are 1 unit long and infinitely thin, and are
// snapped to the coordinate grid. It uses one Godot `GridMap` per direction to store the models.
#[derive(GodotClass)]
#[class(base=Node3D)]
struct WallGrid {
    #[export]
    x_walls: Option<Gd<GridMap>>,
    #[export]
    y_floors: Option<Gd<GridMap>>,
    #[export]
    z_walls: Option<Gd<GridMap>>,
    #[export]
    room: Option<Gd<GridMap>>,

    undo_record: Vec<UndoRecord>,

    base: Base<Node3D>,
}

#[godot_api]
impl WallGrid {
    fn gm(&self, dir: Dir) -> &GridMap {
        match dir {
            Dir::X => self.x_walls.as_ref().unwrap(),
            Dir::Y => self.y_floors.as_ref().unwrap(),
            Dir::Z => self.z_walls.as_ref().unwrap(),
        }
    }

    fn gm_mut(&mut self, dir: Dir) -> &mut GridMap {
        match dir {
            Dir::X => self.x_walls.as_mut().unwrap(),
            Dir::Y => self.y_floors.as_mut().unwrap(),
            Dir::Z => self.z_walls.as_mut().unwrap(),
        }
    }

    pub fn set_range_item_dir(
        &mut self,
        which_grid: Option<Dir>,
        dir: Dir,
        position1: Vector3i,
        position2: Vector3i,
        item: i32,
    ) {
        let orientation = self.gm(Dir::X).get_orthogonal_index_from_basis(dir.basis());

        let start_x = i32::min(position1.x, position2.x);
        let end_x = i32::max(position1.x, position2.x);
        let start_y = i32::min(position1.y, position2.y);
        let end_y = i32::max(position1.y, position2.y);
        let start_z = i32::min(position1.z, position2.z);
        let end_z = i32::max(position1.z, position2.z);

        let mut changed_cells: Vec<ParticularCell> = Vec::new();

        for x in start_x..=end_x {
            for y in start_y..=end_y {
                for z in start_z..=end_z {
                    let position = Vector3i::new(x, y, z);

                    let old_item = match which_grid {
                        Some(gd) => self.gm(gd),
                        None => self.room.as_ref().unwrap(),
                    }
                    .get_cell_item(position);
                    let old_orientation = match which_grid {
                        Some(gd) => self.gm(gd),
                        None => self.room.as_ref().unwrap(),
                    }
                    .get_cell_item_orientation(position);

                    changed_cells.push(ParticularCell {
                        pos: position,
                        mesh_id: old_item,
                        orientation: old_orientation,
                    });

                    match which_grid {
                        Some(gd) => self.gm_mut(gd),
                        None => self.room.as_mut().unwrap(),
                    }
                    .set_cell_item_ex(position, item)
                    .orientation(orientation)
                    .done();
                }
            }
        }

        if !changed_cells.is_empty() {
            self.undo_record.push(UndoRecord {
                grid: which_grid,
                changed: changed_cells,
            });
        }
    }

    #[func]
    pub fn set_mesh_library(&mut self, mesh_library: Gd<MeshLibrary>) {
        self.gm_mut(Dir::X).set_mesh_library(&mesh_library);
        self.gm_mut(Dir::Y).set_mesh_library(&mesh_library);
        self.gm_mut(Dir::Z).set_mesh_library(&mesh_library);
        self.room.as_mut().unwrap().set_mesh_library(&mesh_library);
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

        let end = if d == Dir::X {
            Vector3i::new(end.x, start.y, start.z)
        } else {
            Vector3i::new(start.x, start.y, end.z)
        };

        self.set_range_item_dir(Some(d), d, start, end, selected_mesh_id);
    }

    #[func]
    pub fn floor_drag(&mut self, from: Vector3, to: Vector3, selected_mesh_id: i32) {
        let from_i = from.round().cast_int();
        let to_i = to.round().cast_int();

        let start = Vector3i::coord_min(from_i, to_i);
        let end = Vector3i::coord_max(from_i, to_i) - Vector3i::new(1, 0, 1);

        self.set_range_item_dir(Some(Dir::Y), Dir::Z, start, end, selected_mesh_id);
    }

    #[func]
    // GDScript doesn't have `Option<>`, so we define a flag value
    pub fn nowhere() -> Vector3 {
        Vector3::new(f32::MIN, f32::MIN, f32::MIN)
    }

    #[func]
    pub fn room_plop(&mut self, location: Vector3, selected_mesh_id: i32) {
        let pos = location.round().cast_int();
        self.set_range_item_dir(None, Dir::Z, pos, pos, selected_mesh_id);
    }

    #[func]
    pub fn undo(&mut self) {
        if let Some(undo_record) = self.undo_record.pop() {
            let which_grid = undo_record.grid;
            for cell in undo_record.changed {
                match which_grid {
                    Some(gd) => self.gm_mut(gd),
                    None => self.room.as_mut().unwrap(),
                }
                .set_cell_item_ex(cell.pos, cell.mesh_id)
                .orientation(cell.orientation)
                .done();
            }
        }
    }
}

#[godot_api]
impl INode3D for WallGrid {
    fn init(base: Base<Node3D>) -> Self {
        Self {
            x_walls: None,
            y_floors: None,
            z_walls: None,
            room: None,

            undo_record: Vec::new(),

            base,
        }
    }
}
