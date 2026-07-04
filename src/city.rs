use std::collections::HashMap;
use std::f32::consts::TAU;

#[cfg(autotile_matching)]
use crate::autotile::AutotileResult;
use bevy::ecs::system::SystemParam;
use bevy::math::{IVec3, Quat, Vec3};
use bevy::prelude::{
    AlphaMode, Assets, Color, Commands, Component, DetectChanges, Entity, Image, Mesh, Mesh3d,
    MeshMaterial3d, Query, Res, ResMut, Resource, SceneRoot, StandardMaterial, Transform, With,
};
use serde::{Deserialize, Serialize};

use crate::eorf::{EorfId, EorfInfo, EorfList};
use crate::gi_material::{default_gi_image, GiExtension, GiMaterial};
use crate::sparse3d::{Facing, Slot, SlotCoord, Sparse3D};

/// A score that may be an exact target or a one-sided inequality constraint.
///
/// Serializes as a bare float for `Exact` (backwards-compatible with old `f32` fields)
/// and as `{"at_most": v}` / `{"at_least": v}` for the bounded variants.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ConstrainedScore {
    /// Score must equal this value.
    Exact(f32),
    /// Score must be ≤ this value (no penalty for being lower).
    AtMost { at_most: f32 },
    /// Score must be ≥ this value (no penalty for being higher).
    AtLeast { at_least: f32 },
}

impl ConstrainedScore {
    pub fn value(self) -> f32 {
        match self {
            Self::Exact(v) | Self::AtMost { at_most: v } | Self::AtLeast { at_least: v } => v,
        }
    }
}

impl From<f32> for ConstrainedScore {
    fn from(v: f32) -> Self {
        Self::Exact(v)
    }
}

/// Arithmetic and bound-relaxation operations, defined for both `ConstrainedScore`
/// and `Option<ConstrainedScore>`.
pub trait ConstrainedScoreExt: Sized {
    /// Shift the threshold value by `delta`.
    fn add(self, delta: f32) -> Self;
    /// Shift the threshold value down by `delta`.
    fn subtract(self, delta: f32) -> Self;
    /// Relax to an **at-least** constraint: the score may be higher than the threshold.
    fn unbound_higher(self) -> Self;
    /// Relax to an **at-most** constraint: the score may be lower than the threshold.
    fn unbound_lower(self) -> Self;
}

impl ConstrainedScoreExt for ConstrainedScore {
    fn add(self, delta: f32) -> Self {
        match self {
            Self::Exact(v) => Self::Exact(v + delta),
            Self::AtMost { at_most: v } => Self::AtMost { at_most: v + delta },
            Self::AtLeast { at_least: v } => Self::AtLeast {
                at_least: v + delta,
            },
        }
    }
    fn subtract(self, delta: f32) -> Self {
        self.add(-delta)
    }
    fn unbound_higher(self) -> Self {
        Self::AtLeast {
            at_least: self.value(),
        }
    }
    fn unbound_lower(self) -> Self {
        Self::AtMost {
            at_most: self.value(),
        }
    }
}

