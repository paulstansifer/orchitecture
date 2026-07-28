use std::collections::HashMap;
use std::f32::consts::TAU;

#[cfg(autotile_matching)]
use crate::autotile::AutotiledMeshes;
use bevy::ecs::system::SystemParam;
use bevy::math::{IVec3, Quat, Vec3};
use bevy::prelude::{
    AlphaMode, Assets, Color, Commands, Component, Entity, Image, Mesh, Mesh3d, MeshMaterial3d,
    Res, ResMut, Resource, SceneRoot, StandardMaterial, Transform,
};
use serde::{Deserialize, Serialize};

use crate::eorf::{EorfId, EorfInfo, EorfList};
use crate::gi_material::{default_gi_image, GiExtension, GiMaterial, ShadowOnlyMaterial};
use crate::resource::{UniformResource, UniqueResource};
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
            Self::Exact(v) => Self::Exact((v + delta).clamp(0.0, 1.0)),
            Self::AtMost { at_most: v } => Self::AtMost {
                at_most: (v + delta).clamp(0.0, 1.0),
            },
            Self::AtLeast { at_least: v } => Self::AtLeast {
                at_least: (v + delta).clamp(0.0, 1.0),
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
    pub order: Option<ConstrainedScore>,
    #[serde(default)]
    pub interest: Option<ConstrainedScore>,
}

/// What a structure is built from. Determines its color. Derived from a cell's
/// `build_material` (see `Cell::material`), never stored.
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
    /// Index into the MaterialList. Serialized with a default fallback.
    #[serde(default)]
    pub build_material: crate::materials::BuildMaterialId,
}

impl Cell {
    /// What this structure is made from, for display. Furniture is always
    /// planks regardless of `build_material`; elements take their build
    /// material's world material.
    pub fn material(
        &self,
        eorfs: &[EorfInfo],
        materials: &crate::materials::MaterialList,
    ) -> Material {
        if eorfs[self.id.as_usize()].is_furniture() {
            Material::Planks
        } else {
            materials
                .materials
                .get(self.build_material.0 as usize)
                .map(|m| m.world_material())
                .unwrap_or_default()
        }
    }
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
    /// Places placed in the world, keyed by stable `PlacedPlaceId`.
    pub placed_places: crate::place::PlacedPlaces,
    pub road_forbidden_zone: bool,
    /// Duplicated from the `Res<>` for simplicity:
    pub eorfs: Vec<EorfInfo>,
    /// Duplicated from the `Res<>` for simplicity:
    pub places: Vec<crate::place::Place>,
    /// The idea DAG, topologically sorted. Loaded from `ideas.ron` at
    /// construction; constant thereafter.
    pub ideas: Vec<crate::idea::Idea>,
    /// `ideas`' dependencies as indices, resolved once.
    pub idea_deps: Vec<Vec<usize>>,
    /// Per-idea *understood* segment masks — derived from `IdeaState`'s learned
    /// masks by `idea::sync_idea_progress`, and cached here so that the many
    /// functions taking only `&ConstructedCity` (place formation, workshop
    /// output) can gate on idea progress without threading a second resource
    /// through their signatures.
    pub understood: Vec<u64>,
    /// Per-instance `ParentRestriction` for furniture, keyed by the cube it's
    /// placed at. Set in the UI; absent means `Unrestricted`. Cleared via
    /// `set_cell`/`take_cell` whenever the furniture there is overwritten or
    /// removed, since the restriction belongs to that specific placement.
    pub furniture_restrictions: HashMap<IVec3, crate::place::ParentRestriction>,
    /// Per-bin `UniformResource` restriction, keyed by the cube the bin is
    /// placed at. Set in the UI; absent means unrestricted (any resource may
    /// be stored there). Cleared via `set_cell`/`take_cell` whenever the
    /// furniture there is overwritten or removed, since the restriction
    /// belongs to that specific placement.
    pub bin_resource_restrictions: HashMap<IVec3, UniformResource>,
    /// Per-rack `RackContents` dedication, keyed by the cube the rack is
    /// placed at. Set in the UI; absent means `RackContents::Tools` (a rack
    /// has no "unrestricted" option, unlike a bin). Cleared via
    /// `set_cell`/`take_cell` whenever the furniture there is overwritten or
    /// removed, since the dedication belongs to that specific placement.
    pub rack_restrictions: HashMap<IVec3, crate::resource::RackContents>,
    /// Per-instance installed-resource slots, keyed by the cube the slotted
    /// furniture is placed at. The vector runs parallel to the furniture type's
    /// `EorfInfo::slots`; a `Some` entry is an installed `UniqueResource`
    /// (withdrawn from public storage). Absent means every slot is empty. When
    /// the furniture is overwritten or removed, `set_cell`/`take_cell` return
    /// any installed resources to storage before clearing the entry.
    pub furniture_slots: HashMap<IVec3, Vec<Option<UniqueResource>>>,
    /// Per-workplace `WorkPriority`, keyed by the workplace place's core cube
    /// (`place::place_location`). Set in the UI; absent means the default
    /// (`WorkPriority::Medium`). Cleared via `set_cell`/`take_cell` whenever the
    /// core furniture there is overwritten or removed, since the priority
    /// belongs to that specific workplace. See `work::assign_work`.
    pub work_priorities: HashMap<IVec3, crate::work::WorkPriority>,
}

