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

pub struct StructureEmbedding {
    pub tall: f32,
    pub passable: f32,
    pub decorative: f32,
    pub striated: f32,
}

impl StructureEmbedding {
    fn dist(&self, other: &Self) -> f32 {
        (self.tall - other.tall).abs()
            + (self.passable - other.passable).abs()
            + (self.decorative - other.decorative).abs()
            + (self.striated - other.striated).abs()
    }
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
    pub mesh: Gd<Mesh>,
    pub y_cut_mesh: Option<Gd<Mesh>>,
}

// Rooms: 4 compass directions (4 walls), and up/down (2 floors)
// Walls: 4 compass directions (2 walls, 2 rooms), plus forward/backward x up/down (4 floors)
// Floors: Up/down (rooms), 4 compass x up/down (8 walls)

// pub enum SlotOffset {}

// pub struct Offset {
//     cell: Vector3i,
//     slot: SlotOffset,
// }

// struct Pattern {
//     sub_pats: Vec<(Offset, ())>,
// }

// pub struct OrderedMeshRules {
//     ordered_rules: Vec<(Pattern, crate::mesh_management::MeshId)>,
// }

// pub struct MeshRules {
//     coexistenting_rules: Vec<OrderedMeshRules>,
// }

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
    let structures: Vec<StructureInfo> = serde_json::from_str(json_content).unwrap();

    let mut min_dist = f32::MAX;
    let mut closest_pair = (None, None);

    for i in 0..structures.len() {
        for j in (i + 1)..structures.len() {
            let dist = structures[i].embedding.dist(&structures[j].embedding);
            if dist < min_dist {
                min_dist = dist;
                closest_pair = (Some(&structures[i]), Some(&structures[j]));
            }
        }
    }

    if let (Some(s1), Some(s2)) = closest_pair {
        println!(
            "Closest structure embeddings: {} and {} with distance {}",
            s1.name, s2.name, min_dist
        );
    }

    structures
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
