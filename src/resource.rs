use std::collections::HashMap;

use rgb::Rgb;
use serde::{Deserialize, Serialize};

// Only need to store a quantity, since these are all equivalent
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum UniformResource {
    Potato,
    Timber,
    Straw,
    Canvas,
    Fieldstone,
    Block,
    Lime,
    Plank,
}

impl UniformResource {
    pub const ALL: &'static [UniformResource] = &[
        UniformResource::Potato,
        UniformResource::Timber,
        UniformResource::Straw,
        UniformResource::Canvas,
        UniformResource::Fieldstone,
        UniformResource::Block,
        UniformResource::Lime,
        UniformResource::Plank,
    ];
}

impl UniformResource {
    pub fn label(self) -> &'static str {
        match self {
            UniformResource::Potato => "Potatoes",
            UniformResource::Timber => "Timber",
            UniformResource::Straw => "Straw",
            UniformResource::Canvas => "Canvas",
            UniformResource::Fieldstone => "Fieldstone",
            UniformResource::Block => "Blocks",
            UniformResource::Lime => "Lime",
            UniformResource::Plank => "Planks",
        }
    }

    /// Maximum units of this material construction can absorb per month —
    /// its construction rate. Currently the same for every material; the
    /// per-material split exists so individual materials can diverge later.
    pub fn construct_per_month(self) -> u32 {
        50
    }

    /// Whether tearing down a structure built with this resource returns it
    /// to storage.
    pub fn refundable(self) -> bool {
        !matches!(self, UniformResource::Lime | UniformResource::Straw)
    }

    pub fn farmable(self) -> bool {
        // TODO: add quarries; Lime should not be farmable, and blocks should be getable
        // TODO: add ground-clearing; Fieldstone should not be farmable
        matches!(
            self,
            UniformResource::Potato
                | UniformResource::Canvas
                | UniformResource::Straw
                | UniformResource::Timber
                | UniformResource::Lime
                | UniformResource::Fieldstone
        )
    }

    /// Whether this resource is food. Potato is the sole edible resource (see
    /// `Eat` / `POTATOES_PER_INDIVIDUAL`).
    pub fn edible(self) -> bool {
        matches!(self, UniformResource::Potato)
    }

    /// Every farmable resource that isn't food — the pool a farm produces or
    /// re-rolls into. Derived from `farmable()` so the two can't drift apart.
    pub fn inedible_farmables() -> Vec<UniformResource> {
        Self::ALL
            .iter()
            .copied()
            .filter(|r| r.farmable() && !r.edible())
            .collect()
    }
}

/// The kind of a tool. Tools are otherwise interchangeable, but their kind
/// determines what a farm produces when it specializes with that tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub enum ToolKind {
    CarpentersTools,
}

/// How a tool transforms neighbouring production when a farm specializes:
/// it converts `input` produced by adjacent farms into `output`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Specialization {
    pub input: UniformResource,
    pub output: UniformResource,
}

impl ToolKind {
    pub fn label(self) -> &'static str {
        match self {
            ToolKind::CarpentersTools => "Carpenter's tools",
        }
    }

    /// What this tool turns neighboring production into when a farm specializes.
    pub fn specialization(self) -> Specialization {
        match self {
            ToolKind::CarpentersTools => Specialization {
                input: UniformResource::Timber,
                output: UniformResource::Plank,
            },
        }
    }

    pub fn rate(self) -> f32 {
        4.0
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum UniqueResource {
    Book { title: String },
    Rug { color: Rgb<u8> },
    Tool(ToolKind),
}

/// The broad category of a `UniqueResource`, used to constrain what a piece of
/// furniture's slot may hold (see `eorf::FurnitureSlot`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum UniqueResourceKind {
    Book,
    Rug,
    Tool,
}

