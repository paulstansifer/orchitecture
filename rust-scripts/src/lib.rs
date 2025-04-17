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

    #[export]
    cursor: Option<Gd<Node3D>>,

    #[export]
    view_camera: Option<Gd<Camera3D>>,

    base: Base<Node3D>,
}

#[godot_api]
impl INode3D for WallGrid {
    fn init(base: Base<Node3D>) -> Self {
        godot_print!("Hello, WallGrid!"); // Prints to the Godot console

        Self {
            x_walls: OnReady::new(|| GridMap::new_alloc()),
            z_walls: OnReady::new(|| GridMap::new_alloc()),
            cursor: None,
            view_camera: None,
            base,
        }
    }

    fn input(&mut self, _event: Gd<InputEvent>) {
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

        let snapped_pos = world_position.round();

        self.cursor.as_mut().unwrap().set_position(snapped_pos);
    }
}
