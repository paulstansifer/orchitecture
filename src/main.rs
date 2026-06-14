#![recursion_limit = "256"]

use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use bevy_file_dialog::FileDialogPlugin;
use orchitecture_lib::{
    autotile::{autotile_update_system, load_autotile_handles, spawn_autotile_rules},
    camera::{camera_input_system, spawn_camera, CameraState},
    ceiling_lights::update_ceiling_lights,
    cutaway::{propagate_render_layers_system, update_cutaway_system, CutawayMode},
    grid_preview::GridPreviewPlugin,
    input::{
        building_input_system, cursor_system, recolor_new_mesh_children, spawn_cursors,
        update_room_cursor_mesh, BuildState,
    },
    qnn::ModelPlugin,
    station::spawn_initial_station,
    structure::{spawn_structures, StructureList},
    ui::{
        discover_user_files, enable_ui_input_absorption, handle_file_load, handle_file_save,
        ui_system, FurnitureRightClick, LoadDialog, SandboxMode, SaveDialog, UiState,
    },
    wall_grid::{
        spawn_grid, spawn_highlight_assets, spawn_material_assets, spawn_proposal_overlay_assets,
        update_station_highlight, StationHighlight, WallGrid,
    },
    world::spawn_world,
};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            // Absolute path so assets load correctly regardless of working directory.
            // On wasm, Bevy uses HTTP. ASSET_BASE_URL can be set at compile time to
            // a subpath prefix (e.g. "/orchitecture/") for GitHub Pages deployments;
            // defaults to "" for trunk serve / local builds.
            #[cfg(not(target_arch = "wasm32"))]
            file_path: orchitecture_lib::paths::MANIFEST_DIR.to_string(),
            #[cfg(target_arch = "wasm32")]
            file_path: option_env!("ASSET_BASE_URL").unwrap_or("").to_string(),
            // On wasm, Bevy fetches .meta files over HTTP and gets 404s it can't handle.
            meta_check: bevy::asset::AssetMetaCheck::Never,
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .add_plugins(
            FileDialogPlugin::new()
                .with_save_file::<SaveDialog>()
                .with_load_file::<LoadDialog>(),
        )
        .add_plugins(GridPreviewPlugin)
        .add_plugins(ModelPlugin)
        .insert_resource(CameraState::default())
        .insert_resource(BuildState::default())
        .insert_resource(UiState::default())
        .insert_resource(StructureList::default())
        .insert_resource(CutawayMode::default())
        .insert_resource(SandboxMode::default())
        .insert_resource(FurnitureRightClick::default())
        .insert_resource(StationHighlight::default())
        .add_systems(
            Startup,
            (
                enable_ui_input_absorption,
                // spawn_structures must run before spawn_grid (grid reads StructureList),
                // and spawn_initial_station after spawn_grid (it needs the WallGrid resource).
                (spawn_structures, spawn_grid, spawn_initial_station).chain(),
                spawn_autotile_rules,
                load_autotile_handles,
                spawn_camera,
                spawn_world,
                spawn_cursors,
                spawn_proposal_overlay_assets,
                spawn_material_assets,
                spawn_highlight_assets,
                discover_user_files,
            ),
        )
        .add_systems(
            Update,
            (
                camera_input_system,
                building_input_system,
                cursor_system,
                update_room_cursor_mesh,
                recolor_new_mesh_children,
                autotile_update_system.after(building_input_system),
                update_cutaway_system,
                propagate_render_layers_system.after(update_cutaway_system),
                update_ceiling_lights.run_if(resource_changed::<WallGrid>),
                update_station_highlight,
            ),
        )
        .add_systems(Update, (handle_file_save, handle_file_load))
        // ui_system must run in EguiPrimaryContextPass (not Update) to access Egui contexts.
        .add_systems(EguiPrimaryContextPass, ui_system)
        .run();
}
