use std::collections::HashMap;

use bevy::asset::{AssetServer, Handle};
use bevy::prelude::{Res, ResMut, Resource};
use bevy::scene::Scene;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Copy, Debug)]
pub enum PlacementStyle {
    WallDrag,
    FloorDrag,
    RoomPlop,
    WallPlop,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct StructureEmbedding {
    pub tall: f32,
    pub passable: f32,
    pub decorative: f32,
    pub striated: f32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct StructureInfo {
    pub name: String,
    pub main_mesh: String,
    pub y_cut_mesh: Option<String>,
    pub placement_style: PlacementStyle,
    pub x_char: Option<char>,
    pub z_char: Option<char>,
    pub embedding: StructureEmbedding,
}

pub struct Structure {
    pub info: StructureInfo,
    pub mesh_handle: Handle<Scene>,
    pub cut_handle: Option<Handle<Scene>>,
}

#[derive(Resource, Default)]
pub struct StructureList {
    pub structures: Vec<Structure>,
}

impl StructureList {
    pub fn scene_handle(&self, id: i32) -> &Handle<Scene> {
        &self.structures[id as usize].mesh_handle
    }

    pub fn cut_handle(&self, id: i32) -> Option<&Handle<Scene>> {
        self.structures[id as usize].cut_handle.as_ref()
    }
}

pub fn load_structure_info() -> Vec<StructureInfo> {
    let json_content = include_str!("../buildables/structures.json");
    serde_json::from_str(json_content).unwrap()
}

/// Load all .gltf files from the `buildables/` directory via Bevy's AssetServer.
pub fn load_mesh_handles(asset_server: &AssetServer) -> HashMap<String, Handle<Scene>> {
    let mut handles = HashMap::new();

    let buildables_path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/buildables"));
    let dir = match std::fs::read_dir(buildables_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Could not read buildables/: {e}");
            return handles;
        }
    };

    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("gltf") {
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                let asset_path = format!("buildables/{filename}#Scene0");
                let handle: Handle<Scene> = asset_server.load(&asset_path);
                handles.insert(filename.to_string(), handle);
            }
        }
    }

    handles
}

/// Startup system: loads structure infos and mesh handles, populates StructureList.
pub fn spawn_structures(asset_server: Res<AssetServer>, mut structure_list: ResMut<StructureList>) {
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
        structure_list.structures.push(Structure {
            info: info.clone(),
            mesh_handle,
            cut_handle,
        });
    }
}
