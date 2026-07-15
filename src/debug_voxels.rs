//! Debug overlay: press `X` to toggle a visualization of the Motif autotiler's output (see
//! `qnn/translate.rs::visible_motifs_and_defects`) at the current cursor location, as if `V`
//! had been pressed at that spot.
//!
//! Defects show up as 0.5-radius red spheres. Motif occurrences show up as 0.5-radius blue
//! lozenges (a capsule -- a cylinder with hemispherical caps) spanning their length, oriented
//! along the occurrence's axis; a length-1 (or axis-less) occurrence still renders as a
//! sphere rather than vanishing, since the capsule's own end caps give it a minimum size.
//!
//! This overlay previously visualized the QNN voxel representation directly (see
//! `spawn_debug_voxels`, kept below as dead code for now).

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
#[allow(unused_imports)]
use burn::prelude::*;

use crate::autotile::{DefectAtom, MotifAxis, MotifOccurrence};
use crate::camera::GameCamera;
#[allow(unused_imports)]
use crate::city::Cell;
use crate::city::ConstructedCity;
use crate::input::{cursor_world_pos, BuildState};
use crate::qnn::translate::visible_motifs_and_defects;
#[allow(unused_imports)]
use crate::qnn::translate::{sparse3d_to_tensor, EMBEDDING_SIZE};
use crate::sparse3d::{RelSlot, RelSlotCoord, SlotCoord};

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

    spawn_debug_motifs(
        &mut commands,
        &mut meshes,
        &mut materials,
        &constructed,
        center,
    );
}

/// The world-space center of a slot (a wall's center, not its cube's).
fn slot_world_center(loc: SlotCoord) -> Vec3 {
    let rel: RelSlotCoord = loc.into();
    let (x, y, z) = rel.get_center();
    Vec3::new(x as f32, y as f32, z as f32)
}

fn axis_unit_vector(axis: MotifAxis) -> Vec3 {
    match axis {
        // Arbitrary: an axis-less occurrence always has length 1, so it renders as a sphere
        // regardless of orientation.
        MotifAxis::None => Vec3::Y,
        MotifAxis::X => Vec3::X,
        MotifAxis::Y => Vec3::Y,
        MotifAxis::Z => Vec3::Z,
    }
}

/// Spawns a 0.5-radius red sphere for each defect and a 0.5-radius blue lozenge (a capsule --
/// a cylinder with hemispherical caps) spanning each motif occurrence's length. A capsule's
/// caps alone give it a minimum size, so a length-1 (or axis-less) occurrence still shows up as
/// a sphere rather than vanishing.
fn spawn_debug_motifs(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    constructed: &ConstructedCity,
    center: IVec3,
) {
    let vantage = RelSlotCoord::new(center.x, center.y, center.z, RelSlot::Room);
    let (occurrences, defects) =
        visible_motifs_and_defects(&constructed.contents, vantage, &constructed.eorfs);

    let red = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.0, 0.0),
        unlit: true,
        ..Default::default()
    });
    let blue = materials.add(StandardMaterial {
        base_color: Color::srgb(0.0, 0.0, 1.0),
        unlit: true,
        ..Default::default()
    });

    let sphere_mesh = meshes.add(Sphere::new(0.5));

    for defect in &defects {
        spawn_defect(commands, &sphere_mesh, &red, defect);
    }

    for occ in &occurrences {
        spawn_motif_occurrence(commands, meshes, &blue, occ);
    }

    println!(
        "debug motifs: {} occurrence(s), {} defect(s)",
        occurrences.len(),
        defects.len()
    );
}

fn spawn_defect(
    commands: &mut Commands,
    sphere_mesh: &Handle<Mesh>,
    material: &Handle<StandardMaterial>,
    defect: &DefectAtom,
) {
    commands.spawn((
        Mesh3d(sphere_mesh.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_translation(slot_world_center(defect.loc)),
        DebugVoxelMarker,
    ));
}

fn spawn_motif_occurrence(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: &Handle<StandardMaterial>,
    occ: &MotifOccurrence,
) {
    let dir = axis_unit_vector(occ.axis);
    let base_center = slot_world_center(occ.base);
    // Atom centers are 1 world unit apart along the axis; the run's midpoint sits half of
    // that (length - 1) span from the first atom's center.
    let mid = base_center + dir * ((occ.length as f32 - 1.0) / 2.0);

    // The run's total physical extent is `length` units (one per atom); the capsule's own caps
    // already contribute 1 unit (its 0.5 radius on each end), so the cylindrical part between
    // them makes up the rest. This is never negative: `length` is always at least 1.
    let cylinder_len = (occ.length as f32 - 1.0).max(0.0);
    let mesh = meshes.add(Capsule3d::new(0.5, cylinder_len));
    let rotation = Quat::from_rotation_arc(Vec3::Y, dir);

    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material.clone()),
        Transform::from_translation(mid).with_rotation(rotation),
        DebugVoxelMarker,
    ));
}

#[allow(dead_code)]
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

    // A wall (tall == 1.0, opaque per the ray-trace hack) is its own destination in the
    // ray trace to its own slot; it must not count as blocking the view of itself, or
    // every wall/floor would appear as occluded (`.visibility == 1.0`) instead of
    // structure (`.visibility == 0.5`).
    #[test]
    fn wall_is_not_self_occluding() {
        let mut sparse_data: Sparse3D<u32> = Sparse3D::new();
        let cube = IVec3::new(1, 0, 0);
        sparse_data.set(RelSlotCoord::new(0, 0, 0, RelSlot::Room), 0u32);
        sparse_data.set(
            RelSlotCoord::new(cube.x, cube.y, cube.z, RelSlot::XLoWall),
            1u32,
        );

        let center = IVec3::new(0, 0, 0);
        // tall == 1.0 for id 1 (the wall), matching the "opaque" hack in translate.rs.
        let tensor: Tensor<DebugBackend, 5, burn::tensor::Float> =
            sparse3d_to_tensor(&sparse_data, center, |id: &u32| {
                if *id == 1 {
                    vec![1.0, 0.0, 0.0, 0.0, 0.0]
                } else {
                    vec![0.0, 0.0, 0.0, 0.0, 0.0]
                }
            })
            .unwrap();

        let [_, _channels, size_x, size_y, size_z] = tensor.dims();
        let mut visibilities = Vec::new();
        for ix in 0..size_x {
            for iy in 0..size_y {
                for iz in 0..size_z {
                    let voxel = tensor
                        .clone()
                        .slice(s![0, .., ix, iy, iz])
                        .into_data()
                        .to_vec::<f32>()
                        .unwrap();
                    if voxel[0] == 1.0 {
                        visibilities.push(voxel[5]);
                    }
                }
            }
        }

        assert!(!visibilities.is_empty());
        assert!(visibilities.iter().all(|&v| v == 0.5), "{visibilities:?}");
    }
}