impl ConstructedCity {
    pub fn new(eorfs: Vec<EorfInfo>) -> Self {
        // Ideas come straight from the bundled file rather than being installed
        // by a caller (the way `places` is), so every city -- including the ones
        // test helpers build by hand -- has the real DAG available to gate on.
        let ideas = crate::idea::load_idea_info();
        let idea_deps = crate::idea::dep_indices(&ideas);
        let understood = vec![0; ideas.len()];
        ConstructedCity {
            eorfs,
            contents: Sparse3D::new(),
            places: Vec::new(),
            ideas,
            idea_deps,
            understood,
            placed_places: crate::place::PlacedPlaces::default(),
            road_forbidden_zone: true,
            furniture_restrictions: HashMap::new(),
            bin_resource_restrictions: HashMap::new(),
            rack_restrictions: HashMap::new(),
            furniture_slots: HashMap::new(),
            work_priorities: HashMap::new(),
        }
    }

    /// Return any resources installed in the furniture at `cube`'s slots to
    /// public storage, and forget the slot entry. Called before a Room-slot
    /// cell is overwritten or removed so installed items aren't destroyed.
    fn evict_furniture_slots(&mut self, cube: IVec3) {
        if let Some(installed) = self.furniture_slots.remove(&cube) {
            for item in installed.into_iter().flatten() {
                crate::place::deposit_unique(self, item);
            }
        }
    }

    /// Sets a cell, clearing any furniture/bin/rack restriction recorded for the
    /// cube it occupied (the restriction belongs to the previous occupant).
    pub fn set_cell(&mut self, loc: SlotCoord, cell: Cell) {
        if loc.slot == Slot::Room {
            self.furniture_restrictions.remove(&loc.cube);
            self.bin_resource_restrictions.remove(&loc.cube);
            self.rack_restrictions.remove(&loc.cube);
            self.work_priorities.remove(&loc.cube);
            self.evict_furniture_slots(loc.cube);
        }
        self.contents.set(loc, cell);
    }

    /// Removes a cell, clearing any furniture/bin/rack restriction recorded for it.
    pub fn take_cell(&mut self, loc: SlotCoord) -> Option<Cell> {
        if loc.slot == Slot::Room {
            self.furniture_restrictions.remove(&loc.cube);
            self.bin_resource_restrictions.remove(&loc.cube);
            self.rack_restrictions.remove(&loc.cube);
            self.work_priorities.remove(&loc.cube);
            self.evict_furniture_slots(loc.cube);
        }
        self.contents.take(loc)
    }

    /// The resource installed in slot `slot_idx` of the furniture at `cube`, if
    /// any.
    pub fn slot_contents(&self, cube: IVec3, slot_idx: usize) -> Option<&UniqueResource> {
        self.furniture_slots
            .get(&cube)
            .and_then(|v| v.get(slot_idx))
            .and_then(|o| o.as_ref())
    }

    /// Set (or clear, with `None`) the resource in slot `slot_idx` of the
    /// furniture at `cube`. `slot_count` is the furniture type's slot count, so
    /// the backing vector can be sized correctly. Does not touch storage --
    /// callers withdraw/deposit around this. The entry is dropped when every
    /// slot ends up empty, so `furniture_slots` stays absent-means-empty.
    pub fn set_slot(
        &mut self,
        cube: IVec3,
        slot_idx: usize,
        slot_count: usize,
        item: Option<UniqueResource>,
    ) {
        let v = self
            .furniture_slots
            .entry(cube)
            .or_insert_with(|| vec![None; slot_count]);
        if v.len() < slot_count {
            v.resize(slot_count, None);
        }
        v[slot_idx] = item;
        if v.iter().all(Option::is_none) {
            self.furniture_slots.remove(&cube);
        }
    }

    pub fn get_structure_names(&self) -> Vec<String> {
        self.eorfs.iter().map(|s| s.name.clone()).collect()
    }

