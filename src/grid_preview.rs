use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::window::PrimaryWindow;

use crate::camera::GameCamera;
use crate::input::{cursor_world_pos, BuildState};

#[derive(Component)]
pub struct GridPreviewMarker;

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct GridPreviewMaterial {
    #[uniform(0)]
    pub cursor_xz: Vec4, // xy = cursor world x,z
}

impl Material for GridPreviewMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/grid_preview.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

pub struct GridPreviewPlugin;

impl Plugin for GridPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<GridPreviewMaterial>::default())
            .add_systems(Startup, spawn_grid_preview)
            .add_systems(Update, update_grid_preview);
    }
}

fn spawn_grid_preview(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GridPreviewMaterial>>,
) {
    let mat = materials.add(GridPreviewMaterial {
        cursor_xz: Vec4::ZERO,
    });
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(16.0, 16.0))),
        MeshMaterial3d(mat),
        Transform::default(),
        Visibility::Hidden,
        GridPreviewMarker,
        NotShadowCaster,
    ));
}

fn update_grid_preview(
    build_state: Res<BuildState>,
    mut grid_q: Query<
        (&mut Transform, &mut Visibility, &MeshMaterial3d<GridPreviewMaterial>),
        With<GridPreviewMarker>,
    >,
    mut materials: ResMut<Assets<GridPreviewMaterial>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
) {
    let y = build_state.cur_y as f32 + 0.005;
    let cursor_pos = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32);

    let Ok((mut transform, mut vis, mat_handle)) = grid_q.single_mut() else {
        return;
    };

    match cursor_pos {
        Some(pos) => {
            transform.translation = Vec3::new(pos.x, y, pos.z);
            *vis = Visibility::Inherited;
            if let Some(mat) = materials.get_mut(&mat_handle.0) {
                mat.cursor_xz = Vec4::new(pos.x, pos.z, 0.0, 0.0);
            }
        }
        None => {
            *vis = Visibility::Hidden;
        }
    }
}
