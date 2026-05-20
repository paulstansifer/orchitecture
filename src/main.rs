use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use orchitecture_lib::{
    camera::{camera_input_system, spawn_camera, CameraState},
    input::{building_input_system, cursor_system, spawn_cursors, BuildState},
    structure::{spawn_structures, StructureList},
    ui::{discover_training_files, ui_system, UiState},
    visibility::update_visibility_system,
    wall_grid::spawn_grid,
    world::spawn_world,
};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            // Absolute path so assets load correctly regardless of working directory.
            file_path: env!("CARGO_MANIFEST_DIR").to_string(),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .insert_resource(CameraState::default())
        .insert_resource(BuildState::default())
        .insert_resource(UiState::default())
        .insert_resource(StructureList::default())
        .add_systems(
            Startup,
            (
                // spawn_structures must run before spawn_grid (grid reads StructureList).
                (spawn_structures, spawn_grid).chain(),
                spawn_camera,
                spawn_world,
                spawn_cursors,
                discover_training_files,
            ),
        )
        .add_systems(
            Update,
            (
                camera_input_system,
                building_input_system,
                cursor_system,
                update_visibility_system,
            ),
        )
        // ui_system must run in EguiPrimaryContextPass (not Update) to access Egui contexts.
        .add_systems(EguiPrimaryContextPass, ui_system)
        .run();
}
