use godot::prelude::*;

struct MyExtension;

#[gdextension]
unsafe impl ExtensionLibrary for MyExtension {}

use godot::classes::{GridMap, INode3D, InputEvent, Node3D};

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
    pub fn paint_region(&mut self, from: Vector3, to: Vector3) {
        let x_diff = to.x - from.x;
        let z_diff = to.z - from.z;

        let x_drag = x_diff.abs() > z_diff;

        // TODO: use set_cell to actually pain stuff (need to add the MeshInstance3Ds to the
        // GridMaps first)
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

    fn input(&mut self, event: Gd<InputEvent>) {
        let plane = Plane::new(Vector3::UP, 0.0);

        let mouse_pos = self.base().get_viewport().unwrap().get_mouse_position();
        let camera = self.view_camera.as_ref().unwrap();
        let world_position = match plane.intersect_ray(
            camera.project_ray_origin(mouse_pos),
            camera.project_ray_normal(mouse_pos),
        ) {
            Some(pos) => pos,
            None => return,
        };

        let cur_pos_snapped = world_position.round();

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
