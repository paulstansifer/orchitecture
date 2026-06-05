use bevy::ecs::entity::Entity;
use bevy::math::{Quat, Vec3};
use bevy::prelude::{Commands, Res, ResMut, SceneRoot, Transform};

use crate::sparse3d::SlotLocation;
use crate::structure::{StructureId, StructureList};
use crate::wall_grid::{cell_transform, Cell, GridCellMarker, Proposal, ProposalGhostMarker, WallGrid};

use super::{
    evaluate_autotile_rules, rel_slot_to_unoriented, spec_stem,
    AutotileResult, MeshSpec, UnorientedSlot,
};
use super::resources::{AutotileHandles, AutotileRules};

// ─── Shared helpers ───────────────────────────────────────────────────────────

/// Returns whether character `ch` from an autotile pattern is satisfied by `id`,
/// given the anchor cell's structure name and the full name table.
fn char_matches(ch: char, id: StructureId, anchor_name: &str, all_names: &[String]) -> bool {
    let name = &all_names[id.as_usize()];
    match ch {
        '=' => name == anchor_name,
        'F' => name == "floor",
        'W' => name == "wall",
        'S' => name == "stairs",
        'R' => name == "railing",
        _ => false,
    }
}

/// Computes the world transform for an autotile mesh result, applying any
/// spec-specified rotation on top of the base cell transform.
fn autotile_transform(loc: SlotLocation, cell: &Cell, spec: &MeshSpec) -> Transform {
    let unoriented = rel_slot_to_unoriented(loc.rel_slot);
    let mut transform = cell_transform(loc.rel_slot, cell.facing, loc.cube);
    let rot_deg = spec.outer_rotation();
    if rot_deg != 0 {
        let angle = rot_deg as f32 * std::f32::consts::TAU / 360.0;
        let q = Quat::from_rotation_z(-angle);
        let pivot = if unoriented == UnorientedSlot::Wall {
            Vec3::new(0.5, 0.0, 0.0)
        } else {
            Vec3::new(0.5, 0.5, 0.0)
        };
        transform.translation += transform.rotation * (pivot - q * pivot);
        transform.rotation = transform.rotation * q;
    }
    transform
}

/// Spawns one entity per `AutotileResult::Mesh` in `results`, returning the entity IDs.
/// `spawn_one` receives the `SceneRoot` and `Transform` and is responsible for attaching
/// the correct marker component.
fn spawn_entities_from_results(
    commands: &mut Commands,
    autotile_handles: &AutotileHandles,
    loc: SlotLocation,
    cell: &Cell,
    results: &[AutotileResult],
    mut spawn_one: impl FnMut(&mut Commands, SceneRoot, Transform) -> Entity,
) -> Vec<Entity> {
    let unoriented = rel_slot_to_unoriented(loc.rel_slot);
    let mut entities = Vec::new();
    for result in results {
        if let AutotileResult::Mesh { spec, .. } = result {
            let stem = spec_stem(spec, unoriented);
            if let Some((main_handle, _)) = autotile_handles.handles.get(&stem) {
                let transform = autotile_transform(loc, cell, spec);
                entities.push(spawn_one(commands, SceneRoot(main_handle.clone()), transform));
            }
        }
    }
    entities
}

