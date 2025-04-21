use core::f32;
use std::f32::consts::TAU;

use godot::prelude::*;

struct MyExtension;

#[gdextension]
unsafe impl ExtensionLibrary for MyExtension {}

use godot::classes::{GridMap, INode3D, InputEvent, MeshLibrary, Node3D};

#[derive(PartialEq, Eq, Clone, Copy)]
enum Dir {
    X,
    Z,
}

impl Dir {
    fn basis(self) -> Basis {
        match self {
            Dir::X => Basis::IDENTITY,
            Dir::Z => Basis::IDENTITY.rotated(Vector3::UP, TAU / 4.0),
        }
    }
}

// `WallGrid` will be used to store walls, which are 1 unit long and infinitely thin, and are
// snapped to the coordinate grid. It uses one Godot `GridMap` per direction to store the models.
#[derive(GodotClass)]
#[class(base=Node3D)]
struct WallGrid {
    #[export]
    x_walls: Option<Gd<GridMap>>,
    #[export]
    z_walls: Option<Gd<GridMap>>,

    drag_start: Option<Vector3>,

    #[export]
    cursor: Option<Gd<Node3D>>,
    #[export]
    drag_from_cursor: Option<Gd<Node3D>>,

    #[export]
    drag_helper: Option<Gd<Node3D>>,

    #[export]
    view_camera: Option<Gd<Camera3D>>,

    base: Base<Node3D>,
}

#[godot_api]
impl WallGrid {
    fn gm(&self, dir: Dir) -> &GridMap {
        match dir {
            Dir::X => self.x_walls.as_ref().unwrap(),
            Dir::Z => self.z_walls.as_ref().unwrap(),
        }
    }

    fn gm_mut(&mut self, dir: Dir) -> &mut GridMap {
        match dir {
            Dir::X => self.x_walls.as_mut().unwrap(),
            Dir::Z => self.z_walls.as_mut().unwrap(),
        }
    }

    #[func]
    pub fn set_mesh_library(&mut self, mesh_library: Gd<MeshLibrary>) {
        self.gm_mut(Dir::X).set_mesh_library(&mesh_library);
        self.gm_mut(Dir::Z).set_mesh_library(&mesh_library);
    }

    #[func]
    // The user has dragged bewteen `from` and `to`
    pub fn paint_wall(&mut self, from: Vector3, to: Vector3) {
        let x_diff = to.x - from.x;
        let z_diff = to.z - from.z;

        let d = if x_diff.abs() > z_diff.abs() {
            Dir::X
        } else {
            Dir::Z
        };

        let start = from.round().cast_int();
        let end = to.round().cast_int();

        let orientation = self.gm(Dir::X).get_orthogonal_index_from_basis(d.basis());

        if d == Dir::X {
            for x in i32::min(start.x, end.x)..i32::max(start.x, end.x) {
                self.gm_mut(Dir::X)
                    .set_cell_item_ex(Vector3i::new(x, start.y, start.z - 1), 0)
                    .orientation(orientation)
                    .done();
            }
        } else {
            for z in i32::min(start.z, end.z)..i32::max(start.z, end.z) {
                self.gm_mut(Dir::Z)
                    .set_cell_item_ex(Vector3i::new(start.x - 1, start.y, z), 0)
                    .orientation(orientation)
                    .done();
            }
        }
    }

    #[func]
    // GDScript doesn't have `Option<>`, so we define a flag value
    pub fn nowhere() -> Vector3 {
        Vector3::new(f32::MIN, f32::MIN, f32::MIN)
    }

    // deprecated; do this sort of stuff in GDScript
    pub fn mouse_to_3d_location(&self) -> Vector3 {
        let plane = Plane::new(Vector3::UP, 0.0);

        let mouse_pos = self.base().get_viewport().unwrap().get_mouse_position();
        let camera = self.view_camera.as_ref().unwrap();

        match plane.intersect_ray(
            camera.project_ray_origin(mouse_pos),
            camera.project_ray_normal(mouse_pos),
        ) {
            Some(pos) => pos,
            None => Self::nowhere(),
        }
    }
}

#[godot_api]
impl INode3D for WallGrid {
    fn init(base: Base<Node3D>) -> Self {
        Self {
            x_walls: None,
            z_walls: None,
            drag_start: None,

            // Exports
            cursor: None,
            drag_from_cursor: None,
            drag_helper: None,
            view_camera: None,
            base,
        }
    }

    // This is in change of displaying the helper graphics (later, this should be moved to GDScript)
    fn input(&mut self, event: Gd<InputEvent>) {
        let cur_pos = self.mouse_to_3d_location();
        if cur_pos == Self::nowhere() {
            return;
        }
        let cur_pos_snapped = cur_pos.round();

        self.cursor.as_mut().unwrap().set_position(cur_pos_snapped);

        if event.is_action_pressed("build") {
            self.drag_start = Some(cur_pos_snapped);
            self.drag_from_cursor
                .as_mut()
                .unwrap()
                .set_position(cur_pos_snapped);
            self.drag_from_cursor.as_mut().unwrap().show();
        }

        if event.is_action_released("build") {
            self.drag_from_cursor.as_mut().unwrap().hide();
        }
    }
}
