use bevy::asset::Handle;
use bevy::prelude::Resource;
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
    let json_content = include_str!("../../structures.json");
    serde_json::from_str(json_content).unwrap()
}
