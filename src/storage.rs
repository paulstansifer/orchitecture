//! Inventory and capacity accounting over the city's `public_storage` places.
//!
//! Split out of [`crate::place`], which owns how places *form* (requirement
//! resolution, core anchors, `sync_places`). This module owns what the formed
//! places *hold*: it reads the same `ConstructedCity`, working over whichever
//! placed places have `Place::public_storage` set (see [`storage_ids`]).
//!
//! Three storage kinds run in parallel throughout, and each furniture piece
//! declares capacity per kind (see [`crate::resource::StorageKind`]):
//!   * `Bulk` backs `UniformResource`s -- what bins hold, optionally
//!     restricted per-cube to a single resource.
//!   * `Rack` backs the tool/rug `UniqueResource`s, dedicated per-cube to
//!     `RackContents::{Tools,Rugs}` (racks have no "unrestricted" option).
//!   * `Book` backs books in bookcases, with no per-cube dedication.
//!
//! Each kind has the same shape of helper: a per-place capacity ceiling, a
//! per-place free capacity, and a city-wide sum over [`storage_ids`].

use std::collections::HashMap;

use bevy::math::IVec3;

use crate::city::ConstructedCity;
use crate::place::{FulfilledPorf, ParticularPlace, PlacedPlaceId};
use crate::resource::{
    RackContents, StorageKind, ToolKind, UniformResource, UniqueResource, UniqueResourceKind,
};
use crate::sparse3d::{Slot, SlotCoord};

/// Total number of tools of `kind` held across all rack storage places.
pub fn total_tools_of(cw: &ConstructedCity, kind: ToolKind) -> u32 {
    storage_ids(cw)
        .into_iter()
        .map(|id| cw.placed_places[id].contents.tool_count_of(kind) as u32)
        .sum()
}

/// Total number of tools (of any kind) held across all rack storage places.
pub fn total_tool_count(cw: &ConstructedCity) -> u32 {
    storage_ids(cw)
        .into_iter()
        .map(|id| cw.placed_places[id].contents.tool_count() as u32)
        .sum()
}

/// Total number of rugs held across all rack storage places.
pub fn total_rug_count(cw: &ConstructedCity) -> u32 {
    storage_ids(cw)
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
    storage_ids(cw)
        .into_iter()
        .map(|id| rack_capacity_ceiling(cw, &cw.placed_places[id], contents))
        .sum()
}

