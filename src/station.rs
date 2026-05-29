use bevy::math::IVec3;
use serde::{Deserialize, Serialize};

struct StationReq {
    structure: crate::structure::StructureId,
    min: u8,
    max: Option<u8>,
    worker_visit_weight: f32,
    worker_visit_duration: f32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StationId(pub u32);

pub struct Station {
    name: String,
    // First one is the core station
    requirements: Vec<StationReq>,
}

pub struct ParticularStation {
    //First one is the core:
    structure_locations: Vec<IVec3>,
}
