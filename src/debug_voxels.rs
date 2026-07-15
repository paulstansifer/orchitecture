//! Debug overlay: press `X` to toggle a visualization of the QNN voxel
//! representation (see `qnn/translate.rs`) at the current cursor location,
//! as if `V` had been pressed at that spot.
//!
//! Each voxel is drawn as a half-sized cube (voxels are twice as wide as the
//! grid cells they subdivide, so a 0.5-unit cube exactly fills one slot):
//!   * `.tall` sets the cube's height, scaled from 0.5 (flat) to 1.0 (a full
//!     grid cell tall).
//!   * `.passable` sets its alpha, scaled from 1.0 (impassable things are
//!     fully opaque) to 0.5 (passable things are only half-opaque, so the
//!     real geometry stays visible underneath).
//!   * `.decorative`, `.striated`, and `.temporary` become the red, green,
//!     and blue color channels, respectively.
//! Voxels the vantage can see as indoor open air (`.visibility == 0.0`,
//! equivalent to empty space) are skipped entirely; everything else -- real
//! structure (`.visibility == 0.5`) as well as anything outdoors or occluded
//! (`.visibility == 1.0`, rendered as a 10%-opaque black cube marking the
//! edge of what's visible) -- gets a cube.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use burn::prelude::*;

use crate::camera::GameCamera;
use crate::city::{Cell, ConstructedCity};
use crate::input::{cursor_world_pos, BuildState};
use crate::qnn::translate::{sparse3d_to_tensor, EMBEDDING_SIZE};

// Only used to read back voxel values on the CPU; no model inference happens here.
type DebugBackend = burn::backend::NdArray<f32>;

// Must stay in sync with the radius `sparse3d_to_tensor` builds its tensor around.
const RADIUS: IVec3 = IVec3::new(5, 2, 5);

#[derive(Resource, Default)]
pub struct DebugVoxelsState {
    pub enabled: bool,
}

#[derive(Component)]
pub struct DebugVoxelMarker;

pub struct DebugVoxelsPlugin;

impl Plugin for DebugVoxelsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DebugVoxelsState::default());
    }
}

#[allow(clippy::too_many_arguments)]
pub fn debug_voxels_system(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    egui_wants_input: Res<bevy_egui::input::EguiWantsInput>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    build_state: Res<BuildState>,
    constructed: Res<ConstructedCity>,
    mut state: ResMut<DebugVoxelsState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<DebugVoxelMarker>>,
) {
    let typing = egui_wants_input.wants_keyboard_input();
    if typing || !keyboard.just_pressed(KeyCode::KeyX) {
        return;
    }

    state.enabled = !state.enabled;

    for entity in &existing {
        commands.entity(entity).despawn();
    }

    if !state.enabled {
        return;
    }

    let Some(cursor_pos) = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32) else {
        return;
    };
    let center = cursor_pos.round().as_ivec3();

    spawn_debug_voxels(
        &mut commands,
        &mut meshes,
        &mut materials,
        &constructed,
        center,
    );
}

fn spawn_debug_voxels(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    constructed: &ConstructedCity,
    center: IVec3,
) {
    let min_coord = center - RADIUS;

    let tensor: Tensor<DebugBackend, 5, burn::tensor::Float> =
        sparse3d_to_tensor(&constructed.contents, center, |cell: &Cell| {
            constructed.eorfs[cell.id.as_usize()].embedding.to_vec()
        })
        .expect("debug voxel translation should not fail");

    let [_, channels, size_x, size_y, size_z] = tensor.dims();
    debug_assert_eq!(channels, EMBEDDING_SIZE);

    let (mut indoor_count, mut structure_count, mut outdoor_or_occluded_count) = (0, 0, 0);

    for ix in 0..size_x {
        for iy in 0..size_y {
            for iz in 0..size_z {
                let voxel = tensor
                    .clone()
                    .slice(s![0, .., ix, iy, iz])
                    .into_data()
                    .to_vec::<f32>()
                    .unwrap();
                let [tall, decorative, passable, striated, temporary, visibility] = voxel[..]
                else {
                    continue;
                };

                if visibility == 0.0 {
                    indoor_count += 1;
                } else if visibility == 0.5 {
                    structure_count += 1;
                } else {
                    outdoor_or_occluded_count += 1;
                }

                // `visibility == 0.0` is indoor open air the vantage can see -- equivalent
                // to empty space, so skip it. `0.5` (real structure) and `1.0` (outdoors or
                // occluded) both get a cube.
                if visibility == 0.0 {
                    continue;
                }

                let height = 0.5 + 0.5 * tall;
                // Outdoors/occluded voxels carry no real `.passable` data (there's no cell
                // there), so give them a fixed, mostly-see-through alpha instead of the
                // structure-based formula below.
                let alpha = if visibility == 1.0 {
                    0.1
                } else {
                    1.0 - 0.5 * passable
                };
                let world_center =
                    min_coord.as_vec3() + Vec3::new(ix as f32, iy as f32, iz as f32) * 0.5;

                let mesh = meshes.add(Cuboid::new(0.5, height, 0.5));
                let material = materials.add(StandardMaterial {
                    base_color: Color::srgba(decorative, striated, temporary, alpha),
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    double_sided: true,
                    cull_mode: None,
                    ..Default::default()
                });

                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    Transform::from_translation(world_center),
                    DebugVoxelMarker,
                ));
            }
        }
    }

    println!(
        "debug voxels: {indoor_count} indoor open air, {structure_count} structure, \
         {outdoor_or_occluded_count} outdoors/occluded",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sparse3d::{RelSlot, RelSlotCoord, Sparse3D};

    // A Room-slot cell should land on a voxel whose reconstructed world center
    // matches the grid cube's midpoint (cube + 0.5 on every axis), confirming
    // `world_center`'s formula agrees with `grid_coord_to_voxel_coord`.
    #[test]
    fn room_voxel_center_matches_cube_midpoint() {
        let mut sparse_data: Sparse3D<u32> = Sparse3D::new();
        let cube = IVec3::new(2, 0, -1);
        sparse_data.set(
            RelSlotCoord::new(cube.x, cube.y, cube.z, RelSlot::Room),
            1u32,
        );

        let center = cube;
        let min_coord = center - RADIUS;
        let tensor: Tensor<DebugBackend, 5, burn::tensor::Float> =
            sparse3d_to_tensor(&sparse_data, center, |id: &u32| {
                vec![0.0, 0.0, 0.0, 0.0, *id as f32]
            })
            .unwrap();

        let [_, _channels, size_x, size_y, size_z] = tensor.dims();
        let mut found = None;
        for ix in 0..size_x {
            for iy in 0..size_y {
                for iz in 0..size_z {
                    let voxel = tensor
                        .clone()
                        .slice(s![0, .., ix, iy, iz])
                        .into_data()
                        .to_vec::<f32>()
                        .unwrap();
                    if voxel[5] == 0.5 {
                        found = Some((ix, iy, iz));
                    }
                }
            }
        }

        let (ix, iy, iz) = found.expect("expanded voxel should be present");
        let world_center = min_coord.as_vec3() + Vec3::new(ix as f32, iy as f32, iz as f32) * 0.5;
        assert_eq!(world_center, cube.as_vec3() + Vec3::splat(0.5));
    }
}
