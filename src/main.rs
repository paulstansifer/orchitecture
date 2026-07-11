#![recursion_limit = "256"]

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy_egui::egui::{FontDefinitions, FontFamily};
use bevy_egui::{
    EguiContexts, EguiMultipassSchedule, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext,
};
use bevy_file_dialog::FileDialogPlugin;
use orchitecture_lib::{
    autotile::{autotile_update_system, load_autotile_handles, spawn_autotile_rules},
    build_ui::{build_ui_system, enable_ui_input_absorption, FurnitureRightClick, UiState},
    camera::{
        camera_input_system, disable_auto_egui_primary_context, spawn_camera, CameraState,
        GameCamera,
    },
    city::{
        spawn_grid, spawn_material_assets, spawn_proposal_overlay_assets, ConstructedCity,
        PlaceHighlight,
    },
    cutaway::{
        propagate_render_layers_system, sync_cutaway_shadow_material, update_cutaway_system,
        CutawayMode,
    },
    eorf::{spawn_structures, EorfList},
    game_mode::{GameMode, SandboxMode},
    gi_material::GiPlugin,
    global_illumination::update_global_illumination,
    grid_preview::GridPreviewPlugin,
    input::{
        building_input_system, cursor_system, recolor_new_mesh_children, spawn_cursors,
        update_room_cursor_mesh, BuildState, CursorEntities,
    },
    map_files::{discover_user_files, handle_file_load, handle_file_save, LoadDialog, SaveDialog},
    materials::MaterialList,
    month::{advance_month_system, AdvanceMonthRequested},
    orc::{
        despawn_orc, draw_debug_nav_gizmos, orc_click_to_move_system, orc_input_system,
        setup_orc_animation, spawn_orc, DebugNav,
    },
    ortho_camera::{
        resize_pixel_canvas_system, spawn_pixel_canvas, walk_camera_system, PixelCanvas,
        WalkCameraState,
    },
    pathing::rebuild_navigation_grid,
    place::{spawn_initial_places, sync_places_system},
    population::{spawn_population, sync_assignments, Population},
    qnn::ModelPlugin,
    resource_icons::spawn_resource_icons,
    rng::GameRng,
    scene::spawn_scene,
    selection::{
        animate_selection_rings, spawn_ring_mesh_assets, update_hover_highlight,
        update_selection_ring, HoveredFurniture, RingMaterial,
    },
    surroundings::{
        enter_surroundings_mode, exit_surroundings_mode, generate_farms, surroundings_ui_system,
        GameClock,
    },
    traveler::setup_travelers,
    ui::shared_ui_system,
    walk_input::walk_input_system,
    walk_ui::walk_ui_system,
};

fn configure_fonts_once(mut contexts: EguiContexts, mut done: Local<bool>) {
    if *done {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };

    let mut fonts = FontDefinitions::default();
    fonts
        .families
        .get_mut(&FontFamily::Proportional)
        .unwrap()
        .push("Hack".to_owned());
    ctx.set_fonts(fonts);

    *done = true;
}

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
        .add_plugins(GiPlugin)
        .add_plugins(ModelPlugin)
        .add_plugins(MaterialPlugin::<RingMaterial>::default())
        .init_state::<GameMode>()
        .insert_resource(CameraState::default())
        .insert_resource(WalkCameraState::default())
        .insert_resource(BuildState::default())
        .insert_resource(UiState::default())
        .insert_resource(EorfList::default())
        .insert_resource(CutawayMode::default())
        .insert_resource(SandboxMode::default())
        .insert_resource(FurnitureRightClick::default())
        .insert_resource(PlaceHighlight::default())
        .insert_resource(HoveredFurniture::default())
        .insert_resource(GameClock::default())
        .insert_resource(MaterialList::load())
        // The game's single source of randomness, seeded from OS entropy so each
        // graphical session is unique. Must exist before `generate_farms` runs.
        .insert_resource(GameRng::from_entropy())
        .insert_resource(DebugNav::default())
        .add_message::<AdvanceMonthRequested>()
        .add_systems(OnEnter(GameMode::Walk), (enter_walk_mode, spawn_orc))
        .add_systems(OnExit(GameMode::Walk), despawn_orc)
        .add_systems(OnEnter(GameMode::Build), enter_build_mode)
        .add_systems(OnEnter(GameMode::Surroundings), enter_surroundings_mode)
        .add_systems(OnExit(GameMode::Surroundings), exit_surroundings_mode)
        .add_systems(
            Startup,
            (
                enable_ui_input_absorption,
                // Must run before any camera is spawned, so the automatic Egui setup
                // system never gets a chance to race with `spawn_camera` for the context.
                disable_auto_egui_primary_context,
                // spawn_structures must run before spawn_grid (grid reads EorfList),
                // and spawn_initial_places after spawn_grid (it needs the WallGrid resource).
                (spawn_structures, spawn_grid, spawn_initial_places).chain(),
                spawn_autotile_rules,
                load_autotile_handles,
                generate_farms,
                spawn_population,
                setup_travelers,
                spawn_camera,
                spawn_pixel_canvas,
                spawn_scene,
                spawn_cursors,
                spawn_proposal_overlay_assets,
                spawn_material_assets,
                spawn_ring_mesh_assets,
                discover_user_files,
                spawn_resource_icons,
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
                orc_click_to_move_system.run_if(in_state(GameMode::Walk)),
                orc_input_system
                    .run_if(in_state(GameMode::Walk))
                    .after(orc_click_to_move_system),
                draw_debug_nav_gizmos.run_if(in_state(GameMode::Walk)),
                resize_pixel_canvas_system,
                recolor_new_mesh_children,
                autotile_update_system.after(building_input_system),
                update_cutaway_system.after(autotile_update_system),
                propagate_render_layers_system.after(update_cutaway_system),
                sync_cutaway_shadow_material.after(update_cutaway_system),
                update_global_illumination.run_if(resource_changed::<ConstructedCity>),
                (
                    sync_places_system.run_if(resource_changed::<ConstructedCity>),
                    rebuild_navigation_grid.run_if(resource_changed::<ConstructedCity>),
                    sync_assignments.run_if(
                        resource_changed::<ConstructedCity>.or(resource_changed::<Population>),
                    ),
                )
                    .chain(),
                (
                    update_selection_ring,
                    animate_selection_rings,
                    update_hover_highlight.run_if(in_state(GameMode::Build)),
                ),
            ),
        )
        .add_systems(Update, (handle_file_save, handle_file_load))
        .add_systems(Update, advance_month_system)
        .add_systems(
            EguiPrimaryContextPass,
            (
                configure_fonts_once,
                shared_ui_system,
                build_ui_system.run_if(in_state(GameMode::Build)),
                walk_ui_system.run_if(in_state(GameMode::Walk)),
                surroundings_ui_system.run_if(in_state(GameMode::Surroundings)),
            ),
        )
        .run();
}

