use std::collections::HashMap;

use godot::{classes::Mesh, prelude::*};
use serde::{Deserialize, Serialize};

use crate::mesh_management::load_meshes;

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Copy, Debug)]
pub enum PlacementStyle {
    WallDrag,
    FloorDrag,
    RoomPlop,
    WallPlop,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct StructureInfo {
    pub main_mesh: String,
    pub y_cut_mesh: Option<String>,
    pub placement_style: PlacementStyle,
    pub x_char: Option<char>,
    pub z_char: Option<char>,
}

pub struct Structure {
    pub info: StructureInfo,
    pub mesh: Gd<Mesh>,
    pub y_cut_mesh: Option<Gd<Mesh>>,
}

impl Structure {
    pub fn new(info: StructureInfo, meshes: &HashMap<String, Gd<Mesh>>) -> Structure {
        let main_mesh = meshes.get(&info.main_mesh).unwrap().clone();
        let y_cut_mesh = info
            .y_cut_mesh
            .as_ref()
            .map(|name| meshes.get(name).unwrap().clone());
        Structure {
            info,
            mesh: main_mesh,
            y_cut_mesh,
        }
    }
}

pub fn load_structure_info() -> Vec<StructureInfo> {
    let json_content = include_str!("../../structures.json");
    serde_json::from_str(json_content).unwrap()
}

pub fn load_structures() -> Vec<Structure> {
    let meshes = load_meshes();
    let infos = load_structure_info();
    let mut structures = vec![];

    for info in infos {
        structures.push(Structure::new(info, &meshes));
    }

    structures
}
