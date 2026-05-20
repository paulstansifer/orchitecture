use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use rust_scripts::{
    camera::{camera_input_system, CameraState, GameCamera},
    input::{building_input_system, cursor_system, BuildState, RoomCursorMarker, WallCursorMarker},
    mesh_management::load_mesh_handles,
    structure::{load_structure_info, Structure, StructureList},
    ui::{discover_training_files, ui_system, UiState},
    wall_grid::{
        compute_visibility, cell_transform, CutCellMarker, GridCellMarker, WallGrid,
    },
};

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins.set(AssetPlugin {
                // Absolute path so assets load correctly regardless of working directory.
                file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/..").to_string(),
                ..default()
            }),
        )
        .add_plugins(EguiPlugin::default())
        .insert_resource(CameraState::default())
        .insert_resource(BuildState::default())
        .insert_resource(UiState::default())
        .insert_resource(StructureList::default())
        .add_systems(
            Startup,
            (setup, discover_training_files),
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
        .add_systems(EguiPrimaryContextPass, ui_system)
        .run();
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut structure_list: ResMut<StructureList>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Load structure infos (compile-time via include_str!) and mesh handles.
    let infos = load_structure_info();
    let mesh_handles = load_mesh_handles(&asset_server);

    for info in &infos {
        let mesh_handle = mesh_handles
            .get(&info.main_mesh)
            .unwrap_or_else(|| panic!("Missing mesh: {}", info.main_mesh))
            .clone();
        let cut_handle = info
            .y_cut_mesh
            .as_ref()
            .and_then(|n| mesh_handles.get(n).cloned());
        structure_list
            .structures
            .push(Structure { info: info.clone(), mesh_handle, cut_handle });
    }

    // Build WallGrid resource.
    commands.insert_resource(WallGrid::new(infos));

    // Camera.
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 15.0, 25.0).looking_at(Vec3::ZERO, Vec3::Y),
        GameCamera,
    ));

    // Sun.
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, 0.4, 0.0)),
    ));

    // Ground plane.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(100.0, 100.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.5, 0.3),
            ..default()
        })),
        Transform::default(),
    ));

    // Build cursors.
    let cursor_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.8, 1.0),
        unlit: true,
        ..default()
    });

    // Wall/floor cursor: tall pin (cylinder + sphere on top).
    commands
        .spawn((Transform::default(), Visibility::Hidden, WallCursorMarker))
        .with_children(|p| {
            p.spawn((
                Mesh3d(meshes.add(Cylinder::new(0.04, 0.5))),
                MeshMaterial3d(cursor_mat.clone()),
                Transform::from_xyz(0.0, 0.5, 0.0),
            ));
            p.spawn((
                Mesh3d(meshes.add(Sphere::new(0.12))),
                MeshMaterial3d(cursor_mat.clone()),
                Transform::from_xyz(0.0, 1.12, 0.0),
            ));
        });

    // Room cursor: flat disc.
    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(0.45, 0.025))),
        MeshMaterial3d(cursor_mat),
        Transform::default(),
        Visibility::Hidden,
        RoomCursorMarker,
    ));
}

fn update_visibility_system(
    mut commands: Commands,
    mut wall_grid: ResMut<WallGrid>,
    structure_list: Res<StructureList>,
    build_state: Res<BuildState>,
    camera_q: Query<&GlobalTransform, With<GameCamera>>,
    mut vis_q: Query<(Entity, &GridCellMarker, &mut Visibility)>,
    cut_q: Query<Entity, With<CutCellMarker>>,
) {
    let Ok(cam_gt) = camera_q.single() else {
        return;
    };
    let camera_pos = cam_gt.translation();
    let focus_pos = build_state.focus_pos;

    // Despawn old cut entities.
    for entity in cut_q.iter() {
        commands.entity(entity).despawn();
    }
    wall_grid.cut_entities.clear();

    let (hidden_locs, cut_entries) =
        compute_visibility(&wall_grid.contents, focus_pos, camera_pos);
    let hidden_set: std::collections::HashSet<_> = hidden_locs.into_iter().collect();

    // Show/hide grid cell entities.
    for (entity, marker, mut vis) in vis_q.iter_mut() {
        *vis = if hidden_set.contains(&marker.loc) {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        let _ = entity;
    }

    // Spawn cut-y entities.
    for (loc, id) in cut_entries {
        if let Some(cut_handle) = structure_list.cut_handle(id) {
            let transform = cell_transform(loc.rel_slot, rust_scripts::sparse3d::Facing::NegX, loc.cube);
            let entity = commands
                .spawn((SceneRoot(cut_handle.clone()), transform, CutCellMarker))
                .id();
            wall_grid.cut_entities.push(entity);
        }
    }
}
