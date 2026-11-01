use crate::city::{apply_changes, AssembledCity, Cell, ConstructedCity};
use crate::eorf::EorfList;
use crate::evaluation::{fill_default_quality_factors, QualityAspect, QualityFactor};
use crate::materials::BuildMaterialId;
use crate::pathing::{self, NavigationGrid};
use crate::resource::{
    Approximation, Inventory, InventoryEntry, RackContents, StorageKind, ToolKind, UniformResource,
    UniqueResource, UniqueResourceKind,
};
use crate::sparse3d::{Facing, Slot, SlotCoord};
use bevy::math::IVec3;
use bevy::prelude::{Commands, DetectChangesMut, Res, ResMut};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Effect of assigning a worker
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum WorkEffect {
    /// Find the tool installed in the place, and apply its effect
    ToolEffect,
    /// Currently no effect; implement one later!
    TodoEffect,
}

/// Which "need" an `Individual` can satisfy by being assigned to a `Place`.
/// See `population::assign_places`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssignmentFlavor {
    Sleep,
    Work,
}

impl AssignmentFlavor {
    /// Short human-readable label for UI display.
    pub fn label(&self) -> &'static str {
        match self {
            AssignmentFlavor::Sleep => "Sleep",
            AssignmentFlavor::Work => "Work",
        }
    }
}

/// Restricts which `Place` kinds may claim a particular piece of furniture or
/// placed place to fulfill one of their requirements. Attached to the
/// individual instance, not its kind: furniture's lives in
/// `ConstructedCity::furniture_restrictions` (keyed by slot), and a placed
/// place's in `ParticularPlace::restriction`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub enum ParentRestriction {
    /// May be claimed by any `Place` kind that requires it. (Default.)
    #[default]
    Unrestricted,
    /// May never be claimed by any `Place`.
    Excluded,
    /// May only be claimed by the named `Place` kind.
    RestrictedTo(String),
}

impl ParentRestriction {
    fn allows(&self, parent_name: &str) -> bool {
        match self {
            ParentRestriction::Unrestricted => true,
            ParentRestriction::Excluded => false,
            ParentRestriction::RestrictedTo(name) => name == parent_name,
        }
    }
}

/// `Place` kinds (by name) that have a requirement referencing `porf` --
/// i.e. the parent kinds eligible to include this Furniture/Place kind.
/// Used by the UI to populate the restriction dropdown; an empty result means
/// the dropdown shouldn't be shown (nothing could ever include this kind).
pub fn eligible_parent_kinds(places: &[Place], porf: &Porf) -> Vec<String> {
    places
        .iter()
        .filter(|p| {
            p.requirements.iter().any(|r| match (&r.requirement, porf) {
                (Porf::Furniture(a), Porf::Furniture(b)) => a == b,
                (Porf::Place(a), Porf::Place(b)) => a == b,
                // A furniture kind is eligible to nest in a place that wants a
                // tool installed in it, regardless of whether the tool is
                // actually installed yet -- the player may be reserving it
                // for that purpose.
                (Porf::InstalledTool(_, furniture_name), Porf::Furniture(b)) => furniture_name == b,
                _ => false,
            })
        })
        .map(|p| p.name.clone())
        .collect()
}

/// What a `Place` requirement can be fulfilled by: a piece of Furniture, or
/// another (nested) `Place`. Named by the definition it points at.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Porf {
    Furniture(String),
    Place(String),
    /// A piece of furniture of the named kind with a `Tool` of this kind
    /// installed in one of its slots (see `ConstructedCity::furniture_slots`).
    /// Fulfilled by that piece of furniture.
    InstalledTool(ToolKind, String),
}

impl Porf {
    pub fn name(&self) -> &str {
        match self {
            Porf::Furniture(n) | Porf::Place(n) => n,
            Porf::InstalledTool(_, furniture_name) => furniture_name,
        }
    }
}

fn default_weight() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlaceReq {
    pub requirement: Porf,
    pub min: u8,
    pub max: Option<u8>,
    #[serde(default = "default_weight")]
    pub worker_visit_weight: f32,
    #[serde(default = "default_weight")]
    pub worker_visit_duration: f32,
}

/// Display-rounding precision used by the storage UI when a `Place` doesn't
/// specify its own `accounting`.
pub const DEFAULT_STORAGE_ACCOUNTING: Approximation = Approximation {
    digits: 1,
    max: 100,
};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Place {
    pub name: String,
    // First requirement is the core.
    pub requirements: Vec<PlaceReq>,
    /// Whether this place provides storage using whatever storage-capable
    /// furniture (bins, racks, wagons, ...) it contains -- see
    /// `EorfInfo::storage_capacity`, `storage_ids`, `rack_storage_ids`.
    /// `false` means no storage at all, regardless of furniture (e.g. a
    /// dining room that happens to be near some bins).
    #[serde(default)]
    pub public_storage: bool,
    /// Display-rounding precision for the storage UI; falls back to
    /// `DEFAULT_STORAGE_ACCOUNTING` when unset. Meaningless unless
    /// `public_storage` is set.
    #[serde(default)]
    pub accounting: Option<Approximation>,
    #[serde(default)]
    pub quality_factors: Vec<QualityFactor>,
    /// If set, individuals may be assigned to placed instances of this kind
    /// to satisfy this need (e.g. bedrooms are `Sleep`-assignable). See
    /// `population::assign_places`.
    #[serde(default)]
    pub assignable_for: Option<AssignmentFlavor>,
    /// If set, this place kind is a *workplace*: placed instances are staffed
    /// by workers (see `work::assign_work`) and apply this effect, scaled by
    /// how staffed they are, every month (see `work::apply_work_effects`).
    #[serde(default)]
    pub work: Option<WorkEffect>,
    /// If set, this place kind doesn't exist until an idea is understood well
    /// enough, and works at reduced efficiency until it's understood fully.
    #[serde(default)]
    pub gate: Option<IdeaGate>,
}

/// Gates a place kind on progress through an idea (see [`crate::idea`]).
///
/// Below `unlock_at` the place simply can't form -- it's not offered, and
/// arranging the furniture for it does nothing. From `unlock_at` to `full_at`
/// its efficiency ramps linearly from 0 to 1, so a freshly-unlocked workplace
/// is real but produces nothing until understanding pushes past the threshold.
///
/// Since learning is permanent, progress is monotonic and a gate never
/// re-locks.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IdeaGate {
    /// Name of the gating idea; must exist in `ideas.ron`.
    pub idea: String,
    /// Progress at which the place becomes formable, in `0.0..=1.0`.
    pub unlock_at: f32,
    /// Progress at which it reaches full efficiency, in `0.0..=1.0`.
    pub full_at: f32,
}

impl IdeaGate {
    /// The ramp itself, given how far along the gating idea is: `None` below
    /// `unlock_at`, otherwise `0.0..=1.0`. Pure, so the UI can describe a gate
    /// without a `ConstructedCity` in hand.
    pub fn efficiency(&self, progress: f32) -> Option<f32> {
        (progress >= self.unlock_at).then(|| {
            ((progress - self.unlock_at) / (self.full_at - self.unlock_at)).clamp(0.0, 1.0)
        })
    }
}

/// What actually fulfills one slot of a placed `Place`'s requirements. A
/// `Furniture` fulfillment carries the full `SlotCoord` (not just the cube) so
/// that wall-mounted furniture -- e.g. a `WallPlop` chair in a dining room --
/// is identified unambiguously and never confused with room-slot furniture
/// sharing the same cube.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FulfilledPorf {
    Furniture(SlotCoord),
    Place(PlacedPlaceId),
}

/// A placed place instance.
pub struct ParticularPlace {
    /// Index into `ConstructedCity::places`.
    pub place: usize,
    // First fulfillment is the core.
    pub fulfillments: Vec<FulfilledPorf>,
    pub contents: Inventory,
    /// Which `Place` kinds may nest this particular place. Set per-instance
    /// via the UI. See `ParentRestriction`.
    pub restriction: ParentRestriction,
}

/// Stable identifier for a `ParticularPlace`: unique for the lifetime of a
/// city and never reused. Anything that outlives one pass over
/// `placed_places` -- individual assignments, nested-place fulfillments --
/// must hold one of these, because places are removed (dissolved) as the
/// city changes and positional indices would silently shift.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlacedPlaceId(u32);

impl PlacedPlaceId {
    /// This place's shadow priority: a per-instance tiebreak used to order
    /// workplaces within a single priority level (see `work::assign_work`).
    /// Derived from the id for now; may become independently settable later.
    pub fn shadow(&self) -> f32 {
        self.0 as f32
    }

    /// An arbitrary id, for tests that don't need a real `PlacedPlaces`.
    #[cfg(test)]
    pub fn new_for_test(id: u32) -> Self {
        PlacedPlaceId(id)
    }
}

impl std::fmt::Display for PlacedPlaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The placed places of a city, keyed by stable `PlacedPlaceId`.
#[derive(Default)]
pub struct PlacedPlaces {
    items: Vec<(PlacedPlaceId, ParticularPlace)>,
    next_id: u32,
}

