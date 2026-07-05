use std::collections::BTreeMap;

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};

use crate::resource::UniformResource;

pub type Cost = Vec<(UniformResource, u16)>;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub enum ElementType {
    #[default]
    WallLike,
    RoofLike,
    PillarLike,
    GroundFloorLike,
    CantileverFloorLike,
}

impl ElementType {
    pub fn label(self) -> &'static str {
        match self {
            ElementType::WallLike => "Walls",
            ElementType::RoofLike => "Roof",
            ElementType::PillarLike => "Pillars",
            ElementType::GroundFloorLike => "Ground floor",
            ElementType::CantileverFloorLike => "Upper floor",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BuildMaterialId(pub u32);

/// The default is Ashlar (whose `world_material` is `MarbleBlocks`), so cells
/// that never chose a build material — e.g. anything loaded from disk — keep
/// the marble look. Guarded by `default_build_material_is_ashlar`.
impl Default for BuildMaterialId {
    fn default() -> Self {
        BuildMaterialId(5)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BuildMaterial {
    pub name: String,
    pub costs: BTreeMap<ElementType, Cost>,
    pub fanciness: f32,
}

impl BuildMaterial {
    pub fn world_material(&self) -> crate::city::Material {
        match self.name.as_str() {
            "Thatch" => crate::city::Material::Shingles,
            "Canvas" => crate::city::Material::Canvas,
            "Staves" => crate::city::Material::Timbers,
            "Fieldstone" => crate::city::Material::Fieldstone,
            "Plaster" => crate::city::Material::Stucco,
            "Ashlar" => crate::city::Material::MarbleBlocks,
            _ => crate::city::Material::default(),
        }
    }
}

#[derive(Resource, Default)]
pub struct MaterialList {
    pub materials: Vec<BuildMaterial>,
}

impl MaterialList {
    pub fn load() -> Self {
        let ron_content = include_str!("../buildables/materials.ron");
        MaterialList {
            materials: ron::from_str(ron_content).unwrap(),
        }
    }

    /// Returns materials that have a cost entry for `stype`, in definition order,
    /// paired with their `BuildMaterialId` in the full materials list.
    pub fn for_type(&self, stype: ElementType) -> Vec<(BuildMaterialId, &BuildMaterial)> {
        self.materials
            .iter()
            .enumerate()
            .filter(|(_, m)| m.costs.contains_key(&stype))
            .map(|(idx, m)| (BuildMaterialId(idx as u32), m))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_build_material_is_ashlar() {
        let list = MaterialList::load();
        let def = &list.materials[BuildMaterialId::default().0 as usize];
        assert_eq!(def.name, "Ashlar");
        assert_eq!(def.world_material(), crate::city::Material::MarbleBlocks);
    }
}
