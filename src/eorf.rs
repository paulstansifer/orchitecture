use bevy::asset::{AssetServer, Handle};
use bevy::prelude::{Res, ResMut, Resource};
use bevy::scene::Scene;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EorfId(pub u32);

impl EorfId {
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

/// What an `Eorf` is: a piece of Furniture (fixed cost, independent of the
/// selected build material -- most furniture is made of planks, but e.g.
/// pallets cost straw and canvas) or a building Element (priced by whichever
/// build material is selected for its `ElementType`).
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum FurnitureOrElement {
    Furniture(crate::materials::Cost),
    Element(crate::materials::ElementType),
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct EorfInfo {
    pub name: String,
    pub placement_style: PlacementStyle,
    pub x_char: Option<char>,
    pub z_char: Option<char>,
    pub embedding: StructureEmbedding,
    pub kind: FurnitureOrElement,
    /// Which `Place` kinds may claim this Furniture. Elements are never
    /// claimed by `Place`s, so this is only meaningful for furniture, but it's
    /// on `EorfInfo` (rather than `FurnitureOrElement::Furniture`) for
    /// uniformity with `PlaceInfo::restriction`.
    pub restriction: crate::place::ParentRestriction,
}

impl EorfInfo {
    pub fn is_furniture(&self) -> bool {
        matches!(self.kind, FurnitureOrElement::Furniture(_))
    }

    pub fn furniture_cost(&self) -> Option<&crate::materials::Cost> {
        match &self.kind {
            FurnitureOrElement::Furniture(cost) => Some(cost),
            FurnitureOrElement::Element(_) => None,
        }
    }

    pub fn element_type(&self) -> Option<crate::materials::ElementType> {
        match self.kind {
            FurnitureOrElement::Element(stype) => Some(stype),
            FurnitureOrElement::Furniture(_) => None,
        }
    }
}

/// Indices into `structures` sorted by group (each `ElementType` in its
/// declared order, then Furniture last), then by original position. Use this
/// for UI display order and digit-key shortcuts so both stay consistent.
pub fn sorted_structure_indices(structures: &[EorfInfo]) -> Vec<usize> {
    let mut idxs: Vec<usize> = (0..structures.len()).collect();
    idxs.sort_by_key(|&i| (structures[i].element_type(), !structures[i].is_furniture()));
    idxs
}

pub struct Eorf {
    pub info: EorfInfo,
    pub mesh_handle: Handle<Scene>,
    pub cut_handle: Option<Handle<Scene>>,
}

#[derive(Resource, Default)]
pub struct EorfList {
    pub structures: Vec<Eorf>,
}

impl EorfList {
    pub fn scene_handle(&self, id: EorfId) -> &Handle<Scene> {
        &self.structures[id.as_usize()].mesh_handle
    }

    pub fn cut_handle(&self, id: EorfId) -> Option<&Handle<Scene>> {
        self.structures[id.as_usize()].cut_handle.as_ref()
    }

    pub fn find_by_name(&self, name: &str) -> Option<EorfId> {
        self.structures
            .iter()
            .position(|s| s.info.name == name)
            .map(|idx| EorfId(idx as u32))
    }
}

/// `elements.ron` entry shape: the fields common to every `Eorf`, plus the
/// `ElementType` that drives its build-material cost.
#[derive(Deserialize)]
struct ElementRon {
    name: String,
    placement_style: PlacementStyle,
    x_char: Option<char>,
    z_char: Option<char>,
    embedding: StructureEmbedding,
    element_type: crate::materials::ElementType,
}

/// `furniture.ron` entry shape: the fields common to every `Eorf`, plus its
/// fixed material cost.
#[derive(Deserialize)]
struct FurnitureRon {
    name: String,
    placement_style: PlacementStyle,
    x_char: Option<char>,
    z_char: Option<char>,
    embedding: StructureEmbedding,
    cost: crate::materials::Cost,
}

/// Loads `buildables/elements.ron` and `buildables/furniture.ron`, in that
/// order, into a single unified list -- `EorfId`s are just indices into this
/// list, so elements always sort before furniture.
pub fn load_structure_info() -> Vec<EorfInfo> {
    let elements: Vec<ElementRon> =
        ron::from_str(include_str!("../buildables/elements.ron")).unwrap();
    let furniture: Vec<FurnitureRon> =
        ron::from_str(include_str!("../buildables/furniture.ron")).unwrap();

    elements
        .into_iter()
        .map(|e| EorfInfo {
            name: e.name,
            placement_style: e.placement_style,
            x_char: e.x_char,
            z_char: e.z_char,
            embedding: e.embedding,
            kind: FurnitureOrElement::Element(e.element_type),
            restriction: crate::place::ParentRestriction::Unrestricted,
        })
        .chain(furniture.into_iter().map(|f| EorfInfo {
            name: f.name,
            placement_style: f.placement_style,
            x_char: f.x_char,
            z_char: f.z_char,
            embedding: f.embedding,
            kind: FurnitureOrElement::Furniture(f.cost),
            restriction: crate::place::ParentRestriction::Unrestricted,
        }))
        .collect()
}

pub fn find_structure_by_name(structures: &[EorfInfo], name: &str) -> Option<EorfId> {
    structures
        .iter()
        .position(|s| s.name == name)
        .map(|idx| EorfId(idx as u32))
}

/// The mesh-file stem for a structure: its name with spaces converted to
/// underscores (e.g. `"market stand"` → `"market_stand"`). The fallback meshes
/// for a structure live at `assets/generated/autotile/{stem}.gltf` (and, when the
/// structure has a cutaway variant, `assets/generated/autotile/{stem}-cut-y-pos.gltf`);
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
        .join(format!("assets/generated/autotile/{stem}{suffix}.gltf"));
    std::fs::metadata(&path)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

/// Startup system: loads structure infos and their fallback mesh handles from
/// `assets/generated/autotile/`, populating EorfList. (Most structures are drawn
/// by the autotile system; these handles are the fallback used when no autotile
/// rule matches — and the primary mesh for structures with no autotile rules.)
pub fn spawn_structures(asset_server: Res<AssetServer>, mut structure_list: ResMut<EorfList>) {
    let infos = load_structure_info();

    for info in &infos {
        let stem = structure_mesh_stem(&info.name);
        let mesh_handle =
            asset_server.load(format!("assets/generated/autotile/{stem}.gltf#Scene0"));
        let cut_handle = if autotile_gltf_present(&stem, "-cut-y-pos") {
            Some(asset_server.load(format!(
                "assets/generated/autotile/{stem}-cut-y-pos.gltf#Scene0"
            )))
        } else {
            None
        };
        structure_list.structures.push(Eorf {
            info: info.clone(),
            mesh_handle,
            cut_handle,
        });
    }
}