    pub fn structure_is_room_plop(&self, id: EorfId) -> bool {
        self.eorfs[id.as_usize()].placement_style == crate::eorf::PlacementStyle::RoomPlop
    }

    pub fn structure_is_wall_plop(&self, id: EorfId) -> bool {
        self.eorfs[id.as_usize()].placement_style == crate::eorf::PlacementStyle::WallPlop
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
    /// Cumulative resource units already applied toward the current pending
    /// construction batch. Reset whenever `proposed_changes` is cleared
    /// (`reset()`) or a batch completes (`construct()`). See
    /// `construction::remaining_construction_need`.
    pub resource_progress: HashMap<UniformResource, u32>,
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
            resource_progress: HashMap::new(),
        }
    }

    pub fn num_changes(&self) -> usize {
        self.proposed_changes.iter().count()
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
    pub autotile_results: HashMap<SlotCoord, Vec<crate::autotile::AutotiledMeshes>>,
    /// Entities spawned to visually preview proposals (ghosts + X/ring overlays).
    pub proposal_entities: HashMap<SlotCoord, Vec<Entity>>,
    /// Last-rendered autotile results per proposed-addition location, for change detection.
    #[cfg(autotile_matching)]
    pub proposal_autotile_results: HashMap<SlotCoord, Vec<AutotiledMeshes>>,
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

/// Composes a wall slot's fixed base rotation (`base`) with `WallPlop`'s 180°
/// `flip`, and computes the translation that rotates the mesh around the wall
/// run's center rather than the slot's corner -- otherwise flipping would
/// shift the mesh into the neighboring slot instead of mirroring it in place.
///
/// Wall meshes (see `buildables/wall.scad`, `window.scad`, `doorway.scad`,
/// `column_*.scad`) are all authored with their local origin at the slot's
/// corner and their run (the wall's length) spanning one unit along local
/// `+X`, so `run_offset` is `(0.5, 0, 0)` regardless of slot type.
fn wall_flip_transform(base: Quat, flip: Quat, cube: IVec3) -> (Quat, Vec3) {
    let run_offset = Vec3::new(0.5, 0.0, 0.0);
    let center_offset = run_offset - flip.mul_vec3(run_offset);
    let translation = cube.as_vec3() + base.mul_vec3(center_offset);
    (base * flip, translation)
}

/// Computes the Bevy Transform for a cell at the given grid position.
///
/// For wall slots, `facing` only ever carries a 180° flip (`WallPlop`'s two
/// rotation states -- e.g. `NegX`/`PosX` for `XLoWall`), mirroring the mesh in
/// place; `WallDrag` cells always use the default `Facing` and so render
/// unflipped, matching their pre-`WallPlop` appearance.
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
        Slot::ZLoWall => {
            let flip = if facing == Facing::PosZ {
                TAU / 2.0
            } else {
                0.0
            };
            wall_flip_transform(Quat::IDENTITY, Quat::from_rotation_y(flip), cube)
        }
        Slot::XLoWall => {
            let flip = if facing == Facing::PosX {
                TAU / 2.0
            } else {
                0.0
            };
            wall_flip_transform(ry_neg90, Quat::from_rotation_y(flip), cube)
        }
    };

    Transform {
        translation,
        rotation,
        scale: Vec3::ONE,
    }
}

/// Extra translation needed to place a Wings3D-sourced mesh correctly, given
/// the rotation `cell_transform` already computed for its cell. Wings3D
/// meshes are authored directly in game (Y-up) coordinates, spanning `[0,1]`
/// on every axis. OpenSCAD-derived meshes instead pick up a
/// `rotate([-90,0,0])` Z-up-to-Y-up correction in `build.rs`, which leaves
/// their local Z spanning `[-1,0]` rather than `[0,1]` -- and `cell_transform`
/// was written to place that asymmetric range correctly. Shifting a Wings3D
/// mesh's local Z by -1 before that same rotation (i.e. adding
/// `rotation * (0,0,-1)` to the translation) makes it land the same way.
pub fn wings_offset(rotation: Quat) -> Vec3 {
    rotation * Vec3::new(0.0, 0.0, -1.0)
}

