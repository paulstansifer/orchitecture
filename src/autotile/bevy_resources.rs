use std::collections::HashMap;

use bevy::asset::{AssetServer, Handle};
use bevy::prelude::{Commands, Res, ResMut, Resource, SceneRoot};
use bevy::scene::Scene;

use crate::sparse3d::SlotLocation;
use crate::structure::{StructureId, StructureList};
use crate::wall_grid::{cell_transform, GridCellMarker, WallGrid};

use super::{
    compile, evaluate_autotile_rules, parse, rel_slot_to_unoriented, spec_stem,
    AutotileOriented, AutotileResult,
};

#[derive(Resource)]
pub struct AutotileRules(pub Vec<AutotileOriented>);

#[derive(Resource)]
pub struct AutotileHandles {
    /// stem → (main handle, cut handle)
    pub handles: HashMap<String, (Handle<Scene>, Handle<Scene>)>,
}

pub fn spawn_autotile_rules(mut commands: Commands) {
    let src = include_str!("../../buildables/structures.autotile");
    let file = parse(src).expect("structures.autotile parse failed");
    commands.insert_resource(AutotileRules(compile(&file)));
}

pub fn load_autotile_handles(asset_server: Res<AssetServer>, mut commands: Commands) {
    let src = include_str!("../../buildables/structures.autotile");
    let file = parse(src).expect("structures.autotile parse failed");
    let oriented = compile(&file);

    let mut handles: HashMap<String, (Handle<Scene>, Handle<Scene>)> = HashMap::new();
    for rule in &oriented {
        for case in &rule.cases {
            if let AutotileResult::Mesh { spec, .. } = &case.result {
                let stem = spec_stem(spec, rule.slot);
                if handles.contains_key(&stem) {
                    continue;
                }
                let main: Handle<Scene> =
                    asset_server.load(format!("buildables/autotile/{stem}.gltf#Scene0"));
                let cut: Handle<Scene> = asset_server
                    .load(format!("buildables/autotile/{stem}-cut-y-pos.gltf#Scene0"));
                handles.insert(stem, (main, cut));
            }
        }
    }
    commands.insert_resource(AutotileHandles { handles });
}

pub fn autotile_update_system(
    mut commands: Commands,
    mut wall_grid: ResMut<WallGrid>,
    autotile_rules: Res<AutotileRules>,
    autotile_handles: Res<AutotileHandles>,
    structure_list: Res<StructureList>,
) {
    // Pre-extract names to avoid repeated borrow conflicts in the closures below.
    let struct_names: Vec<String> = structure_list
        .structures
        .iter()
        .map(|s| s.info.name.clone())
        .collect();

    // Phase A: collect all locations with autotile rules, compute new results.
    // Borrows wall_grid.contents immutably; all mutation is deferred to Phase B.
    let updates: Vec<(SlotLocation, crate::wall_grid::Cell, Vec<AutotileResult>)> = {
        wall_grid
            .contents
            .iter()
            .filter_map(|(loc, cell)| {
                let struct_name = &struct_names[cell.id.as_usize()];
                let char_matches = |ch: char, id: StructureId| -> bool {
                    let name = &struct_names[id.as_usize()];
                    match ch {
                        '=' => name.as_str() == struct_name.as_str(),
                        'F' => name == "floor",
                        'W' => name == "wall",
                        'S' => name == "stairs",
                        'R' => name == "railing",
                        _ => false,
                    }
                };
                let results = evaluate_autotile_rules(
                    loc,
                    struct_name,
                    &autotile_rules.0,
                    &wall_grid.contents,
                    char_matches,
                )?;
                Some((loc, cell.clone(), results))
            })
            .collect()
    };

    // Phase B: apply changes where the results differ from last frame.
    for (loc, cell, new_results) in updates {
        if wall_grid.autotile_results.get(&loc) == Some(&new_results) {
            continue;
        }
        // Despawn old entities for this location.
        if let Some(entities) = wall_grid.cell_entities.remove(&loc) {
            for e in entities {
                commands.entity(e).despawn();
            }
        }

        let unoriented = rel_slot_to_unoriented(loc.rel_slot);
        let mut new_entities = Vec::new();
        for result in &new_results {
            if let AutotileResult::Mesh { spec, .. } = result {
                let stem = spec_stem(spec, unoriented);
                if let Some((main_handle, _)) = autotile_handles.handles.get(&stem) {
                    let transform = cell_transform(loc.rel_slot, cell.facing, loc.cube);
                    let entity = commands
                        .spawn((
                            SceneRoot(main_handle.clone()),
                            transform,
                            GridCellMarker { loc },
                        ))
                        .id();
                    new_entities.push(entity);
                }
            }
        }

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
