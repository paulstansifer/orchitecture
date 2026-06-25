#![recursion_limit = "256"]

use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use bevy_file_dialog::FileDialogPlugin;
use orchitecture_lib::{
    autotile::{autotile_update_system, load_autotile_handles, spawn_autotile_rules},
    build_ui::{
        build_ui_system, discover_user_files, enable_ui_input_absorption, handle_file_load,
        handle_file_save, FurnitureRightClick, LoadDialog, SandboxMode, SaveDialog, UiState,
    },
    camera::{camera_input_system, spawn_camera, CameraState, GameCamera},
    ceiling_lights::{update_ceiling_lights, update_window_lights},
    cutaway::{propagate_render_layers_system, update_cutaway_system, CutawayMode},
    game_mode::GameMode,
    grid_preview::GridPreviewPlugin,
    input::{
        building_input_system, cursor_system, recolor_new_mesh_children, spawn_cursors,
        update_room_cursor_mesh, BuildState, CursorEntities,
    },
    orc::{despawn_orc, orc_input_system, setup_orc_animation, spawn_orc},
    ortho_camera::{walk_camera_system, WalkCameraState},
    qnn::ModelPlugin,
    station::spawn_initial_station,
    structure::{spawn_structures, StructureList},
    surroundings::{
        enter_surroundings_mode, exit_surroundings_mode, generate_farms, surroundings_ui_system,
        GameClock,
    },
    walk_input::walk_input_system,
    walk_ui::walk_ui_system,
    wall_grid::{
        spawn_grid, spawn_highlight_assets, spawn_material_assets, spawn_proposal_overlay_assets,
        update_station_highlight, StationHighlight, WallGrid,
    },
    world::{apply_lighting_mode_system, lighting_input_system, spawn_world, LightingMode},
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
        .init_state::<GameMode>()
        .insert_resource(CameraState::default())
        .insert_resource(WalkCameraState::default())
        .insert_resource(BuildState::default())
        .insert_resource(UiState::default())
        .insert_resource(StructureList::default())
        .insert_resource(CutawayMode::default())
        .insert_resource(SandboxMode::default())
        .insert_resource(FurnitureRightClick::default())
        .insert_resource(StationHighlight::default())
        .insert_resource(GameClock::default())
        .insert_resource(LightingMode::default())
        .add_systems(OnEnter(GameMode::Walk), (enter_walk_mode, spawn_orc))
        .add_systems(OnExit(GameMode::Walk), despawn_orc)
        .add_systems(OnEnter(GameMode::Build), enter_build_mode)
        .add_systems(OnEnter(GameMode::Surroundings), enter_surroundings_mode)
        .add_systems(OnExit(GameMode::Surroundings), exit_surroundings_mode)
        .add_systems(
            Startup,
            (
                enable_ui_input_absorption,
                // spawn_structures must run before spawn_grid (grid reads StructureList),
                // and spawn_initial_station after spawn_grid (it needs the WallGrid resource).
                (spawn_structures, spawn_grid, spawn_initial_station).chain(),
                spawn_autotile_rules,
                load_autotile_handles,
                generate_farms,
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
                camera_input_system.run_if(in_state(GameMode::Build)),
                building_input_system.run_if(in_state(GameMode::Build)),
                cursor_system.run_if(in_state(GameMode::Build)),
                update_room_cursor_mesh.run_if(in_state(GameMode::Build)),
                walk_input_system.run_if(in_state(GameMode::Walk)),
                walk_camera_system.run_if(in_state(GameMode::Walk)),
                setup_orc_animation.run_if(in_state(GameMode::Walk)),
                orc_input_system.run_if(in_state(GameMode::Walk)),
                recolor_new_mesh_children,
                autotile_update_system.after(building_input_system),
                update_cutaway_system,
                propagate_render_layers_system.after(update_cutaway_system),
                update_ceiling_lights.run_if(resource_changed::<WallGrid>),
                update_window_lights.run_if(resource_changed::<WallGrid>),
                lighting_input_system,
                apply_lighting_mode_system.after(lighting_input_system),
                update_station_highlight,
            ),
        )
        .add_systems(Update, (handle_file_save, handle_file_load))
        .add_systems(
            EguiPrimaryContextPass,
            (
                build_ui_system.run_if(in_state(GameMode::Build)),
                walk_ui_system.run_if(in_state(GameMode::Walk)),
                surroundings_ui_system.run_if(in_state(GameMode::Surroundings)),
            ),
        )
        .run();
}

fn enter_walk_mode(
    camera_state: Res<CameraState>,
    mut walk_state: ResMut<WalkCameraState>,
    cursor_entities: Res<CursorEntities>,
    mut visibility_q: Query<&mut Visibility>,
    mut camera_q: Query<&mut Msaa, With<GameCamera>>,
) {
    walk_state.target_position = camera_state.target_position;
    walk_state.camera_direction = 0;
    walk_state.current_display_angle = 0.0;
    walk_state.requested_direction = 0.0;
    walk_state.is_right_dragging = false;
    walk_state.drag_delta = Vec2::ZERO;

    if let Ok(mut msaa) = camera_q.single_mut() {
        *msaa = Msaa::Off;
    }

    for entity in [
        cursor_entities.wall,
        cursor_entities.room,
        cursor_entities.preview,
    ] {
        if let Ok(mut vis) = visibility_q.get_mut(entity) {
            *vis = Visibility::Hidden;
        }
    }
}

fn enter_build_mode(
    walk_state: Res<WalkCameraState>,
    mut camera_state: ResMut<CameraState>,
    mut camera_q: Query<(&mut Projection, &mut Msaa), With<GameCamera>>,
) {
    camera_state.target_position = walk_state.target_position;
    if let Ok((mut projection, mut msaa)) = camera_q.single_mut() {
        *projection = Projection::Perspective(PerspectiveProjection::default());
        *msaa = Msaa::default();
    }
}
