use crate::city::{apply_changes, AssembledCity, Cell, ConstructedCity};
use crate::eorf::EorfList;
use crate::evaluation::{fill_default_quality_factors, QualityAspect, QualityFactor};
use crate::materials::BuildMaterialId;
use crate::pathing::{self, NavigationGrid};
use crate::resource::{
    Approximation, Inventory, StorageKind, ToolKind, UniformResource, UniqueResource,
};
use crate::sparse3d::{Facing, Slot, SlotCoord};
use bevy::math::IVec3;
use bevy::prelude::{Commands, DetectChangesMut, Res, ResMut};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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

/// Restricts which `Place` kinds may claim a Furniture/Place kind to fulfill
/// one of their requirements. Attached to the Furniture/Place *kind* (an
/// `EorfInfo`/`PlaceInfo`), not to any particular placed instance.
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
    /// A furniture cube of the named kind with a `Tool` of this kind installed
    /// in one of its slots (see `ConstructedCity::furniture_slots`). Fulfilled
    /// by that furniture cube.
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlaceReq {
    pub requirement: Porf,
    pub min: u8,
    pub max: Option<u8>,
    pub worker_visit_weight: f32,
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
    /// `EorfInfo::storage_capacity`, `storage_ids`.
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
#[derive(Clone, Copy, Debug, PartialEq)]
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
    pub fn insert(&mut self, place: ParticularPlace) -> PlacedPlaceId {
        let id = PlacedPlaceId(self.next_id);
        self.next_id += 1;
        self.items.push((id, place));
        id
    }

    /// Removes the place with `id`. A stale (already-removed) id is a no-op.
    pub fn remove(&mut self, id: PlacedPlaceId) {
        self.items.retain(|(i, _)| *i != id);
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
/// never match anything (or, for a gate, silently lock the place forever).
fn validate_place_info(
    places: &[Place],
    eorfs: &[crate::eorf::EorfInfo],
    ideas: &[crate::idea::Idea],
) {
    let furniture_exists = |name: &str| eorfs.iter().any(|e| e.is_furniture() && e.name == name);
    let place_exists = |name: &str| places.iter().any(|p| p.name == name);

    for place in places {
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

/// The world location of a placed place: its core fulfillment's location,
/// resolved recursively through nested places. `None` when `id` (or a nested
/// place it points through) no longer exists.
fn try_place_location(cw: &ConstructedCity, id: PlacedPlaceId) -> Option<IVec3> {
    match cw.placed_places.get(id)?.fulfillments.first()? {
        FulfilledPorf::Furniture(loc) => Some(loc.cube),
        FulfilledPorf::Place(inner) => try_place_location(cw, *inner),
    }
}

/// The world location of a placed place; panics on a stale id.
pub fn place_location(cw: &ConstructedCity, id: PlacedPlaceId) -> IVec3 {
    try_place_location(cw, id).expect("placed place has no resolvable location")
}

fn fulfillment_location(cw: &ConstructedCity, f: &FulfilledPorf) -> IVec3 {
    match f {
        FulfilledPorf::Furniture(loc) => loc.cube,
        FulfilledPorf::Place(id) => place_location(cw, *id),
    }
}

/// Slots a piece of furniture can occupy: room-plopped in the cube's interior,
/// or wall-plopped on one of its two low-side walls. (Only `*LoWall` slots are
/// ever stored -- see `nearest_wall_slot` -- so scanning each cube's own low
/// walls visits every wall slot exactly once across the grid.)
const FURNITURE_SLOTS: [Slot; 3] = [Slot::Room, Slot::XLoWall, Slot::ZLoWall];

/// All furniture named `name` within navigable reach (see
/// `reachable_near_anchors`) of any of `anchors`, at whatever slot they
/// occupy (room or wall). Includes furniture at an anchor itself when it
/// qualifies.
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
    found
}

/// True if the furniture at `cube` is named `furniture_name` and has a
/// `Tool(kind)` installed in one of its slots. Slots live on Room-slot
/// furniture (keyed by cube), so no slot disambiguation is needed.
fn has_installed_tool(
    cw: &ConstructedCity,
    cube: IVec3,
    kind: ToolKind,
    furniture_name: &str,
) -> bool {
    let is_named_furniture = cw
        .contents
        .get(SlotCoord {
            cube,
            slot: Slot::Room,
        })
        .is_some_and(|c| cw.eorfs[c.id.as_usize()].name == furniture_name);
    is_named_furniture
        && cw.furniture_slots.get(&cube).is_some_and(|slots| {
            slots
                .iter()
                .flatten()
                .any(|item| *item == UniqueResource::Tool(kind))
        })
}

/// All furniture named `furniture_name` within navigable reach (see
/// `reachable_near_anchors`) of any of `anchors` that has a `Tool(kind)`
/// installed. Returns the furniture's Room `SlotCoord`.
fn furniture_with_installed_tool_near(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    anchors: &[IVec3],
    kind: ToolKind,
    furniture_name: &str,
) -> Vec<SlotCoord> {
    let reachable = reachable_near_anchors(nav, anchors);
    all_furniture_with_installed_tool(cw, kind, furniture_name)
        .into_iter()
        .filter(|cube| reachable.contains(cube))
        .map(|cube| SlotCoord {
            cube,
            slot: Slot::Room,
        })
        .collect()
}

/// All furniture (anywhere) named `furniture_name` with a `Tool(kind)`
/// installed -- for core-candidate collection in `sync_places`.
fn all_furniture_with_installed_tool(
    cw: &ConstructedCity,
    kind: ToolKind,
    furniture_name: &str,
) -> Vec<IVec3> {
    cw.furniture_slots
        .iter()
        .filter(|(cube, slots)| {
            slots
                .iter()
                .flatten()
                .any(|item| *item == UniqueResource::Tool(kind))
                && cw
                    .contents
                    .get(SlotCoord {
                        cube: **cube,
                        slot: Slot::Room,
                    })
                    .is_some_and(|c| cw.eorfs[c.id.as_usize()].name == furniture_name)
        })
        .map(|(cube, _)| *cube)
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
            cw.places[pp.place].name == name && reachable.contains(&place_location(cw, *id))
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

/// Every cube/place fulfilling `req` within range of any of `anchors`,
/// eligible to be claimed by a `Place` named `parent_name` (see
/// `ParentRestriction`).
fn candidates_near(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    anchors: &[IVec3],
    req: &Porf,
    parent_name: &str,
) -> Vec<FulfilledPorf> {
    match req {
        Porf::Furniture(name) => furniture_of_name_near(cw, nav, anchors, name)
            .into_iter()
            .filter(|loc| {
                cw.furniture_restrictions
                    .get(&loc.cube)
                    .is_none_or(|r| r.allows(parent_name))
            })
            .map(FulfilledPorf::Furniture)
            .collect(),
        Porf::Place(name) => places_of_name_near(cw, nav, anchors, name)
            .into_iter()
            .filter(|&id| cw.placed_places[id].restriction.allows(parent_name))
            .map(FulfilledPorf::Place)
            .collect(),
        Porf::InstalledTool(kind, furniture_name) => {
            furniture_with_installed_tool_near(cw, nav, anchors, *kind, furniture_name)
                .into_iter()
                .filter(|loc| {
                    cw.furniture_restrictions
                        .get(&loc.cube)
                        .is_none_or(|r| r.allows(parent_name))
                })
                .map(FulfilledPorf::Furniture)
                .collect()
        }
    }
}

/// Grows a seed core location into the full connected set of core-type
/// (`requirements[0]`) anchors reachable through a chain of same-type cores
/// each within `PLACE_DIST` of the next -- so a long row of e.g. bins forms
/// one large place regardless of which bin the chain started from. Also
/// restricted, at each step, to fulfillments a `place` named `parent_name`
/// would actually be allowed to claim (see `ParentRestriction`).
fn grow_core_anchors(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    place: &Place,
    seed: IVec3,
) -> Vec<IVec3> {
    let Some(core_req) = place.requirements.first() else {
        return vec![seed];
    };
    let mut anchors = vec![seed];
    loop {
        let reached: Vec<IVec3> =
            candidates_near(cw, nav, &anchors, &core_req.requirement, &place.name)
                .into_iter()
                .map(|f| fulfillment_location(cw, &f))
                .collect();
        let mut grew = false;
        for loc in reached {
            if !anchors.contains(&loc) {
                anchors.push(loc);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    anchors
}

/// The full connected set of core-type anchors for an already-placed
/// instance, re-derived (rather than trusted from its stored fulfillments,
/// which may be capped by the core requirement's `max`) from any one of its
/// current core-type fulfillments. Used to compute a placed place's
/// accessible range for display. Falls back to the place's resolved location
/// if, somehow, none of its fulfillments still match the core requirement.
fn placed_core_anchors(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    id: PlacedPlaceId,
) -> Vec<IVec3> {
    let pp = &cw.placed_places[id];
    let place = &cw.places[pp.place];
    let core_req = place.requirements.first().map(|r| &r.requirement);
    let seed = core_req
        .and_then(|req| {
            pp.fulfillments
                .iter()
                .find(|f| fulfillment_matches(cw, f, req))
        })
        .map(|f| fulfillment_location(cw, f))
        .unwrap_or_else(|| place_location(cw, id));
    grow_core_anchors(cw, nav, place, seed)
}

/// The core-type fulfillment locations actually owned by a placed instance
/// (unlike `placed_core_anchors`, not grown out to every reachable core --
/// a core that exceeded the requirement's `max` and so was never added to
/// `fulfillments` is excluded). Falls back to the place's resolved location
/// if it has no core-type fulfillments.
fn included_core_locations(cw: &ConstructedCity, id: PlacedPlaceId) -> Vec<IVec3> {
    let pp = &cw.placed_places[id];
    let place = &cw.places[pp.place];
    let locs: Vec<IVec3> = match place.requirements.first().map(|r| &r.requirement) {
        Some(core_req) => pp
            .fulfillments
            .iter()
            .filter(|f| fulfillment_matches(cw, f, core_req))
            .map(|f| fulfillment_location(cw, f))
            .collect(),
        None => Vec::new(),
    };
    if locs.is_empty() {
        vec![place_location(cw, id)]
    } else {
        locs
    }
}

/// The full accessible range of a placed instance: every cube within
/// navigable reach (see `reachable_near_anchors`) of any of its owned
/// core-type fulfillments (see `included_core_locations`), deduplicated.
/// Used to paint the "accessible range" overlay for the currently-selected
/// place.
pub fn place_accessible_range(cw: &ConstructedCity, id: PlacedPlaceId) -> Vec<IVec3> {
    let nav = pathing::build_navigation_grid(cw);
    reachable_near_anchors(&nav, &included_core_locations(cw, id))
        .into_iter()
        .collect()
}

/// True if a fulfillment still satisfies the named requirement it was chosen
/// for (used to re-check a donor place after some of its members are pulled).
fn fulfillment_matches(cw: &ConstructedCity, f: &FulfilledPorf, req: &Porf) -> bool {
    match (f, req) {
        (FulfilledPorf::Furniture(loc), Porf::Furniture(name)) => cw
            .contents
            .get(*loc)
            .map(|c| cw.eorfs[c.id.as_usize()].name == *name)
            .unwrap_or(false),
        (FulfilledPorf::Place(id), Porf::Place(name)) => cw
            .placed_places
            .get(*id)
            .is_some_and(|pp| cw.places[pp.place].name == *name),
        (FulfilledPorf::Furniture(loc), Porf::InstalledTool(kind, furniture_name)) => {
            has_installed_tool(cw, loc.cube, *kind, furniture_name)
        }
        _ => false,
    }
}

/// True if `place`'s idea gate is met and `anchors` (the connected set of
/// core-type fulfillments) has at least `min` of every requirement within
/// range.
///
/// The gate check lives here, rather than at each call site, because this is
/// the single choke point every path runs through: formation (via
/// `plan_assignment` -> `choose_core`), the dissolve pass in `sync_places`, and
/// `valid_places_for`. A gated place therefore can't form, isn't offered, and
/// -- were progress ever to regress, which permanent learning rules out -- would
/// dissolve.
fn requirements_met(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    anchors: &[IVec3],
    place: &Place,
) -> bool {
    if gate_efficiency_of(cw, place).is_none() {
        return false;
    }
    place.requirements.iter().all(|req| {
        candidates_near(cw, nav, anchors, &req.requirement, &place.name).len() >= req.min as usize
    })
}

/// Choose the core anchor set for `place_idx` grown from whichever qualifying
/// core is nearest to `cube` (the cube itself preferred), whose combined
/// surroundings satisfy every requirement. Every core-type fulfillment
/// transitively chained (each within navigable reach of the next) into the
/// resulting set can equally well serve as the anchor -- it doesn't matter
/// which one gets picked first.
fn choose_core(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    cube: IVec3,
    place_idx: usize,
) -> Option<Vec<IVec3>> {
    let place = &cw.places[place_idx];
    let core_req = &place.requirements[0].requirement;
    let mut cores = candidates_near(cw, nav, &[cube], core_req, &place.name);
    cores.sort_by_key(|c| {
        let loc = fulfillment_location(cw, c);
        (loc != cube, manhattan2d(loc, cube))
    });
    cores.into_iter().find_map(|core| {
        let anchors = grow_core_anchors(cw, nav, place, fulfillment_location(cw, &core));
        requirements_met(cw, nav, &anchors, place).then_some(anchors)
    })
}

/// Places (indices into `cw.places`) that could be formed around `cube`.
pub fn valid_places_for(cw: &ConstructedCity, cube: IVec3) -> Vec<usize> {
    let nav = pathing::build_navigation_grid(cw);
    (0..cw.places.len())
        .filter(|&idx| choose_core(cw, &nav, cube, idx).is_some())
        .collect()
}

fn place_contains(cw: &ConstructedCity, id: PlacedPlaceId, cube: IVec3) -> bool {
    cw.placed_places.get(id).is_some_and(|pp| {
        pp.fulfillments.iter().any(|f| match f {
            FulfilledPorf::Furniture(loc) => loc.cube == cube,
            FulfilledPorf::Place(inner) => place_contains(cw, *inner, cube),
        })
    })
}

/// The placed place that owns `cube`, if any -- searching recursively through
/// nested place fulfillments.
pub fn place_id_at(cw: &ConstructedCity, cube: IVec3) -> Option<PlacedPlaceId> {
    cw.placed_places
        .iter()
        .map(|(id, _)| id)
        .find(|&id| place_contains(cw, id, cube))
}

/// The chain of places containing `cube`, innermost first, up to the root.
pub fn containing_chain(cw: &ConstructedCity, cube: IVec3) -> Vec<PlacedPlaceId> {
    let mut chain = Vec::new();
    let Some(mut id) = place_id_at(cw, cube) else {
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

/// The placed place directly holding fulfillment `f` (not recursive -- only
/// used to find donors during (re)assignment).
fn owner_of(cw: &ConstructedCity, f: &FulfilledPorf) -> Option<PlacedPlaceId> {
    cw.placed_places
        .iter()
        .find(|(_, pp)| pp.fulfillments.contains(f))
        .map(|(id, _)| id)
}

/// A pre-computed assignment, shared by the panel preview and the commit so the
/// displayed "Pulls {N}" can never disagree with the actual effect.
pub struct AssignmentPlan {
    /// Fulfillments the new place will own.
    pub chosen: Vec<FulfilledPorf>,
    /// How many of `chosen` had to be pulled from other places to meet a min.
    pub pulled: usize,
    /// Placed places that drop below a min and must be destroyed.
    pub destroy: Vec<PlacedPlaceId>,
}

/// Plan assigning fulfillments to a new instance of `place_idx` around `cube`.
/// Prefers unassigned furniture/places; only pulls from other places to reach `min`.
pub fn plan_assignment(
    cw: &ConstructedCity,
    cube: IVec3,
    place_idx: usize,
) -> Option<AssignmentPlan> {
    let nav = pathing::build_navigation_grid(cw);
    plan_assignment_with_nav(cw, &nav, cube, place_idx)
}

fn plan_assignment_with_nav(
    cw: &ConstructedCity,
    nav: &NavigationGrid,
    cube: IVec3,
    place_idx: usize,
) -> Option<AssignmentPlan> {
    let anchors = choose_core(cw, nav, cube, place_idx)?;
    let place = &cw.places[place_idx];

    let mut chosen: Vec<FulfilledPorf> = Vec::new();
    // For each donor place, which of its fulfillments we'd take.
    let mut pulled_from: std::collections::HashMap<PlacedPlaceId, Vec<FulfilledPorf>> =
        std::collections::HashMap::new();

    for req in &place.requirements {
        let max = req.max.map(|m| m as usize).unwrap_or(usize::MAX);
        let min = req.min as usize;

        // Partition reachable fulfillments into unassigned ("free") and those
        // already owned by another place, keeping each owner's id.
        let mut free: Vec<FulfilledPorf> = Vec::new();
        let mut assigned: Vec<(FulfilledPorf, PlacedPlaceId)> = Vec::new();
        for c in candidates_near(cw, nav, &anchors, &req.requirement, &place.name) {
            if chosen.contains(&c) {
                continue;
            }
            match owner_of(cw, &c) {
                None => free.push(c),
                Some(owner) => assigned.push((c, owner)),
            }
        }

        let mut taken: Vec<FulfilledPorf> = free.into_iter().take(max).collect();
        // Only pull from other places if free ones can't satisfy the minimum.
        for (c, owner) in assigned {
            if taken.len() >= min || taken.len() >= max {
                break;
            }
            pulled_from.entry(owner).or_default().push(c);
            taken.push(c);
        }
        chosen.extend(taken);
    }

    // A donor place is destroyed if, after losing its pulled fulfillments, it
    // no longer meets some minimum.
    let mut destroy = Vec::new();
    for (&pp_id, pulled_fs) in &pulled_from {
        let pp = &cw.placed_places[pp_id];
        let def = &cw.places[pp.place];
        let still_meets = def.requirements.iter().all(|req| {
            pp.fulfillments
                .iter()
                .filter(|f| !pulled_fs.contains(f))
                .filter(|f| fulfillment_matches(cw, f, &req.requirement))
                .count()
                >= req.min as usize
        });
        if !still_meets {
            destroy.push(pp_id);
        }
    }
    // Deterministic destruction order (HashMap iteration above is not).
    destroy.sort_unstable();

    let pulled = pulled_from.values().map(Vec::len).sum();
    Some(AssignmentPlan {
        chosen,
        pulled,
        destroy,
    })
}

/// Commit an assignment: create the place, pulling/destroying as planned.
pub fn commit_assignment(cw: &mut ConstructedCity, cube: IVec3, place_idx: usize) {
    let nav = pathing::build_navigation_grid(cw);
    commit_assignment_with_nav(cw, &nav, cube, place_idx);
}

fn commit_assignment_with_nav(
    cw: &mut ConstructedCity,
    nav: &NavigationGrid,
    cube: IVec3,
    place_idx: usize,
) {
    let Some(plan) = plan_assignment_with_nav(cw, nav, cube, place_idx) else {
        return;
    };

    // Take chosen fulfillments away from any place currently holding them.
    for (_, pp) in cw.placed_places.iter_mut() {
        pp.fulfillments.retain(|f| !plan.chosen.contains(f));
    }

    // Destroy donor places that fell below a minimum. Their inventory is
    // discarded.
    for id in &plan.destroy {
        cw.placed_places.remove(*id);
    }

    let capacity_for = |kind: StorageKind| -> f32 {
        plan.chosen
            .iter()
            .map(|f| match f {
                FulfilledPorf::Furniture(loc) => {
                    crate::storage::slot_storage_capacity(cw, *loc, kind)
                }
                FulfilledPorf::Place(_) => 0.0,
            })
            .sum()
    };
    let mut contents = Inventory::new([
        (StorageKind::Bulk, capacity_for(StorageKind::Bulk)),
        (StorageKind::Rack, capacity_for(StorageKind::Rack)),
        (StorageKind::Book, capacity_for(StorageKind::Book)),
    ]);
    if cw.places[place_idx].name == "camp" {
        contents.add_uniform(UniformResource::Plank, 18);
        contents.add_uniform(UniformResource::Canvas, 6);
        contents.add_uniform(UniformResource::Potato, 20);
    }

    cw.placed_places.insert(ParticularPlace {
        place: place_idx,
        fulfillments: plan.chosen,
        contents,
        restriction: ParentRestriction::Unrestricted,
    });
}

/// Remove a placed place, discarding its inventory contents.
pub fn unassign_place(cw: &mut ConstructedCity, id: PlacedPlaceId) {
    cw.placed_places.remove(id);
}

/// The cubes of all furniture named `name` anywhere in the grid (unbounded,
/// unlike `furniture_of_name_near`) -- used by `sync_places` to enumerate
/// candidate cores without a "clicked cube" to search near.
///
/// Furniture at *any* slot qualifies (room-plopped or wall-mounted), so a place
/// may be cored on wall-slot furniture. Only the cube is returned: it seeds
/// `choose_core`, which re-resolves the exact (slot-aware) core via
/// `candidates_near`. A cube hosting the named furniture in more than one slot
/// appears once per occurrence, which is harmless -- the first to form a place
/// there makes the rest no-ops (see the `place_id_at` guard in `sync_places`).
fn all_furniture_named(cw: &ConstructedCity, name: &str) -> Vec<IVec3> {
    cw.contents
        .iter()
        .filter_map(|(loc, cell)| {
            let info = &cw.eorfs[cell.id.as_usize()];
            (info.is_furniture() && info.name == name).then_some(loc.cube)
        })
        .collect()
}

/// Deterministically (re)forms `Place`s from the eligible furniture/places in
/// `cw`. Mirrors the (formerly UI-triggered) `plan_assignment`/
/// `commit_assignment` flow, but runs it over every candidate core instead of
/// a single clicked cube, in a stable order (ascending cube coordinate, then
/// place-definition index), so re-running it is idempotent.
///
/// Existing `ParticularPlace`s that still meet their minimums are left
/// untouched (so their `Inventory` persists across edits); a place that drops
/// below a minimum (e.g. a required structure was removed) is dissolved,
/// freeing its members for reformation in the same pass. Returns `true` if
/// anything changed.
pub fn sync_places(cw: &mut ConstructedCity) -> bool {
    // Built once up front: `cw.contents` (what the grid is derived from) is
    // never mutated over the course of this pass, only `cw.placed_places`.
    let nav = pathing::build_navigation_grid(cw);
    let mut changed = false;

    // Evict any currently-held fulfillment whose restriction no longer
    // allows its owner (e.g. the user just excluded a bin already claimed by
    // a storage room). Done before the minimums check below, since a place
    // can still nominally meet its minimum -- via other nearby candidates --
    // while still listing a now-disallowed member.
    let mut evictions: Vec<(PlacedPlaceId, FulfilledPorf)> = Vec::new();
    for (id, pp) in cw.placed_places.iter() {
        let parent_name = &cw.places[pp.place].name;
        for f in &pp.fulfillments {
            let allowed = match f {
                FulfilledPorf::Furniture(loc) => cw
                    .furniture_restrictions
                    .get(&loc.cube)
                    .is_none_or(|r| r.allows(parent_name)),
                FulfilledPorf::Place(pid) => cw
                    .placed_places
                    .get(*pid)
                    .is_some_and(|p| p.restriction.allows(parent_name)),
            };
            if !allowed {
                evictions.push((id, *f));
            }
        }
    }
    for (id, f) in evictions {
        if let Some(pp) = cw.placed_places.get_mut(id) {
            pp.fulfillments.retain(|x| *x != f);
        }
        changed = true;
    }

    // Dissolve any existing place that no longer meets its own minimums (or,
    // if nested, whose core no longer resolves to a location).
    loop {
        let stale = cw.placed_places.iter().find_map(|(id, pp)| {
            let def = &cw.places[pp.place];
            let ok = try_place_location(cw, id).is_some()
                && requirements_met(cw, &nav, &placed_core_anchors(cw, &nav, id), def);
            (!ok).then_some(id)
        });
        match stale {
            Some(id) => {
                cw.placed_places.remove(id);
                changed = true;
            }
            None => break,
        }
    }

    // Collect every candidate core, across every place definition, in a
    // stable scan order.
    let mut candidates: Vec<(IVec3, usize)> = Vec::new();
    for place_idx in 0..cw.places.len() {
        let Some(core_req) = cw.places[place_idx].requirements.first() else {
            continue;
        };
        let parent_name = &cw.places[place_idx].name;
        let cubes: Vec<IVec3> = match &core_req.requirement {
            Porf::Furniture(name) => all_furniture_named(cw, name)
                .into_iter()
                .filter(|cube| {
                    cw.furniture_restrictions
                        .get(cube)
                        .is_none_or(|r| r.allows(parent_name))
                })
                .collect(),
            Porf::Place(name) => cw
                .placed_places
                .iter()
                .filter(|(_, pp)| {
                    cw.places[pp.place].name == *name && pp.restriction.allows(parent_name)
                })
                .map(|(id, _)| place_location(cw, id))
                .collect(),
            Porf::InstalledTool(kind, furniture_name) => {
                all_furniture_with_installed_tool(cw, *kind, furniture_name)
                    .into_iter()
                    .filter(|cube| {
                        cw.furniture_restrictions
                            .get(cube)
                            .is_none_or(|r| r.allows(parent_name))
                    })
                    .collect()
            }
        };
        candidates.extend(cubes.into_iter().map(|cube| (cube, place_idx)));
    }
    candidates.sort_by_key(|(cube, place_idx)| (cube.x, cube.y, cube.z, *place_idx));

    for (cube, place_idx) in candidates {
        if place_id_at(cw, cube).is_some() {
            continue;
        }
        if plan_assignment_with_nav(cw, &nav, cube, place_idx).is_some() {
            commit_assignment_with_nav(cw, &nav, cube, place_idx);
            changed = true;
        }
    }

    changed
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
/// host one invited farm per month, so this is the invite capacity.
pub fn market_stand_count(cw: &ConstructedCity) -> usize {
    count_furniture_named_in_places(cw, "market stand", "market")
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

/// Fixtures shared between this module's tests and [`crate::storage`]'s, which
/// build the same kind of hand-made city out of test furniture.
#[cfg(test)]
pub(crate) mod test_fixtures {
    use super::*;
    use crate::eorf::{EorfInfo, PlacementStyle, StructureEmbedding};

    /// Test furniture: "bin" (`Bulk` capacity 20, matching the old flat
    /// per-bin constant) and "rack" (`Rack` capacity 10, matching the old
    /// flat per-rack constant).
    pub(crate) fn test_structures() -> Vec<EorfInfo> {
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
            // A `WallPlop` piece -- lives in a wall slot, not the cube interior.
            EorfInfo {
                name: "chair".to_string(),
                placement_style: PlacementStyle::WallPlop,
                x_char: None,
                z_char: None,
                embedding,
                kind: crate::eorf::FurnitureOrElement::Furniture(vec![(
                    crate::resource::UniformResource::Plank,
                    1,
                )]),
                vantage_evaluated: false,
                storage_capacity: vec![],
                placeable: true,
                slots: vec![],
            },
        ]
    }

    pub(crate) fn place_def(min: u8, max: Option<u8>) -> Place {
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

    pub(crate) fn grid_with_bins(def: Place, bins: &[IVec3]) -> ConstructedCity {
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

    pub(crate) fn b(x: i32, z: i32) -> IVec3 {
        IVec3::new(x, 0, z)
    }

    /// A room-slot furniture fulfillment at `cube` -- the common case for
    /// these tests, whose helper grids place furniture in `Slot::Room`.
    pub(crate) fn f(cube: IVec3) -> FulfilledPorf {
        FulfilledPorf::Furniture(SlotCoord {
            cube,
            slot: Slot::Room,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_fixtures::*;
    use super::*;

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
    fn assigns_all_free_bins_without_pulling() {
        let grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(0, 1), b(0, 2)]);
        let plan = plan_assignment(&grid, b(0, 0), 0).unwrap();
        assert_eq!(plan.chosen.len(), 3);
        assert_eq!(plan.pulled, 0);
        assert!(plan.destroy.is_empty());
    }

    #[test]
    fn unlimited_max_grabs_every_reachable_bin() {
        // A bin 10 apart is outside PLACE_DIST (6) and must be excluded.
        let grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(0, 1), b(10, 0)]);
        let plan = plan_assignment(&grid, b(0, 0), 0).unwrap();
        assert!(plan.chosen.contains(&f(b(0, 0))));
        assert!(plan.chosen.contains(&f(b(0, 1))));
        assert!(!plan.chosen.contains(&f(b(10, 0))));
    }

    #[test]
    fn a_chain_of_cores_forms_one_large_place_regardless_of_which_end_is_clicked() {
        // Each neighbor is 5 apart (within PLACE_DIST), but the chain spans 10,
        // which is itself beyond PLACE_DIST -- only reachable by chaining
        // through the middle bin.
        let grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(5, 0), b(10, 0)]);

        for &click in &[b(0, 0), b(5, 0), b(10, 0)] {
            let plan = plan_assignment(&grid, click, 0).unwrap();
            assert_eq!(
                plan.chosen.len(),
                3,
                "clicking {click:?} should chain through the whole row"
            );
        }
    }

    #[test]
    fn pulls_to_meet_min_and_destroys_starved_donor() {
        // Place needs min 2 bins. An existing place owns two of three bins.
        let mut grid = grid_with_bins(place_def(2, None), &[b(0, 0), b(0, 1), b(0, 2)]);
        let donor = grid.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![f(b(0, 1)), f(b(0, 2))],
            contents: Inventory::new([(StorageKind::Bulk, 40.0)]),
            restriction: ParentRestriction::Unrestricted,
        });

        // Right-click the free bin and form a new place from it.
        let plan = plan_assignment(&grid, b(0, 0), 0).unwrap();
        assert_eq!(plan.pulled, 1, "should pull exactly one bin to reach min 2");
        assert_eq!(
            plan.destroy,
            vec![donor],
            "donor falls below min and is destroyed"
        );

        commit_assignment(&mut grid, b(0, 0), 0);
        assert_eq!(
            grid.placed_places.len(),
            1,
            "donor destroyed, new one added"
        );
        let (new_id, new) = grid.placed_places.iter().next().unwrap();
        assert_ne!(new_id, donor, "the new place gets a fresh id");
        assert!(new.fulfillments.contains(&f(b(0, 0))));
        assert_eq!(new.fulfillments.len(), 2);
    }

    #[test]
    fn unassign_removes_the_place() {
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0)]);
        let id = grid.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![f(b(0, 0))],
            contents: Inventory::new([(StorageKind::Bulk, 20.0)]),
            restriction: ParentRestriction::Unrestricted,
        });
        assert_eq!(place_id_at(&grid, b(0, 0)), Some(id));
        unassign_place(&mut grid, id);
        assert!(grid.placed_places.is_empty());
        assert_eq!(place_id_at(&grid, b(0, 0)), None);
    }

    #[test]
    fn unused_furniture_not_part_of_any_place() {
        let grid = grid_with_bins(place_def(1, None), &[b(0, 0)]);
        assert_eq!(place_id_at(&grid, b(0, 0)), None);
        assert_eq!(valid_places_for(&grid, b(0, 0)), vec![0]);
    }

    #[test]
    fn sync_places_forms_and_dissolves_automatically() {
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(5, 5)]);
        assert!(sync_places(&mut grid));
        assert_eq!(grid.placed_places.len(), 2);
        assert!(place_id_at(&grid, b(0, 0)).is_some());
        assert!(place_id_at(&grid, b(5, 5)).is_some());

        // Re-running with nothing changed is a no-op.
        assert!(!sync_places(&mut grid));

        // Removing a bin dissolves its place.
        grid.contents.take(SlotCoord {
            cube: b(0, 0),
            slot: Slot::Room,
        });
        assert!(sync_places(&mut grid));
        assert_eq!(grid.placed_places.len(), 1);
        assert!(place_id_at(&grid, b(5, 5)).is_some());
    }

    #[test]
    fn excluding_a_held_bin_evicts_it_even_if_the_room_still_meets_its_minimum() {
        // Three bins within range: min 1, so the room can spare one.
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(0, 1), b(0, 2)]);
        assert!(sync_places(&mut grid));
        let id = place_id_at(&grid, b(0, 0)).unwrap();
        assert_eq!(grid.placed_places[id].fulfillments.len(), 3);

        grid.furniture_restrictions
            .insert(b(0, 1), ParentRestriction::Excluded);
        assert!(sync_places(&mut grid));

        assert!(!grid.placed_places[id].fulfillments.contains(&f(b(0, 1))));
        assert!(grid.placed_places[id].fulfillments.contains(&f(b(0, 0))));
        assert!(grid.placed_places[id].fulfillments.contains(&f(b(0, 2))));
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
}