/// Snaps a continuous ground-plane position to the nearest wall boundary for
/// `WallPlop` placement: picks `XLoWall` when `pos.x` sits closer to an
/// integer grid line than `pos.z` does, otherwise `ZLoWall`. Unlike
/// `WallDrag` (which infers the wall axis from drag direction), this lets a
/// single click/plop pick whichever wall is nearest the cursor.
pub fn nearest_wall_slot(pos: Vec3) -> SlotCoord {
    let cube = pos.round().as_ivec3();
    let dx = (pos.x - pos.x.round()).abs();
    let dz = (pos.z - pos.z.round()).abs();
    let slot = if dx <= dz {
        Slot::XLoWall
    } else {
        Slot::ZLoWall
    };
    SlotCoord { cube, slot }
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
            let mut transform = cell_transform(loc.slot, cell.facing, loc.cube);
            if structure_list.is_wings(cell.id) {
                transform.translation += wings_offset(transform.rotation);
            }
            let handle = structure_list.scene_handle(cell.id).clone();
            let entity = commands
                .spawn((SceneRoot(handle), transform, GridCellMarker { loc }))
                .id();
            // TODO(installed-slots): when installed-resource assets exist, spawn a
            // child entity per installed `UniqueResource` here -- one per filled
            // `EorfInfo::slots` entry (looked up in `ConstructedCity::furniture_slots`
            // by `loc.cube`) with a child `Transform` at the slot's
            // `FurnitureSlot::render_offset`, parented to `entity` and pushed into
            // the `cell_entities` vec below. The parent's rotation then applies
            // automatically. (Requires threading the slot data into `apply_changes`.)
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
    /// Shared invisible-but-casts-shadow material for cutaway-hidden cells; see
    /// `crate::gi_material::ShadowOnlyMaterial` and `cutaway::sync_cutaway_shadow_material`.
    shadow_only: Handle<ShadowOnlyMaterial>,
}

impl MaterialAssets {
    pub fn get(&self, material: Material) -> Handle<GiMaterial> {
        self.handles[material as usize].clone()
    }

    /// The shared shadow-only material used for cutaway-hidden geometry.
    pub fn shadow_only(&self) -> Handle<ShadowOnlyMaterial> {
        self.shadow_only.clone()
    }

    /// All material handles (one per `Material` variant).
    pub fn all(&self) -> impl Iterator<Item = &Handle<GiMaterial>> {
        self.handles.iter()
    }

    /// Builds a `MaterialAssets` from bare handles for tests (no `Assets` needed).
    #[cfg(test)]
    pub(crate) fn for_test(
        gi: Handle<GiMaterial>,
        shadow_only: Handle<ShadowOnlyMaterial>,
    ) -> Self {
        MaterialAssets {
            handles: std::array::from_fn(|_| gi.clone()),
            shadow_only,
        }
    }
}

/// Startup system: creates one `GiMaterial` per `Material` variant. The GI volume
/// starts as a 1×1×1 zero texture (no GI) until `update_global_illumination` runs.
pub fn spawn_material_assets(
    mut commands: Commands,
    mut materials: ResMut<Assets<GiMaterial>>,
    mut shadow_only_materials: ResMut<Assets<ShadowOnlyMaterial>>,
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
    let shadow_only = shadow_only_materials.add(ShadowOnlyMaterial::default());
    commands.insert_resource(MaterialAssets {
        handles,
        shadow_only,
    });
}

/// Cells to highlight in the 3D view: the currently-inspected place's
/// furniture (always `Slot::Room`) plus the exact cell that was right-clicked
/// to open the panel, whatever its slot. Written by `ui_system`, rendered by
/// `selection::update_selection_ring`.
#[derive(Resource, Default, PartialEq)]
pub struct PlaceHighlight(pub Vec<SlotCoord>);