impl UniqueResourceKind {
    pub fn label(self) -> &'static str {
        match self {
            UniqueResourceKind::Book => "Book",
            UniqueResourceKind::Rug => "Rug",
            UniqueResourceKind::Tool => "Tool",
        }
    }

    /// Whether `res` is of this kind.
    pub fn matches(self, res: &UniqueResource) -> bool {
        res.kind() == self
    }
}

/// What a Rack is dedicated to holding. Unlike a bin's `UniformResource`
/// restriction, a rack has no "unrestricted" option -- it always holds one or
/// the other, defaulting to `Tools`. (Books live in bookcases, not racks --
/// see `StorageKind::Book`.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum RackContents {
    #[default]
    Tools,
    Rugs,
}

/// Which kind of storage a piece of furniture provides -- see
/// `EorfInfo::storage_capacity`. `Bulk` backs `UniformResource`s (what bins
/// hold); `Rack` backs the tool/rug `UniqueResource`s, further dedicated
/// per-cube to `RackContents::{Tools,Rugs}` (what racks hold); `Book` backs
/// book `UniqueResource`s (what bookcases hold, with no per-cube dedication).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub enum StorageKind {
    Bulk,
    Rack,
    Book,
}

impl RackContents {
    pub fn label(self) -> &'static str {
        match self {
            RackContents::Tools => "Tools",
            RackContents::Rugs => "Rugs",
        }
    }
}

impl UniqueResource {
    pub fn kind(&self) -> UniqueResourceKind {
        match self {
            UniqueResource::Book { .. } => UniqueResourceKind::Book,
            UniqueResource::Rug { .. } => UniqueResourceKind::Rug,
            UniqueResource::Tool(_) => UniqueResourceKind::Tool,
        }
    }

    /// A short human-readable description of this specific resource, for UI
    /// lists (e.g. the install pop-up).
    pub fn label(&self) -> String {
        match self {
            UniqueResource::Book { title } => format!("Book: {title}"),
            UniqueResource::Rug { color } => {
                format!("Rug ({}, {}, {})", color.r, color.g, color.b)
            }
            UniqueResource::Tool(kind) => kind.label().to_string(),
        }
    }

