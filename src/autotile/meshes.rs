use std::collections::HashMap;

use bevy::asset::{AssetServer, Handle};
use bevy::prelude::{Commands, Res, Resource};
use bevy::scene::Scene;

use super::{compile, parse, spec_stem, AutotileOriented, AutotileResult};

#[derive(Resource)]
pub struct AutotileRules(pub Vec<AutotileOriented>);

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

pub fn load_autotile_handles(asset_server: Res<AssetServer>, mut commands: Commands) {
    let src = include_str!("../../buildables/structures.autotile");
    let file = parse(src).expect("structures.autotile parse failed");
    let oriented = compile(&file);

    let mut handles: HashMap<String, (Handle<Scene>, Option<Handle<Scene>>)> = HashMap::new();
    for rule in &oriented {
        for case in &rule.cases {
            if let AutotileResult::Mesh { spec, .. } = &case.result {
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