impl PlacedPlaces {
    /// A fresh id, never handed out before.
    fn allocate_id(&mut self) -> PlacedPlaceId {
        let id = PlacedPlaceId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn insert(&mut self, place: ParticularPlace) -> PlacedPlaceId {
        let id = self.allocate_id();
        self.items.push((id, place));
        id
    }

    /// Removes and returns the place with `id`. A stale (already-removed) id
    /// is a no-op.
    pub fn remove(&mut self, id: PlacedPlaceId) -> Option<ParticularPlace> {
        let pos = self.items.iter().position(|(i, _)| *i == id)?;
        Some(self.items.remove(pos).1)
    }

    /// Where the place with `id` is: its core furniture's slot, resolved
    /// recursively through nested places. `None` when `id` (or a nested place
    /// it points through) no longer exists.
    pub fn location(&self, id: PlacedPlaceId) -> Option<SlotCoord> {
        match self.get(id)?.fulfillments.first()? {
            FulfilledPorf::Furniture(loc) => Some(*loc),
            FulfilledPorf::Place(inner) => self.location(*inner),
        }
    }

    pub fn get(&self, id: PlacedPlaceId) -> Option<&ParticularPlace> {
        self.items.iter().find(|(i, _)| *i == id).map(|(_, pp)| pp)
    }

    pub fn get_mut(&mut self, id: PlacedPlaceId) -> Option<&mut ParticularPlace> {
        self.items
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, pp)| pp)
    }

    pub fn iter(&self) -> impl Iterator<Item = (PlacedPlaceId, &ParticularPlace)> {
        self.items.iter().map(|(id, pp)| (*id, pp))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (PlacedPlaceId, &mut ParticularPlace)> {
        self.items.iter_mut().map(|(id, pp)| (*id, pp))
    }

    /// Snapshot of every id, for loops that mutate while scanning.
    pub fn ids(&self) -> Vec<PlacedPlaceId> {
        self.items.iter().map(|(id, _)| *id).collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl std::ops::Index<PlacedPlaceId> for PlacedPlaces {
    type Output = ParticularPlace;
    fn index(&self, id: PlacedPlaceId) -> &ParticularPlace {
        self.get(id).expect("no placed place with this id")
    }
}

impl std::ops::IndexMut<PlacedPlaceId> for PlacedPlaces {
    fn index_mut(&mut self, id: PlacedPlaceId) -> &mut ParticularPlace {
        self.get_mut(id).expect("no placed place with this id")
    }
}

/// Loads the place definitions bundled at compile time, panicking on any
/// reference to a furniture, place, or idea name that doesn't exist.
pub fn load_place_info(eorfs: &[crate::eorf::EorfInfo], ideas: &[crate::idea::Idea]) -> Vec<Place> {
    let ron_content = include_str!("../buildables/places.ron");
    let mut infos: Vec<Place> = ron::from_str(ron_content).unwrap();
    for info in &mut infos {
        fill_default_quality_factors(info);
    }
    validate_place_info(&infos, eorfs, ideas);
    infos
}

/// Panics if any place definition cross-references an unknown furniture,
/// place, or idea name -- a typo in places.ron would otherwise just silently
/// never match anything (or, for a gate, silently lock the place forever) --
/// or if place kinds require each other in a cycle.
fn validate_place_info(
    places: &[Place],
    eorfs: &[crate::eorf::EorfInfo],
    ideas: &[crate::idea::Idea],
) {
    let furniture_exists = |name: &str| eorfs.iter().any(|e| e.is_furniture() && e.name == name);
    let place_exists = |name: &str| places.iter().any(|p| p.name == name);

    for (idx, place) in places.iter().enumerate() {
        nesting_level(places, idx);
        let bad = |what: &str, name: &str| {
            panic!(
                "place {:?} references unknown {what} {name:?} in places.ron",
                place.name
            )
        };
        for req in &place.requirements {
            match &req.requirement {
                Porf::Furniture(name) if !furniture_exists(name) => bad("furniture", name),
                Porf::Place(name) if !place_exists(name) => bad("place", name),
                _ => {}
            }
        }
        for factor in &place.quality_factors {
            if let QualityAspect::NumberOf { porf_name } = &factor.aspect {
                if !furniture_exists(porf_name) && !place_exists(porf_name) {
                    bad("furniture or place", porf_name);
                }
            }
        }
        if let Some(gate) = &place.gate {
            if crate::idea::idea_by_name(ideas, &gate.idea).is_none() {
                bad("idea", &gate.idea);
            }
            assert!(
                gate.unlock_at < gate.full_at,
                "place {:?} has a gate whose unlock_at ({}) isn't below its full_at ({})",
                place.name,
                gate.unlock_at,
                gate.full_at
            );
        }
    }
}

/// How well a place kind works, given how far along its gating idea is:
/// `None` when the gate isn't met at all (the place can't form), otherwise a
/// factor in `0.0..=1.0` scaling whatever the place produces. Ungated places
/// are always `Some(1.0)`.
pub fn gate_efficiency(cw: &ConstructedCity, place_idx: usize) -> Option<f32> {
    gate_efficiency_of(cw, &cw.places[place_idx])
}

/// [`gate_efficiency`] by definition rather than index, for the paths that
/// already hold a `&Place`.
pub fn gate_efficiency_of(cw: &ConstructedCity, place: &Place) -> Option<f32> {
    let Some(gate) = &place.gate else {
        return Some(1.0);
    };
    // An unknown idea is rejected by `validate_place_info`, but a hand-built
    // test city may carry places the idea list doesn't know about; treat that
    // as locked rather than panicking mid-frame.
    let idx = crate::idea::idea_by_name(&cw.ideas, &gate.idea)?;
    gate.efficiency(crate::idea::progress(&cw.understood, idx))
}

/// Maximum 2D Manhattan distance (within a single y-layer) for a requirement
/// to count as belonging to a place, on fully open ground. Tunable: the 4×3
/// starting room spans ~5.
pub const PLACE_DIST: i32 = 6;

/// Navigable-distance budget equivalent to `PLACE_DIST`: an open horizontal
/// step costs 2 in `NavigationGrid` terms (a `Nav::Passable(1)` boundary
/// crossing plus a `Nav::Passable(1)` room entry), so this reproduces
/// `PLACE_DIST`'s reach exactly on open floor while letting walls, doors, and
/// stairs actually shape it -- see `reachable_near_anchors`.
const PLACE_NAV_BUDGET: u32 = PLACE_DIST as u32 * 2;

fn manhattan2d(a: IVec3, b: IVec3) -> i32 {
    (a.x - b.x).abs() + (a.z - b.z).abs()
}

/// Every cube walkably reachable, within `PLACE_NAV_BUDGET`, from any of
/// `anchors` -- the navigable-distance replacement for a `PLACE_DIST`
/// Manhattan-radius scan, so walls, doors, and stairs actually shape which
/// cubes are "near" (each anchor itself is always included, at cost 0).
fn reachable_near_anchors(nav: &NavigationGrid, anchors: &[IVec3]) -> HashSet<IVec3> {
    anchors
        .iter()
        .flat_map(|&a| nav.reachable_within(a, PLACE_NAV_BUDGET))
        .collect()
}

/// The world location of a placed place (see `PlacedPlaces::location`);
/// panics on a stale id.
pub fn place_location(cw: &ConstructedCity, id: PlacedPlaceId) -> SlotCoord {
    cw.placed_places
        .location(id)
        .expect("placed place has no resolvable location")
}

/// Slots a piece of furniture can occupy: room-plopped in the cube's interior,
/// or wall-plopped on one of its two low-side walls. (Only `*LoWall` slots are
/// ever stored -- see `nearest_wall_slot` -- so scanning each cube's own low
/// walls visits every wall slot exactly once across the grid.)
const FURNITURE_SLOTS: [Slot; 3] = [Slot::Room, Slot::XLoWall, Slot::ZLoWall];

/// Position of `slot` in `FURNITURE_SLOTS`, for deterministic tie-breaking
/// between furniture sharing a cube.
fn slot_rank(slot: Slot) -> Option<usize> {
    FURNITURE_SLOTS.iter().position(|&s| s == slot)
}

/// Sorts furniture candidates nearest-first (to the closest of `anchors`),
/// then by position, so place formation -- which claims the first few free
/// candidates -- is deterministic and prefers what's closest. (Candidates are
/// gathered from hash sets, whose order varies from run to run.)
fn sort_nearest_first(found: &mut [SlotCoord], anchors: &[IVec3]) {
    found.sort_by_key(|loc| {
        let dist = anchors
            .iter()
            .map(|&a| manhattan2d(a, loc.cube) + (a.y - loc.cube.y).abs())
            .min()
            .unwrap_or(0);
        (
            dist,
            loc.cube.x,
            loc.cube.y,
            loc.cube.z,
            slot_rank(loc.slot),
        )
    });
}

/// All furniture named `name` within navigable reach (see
/// `reachable_near_anchors`) of any of `anchors`, at whatever slot they
/// occupy (room or wall), nearest first. Includes furniture at an anchor
/// itself when it qualifies.
fn furniture_of_name_near(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    anchors: &[IVec3],
    name: &str,
) -> Vec<SlotCoord> {
    let mut found = Vec::new();
    for cube in reachable_near_anchors(nav, anchors) {
        for slot in FURNITURE_SLOTS {
            let loc = SlotCoord { cube, slot };
            if let Some(cell) = cw.contents.get(loc) {
                let info = &cw.eorfs[cell.id.as_usize()];
                if info.is_furniture() && info.name == name && !found.contains(&loc) {
                    found.push(loc);
                }
            }
        }
    }
    sort_nearest_first(&mut found, anchors);
    found
}

/// True if the furniture at `loc` is named `furniture_name` and has a
/// `Tool(kind)` installed in one of its slots.
fn has_installed_tool(
    cw: &ConstructedCity,
    loc: SlotCoord,
    kind: ToolKind,
    furniture_name: &str,
) -> bool {
    let is_named_furniture = cw
        .contents
        .get(loc)
        .is_some_and(|c| cw.eorfs[c.id.as_usize()].name == furniture_name);
    is_named_furniture
        && cw.furniture_slots.get(&loc).is_some_and(|slots| {
            slots
                .iter()
                .flatten()
                .any(|item| *item == UniqueResource::Tool(kind))
        })
}

/// All furniture (anywhere) named `furniture_name` with a `Tool(kind)`
/// installed -- for core-candidate collection in `sync_places`.
fn all_furniture_with_installed_tool(
    cw: &ConstructedCity,
    kind: ToolKind,
    furniture_name: &str,
) -> Vec<SlotCoord> {
    cw.furniture_slots
        .keys()
        .copied()
        .filter(|&loc| has_installed_tool(cw, loc, kind, furniture_name))
        .collect()
}

/// All placed places named `name` within navigable reach (see
/// `reachable_near_anchors`) of any of `anchors` (measured from each
/// candidate's own resolved location).
fn places_of_name_near(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    anchors: &[IVec3],
    name: &str,
) -> Vec<PlacedPlaceId> {
    let reachable = reachable_near_anchors(nav, anchors);
    cw.placed_places
        .iter()
        .filter(|(id, pp)| {
            cw.places[pp.place].name == name && reachable.contains(&place_location(cw, *id).cube)
        })
        .map(|(id, _)| id)
        .collect()
}

/// Count of `Porf`s (furniture or places) named `name` near `origin` -- for
/// the `QualityAspect::NumberOf` factor. Builds its own navigation grid (not
/// shared with any in-progress `sync_places` pass) since it's also called
/// standalone from UI quality-breakdown previews.
pub fn count_named_near(cw: &ConstructedCity, origin: IVec3, name: &str) -> usize {
    let nav = pathing::build_navigation_grid(cw);
    let anchors = [origin];
    furniture_of_name_near(cw, &nav, &anchors, name).len()
        + places_of_name_near(cw, &nav, &anchors, name).len()
}

/// Every cube within reach of a placed instance -- the same reach formation
/// used to gather its furniture, so for a kind whose core requirement is
/// uncapped, that's around its whole chain of cores (see `anchors_for`), not
/// just the ones it owns. Used to paint the "accessible range" overlay for
/// the currently-selected place.
pub fn place_accessible_range(cw: &ConstructedCity, id: PlacedPlaceId) -> Vec<IVec3> {
    let nav = pathing::build_navigation_grid(cw);
    let pp = &cw.placed_places[id];
    let def = &cw.places[pp.place];
    let anchors = match (def.requirements.first(), pp.fulfillments.first()) {
        (Some(core_req), Some(core)) => {
            let cores = eligible_pieces(cw, &cw.placed_places, &core_req.requirement, &def.name);
            cores
                .iter()
                .position(|p| p.porf == *core)
                .map(|seed| anchors_for(&nav, def, &cores, seed))
        }
        _ => None,
    }
    // A place whose core no longer qualifies (it's about to dissolve) still
    // has a location.
    .unwrap_or_else(|| vec![place_location(cw, id).cube]);
    Candidate::new(&nav, pp.place, def, &anchors)
        .reach
        .into_keys()
        .collect()
}

/// The placed place directly holding fulfillment `f`.
fn owner_of(cw: &ConstructedCity, f: &FulfilledPorf) -> Option<PlacedPlaceId> {
    cw.placed_places
        .iter()
        .find(|(_, pp)| pp.fulfillments.contains(f))
        .map(|(id, _)| id)
}

/// The innermost placed place that owns the furniture at `loc`, if any.
pub fn place_id_at(cw: &ConstructedCity, loc: SlotCoord) -> Option<PlacedPlaceId> {
    owner_of(cw, &FulfilledPorf::Furniture(loc))
}

/// The chain of places containing the furniture at `loc`, innermost first, up
/// to the root.
pub fn containing_chain(cw: &ConstructedCity, loc: SlotCoord) -> Vec<PlacedPlaceId> {
    let mut chain = Vec::new();
    let Some(mut id) = place_id_at(cw, loc) else {
        return chain;
    };
    loop {
        chain.push(id);
        match owner_of(cw, &FulfilledPorf::Place(id)) {
            Some(parent) => id = parent,
            None => break,
        }
    }
    chain
}

/// Every piece of furniture named `name` anywhere in the grid (unbounded,
/// unlike `furniture_of_name_near`), at whatever slot it occupies -- so a
/// place may be cored on wall-slot furniture.
fn all_furniture_named(cw: &ConstructedCity, name: &str) -> Vec<SlotCoord> {
    cw.contents
        .iter()
        .filter_map(|(loc, cell)| {
            let info = &cw.eorfs[cell.id.as_usize()];
            (info.is_furniture() && info.name == name).then_some(loc)
        })
        .collect()
}

// ── place formation ─────────────────────────────────────────────────────────

/// How specific a place kind's core requirement is, for deciding which kinds
/// get first pick of contested furniture (see `sync_places`): a core that
/// needs a particular tool installed beats one that's any piece of some
/// furniture. Deliberately rough; ties fall back to `places.ron` order.
fn core_specificity(place: &Place) -> u8 {
    match place.requirements.first().map(|r| &r.requirement) {
        Some(Porf::InstalledTool(..)) => 1,
        Some(Porf::Furniture(_)) | Some(Porf::Place(_)) | None => 0,
    }
}

/// How deeply `places[idx]` nests other places: 0 if it requires no places,
/// otherwise one more than the deepest place kind it requires. Panics if a
/// place kind (transitively) requires itself.
fn nesting_level(places: &[Place], idx: usize) -> usize {
    fn level(places: &[Place], idx: usize, visiting: &mut Vec<usize>) -> usize {
        assert!(
            !visiting.contains(&idx),
            "place {:?} (transitively) requires itself in places.ron",
            places[idx].name
        );
        visiting.push(idx);
        let deepest = places[idx]
            .requirements
            .iter()
            .filter_map(|r| match &r.requirement {
                Porf::Place(name) => places.iter().position(|p| p.name == *name),
                Porf::Furniture(_) | Porf::InstalledTool(..) => None,
            })
            .map(|inner| level(places, inner, visiting) + 1)
            .max()
            .unwrap_or(0);
        visiting.pop();
        deepest
    }
    level(places, idx, &mut Vec::new())
}

/// Every place kind that can currently form (idea-gated kinds below their
/// gate can't), grouped by `nesting_level` -- so an inn's dining room exists
/// before the inn looks for it -- and each level in priority order: most
/// specific core first (see `core_specificity`), then `places.ron` order.
fn formation_levels(cw: &ConstructedCity) -> Vec<Vec<usize>> {
    let mut kinds: Vec<(usize, usize)> = (0..cw.places.len())
        .filter(|&k| !cw.places[k].requirements.is_empty() && gate_efficiency(cw, k).is_some())
        .map(|k| (nesting_level(&cw.places, k), k))
        .collect();
    kinds.sort_by_key(|&(level, k)| (level, std::cmp::Reverse(core_specificity(&cw.places[k])), k));
    let mut levels: Vec<Vec<usize>> = Vec::new();
    for (level, k) in kinds {
        if levels.len() <= level {
            levels.resize(level + 1, Vec::new());
        }
        levels[level].push(k);
    }
    levels
}

/// Something that could fulfill a requirement during formation -- a piece of
/// furniture, or an already-formed place of a lower nesting level -- and
/// where it is.
#[derive(Clone, Copy, Debug)]
struct Piece {
    porf: FulfilledPorf,
    loc: SlotCoord,
}

/// A deterministic order for pieces: by location (then, for nested places
/// sharing a core location, like an inn and its dining room, by id).
fn piece_key(p: &Piece) -> (i32, i32, i32, Option<usize>, Option<PlacedPlaceId>) {
    let id = match p.porf {
        FulfilledPorf::Place(id) => Some(id),
        FulfilledPorf::Furniture(_) => None,
    };
    let c = p.loc.cube;
    (c.x, c.y, c.z, slot_rank(p.loc.slot), id)
}

/// Why the furniture at `loc` doesn't work, or `None` if it does. Broken furniture can't
/// fulfill any place requirement, and the furniture panel shows the reason.
pub fn furniture_problem(cw: &ConstructedCity, loc: SlotCoord) -> Option<String> {
    crate::chimney::hearth_problem(&cw.contents, &cw.eorfs, loc)
}

/// Every piece anywhere fulfilling `req` that a place named `parent_name` is
/// allowed to claim (see `ParentRestriction`), in `piece_key` order. Nested
/// places are drawn from `formed`.
fn eligible_pieces(
    cw: &ConstructedCity,
    formed: &PlacedPlaces,
    req: &Porf,
    parent_name: &str,
) -> Vec<Piece> {
    let furniture = |locs: Vec<SlotCoord>| -> Vec<Piece> {
        locs.into_iter()
            .filter(|loc| {
                cw.furniture_restrictions
                    .get(loc)
                    .is_none_or(|r| r.allows(parent_name))
            })
            .filter(|&loc| furniture_problem(cw, loc).is_none())
            .map(|loc| Piece {
                porf: FulfilledPorf::Furniture(loc),
                loc,
            })
            .collect()
    };
    let mut pieces = match req {
        Porf::Furniture(name) => furniture(all_furniture_named(cw, name)),
        Porf::InstalledTool(kind, name) => {
            furniture(all_furniture_with_installed_tool(cw, *kind, name))
        }
        Porf::Place(name) => formed
            .iter()
            .filter(|(_, pp)| {
                cw.places[pp.place].name == *name && pp.restriction.allows(parent_name)
            })
            .filter_map(|(id, _)| {
                Some(Piece {
                    porf: FulfilledPorf::Place(id),
                    loc: formed.location(id)?,
                })
            })
            .collect(),
    };
    pieces.sort_by_key(piece_key);
    pieces
}

/// Groups `cores` into chains, each linked through members within navigable
/// reach of one another -- so a long row of bins forms one place no matter
/// where along it you look. Each chain lists indices into `cores`, ascending.
fn core_chains(nav: &NavigationGrid, cores: &[Piece]) -> Vec<Vec<usize>> {
    let mut chained = vec![false; cores.len()];
    let mut chains = Vec::new();
    for start in 0..cores.len() {
        if chained[start] {
            continue;
        }
        chained[start] = true;
        let mut chain = vec![start];
        // Only the newest members can reach anything not yet chained.
        let mut frontier = vec![cores[start].loc.cube];
        while !frontier.is_empty() {
            let reach = reachable_near_anchors(nav, &frontier);
            frontier.clear();
            for (i, core) in cores.iter().enumerate() {
                if !chained[i] && reach.contains(&core.loc.cube) {
                    chained[i] = true;
                    chain.push(i);
                    frontier.push(core.loc.cube);
                }
            }
        }
        chain.sort_unstable();
        chains.push(chain);
    }
    chains
}

/// The cubes a place cored on `cores[seed]` measures its reach from: for a
/// kind whose core requirement is uncapped, the whole chain of cores the seed
/// belongs to (see `core_chains`); otherwise just the seed.
fn anchors_for(nav: &NavigationGrid, def: &Place, cores: &[Piece], seed: usize) -> Vec<IVec3> {
    let group = if def.requirements[0].max.is_none() {
        core_chains(nav, cores)
            .into_iter()
            .find(|chain| chain.contains(&seed))
            .unwrap_or_else(|| vec![seed])
    } else {
        vec![seed]
    };
    group.iter().map(|&i| cores[i].loc.cube).collect()
}

/// A place being formed by `sync_places`.
struct Candidate {
    kind: usize,
    /// Walking cost from the place's anchors (see `anchors_for`) to every cube
    /// within reach.
    reach: HashMap<IVec3, u32>,
    /// What it has claimed, per requirement (the core requirement first, its
    /// seed first within that).
    claimed: Vec<Vec<Piece>>,
}

impl Candidate {
    fn new(nav: &NavigationGrid, kind: usize, def: &Place, anchors: &[IVec3]) -> Self {
        let mut reach = nav.costs_from_within(anchors, PLACE_NAV_BUDGET);
        // Even an impassable anchor is part of its own place.
        for &a in anchors {
            reach.insert(a, 0);
        }
        Candidate {
            kind,
            reach,
            claimed: vec![Vec::new(); def.requirements.len()],
        }
    }

    /// Claims `seed` as the core, then the nearest free pieces within reach
    /// until every requirement has its minimum, adding the claims to `taken`.
    /// Claims nothing and returns `false` if some minimum can't be met.
    fn claim_minimums(
        &mut self,
        def: &Place,
        pieces: &[Vec<Piece>],
        seed: Piece,
        taken: &mut HashSet<FulfilledPorf>,
    ) -> bool {
        let mut tentative = HashSet::from([seed.porf]);
        self.claimed[0].push(seed);
        for (r, req) in def.requirements.iter().enumerate() {
            let need = (req.min as usize).saturating_sub(self.claimed[r].len());
            let mut nearest: Vec<(u32, Piece)> = pieces[r]
                .iter()
                .filter(|p| !taken.contains(&p.porf) && !tentative.contains(&p.porf))
                .filter_map(|p| Some((*self.reach.get(&p.loc.cube)?, *p)))
                .collect();
            if nearest.len() < need {
                self.claimed.iter_mut().for_each(Vec::clear);
                return false;
            }
            // Stable, so equally-near pieces stay in `piece_key` order.
            nearest.sort_by_key(|&(dist, _)| dist);
            for (_, p) in nearest.into_iter().take(need) {
                tentative.insert(p.porf);
                self.claimed[r].push(p);
            }
        }
        taken.extend(tentative);
        true
    }

    fn fulfillments(&self) -> Vec<FulfilledPorf> {
        self.claimed.iter().flatten().map(|p| p.porf).collect()
    }
}

/// Forms the places of one nesting level, whose kinds are `kinds` in priority
/// order, from pieces not already in `taken` (and adds what they claim to
/// it). Nested places are drawn from `formed`, the lower levels' results.
///
/// Two phases:
/// 1. In priority order, each potential place claims only the nearest free
///    pieces it needs to meet its minimums; if it can't, it doesn't form and
///    claims nothing.
/// 2. Every piece still free goes to the nearest formed place that can use it
///    and has room (under its `max`), the higher-priority place winning ties.
fn form_level(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    formed: &PlacedPlaces,
    kinds: &[usize],
    taken: &mut HashSet<FulfilledPorf>,
) -> Vec<Candidate> {
    let pieces: HashMap<usize, Vec<Vec<Piece>>> = kinds
        .iter()
        .map(|&k| {
            let def = &cw.places[k];
            let per_req = def
                .requirements
                .iter()
                .map(|r| eligible_pieces(cw, formed, &r.requirement, &def.name))
                .collect();
            (k, per_req)
        })
        .collect();

    let mut places: Vec<Candidate> = Vec::new();
    for &k in kinds {
        let def = &cw.places[k];
        let cores = &pieces[&k][0];
        let groups: Vec<Vec<usize>> = if def.requirements[0].max.is_none() {
            core_chains(nav, cores)
        } else {
            (0..cores.len()).map(|i| vec![i]).collect()
        };
        for group in groups {
            let Some(&seed) = group.iter().find(|&&i| !taken.contains(&cores[i].porf)) else {
                continue;
            };
            let anchors: Vec<IVec3> = group.iter().map(|&i| cores[i].loc.cube).collect();
            let mut place = Candidate::new(nav, k, def, &anchors);
            if place.claim_minimums(def, &pieces[&k], cores[seed], taken) {
                places.push(place);
            }
        }
    }

    // `places` is already in priority order, so its index breaks ties.
    let mut offers: Vec<(u32, usize, usize, Piece)> = Vec::new();
    for (pi, place) in places.iter().enumerate() {
        for (r, req_pieces) in pieces[&place.kind].iter().enumerate() {
            for p in req_pieces.iter().filter(|p| !taken.contains(&p.porf)) {
                if let Some(&dist) = place.reach.get(&p.loc.cube) {
                    offers.push((dist, pi, r, *p));
                }
            }
        }
    }
    offers.sort_by_key(|(dist, pi, r, p)| (*dist, *pi, *r, piece_key(p)));
    for (_, pi, r, p) in offers {
        let max = cw.places[places[pi].kind].requirements[r]
            .max
            .map_or(usize::MAX, usize::from);
        if places[pi].claimed[r].len() < max && taken.insert(p.porf) {
            places[pi].claimed[r].push(p);
        }
    }
    places
}

/// Matches each newly formed place to the old place of the same kind it
/// overlaps most (preferring the one that held its core), so that ids -- and
/// everything keyed by them: homes, jobs, nesting restrictions, inventories
/// -- survive re-forming a place that didn't really change. `matched` holds
/// old places already spoken for.
fn carry_over_ids(
    cw: &ConstructedCity,
    formed: &[Candidate],
    fulfillments: &[Vec<FulfilledPorf>],
    old_owner: &HashMap<FulfilledPorf, PlacedPlaceId>,
    matched: &mut HashSet<PlacedPlaceId>,
) -> Vec<Option<PlacedPlaceId>> {
    // (held the core, overlap, new place, old place)
    let mut pairs: Vec<(bool, usize, usize, PlacedPlaceId)> = Vec::new();
    for (i, fs) in fulfillments.iter().enumerate() {
        let mut overlap: HashMap<PlacedPlaceId, usize> = HashMap::new();
        for f in fs {
            if let Some(&old) = old_owner.get(f) {
                if cw.placed_places[old].place == formed[i].kind {
                    *overlap.entry(old).or_default() += 1;
                }
            }
        }
        let core_owner = fs.first().and_then(|f| old_owner.get(f));
        for (old, n) in overlap {
            pairs.push((core_owner == Some(&old), n, i, old));
        }
    }
    pairs.sort_by_key(|&(core, n, i, old)| (std::cmp::Reverse(core), std::cmp::Reverse(n), i, old));
    let mut ids = vec![None; formed.len()];
    for (_, _, i, old) in pairs {
        if ids[i].is_none() && matched.insert(old) {
            ids[i] = Some(old);
        }
    }
    ids
}

/// A fresh, empty inventory sized by the storage capacity of `fulfillments`'
/// furniture.
fn inventory_for(cw: &ConstructedCity, fulfillments: &[FulfilledPorf]) -> Inventory {
    let capacity_for = |kind: StorageKind| -> f32 {
        fulfillments
            .iter()
            .map(|f| match f {
                FulfilledPorf::Furniture(loc) => slot_storage_capacity(cw, *loc, kind),
                FulfilledPorf::Place(_) => 0.0,
            })
            .sum()
    };
    Inventory::new([
        (StorageKind::Bulk, capacity_for(StorageKind::Bulk)),
        (StorageKind::Rack, capacity_for(StorageKind::Rack)),
        (StorageKind::Book, capacity_for(StorageKind::Book)),
    ])
}

/// Re-forms every `Place` in `cw` from scratch, so the result depends only on
/// what's built where -- never on the order it was built in.
///
/// Kinds form level by level (see `formation_levels`); within a level, more
/// specific kinds get first pick, each place first claiming only what it
/// needs to meet its minimums, and then everything left over goes to the
/// nearest place that can use it (see `form_level`).
///
/// Each resulting place takes over the id of the old place it most resembles
/// (see `carry_over_ids`). One whose furniture is unchanged keeps its
/// `Inventory` as is; the goods of any other old place (one that changed
/// shape, merged into another, or dissolved) go back into storage wherever
/// there's room, and whatever doesn't fit is lost. Returns `true` if anything
/// changed.
pub fn sync_places(cw: &mut ConstructedCity) -> bool {
    // Built once up front: `cw.contents` (what the grid is derived from) is
    // never mutated over the course of this pass.
    let nav = pathing::build_navigation_grid(cw);
    let old_owner: HashMap<FulfilledPorf, PlacedPlaceId> = cw
        .placed_places
        .iter()
        .flat_map(|(id, pp)| pp.fulfillments.iter().map(move |f| (*f, id)))
        .collect();

    let mut next = PlacedPlaces {
        items: Vec::new(),
        next_id: cw.placed_places.next_id,
    };
    let mut taken: HashSet<FulfilledPorf> = HashSet::new();
    let mut matched: HashSet<PlacedPlaceId> = HashSet::new();
    for kinds in formation_levels(cw) {
        let formed = form_level(cw, &nav, &next, &kinds, &mut taken);
        let fulfillments: Vec<Vec<FulfilledPorf>> =
            formed.iter().map(Candidate::fulfillments).collect();
        let ids = carry_over_ids(cw, &formed, &fulfillments, &old_owner, &mut matched);
        for ((place, fulfillments), old) in formed.iter().zip(fulfillments).zip(ids) {
            let id = old.unwrap_or_else(|| next.allocate_id());
            let restriction = old
                .map(|old| cw.placed_places[old].restriction.clone())
                .unwrap_or_default();
            next.items.push((
                id,
                ParticularPlace {
                    place: place.kind,
                    fulfillments,
                    contents: Inventory::new([]),
                    restriction,
                },
            ));
        }
    }
    next.items.sort_by_key(|(id, _)| *id);

    let unchanged = next.len() == cw.placed_places.len()
        && next.iter().all(|(id, pp)| {
            cw.placed_places
                .get(id)
                .is_some_and(|old| old.place == pp.place && old.fulfillments == pp.fulfillments)
        });
    if unchanged {
        return false;
    }

    let mut old_places = std::mem::take(&mut cw.placed_places);
    let mut spilled: Vec<InventoryEntry> = Vec::new();
    for (id, pp) in next.iter_mut() {
        match old_places.remove(id) {
            Some(old) if old.fulfillments == pp.fulfillments => pp.contents = old.contents,
            old => {
                pp.contents = inventory_for(cw, &pp.fulfillments);
                match old {
                    Some(old) => spilled.extend(old.contents.into_entries()),
                    None if cw.places[pp.place].name == "camp" => {
                        pp.contents.add_uniform(UniformResource::Plank, 18);
                        pp.contents.add_uniform(UniformResource::Canvas, 6);
                        pp.contents.add_uniform(UniformResource::Potato, 20);
                    }
                    None => {}
                }
            }
        }
    }
    for (_, old) in old_places.items {
        spilled.extend(old.contents.into_entries());
    }
    cw.placed_places = next;
    for entry in spilled {
        match entry {
            InventoryEntry::Uniform(res, qty) => {
                deposit_uniform_with_capacity(cw, res, qty);
            }
            InventoryEntry::Collection(items) => {
                for item in items {
                    deposit_unique(cw, item);
                }
            }
        }
    }
    true
}

/// Place kinds (indices into `cw.places`) that could form around the
/// furniture at `loc`, ignoring competition from other places: those it can
/// be the core of, with enough of everything else within reach to meet their
/// minimums.
pub fn valid_places_for(cw: &ConstructedCity, loc: SlotCoord) -> Vec<usize> {
    let nav = pathing::build_navigation_grid(cw);
    (0..cw.places.len())
        .filter(|&k| {
            let def = &cw.places[k];
            if def.requirements.is_empty() || gate_efficiency(cw, k).is_none() {
                return false;
            }
            let pieces: Vec<Vec<Piece>> = def
                .requirements
                .iter()
                .map(|r| eligible_pieces(cw, &cw.placed_places, &r.requirement, &def.name))
                .collect();
            let cores = &pieces[0];
            let Some(seed) = cores.iter().position(|p| p.loc == loc) else {
                return false;
            };
            let anchors = anchors_for(&nav, def, cores, seed);
            Candidate::new(&nav, k, def, &anchors).claim_minimums(
                def,
                &pieces,
                cores[seed],
                &mut HashSet::new(),
            )
        })
        .collect()
}

/// Re-runs `sync_places` whenever `ConstructedCity` changes (e.g. after an
/// edit is constructed). Mutates through `bypass_change_detection` and only
/// calls `set_changed` when something actually changed, for the same reason
/// `sync_homes` does (see `population.rs`): an unconditional mark would make
/// this system re-trigger itself on every frame forever after the first
/// real change.
pub fn sync_places_system(mut constructed: ResMut<ConstructedCity>) {
    let changed = sync_places(constructed.bypass_change_detection());
    if changed {
        constructed.set_changed();
    }
}

/// Total number of tools of `kind` held across all rack storage places.
pub fn total_tools_of(cw: &ConstructedCity, kind: ToolKind) -> u32 {
    rack_storage_ids(cw)
        .into_iter()
        .map(|id| cw.placed_places[id].contents.tool_count_of(kind) as u32)
        .sum()
}

/// Total number of tools (of any kind) held across all rack storage places.
pub fn total_tool_count(cw: &ConstructedCity) -> u32 {
    rack_storage_ids(cw)
        .into_iter()
        .map(|id| cw.placed_places[id].contents.tool_count() as u32)
        .sum()
}

/// Total number of rugs held across all rack storage places.
pub fn total_rug_count(cw: &ConstructedCity) -> u32 {
    rack_storage_ids(cw)
        .into_iter()
        .map(|id| cw.placed_places[id].contents.rug_count() as u32)
        .sum()
}

/// Total number of books held across all (bookcase-backed) storage places.
pub fn total_book_count(cw: &ConstructedCity) -> u32 {
    storage_ids(cw)
        .into_iter()
        .map(|id| cw.placed_places[id].contents.book_count() as u32)
        .sum()
}

/// Total book (`StorageKind::Book`) capacity across all storage places -- the
/// combined shelf space of every bookcase in a `public_storage` place. Unlike
/// racks, bookcases have no per-cube dedication, so this just sums their
/// capacity.
pub fn book_capacity(cw: &ConstructedCity) -> f32 {
    storage_ids(cw)
        .into_iter()
        .flat_map(|id| cw.placed_places[id].fulfillments.clone())
        .filter_map(|f| match f {
            FulfilledPorf::Furniture(loc) => {
                Some(slot_storage_capacity(cw, loc, StorageKind::Book))
            }
            FulfilledPorf::Place(_) => None,
        })
        .sum()
}

/// Total capacity (not just free room) dedicated to `contents` across all
/// rack storage places -- for display, alongside `rack_free_capacity`.
pub fn rack_capacity(cw: &ConstructedCity, contents: RackContents) -> f32 {
    rack_storage_ids(cw)
        .into_iter()
        .map(|id| rack_capacity_ceiling(cw, &cw.placed_places[id], contents))
        .sum()
}

/// Remove one tool of `kind` from the first rack storage place that holds one.
/// Returns `true` if a tool was removed.
pub fn consume_tool(cw: &mut ConstructedCity, kind: ToolKind) -> bool {
    for id in rack_storage_ids(cw) {
        if cw.placed_places[id]
            .contents
            .remove_unique(&UniqueResource::Tool(kind))
        {
            return true;
        }
    }
    false
}

/// Deposit one tool of `kind` into the first rack storage place dedicated to
/// `Tools` with room for it. Returns `true` on success (`false` if there's no
/// such rack, or none with room).
pub fn deposit_tool(cw: &mut ConstructedCity, kind: ToolKind) -> bool {
    for id in rack_storage_ids(cw) {
        if rack_free_capacity_for(cw, &cw.placed_places[id], RackContents::Tools) >= 1.0 {
            cw.placed_places[id]
                .contents
                .add_unique(UniqueResource::Tool(kind));
            return true;
        }
    }
    false
}

/// Every `UniqueResource` of `kind` currently held in public storage -- the
/// pool the install pop-up offers to draw from. Installed resources have
/// already been withdrawn, so they don't appear here.
pub fn available_uniques_of_kind(
    cw: &ConstructedCity,
    kind: UniqueResourceKind,
) -> Vec<UniqueResource> {
    storage_ids(cw)
        .into_iter()
        .flat_map(|id| {
            cw.placed_places[id]
                .contents
                .unique_items()
                .filter(|item| kind.matches(item))
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Remove one unique equal to `item` from the first storage place that holds
/// it (used when installing it into a furniture slot). Returns `true` if one
/// was removed.
pub fn withdraw_unique(cw: &mut ConstructedCity, item: &UniqueResource) -> bool {
    for id in storage_ids(cw) {
        if cw.placed_places[id].contents.remove_unique(item) {
            return true;
        }
    }
    false
}

/// Return `item` to public storage (used when removing it from a slot, or when
/// the furniture holding it is removed/overwritten). Books go to bookcases,
/// tools/rugs to their dedicated racks. Returns `false` (dropping the item) if
/// there's nowhere with room -- the caller has no better recourse.
pub fn deposit_unique(cw: &mut ConstructedCity, item: UniqueResource) -> bool {
    match item {
        UniqueResource::Tool(_) => deposit_into_rack(cw, item, RackContents::Tools),
        UniqueResource::Rug { .. } => deposit_into_rack(cw, item, RackContents::Rugs),
        UniqueResource::Book { .. } => deposit_into_bookcase(cw, item),
    }
}

/// Deposit `item` into the first rack storage place dedicated to `contents`
/// with room for it.
fn deposit_into_rack(
    cw: &mut ConstructedCity,
    item: UniqueResource,
    contents: RackContents,
) -> bool {
    for id in rack_storage_ids(cw) {
        if rack_free_capacity_for(cw, &cw.placed_places[id], contents) >= 1.0 {
            cw.placed_places[id].contents.add_unique(item);
            return true;
        }
    }
    false
}

/// Deposit a book into the first bookcase-backed storage place with room.
fn deposit_into_bookcase(cw: &mut ConstructedCity, item: UniqueResource) -> bool {
    let volume = item.volume();
    for id in storage_ids(cw) {
        if book_free_capacity_for(cw, &cw.placed_places[id]) >= volume {
            cw.placed_places[id].contents.add_unique(item);
            return true;
        }
    }
    false
}

/// Shelf volume still free in one storage place: whichever is smaller of the
/// room left on its own bookcases and the room left in its inventory's `Book`
/// pool.
fn book_free_capacity_for(cw: &ConstructedCity, pp: &ParticularPlace) -> f32 {
    let ceiling: f32 = pp
        .fulfillments
        .iter()
        .filter_map(|f| match f {
            FulfilledPorf::Furniture(loc) => {
                Some(slot_storage_capacity(cw, *loc, StorageKind::Book))
            }
            FulfilledPorf::Place(_) => None,
        })
        .sum();
    let current = pp.contents.book_count() as f32 * book_volume();
    (ceiling - current)
        .min(pp.contents.remaining_capacity(StorageKind::Book))
        .max(0.0)
}

/// The shelf volume one book occupies.
pub fn book_volume() -> f32 {
    UniqueResource::Book {
        title: String::new(),
    }
    .volume()
}

/// Shelf volume still free across all storage places -- the book counterpart of
/// `rack_free_capacity`, used to decide whether a book a traveler is offering
/// has anywhere to go.
pub fn book_free_capacity(cw: &ConstructedCity) -> f32 {
    storage_ids(cw)
        .into_iter()
        .map(|id| book_free_capacity_for(cw, &cw.placed_places[id]))
        .sum()
}

fn is_public_storage(cw: &ConstructedCity, pp: &ParticularPlace) -> bool {
    cw.places
        .get(pp.place)
        .is_some_and(|info| info.public_storage)
}

/// The storage capacity the furniture at `loc` contributes for `kind` (0.0 if
/// it has none, or nothing is there). Slot-precise, so a wall-mounted chair
/// contributes nothing even when a storage cube shares its coordinate.
fn slot_storage_capacity(cw: &ConstructedCity, loc: SlotCoord, kind: StorageKind) -> f32 {
    cw.contents
        .get(loc)
        .map(|cell| cw.eorfs[cell.id.as_usize()].storage_capacity_for(kind))
        .unwrap_or(0.0)
}

/// Every `public_storage` place, in placement order -- used both for
/// `UniformResource` (bin-backed) and `UniqueResource` (rack-backed) storage,
/// since a single place's furniture (e.g. a wagon) may provide both at once.
pub fn storage_ids(cw: &ConstructedCity) -> Vec<PlacedPlaceId> {
    cw.placed_places
        .iter()
        .filter(|(_, pp)| is_public_storage(cw, pp))
        .map(|(id, _)| id)
        .collect()
}

/// Same set as `storage_ids`, kept as a separate name for callers concerned
/// specifically with rack (`UniqueResource`) storage.
pub fn rack_storage_ids(cw: &ConstructedCity) -> Vec<PlacedPlaceId> {
    storage_ids(cw)
}

/// Per-cube capacity ceiling for `contents` within `pp`: the `Rack` storage
/// capacity of every fulfillment dedicated to `contents` (racks have no
/// "unrestricted" option -- an unset cube defaults to `RackContents::Tools`).
fn rack_capacity_ceiling(
    cw: &ConstructedCity,
    pp: &ParticularPlace,
    contents: RackContents,
) -> f32 {
    pp.fulfillments
        .iter()
        .filter_map(|f| match f {
            FulfilledPorf::Furniture(loc) => {
                (cw.rack_restrictions.get(loc).copied().unwrap_or_default() == contents)
                    .then(|| slot_storage_capacity(cw, *loc, StorageKind::Rack))
            }
            FulfilledPorf::Place(_) => None,
        })
        .sum()
}

/// Free volume `pp` can currently accept for `contents` (Tools or Rugs):
/// bounded by its racks' dedication (`rack_capacity_ceiling`) and by the
/// place's own `Rack`-pool volume (shared with Bulk goods only through the
/// same furniture's total capacity, never through their contents).
fn rack_free_capacity_for(
    cw: &ConstructedCity,
    pp: &ParticularPlace,
    contents: RackContents,
) -> f32 {
    let current = match contents {
        RackContents::Tools => pp.contents.tool_count(),
        RackContents::Rugs => pp.contents.rug_count(),
    } as f32;
    let ceiling_free = (rack_capacity_ceiling(cw, pp, contents) - current).max(0.0);
    ceiling_free.min(pp.contents.remaining_capacity(StorageKind::Rack))
}

/// Free volume available for `contents` (Tools or Rugs) across all rack
/// storage places.
pub fn rack_free_capacity(cw: &ConstructedCity, contents: RackContents) -> f32 {
    rack_storage_ids(cw)
        .into_iter()
        .map(|id| rack_free_capacity_for(cw, &cw.placed_places[id], contents))
        .sum()
}

/// True if the furniture at `loc` is a fulfillment (with `Bulk` capacity) of
/// some `public_storage` place -- used by the UI to decide whether to offer a
/// per-bin resource-restriction dropdown.
pub fn is_storage_bin(cw: &ConstructedCity, loc: SlotCoord) -> bool {
    slot_storage_capacity(cw, loc, StorageKind::Bulk) > 0.0 && in_public_storage(cw, loc)
}

/// True if the furniture at `loc` is a fulfillment (with `Rack` capacity) of
/// some `public_storage` place -- used by the UI to decide whether to offer a
/// per-rack contents dropdown.
pub fn is_rack(cw: &ConstructedCity, loc: SlotCoord) -> bool {
    slot_storage_capacity(cw, loc, StorageKind::Rack) > 0.0 && in_public_storage(cw, loc)
}

fn in_public_storage(cw: &ConstructedCity, loc: SlotCoord) -> bool {
    let f = FulfilledPorf::Furniture(loc);
    cw.placed_places
        .iter()
        .any(|(_, pp)| is_public_storage(cw, pp) && pp.fulfillments.contains(&f))
}

/// Number of furniture pieces named `furniture_name` fulfilling placed places
/// whose place type is named `place_name` (e.g. how many "market stand"
/// furniture are placed across all "market" places).
pub fn count_furniture_named_in_places(
    cw: &ConstructedCity,
    furniture_name: &str,
    place_name: &str,
) -> usize {
    cw.placed_places
        .iter()
        .filter(|(_, pp)| cw.places[pp.place].name == place_name)
        .flat_map(|(_, pp)| pp.fulfillments.iter())
        .filter(|f| match f {
            FulfilledPorf::Furniture(loc) => cw.contents.get(*loc).is_some_and(|cell| {
                let info = &cw.eorfs[cell.id.as_usize()];
                info.is_furniture() && info.name == furniture_name
            }),
            FulfilledPorf::Place(_) => false,
        })
        .count()
}

/// Number of market stands placed across all market places — each stand can
/// host one visiting farm per month.
pub fn market_stand_count(cw: &ConstructedCity) -> usize {
    count_furniture_named_in_places(cw, "market stand", "market")
}

/// Cubes of every market stand placed in a market place.
pub fn market_stand_cubes(cw: &ConstructedCity) -> Vec<IVec3> {
    cw.placed_places
        .iter()
        .filter(|(_, pp)| cw.places[pp.place].name == "market")
        .flat_map(|(_, pp)| pp.fulfillments.iter())
        .filter_map(|f| match f {
            FulfilledPorf::Furniture(loc) => cw
                .contents
                .get(*loc)
                .is_some_and(|cell| cw.eorfs[cell.id.as_usize()].name == "market stand")
                .then_some(loc.cube),
            FulfilledPorf::Place(_) => None,
        })
        .collect()
}

/// Every bedroom nested directly in an inn: the guest rooms that visiting
/// farmers and travelers stay in (and that residents therefore don't sleep in;
/// see `population::assign_places`). Ascending by id.
pub fn inn_room_ids(cw: &ConstructedCity) -> Vec<PlacedPlaceId> {
    let mut rooms: Vec<PlacedPlaceId> = cw
        .placed_places
        .iter()
        .filter(|(_, pp)| cw.places[pp.place].name == "inn")
        .flat_map(|(_, pp)| pp.fulfillments.iter())
        .filter_map(|f| match f {
            FulfilledPorf::Place(id) => cw
                .placed_places
                .get(*id)
                .is_some_and(|room| cw.places[room.place].name == "bedroom")
                .then_some(*id),
            FulfilledPorf::Furniture(_) => None,
        })
        .collect();
    rooms.sort();
    rooms
}

/// Total quantity of `res` held across all storage places.
pub fn total_uniform(cw: &ConstructedCity, res: UniformResource) -> u32 {
    storage_ids(cw)
        .into_iter()
        .flat_map(|id| cw.placed_places[id].contents.uniform_totals())
        .filter(|(r, _)| *r == res)
        .map(|(_, q)| q)
        .sum()
}

/// Remove `qty` of `res` from storage places, spreading the deduction across
/// places in order. Returns `true` and commits if the total held is ≥ `qty`;
/// returns `false` and makes no changes otherwise.
pub fn consume_uniform(cw: &mut ConstructedCity, res: UniformResource, qty: u32) -> bool {
    if total_uniform(cw, res) < qty {
        return false;
    }
    let mut remaining = qty;
    for id in storage_ids(cw) {
        if remaining == 0 {
            break;
        }
        let here = cw.placed_places[id]
            .contents
            .uniform_totals()
            .into_iter()
            .find(|(r, _)| *r == res)
            .map(|(_, q)| q)
            .unwrap_or(0);
        let take = here.min(remaining);
        if take > 0 {
            cw.placed_places[id].contents.subtract_uniform(res, take);
            remaining -= take;
        }
    }
    true
}

/// Current per-resource totals held across all storage places (raw,
/// unrounded — for internal calculations, unlike the display-oriented
/// [`place_resource_totals`]).
pub fn storage_totals(cw: &ConstructedCity) -> HashMap<UniformResource, u32> {
    let mut totals = HashMap::new();
    for id in storage_ids(cw) {
        for (res, qty) in cw.placed_places[id].contents.uniform_totals() {
            *totals.entry(res).or_insert(0) += qty;
        }
    }
    totals
}

/// Totals of all resources across every storage place, rounded per each
/// place's accounting precision and sorted for display (unlike the raw
/// [`storage_totals`]). Returns `(resource, total_quantity, precision)`.
pub fn place_resource_totals(
    constructed: &ConstructedCity,
) -> Vec<(UniformResource, u32, crate::resource::Precision)> {
    use crate::resource::{round, Precision};

    let mut map: HashMap<UniformResource, (u32, Precision)> = HashMap::new();
    for (_, place) in constructed.placed_places.iter() {
        let Some(info) = constructed.places.get(place.place) else {
            continue;
        };
        if !info.public_storage {
            continue;
        }
        let accounting = info.accounting.unwrap_or(DEFAULT_STORAGE_ACCOUNTING);
        for (res, qty) in place.contents.uniform_totals() {
            let (rounded, precision) = round(qty, accounting);
            let entry = map.entry(res).or_insert((0, Precision::Exact));
            entry.0 += rounded;
            if precision != Precision::Exact {
                entry.1 = precision;
            }
        }
    }
    let mut result: Vec<_> = map.into_iter().map(|(r, (q, p))| (r, q, p)).collect();
    result.sort_by_key(|(r, _, _)| *r);
    result
}

/// Per-bin capacity ceiling for `res` within `pp`: the `Bulk` storage
/// capacity of every fulfillment dedicated to `res`. Bins dedicated to a
/// different resource, and bins not yet assigned a resource ("unused"),
/// contribute nothing.
fn place_capacity_ceiling(cw: &ConstructedCity, pp: &ParticularPlace, res: UniformResource) -> f32 {
    pp.fulfillments
        .iter()
        .filter_map(|f| match f {
            FulfilledPorf::Furniture(loc) => cw
                .bin_resource_restrictions
                .get(loc)
                .is_some_and(|r| *r == res)
                .then(|| slot_storage_capacity(cw, *loc, StorageKind::Bulk)),
            FulfilledPorf::Place(_) => None,
        })
        .sum()
}

/// Free volume `pp` can currently accept for `res`: bounded by its bins'
/// individual resource restrictions (via `place_capacity_ceiling`) and by the
/// place's own `Bulk`-pool volume.
fn place_free_capacity_for(
    cw: &ConstructedCity,
    pp: &ParticularPlace,
    res: UniformResource,
) -> f32 {
    let current = pp
        .contents
        .uniform_totals()
        .into_iter()
        .find(|(r, _)| *r == res)
        .map(|(_, q)| q as f32)
        .unwrap_or(0.0);
    let ceiling_free = (place_capacity_ceiling(cw, pp, res) - current).max(0.0);
    ceiling_free.min(pp.contents.remaining_capacity(StorageKind::Bulk))
}

/// Free volume available for `res` across all storage places, honoring each
/// bin's individual resource restriction (see `place_capacity_ceiling`).
pub fn storage_free_capacity(cw: &ConstructedCity, res: UniformResource) -> f32 {
    storage_ids(cw)
        .into_iter()
        .map(|id| place_free_capacity_for(cw, &cw.placed_places[id], res))
        .sum()
}

/// Total remaining room volume across all storage places, resource-agnostic
/// — a safety-net ceiling on how much can be stored this month regardless of
/// how it splits across resources, used by
/// `resource::distribute_incoming_resources`. (Every bin is dedicated to one
/// resource, so summing `storage_free_capacity` across resources is already
/// exact; this just bounds it by the places' own pools.)
pub fn storage_overall_free_capacity(cw: &ConstructedCity) -> f32 {
    storage_ids(cw)
        .into_iter()
        .map(|id| {
            cw.placed_places[id]
                .contents
                .remaining_capacity(StorageKind::Bulk)
        })
        .sum()
}

/// Per-resource capacity ceiling contributed by bins dedicated to that
/// resource, summed across all storage places. Resources with no dedicated
/// bins are absent from the map.
pub fn storage_dedicated_ceilings(cw: &ConstructedCity) -> HashMap<UniformResource, f32> {
    let mut ceilings = HashMap::new();
    for id in storage_ids(cw) {
        for f in &cw.placed_places[id].fulfillments {
            if let FulfilledPorf::Furniture(loc) = f {
                if let Some(&res) = cw.bin_resource_restrictions.get(loc) {
                    *ceilings.entry(res).or_insert(0.0) +=
                        slot_storage_capacity(cw, *loc, StorageKind::Bulk);
                }
            }
        }
    }
    ceilings
}

/// Deposits `qty` of `res`, spreading it across storage places' free
/// capacity for `res` (mirrors `consume_uniform`'s spreading pattern), which
/// honors each bin's individual resource restriction so a resource is never
/// deposited into a bin restricted to a different one. Returns the amount
/// actually stored, which may be less than `qty` if free capacity runs out.
pub fn deposit_uniform_with_capacity(
    cw: &mut ConstructedCity,
    res: UniformResource,
    qty: u32,
) -> u32 {
    let mut remaining = qty;
    let mut deposited = 0;
    for id in storage_ids(cw) {
        if remaining == 0 {
            break;
        }
        let free = place_free_capacity_for(cw, &cw.placed_places[id], res);
        let take = (free.floor().max(0.0) as u32).min(remaining);
        if take > 0 {
            cw.placed_places[id].contents.add_uniform(res, take);
            remaining -= take;
            deposited += take;
        }
    }
    deposited
}

/// Places the starting market stands and camp wagon directly into
/// `constructed` (real cells, no proposal step). Returns the real-cell deltas
/// for the caller to pass to `apply_changes`. The wagon (not otherwise
/// player-placeable -- see `EorfInfo::placeable`) auto-forms a "camp" place
/// via `sync_places`, giving the player some storage from the start; beyond
/// that, storage rooms/bins must be built normally through construction.
pub fn place_initial_places(constructed: &mut ConstructedCity) -> Vec<(SlotCoord, Option<Cell>)> {
    let mut changes: Vec<(SlotCoord, Option<Cell>)> = Vec::new();

    let wagon_id = constructed.find_structure_by_name("wagon").unwrap();

    let loc = SlotCoord {
        cube: IVec3::new(7, 0, -1),
        slot: Slot::Room,
    };
    let cell = Cell {
        id: wagon_id,
        facing: Facing::default(),
        evaluation: None,
        build_material: BuildMaterialId::default(),
    };
    constructed.contents.set(loc, cell.clone());
    changes.push((loc, Some(cell)));

    changes
}

/// Startup system: places the initial places and spawns its meshes. Must run
/// after `spawn_grid`.
pub fn spawn_initial_places(
    mut commands: Commands,
    eorf_list: Res<EorfList>,
    mut constructed: ResMut<ConstructedCity>,
    mut assembled: ResMut<AssembledCity>,
) {
    let changes = place_initial_places(&mut constructed);
    apply_changes(&mut commands, &mut assembled, &eorf_list, changes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eorf::{EorfInfo, PlacementStyle, StructureEmbedding};

    /// Test furniture: "bin" (`Bulk` capacity 20, matching the old flat
    /// per-bin constant) and "rack" (`Rack` capacity 10, matching the old
    /// flat per-rack constant).
    fn test_structures() -> Vec<EorfInfo> {
        let embedding = StructureEmbedding {
            tall: 0.0,
            // Non-zero: real furniture is never fully impassable (see
            // furniture.ron), and a place's core cube must stay navigable
            // to/from itself now that `PLACE_DIST` reach is nav-based.
            passable: 0.8,
            decorative: 0.0,
            striated: 0.0,
            temporary: 1.0,
        };
        vec![
            EorfInfo {
                name: "bin".to_string(),
                placement_style: PlacementStyle::RoomPlop,
                x_char: None,
                z_char: None,
                embedding: embedding.clone(),
                kind: crate::eorf::FurnitureOrElement::Furniture(vec![(
                    crate::resource::UniformResource::Plank,
                    1,
                )]),
                vantage_evaluated: false,
                storage_capacity: vec![(StorageKind::Bulk, 20.0)],
                placeable: true,
                slots: vec![],
            },
            EorfInfo {
                name: "rack".to_string(),
                placement_style: PlacementStyle::RoomPlop,
                x_char: None,
                z_char: None,
                embedding: embedding.clone(),
                kind: crate::eorf::FurnitureOrElement::Furniture(vec![(
                    crate::resource::UniformResource::Plank,
                    1,
                )]),
                vantage_evaluated: false,
                storage_capacity: vec![(StorageKind::Rack, 10.0)],
                placeable: true,
                slots: vec![],
            },
            EorfInfo {
                name: "bookcase".to_string(),
                placement_style: PlacementStyle::RoomPlop,
                x_char: None,
                z_char: None,
                embedding: embedding.clone(),
                kind: crate::eorf::FurnitureOrElement::Furniture(vec![(
                    crate::resource::UniformResource::Plank,
                    1,
                )]),
                vantage_evaluated: false,
                storage_capacity: vec![(StorageKind::Book, 10.0)],
                placeable: true,
                slots: vec![],
            },
            EorfInfo {
                name: "table".to_string(),
                placement_style: PlacementStyle::RoomPlop,
                x_char: None,
                z_char: None,
                embedding: embedding.clone(),
                kind: crate::eorf::FurnitureOrElement::Furniture(vec![(
                    crate::resource::UniformResource::Plank,
                    1,
                )]),
                vantage_evaluated: false,
                storage_capacity: vec![],
                placeable: true,
                slots: vec![crate::eorf::FurnitureSlot {
                    kind: crate::resource::UniqueResourceKind::Tool,
                    render_offset: bevy::math::Vec3::ZERO,
                }],
            },
            EorfInfo {
                name: "hearth".to_string(),
                placement_style: PlacementStyle::RoomPlop,
                x_char: None,
                z_char: None,
                embedding: embedding.clone(),
                kind: crate::eorf::FurnitureOrElement::Furniture(vec![(
                    crate::resource::UniformResource::Plank,
                    1,
                )]),
                vantage_evaluated: false,
                storage_capacity: vec![],
                placeable: true,
                slots: vec![],
            },
            // A `WallPlop` piece -- lives in a wall slot, not the cube interior.
            EorfInfo {
                name: "chair".to_string(),
                placement_style: PlacementStyle::WallPlop,
                x_char: None,
                z_char: None,
                embedding: embedding.clone(),
                kind: crate::eorf::FurnitureOrElement::Furniture(vec![(
                    crate::resource::UniformResource::Plank,
                    1,
                )]),
                vantage_evaluated: false,
                storage_capacity: vec![],
                placeable: true,
                slots: vec![],
            },
            // A hearth only works built into one (see `chimney::hearth_problem`).
            EorfInfo {
                name: "wall".to_string(),
                placement_style: PlacementStyle::WallDrag,
                x_char: None,
                z_char: None,
                embedding: StructureEmbedding {
                    passable: 0.0,
                    ..embedding
                },
                kind: crate::eorf::FurnitureOrElement::Element(
                    crate::materials::ElementType::WallLike,
                ),
                vantage_evaluated: false,
                storage_capacity: vec![],
                placeable: true,
                slots: vec![],
            },
        ]
    }

    /// The wall a default-facing hearth at `cube` needs behind it.
    fn hearth_wall(cube: IVec3) -> SlotCoord {
        crate::chimney::hearth_back_wall(cube, Facing::default())
    }

    fn place_def(min: u8, max: Option<u8>) -> Place {
        Place {
            name: "storage room".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("bin".to_string()),
                min,
                max,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            public_storage: false,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        }
    }

    fn grid_with_bins(def: Place, bins: &[IVec3]) -> ConstructedCity {
        let mut cw = ConstructedCity::new(test_structures());
        cw.road_forbidden_zone = false;
        cw.places = vec![def];
        let bin_id = cw.find_structure_by_name("bin").unwrap();
        for cube in bins {
            cw.contents.set(
                SlotCoord {
                    cube: *cube,
                    slot: Slot::Room,
                },
                Cell {
                    id: bin_id,
                    facing: Facing::default(),
                    evaluation: None,
                    build_material: BuildMaterialId::default(),
                },
            );
        }
        cw
    }

    fn b(x: i32, z: i32) -> IVec3 {
        IVec3::new(x, 0, z)
    }

    /// The room slot of `cube` -- where these tests' helper grids place
    /// furniture.
    fn r(cube: IVec3) -> SlotCoord {
        SlotCoord {
            cube,
            slot: Slot::Room,
        }
    }

    /// A room-slot furniture fulfillment at `cube` -- the common case for
    /// these tests.
    fn f(cube: IVec3) -> FulfilledPorf {
        FulfilledPorf::Furniture(r(cube))
    }

    /// Every placed place, as (kind name, fulfillments), sorted -- for
    /// comparing formation results independent of ids.
    fn formed(cw: &ConstructedCity) -> Vec<(String, Vec<FulfilledPorf>)> {
        let mut formed: Vec<(String, Vec<FulfilledPorf>)> = cw
            .placed_places
            .iter()
            .map(|(_, pp)| (cw.places[pp.place].name.clone(), pp.fulfillments.clone()))
            .collect();
        formed.sort_by_key(|(name, fs)| (name.clone(), format!("{fs:?}")));
        formed
    }

    /// Place a piece of `name`d furniture at `loc` (any slot).
    fn put(cw: &mut ConstructedCity, loc: SlotCoord, name: &str) {
        let id = cw.find_structure_by_name(name).unwrap();
        cw.contents.set(
            loc,
            Cell {
                id,
                facing: Facing::default(),
                evaluation: None,
                build_material: BuildMaterialId::default(),
            },
        );
    }

    #[test]
    fn wall_mounted_furniture_fulfills_a_place_requirement() {
        // A dining-room-like place: a room-slot `table` core plus two
        // `WallPlop` chairs, which live in wall slots. The place system must
        // still find and claim them.
        let mut cw = ConstructedCity::new(test_structures());
        cw.road_forbidden_zone = false;
        cw.places = vec![Place {
            name: "dining room".to_string(),
            requirements: vec![
                PlaceReq {
                    requirement: Porf::Furniture("table".to_string()),
                    min: 1,
                    max: None,
                    worker_visit_weight: 1.0,
                    worker_visit_duration: 1.0,
                },
                PlaceReq {
                    requirement: Porf::Furniture("chair".to_string()),
                    min: 2,
                    max: None,
                    worker_visit_weight: 1.0,
                    worker_visit_duration: 1.0,
                },
            ],
            public_storage: false,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        }];

        put(
            &mut cw,
            SlotCoord {
                cube: b(0, 0),
                slot: Slot::Room,
            },
            "table",
        );
        put(
            &mut cw,
            SlotCoord {
                cube: b(0, 0),
                slot: Slot::ZLoWall,
            },
            "chair",
        );
        put(
            &mut cw,
            SlotCoord {
                cube: b(0, 1),
                slot: Slot::ZLoWall,
            },
            "chair",
        );

        sync_places(&mut cw);

        assert_eq!(cw.placed_places.iter().count(), 1);
        let (_, pp) = cw.placed_places.iter().next().unwrap();
        assert_eq!(cw.places[pp.place].name, "dining room");
        // The table plus both wall chairs.
        assert_eq!(pp.fulfillments.len(), 3);
        let wall_chairs = pp
            .fulfillments
            .iter()
            .filter(|f| matches!(f, FulfilledPorf::Furniture(loc) if loc.slot != Slot::Room))
            .count();
        assert_eq!(wall_chairs, 2);
    }

    #[test]
    fn wall_slot_furniture_can_be_a_place_core() {
        // A place whose *core* requirement is a wall-mounted chair. Exercises
        // core-candidate enumeration (`all_furniture_named`) for non-room slots.
        let mut cw = ConstructedCity::new(test_structures());
        cw.road_forbidden_zone = false;
        cw.places = vec![Place {
            name: "nook".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("chair".to_string()),
                min: 1,
                max: None,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            public_storage: false,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        }];

        let core = SlotCoord {
            cube: b(2, 2),
            slot: Slot::XLoWall,
        };
        put(&mut cw, core, "chair");

        sync_places(&mut cw);

        assert_eq!(cw.placed_places.iter().count(), 1);
        let (_, pp) = cw.placed_places.iter().next().unwrap();
        assert_eq!(cw.places[pp.place].name, "nook");
        // The place is cored on the wall chair itself.
        assert_eq!(pp.fulfillments, vec![FulfilledPorf::Furniture(core)]);
    }

    #[test]
    fn wall_core_forms_even_when_its_cubes_room_furniture_is_claimed() {
        // A bin (room slot) and a chair (wall slot) share a cube. The bin's
        // storage room forms first (same cube, lower place index); the chair
        // must still be free to core its own nook, since "already claimed" is
        // judged per slot, not per cube.
        let mut nook = place_def(1, None);
        nook.name = "nook".to_string();
        nook.requirements[0].requirement = Porf::Furniture("chair".to_string());
        let mut cw = grid_with_bins(place_def(1, None), &[b(2, 2)]);
        cw.places.push(nook);
        let chair = SlotCoord {
            cube: b(2, 2),
            slot: Slot::XLoWall,
        };
        put(&mut cw, chair, "chair");

        sync_places(&mut cw);

        let mut formed: Vec<(&str, &[FulfilledPorf])> = cw
            .placed_places
            .iter()
            .map(|(_, pp)| (cw.places[pp.place].name.as_str(), &pp.fulfillments[..]))
            .collect();
        formed.sort_by_key(|(name, _)| *name);
        assert_eq!(
            formed,
            vec![
                ("nook", &[FulfilledPorf::Furniture(chair)][..]),
                ("storage room", &[f(b(2, 2))][..]),
            ]
        );
    }

    #[test]
    fn bundled_place_definitions_cross_reference_cleanly() {
        // Panics if places.ron names a furniture/place that doesn't exist.
        let infos = load_place_info(
            &crate::eorf::load_structure_info(),
            &crate::idea::load_idea_info(),
        );
        assert!(!infos.is_empty());
    }

    #[test]
    #[should_panic(expected = "unknown furniture")]
    fn validation_rejects_unknown_furniture_reference() {
        let places = vec![place_def(1, None)]; // requires furniture "bin"
        validate_place_info(&places, &[], &[]); // ...but no eorfs exist
    }

    #[test]
    #[should_panic(expected = "unknown idea")]
    fn validation_rejects_a_gate_on_an_unknown_idea() {
        let mut place = place_def(1, None);
        place.gate = Some(IdeaGate {
            idea: "Astrology".to_string(),
            unlock_at: 0.5,
            full_at: 1.0,
        });
        validate_place_info(&[place], &test_structures(), &crate::idea::load_idea_info());
    }

    #[test]
    #[should_panic(expected = "requires itself")]
    fn validation_rejects_places_that_nest_in_a_cycle() {
        let places = vec![
            kind("chicken", &[(Porf::Place("egg".to_string()), 1, None)]),
            kind("egg", &[(Porf::Place("chicken".to_string()), 1, None)]),
        ];
        validate_place_info(&places, &test_structures(), &[]);
    }

    /// A gate that never ramps would divide by zero when computing efficiency.
    #[test]
    #[should_panic(expected = "isn't below its full_at")]
    fn validation_rejects_a_gate_with_an_empty_ramp() {
        let mut place = place_def(1, None);
        place.gate = Some(IdeaGate {
            idea: "Specialization".to_string(),
            unlock_at: 1.0,
            full_at: 1.0,
        });
        validate_place_info(&[place], &test_structures(), &crate::idea::load_idea_info());
    }

    #[test]
    fn a_place_takes_every_free_bin_in_reach() {
        // A bin 10 apart is outside PLACE_DIST (6), so it forms its own place.
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(0, 1), b(10, 0)]);
        sync_places(&mut grid);
        assert_eq!(
            formed(&grid),
            vec![
                ("storage room".to_string(), vec![f(b(0, 0)), f(b(0, 1))]),
                ("storage room".to_string(), vec![f(b(10, 0))]),
            ]
        );
    }

    #[test]
    fn a_chain_of_cores_forms_one_large_place() {
        // Each neighbor is 5 apart (within PLACE_DIST), but the chain spans 10,
        // which is itself beyond PLACE_DIST -- only reachable by chaining
        // through the middle bin.
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(5, 0), b(10, 0)]);
        sync_places(&mut grid);
        assert_eq!(
            formed(&grid),
            vec![(
                "storage room".to_string(),
                vec![f(b(0, 0)), f(b(5, 0)), f(b(10, 0))]
            )]
        );
    }

    #[test]
    fn places_are_re_formed_from_scratch_keeping_the_old_id() {
        // An existing place holds only two of three bins in reach -- not how
        // formation would arrange them. Re-forming gives it all three, and it
        // keeps its id, since it's recognizably the same place.
        let mut grid = grid_with_bins(place_def(2, None), &[b(0, 0), b(0, 1), b(0, 2)]);
        let old = grid.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![f(b(0, 1)), f(b(0, 2))],
            contents: Inventory::new([(StorageKind::Bulk, 40.0)]),
            restriction: ParentRestriction::Unrestricted,
        });

        assert!(sync_places(&mut grid));
        assert_eq!(grid.placed_places.ids(), vec![old]);
        assert_eq!(
            grid.placed_places[old].fulfillments,
            vec![f(b(0, 0)), f(b(0, 1)), f(b(0, 2))]
        );
    }

    #[test]
    fn place_id_at_finds_the_owner() {
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0)]);
        let id = grid.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![f(b(0, 0))],
            contents: Inventory::new([(StorageKind::Bulk, 20.0)]),
            restriction: ParentRestriction::Unrestricted,
        });
        assert_eq!(place_id_at(&grid, r(b(0, 0))), Some(id));
        grid.placed_places.remove(id);
        assert!(grid.placed_places.is_empty());
        assert_eq!(place_id_at(&grid, r(b(0, 0))), None);
    }

    #[test]
    fn unused_furniture_not_part_of_any_place() {
        let grid = grid_with_bins(place_def(1, None), &[b(0, 0)]);
        assert_eq!(place_id_at(&grid, r(b(0, 0))), None);
        assert_eq!(valid_places_for(&grid, r(b(0, 0))), vec![0]);
    }

    #[test]
    fn sync_places_forms_and_dissolves_automatically() {
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(5, 5)]);
        assert!(sync_places(&mut grid));
        assert_eq!(grid.placed_places.len(), 2);
        assert!(place_id_at(&grid, r(b(0, 0))).is_some());
        assert!(place_id_at(&grid, r(b(5, 5))).is_some());

        // Re-running with nothing changed is a no-op.
        assert!(!sync_places(&mut grid));

        // Removing a bin dissolves its place.
        grid.contents.take(SlotCoord {
            cube: b(0, 0),
            slot: Slot::Room,
        });
        assert!(sync_places(&mut grid));
        assert_eq!(grid.placed_places.len(), 1);
        assert!(place_id_at(&grid, r(b(5, 5))).is_some());
    }

    #[test]
    fn excluding_a_held_bin_evicts_it_even_if_the_room_still_meets_its_minimum() {
        // Three bins within range: min 1, so the room can spare one.
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(0, 1), b(0, 2)]);
        assert!(sync_places(&mut grid));
        let id = place_id_at(&grid, r(b(0, 0))).unwrap();
        assert_eq!(grid.placed_places[id].fulfillments.len(), 3);

        grid.furniture_restrictions
            .insert(r(b(0, 1)), ParentRestriction::Excluded);
        assert!(sync_places(&mut grid));

        assert!(!grid.placed_places[id].fulfillments.contains(&f(b(0, 1))));
        assert!(grid.placed_places[id].fulfillments.contains(&f(b(0, 0))));
        assert!(grid.placed_places[id].fulfillments.contains(&f(b(0, 2))));
    }

    /// A place kind named `name`, with `(requirement, min, max)` requirements
    /// (the first being the core).
    fn kind(name: &str, reqs: &[(Porf, u8, Option<u8>)]) -> Place {
        Place {
            name: name.to_string(),
            requirements: reqs
                .iter()
                .map(|(requirement, min, max)| PlaceReq {
                    requirement: requirement.clone(),
                    min: *min,
                    max: *max,
                    worker_visit_weight: 1.0,
                    worker_visit_duration: 1.0,
                })
                .collect(),
            public_storage: false,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        }
    }

    fn furniture(name: &str) -> Porf {
        Porf::Furniture(name.to_string())
    }

    fn city_with(places: Vec<Place>) -> ConstructedCity {
        let mut cw = ConstructedCity::new(test_structures());
        cw.road_forbidden_zone = false;
        cw.places = places;
        cw
    }

    /// A wall-mounted chair at `cube`.
    fn chair_at(cube: IVec3) -> SlotCoord {
        SlotCoord {
            cube,
            slot: Slot::ZLoWall,
        }
    }

    #[test]
    fn more_specific_cores_get_first_pick() {
        // A table with carpenter's tools installed could core either a dining
        // room or a workshop. The workshop's core is more specific, so it
        // wins -- even though the dining room comes first in the list.
        let mut cw = city_with(vec![
            kind(
                "dining room",
                &[(furniture("table"), 1, None), (furniture("chair"), 2, None)],
            ),
            kind(
                "workshop",
                &[
                    (
                        Porf::InstalledTool(ToolKind::CarpentersTools, "table".to_string()),
                        1,
                        Some(1),
                    ),
                    (furniture("chair"), 1, None),
                ],
            ),
        ]);
        put(&mut cw, r(b(0, 0)), "table");
        put(&mut cw, chair_at(b(0, 1)), "chair");
        put(&mut cw, chair_at(b(1, 1)), "chair");

        // Without the tool, it's just a dining table.
        sync_places(&mut cw);
        assert_eq!(formed(&cw)[0].0, "dining room");

        cw.set_slot(
            r(b(0, 0)),
            0,
            1,
            Some(UniqueResource::Tool(ToolKind::CarpentersTools)),
        );
        sync_places(&mut cw);
        assert_eq!(
            formed(&cw),
            vec![(
                "workshop".to_string(),
                vec![
                    f(b(0, 0)),
                    FulfilledPorf::Furniture(chair_at(b(0, 1))),
                    FulfilledPorf::Furniture(chair_at(b(1, 1))),
                ]
            )]
        );
    }

    #[test]
    fn leftovers_go_to_the_nearest_place() {
        // Two desks (each a table plus any number of chairs) within reach of
        // each other. Each first takes its nearest chair; the third chair is
        // within reach of both, but nearer the second desk.
        let mut cw = city_with(vec![kind(
            "desk",
            &[
                (furniture("table"), 1, Some(1)),
                (furniture("chair"), 1, None),
            ],
        )]);
        put(&mut cw, r(b(0, 0)), "table");
        put(&mut cw, r(b(4, 0)), "table");
        for cube in [b(1, 0), b(3, 0), b(4, 1)] {
            put(&mut cw, chair_at(cube), "chair");
        }

        sync_places(&mut cw);

        let chair = |cube| FulfilledPorf::Furniture(chair_at(cube));
        assert_eq!(
            formed(&cw),
            vec![
                ("desk".to_string(), vec![f(b(0, 0)), chair(b(1, 0))]),
                (
                    "desk".to_string(),
                    vec![f(b(4, 0)), chair(b(3, 0)), chair(b(4, 1))]
                ),
            ]
        );
    }

    #[test]
    fn the_result_does_not_depend_on_build_order() {
        // A kitchen (a hearth and up to four tables) beside a dining room
        // (tables and chairs). Whichever is built first, the kitchen gets
        // the table nearest its hearth and the dining room keeps the other.
        let places = vec![
            kind(
                "kitchen",
                &[
                    (furniture("hearth"), 1, Some(1)),
                    (furniture("table"), 1, Some(4)),
                ],
            ),
            kind(
                "dining room",
                &[(furniture("table"), 1, None), (furniture("chair"), 2, None)],
            ),
        ];
        let table_a = (r(b(0, 0)), "table");
        let chairs = [(chair_at(b(0, 1)), "chair"), (chair_at(b(1, 1)), "chair")];
        let hearth = (r(b(2, 0)), "hearth");
        let wall = (hearth_wall(b(2, 0)), "wall");
        let table_b = (r(b(3, 0)), "table");

        let build = |order: Vec<(SlotCoord, &str)>| {
            let mut cw = city_with(places.clone());
            for (loc, name) in order {
                put(&mut cw, loc, name);
                sync_places(&mut cw);
            }
            formed(&cw)
        };
        let dining_first = build(vec![table_a, chairs[0], chairs[1], wall, hearth, table_b]);
        let kitchen_first = build(vec![hearth, wall, table_b, table_a, chairs[0], chairs[1]]);

        assert_eq!(dining_first, kitchen_first);
        assert_eq!(
            dining_first,
            vec![
                (
                    "dining room".to_string(),
                    vec![
                        f(b(0, 0)),
                        FulfilledPorf::Furniture(chairs[0].0),
                        FulfilledPorf::Furniture(chairs[1].0),
                    ]
                ),
                ("kitchen".to_string(), vec![f(b(2, 0)), f(b(3, 0))]),
            ]
        );
    }

    #[test]
    fn a_kitchen_needs_a_working_hearth() {
        let places = vec![kind(
            "kitchen",
            &[
                (furniture("hearth"), 1, Some(1)),
                (furniture("table"), 1, Some(4)),
            ],
        )];
        let mut cw = city_with(places);
        put(&mut cw, r(b(0, 0)), "hearth");
        put(&mut cw, r(b(1, 0)), "table");
        sync_places(&mut cw);
        assert_eq!(formed(&cw), vec![], "no wall behind the hearth");
        assert!(furniture_problem(&cw, r(b(0, 0))).is_some());

        put(&mut cw, hearth_wall(b(0, 0)), "wall");
        sync_places(&mut cw);
        assert_eq!(formed(&cw).len(), 1);
        assert_eq!(furniture_problem(&cw, r(b(0, 0))), None);

        // Anything in a floor slot above the chimney's top covers it; these
        // tests' structures have no roof, so a bin stands in for one.
        let chimney = crate::chimney::chimney_of(&cw.contents, &cw.eorfs, r(b(0, 0))).unwrap();
        let over_top = SlotCoord {
            cube: chimney.top.cube + IVec3::Y,
            slot: Slot::Floor,
        };
        put(&mut cw, over_top, "bin");
        sync_places(&mut cw);
        assert_eq!(formed(&cw), vec![]);
        assert!(furniture_problem(&cw, r(b(0, 0))).is_some());
    }

    #[test]
    fn accessible_range_reaches_through_cores_owned_by_other_places() {
        // A row of bins chains into one storage room, but a nook (a chair
        // that needs a bin) claims the bin at the far end. The storage room's
        // reach still extends around that bin, since it's part of the chain.
        let mut cw = grid_with_bins(place_def(1, None), &[b(0, 0), b(5, 0), b(10, 0)]);
        cw.places.push(kind(
            "nook",
            &[(furniture("chair"), 1, None), (furniture("bin"), 1, None)],
        ));
        put(&mut cw, chair_at(b(14, 0)), "chair");
        sync_places(&mut cw);

        let storage = place_id_at(&cw, r(b(0, 0))).unwrap();
        assert_eq!(
            cw.placed_places[storage].fulfillments,
            vec![f(b(0, 0)), f(b(5, 0))]
        );
        let range = place_accessible_range(&cw, storage);
        // 4 from the nook's bin, but 9 from the nearest bin the room owns.
        assert!(range.contains(&b(14, 0)));
    }

    #[test]
    fn a_dissolved_places_goods_move_to_other_storage() {
        let mut cw = grid_with_bins(storage_place_def(), &[b(0, 0), b(10, 0)]);
        for cube in [b(0, 0), b(10, 0)] {
            cw.bin_resource_restrictions
                .insert(r(cube), UniformResource::Timber);
        }
        sync_places(&mut cw);
        assert_eq!(
            deposit_uniform_with_capacity(&mut cw, UniformResource::Timber, 5),
            5
        );
        let first = place_id_at(&cw, r(b(0, 0))).unwrap();
        assert_eq!(
            cw.placed_places[first].contents.uniform_totals(),
            vec![(UniformResource::Timber, 5)]
        );

        cw.take_cell(r(b(0, 0)));
        assert!(sync_places(&mut cw));

        assert_eq!(cw.placed_places.len(), 1);
        assert_eq!(total_uniform(&cw, UniformResource::Timber), 5);
    }

    // ── automatic place formation through the headless REPL ────────────────

    use crate::headless::HeadlessSession;

    /// Dispatches `cmd`, panicking with the session's error on failure -- test
    /// bodies read as the command script they are.
    fn dispatch_ok(session: &mut HeadlessSession, cmd: &str) -> Vec<String> {
        session
            .dispatch(cmd)
            .unwrap_or_else(|e| panic!("{cmd}: {e}"))
    }

    /// Exercises the full automatic place-formation flow through the real
    /// Bevy schedule -- `place` -> `tick` -- and checks that `sync_places`
    /// forms the place and `sync_homes` reacts, then that removing the
    /// pallet dissolves the place and evicts the individual again.
    #[test]
    fn placing_a_pallet_auto_forms_a_bedroom_and_drives_home_assignment() {
        let mut session = HeadlessSession::new(1);

        // Place a pallet far from the initial market stands so it can't be
        // swept into an existing place.
        dispatch_ok(&mut session, "place 100 0 100 room pallet");
        dispatch_ok(&mut session, "tick");

        let population = dispatch_ok(&mut session, "query population");
        assert_eq!(population.len(), 1);
        assert!(
            !population[0].contains("home=none"),
            "expected the individual to move into the auto-formed bedroom: {population:?}"
        );

        dispatch_ok(&mut session, "remove 100 0 100 room");
        dispatch_ok(&mut session, "tick");
        let population = dispatch_ok(&mut session, "query population");
        assert!(
            population[0].contains("home=none"),
            "expected the individual to be evicted: {population:?}"
        );
    }

    /// A place cored on another place (an inn, cored on its dining room)
    /// must still form even though its core cube already belongs to that
    /// inner place -- and once it does, its bedrooms are guest rooms that
    /// residents don't sleep in.
    #[test]
    fn an_inn_forms_around_its_dining_room() {
        use crate::headless::HeadlessSession;

        let mut session = HeadlessSession::new(1);
        crate::headless::build_inn(&mut session, 100, 100);
        let dining = session.dispatch("query place 100 0 100").unwrap();
        assert!(
            dining[0].contains("within=[inn#"),
            "the dining room should be nested in an inn: {dining:?}"
        );
        let bedroom = session.dispatch("query place 101 0 102").unwrap();
        assert!(bedroom[0].contains("type=bedroom") && bedroom[0].contains("within=[inn#"));
        let population = session.dispatch("query population").unwrap();
        assert!(
            population[0].contains("home=none"),
            "inn bedrooms are for guests: {population:?}"
        );
    }

    /// Exercises the `InstalledTool` requirement end-to-end through the real
    /// headless REPL and the bundled `places.ron`/`furniture.ron`: a table's
    /// tool slot must actually have carpenter's tools installed (not just a
    /// bare table) before a "carpenter's workshop" forms alongside a bin and a
    /// chair, and removing the tool dissolves it again.
    ///
    /// The workshop is also idea-gated, so this starts by learning enough
    /// Specialization to clear the gate -- see
    /// `an_idea_gate_hides_the_carpenters_workshop_until_it_is_met` for the
    /// gate's own coverage.
    #[test]
    fn installing_carpenters_tools_on_a_table_forms_a_workshop() {
        let mut session = HeadlessSession::new(1);
        dispatch_ok(&mut session, "learn Specialization 50");

        // Somewhere far from the initial furniture so nothing else is swept in.
        dispatch_ok(&mut session, "place 100 0 100 room table");
        dispatch_ok(&mut session, "place 100 0 101 room bin");
        dispatch_ok(&mut session, "place 100 0 102 xwall chair");
        dispatch_ok(&mut session, "tick");

        // A bare table (no tool installed) isn't enough to form the workshop.
        let places = dispatch_ok(&mut session, "query place 100 0 100");
        assert!(
            !places.iter().any(|l| l.contains("carpenter's_workshop")),
            "workshop shouldn't form without an installed tool: {places:?}"
        );

        dispatch_ok(&mut session, "deposit_tool");
        dispatch_ok(&mut session, "install 100 0 100 0");
        dispatch_ok(&mut session, "tick");

        let slots = dispatch_ok(&mut session, "query slots 100 0 100");
        assert!(
            slots.iter().any(|l| l.contains("Carpenter's tools")),
            "expected the tool to show as installed: {slots:?}"
        );
        let places = dispatch_ok(&mut session, "query place 100 0 100");
        assert!(
            places.iter().any(|l| l.contains("carpenter's_workshop")),
            "expected a carpenter's workshop to form: {places:?}"
        );

        // Removing the tool returns it to storage and dissolves the workshop.
        dispatch_ok(&mut session, "uninstall 100 0 100 0");
        dispatch_ok(&mut session, "tick");
        let places = dispatch_ok(&mut session, "query place 100 0 100");
        assert!(
            !places.iter().any(|l| l.contains("carpenter's_workshop")),
            "expected the workshop to dissolve once its tool is removed: {places:?}"
        );
    }

    /// The gate, end-to-end through the real systems: with the furniture and
    /// the installed tool all in place, a carpenter's workshop still doesn't
    /// exist until Specialization is half understood -- and it isn't even
    /// *offered* (`valid_places`) below the threshold. Crossing it mid-session
    /// forms the workshop with no further edit, which is the cascade
    /// `sync_idea_progress` -> `sync_places_system` doing its job.
    #[test]
    fn an_idea_gate_hides_the_carpenters_workshop_until_it_is_met() {
        let mut session = HeadlessSession::new(1);

        dispatch_ok(&mut session, "place 100 0 100 room table");
        dispatch_ok(&mut session, "place 100 0 101 room bin");
        dispatch_ok(&mut session, "place 100 0 102 xwall chair");
        dispatch_ok(&mut session, "deposit_tool");
        dispatch_ok(&mut session, "install 100 0 100 0");
        dispatch_ok(&mut session, "tick");

        let workshop_formed = |session: &mut HeadlessSession| {
            dispatch_ok(session, "query place 100 0 100")
                .iter()
                .any(|l| l.contains("carpenter's_workshop"))
        };
        let workshop_offered = |session: &mut HeadlessSession| {
            dispatch_ok(session, "query valid_places 100 0 100")
                .iter()
                .any(|l| l.contains("carpenter's_workshop"))
        };

        assert!(
            !workshop_formed(&mut session),
            "no Specialization at all: the workshop must not form"
        );
        assert!(
            !workshop_offered(&mut session),
            "a locked place kind shouldn't be offered either"
        );

        // 24/50 is 48% -- still short of the 50% threshold.
        dispatch_ok(&mut session, "learn Specialization 24");
        dispatch_ok(&mut session, "tick");
        assert!(
            !workshop_formed(&mut session),
            "48% understood is below the 50% gate"
        );

        // 25/50 is exactly 50%: unlocked, though at zero efficiency.
        dispatch_ok(&mut session, "learn Specialization 25");
        dispatch_ok(&mut session, "tick");
        assert!(
            workshop_formed(&mut session),
            "crossing the threshold should form the workshop with no further edit"
        );
        assert!(workshop_offered(&mut session));

        let ideas = dispatch_ok(&mut session, "query ideas");
        assert!(
            ideas
                .iter()
                .any(|l| l.contains("carpenter's_workshop") && l.contains("efficiency=0.00")),
            "at exactly the unlock threshold the workshop should be useless: {ideas:?}"
        );
    }

    // ── storage capacity helpers ────────────────────────────────────────────

    fn storage_place_def() -> Place {
        Place {
            name: "storage room".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("bin".to_string()),
                min: 1,
                max: None,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            public_storage: true,
            accounting: Some(Approximation {
                digits: 2,
                max: 999,
            }),
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        }
    }

    /// Builds a grid with `def`'s core furniture (from `def.requirements[0]`)
    /// placed at each of `bins`, plus a `ParticularPlace` fulfilled by them --
    /// so both the requirement lookup and the furniture-driven capacity
    /// lookup (`cube_storage_capacity`) see consistent furniture.
    fn grid_with_storage_bins(def: Place, bins: &[IVec3], inv: Inventory) -> ConstructedCity {
        let furniture_name = match &def.requirements[0].requirement {
            Porf::Furniture(name) => name.clone(),
            Porf::Place(_) | Porf::InstalledTool(..) => {
                panic!("test helper expects a furniture-backed place")
            }
        };
        let mut cw = ConstructedCity::new(test_structures());
        cw.road_forbidden_zone = false;
        cw.places = vec![def];
        let furniture_id = cw.find_structure_by_name(&furniture_name).unwrap();
        for cube in bins {
            cw.contents.set(
                SlotCoord {
                    cube: *cube,
                    slot: Slot::Room,
                },
                Cell {
                    id: furniture_id,
                    facing: Facing::default(),
                    evaluation: None,
                    build_material: BuildMaterialId::default(),
                },
            );
        }
        cw.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: bins.iter().map(|&cube| f(cube)).collect(),
            contents: inv,
            restriction: ParentRestriction::Unrestricted,
        });
        cw
    }

    fn grid_with_storage(def: Place, inv: Inventory) -> ConstructedCity {
        grid_with_storage_bins(def, &[b(0, 0)], inv)
    }

    #[test]
    fn storage_totals_sums_across_places() {
        let mut inv = Inventory::new([(StorageKind::Bulk, 20.0)]);
        inv.add_uniform(UniformResource::Timber, 5);
        let cw = grid_with_storage(storage_place_def(), inv);
        assert_eq!(storage_totals(&cw).get(&UniformResource::Timber), Some(&5));
    }

    #[test]
    fn storage_free_capacity_reflects_remaining_volume() {
        let mut inv = Inventory::new([(StorageKind::Bulk, 20.0)]);
        inv.add_uniform(UniformResource::Timber, 12);
        let mut cw = grid_with_storage(storage_place_def(), inv);
        cw.bin_resource_restrictions
            .insert(r(b(0, 0)), UniformResource::Timber);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Timber), 8.0);
    }

    #[test]
    fn deposit_with_capacity_caps_at_remaining_volume() {
        let inv = Inventory::new([(StorageKind::Bulk, 5.0)]);
        let mut cw = grid_with_storage(storage_place_def(), inv);
        cw.bin_resource_restrictions
            .insert(r(b(0, 0)), UniformResource::Timber);
        let deposited = deposit_uniform_with_capacity(&mut cw, UniformResource::Timber, 12);
        assert_eq!(deposited, 5);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Timber), 0.0);
    }

    #[test]
    fn bin_restricted_to_a_resource_excludes_others_from_capacity() {
        let mut inv = Inventory::new([(StorageKind::Bulk, 20.0)]);
        inv.add_uniform(UniformResource::Timber, 5);
        let mut cw = grid_with_storage_bins(storage_place_def(), &[b(0, 0)], inv);
        cw.bin_resource_restrictions
            .insert(r(b(0, 0)), UniformResource::Timber);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Timber), 15.0);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Straw), 0.0);
    }

    #[test]
    fn unassigned_bins_hold_nothing() {
        let inv = Inventory::new([(StorageKind::Bulk, 20.0)]);
        let mut cw = grid_with_storage_bins(storage_place_def(), &[b(0, 0)], inv);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Timber), 0.0);
        assert_eq!(
            deposit_uniform_with_capacity(&mut cw, UniformResource::Timber, 5),
            0
        );
        assert!(storage_dedicated_ceilings(&cw).is_empty());
    }

    #[test]
    fn bins_assigned_to_different_resources_split_capacity() {
        let inv = Inventory::new([(StorageKind::Bulk, 40.0)]);
        let mut cw = grid_with_storage_bins(storage_place_def(), &[b(0, 0), b(0, 1)], inv);
        cw.bin_resource_restrictions
            .insert(r(b(0, 0)), UniformResource::Timber);
        cw.bin_resource_restrictions
            .insert(r(b(0, 1)), UniformResource::Straw);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Timber), 20.0);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Straw), 20.0);
        assert_eq!(
            storage_dedicated_ceilings(&cw).get(&UniformResource::Timber),
            Some(&20.0)
        );
    }

    #[test]
    fn deposit_with_capacity_refuses_to_overfill_a_restricted_bin() {
        let inv = Inventory::new([(StorageKind::Bulk, 20.0)]);
        let mut cw = grid_with_storage_bins(storage_place_def(), &[b(0, 0)], inv);
        cw.bin_resource_restrictions
            .insert(r(b(0, 0)), UniformResource::Timber);
        let deposited = deposit_uniform_with_capacity(&mut cw, UniformResource::Straw, 10);
        assert_eq!(deposited, 0);
        assert_eq!(storage_totals(&cw).get(&UniformResource::Straw), None);
    }

    fn rack_place_def() -> Place {
        Place {
            name: "shelving".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("rack".to_string()),
                min: 1,
                max: None,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            public_storage: true,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        }
    }

    fn bookroom_place_def() -> Place {
        Place {
            name: "bookroom".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("bookcase".to_string()),
                min: 2,
                max: None,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            public_storage: true,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        }
    }

    #[test]
    fn bookcases_provide_book_capacity_not_rack_capacity() {
        let cw = grid_with_storage_bins(
            bookroom_place_def(),
            &[b(0, 0), b(0, 1)],
            Inventory::new([(StorageKind::Book, 30.0)]),
        );
        // Two bookcases at 10 each back a book capacity of 20...
        assert_eq!(book_capacity(&cw), 20.0);
        // ...and contribute nothing to rack (tool/rug) capacity.
        assert_eq!(rack_capacity(&cw, RackContents::Tools), 0.0);
        assert_eq!(rack_capacity(&cw, RackContents::Rugs), 0.0);
    }

    #[test]
    fn racks_provide_no_book_capacity() {
        let cw = grid_with_storage_bins(
            rack_place_def(),
            &[b(0, 0)],
            Inventory::new([(StorageKind::Rack, 10.0)]),
        );
        assert_eq!(book_capacity(&cw), 0.0);
        assert_eq!(rack_capacity(&cw, RackContents::Tools), 10.0);
    }

    #[test]
    fn rack_defaults_to_tools_when_unset() {
        let cw = grid_with_storage_bins(
            rack_place_def(),
            &[b(0, 0)],
            Inventory::new([(StorageKind::Rack, 10.0)]),
        );
        assert_eq!(rack_free_capacity(&cw, RackContents::Tools), 10.0);
        assert_eq!(rack_free_capacity(&cw, RackContents::Rugs), 0.0);
    }

    #[test]
    fn rack_dedicated_to_rugs_excludes_tools() {
        let mut cw = grid_with_storage_bins(
            rack_place_def(),
            &[b(0, 0)],
            Inventory::new([(StorageKind::Rack, 10.0)]),
        );
        cw.rack_restrictions.insert(r(b(0, 0)), RackContents::Rugs);
        assert_eq!(rack_free_capacity(&cw, RackContents::Rugs), 10.0);
        assert_eq!(rack_free_capacity(&cw, RackContents::Tools), 0.0);
    }

    #[test]
    fn deposit_tool_only_lands_in_a_tools_rack_with_room() {
        let mut cw = grid_with_storage_bins(
            rack_place_def(),
            &[b(0, 0), b(0, 1)],
            Inventory::new([(StorageKind::Rack, 10.0)]),
        );
        cw.rack_restrictions.insert(r(b(0, 0)), RackContents::Rugs);
        // b(0, 0) is dedicated to Rugs, so the tool must land via b(0, 1),
        // which defaults to Tools.
        assert!(deposit_tool(&mut cw, ToolKind::CarpentersTools));
        assert_eq!(total_tool_count(&cw), 1);
    }

    #[test]
    fn bins_never_hold_tools_or_rugs() {
        // A plain bin-backed storage room has no rack fulfillments at all, so
        // it's never returned by `rack_storage_ids` and can't receive a tool.
        let mut cw = grid_with_storage_bins(
            storage_place_def(),
            &[b(0, 0)],
            Inventory::new([(StorageKind::Bulk, 20.0)]),
        );
        assert!(!deposit_tool(&mut cw, ToolKind::CarpentersTools));
        assert_eq!(total_tool_count(&cw), 0);
    }

    #[test]
    fn rack_capacity_reflects_total_ceiling_not_just_free_room() {
        let mut cw = grid_with_storage_bins(
            rack_place_def(),
            &[b(0, 0), b(0, 1)],
            Inventory::new([(StorageKind::Rack, 20.0)]),
        );
        cw.rack_restrictions.insert(r(b(0, 0)), RackContents::Rugs);
        // b(0, 1) defaults to Tools; deposit one so free capacity (10) no
        // longer equals the total ceiling (still 10, since it's per-cube).
        deposit_tool(&mut cw, ToolKind::CarpentersTools);
        assert_eq!(rack_capacity(&cw, RackContents::Tools), 10.0);
        assert_eq!(rack_capacity(&cw, RackContents::Rugs), 10.0);
        assert_eq!(total_rug_count(&cw), 0);
    }
}
