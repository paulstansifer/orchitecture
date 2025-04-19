use core::f32;

use godot::prelude::*;

struct MyExtension;

#[gdextension]
unsafe impl ExtensionLibrary for MyExtension {}

use godot::classes::{GridMap, INode3D, InputEvent, Node3D};

// `WallGrid` will be used to store walls, which are 1 unit long and infinitely thin, and are
// snapped to the coordinate grid. It uses one Godot `GridMap` per direction to store the models.
#[derive(GodotClass)]
#[class(base=Node3D)]
struct WallGrid {
    x_walls: OnReady<Gd<GridMap>>,
    z_walls: OnReady<Gd<GridMap>>,

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
    // The user has dragged bewteen `from` and `to`
    pub fn paint_wall(&mut self, from: Vector3, to: Vector3) {
        let x_diff = to.x - from.x;
        let z_diff = to.z - from.z;

        let x_drag = x_diff.abs() > z_diff;

        // TODO: use set_cell to actually paint stuff (need to add the MeshInstance3Ds to the
        // GridMaps first)
    }

    // GDScript doesn't have `Option<>`, so we define a flag value
    pub fn nowhere(&self) -> Vector3 {
        Vector3::new(f32::MIN, f32::MIN, f32::MIN)
    }

    pub fn mouse_to_3d_location(&self) -> Vector3 {
        let plane = Plane::new(Vector3::UP, 0.0);

        let mouse_pos = self.base().get_viewport().unwrap().get_mouse_position();
        let camera = self.view_camera.as_ref().unwrap();

        match plane.intersect_ray(
            camera.project_ray_origin(mouse_pos),
            camera.project_ray_normal(mouse_pos),
        ) {
            Some(pos) => pos,
            None => self.nowhere(),
        }
    }
}

#[godot_api]
impl INode3D for WallGrid {
    fn init(base: Base<Node3D>) -> Self {
        Self {
            x_walls: OnReady::new(|| GridMap::new_alloc()),
            z_walls: OnReady::new(|| GridMap::new_alloc()),
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
        if cur_pos == self.nowhere() {
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