// ─── Systems ──────────────────────────────────────────────────────────────────

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

    // Phase A: collect locations with autotile rules and compute new results.
    let updates: Vec<(SlotLocation, Cell, Vec<AutotileResult>)> = wall_grid
        .contents
        .iter()
        .filter_map(|(loc, cell)| {
            let anchor = &struct_names[cell.id.as_usize()];
            let results = evaluate_autotile_rules(
                loc,
                anchor,
                &autotile_rules.0,
                |nloc| wall_grid.contents.get(nloc).map(|c| c.id),
                |ch, id| char_matches(ch, id, anchor, &struct_names),
            )?;
            Some((loc, cell.clone(), results))
        })
        .collect();

    // Phase B: apply where results differ from last frame.
    for (loc, cell, new_results) in updates {
        if wall_grid.autotile_results.get(&loc) == Some(&new_results) {
            continue;
        }
        if let Some(entities) = wall_grid.cell_entities.remove(&loc) {
            for e in entities { commands.entity(e).despawn(); }
        }
        let new_entities = spawn_entities_from_results(
            &mut commands, &autotile_handles, loc, &cell, &new_results,
            |cmd, scene, transform| cmd.spawn((scene, transform, GridCellMarker { loc })).id(),
        );
        wall_grid.cell_entities.insert(loc, new_entities);
        wall_grid.autotile_results.insert(loc, new_results);
    }

    // Purge stale entries for cells that no longer exist.
    let stale: Vec<SlotLocation> = wall_grid
        .autotile_results
        .keys()
        .filter(|&&loc| wall_grid.contents.get(loc).is_none())
        .copied()
        .collect();
    for loc in stale {
        wall_grid.autotile_results.remove(&loc);
    }
}

/// Per-frame system: evaluates autotile rules for proposed additions using
/// proposed-or-real neighbor lookup, then spawns/despawns ghost entities.
pub fn proposal_autotile_update_system(
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

    // Phase A part 1: snapshot proposed pure additions to release the borrow on proposed_changes.
    let proposed_additions: Vec<(SlotLocation, Cell)> = wall_grid
        .proposed_changes
        .iter()
        .filter_map(|(loc, proposal)| match proposal {
            Proposal::Place(cell) if wall_grid.contents.get(loc).is_none() => {
                Some((loc, cell.clone()))
            }
            _ => None,
        })
        .collect();

    // Phase A part 2: evaluate autotile rules using proposed-or-real neighbor lookup.
    // get_proposed_or_real is safe now that proposed_changes is no longer borrowed.
    let updates: Vec<(SlotLocation, Cell, Vec<AutotileResult>)> = proposed_additions
        .into_iter()
        .map(|(loc, cell)| {
            let anchor = &struct_names[cell.id.as_usize()];
            let results = evaluate_autotile_rules(
                loc,
                anchor,
                &autotile_rules.0,
                |nloc| wall_grid.get_proposed_or_real(nloc).map(|c| c.id),
                |ch, id| char_matches(ch, id, anchor, &struct_names),
            )
            .unwrap_or_default(); // empty vec = no autotile rules → use default mesh
            (loc, cell, results)
        })
        .collect();

    // Phase B: apply where results differ from last frame.
    for (loc, cell, new_results) in updates {
        if wall_grid.proposal_autotile_results.get(&loc) == Some(&new_results) {
            continue;
        }
        if let Some(entities) = wall_grid.proposal_entities.remove(&loc) {
            for e in entities { commands.entity(e).despawn(); }
        }
        let new_entities = if new_results.is_empty() {
            // No autotile rules: fall back to the structure's default scene handle.
            let handle = structure_list.scene_handle(cell.id).clone();
            let mut transform = cell_transform(loc.rel_slot, cell.facing, loc.cube);
            vec![commands.spawn((SceneRoot(handle), transform, ProposalGhostMarker { loc })).id()]
        } else {
            spawn_entities_from_results(
                &mut commands, &autotile_handles, loc, &cell, &new_results,
                |cmd, scene, transform| cmd.spawn((scene, transform, ProposalGhostMarker { loc })).id(),
            )
        };
        wall_grid.proposal_entities.insert(loc, new_entities);
        wall_grid.proposal_autotile_results.insert(loc, new_results);
    }

    // Purge stale entries for locations that are no longer proposed pure additions.
    let stale: Vec<SlotLocation> = wall_grid
        .proposal_autotile_results
        .keys()
        .filter(|&&loc| {
            !matches!(wall_grid.proposed_changes.get(loc), Some(Proposal::Place(_)))
                || wall_grid.contents.get(loc).is_some()
        })
        .copied()
        .collect();
    for loc in stale {
        wall_grid.proposal_autotile_results.remove(&loc);
    }
}
