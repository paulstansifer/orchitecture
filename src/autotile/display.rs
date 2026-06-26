use std::collections::HashMap;

use bevy::ecs::entity::Entity;
use bevy::math::{Quat, Vec3};
use bevy::prelude::{Commands, DetectChangesMut, Res, ResMut, SceneRoot, Transform};

use crate::sparse3d::{Facing, SlotCoord};
use crate::structure::{StructureId, StructureList};
use crate::util::zup_scene_transform;
use crate::wall_grid::{
    cell_transform, Cell, GridCellMarker, Proposal, ProposalGhostMarker, WallGrid,
};

use super::parser::char_matches_name;
use super::resources::{AutotileHandles, AutotileRules};
use super::{
    evaluate_autotile_rules, slot_to_unoriented, spec_stem, AutotileResult, MeshSpec,
    UnorientedSlot,
};

// ─── Shared helpers ───────────────────────────────────────────────────────────

fn char_matches(
    ch: char,
    id: StructureId,
    _facing: Facing,
    anchor_name: &str,
    all_names: &[String],
) -> bool {
    let name = &all_names[id.as_usize()];
    match ch {
        '=' => name == anchor_name,
        other => char_matches_name(other, name),
    }
}

/// Computes the transform for an autotile mesh at `loc`, applying the rotation
/// the matched rule (`spec`) assigned. The input cell's facing is deliberately
/// ignored — only the rule's matched direction matters.
pub fn autotile_transform(loc: SlotCoord, spec: &MeshSpec) -> Transform {
    let unoriented = slot_to_unoriented(loc.slot);
    // Ignore the direction of the input cell! Only the direction the rule matched in matters.
    let mut transform = cell_transform(loc.slot, crate::sparse3d::Facing::NegX, loc.cube);
    let rot_deg = spec.outer_rotation();
    if rot_deg != 0 {
        let angle = rot_deg as f32 * std::f32::consts::TAU / 360.0;
        let q = Quat::from_rotation_y(-angle);
        let pivot = if unoriented == UnorientedSlot::Wall {
            Vec3::new(0.5, 0.0, 0.0)
        } else {
            Vec3::new(0.5, 0.0, -0.5)
        };
        transform.translation += transform.rotation * (pivot - q * pivot);
        transform.rotation = transform.rotation * q;
    }
    transform
}

fn spawn_entities_from_results(
    commands: &mut Commands,
    autotile_handles: &AutotileHandles,
    loc: SlotCoord,
    results: &[AutotileResult],
    mut spawn_one: impl FnMut(&mut Commands, SceneRoot, Transform) -> Entity,
) -> Vec<Entity> {
    let unoriented = slot_to_unoriented(loc.slot);
    let mut entities = Vec::new();
    for result in results {
        if let AutotileResult::Mesh { spec, .. } = result {
            let stem = spec_stem(spec, unoriented);
            if let Some((main_handle, _)) = autotile_handles.handles.get(&stem) {
                let transform = zup_scene_transform(autotile_transform(loc, spec));
                entities.push(spawn_one(
                    commands,
                    SceneRoot(main_handle.clone()),
                    transform,
                ));
            }
        }
    }
    entities
}

fn apply_autotile_updates(
    commands: &mut Commands,
    autotile_handles: &AutotileHandles,
    structure_list: &StructureList,
    updates: Vec<(SlotCoord, Cell, Vec<AutotileResult>)>,
    stale_locs: Vec<SlotCoord>,
    results_cache: &mut HashMap<SlotCoord, Vec<AutotileResult>>,
    entity_cache: &mut HashMap<SlotCoord, Vec<Entity>>,
    use_fallback: bool,
    make_entity: impl Fn(&mut Commands, SceneRoot, Transform, SlotCoord) -> Entity,
) {
    for (loc, cell, new_results) in updates {
        if results_cache.get(&loc) == Some(&new_results) {
            continue;
        }
        if let Some(entities) = entity_cache.remove(&loc) {
            for e in entities {
                commands.entity(e).despawn();
            }
        }
        let new_entities = if use_fallback && new_results.is_empty() {
            let handle = structure_list.scene_handle(cell.id).clone();
            let transform = zup_scene_transform(cell_transform(loc.slot, cell.facing, loc.cube));
            vec![make_entity(commands, SceneRoot(handle), transform, loc)]
        } else {
            spawn_entities_from_results(
                commands,
                autotile_handles,
                loc,
                &new_results,
                |cmd, scene, transform| make_entity(cmd, scene, transform, loc),
            )
        };
        entity_cache.insert(loc, new_entities);
        results_cache.insert(loc, new_results);
    }
    for loc in stale_locs {
        results_cache.remove(&loc);
    }
}

