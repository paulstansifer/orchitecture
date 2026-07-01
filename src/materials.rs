use std::collections::BTreeMap;

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};

use crate::resource::UniformResource;

type Cost = Vec<(UniformResource, u16)>;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub enum StructureType {
    WallLike,
    RoofLike,
    PillarLike,
    GroundFloorLike,
    CantileverFloorLike,
    #[default]
    Furniture,
}

impl StructureType {
    pub fn label(self) -> &'static str {
        match self {
            StructureType::WallLike => "Walls",
            StructureType::RoofLike => "Roof",
            StructureType::PillarLike => "Pillars",
            StructureType::GroundFloorLike => "Ground floor",
            StructureType::CantileverFloorLike => "Upper floor",
            StructureType::Furniture => "Furniture",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct BuildMaterialId(pub u32);

#[derive(Clone, Debug, Deserialize)]
pub struct BuildMaterial {
    pub name: String,
    pub costs: BTreeMap<StructureType, Cost>,
    pub fanciness: f32,
}

impl BuildMaterial {
    pub fn world_material(&self) -> crate::world::Material {
        match self.name.as_str() {
            "Thatch" => crate::world::Material::Shingles,
            "Canvas" => crate::world::Material::Canvas,
            "Staves" => crate::world::Material::Timbers,
            "Fieldstone" => crate::world::Material::Fieldstone,
            "Plaster" => crate::world::Material::Stucco,
            "Ashlar" => crate::world::Material::MarbleBlocks,
            _ => crate::world::Material::default(),
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
    pub fn for_type(&self, stype: StructureType) -> Vec<(BuildMaterialId, &BuildMaterial)> {
        self.materials
            .iter()
            .enumerate()
            .filter(|(_, m)| m.costs.contains_key(&stype))
            .map(|(idx, m)| (BuildMaterialId(idx as u32), m))
            .collect()
    }
}