/// Remove one tool of `kind` from the first rack storage place that holds one.
/// Returns `true` if a tool was removed.
pub fn consume_tool(cw: &mut ConstructedCity, kind: ToolKind) -> bool {
    for id in storage_ids(cw) {
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
    for id in storage_ids(cw) {
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
    for id in storage_ids(cw) {
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
pub(crate) fn slot_storage_capacity(
    cw: &ConstructedCity,
    loc: SlotCoord,
    kind: StorageKind,
) -> f32 {
    cw.contents
        .get(loc)
        .map(|cell| cw.eorfs[cell.id.as_usize()].storage_capacity_for(kind))
        .unwrap_or(0.0)
}

/// The storage capacity `cube`'s room-slot furniture contributes for `kind`
/// (0.0 if it has none, or nothing is there). For callers that hold a bare cube
/// (e.g. a clicked location); fulfillment-based callers use
/// [`slot_storage_capacity`] so wall furniture is handled correctly.
fn cube_storage_capacity(cw: &ConstructedCity, cube: IVec3, kind: StorageKind) -> f32 {
    slot_storage_capacity(
        cw,
        SlotCoord {
            cube,
            slot: Slot::Room,
        },
        kind,
    )
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
            FulfilledPorf::Furniture(loc) => (cw
                .rack_restrictions
                .get(&loc.cube)
                .copied()
                .unwrap_or_default()
                == contents)
                .then(|| slot_storage_capacity(cw, *loc, StorageKind::Rack)),
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
    storage_ids(cw)
        .into_iter()
        .map(|id| rack_free_capacity_for(cw, &cw.placed_places[id], contents))
        .sum()
}

/// True if `cube` is a fulfillment (with `Bulk` capacity) of some
/// `public_storage` place -- used by the UI to decide whether to offer a
/// per-bin resource-restriction dropdown.
pub fn cube_is_storage_bin(cw: &ConstructedCity, cube: IVec3) -> bool {
    let room = FulfilledPorf::Furniture(SlotCoord {
        cube,
        slot: Slot::Room,
    });
    cube_storage_capacity(cw, cube, StorageKind::Bulk) > 0.0
        && cw
            .placed_places
            .iter()
            .any(|(_, pp)| is_public_storage(cw, pp) && pp.fulfillments.contains(&room))
}

/// True if `cube` is a fulfillment (with `Rack` capacity) of some
/// `public_storage` place -- used by the UI to decide whether to offer a
/// per-rack contents dropdown.
pub fn cube_is_rack(cw: &ConstructedCity, cube: IVec3) -> bool {
    let room = FulfilledPorf::Furniture(SlotCoord {
        cube,
        slot: Slot::Room,
    });
    cube_storage_capacity(cw, cube, StorageKind::Rack) > 0.0
        && cw
            .placed_places
            .iter()
            .any(|(_, pp)| is_public_storage(cw, pp) && pp.fulfillments.contains(&room))
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
        let accounting = info
            .accounting
            .unwrap_or(crate::place::DEFAULT_STORAGE_ACCOUNTING);
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
/// capacity of every fulfillment that is unrestricted or restricted to
/// `res`; bins restricted to a different resource contribute nothing.
fn place_capacity_ceiling(cw: &ConstructedCity, pp: &ParticularPlace, res: UniformResource) -> f32 {
    pp.fulfillments
        .iter()
        .filter_map(|f| match f {
            FulfilledPorf::Furniture(loc) => cw
                .bin_resource_restrictions
                .get(&loc.cube)
                .is_none_or(|r| *r == res)
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
/// — the hard ceiling on how much can be stored this month regardless of how
/// it splits across resources. Since dedicated (restricted) bins' free
/// capacity is disjoint per resource but unrestricted bins' capacity is
/// shared, summing `storage_free_capacity` across several contending
/// resources can double-count shared bins; this is the safety net used by
/// `resource::distribute_incoming_resources` to cap that sum.
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

/// Total volume across all storage places' bins that are *not* dedicated to
/// any particular resource — i.e. general-purpose room available to
/// whichever resource wants it, as opposed to a bin restricted to one
/// resource (see [`ConstructedCity::bin_resource_restrictions`]).
pub fn storage_shared_ceiling(cw: &ConstructedCity) -> f32 {
    storage_ids(cw)
        .into_iter()
        .flat_map(|id| cw.placed_places[id].fulfillments.iter())
        .filter_map(|f| match f {
            FulfilledPorf::Furniture(loc)
                if !cw.bin_resource_restrictions.contains_key(&loc.cube) =>
            {
                Some(slot_storage_capacity(cw, *loc, StorageKind::Bulk))
            }
            _ => None,
        })
        .sum()
}

/// Per-resource capacity ceiling contributed by bins dedicated (restricted)
/// to that resource specifically, summed across all storage places. Resources
/// with no dedicated bins are absent from the map.
pub fn storage_dedicated_ceilings(cw: &ConstructedCity) -> HashMap<UniformResource, f32> {
    let mut ceilings = HashMap::new();
    for id in storage_ids(cw) {
        for f in &cw.placed_places[id].fulfillments {
            if let FulfilledPorf::Furniture(loc) = f {
                if let Some(&res) = cw.bin_resource_restrictions.get(&loc.cube) {
                    *ceilings.entry(res).or_insert(0.0) +=
                        slot_storage_capacity(cw, *loc, StorageKind::Bulk);
                }
            }
        }
    }
    ceilings
}

/// Free volume across shared (undedicated) storage bins, given a snapshot of
/// per-resource storage `totals`. Dedicated bins' free space is excluded even
/// when empty, since that room is earmarked for one resource rather than
/// generally available; any stored amount beyond what a resource's own
/// dedicated bins can hold is assumed to occupy shared bins.
pub fn uncommitted_free_capacity(
    shared_ceiling: f32,
    dedicated_ceilings: &HashMap<UniformResource, f32>,
    totals: &HashMap<UniformResource, u32>,
) -> f32 {
    let shared_used: f32 = totals
        .iter()
        .map(|(res, &qty)| {
            (qty as f32 - dedicated_ceilings.get(res).copied().unwrap_or(0.0)).max(0.0)
        })
        .sum();
    (shared_ceiling - shared_used).max(0.0)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::city::Cell;
    use crate::materials::BuildMaterialId;
    use crate::place::test_fixtures::*;
    use crate::place::{ParentRestriction, Place, PlaceReq, Porf};
    use crate::resource::{Approximation, Inventory};
    use crate::sparse3d::Facing;

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
        let cw = grid_with_storage(storage_place_def(), inv);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Timber), 8.0);
    }

    #[test]
    fn deposit_with_capacity_caps_at_remaining_volume() {
        let inv = Inventory::new([(StorageKind::Bulk, 5.0)]);
        let mut cw = grid_with_storage(storage_place_def(), inv);
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
            .insert(b(0, 0), UniformResource::Timber);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Timber), 15.0);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Straw), 0.0);
    }

    #[test]
    fn one_restricted_and_one_unrestricted_bin_split_capacity_per_resource() {
        let inv = Inventory::new([(StorageKind::Bulk, 40.0)]);
        let mut cw = grid_with_storage_bins(storage_place_def(), &[b(0, 0), b(0, 1)], inv);
        cw.bin_resource_restrictions
            .insert(b(0, 0), UniformResource::Timber);
        // b(0, 1) is unrestricted, so it counts toward every resource's
        // ceiling; b(0, 0) only counts toward Timber's.
        assert_eq!(storage_free_capacity(&cw, UniformResource::Timber), 40.0);
        assert_eq!(storage_free_capacity(&cw, UniformResource::Straw), 20.0);
    }

    #[test]
    fn deposit_with_capacity_refuses_to_overfill_a_restricted_bin() {
        let inv = Inventory::new([(StorageKind::Bulk, 20.0)]);
        let mut cw = grid_with_storage_bins(storage_place_def(), &[b(0, 0)], inv);
        cw.bin_resource_restrictions
            .insert(b(0, 0), UniformResource::Timber);
        let deposited = deposit_uniform_with_capacity(&mut cw, UniformResource::Straw, 10);
        assert_eq!(deposited, 0);
        assert_eq!(storage_totals(&cw).get(&UniformResource::Straw), None);
    }

    #[test]
    fn shared_ceiling_excludes_dedicated_bins() {
        let inv = Inventory::new([(StorageKind::Bulk, 40.0)]);
        let mut cw = grid_with_storage_bins(storage_place_def(), &[b(0, 0), b(0, 1)], inv);
        cw.bin_resource_restrictions
            .insert(b(0, 0), UniformResource::Timber);
        // Only b(0, 1) is unrestricted.
        assert_eq!(storage_shared_ceiling(&cw), 20.0);
        assert_eq!(
            storage_dedicated_ceilings(&cw).get(&UniformResource::Timber),
            Some(&20.0)
        );
    }

    #[test]
    fn uncommitted_capacity_excludes_dedicated_room_even_when_empty() {
        let inv = Inventory::new([(StorageKind::Bulk, 40.0)]);
        let mut cw = grid_with_storage_bins(storage_place_def(), &[b(0, 0), b(0, 1)], inv);
        cw.bin_resource_restrictions
            .insert(b(0, 0), UniformResource::Timber);
        let dedicated = storage_dedicated_ceilings(&cw);
        let shared_ceiling = storage_shared_ceiling(&cw);
        // No Timber stored yet, but the dedicated bin still isn't "uncommitted".
        assert_eq!(
            uncommitted_free_capacity(shared_ceiling, &dedicated, &storage_totals(&cw)),
            20.0
        );
    }

    #[test]
    fn uncommitted_capacity_counts_overflow_into_shared_bins() {
        let mut inv = Inventory::new([(StorageKind::Bulk, 40.0)]);
        inv.add_uniform(UniformResource::Timber, 25);
        let mut cw = grid_with_storage_bins(storage_place_def(), &[b(0, 0), b(0, 1)], inv);
        cw.bin_resource_restrictions
            .insert(b(0, 0), UniformResource::Timber);
        let dedicated = storage_dedicated_ceilings(&cw);
        let shared_ceiling = storage_shared_ceiling(&cw);
        // Timber's dedicated bin (20) is full; the extra 5 units spill into
        // the 20-unit shared bin, leaving 15 uncommitted.
        assert_eq!(
            uncommitted_free_capacity(shared_ceiling, &dedicated, &storage_totals(&cw)),
            15.0
        );
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
        cw.rack_restrictions.insert(b(0, 0), RackContents::Rugs);
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
        cw.rack_restrictions.insert(b(0, 0), RackContents::Rugs);
        // b(0, 0) is dedicated to Rugs, so the tool must land via b(0, 1),
        // which defaults to Tools.
        assert!(deposit_tool(&mut cw, ToolKind::CarpentersTools));
        assert_eq!(total_tool_count(&cw), 1);
    }

    #[test]
    fn bins_never_hold_tools_or_rugs() {
        // A plain bin-backed storage room has no rack fulfillments at all, so
        // it's never returned by `storage_ids` and can't receive a tool.
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
        cw.rack_restrictions.insert(b(0, 0), RackContents::Rugs);
        // b(0, 1) defaults to Tools; deposit one so free capacity (10) no
        // longer equals the total ceiling (still 10, since it's per-cube).
        deposit_tool(&mut cw, ToolKind::CarpentersTools);
        assert_eq!(rack_capacity(&cw, RackContents::Tools), 10.0);
        assert_eq!(rack_capacity(&cw, RackContents::Rugs), 10.0);
        assert_eq!(total_rug_count(&cw), 0);
    }
}
