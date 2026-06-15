use bevy::asset::{AssetServer, Handle};
use bevy::prelude::{Res, ResMut, Resource};
use bevy::scene::Scene;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StructureId(pub u32);

impl StructureId {
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Copy, Debug)]
pub enum PlacementStyle {
    WallDrag,
    FloorDrag,
    RoomPlop,
    RoomDrag,
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
    pub placement_style: PlacementStyle,
    pub x_char: Option<char>,
    pub z_char: Option<char>,
    pub embedding: StructureEmbedding,
    /// Furniture is always made of planks, regardless of the selected material.
    #[serde(default)]
    pub furniture: bool,
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
    pub fn scene_handle(&self, id: StructureId) -> &Handle<Scene> {
        &self.structures[id.as_usize()].mesh_handle
    }

    pub fn cut_handle(&self, id: StructureId) -> Option<&Handle<Scene>> {
        self.structures[id.as_usize()].cut_handle.as_ref()
    }

    pub fn find_by_name(&self, name: &str) -> Option<StructureId> {
        self.structures
            .iter()
            .position(|s| s.info.name == name)
            .map(|idx| StructureId(idx as u32))
    }
}

pub fn load_structure_info() -> Vec<StructureInfo> {
    let ron_content = include_str!("../buildables/structures.ron");
    ron::from_str(ron_content).unwrap()
}

pub fn find_structure_by_name(structures: &[StructureInfo], name: &str) -> Option<StructureId> {
    structures
        .iter()
        .position(|s| s.name == name)
        .map(|idx| StructureId(idx as u32))
}

/// The mesh-file stem for a structure: its name with spaces converted to
/// underscores (e.g. `"market stand"` → `"market_stand"`). The fallback meshes
/// for a structure live at `buildables/autotile/{stem}.gltf` (and, when the
/// structure has a cutaway variant, `buildables/autotile/{stem}-cut-y-pos.gltf`);
/// build.rs generates both from `buildables/{stem}.scad`.
pub fn structure_mesh_stem(name: &str) -> String {
    name.replace(' ', "_")
}

/// True if the autotile .gltf for `stem` (with `suffix`, e.g. `-cut-y-pos`) exists
/// and is non-empty. Furniture has no cut mesh, and build.rs writes an empty
/// (sentinel) file for intentionally-invisible cut variants, so both are treated
/// as "no mesh".
fn autotile_gltf_present(stem: &str, suffix: &str) -> bool {
    let path = std::path::Path::new(crate::paths::MANIFEST_DIR)
        .join(format!("buildables/autotile/{stem}{suffix}.gltf"));
    std::fs::metadata(&path)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

/// Startup system: loads structure infos and their fallback mesh handles from
/// `buildables/autotile/`, populating StructureList. (Most structures are drawn
/// by the autotile system; these handles are the fallback used when no autotile
/// rule matches — and the primary mesh for structures with no autotile rules.)
pub fn spawn_structures(asset_server: Res<AssetServer>, mut structure_list: ResMut<StructureList>) {
    let infos = load_structure_info();

    for info in &infos {
        let stem = structure_mesh_stem(&info.name);
        let mesh_handle = asset_server.load(format!("buildables/autotile/{stem}.gltf#Scene0"));
        let cut_handle = if autotile_gltf_present(&stem, "-cut-y-pos") {
            Some(asset_server.load(format!("buildables/autotile/{stem}-cut-y-pos.gltf#Scene0")))
        } else {
            None
        };
        structure_list.structures.push(Structure {
            info: info.clone(),
            mesh_handle,
            cut_handle,
        });
    }
}
