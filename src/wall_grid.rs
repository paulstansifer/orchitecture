use std::collections::HashMap;
use std::f32::consts::TAU;

use bevy::math::{IVec3, Quat, Vec3};
use bevy::prelude::{
    AlphaMode, Assets, Color, Commands, Component, Entity, Mesh, Mesh3d, MeshMaterial3d, ResMut,
    Resource, SceneRoot, StandardMaterial, Transform,
};
use serde::{Deserialize, Serialize};

use crate::sparse3d::{Facing, RelSlot, SlotLocation, Sparse3D};
use crate::structure::{StructureId, StructureInfo, StructureList};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct VantageEvaluation {
    #[serde(default)]
    pub coherence: Option<f32>,
    #[serde(default)]
    pub interest: Option<f32>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Cell {
    pub id: StructureId,
    #[serde(default)]
    pub facing: Facing,
    pub evaluation: Option<VantageEvaluation>,
}

impl crate::sparse3d::Rotateable for Cell {
    fn rotate(self, rotation: crate::sparse3d::Rotation) -> Self {
        Cell {
            facing: self.facing.rotate(rotation),
            ..self
        }
    }
}

/// A proposed change at one grid location: place a new cell or remove the existing one.
#[derive(Clone, Debug, PartialEq)]
pub enum Proposal {
    Place(Cell),
    Remove,
}

/// What visual treatment a location should receive after a proposal edit.
#[derive(Clone, Debug)]
pub enum ProposalView {
    /// No proposal at this location; despawn any existing ghost/overlay.
    None,
    /// Proposed addition (no real cell): show translucent ghost.
    Add(Cell),
    /// Proposed removal (real cell exists): show real cell + red X.
    Remove,
    /// Proposed replacement (real cell exists, different new cell): show real cell + yellow ring.
    Replace,
}

pub(crate) struct UndoRecord {
    // (location, what proposal was there before — None = no proposal)
    pub(crate) changed: Vec<(SlotLocation, Option<Proposal>)>,
}

/// Marker component for entities that represent placed grid cells.
#[derive(Component)]
pub struct GridCellMarker {
    pub loc: SlotLocation,
}

/// Marker component for translucent ghost entities representing proposed additions.
#[derive(Component)]
pub struct ProposalGhostMarker {
    pub loc: SlotLocation,
}

/// Marker component for X or ring overlay entities on proposed removals/replacements.
#[derive(Component)]
pub struct ProposalOverlayMarker {
    pub loc: SlotLocation,
}

/// Marker component for cut-plane entities that replace a proposed-only (ghost) wall.
/// Children get the ghost material applied by `recolor_new_mesh_children`.
#[derive(Component)]
pub struct ProposedCutMarker;

/// Pre-built mesh/material handles used to spawn proposal overlays.
#[derive(Resource)]
pub struct ProposalOverlayAssets {
    /// Thin elongated cuboid along the X axis — used for X arms on floors/rooms/Z-walls.
    pub arm_along_x: Handle<Mesh>,
    /// Thin elongated cuboid along the Y axis — used for X arms on X-walls.
    pub arm_along_y: Handle<Mesh>,
    /// Torus ring for replacement indicators.
    pub ring_mesh: Handle<Mesh>,
    pub red_mat: Handle<StandardMaterial>,
    pub yellow_mat: Handle<StandardMaterial>,
    /// Translucent material applied to ghost entity children.
    pub ghost_mat: Handle<StandardMaterial>,
}

use bevy::asset::Handle;

#[derive(Resource)]
pub struct WallGrid {
    pub structures: Vec<StructureInfo>,
    pub contents: Sparse3D<Cell>,
    /// Proposed changes not yet committed; does not affect shadows or ceiling lights.
    pub proposed_changes: Sparse3D<Proposal>,
    /// Entity spawned for each placed (real) cell.
    pub cell_entities: HashMap<SlotLocation, Entity>,
    /// Entities spawned to visually preview proposals (ghosts + X/ring overlays).
    pub proposal_entities: HashMap<SlotLocation, Vec<Entity>>,
    /// Entities spawned for the y-cut visibility layer (cleared each visibility update).
    pub cut_entities: Vec<Entity>,
    /// Persistent cut entities for proposed-only walls; keyed by location, managed by diff.
    pub proposed_cut_entities: HashMap<SlotLocation, Entity>,
    pub(crate) undo_record: Vec<UndoRecord>,
    pub road_forbidden_zone: bool,
}

impl WallGrid {
    pub fn new(structures: Vec<StructureInfo>) -> Self {
        WallGrid {
            structures,
            contents: Sparse3D::new(),
            proposed_changes: Sparse3D::new(),
            cell_entities: HashMap::new(),
            proposal_entities: HashMap::new(),
            cut_entities: Vec::new(),
            proposed_cut_entities: HashMap::new(),
            undo_record: Vec::new(),
            road_forbidden_zone: true,
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

    pub fn structure_is_room_plop(&self, id: StructureId) -> bool {
        self.structures[id.as_usize()].placement_style == crate::structure::PlacementStyle::RoomPlop
    }

    /// Returns `(real, proposed_add)`:
    /// - `real`: the cell in `contents`, if any (present even under a `Proposal::Remove`).
    /// - `proposed_add`: the proposed cell only when it is an addition with no real cell beneath it.
    pub fn get_real_and_proposed(&self, loc: SlotLocation) -> (Option<&Cell>, Option<&Cell>) {
        let real = self.contents.get(loc);
        let proposed_add = match self.proposed_changes.get(loc) {
            Some(Proposal::Place(cell)) if real.is_none() => Some(cell),
            _ => None,
        };
        (real, proposed_add)
    }

    /// If both, returns `real`.
    pub fn get_real_or_proposed(&self, loc: SlotLocation) -> Option<&Cell> {
        let (real, proposed) = self.get_real_and_proposed(loc);
        real.or(proposed)
    }

    pub fn num_proposed_changes(&self) -> usize {
        self.proposed_changes.iter().count()
    }

    pub fn months_for_construction(&self) -> usize {
        (self.num_proposed_changes() + 79) / 80
    }
}

/// Computes the Bevy Transform for a cell at the given grid position.
pub fn cell_transform(slot: RelSlot, facing: Facing, cube: IVec3) -> Transform {
    let rx = Quat::from_rotation_x(-TAU / 4.0);
    let ry_neg90 = Quat::from_rotation_y(-TAU / 4.0);

    let (rotation, translation) = match slot {
        RelSlot::Room => {
            let facing_angle = (1.0 - facing as u8 as f32) * (-TAU / 4.0);
            let rotation = Quat::from_rotation_y(-TAU / 4.0 + facing_angle) * rx;
            // Rotate around the cell center rather than the cell corner, so the
            // desk stays in the same cell regardless of facing direction.
            let facing_rot = Quat::from_rotation_y(facing_angle);
            let cell_center = cube.as_vec3() + Vec3::splat(0.5);
            let translation = cell_center + facing_rot.mul_vec3(Vec3::splat(-0.5));
            (rotation, translation)
        }
        RelSlot::XLoWall | RelSlot::XHiWall | RelSlot::Floor | RelSlot::Ceiling => {
            (ry_neg90 * rx, cube.as_vec3())
        }
        RelSlot::ZLoWall | RelSlot::ZHiWall => (rx, cube.as_vec3()),
    };

    Transform {
        translation,
        rotation,
        scale: Vec3::ONE,
    }
}

/// Applies a list of real cell changes: despawns old entities, spawns new ones.
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

/// World-space center of a slot, used for positioning overlays.
fn slot_center(loc: SlotLocation) -> Vec3 {
    let base = loc.cube.as_vec3() + Vec3::splat(0.5);
    match loc.rel_slot {
        RelSlot::Room => base,
        RelSlot::XLoWall => Vec3::new(base.x - 0.5, base.y, base.z),
        RelSlot::XHiWall => Vec3::new(base.x + 0.5, base.y, base.z),
        RelSlot::Floor => Vec3::new(base.x, base.y - 0.5, base.z),
        RelSlot::Ceiling => Vec3::new(base.x, base.y + 0.5, base.z),
        RelSlot::ZLoWall => Vec3::new(base.x, base.y, base.z - 0.5),
        RelSlot::ZHiWall => Vec3::new(base.x, base.y, base.z + 0.5),
    }
}

/// (arm_mesh, rot_for_arm1, rot_for_arm2) to form an X in the plane of the slot.
///
/// The arm mesh is a thin stick. The rotations orient the two arms as diagonals
/// crossing in the slot's surface plane.
///
/// Geometry:
///   Floor/Ceiling/Room: X in XZ plane  — arm along X, rotate ±45° around Y
///   XLoWall/XHiWall:    X in YZ plane  — arm along Y, rotate ±45° around X
///   ZLoWall/ZHiWall:    X in XY plane  — arm along X, rotate ±45° around Z
fn x_mesh_and_rotations(
    slot: RelSlot,
    assets: &ProposalOverlayAssets,
) -> (Handle<Mesh>, Quat, Quat) {
    const ANGLE: f32 = TAU / 8.0;
    match slot {
        RelSlot::Floor | RelSlot::Ceiling | RelSlot::Room => (
            assets.arm_along_x.clone(),
            Quat::from_rotation_y(ANGLE),
            Quat::from_rotation_y(-ANGLE),
        ),
        RelSlot::XLoWall | RelSlot::XHiWall => (
            assets.arm_along_y.clone(),
            Quat::from_rotation_x(ANGLE),
            Quat::from_rotation_x(-ANGLE),
        ),
        RelSlot::ZLoWall | RelSlot::ZHiWall => (
            assets.arm_along_x.clone(),
            Quat::from_rotation_z(ANGLE),
            Quat::from_rotation_z(-ANGLE),
        ),
    }
}

/// Rotation to orient a Torus (default normal = +Y) to face the slot's surface normal.
fn ring_rotation(slot: RelSlot) -> Quat {
    match slot {
        RelSlot::Floor | RelSlot::Ceiling | RelSlot::Room => Quat::IDENTITY,
        // Rotate normal from +Y to +X: rotate -90° around Z
        RelSlot::XLoWall | RelSlot::XHiWall => Quat::from_rotation_z(-TAU / 4.0),
        // Rotate normal from +Y to +Z: rotate +90° around X
        RelSlot::ZLoWall | RelSlot::ZHiWall => Quat::from_rotation_x(TAU / 4.0),
    }
}

/// Returns the axis along which overlays should be duplicated so they protrude
/// from the slot surface and remain visible rather than buried in the mesh.
fn protrude_axis(slot: RelSlot) -> Option<Vec3> {
    match slot {
        RelSlot::XLoWall | RelSlot::XHiWall => Some(Vec3::X),
        RelSlot::ZLoWall | RelSlot::ZHiWall => Some(Vec3::Z),
        RelSlot::Floor | RelSlot::Ceiling => Some(Vec3::Y),
        RelSlot::Room => None,
    }
}

fn spawn_x_overlay(
    commands: &mut Commands,
    assets: &ProposalOverlayAssets,
    loc: SlotLocation,
) -> Vec<Entity> {
    let center = slot_center(loc);
    let (arm_mesh, rot1, rot2) = x_mesh_and_rotations(loc.rel_slot, assets);

    // For wall slots spawn on both faces so the X is never buried.
    const PROTRUDE: f32 = 0.15;
    let offsets: &[Vec3] = match protrude_axis(loc.rel_slot) {
        Some(n) => &[n * PROTRUDE, n * -PROTRUDE],
        None => &[Vec3::ZERO],
    };

    let mut entities = Vec::new();
    for &offset in offsets {
        let c = center + offset;
        entities.push(
            commands
                .spawn((
                    Mesh3d(arm_mesh.clone()),
                    MeshMaterial3d(assets.red_mat.clone()),
                    Transform::from_translation(c).with_rotation(rot1),
                    ProposalOverlayMarker { loc },
                ))
                .id(),
        );
        entities.push(
            commands
                .spawn((
                    Mesh3d(arm_mesh.clone()),
                    MeshMaterial3d(assets.red_mat.clone()),
                    Transform::from_translation(c).with_rotation(rot2),
                    ProposalOverlayMarker { loc },
                ))
                .id(),
        );
    }
    entities
}

fn spawn_ring_overlay(
    commands: &mut Commands,
    assets: &ProposalOverlayAssets,
    loc: SlotLocation,
) -> Vec<Entity> {
    let mut center = slot_center(loc);
    if loc.rel_slot == RelSlot::Room {
        center.y += 0.25; // Float in upper half of cell for room objects
    }

    const PROTRUDE: f32 = 0.15;
    let offsets: &[Vec3] = match protrude_axis(loc.rel_slot) {
        Some(n) => &[n * PROTRUDE, n * -PROTRUDE],
        None => &[Vec3::ZERO],
    };

    let mut entities = Vec::new();
    for &offset in offsets {
        entities.push(
            commands
                .spawn((
                    Mesh3d(assets.ring_mesh.clone()),
                    MeshMaterial3d(assets.yellow_mat.clone()),
                    Transform::from_translation(center + offset)
                        .with_rotation(ring_rotation(loc.rel_slot)),
                    ProposalOverlayMarker { loc },
                ))
                .id(),
        );
    }
    entities
}

/// Applies a list of proposal view changes: despawns old overlays/ghosts, spawns new ones.
pub fn apply_proposal_changes(
    commands: &mut Commands,
    wall_grid: &mut WallGrid,
    structure_list: &StructureList,
    overlay_assets: &ProposalOverlayAssets,
    changes: Vec<(SlotLocation, ProposalView)>,
) {
    for (loc, view) in changes {
        if let Some(entities) = wall_grid.proposal_entities.remove(&loc) {
            for entity in entities {
                commands.entity(entity).despawn();
            }
        }
        match view {
            ProposalView::None => {}
            ProposalView::Add(cell) => {
                let mut transform = cell_transform(loc.rel_slot, cell.facing, loc.cube);
                transform.scale *= 0.999;
                let handle = structure_list.scene_handle(cell.id).clone();
                let entity = commands
                    .spawn((SceneRoot(handle), transform, ProposalGhostMarker { loc }))
                    .id();
                wall_grid.proposal_entities.insert(loc, vec![entity]);
            }
            ProposalView::Remove => {
                let entities = spawn_x_overlay(commands, overlay_assets, loc);
                wall_grid.proposal_entities.insert(loc, entities);
            }
            ProposalView::Replace => {
                let entities = spawn_ring_overlay(commands, overlay_assets, loc);
                wall_grid.proposal_entities.insert(loc, entities);
            }
        }
    }
}

/// Startup system: creates mesh/material handles for proposal overlays.
pub fn spawn_proposal_overlay_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    use bevy::prelude::Torus;

    let arm_along_x = meshes.add(bevy::prelude::Cuboid::new(0.8, 0.07, 0.07));
    let arm_along_y = meshes.add(bevy::prelude::Cuboid::new(0.07, 0.8, 0.07));
    let ring_mesh = meshes.add(Torus {
        minor_radius: 0.07,
        major_radius: 0.35,
    });

    let red_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.15, 0.15, 0.9),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..Default::default()
    });
    let yellow_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.85, 0.1, 0.9),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..Default::default()
    });
    let ghost_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.75, 0.9, 1.0, 0.45),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..Default::default()
    });

    commands.insert_resource(ProposalOverlayAssets {
        arm_along_x,
        arm_along_y,
        ring_mesh,
        red_mat,
        yellow_mat,
        ghost_mat,
    });
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
