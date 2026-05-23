use std::collections::HashMap;
use std::f32::consts::TAU;

use bevy::math::{IVec3, Quat, Vec3};
use bevy::prelude::{Commands, Component, Entity, Resource, SceneRoot, Transform};
use serde::{Deserialize, Serialize};

use crate::sparse3d::{Facing, RelSlot, SlotLocation, Sparse3D};
use crate::structure::{PlacementStyle, StructureInfo, StructureList};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct VantageEvaluation {
    pub coherence: f32,
    pub interest: f32,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Cell {
    pub id: i32,
    #[serde(default)]
    pub facing: Facing,
    pub evaluation: Option<VantageEvaluation>,
}

impl crate::sparse3d::Rotateable for Cell {
    fn rotate(self, rotation: crate::sparse3d::Rotation) -> Self {
        Cell { facing: self.facing.rotate(rotation), ..self }
    }
}

pub(crate) struct UndoRecord {
    // (location, what_was_there_before)
    pub(crate) changed: Vec<(SlotLocation, Option<Cell>)>,
}

/// Marker component for entities that represent placed grid cells.
#[derive(Component)]
pub struct GridCellMarker {
    pub loc: SlotLocation,
}

#[derive(Resource)]
pub struct WallGrid {
    pub structures: Vec<StructureInfo>,
    pub contents: Sparse3D<Cell>,
    /// Entity spawned for each placed cell.
    pub cell_entities: HashMap<SlotLocation, Entity>,
    /// Entities spawned for the y-cut visibility layer (cleared each visibility update).
    pub cut_entities: Vec<Entity>,
    pub(crate) undo_record: Vec<UndoRecord>,
}

impl WallGrid {
    pub fn new(structures: Vec<StructureInfo>) -> Self {
        WallGrid {
            structures,
            contents: Sparse3D::new(),
            cell_entities: HashMap::new(),
            cut_entities: Vec::new(),
            undo_record: Vec::new(),
        }
    }

    pub fn get_structure_names(&self) -> Vec<String> {
        self.structures
            .iter()
            .map(|s| {
                std::path::Path::new(&s.main_mesh)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&s.main_mesh)
                    .to_string()
            })
            .collect()
    }

    pub fn structure_is_room_plop(&self, id: i32) -> bool {
        self.structures[id as usize].placement_style == PlacementStyle::RoomPlop
    }
}

/// Computes the Bevy Transform for a cell at the given grid position.
pub fn cell_transform(slot: RelSlot, facing: Facing, cube: IVec3) -> Transform {
    let rx = Quat::from_rotation_x(-TAU / 4.0);
    let ry_neg90 = Quat::from_rotation_y(-TAU / 4.0);

    let rotation = match slot {
        RelSlot::Room => {
            let facing_angle = (1.0 - facing as u8 as f32) * (-TAU / 4.0);
            Quat::from_rotation_y(-TAU / 4.0 + facing_angle) * rx
        }
        RelSlot::XLoWall | RelSlot::XHiWall | RelSlot::Floor | RelSlot::Ceiling => ry_neg90 * rx,
        RelSlot::ZLoWall | RelSlot::ZHiWall => rx,
    };

    Transform {
        translation: cube.as_vec3(),
        rotation,
        scale: Vec3::ONE,
    }
}

/// Applies a list of cell changes to the world: despawns old entities, spawns new ones.
pub fn apply_changes(
    commands: &mut Commands,
    wall_grid: &mut WallGrid,
    structure_list: &StructureList,
    changes: Vec<(SlotLocation, Option<Cell>)>,
) {
    for (loc, new_cell) in changes {
        if let Some(old_entity) = wall_grid.cell_entities.remove(&loc) {
            commands.entity(old_entity).despawn();
        }
        if let Some(cell) = new_cell {
            let transform = cell_transform(loc.rel_slot, cell.facing, loc.cube);
            let handle = structure_list.scene_handle(cell.id).clone();
            let entity = commands
                .spawn((SceneRoot(handle), transform, GridCellMarker { loc }))
                .id();
            wall_grid.cell_entities.insert(loc, entity);
        }
    }
}

/// Startup system: creates the WallGrid resource from the already-populated StructureList.
pub fn spawn_grid(mut commands: Commands, structure_list: bevy::prelude::Res<StructureList>) {
    let infos = structure_list
        .structures
        .iter()
        .map(|s| s.info.clone())
        .collect();
    commands.insert_resource(WallGrid::new(infos));
}
