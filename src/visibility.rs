use bevy::math::{IVec3, Vec3};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::camera::GameCamera;
use crate::input::{cursor_world_pos, BuildState};
use crate::sparse3d::{RelSlot, SlotLocation, Sparse3D};
use crate::structure::StructureList;
use crate::wall_grid::{cell_transform, Cell, CutCellMarker, GridCellMarker, WallGrid};

/// Compute which cells are hidden / need cut variants based on camera view.
/// Returns (cells_to_hide, cells_needing_cut_variant).
pub fn compute_visibility(
    contents: &Sparse3D<Cell>,
    focus_location: Vec3,
    camera_location: Vec3,
) -> (Vec<SlotLocation>, Vec<(SlotLocation, i32)>) {
    let diff = focus_location - camera_location;
    let view_dir = IVec3::new(
        diff.x.signum() as i32,
        diff.y.signum() as i32,
        diff.z.signum() as i32,
    );
    let effective_focus = focus_location.round().as_ivec3()
        + IVec3::new(view_dir.x * 2, 0, view_dir.z * 2);
    let last_y_layer = IVec3::new(view_dir.x, 0, view_dir.z);

    let sign = |v: IVec3| IVec3::new(v.x.signum(), v.y.signum(), v.z.signum());

    let mut hidden: Vec<SlotLocation> = Vec::new();
    let mut cut: Vec<(SlotLocation, i32)> = Vec::new();

    for (loc, cell) in contents.iter() {
        let delta = effective_focus - loc.cube;
        if sign(delta) == view_dir {
            hidden.push(loc);
        } else if sign(delta) == last_y_layer {
            // Floors and ceilings at the current layer have no cut variant and
            // should remain visible; only hide/cut wall and room cells here.
            match loc.rel_slot {
                RelSlot::Floor | RelSlot::Ceiling => {}
                _ => {
                    hidden.push(loc);
                    cut.push((loc, cell.id));
                }
            }
        }
    }
    (hidden, cut)
}

pub fn update_visibility_system(
    mut commands: Commands,
    mut wall_grid: ResMut<WallGrid>,
    structure_list: Res<StructureList>,
    build_state: Res<BuildState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    mut vis_q: Query<(Entity, &GridCellMarker, &mut Visibility)>,
    cut_q: Query<Entity, With<CutCellMarker>>,
) {
    let Ok((_, cam_gt)) = camera_q.single() else {
        return;
    };
    let camera_pos = cam_gt.translation();
    let focus_pos = cursor_world_pos(&windows, &camera_q, build_state.cur_y as f32)
        .unwrap_or_else(|| Vec3::new(0.0, build_state.cur_y as f32, 0.0));

    for entity in cut_q.iter() {
        commands.entity(entity).despawn();
    }
    // bypass_change_detection so per-frame cut_entities writes don't mark WallGrid
    // as changed and don't trigger ceiling light rebuilds every frame.
    wall_grid.bypass_change_detection().cut_entities.clear();

    let (hidden_locs, cut_entries) =
        compute_visibility(&wall_grid.contents, focus_pos, camera_pos);
    let hidden_set: std::collections::HashSet<_> = hidden_locs.into_iter().collect();

    for (entity, marker, mut vis) in vis_q.iter_mut() {
        *vis = if hidden_set.contains(&marker.loc) {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        let _ = entity;
    }

    for (loc, id) in cut_entries {
        if let Some(cut_handle) = structure_list.cut_handle(id) {
            let transform = cell_transform(loc.rel_slot, crate::sparse3d::Facing::NegX, loc.cube);
            let entity = commands
                .spawn((SceneRoot(cut_handle.clone()), transform, CutCellMarker))
                .id();
            wall_grid.bypass_change_detection().cut_entities.push(entity);
        }
    }
}