/// Startup system: creates the four world resources from the already-populated EorfList.
pub fn spawn_grid(mut commands: Commands, structure_list: bevy::prelude::Res<EorfList>) {
    let infos = structure_list
        .structures
        .iter()
        .map(|s| s.info.clone())
        .collect();
    let mut constructed = ConstructedCity::new(infos);
    constructed.places = crate::place::load_place_info(&constructed.eorfs, &constructed.ideas);
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
    fn constrained_score_json_roundtrip() {
        check!(
            serde_json::from_str::<ConstrainedScore>("1.0").unwrap()
                == ConstrainedScore::Exact(1.0)
        );
        check!(
            serde_json::from_str::<ConstrainedScore>(r#"{"at_most":1.0}"#).unwrap()
                == ConstrainedScore::AtMost { at_most: 1.0 }
        );
        check!(
            serde_json::from_str::<ConstrainedScore>(r#"{"at_least":1.0}"#).unwrap()
                == ConstrainedScore::AtLeast { at_least: 1.0 }
        );
        check!(
            serde_json::to_string(&ConstrainedScore::AtMost { at_most: 2.0 }).unwrap()
                == r#"{"at_most":2.0}"#
        );
    }

    #[test]
    fn constrained_score_add_preserves_variant() {
        check!(ConstrainedScore::Exact(0.3).add(0.1) == ConstrainedScore::Exact(0.4));
        check!(
            ConstrainedScore::AtMost { at_most: 0.5 }.add(-0.2)
                == ConstrainedScore::AtMost { at_most: 0.3 }
        );
        check!(
            ConstrainedScore::AtLeast { at_least: 0.1 }.add(0.05)
                == ConstrainedScore::AtLeast { at_least: 0.15 }
        );
    }

    #[test]
    fn constrained_score_subtract_is_neg_add() {
        check!(ConstrainedScore::Exact(0.5).subtract(0.2) == ConstrainedScore::Exact(0.3));
        check!(
            ConstrainedScore::AtLeast { at_least: 0.4 }.subtract(0.1)
                == ConstrainedScore::AtLeast { at_least: 0.3 }
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
        let some = Some(ConstrainedScore::Exact(0.2));
        check!(some.add(0.3) == Some(ConstrainedScore::Exact(0.5)));
        check!(some.unbound_lower() == Some(ConstrainedScore::AtMost { at_most: 0.2 }));
    }

    #[test]
    fn constrained_score_chained_ops() {
        let result = ConstrainedScore::Exact(0.1)
            .add(0.2)
            .unbound_higher()
            .add(0.1);
        check!(result == ConstrainedScore::AtLeast { at_least: 0.4 });
    }

    // ── cell_transform wall flip ─────────────────────────────────────────────

    #[test]
    fn cell_transform_xlowall_unflipped_matches_pre_wallplop_rotation() {
        use crate::sparse3d::{Facing, Slot};
        use bevy::math::IVec3;

        let t = super::cell_transform(Slot::XLoWall, Facing::NegX, IVec3::new(2, 0, 3));
        check!(t.rotation == bevy::math::Quat::from_rotation_y(-std::f32::consts::TAU / 4.0));
    }

    #[test]
    fn cell_transform_zlowall_unflipped_is_identity() {
        use crate::sparse3d::{Facing, Slot};
        use bevy::math::IVec3;

        let t = super::cell_transform(Slot::ZLoWall, Facing::NegZ, IVec3::new(2, 0, 3));
        check!(t.rotation == bevy::math::Quat::IDENTITY);
    }

    #[test]
    fn cell_transform_wall_flip_mirrors_in_place() {
        use crate::sparse3d::{Facing, Slot};
        use bevy::math::{IVec3, Vec3};

        let cube = IVec3::new(2, 0, 3);

        for (slot, unflipped_facing, flipped_facing) in [
            (Slot::XLoWall, Facing::NegX, Facing::PosX),
            (Slot::ZLoWall, Facing::NegZ, Facing::PosZ),
        ] {
            let unflipped = super::cell_transform(slot, unflipped_facing, cube);
            let flipped = super::cell_transform(slot, flipped_facing, cube);

            // 180° apart in orientation.
            check!(
                (flipped.rotation.angle_between(unflipped.rotation) - std::f32::consts::PI).abs()
                    < 1e-4
            );

            // The mesh's local run-axis endpoints (0,0,0) and (1,0,0) should land
            // on the same pair of world points, just swapped -- i.e. the flip
            // mirrors the mesh in place (within its wall slot) rather than
            // shifting it into the neighboring slot (the bug this test guards
            // against: rotating around the slot's corner instead of its center).
            let unflipped_ends = [
                unflipped.transform_point(Vec3::ZERO),
                unflipped.transform_point(Vec3::X),
            ];
            let flipped_ends = [
                flipped.transform_point(Vec3::ZERO),
                flipped.transform_point(Vec3::X),
            ];
            check!(flipped_ends[0].distance(unflipped_ends[1]) < 1e-4);
            check!(flipped_ends[1].distance(unflipped_ends[0]) < 1e-4);
        }
    }

    // ── nearest_wall_slot ─────────────────────────────────────────────────────

    #[test]
    fn nearest_wall_slot_picks_xlowall_near_x_boundary() {
        use crate::sparse3d::Slot;
        use bevy::math::{IVec3, Vec3};

        let loc = super::nearest_wall_slot(Vec3::new(3.02, 0.0, 3.6));
        check!(loc.slot == Slot::XLoWall);
        check!(loc.cube == IVec3::new(3, 0, 4));
    }

    #[test]
    fn nearest_wall_slot_picks_zlowall_near_z_boundary() {
        use crate::sparse3d::Slot;
        use bevy::math::{IVec3, Vec3};

        let loc = super::nearest_wall_slot(Vec3::new(3.6, 0.0, 3.02));
        check!(loc.slot == Slot::ZLoWall);
        check!(loc.cube == IVec3::new(4, 0, 3));
    }
}