fn enter_walk_mode(
    mut commands: Commands,
    camera_state: Res<CameraState>,
    mut walk_state: ResMut<WalkCameraState>,
    cursor_entities: Res<CursorEntities>,
    pixel_canvas: Option<Res<PixelCanvas>>,
    mut visibility_q: Query<&mut Visibility>,
    mut camera_q: Query<&mut Camera>,
    game_camera_q: Query<Entity, With<GameCamera>>,
    mut msaa_q: Query<&mut Msaa, With<GameCamera>>,
    mut target_q: Query<&mut RenderTarget, With<GameCamera>>,
) {
    walk_state.target_position = camera_state.target_position;
    walk_state.camera_direction = 0;
    walk_state.current_display_angle = 0.0;
    walk_state.requested_direction = 0.0;
    walk_state.is_right_dragging = false;
    walk_state.drag_delta = Vec2::ZERO;

    if let Ok(mut msaa) = msaa_q.single_mut() {
        *msaa = Msaa::Off;
    }
    // Render GameCamera to the low-resolution canvas instead of the window, and turn
    // on the camera/sprite that upscale that canvas back onto the window.
    // `pixel_canvas` doesn't exist yet the very first time this runs, since the initial
    // state's OnEnter fires before Startup's `spawn_pixel_canvas`.
    if let Some(pixel_canvas) = pixel_canvas {
        if let Ok(mut target) = target_q.single_mut() {
            *target = RenderTarget::Image(pixel_canvas.image.clone().into());
        }
        if let Ok(mut cam) = camera_q.get_mut(pixel_canvas.camera) {
            cam.is_active = true;
        }
        if let Ok(mut vis) = visibility_q.get_mut(pixel_canvas.sprite) {
            *vis = Visibility::Visible;
        }
        // WebGL can only have one *active* camera targeting the real window at a
        // time, so the Egui context has to move from GameCamera (now off-screen) to
        // the pixel-canvas camera (now the one facing the window) rather than living
        // on a separate always-on UI camera.
        // Also strip `EguiMultipassSchedule`: bevy_egui's insert hook on `PrimaryEguiContext`
        // adds it automatically, but removing the marker doesn't remove it, so it'd be left
        // stale on this camera and collide with the one about to be added to the other camera
        // (bevy_egui panics if two entities share the same multi-pass schedule).
        if let Ok(game_camera) = game_camera_q.single() {
            commands
                .entity(game_camera)
                .remove::<(PrimaryEguiContext, EguiMultipassSchedule)>();
        }
        commands
            .entity(pixel_canvas.camera)
            .insert(PrimaryEguiContext);
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
    mut commands: Commands,
    walk_state: Res<WalkCameraState>,
    mut camera_state: ResMut<CameraState>,
    pixel_canvas: Option<Res<PixelCanvas>>,
    mut visibility_q: Query<&mut Visibility>,
    mut camera_q: Query<&mut Camera>,
    game_camera_q: Query<Entity, With<GameCamera>>,
    mut projection_q: Query<(&mut Projection, &mut Msaa), With<GameCamera>>,
    mut target_q: Query<&mut RenderTarget, With<GameCamera>>,
) {
    camera_state.target_position = walk_state.target_position;
    // Render GameCamera directly to the window again, and turn off the pixel canvas.
    // `pixel_canvas` doesn't exist yet the very first time this runs, since the initial
    // state's OnEnter fires before Startup's `spawn_pixel_canvas`.
    if let Some(pixel_canvas) = pixel_canvas {
        if let Ok(mut target) = target_q.single_mut() {
            *target = RenderTarget::default();
        }
        if let Ok(mut cam) = camera_q.get_mut(pixel_canvas.camera) {
            cam.is_active = false;
        }
        if let Ok(mut vis) = visibility_q.get_mut(pixel_canvas.sprite) {
            *vis = Visibility::Hidden;
        }
        // Hand the Egui context back to GameCamera now that it's the one facing the
        // window again (see the matching comment in `enter_walk_mode`).
        // See the matching comment in `enter_walk_mode` for why `EguiMultipassSchedule`
        // needs to be stripped here too.
        commands
            .entity(pixel_canvas.camera)
            .remove::<(PrimaryEguiContext, EguiMultipassSchedule)>();
        if let Ok(game_camera) = game_camera_q.single() {
            commands.entity(game_camera).insert(PrimaryEguiContext);
        }
    }
    if let Ok((mut projection, mut msaa)) = projection_q.single_mut() {
        *projection = Projection::Perspective(PerspectiveProjection::default());
        *msaa = Msaa::default();
    }
}