impl ConstrainedScoreExt for Option<ConstrainedScore> {
    fn add(self, delta: f32) -> Self {
        self.map(|cs| cs.add(delta))
    }
    fn subtract(self, delta: f32) -> Self {
        self.map(|cs| cs.subtract(delta))
    }
    fn unbound_higher(self) -> Self {
        self.map(|cs| cs.unbound_higher())
    }
    fn unbound_lower(self) -> Self {
        self.map(|cs| cs.unbound_lower())
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct VantageEvaluation {
    #[serde(default)]
    pub coherence: Option<ConstrainedScore>,
    #[serde(default)]
    pub interest: Option<ConstrainedScore>,
}

/// What a structure is built from. Determines its color. Not serialized: anything
/// loaded from disk is assumed to be `MarbleBlocks` (the default).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Material {
    Timbers,
    Fieldstone,
    Canvas,
    Planks,
    Shingles,
    Stucco,
    Bricks,
    #[default]
    MarbleBlocks,
}

impl Material {
    /// All materials, in display order.
    pub const ALL: [Material; 8] = [
        Material::Timbers,
        Material::Fieldstone,
        Material::Canvas,
        Material::Planks,
        Material::Shingles,
        Material::Stucco,
        Material::Bricks,
        Material::MarbleBlocks,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Material::Timbers => "Timbers",
            Material::Fieldstone => "Fieldstone",
            Material::Canvas => "Canvas",
            Material::Planks => "Planks",
            Material::Shingles => "Shingles",
            Material::Stucco => "Stucco",
            Material::Bricks => "Bricks",
            Material::MarbleBlocks => "Marble blocks",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Material::Timbers => Color::srgb(0.40, 0.26, 0.13),
            Material::Fieldstone => Color::srgb(0.44, 0.49, 0.44),
            Material::Canvas => Color::srgb(0.60, 0.75, 0.95),
            Material::Planks => Color::srgb(0.72, 0.55, 0.35),
            Material::Shingles => Color::srgb(0.28, 0.18, 0.10),
            Material::Stucco => Color::srgb(0.94, 0.90, 0.65),
            Material::Bricks => Color::srgb(0.62, 0.25, 0.20),
            Material::MarbleBlocks => Color::srgb(0.85, 0.85, 0.87),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Cell {
    pub id: EorfId,
    #[serde(default)]
    pub facing: Facing,
    pub evaluation: Option<VantageEvaluation>,
    /// What the structure is made from. Not serialized (see `Material`).
    #[serde(skip)]
    pub material: Material,
    /// Index into the MaterialList. Serialized with a default fallback (unlike `material`,
    /// which is derived from it and skipped entirely).
    #[serde(default)]
    pub build_material: crate::materials::BuildMaterialId,
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
    // (location, the desired cell before this action — None = empty/no cell).
    // "Desired" resolves any proposal (Place/Remove) and otherwise falls back to
    // the real cell, so undo records survive `construct()` and can revert
    // already-committed cells by creating reverse proposals.
    pub(crate) changed: Vec<(SlotCoord, Option<Cell>)>,
}

/// Marker component for entities that represent placed grid cells.
#[derive(Component)]
pub struct GridCellMarker {
    pub loc: SlotCoord,
}

/// Marker component for translucent ghost entities representing proposed additions.
#[derive(Component)]
pub struct ProposalGhostMarker {
    pub loc: SlotCoord,
}

/// Marker component for X or ring overlay entities on proposed removals/replacements.
#[derive(Component)]
pub struct ProposalOverlayMarker {
    pub loc: SlotCoord,
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

/// Committed, authoritative world state. Change detection on this resource
/// triggers GI recomputation.
#[derive(Resource)]
pub struct ConstructedCity {
    pub contents: Sparse3D<Cell>,
    /// Places placed in the world.
    pub placed_places: Vec<crate::place::ParticularPlace>,
    pub road_forbidden_zone: bool,
    /// Duplicated from the `Res<>` for simplicity:
    pub eorfs: Vec<EorfInfo>,
    /// Duplicated from the `Res<>` for simplicity:
    pub places: Vec<crate::place::PlaceInfo>,
}

impl ConstructedCity {
    pub fn new(eorfs: Vec<EorfInfo>) -> Self {
        ConstructedCity {
            eorfs,
            contents: Sparse3D::new(),
            places: Vec::new(),
            placed_places: Vec::new(),
            road_forbidden_zone: true,
        }
    }

    pub fn get_structure_names(&self) -> Vec<String> {
        self.eorfs.iter().map(|s| s.name.clone()).collect()
    }

    pub fn structure_is_room_plop(&self, id: EorfId) -> bool {
        self.eorfs[id.as_usize()].placement_style == crate::eorf::PlacementStyle::RoomPlop
    }

    pub fn find_structure_by_name(&self, name: &str) -> Option<EorfId> {
        crate::eorf::find_structure_by_name(&self.eorfs, name)
    }

    pub fn replace_contents(
        &mut self,
        new_contents: Sparse3D<Cell>,
    ) -> Vec<(SlotCoord, Option<Cell>)> {
        let mut changes: Vec<(SlotCoord, Option<Cell>)> = Vec::new();
        for (loc, _) in self.contents.iter() {
            changes.push((loc, None));
        }
        for (loc, cell) in new_contents.iter() {
            changes.push((loc, Some(cell.clone())));
        }
        self.contents = new_contents;
        changes
    }
}

/// Proposed edits not yet committed via Construct!. Mutating this does NOT
/// trigger GI recomputation.
#[derive(Resource)]
pub struct ProposedCity {
    pub proposed_changes: Sparse3D<Proposal>,
    pub(crate) undo_record: Vec<UndoRecord>,
    /// Inverse of undone actions, for redo. Cleared when a fresh edit is made.
    pub(crate) redo_record: Vec<UndoRecord>,
    /// Months already waited toward completing the current construction project.
    pub months_waited: u32,
}

impl Default for ProposedCity {
    fn default() -> Self {
        Self::new()
    }
}

impl ProposedCity {
    pub fn new() -> Self {
        ProposedCity {
            proposed_changes: Sparse3D::new(),
            undo_record: Vec::new(),
            redo_record: Vec::new(),
            months_waited: 0,
        }
    }

    pub fn num_changes(&self) -> usize {
        self.proposed_changes.iter().count()
    }

    pub fn months_for_construction(&self, population_size: usize) -> usize {
        self.num_changes().div_ceil(80 * population_size.max(1))
    }
}

pub fn clear_proposal_entities(commands: &mut Commands, assembled: &mut AssembledCity) {
    for (_, entities) in assembled.proposal_entities.drain() {
        for entity in entities {
            commands.entity(entity).despawn();
        }
    }
}

pub fn clear_proposed_cut_entities(commands: &mut Commands, viewable: &mut ViewableWorld) {
    for (_, (_, entities)) in viewable.proposed_cut_entities.drain() {
        for entity in entities {
            commands.entity(entity).despawn();
        }
    }
}

/// Autotile-generated ECS entities for both real and proposed cells.
#[derive(Resource)]
pub struct AssembledCity {
    /// Entities spawned for each placed (real) cell (may be multiple for autotile cells).
    pub cell_entities: HashMap<SlotCoord, Vec<Entity>>,
    /// Last-rendered autotile results per location (one per matching rule), for change detection.
    pub autotile_results: HashMap<SlotCoord, Vec<crate::autotile::AutotileResult>>,
    /// Entities spawned to visually preview proposals (ghosts + X/ring overlays).
    pub proposal_entities: HashMap<SlotCoord, Vec<Entity>>,
    /// Last-rendered autotile results per proposed-addition location, for change detection.
    #[cfg(autotile_matching)]
    pub proposal_autotile_results: HashMap<SlotCoord, Vec<AutotileResult>>,
}

impl Default for AssembledCity {
    fn default() -> Self {
        Self::new()
    }
}

impl AssembledCity {
    pub fn new() -> Self {
        AssembledCity {
            cell_entities: HashMap::new(),
            autotile_results: HashMap::new(),
            proposal_entities: HashMap::new(),
            #[cfg(autotile_matching)]
            proposal_autotile_results: HashMap::new(),
        }
    }
}

/// Cutaway-generated ECS entities (cut-plane variants of walls/floors).
#[derive(Resource)]
pub struct ViewableWorld {
    /// Persistent cut entities for the y-cut cutaway layer of real cells; keyed by
    /// location with the structure they were built for, managed by diff so they live
    /// long enough to be recolored by material.
    pub cut_entities: HashMap<SlotCoord, (EorfId, Vec<Entity>)>,
    /// Persistent cut entities for proposed-only walls; keyed by location, managed by diff.
    pub proposed_cut_entities: HashMap<SlotCoord, (EorfId, Vec<Entity>)>,
}

impl Default for ViewableWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewableWorld {
    pub fn new() -> Self {
        ViewableWorld {
            cut_entities: HashMap::new(),
            proposed_cut_entities: HashMap::new(),
        }
    }
}

/// Groups the three mutable world resources into a single `SystemParam` so that
/// systems with many other parameters don't exceed Bevy's 16-parameter limit.
#[derive(SystemParam)]
pub struct CityMut<'w> {
    pub constructed: ResMut<'w, ConstructedCity>,
    pub pending: ResMut<'w, ProposedCity>,
    pub assembled: ResMut<'w, AssembledCity>,
}

/// Read-only counterpart to `WorldMut`, for systems that only need to
/// inspect the world resources.
#[derive(SystemParam)]
pub struct City<'w> {
    pub constructed: Res<'w, ConstructedCity>,
    pub pending: Res<'w, ProposedCity>,
    pub assembled: Res<'w, AssembledCity>,
}

/// Returns `(real, proposed_add)`:
/// - `real`: the cell in `contents`, if any (present even under a `Proposal::Remove`).
/// - `proposed_add`: the proposed cell only when it is an addition with no real cell beneath it.
pub fn get_real_and_proposed<'a>(
    cw: &'a ConstructedCity,
    pw: &'a ProposedCity,
    loc: impl Into<SlotCoord>,
) -> (Option<&'a Cell>, Option<&'a Cell>) {
    let loc: SlotCoord = loc.into();
    let real = cw.contents.get(loc);
    let proposed_add = match pw.proposed_changes.get(loc) {
        Some(Proposal::Place(cell)) if real.is_none() => Some(cell),
        _ => None,
    };
    (real, proposed_add)
}

/// Returns real cell if present, otherwise the proposed addition if present.
/// If both, returns `real`.
pub fn get_real_or_proposed<'a>(
    cw: &'a ConstructedCity,
    pw: &'a ProposedCity,
    loc: impl Into<SlotCoord>,
) -> Option<&'a Cell> {
    let (real, proposed) = get_real_and_proposed(cw, pw, loc);
    real.or(proposed)
}

