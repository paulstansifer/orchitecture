use crate::city::{apply_changes, AssembledCity, Cell, ConstructedCity, Material};
use crate::eorf::EorfList;
use crate::materials::BuildMaterialId;
use crate::resource::{Approximation, Inventory, ToolKind, UniformResource, UniqueResource};
use crate::sparse3d::{Facing, Slot, SlotCoord};
use bevy::math::IVec3;
use bevy::prelude::{Commands, DetectChangesMut, Res, ResMut};
use serde::{Deserialize, Serialize};

#[allow(unused)]
enum QualityFactor {
    FloorArea { area_max: u16 },
    Spaciousness { sightline_max: u8 },
    Quiet { min: f32 },
}

/// What a `Place` requirement can be fulfilled by: a piece of Furniture, or
/// another (nested) `Place`. Named by the definition it points at.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Porf {
    Furniture(String),
    Place(String),
}

impl Porf {
    pub fn name(&self) -> &str {
        match self {
            Porf::Furniture(n) | Porf::Place(n) => n,
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlaceStorageSpec {
    pub just_one_kind: bool,
    pub accounting: Approximation,
    // max storage space is 20.0 * bins + 10.0 * racks
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlaceInfo {
    pub name: String,
    // First requirement is the core.
    pub requirements: Vec<PlaceReq>,
    pub storage: Option<PlaceStorageSpec>,
}

/// What actually fulfills one slot of a placed `Place`'s requirements.
#[derive(Clone, Debug, PartialEq)]
pub enum FulfilledPorf {
    Furniture(IVec3),
    /// Index into `ConstructedCity::placed_places`.
    Place(usize),
}

/// A placed place instance.
pub struct ParticularPlace {
    /// Index into `ConstructedCity::places`.
    pub place: usize,
    // First fulfillment is the core.
    pub fulfillments: Vec<FulfilledPorf>,
    pub contents: Inventory,
}

/// Loads the place definitions bundled at compile time.
pub fn load_place_info() -> Vec<PlaceInfo> {
    let ron_content = include_str!("../buildables/places.ron");
    ron::from_str(ron_content).unwrap()
}

/// Maximum 2D Manhattan distance (within a single y-layer) for a requirement
/// to count as belonging to a place. Tunable: the 4×3 starting room spans ~5.
pub const PLACE_DIST: i32 = 6;

fn manhattan2d(a: IVec3, b: IVec3) -> i32 {
    (a.x - b.x).abs() + (a.z - b.z).abs()
}

/// The world location of a placed place: its core fulfillment's location,
/// resolved recursively through nested places.
pub fn place_location(cw: &ConstructedCity, idx: usize) -> IVec3 {
    match cw.placed_places[idx].fulfillments[0] {
        FulfilledPorf::Furniture(cube) => cube,
        FulfilledPorf::Place(inner) => place_location(cw, inner),
    }
}

fn fulfillment_location(cw: &ConstructedCity, f: &FulfilledPorf) -> IVec3 {
    match f {
        FulfilledPorf::Furniture(cube) => *cube,
        FulfilledPorf::Place(idx) => place_location(cw, *idx),
    }
}

/// All furniture cubes named `name` within `PLACE_DIST` (2D Manhattan, same
/// y-layer) of `origin`. Includes `origin` itself when it qualifies.
fn furniture_of_name_near(cw: &ConstructedCity, origin: IVec3, name: &str) -> Vec<IVec3> {
    let mut found = Vec::new();
    for dx in -PLACE_DIST..=PLACE_DIST {
        let zspan = PLACE_DIST - dx.abs();
        for dz in -zspan..=zspan {
            let cube = IVec3::new(origin.x + dx, origin.y, origin.z + dz);
            let loc = SlotCoord {
                cube,
                slot: Slot::Room,
            };
            if let Some(cell) = cw.contents.get(loc) {
                let info = &cw.eorfs[cell.id.as_usize()];
                if info.is_furniture() && info.name == name {
                    found.push(cube);
                }
            }
        }
    }
    found
}

/// All placed-place indices named `name` within `PLACE_DIST` of `origin`
/// (measured from each candidate's own resolved location).
fn places_of_name_near(cw: &ConstructedCity, origin: IVec3, name: &str) -> Vec<usize> {
    (0..cw.placed_places.len())
        .filter(|&idx| {
            cw.places[cw.placed_places[idx].place].name == name
                && manhattan2d(place_location(cw, idx), origin) <= PLACE_DIST
        })
        .collect()
}

/// Every cube/place fulfilling `req` within range of `origin`.
fn candidates_near(cw: &ConstructedCity, origin: IVec3, req: &Porf) -> Vec<FulfilledPorf> {
    match req {
        Porf::Furniture(name) => furniture_of_name_near(cw, origin, name)
            .into_iter()
            .map(FulfilledPorf::Furniture)
            .collect(),
        Porf::Place(name) => places_of_name_near(cw, origin, name)
            .into_iter()
            .map(FulfilledPorf::Place)
            .collect(),
    }
}

/// True if a fulfillment still satisfies the named requirement it was chosen
/// for (used to re-check a donor place after some of its members are pulled).
fn fulfillment_matches(cw: &ConstructedCity, f: &FulfilledPorf, req: &Porf) -> bool {
    match (f, req) {
        (FulfilledPorf::Furniture(cube), Porf::Furniture(name)) => cw
            .contents
            .get(SlotCoord {
                cube: *cube,
                slot: Slot::Room,
            })
            .map(|c| cw.eorfs[c.id.as_usize()].name == *name)
            .unwrap_or(false),
        (FulfilledPorf::Place(idx), Porf::Place(name)) => {
            cw.places[cw.placed_places[*idx].place].name == *name
        }
        _ => false,
    }
}

/// True if `core` has at least `min` of every requirement within range.
fn requirements_met(cw: &ConstructedCity, core: IVec3, place: &PlaceInfo) -> bool {
    place
        .requirements
        .iter()
        .all(|req| candidates_near(cw, core, &req.requirement).len() >= req.min as usize)
}

/// Choose the core fulfillment for `place_idx` nearest to `cube` (the cube
/// itself preferred) whose surroundings satisfy every requirement.
fn choose_core(cw: &ConstructedCity, cube: IVec3, place_idx: usize) -> Option<FulfilledPorf> {
    let place = &cw.places[place_idx];
    let core_req = &place.requirements[0].requirement;
    let mut cores = candidates_near(cw, cube, core_req);
    cores.sort_by_key(|c| {
        let loc = fulfillment_location(cw, c);
        (loc != cube, manhattan2d(loc, cube))
    });
    cores
        .into_iter()
        .find(|core| requirements_met(cw, fulfillment_location(cw, core), place))
}

/// Places (indices into `cw.places`) that could be formed around `cube`.
pub fn valid_places_for(cw: &ConstructedCity, cube: IVec3) -> Vec<usize> {
    (0..cw.places.len())
        .filter(|&idx| choose_core(cw, cube, idx).is_some())
        .collect()
}

fn place_contains(cw: &ConstructedCity, idx: usize, cube: IVec3) -> bool {
    cw.placed_places[idx].fulfillments.iter().any(|f| match f {
        FulfilledPorf::Furniture(c) => *c == cube,
        FulfilledPorf::Place(inner) => place_contains(cw, *inner, cube),
    })
}

/// The placed-place index (into `cw.placed_places`) that owns `cube`, if any
/// -- searching recursively through nested place fulfillments.
pub fn place_index_at(cw: &ConstructedCity, cube: IVec3) -> Option<usize> {
    (0..cw.placed_places.len()).find(|&idx| place_contains(cw, idx, cube))
}

/// The chain of places containing `cube`, innermost first, up to the root.
pub fn containing_chain(cw: &ConstructedCity, cube: IVec3) -> Vec<usize> {
    let mut chain = Vec::new();
    let Some(mut idx) = place_index_at(cw, cube) else {
        return chain;
    };
    loop {
        chain.push(idx);
        match cw
            .placed_places
            .iter()
            .position(|pp| pp.fulfillments.contains(&FulfilledPorf::Place(idx)))
        {
            Some(parent) => idx = parent,
            None => break,
        }
    }
    chain
}

/// The placed-place index directly holding fulfillment `f` (not recursive --
/// only used to find donors during (re)assignment).
fn owner_of(cw: &ConstructedCity, f: &FulfilledPorf) -> Option<usize> {
    cw.placed_places
        .iter()
        .position(|pp| pp.fulfillments.contains(f))
}

/// A pre-computed assignment, shared by the panel preview and the commit so the
/// displayed "Pulls {N}" can never disagree with the actual effect.
pub struct AssignmentPlan {
    /// Fulfillments the new place will own.
    pub chosen: Vec<FulfilledPorf>,
    /// How many of `chosen` had to be pulled from other places to meet a min.
    pub pulled: usize,
    /// Indices into `placed_places` that drop below a min and must be destroyed.
    ///
    /// NOTE: these indices are only valid against `cw.placed_places` as it
    /// stood when this plan was computed. `commit_assignment` removes them in
    /// descending order so earlier indices stay valid *for this call*, but if
    /// `chosen` also contains a `FulfilledPorf::Place` (a nested place used to
    /// satisfy a `Porf::Place` requirement), removing a lower-indexed destroyed
    /// place would shift that reference. No current place definitions nest,
    /// so this can't happen today -- it would need a stable-ID rework
    /// (replacing `usize` indices with real IDs) before nesting is exercised
    /// in anger.
    pub destroy: Vec<usize>,
}

/// Plan assigning fulfillments to a new instance of `place_idx` around `cube`.
/// Prefers unassigned furniture/places; only pulls from other places to reach `min`.
pub fn plan_assignment(
    cw: &ConstructedCity,
    cube: IVec3,
    place_idx: usize,
) -> Option<AssignmentPlan> {
    let core = choose_core(cw, cube, place_idx)?;
    let core_loc = fulfillment_location(cw, &core);
    let place = &cw.places[place_idx];

    let mut chosen: Vec<FulfilledPorf> = Vec::new();
    // For each donor place, which of its fulfillments we'd take.
    let mut pulled_from: std::collections::HashMap<usize, Vec<FulfilledPorf>> =
        std::collections::HashMap::new();

    for req in &place.requirements {
        let max = req.max.map(|m| m as usize).unwrap_or(usize::MAX);
        let min = req.min as usize;

        // Partition reachable fulfillments into unassigned ("free") and those
        // already owned by another place, keeping each owner's index.
        let mut free: Vec<FulfilledPorf> = Vec::new();
        let mut assigned: Vec<(FulfilledPorf, usize)> = Vec::new();
        for c in candidates_near(cw, core_loc, &req.requirement) {
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
            pulled_from.entry(owner).or_default().push(c.clone());
            taken.push(c);
        }
        chosen.extend(taken);
    }

    // A donor place is destroyed if, after losing its pulled fulfillments, it
    // no longer meets some minimum.
    let mut destroy = Vec::new();
    for (&pp_idx, pulled_fs) in &pulled_from {
        let pp = &cw.placed_places[pp_idx];
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
            destroy.push(pp_idx);
        }
    }
    // Descending so `commit_assignment` can `remove` by index without shifting.
    destroy.sort_unstable_by(|a, b| b.cmp(a));

    let pulled = pulled_from.values().map(Vec::len).sum();
    Some(AssignmentPlan {
        chosen,
        pulled,
        destroy,
    })
}

/// Commit an assignment: create the place, pulling/destroying as planned.
pub fn commit_assignment(cw: &mut ConstructedCity, cube: IVec3, place_idx: usize) {
    let Some(plan) = plan_assignment(cw, cube, place_idx) else {
        return;
    };

    // Take chosen fulfillments away from any place currently holding them.
    for pp in &mut cw.placed_places {
        pp.fulfillments.retain(|f| !plan.chosen.contains(f));
    }

    // Destroy donor places that fell below a minimum. `plan.destroy` is sorted
    // descending so earlier indices stay valid. Their inventory is discarded.
    for idx in &plan.destroy {
        cw.placed_places.remove(*idx);
    }

    let max_volume = 20.0 * plan.chosen.len() as f32;
    cw.placed_places.push(ParticularPlace {
        place: place_idx,
        fulfillments: plan.chosen,
        contents: Inventory::new(8, max_volume),
    });
}

/// Remove a placed place, discarding its inventory contents.
pub fn unassign_place(cw: &mut ConstructedCity, idx: usize) {
    if idx < cw.placed_places.len() {
        cw.placed_places.remove(idx);
    }
}

/// All furniture cubes named `name` anywhere in the grid (unbounded, unlike
/// `furniture_of_name_near`) -- used by `sync_places` to enumerate candidate
/// cores without a "clicked cube" to search near.
fn all_furniture_named(cw: &ConstructedCity, name: &str) -> Vec<IVec3> {
    cw.contents
        .iter()
        .filter(|(loc, _)| loc.slot == Slot::Room)
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
    let mut changed = false;

    // Dissolve any existing place that no longer meets its own minimums.
    loop {
        let stale = (0..cw.placed_places.len()).find(|&idx| {
            let def = &cw.places[cw.placed_places[idx].place];
            !requirements_met(cw, place_location(cw, idx), def)
        });
        match stale {
            Some(idx) => {
                cw.placed_places.remove(idx);
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
        let cubes: Vec<IVec3> = match &core_req.requirement {
            Porf::Furniture(name) => all_furniture_named(cw, name),
            Porf::Place(name) => (0..cw.placed_places.len())
                .filter(|&i| cw.places[cw.placed_places[i].place].name == *name)
                .map(|i| place_location(cw, i))
                .collect(),
        };
        candidates.extend(cubes.into_iter().map(|cube| (cube, place_idx)));
    }
    candidates.sort_by_key(|(cube, place_idx)| (cube.x, cube.y, cube.z, *place_idx));

    for (cube, place_idx) in candidates {
        if place_index_at(cw, cube).is_some() {
            continue;
        }
        if plan_assignment(cw, cube, place_idx).is_some() {
            commit_assignment(cw, cube, place_idx);
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

/// Total number of tools of `kind` held across all storage places.
pub fn total_tools_of(cw: &ConstructedCity, kind: ToolKind) -> u32 {
    (0..cw.placed_places.len())
        .filter(|&i| is_storage(cw, i))
        .map(|i| cw.placed_places[i].contents.tool_count_of(kind) as u32)
        .sum()
}

/// Remove one tool of `kind` from the first storage place that holds one.
/// Returns `true` if a tool was removed.
pub fn consume_tool(cw: &mut ConstructedCity, kind: ToolKind) -> bool {
    for i in 0..cw.placed_places.len() {
        if !is_storage(cw, i) {
            continue;
        }
        if cw.placed_places[i]
            .contents
            .remove_unique(&UniqueResource::Tool(kind))
        {
            return true;
        }
    }
    false
}

/// Deposit one tool of `kind` into the first storage place. Returns `true` on
/// success (`false` if there is no storage place to receive it).
pub fn deposit_tool(cw: &mut ConstructedCity, kind: ToolKind) -> bool {
    if let Some(i) = (0..cw.placed_places.len()).find(|&i| is_storage(cw, i)) {
        cw.placed_places[i]
            .contents
            .add_unique(UniqueResource::Tool(kind));
        true
    } else {
        false
    }
}

fn is_storage(cw: &ConstructedCity, pp_idx: usize) -> bool {
    cw.places
        .get(cw.placed_places[pp_idx].place)
        .is_some_and(|info| info.storage.is_some())
}

/// Total quantity of `res` held across all storage places.
pub fn total_uniform(cw: &ConstructedCity, res: UniformResource) -> u32 {
    (0..cw.placed_places.len())
        .filter(|&i| is_storage(cw, i))
        .flat_map(|i| cw.placed_places[i].contents.uniform_totals())
        .filter(|(r, _)| *r == res)
        .map(|(_, q)| q as u32)
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
    for i in 0..cw.placed_places.len() {
        if remaining == 0 {
            break;
        }
        if !is_storage(cw, i) {
            continue;
        }
        let here = cw.placed_places[i]
            .contents
            .uniform_totals()
            .into_iter()
            .find(|(r, _)| *r == res)
            .map(|(_, q)| q as u32)
            .unwrap_or(0);
        let take = here.min(remaining);
        if take > 0 {
            cw.placed_places[i]
                .contents
                .subtract_uniform(res, take as u16);
            remaining -= take;
        }
    }
    true
}

/// The starting storage room: a 4×3 area set one cell back from the road's NE
/// inside corner. The E-W road occupies z ∈ [0, 4); the north arm occupies
/// x ∈ [0, 4) for z ≥ 4. Stepping one cell off both road edges puts the room at
/// x ∈ [5, 9), z ∈ [5, 8).
const ROOM_X: std::ops::Range<i32> = 5..9;
const ROOM_Z: std::ops::Range<i32> = 5..8;
const NUM_BINS: usize = 5;

/// Places the starting storage room with randomly-positioned bins and market
/// stands, pre-stocked with potatoes, timber, and canvas, directly into
/// `constructed` (real cells, no proposal step) and registers their places.
/// Returns the real-cell deltas for the caller to pass to `apply_changes`.
/// Pure aside from `rng`, so it can be driven deterministically (e.g. by the
/// headless testing harness) as well as by the `spawn_initial_places` startup system.
pub fn place_initial_places(
    constructed: &mut ConstructedCity,
    rng: &mut impl rand::Rng,
) -> Vec<(SlotCoord, Option<Cell>)> {
    let Some(bin_id) = constructed.find_structure_by_name("bin") else {
        return Vec::new();
    };
    let Some(storage_room_index) = constructed
        .places
        .iter()
        .position(|s| s.name == "storage room")
    else {
        return Vec::new();
    };

    // Pick NUM_BINS distinct cells from the 4×3 footprint.
    let mut candidates: Vec<IVec3> = Vec::new();
    for x in ROOM_X {
        for z in ROOM_Z {
            candidates.push(IVec3::new(x, 0, z));
        }
    }
    use rand::seq::SliceRandom;
    candidates.shuffle(rng);
    let chosen: Vec<IVec3> = candidates.into_iter().take(NUM_BINS).collect();

    // Place the bins as real cells and spawn their meshes.
    let mut changes: Vec<(SlotCoord, Option<Cell>)> = Vec::new();
    for cube in &chosen {
        let loc = SlotCoord {
            cube: *cube,
            slot: Slot::Room,
        };
        let cell = Cell {
            id: bin_id,
            facing: Facing::default(),
            evaluation: None,
            material: Material::Planks,
            build_material: BuildMaterialId::default(),
        };
        constructed.contents.set(loc, cell.clone());
        changes.push((loc, Some(cell)));
    }

    // Stock the inventory and register the storage room place.
    let mut inv = Inventory::new(8, 20.0 * NUM_BINS as f32);
    inv.add_uniform(UniformResource::Potato, 9);
    inv.add_uniform(UniformResource::Timber, 20);
    inv.add_uniform(UniformResource::Canvas, 10);

    constructed.placed_places.push(ParticularPlace {
        place: storage_room_index,
        fulfillments: chosen
            .iter()
            .map(|c| FulfilledPorf::Furniture(*c))
            .collect(),
        contents: inv,
    });

    // Place market stands opposite the stockpile (south of the E-W road at z = -1),
    // with one space between each structure.
    let Some(market_stand_id) = constructed.find_structure_by_name("market stand") else {
        return changes;
    };
    let Some(market_stand_place_index) = constructed
        .places
        .iter()
        .position(|s| s.name == "market stand")
    else {
        return changes;
    };

    let market_stand_positions = [
        IVec3::new(1, 0, -1),
        IVec3::new(3, 0, -1),
        IVec3::new(5, 0, -1),
    ];
    for cube in &market_stand_positions {
        let loc = SlotCoord {
            cube: *cube,
            slot: Slot::Room,
        };
        let cell = Cell {
            id: market_stand_id,
            facing: Facing::default(),
            evaluation: None,
            material: Material::Planks,
            build_material: BuildMaterialId::default(),
        };
        constructed.contents.set(loc, cell.clone());
        changes.push((loc, Some(cell)));
    }

    // Register each market stand as its own place.
    for cube in &market_stand_positions {
        constructed.placed_places.push(ParticularPlace {
            place: market_stand_place_index,
            fulfillments: vec![FulfilledPorf::Furniture(*cube)],
            contents: Inventory::new(8, 20.0),
        });
    }

    changes
}

/// Startup system: places the initial places using thread-local randomness
/// and spawns its meshes. Must run after `spawn_grid`.
pub fn spawn_initial_places(
    mut commands: Commands,
    eorf_list: Res<EorfList>,
    mut constructed: ResMut<ConstructedCity>,
    mut assembled: ResMut<AssembledCity>,
) {
    let changes = place_initial_places(&mut constructed, &mut rand::rng());
    apply_changes(&mut commands, &mut assembled, &eorf_list, changes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eorf::{EorfInfo, PlacementStyle, StructureEmbedding};

    fn bin_structures() -> Vec<EorfInfo> {
        vec![EorfInfo {
            name: "bin".to_string(),
            placement_style: PlacementStyle::RoomPlop,
            x_char: None,
            z_char: None,
            embedding: StructureEmbedding {
                tall: 0.0,
                passable: 0.0,
                decorative: 0.0,
                striated: 0.0,
            },
            kind: crate::eorf::FurnitureOrElement::Furniture(vec![(
                crate::resource::UniformResource::Plank,
                1,
            )]),
        }]
    }

    fn place_def(min: u8, max: Option<u8>) -> PlaceInfo {
        PlaceInfo {
            name: "storage room".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("bin".to_string()),
                min,
                max,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            storage: None,
        }
    }

    fn grid_with_bins(def: PlaceInfo, bins: &[IVec3]) -> ConstructedCity {
        let mut cw = ConstructedCity::new(bin_structures());
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
                    material: Material::Planks,
                    build_material: BuildMaterialId::default(),
                },
            );
        }
        cw
    }

    fn b(x: i32, z: i32) -> IVec3 {
        IVec3::new(x, 0, z)
    }

    fn f(cube: IVec3) -> FulfilledPorf {
        FulfilledPorf::Furniture(cube)
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
    fn pulls_to_meet_min_and_destroys_starved_donor() {
        // Place needs min 2 bins. An existing place owns two of three bins.
        let mut grid = grid_with_bins(place_def(2, None), &[b(0, 0), b(0, 1), b(0, 2)]);
        grid.placed_places.push(ParticularPlace {
            place: 0,
            fulfillments: vec![f(b(0, 1)), f(b(0, 2))],
            contents: Inventory::new(8, 40.0),
        });

        // Right-click the free bin and form a new place from it.
        let plan = plan_assignment(&grid, b(0, 0), 0).unwrap();
        assert_eq!(plan.pulled, 1, "should pull exactly one bin to reach min 2");
        assert_eq!(
            plan.destroy,
            vec![0],
            "donor falls below min and is destroyed"
        );

        commit_assignment(&mut grid, b(0, 0), 0);
        assert_eq!(
            grid.placed_places.len(),
            1,
            "donor destroyed, new one added"
        );
        let new = &grid.placed_places[0];
        assert!(new.fulfillments.contains(&f(b(0, 0))));
        assert_eq!(new.fulfillments.len(), 2);
    }

    #[test]
    fn unassign_removes_the_place() {
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0)]);
        grid.placed_places.push(ParticularPlace {
            place: 0,
            fulfillments: vec![f(b(0, 0))],
            contents: Inventory::new(8, 20.0),
        });
        assert_eq!(place_index_at(&grid, b(0, 0)), Some(0));
        unassign_place(&mut grid, 0);
        assert!(grid.placed_places.is_empty());
        assert_eq!(place_index_at(&grid, b(0, 0)), None);
    }

    #[test]
    fn unused_furniture_not_part_of_any_place() {
        let grid = grid_with_bins(place_def(1, None), &[b(0, 0)]);
        assert_eq!(place_index_at(&grid, b(0, 0)), None);
        assert_eq!(valid_places_for(&grid, b(0, 0)), vec![0]);
    }

    #[test]
    fn sync_places_forms_and_dissolves_automatically() {
        let mut grid = grid_with_bins(place_def(1, None), &[b(0, 0), b(5, 5)]);
        assert!(sync_places(&mut grid));
        assert_eq!(grid.placed_places.len(), 2);
        assert!(place_index_at(&grid, b(0, 0)).is_some());
        assert!(place_index_at(&grid, b(5, 5)).is_some());

        // Re-running with nothing changed is a no-op.
        assert!(!sync_places(&mut grid));

        // Removing a bin dissolves its place.
        grid.contents.take(SlotCoord {
            cube: b(0, 0),
            slot: Slot::Room,
        });
        assert!(sync_places(&mut grid));
        assert_eq!(grid.placed_places.len(), 1);
        assert!(place_index_at(&grid, b(5, 5)).is_some());
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

        // Place a pallet far from the initial storage room / market stands so
        // it can't be swept into an existing place.
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
}