// ─── System ───────────────────────────────────────────────────────────────────

pub fn autotile_update_system(
    mut commands: Commands,
    mut wall_grid: ResMut<WallGrid>,
    autotile_rules: Res<AutotileRules>,
    autotile_handles: Res<AutotileHandles>,
    structure_list: Res<StructureList>,
) {
    let struct_names: Vec<String> = structure_list
        .structures
        .iter()
        .map(|s| s.info.name.clone())
        .collect();

    // Real cells.
    let real_updates: Vec<(SlotCoord, Cell, Vec<AutotileResult>)> = wall_grid
        .contents
        .iter()
        .filter_map(|(loc, cell)| {
            let anchor = &struct_names[cell.id.as_usize()];
            let results = evaluate_autotile_rules(
                loc.into(),
                anchor,
                &autotile_rules.0,
                |nloc| wall_grid.contents.get(nloc).map(|c| (c.id, c.facing)),
                |ch, id, facing| char_matches(ch, id, facing, anchor, &struct_names),
                |name, id| struct_names[id.as_usize()] == name,
            )?;
            Some((loc, cell.clone(), results))
        })
        .collect();

    let real_stale: Vec<SlotCoord> = wall_grid
        .autotile_results
        .keys()
        .filter(|&&loc| wall_grid.contents.get(loc).is_none())
        .copied()
        .collect();

    // Proposed additions (snapshot before calling get_proposed_or_real).
    let proposed_additions: Vec<(SlotCoord, Cell)> = wall_grid
        .proposed_changes
        .iter()
        .filter_map(|(loc, proposal)| match proposal {
            Proposal::Place(cell) if wall_grid.contents.get(loc).is_none() => {
                Some((loc, cell.clone()))
            }
            _ => None,
        })
        .collect();

    let proposal_updates: Vec<(SlotCoord, Cell, Vec<AutotileResult>)> = proposed_additions
        .into_iter()
        .map(|(loc, cell)| {
            let anchor = &struct_names[cell.id.as_usize()];
            let results = evaluate_autotile_rules(
                loc.into(),
                anchor,
                &autotile_rules.0,
                |nloc| {
                    wall_grid
                        .get_proposed_or_real(nloc)
                        .map(|c| (c.id, c.facing))
                },
                |ch, id, facing| char_matches(ch, id, facing, anchor, &struct_names),
                |name, id| struct_names[id.as_usize()] == name,
            )
            .unwrap_or_default();
            (loc, cell, results)
        })
        .collect();

    let proposal_stale: Vec<SlotCoord> = wall_grid
        .proposal_autotile_results
        .keys()
        .filter(|&&loc| {
            !matches!(
                wall_grid.proposed_changes.get(loc),
                Some(Proposal::Place(_))
            ) || wall_grid.contents.get(loc).is_some()
        })
        .copied()
        .collect();

    // Apply — split-borrow the four cache fields so both calls can proceed.
    // bypass_change_detection: autotile only updates visual caches (autotile_results,
    // cell_entities), not structural contents, so we must not trigger WallGrid change detection.
    let wg = wall_grid.bypass_change_detection();
    apply_autotile_updates(
        &mut commands,
        &autotile_handles,
        &structure_list,
        real_updates,
        real_stale,
        &mut wg.autotile_results,
        &mut wg.cell_entities,
        false,
        |cmd, scene, transform, loc| cmd.spawn((scene, transform, GridCellMarker { loc })).id(),
    );
    apply_autotile_updates(
        &mut commands,
        &autotile_handles,
        &structure_list,
        proposal_updates,
        proposal_stale,
        &mut wg.proposal_autotile_results,
        &mut wg.proposal_entities,
        true,
        |cmd, scene, transform, loc| {
            cmd.spawn((scene, transform, ProposalGhostMarker { loc }))
                .id()
        },
    );
}