/// Returns proposed addition if present, otherwise the real cell.
/// If both, returns `real`.
pub fn get_proposed_or_real<'a>(
    cw: &'a ConstructedCity,
    pw: &'a ProposedCity,
    loc: impl Into<SlotCoord>,
) -> Option<&'a Cell> {
    let (real, proposed) = get_real_and_proposed(cw, pw, loc);
    proposed.or(real)
}

/// The cell the user currently wants at `loc`: the proposed state if one
/// exists (`Place` → that cell, `Remove` → empty), otherwise the real cell.
pub fn desired(cw: &ConstructedCity, pw: &ProposedCity, loc: impl Into<SlotCoord>) -> Option<Cell> {
    let loc: SlotCoord = loc.into();
    match pw.proposed_changes.get(loc) {
        Some(Proposal::Place(cell)) => Some(cell.clone()),
        Some(Proposal::Remove) => None,
        None => cw.contents.get(loc).cloned(),
    }
}

/// Computes the Bevy Transform for a cell at the given grid position.
pub fn cell_transform(slot: Slot, facing: Facing, cube: IVec3) -> Transform {
    let ry_neg90 = Quat::from_rotation_y(-TAU / 4.0);

    let (rotation, translation) = match slot {
        Slot::Room | Slot::Floor => {
            let facing_angle = (1.0 - facing as u8 as f32) * (-TAU / 4.0);
            let rotation = Quat::from_rotation_y(-TAU / 4.0 + facing_angle);
            // Rotate around the cell center rather than the cell corner, so the
            // desk stays in the same cell regardless of facing direction.
            let facing_rot = Quat::from_rotation_y(facing_angle);
            let cell_center = cube.as_vec3() + Vec3::splat(0.5);
            let translation = cell_center + facing_rot.mul_vec3(Vec3::splat(-0.5));
            (rotation, translation)
        }
        Slot::ZLoWall => (Quat::IDENTITY, cube.as_vec3()),
        Slot::XLoWall => (ry_neg90, cube.as_vec3()),
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
    assembled: &mut AssembledCity,
    structure_list: &EorfList,
    changes: Vec<(SlotCoord, Option<Cell>)>,
) {
    for (loc, new_cell) in changes {
        if let Some(old_entities) = assembled.cell_entities.remove(&loc) {
            for e in old_entities {
                commands.entity(e).despawn();
            }
        }
        // Clear autotile state so the per-frame system unconditionally re-evaluates.
        assembled.autotile_results.remove(&loc);
        if let Some(cell) = new_cell {
            let transform =
                crate::util::zup_scene_transform(cell_transform(loc.slot, cell.facing, loc.cube));
            let handle = structure_list.scene_handle(cell.id).clone();
            let entity = commands
                .spawn((SceneRoot(handle), transform, GridCellMarker { loc }))
                .id();
            assembled.cell_entities.insert(loc, vec![entity]);
        }
    }
}

/// World-space center of a slot, used for positioning overlays.
pub fn slot_center(loc: SlotCoord) -> Vec3 {
    let base = loc.cube.as_vec3() + Vec3::splat(0.5);
    match loc.slot {
        Slot::Room => base,
        Slot::XLoWall => Vec3::new(base.x - 0.5, base.y, base.z),
        Slot::Floor => Vec3::new(base.x, base.y - 0.5, base.z),
        Slot::ZLoWall => Vec3::new(base.x, base.y, base.z - 0.5),
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
fn x_mesh_and_rotations(slot: Slot, assets: &ProposalOverlayAssets) -> (Handle<Mesh>, Quat, Quat) {
    const ANGLE: f32 = TAU / 8.0;
    match slot {
        Slot::Floor | Slot::Room => (
            assets.arm_along_x.clone(),
            Quat::from_rotation_y(ANGLE),
            Quat::from_rotation_y(-ANGLE),
        ),
        Slot::XLoWall => (
            assets.arm_along_y.clone(),
            Quat::from_rotation_x(ANGLE),
            Quat::from_rotation_x(-ANGLE),
        ),
        Slot::ZLoWall => (
            assets.arm_along_x.clone(),
            Quat::from_rotation_z(ANGLE),
            Quat::from_rotation_z(-ANGLE),
        ),
    }
}

/// Rotation to orient a Torus (default normal = +Y) to face the slot's surface normal.
fn ring_rotation(slot: Slot) -> Quat {
    match slot {
        Slot::Floor | Slot::Room => Quat::IDENTITY,
        // Rotate normal from +Y to +X: rotate -90° around Z
        Slot::XLoWall => Quat::from_rotation_z(-TAU / 4.0),
        // Rotate normal from +Y to +Z: rotate +90° around X
        Slot::ZLoWall => Quat::from_rotation_x(TAU / 4.0),
    }
}

/// Returns the axis along which overlays should be duplicated so they protrude
/// from the slot surface and remain visible rather than buried in the mesh.
fn protrude_axis(slot: Slot) -> Option<Vec3> {
    match slot {
        Slot::XLoWall => Some(Vec3::X),
        Slot::ZLoWall => Some(Vec3::Z),
        Slot::Floor => Some(Vec3::Y),
        Slot::Room => None,
    }
}

fn spawn_x_overlay(
    commands: &mut Commands,
    assets: &ProposalOverlayAssets,
    loc: SlotCoord,
) -> Vec<Entity> {
    let center = slot_center(loc);
    let (arm_mesh, rot1, rot2) = x_mesh_and_rotations(loc.slot, assets);

    // For wall slots spawn on both faces so the X is never buried.
    const PROTRUDE: f32 = 0.15;
    let offsets: &[Vec3] = match protrude_axis(loc.slot) {
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
    loc: SlotCoord,
) -> Vec<Entity> {
    let mut center = slot_center(loc);
    if loc.slot == Slot::Room {
        center.y += 0.25; // Float in upper half of cell for room objects
    }

    const PROTRUDE: f32 = 0.15;
    let offsets: &[Vec3] = match protrude_axis(loc.slot) {
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
                        .with_rotation(ring_rotation(loc.slot)),
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
    assembled: &mut AssembledCity,
    _structure_list: &EorfList,
    overlay_assets: &ProposalOverlayAssets,
    changes: Vec<(SlotCoord, ProposalView)>,
) {
    for (loc, view) in changes {
        if let Some(entities) = assembled.proposal_entities.remove(&loc) {
            for entity in entities {
                commands.entity(entity).despawn();
            }
        }
        #[cfg(autotile_matching)]
        assembled.proposal_autotile_results.remove(&loc);
        match view {
            ProposalView::None => {}
            ProposalView::Add(_) => {
                // Ghost entities for additions are managed by `proposal_autotile_update_system`.
                // The cached results were already cleared above, so that system re-evaluates
                // this location next frame.
            }
            ProposalView::Remove => {
                let entities = spawn_x_overlay(commands, overlay_assets, loc);
                assembled.proposal_entities.insert(loc, entities);
            }
            ProposalView::Replace => {
                let entities = spawn_ring_overlay(commands, overlay_assets, loc);
                assembled.proposal_entities.insert(loc, entities);
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

/// Solid-color material handles, one per `Material`, used to recolor cell meshes.
/// These are `GiMaterial` (StandardMaterial + global-illumination extension); the
/// extension's GI volume is rebound by `update_global_illumination`.
#[derive(Resource)]
pub struct MaterialAssets {
    handles: [Handle<GiMaterial>; Material::ALL.len()],
}

impl MaterialAssets {
    pub fn get(&self, material: Material) -> Handle<GiMaterial> {
        self.handles[material as usize].clone()
    }

    /// All material handles (one per `Material` variant).
    pub fn all(&self) -> impl Iterator<Item = &Handle<GiMaterial>> {
        self.handles.iter()
    }
}

/// Startup system: creates one `GiMaterial` per `Material` variant. The GI volume
/// starts as a 1×1×1 zero texture (no GI) until `update_global_illumination` runs.
pub fn spawn_material_assets(
    mut commands: Commands,
    mut materials: ResMut<Assets<GiMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let gi_tex = images.add(default_gi_image());
    let handles = Material::ALL.map(|material| {
        materials.add(GiMaterial {
            base: StandardMaterial {
                base_color: material.color(),
                perceptual_roughness: 0.9,
                ..Default::default()
            },
            extension: GiExtension {
                min_cube: bevy::math::Vec4::ZERO,
                resolution: bevy::math::Vec4::ONE,
                gi_tex: gi_tex.clone(),
            },
        })
    });
    commands.insert_resource(MaterialAssets { handles });
}

/// Furniture cubes (always `Slot::Room`) to highlight in the 3D view for the
/// currently-inspected place. Written by `ui_system`, rendered by
/// `update_place_highlight`.
#[derive(Resource, Default, PartialEq)]
pub struct PlaceHighlight(pub Vec<IVec3>);

/// Mesh + translucent material for place-highlight overlay cuboids.
#[derive(Resource)]
pub struct HighlightAssets {
    mesh: Handle<Mesh>,
    mat: Handle<StandardMaterial>,
}

/// Marks a spawned place-highlight overlay entity for despawn on the next change.
#[derive(Component)]
pub struct PlaceHighlightMarker;

/// Startup system: creates the highlight cuboid mesh and material.
pub fn spawn_highlight_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(bevy::prelude::Cuboid::new(0.95, 0.95, 0.95));
    let mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.3, 0.9, 1.0, 0.35),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..Default::default()
    });
    commands.insert_resource(HighlightAssets { mesh, mat });
}

/// Despawns prior highlight overlays and spawns one translucent cuboid per
/// highlighted furniture cube whenever the highlight set changes.
pub fn update_place_highlight(
    mut commands: Commands,
    highlight: Res<PlaceHighlight>,
    assets: Option<Res<HighlightAssets>>,
    existing: Query<Entity, With<PlaceHighlightMarker>>,
) {
    if !highlight.is_changed() {
        return;
    }
    let Some(assets) = assets else {
        return;
    };
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    for cube in &highlight.0 {
        let center = slot_center(SlotCoord {
            cube: *cube,
            slot: Slot::Room,
        });
        commands.spawn((
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.mat.clone()),
            Transform::from_translation(center),
            PlaceHighlightMarker,
        ));
    }
}

/// Startup system: creates the four world resources from the already-populated EorfList.
pub fn spawn_grid(mut commands: Commands, structure_list: bevy::prelude::Res<EorfList>) {
    let infos = structure_list
        .structures
        .iter()
        .map(|s| s.info.clone())
        .collect();
    let mut constructed = ConstructedCity::new(infos);
    constructed.places = crate::place::load_place_info();
    commands.insert_resource(constructed);
    commands.insert_resource(ProposedCity::new());
    commands.insert_resource(AssembledCity::new());
    commands.insert_resource(ViewableWorld::new());
}

#[cfg(test)]
mod tests {
    use assert2::check;

    use super::{ConstrainedScore, ConstrainedScoreExt};

    #[test]
    fn constrained_score_add_preserves_variant() {
        check!(ConstrainedScore::Exact(3.0).add(1.5) == ConstrainedScore::Exact(4.5));
        check!(
            ConstrainedScore::AtMost { at_most: 5.0 }.add(-2.0)
                == ConstrainedScore::AtMost { at_most: 3.0 }
        );
        check!(
            ConstrainedScore::AtLeast { at_least: 1.0 }.add(0.5)
                == ConstrainedScore::AtLeast { at_least: 1.5 }
        );
    }

    #[test]
    fn constrained_score_subtract_is_neg_add() {
        check!(ConstrainedScore::Exact(5.0).subtract(2.0) == ConstrainedScore::Exact(3.0));
        check!(
            ConstrainedScore::AtLeast { at_least: 4.0 }.subtract(1.0)
                == ConstrainedScore::AtLeast { at_least: 3.0 }
        );
    }

    #[test]
    fn constrained_score_unbound_higher_converts_to_at_least() {
        check!(
            ConstrainedScore::Exact(4.0).unbound_higher()
                == ConstrainedScore::AtLeast { at_least: 4.0 }
        );
        check!(
            ConstrainedScore::AtMost { at_most: 2.0 }.unbound_higher()
                == ConstrainedScore::AtLeast { at_least: 2.0 }
        );
        check!(
            ConstrainedScore::AtLeast { at_least: 7.0 }.unbound_higher()
                == ConstrainedScore::AtLeast { at_least: 7.0 }
        );
    }

    #[test]
    fn constrained_score_unbound_lower_converts_to_at_most() {
        check!(
            ConstrainedScore::Exact(4.0).unbound_lower()
                == ConstrainedScore::AtMost { at_most: 4.0 }
        );
        check!(
            ConstrainedScore::AtLeast { at_least: 2.0 }.unbound_lower()
                == ConstrainedScore::AtMost { at_most: 2.0 }
        );
        check!(
            ConstrainedScore::AtMost { at_most: 7.0 }.unbound_lower()
                == ConstrainedScore::AtMost { at_most: 7.0 }
        );
    }

    #[test]
    fn constrained_score_value_extracts_threshold() {
        check!(ConstrainedScore::Exact(3.0).value() == 3.0);
        check!(ConstrainedScore::AtMost { at_most: 5.0 }.value() == 5.0);
        check!(ConstrainedScore::AtLeast { at_least: 7.0 }.value() == 7.0);
    }

    #[test]
    fn option_constrained_score_propagates_through_none() {
        let none: Option<ConstrainedScore> = None;
        check!(none.add(1.0).is_none());
        check!(none.subtract(1.0).is_none());
        check!(none.unbound_higher().is_none());
        check!(none.unbound_lower().is_none());
    }

    #[test]
    fn option_constrained_score_maps_through_some() {
        let some = Some(ConstrainedScore::Exact(2.0));
        check!(some.add(3.0) == Some(ConstrainedScore::Exact(5.0)));
        check!(some.unbound_lower() == Some(ConstrainedScore::AtMost { at_most: 2.0 }));
    }

    #[test]
    fn constrained_score_chained_ops() {
        let result = ConstrainedScore::Exact(1.0)
            .add(2.0)
            .unbound_higher()
            .add(1.0);
        check!(result == ConstrainedScore::AtLeast { at_least: 4.0 });
    }

    #[test]
    fn months_for_construction_scales_with_population() {
        use crate::sparse3d::{Slot, SlotCoord};
        use bevy::math::IVec3;

        let mut pw = super::ProposedCity::new();
        for x in 0..100 {
            pw.proposed_changes.set(
                SlotCoord {
                    cube: IVec3::new(x, 0, 0),
                    slot: Slot::Room,
                },
                super::Proposal::Remove,
            );
        }
        check!(pw.num_changes() == 100);
        // At population 1, the rate is 80 cells/month (today's behavior): 100 -> 2 months.
        check!(pw.months_for_construction(1) == 2);
        // At population 2, the rate doubles to 160 cells/month: 100 -> 1 month.
        check!(pw.months_for_construction(2) == 1);
        // Population 0 is treated the same as population 1 (never divide by zero).
        check!(pw.months_for_construction(0) == 2);
    }
}
