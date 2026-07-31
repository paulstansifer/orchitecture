use std::collections::HashMap;

use bevy::asset::{AssetServer, Handle};
use bevy::prelude::{Commands, Res, Resource};
use bevy::scene::Scene;

use super::matcher::{build_empty_anchor_index, EmptyAnchorIndex};
use super::parser::char_matches_name;
use super::{compile, parse, spec_stem, AutotileOriented, AutotiledMeshes};
use crate::eorf::EorfList;

#[derive(Resource)]
pub struct AutotileRules(pub Vec<AutotileOriented<AutotiledMeshes>>);

/// Dispatch index for `==empty:...==` rules (see `matcher::EmptyAnchorIndex`). Built once at
/// startup by `spawn_empty_anchor_index`, after both `AutotileRules` and `EorfList` are
/// populated — neither changes at runtime, so there's no need to rebuild this per-frame.
#[derive(Resource)]
pub struct EmptyAnchorRules(pub EmptyAnchorIndex<AutotiledMeshes>);

#[derive(Resource)]
pub struct AutotileHandles {
    /// stem → (main handle, cut handle)
    /// Cut handle is None when the cut .gltf is intentionally empty (invisible).
    pub handles: HashMap<String, (Handle<Scene>, Option<Handle<Scene>>)>,
}

fn gltf_is_empty(stem: &str, suffix: &str) -> bool {
    let path = std::path::Path::new(crate::paths::MANIFEST_DIR)
        .join(format!("assets/generated/autotile/{stem}{suffix}.gltf"));
    std::fs::metadata(&path)
        .map(|m| m.len() == 0)
        .unwrap_or(false)
}

pub fn spawn_autotile_rules(mut commands: Commands) {
    let src = include_str!("../../buildables/structures.autotile");
    let file = parse(src).expect("structures.autotile parse failed");
    commands.insert_resource(AutotileRules(compile(&file)));
}

/// Must run after both `spawn_autotile_rules` (needs `AutotileRules`) and `spawn_structures`
/// (needs `EorfList` populated).
pub fn spawn_empty_anchor_index(
    structure_list: Res<EorfList>,
    rules: Res<AutotileRules>,
    mut commands: Commands,
) {
    let names: Vec<String> = structure_list
        .structures
        .iter()
        .map(|s| s.info.name.clone())
        .collect();
    // `'='` (only meaningful relative to a named anchor's own structure) never matches here;
    // empty-anchored rules have no such anchor, so their dispatch character can't sensibly use it.
    let index = build_empty_anchor_index(&rules.0, &names, |ch, id, _facing| {
        char_matches_name(ch, &names[id.as_usize()])
    });
    commands.insert_resource(EmptyAnchorRules(index));
}

/// Must run after `spawn_autotile_rules` (shares its parsed `AutotileRules` rather than
/// re-parsing `structures.autotile`).
pub fn load_autotile_handles(
    asset_server: Res<AssetServer>,
    rules: Res<AutotileRules>,
    mut commands: Commands,
) {
    let mut handles: HashMap<String, (Handle<Scene>, Option<Handle<Scene>>)> = HashMap::new();
    for rule in &rules.0 {
        for case in &rule.cases {
            if let AutotiledMeshes::Mesh { spec, .. } = &case.result {
                let stem = spec_stem(spec, rule.slot);
                if handles.contains_key(&stem) {
                    continue;
                }
                assert!(
                    !gltf_is_empty(&stem, ""),
                    "main gltf for {stem} is empty; only cut meshes may be empty"
                );
                let main: Handle<Scene> =
                    asset_server.load(format!("assets/generated/autotile/{stem}.gltf#Scene0"));
                let cut: Option<Handle<Scene>> = if gltf_is_empty(&stem, "-cut-y-pos") {
                    None
                } else {
                    Some(asset_server.load(format!(
                        "assets/generated/autotile/{stem}-cut-y-pos.gltf#Scene0"
                    )))
                };
                handles.insert(stem, (main, cut));
            }
        }
    }
    commands.insert_resource(AutotileHandles { handles });
}
