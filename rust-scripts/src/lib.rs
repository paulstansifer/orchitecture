use godot::prelude::*;

struct MyExtension;

#[gdextension]
unsafe impl ExtensionLibrary for MyExtension {}

mod mesh_management;
mod qnn;
mod qnn_translate;
mod serialization;
mod sparse3d;
mod structure;
mod wall_grid;

#[allow(dead_code)]
fn main() {
    qnn::train();
}