    pub fn volume(&self) -> f32 {
        match self {
            UniqueResource::Book { .. } => 0.05,
            UniqueResource::Rug { .. } => 1.0,
            UniqueResource::Tool(_) => 0.5,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum InventoryEntry {
    Uniform(UniformResource, u32),
    Collection(Vec<UniqueResource>),
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Inventory {
    contents: Vec<InventoryEntry>,
    /// Capacity per `StorageKind`, tracked separately so that -- on a place
    /// like the starting wagon, whose furniture provides both `Bulk` and
    /// `Rack` capacity -- filling one up can never eat into the other's room.
    capacities: HashMap<StorageKind, f32>,
}

impl Inventory {
    pub fn new(capacities: impl IntoIterator<Item = (StorageKind, f32)>) -> Self {
        Inventory {
            contents: Vec::new(),
            capacities: capacities.into_iter().collect(),
        }
    }

    fn capacity_for(&self, kind: StorageKind) -> f32 {
        self.capacities.get(&kind).copied().unwrap_or(0.0)
    }

    /// Volume currently occupied within `kind`'s own pool: uniform resources
    /// for `Bulk`, or the matching `UniqueResource` subtype's volume for
    /// `Rack`/`Book`.
    fn volume_of_kind(&self, kind: StorageKind) -> f32 {
        match kind {
            StorageKind::Bulk => self
                .uniform_totals()
                .into_iter()
                .map(|(_, qty)| qty as f32)
                .sum(),
            StorageKind::Rack => self
                .unique_items()
                .filter(|i| matches!(i, UniqueResource::Tool(_) | UniqueResource::Rug { .. }))
                .map(UniqueResource::volume)
                .sum(),
            StorageKind::Book => self
                .unique_items()
                .filter(|i| matches!(i, UniqueResource::Book { .. }))
                .map(UniqueResource::volume)
                .sum(),
        }
    }

    /// Remaining volume this inventory can accept for `kind` before hitting
    /// its own dedicated capacity -- independent of how full any other kind's
    /// pool is.
    pub fn remaining_capacity(&self, kind: StorageKind) -> f32 {
        (self.capacity_for(kind) - self.volume_of_kind(kind)).max(0.0)
    }

    /// Adds a quantity of a uniform resource, merging into an existing entry of the
    /// same kind if present.
    pub fn add_uniform(&mut self, res: UniformResource, qty: u32) {
        for entry in &mut self.contents {
            if let InventoryEntry::Uniform(existing, existing_qty) = entry {
                if *existing == res {
                    *existing_qty = existing_qty.saturating_add(qty);
                    return;
                }
            }
        }
        self.contents.push(InventoryEntry::Uniform(res, qty));
    }

    /// Per-kind totals of uniform resources held in this inventory. `add_uniform`
    /// keeps at most one `Uniform` entry per resource, so no merging is needed here.
    pub fn uniform_totals(&self) -> Vec<(UniformResource, u32)> {
        self.contents
            .iter()
            .filter_map(|entry| match entry {
                InventoryEntry::Uniform(res, qty) => Some((*res, *qty)),
                InventoryEntry::Collection(_) => None,
            })
            .collect()
    }

    /// The `Collection` entry holding this inventory's unique items, if any
    /// have been added yet. There's at most one -- `add_unique` reuses it.
    fn collection_entry_mut(&mut self) -> Option<&mut Vec<UniqueResource>> {
        self.contents.iter_mut().find_map(|entry| match entry {
            InventoryEntry::Collection(items) => Some(items),
            InventoryEntry::Uniform(..) => None,
        })
    }

    pub fn unique_items(&self) -> impl Iterator<Item = &UniqueResource> {
        self.contents
            .iter()
            .filter_map(|entry| match entry {
                InventoryEntry::Collection(items) => Some(items.iter()),
                InventoryEntry::Uniform(..) => None,
            })
            .flatten()
    }

    pub fn add_unique(&mut self, item: UniqueResource) {
        if let Some(items) = self.collection_entry_mut() {
            items.push(item);
        } else {
            self.contents.push(InventoryEntry::Collection(vec![item]));
        }
    }

    pub fn tool_count(&self) -> usize {
        self.unique_items()
            .filter(|i| matches!(i, UniqueResource::Tool(_)))
            .count()
    }

    pub fn book_count(&self) -> usize {
        self.unique_items()
            .filter(|i| matches!(i, UniqueResource::Book { .. }))
            .count()
    }

    pub fn rug_count(&self) -> usize {
        self.unique_items()
            .filter(|i| matches!(i, UniqueResource::Rug { .. }))
            .count()
    }

    /// Count tools of a specific kind held in this inventory.
    pub fn tool_count_of(&self, kind: ToolKind) -> usize {
        self.unique_items()
            .filter(|i| matches!(i, UniqueResource::Tool(k) if *k == kind))
            .count()
    }

    /// Remove a single matching unique item. Returns `true` if one was removed.
    pub fn remove_unique(&mut self, item: &UniqueResource) -> bool {
        let Some(items) = self.collection_entry_mut() else {
            return false;
        };
        let Some(pos) = items.iter().position(|i| i == item) else {
            return false;
        };
        items.remove(pos);
        true
    }

    pub fn subtract_uniform(&mut self, res: UniformResource, qty: u32) {
        for entry in &mut self.contents {
            if let InventoryEntry::Uniform(existing, existing_qty) = entry {
                if *existing == res {
                    *existing_qty = existing_qty.saturating_sub(qty);
                    return;
                }
            }
        }
    }
}

/// Outcome of distributing one month's leftover (post-claims) incoming
/// resources across storage and loss. Invariant: `stored + lost ==
/// incoming_qty` for every resource present in the `incoming` passed to
/// `distribute_incoming_resources`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceFlow {
    pub stored: u32,
    pub lost: u32,
}

/// Distributes `incoming` resources across storage and loss. `incoming` is
/// expected to already be *leftover* — whatever wasn't claimed by Eat,
/// TravelerVisit, or Construction this month (see `city_effect`) — so this
/// function only ever handles the "what happens to the rest" question.
///
/// Bins can be individually restricted to one resource (see
/// `place::place_capacity_ceiling`), so `storage_free_capacity` entries for
/// different resources may draw on disjoint dedicated bins *and* on shared
/// unrestricted bins at the same time. When combined leftover exceeds
/// capacity, resources are bumped out starting from the most "plentiful"
/// one — `storage_snapshot[r]` descending, then `known_farm_output[r]`
/// descending, then `UniformResource`'s `Ord` as a final deterministic
/// tie-break — so scarce resources keep priority for the remaining space.
/// The overall budget is the *sum* of each contending resource's free
/// capacity (dedicated bins are additive), capped by `overall_free_capacity`
/// — the room-wide, resource-agnostic remaining volume (see
/// `place::storage_overall_free_capacity`) — so resources sharing the same
/// unrestricted bins can't have their free capacity double-counted.
pub fn distribute_incoming_resources(
    incoming: &[(UniformResource, u32)],
    storage_snapshot: &HashMap<UniformResource, u32>,
    storage_free_capacity: &HashMap<UniformResource, f32>,
    overall_free_capacity: f32,
    known_farm_output: &HashMap<UniformResource, u32>,
) -> HashMap<UniformResource, ResourceFlow> {
    let plentifulness_key = |r: &UniformResource| {
        (
            *storage_snapshot.get(r).unwrap_or(&0),
            *known_farm_output.get(r).unwrap_or(&0),
        )
    };
    // Sort descending by plentifulness (most plentiful first); `UniformResource`'s
    // derived `Ord` breaks remaining ties deterministically.
    let sort_most_plentiful_first = |xs: &mut Vec<UniformResource>| {
        xs.sort_by(|a, b| {
            plentifulness_key(b)
                .cmp(&plentifulness_key(a))
                .then(a.cmp(b))
        });
    };
    // Greedily zero out/reduce `amounts` starting from the most-plentiful
    // resource until their sum no longer exceeds `budget`.
    let cap_total = |amounts: &mut HashMap<UniformResource, u32>, budget: u32| {
        let total: u32 = amounts.values().sum();
        if total <= budget {
            return;
        }
        let mut excess = total - budget;
        let mut ordered: Vec<UniformResource> = amounts.keys().copied().collect();
        sort_most_plentiful_first(&mut ordered);
        for r in ordered {
            if excess == 0 {
                break;
            }
            let v = amounts.get_mut(&r).unwrap();
            let cut = (*v).min(excess);
            *v -= cut;
            excess -= cut;
        }
    };

    // Whatever arrived tries storage. Each resource's own free capacity is
    // an upper bound (dedicated bins are additive across resources), but the
    // combined budget can't exceed the room-wide overall free capacity
    // (shared, unrestricted bins can't be double-counted across resources).
    let mut stored: HashMap<UniformResource, u32> = incoming
        .iter()
        .filter(|(_, qty)| *qty > 0)
        .map(|(r, qty)| (*r, *qty))
        .collect();
    for (r, qty) in stored.iter_mut() {
        let own_capacity = storage_free_capacity.get(r).copied().unwrap_or(0.0);
        *qty = (*qty).min(own_capacity.max(0.0).floor() as u32);
    }
    let pool_capacity: f32 = stored
        .keys()
        .filter_map(|r| storage_free_capacity.get(r).copied())
        .sum();
    let max_storable = pool_capacity.min(overall_free_capacity).max(0.0).floor() as u32;
    cap_total(&mut stored, max_storable);

    // Assemble per-resource flows; remainder is lost.
    incoming
        .iter()
        .map(|(r, qty)| {
            let stored_qty = stored.get(r).copied().unwrap_or(0);
            let lost = qty - stored_qty;
            (
                *r,
                ResourceFlow {
                    stored: stored_qty,
                    lost,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod distribute_tests {
    use super::*;
    use UniformResource::*;

    fn m(pairs: &[(UniformResource, u32)]) -> HashMap<UniformResource, u32> {
        pairs.iter().copied().collect()
    }

    fn cap(pairs: &[(UniformResource, f32)]) -> HashMap<UniformResource, f32> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn fully_fits_in_storage() {
        let flows = distribute_incoming_resources(
            &[(Timber, 10)],
            &m(&[]),
            &cap(&[(Timber, 30.0)]),
            30.0,
            &m(&[]),
        );
        assert_eq!(
            flows[&Timber],
            ResourceFlow {
                stored: 10,
                lost: 0
            }
        );
    }

    #[test]
    fn excess_over_capacity_is_lost() {
        let flows = distribute_incoming_resources(
            &[(Timber, 10)],
            &m(&[]),
            &cap(&[(Timber, 3.0)]),
            3.0,
            &m(&[]),
        );
        assert_eq!(flows[&Timber], ResourceFlow { stored: 3, lost: 7 });
    }

    #[test]
    fn storage_contention_discards_most_plentiful_first() {
        // Both resources draw on the same shared (unrestricted) pool, so the
        // overall budget (25) is well below the sum of their individual caps
        // (50) -- this is the "one shared pool" case.
        let flows = distribute_incoming_resources(
            &[(Timber, 20), (Straw, 20)],
            &m(&[(Timber, 100), (Straw, 10)]), // Timber more plentiful in storage
            &cap(&[(Timber, 25.0), (Straw, 25.0)]),
            25.0,
            &m(&[]),
        );
        // Timber is most plentiful -> discarded first from storage.
        assert_eq!(flows[&Straw].stored, 20);
        assert_eq!(flows[&Timber].stored, 5);
        assert_eq!(flows[&Timber].lost, 15);
    }

    #[test]
    fn farm_output_breaks_ties_when_storage_equal() {
        let flows = distribute_incoming_resources(
            &[(Timber, 20), (Straw, 20)],
            &m(&[(Timber, 5), (Straw, 5)]), // tied storage
            &cap(&[(Timber, 25.0), (Straw, 25.0)]),
            25.0,
            &m(&[(Timber, 20), (Straw, 1)]), // Timber more plentiful via farm output
        );
        assert_eq!(flows[&Straw].stored, 20);
        assert_eq!(flows[&Timber].stored, 5);
    }

    #[test]
    fn no_storage_places_loses_everything() {
        let flows = distribute_incoming_resources(&[(Potato, 5)], &m(&[]), &cap(&[]), 0.0, &m(&[]));
        assert_eq!(flows[&Potato], ResourceFlow { stored: 0, lost: 5 });
    }

    #[test]
    fn conservation_holds_across_all_flows() {
        let flows = distribute_incoming_resources(
            &[(Timber, 17), (Potato, 8)],
            &m(&[(Timber, 2)]),
            &cap(&[(Timber, 4.0), (Potato, 4.0)]),
            4.0,
            &m(&[]),
        );
        let t = flows[&Timber];
        assert_eq!(t.stored + t.lost, 17);
        let p = flows[&Potato];
        assert_eq!(p.stored + p.lost, 8);
    }

    #[test]
    fn dedicated_bins_let_multiple_resources_store_simultaneously() {
        // Timber and Straw each have their own dedicated 20-unit bin, so the
        // room-wide overall capacity (40) allows both to fully store, unlike
        // the shared-pool case above where a single 25-unit budget forces
        // contention between them.
        let flows = distribute_incoming_resources(
            &[(Timber, 20), (Straw, 20)],
            &m(&[]),
            &cap(&[(Timber, 20.0), (Straw, 20.0)]),
            40.0,
            &m(&[]),
        );
        assert_eq!(
            flows[&Timber],
            ResourceFlow {
                stored: 20,
                lost: 0
            }
        );
        assert_eq!(
            flows[&Straw],
            ResourceFlow {
                stored: 20,
                lost: 0
            }
        );
    }

    #[test]
    fn overall_free_capacity_prevents_double_counting_a_shared_bin() {
        // Both resources are only eligible for the same single unrestricted
        // 20-unit bin, so each is independently reported as having 20 free
        // -- but the combined budget must stay at 20, not 40.
        let flows = distribute_incoming_resources(
            &[(Timber, 20), (Straw, 20)],
            &m(&[(Timber, 5), (Straw, 5)]), // tied storage
            &cap(&[(Timber, 20.0), (Straw, 20.0)]),
            20.0,
            &m(&[(Timber, 1), (Straw, 1)]), // tied farm output
        );
        let total_stored = flows[&Timber].stored + flows[&Straw].stored;
        assert_eq!(total_stored, 20);
    }
}

// How good is our bookkkeeping?
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Approximation {
    pub digits: u8,
    pub max: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    Exact,
    Approximate,
    Conservative,
}

// Simulate bookkeepping limitations; returns the rounded value and precision info
pub fn round(orig: u32, approx: Approximation) -> (u32, Precision) {
    let max = approx.max as u32;
    let capped = orig > max;
    let res = orig.min(max);

    let too_long = u32::pow(10, approx.digits.into());

    let mut res_digits = res;
    let mut res_zeroes = 1;
    while res_digits >= too_long {
        res_digits /= 10;
        res_zeroes *= 10;
    }

    let res = res_digits * res_zeroes;

    let precision = if res == orig {
        Precision::Exact
    } else if capped {
        Precision::Conservative
    } else {
        Precision::Approximate
    };

    (res, precision)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCT: Approximation = Approximation {
        digits: 1,
        max: 100,
    };

    #[test]
    fn round_small_values_unchanged() {
        assert_eq!(round(9, ACCT), (9, Precision::Exact));
        assert_eq!(round(20, ACCT), (20, Precision::Exact));
        assert_eq!(round(10, ACCT), (10, Precision::Exact));
    }

    #[test]
    fn round_multi_digit_drops_to_one_sig_digit() {
        // 137 -> capped at 100, which is already 1 significant digit.
        assert_eq!(round(137, ACCT), (100, Precision::Conservative));
    }

    #[test]
    fn round_caps_at_max() {
        // Above max: capped to 100 and flagged as conservative.
        assert_eq!(round(250, ACCT), (100, Precision::Conservative));
    }

    #[test]
    fn round_truncates_significant_digits() {
        // Under the cap but more than one significant digit: 47 -> 40.
        assert_eq!(round(47, ACCT), (40, Precision::Approximate));
    }

    /// Regression test: a single `Inventory` can back furniture (like the
    /// starting wagon) that provides both `Bulk` and `Rack` capacity. Each
    /// kind must have its own independent pool, so filling one all the way
    /// up never reduces the other's remaining capacity.
    #[test]
    fn bulk_and_rack_capacity_are_independent_pools() {
        let mut inv = Inventory::new([(StorageKind::Bulk, 8.0), (StorageKind::Rack, 2.0)]);
        inv.add_uniform(UniformResource::Plank, 8);
        assert_eq!(inv.remaining_capacity(StorageKind::Bulk), 0.0);
        assert_eq!(
            inv.remaining_capacity(StorageKind::Rack),
            2.0,
            "a full Bulk pool must not eat into Rack's own capacity"
        );

        inv.add_unique(UniqueResource::Tool(ToolKind::CarpentersTools));
        assert_eq!(
            inv.remaining_capacity(StorageKind::Rack),
            1.5,
            "a Tool occupies 0.5 volume within Rack's own pool"
        );
        assert_eq!(
            inv.remaining_capacity(StorageKind::Bulk),
            0.0,
            "storing a tool must not eat into Bulk's own capacity"
        );
    }
}
