use std::collections::{BTreeMap, HashMap};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::city_effect::{CityEffect, LedgerSource, MonthInputs, Pool};
use crate::resource::{ToolKind, UniformResource};
use crate::surroundings::attendance::{MarketWants, WANTED_BOOST};
use crate::surroundings::road_network::{RoadNetwork, INITIAL_TRIPS};

/// Stockpile ceiling: production beyond this is simply wasted, which is what
/// gives a full farm a reason to make the trip (see
/// [`crate::surroundings::attendance`]).
pub const STOCKPILE_MAX: u32 = 40;

/// Market boundary in map units. A farm at this distance pays 8 potatoes travel cost.
pub const MARKET_RADIUS: f32 = 50.0;

/// How much a farm's production is boosted by attending the market.
pub const MARKET_BOOST: i32 = 4;

/// The declining production penalty a farm takes on for the "Adopt" action.
/// Also the minimum production capacity required to adopt (`can_adopt`): the
/// penalty may bring capacity all the way to zero, so a farm at exactly this
/// much still qualifies.
pub const ADOPT_PENALTY: i32 = 8;

/// How many potatoes the market pool must supply for a farm to reconfigure its
/// production. The reconfiguring farm also burns its own potato stockpile
/// rather than contributing it to the pool (see `seed_market`).
pub const RECONFIGURE_COST: u32 = 10;

/// Persistent production type stored on each farm. Determines what the farm produces
/// every month, regardless of whether it is invited to the market.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FarmProduction {
    /// Produces the given inedible resource every month.
    Regular(UniformResource),
    /// Converts adjacent farms' input resource into this tool's output every month.
    /// The tool is considered permanently embedded in the farm while this is active.
    Specialized(ToolKind),
}

impl Default for FarmProduction {
    fn default() -> Self {
        FarmProduction::Regular(UniformResource::Straw)
    }
}

impl FarmProduction {
    pub fn produced_resource(self) -> UniformResource {
        match self {
            FarmProduction::Regular(r) => r,
            FarmProduction::Specialized(t) => t.specialization().output,
        }
    }

    pub fn specialized_tool(self) -> Option<ToolKind> {
        match self {
            FarmProduction::Specialized(t) => Some(t),
            FarmProduction::Regular(_) => None,
        }
    }
}

/// What a farm's production becomes as a result of a `FarmEvent::Reconfigure`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum NewProduction {
    /// Re-roll to a random inedible resource, decided when time advances.
    RandomRegular,
    /// Switch to producing via the given tool.
    Tool(ToolKind),
}

/// Per-cycle market instruction: what an invited farm does at this month's market.
/// Stored on `FarmData::event`, `#[serde(skip)]` since it resets every advance
/// (see `reset_farm_events`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum FarmEvent {
    /// Participate normally: contribute stockpiles, receive a `MARKET_BOOST` boost.
    #[default]
    Market,
    /// Spend `RECONFIGURE_COST` potatoes from the pool (plus the farm's own potato
    /// stockpile) to permanently switch the farm's production to `new_production`.
    Reconfigure(NewProduction),
    /// Take on a -8 declining production penalty in exchange for growing the
    /// city's population by one individual.
    Adopt,
}

impl FarmEvent {
    /// Short name for what the farm will do at the market it's attending.
    pub fn label(self) -> &'static str {
        match self {
            FarmEvent::Market => "Trade",
            FarmEvent::Reconfigure(NewProduction::RandomRegular) => "Change",
            FarmEvent::Reconfigure(NewProduction::Tool(_)) => "Specialize",
            FarmEvent::Adopt => "Adopt",
        }
    }
}

/// Identifies a farm by its position in `FarmsResource::farms`. Farms are
/// never added or removed after generation, so a plain index is stable —
/// this newtype exists purely so a farm index can't be silently mixed up
/// with some other `usize` (a place id, a vertex index, ...) at a function
/// boundary. Raw indices still cross I/O boundaries (headless commands,
/// `query farms` output) as plain integers; `FarmId` is the internal
/// game-logic vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FarmId(usize);

impl FarmId {
    pub fn new(index: usize) -> Self {
        FarmId(index)
    }

    pub fn index(self) -> usize {
        self.0
    }
}

impl std::fmt::Display for FarmId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Serialize, Deserialize)]
pub struct FarmData {
    pub seed: Vec2,
    pub polygon: Vec<Vec2>,
    pub area: f32,
    pub fertility: f32,
    /// What this farm produces every month (Regular resource or Specialized plank output).
    pub production: FarmProduction,
    pub potato_stockpile: u32,
    pub inedible_stockpile: u32,
    pub boost: i32,
    /// Whether this farm is at the coming month's market. Derived state, not
    /// a player choice: `attendance::apply_attendance` recomputes it from the
    /// farm's own circumstances (what it has to sell, how far it is, what the
    /// market is advertising for) against the city's market stand count.
    /// Everything downstream — `seed_market`, `record_road_trips`,
    /// `pave_fieldstone_routes` — only reads it.
    pub invited: bool,
    /// This month's market instruction, if attending. Resets to `Market` every
    /// advance (see `reset_farm_events`), so it isn't worth persisting.
    #[serde(skip)]
    pub event: FarmEvent,
}

impl FarmData {
    pub fn centroid(&self) -> Vec2 {
        self.seed
    }

    pub fn base_production(&self) -> u32 {
        self.area.round() as u32
    }

    pub fn production_capacity(&self) -> u32 {
        ((self.base_production() as i32) + self.boost).max(0) as u32
    }

    /// Whether this farm's total production is large enough to take on the
    /// "Adopt" action's declining penalty. The penalty may bring capacity all
    /// the way to zero, so a farm at exactly `ADOPT_PENALTY` still qualifies.
    pub fn can_adopt(&self) -> bool {
        self.production_capacity() >= ADOPT_PENALTY as u32
    }

    pub fn specialized_tool(&self) -> Option<ToolKind> {
        self.production.specialized_tool()
    }

    pub fn produced_resource(&self) -> UniformResource {
        self.production.produced_resource()
    }
}

/// Permanent game-state resource: generated once, persists across mode changes, saved/loaded.
#[derive(Resource, Serialize, Deserialize)]
pub struct FarmsResource {
    pub farms: Vec<FarmData>,
    pub circle_pos: Vec2,
    /// Paths revealed by traveler arrivals.
    #[serde(default)]
    pub traveler_reveals: Vec<Vec<Vec2>>,
    /// What the market is advertising that it will buy — the player's standing
    /// influence over which farms find the trip worth making. See
    /// [`crate::surroundings::attendance`].
    #[serde(default)]
    pub market_wants: MarketWants,
    /// Voronoi adjacency: `neighbors[i]` lists the farms sharing an edge with farm `i`.
    /// Rebuilt from polygons when empty (covers fresh generation and loaded saves).
    #[serde(skip)]
    pub neighbors: Vec<Vec<FarmId>>,
    /// Persisted per-edge road trip counts, indexed to match `roads.edges`.
    /// Drives road development (lower travel cost, a darker brown line).
    #[serde(default)]
    pub road_trips: Vec<u32>,
    /// Persisted per-edge paved fractions, indexed to match `roads.edges`.
    /// See `pave_fieldstone_routes`.
    #[serde(default)]
    pub road_paved: Vec<f32>,
    /// The road network over the farm polygons. Rebuilt from polygons +
    /// `road_trips`/`road_paved` when absent (fresh generation and loaded saves).
    #[serde(skip)]
    pub roads: Option<RoadNetwork>,
}

impl std::ops::Index<FarmId> for FarmsResource {
    type Output = FarmData;
    fn index(&self, id: FarmId) -> &FarmData {
        &self.farms[id.index()]
    }
}

impl std::ops::IndexMut<FarmId> for FarmsResource {
    fn index_mut(&mut self, id: FarmId) -> &mut FarmData {
        &mut self.farms[id.index()]
    }
}

/// Two polygon vertices are "the same" corner if within this map-unit distance.
const ADJACENCY_EPS: f32 = 0.01;

/// Build Voronoi adjacency: two cells are neighbours when their polygons share an
/// edge, i.e. they have two vertices in common.
pub fn build_adjacency(farms: &[FarmData]) -> Vec<Vec<FarmId>> {
    let n = farms.len();
    let mut neighbors = vec![Vec::new(); n];
    for i in 0..n {
        for j in (i + 1)..n {
            let shared = farms[i]
                .polygon
                .iter()
                .filter(|&&vi| {
                    farms[j]
                        .polygon
                        .iter()
                        .any(|&vj| vi.distance(vj) <= ADJACENCY_EPS)
                })
                .count();
            if shared >= 2 {
                neighbors[i].push(FarmId::new(j));
                neighbors[j].push(FarmId::new(i));
            }
        }
    }
    neighbors
}

impl FarmsResource {
    /// Ensure the adjacency cache is populated (no-op once built).
    pub fn ensure_adjacency(&mut self) {
        if self.neighbors.len() != self.farms.len() {
            self.neighbors = build_adjacency(&self.farms);
        }
    }

    /// Ensure the road network is built and its shortest-path tree is current.
    /// Builds topology from the farm polygons on first use (or after a load);
    /// seeds fresh/mismatched `road_trips`/`road_paved` (to `INITIAL_TRIPS`, the
    /// factor-4.0 dirt-road start, and to unpaved respectively); then recomputes
    /// the city-rooted distance tree so routing reads reflect the latest state.
    pub fn ensure_roads(&mut self) {
        if self.roads.is_none() {
            let polygons: Vec<Vec<Vec2>> = self.farms.iter().map(|f| f.polygon.clone()).collect();
            let mut roads = RoadNetwork::build(&polygons, self.circle_pos);
            if self.road_trips.len() != roads.edge_count() {
                self.road_trips = vec![INITIAL_TRIPS; roads.edge_count()];
            }
            if self.road_paved.len() != roads.edge_count() {
                self.road_paved = vec![0.0; roads.edge_count()];
            }
            roads.set_trips(&self.road_trips);
            roads.set_paved(&self.road_paved);
            self.roads = Some(roads);
        }
        if let Some(roads) = self.roads.as_mut() {
            roads.recompute_dist();
        }
    }

    /// Record one trip along every invited farm's cheapest delivery route, and
    /// (if it visited) the traveler's route from `traveler_start`. Roads develop
    /// where traffic actually flows. Persists the updated counts into
    /// `road_trips`; the next `ensure_roads` recomputes distances.
    pub fn record_road_trips(&mut self, traveler_start: Option<Vec2>) {
        let Some(roads) = self.roads.as_mut() else {
            return;
        };
        let mut edges: Vec<usize> = Vec::new();
        for farm in &self.farms {
            if !farm.invited {
                continue;
            }
            if let Some((_, corner)) = roads.farm_delivery(&farm.polygon) {
                edges.extend(roads.path_edges(corner));
            }
        }
        if let Some(start) = traveler_start {
            let node = roads.nearest_node(start);
            edges.extend(roads.path_edges(node));
        }
        for edge_idx in edges {
            roads.bump_trips(edge_idx);
        }
        self.road_trips = roads.trips();
        roads.recompute_dist();
    }

    /// Spends `fieldstone_budget` paving this month's Fieldstone delivery
    /// routes — every invited Fieldstone-producing farm's cheapest route to
    /// the city — closest-to-city leg first. No-op if the road network isn't
    /// built yet. Persists the updated paved fractions into `road_paved`.
    /// Returns whatever fieldstone went unused (e.g. every candidate route was
    /// already fully paved), which the caller should give back to the player.
    pub fn pave_fieldstone_routes(&mut self, fieldstone_budget: u32) -> u32 {
        if fieldstone_budget == 0 {
            return 0;
        }
        let farms = &self.farms;
        let Some(roads) = self.roads.as_mut() else {
            return fieldstone_budget;
        };
        let edges: Vec<usize> = farms
            .iter()
            .filter(|f| f.invited && f.produced_resource() == UniformResource::Fieldstone)
            .filter_map(|f| roads.farm_delivery(&f.polygon))
            .flat_map(|(_, corner)| roads.path_edges(corner))
            .collect();
        let leftover = roads.pave_edges(edges, fieldstone_budget as f32);
        roads.recompute_dist();
        self.road_paved = roads.paved();
        leftover.floor().max(0.0) as u32
    }

    fn neighbors_of(&self, id: FarmId) -> &[FarmId] {
        self.neighbors.get(id.index()).map_or(&[], |v| v.as_slice())
    }

    pub fn farm_event(&self, id: FarmId) -> FarmEvent {
        self[id].event
    }

    /// How many farms are coming to the next market.
    pub fn attending_count(&self) -> usize {
        self.farms.iter().filter(|f| f.invited).count()
    }

    pub fn set_farm_event(&mut self, id: FarmId, event: FarmEvent) {
        self[id].event = event;
    }
}

/// Transient resource: exists only while in Surroundings mode.
#[derive(Resource)]
pub struct SurroundingsState {
    pub viewport_offset: Vec2,
    /// Which farm's "…" configuration menu is open, if any.
    pub open_farm_menu: Option<FarmId>,
}

/// In-game calendar.
#[derive(Resource, Default, Serialize, Deserialize)]
pub struct GameClock {
    pub months: u32,
}

impl GameClock {
    pub fn month(&self) -> u32 {
        self.months
    }

    pub fn advance_month(&mut self) {
        self.months += 1;
    }
}

#[derive(Clone, Copy)]
pub enum MarketModeEffect {
    /// `Market` event: attending the market raised the farm's boost by
    /// `amount` — `MARKET_BOOST`, plus `attendance::WANTED_BOOST` if the farm
    /// sold into the market's advertised demand.
    Boost { amount: i32 },
    /// `Reconfigure` event: paid this many potatoes to switch production to
    /// `new_production`. `paid_from_storage` is how much of `paid` came out of
    /// the player's pre-existing stored potatoes (as opposed to this month's
    /// market inflow), which the executor must physically withdraw.
    Reconfigure {
        paid: u32,
        paid_from_storage: u32,
        new_production: NewProduction,
    },
    /// `Adopt` event: population grows by one, production takes an
    /// `ADOPT_PENALTY` declining penalty.
    Adopt,
}

/// Read-only snapshot of what applying this month's market effects would do
/// given the current state.
pub struct MarketOutcome {
    /// Per invited farm index: its `CityEffect::Market`.
    pub farm_effects: BTreeMap<FarmId, crate::city_effect::CityEffect>,
    /// Resources the player will gain (resource, quantity).
    pub player_gains: Vec<(UniformResource, u32)>,
}

/// What getting to market costs farm `id`, in potatoes eaten on the way. The
/// resource teleports free to the farm's cheapest non-city corner, then follows
/// the lowest-cost road route in; the route's `length * factor` weight converts
/// to potatoes with the same scale as the old straight-line formula (a
/// factor-1.0 road reproduces it). Falls back to the straight-line estimate if
/// the road graph isn't ready.
///
/// Shared with `attendance`, which weighs this against what a farm has to sell
/// when deciding whether the trip is worth making at all.
pub fn travel_cost(fr: &FarmsResource, id: FarmId) -> u32 {
    let farm = &fr[id];
    let weight = fr
        .roads
        .as_ref()
        .and_then(|roads| roads.farm_delivery(&farm.polygon))
        .map(|(w, _)| w)
        .unwrap_or_else(|| farm.seed.distance(fr.circle_pos));
    (weight * 8.0 / MARKET_RADIUS.max(1.0)).round() as u32
}

/// Each attending farm's `(id, travel_cost)`.
fn invited_with_costs(fr: &FarmsResource) -> Vec<(FarmId, u32)> {
    (0..fr.farms.len())
        .map(FarmId::new)
        .filter(|&id| fr[id].invited)
        .map(|id| (id, travel_cost(fr, id)))
        .collect()
}

/// How many potatoes farm `id` brings to market: its stockpile less the travel
/// cost of getting there. A reconfiguring farm brings none — its whole potato
/// stockpile is consumed by the reconfiguration itself, on top of the
/// `RECONFIGURE_COST` it claims back out of the pool.
fn potato_contribution(fr: &FarmsResource, id: FarmId, travel_cost: u32) -> u32 {
    if matches!(fr.farm_event(id), FarmEvent::Reconfigure(_)) {
        return 0;
    }
    fr[id].potato_stockpile.saturating_sub(travel_cost)
}

/// Seeds `pool` with every invited farm's stockpiles as this month's market
/// inflow (attributed to `LedgerSource::Market`), and returns each invited
/// farm's `(id, travel_cost)`. The travel cost's worth of potatoes is spent
/// getting to market, so only the remainder is contributed. Seeding happens
/// before any outside claim (Eat, a traveler's visit) so those can take from
/// the pool ahead of farms' own market participation in `resolve_market`.
pub fn seed_market(fr: &FarmsResource, pool: &mut Pool) -> Vec<(FarmId, u32)> {
    let invited = invited_with_costs(fr);
    for &(id, cost) in &invited {
        let farm = &fr[id];
        pool.contribute(
            LedgerSource::Market,
            UniformResource::Potato,
            potato_contribution(fr, id, cost),
        );
        pool.contribute(
            LedgerSource::Market,
            farm.produced_resource(),
            farm.inedible_stockpile,
        );
    }
    invited
}

/// The production boost farm `id` takes home from a market it actually traded
/// at: `MARKET_BOOST`, plus a premium if it sold into the demand the market
/// advertised (see `attendance::WANTED_BOOST`) — the advertisement isn't just
/// a lure, it's a better price.
fn market_boost(fr: &FarmsResource, id: FarmId) -> i32 {
    if fr.market_wants.is_wanted(fr[id].produced_resource()) {
        MARKET_BOOST + WANTED_BOOST
    } else {
        MARKET_BOOST
    }
}

/// Resolves each invited farm's event (Boost/Reroll/Specialize/Adopt) against
/// `pool`'s inflow, claiming from it as it goes and recording the claims in the
/// pool's ledger. `invited` comes from `seed_market`, which must already have
/// contributed the farms' stockpiles (and any higher-priority claimant must
/// have taken its share) before this runs. Returns each farm's `CityEffect::Market`.
pub fn resolve_market(
    fr: &FarmsResource,
    invited: &[(FarmId, u32)],
    pool: &mut Pool,
) -> BTreeMap<FarmId, CityEffect> {
    let mut farm_effects = BTreeMap::new();
    for &(id, cost) in invited {
        let farm = &fr[id];
        let effect = match fr.farm_event(id) {
            FarmEvent::Market => MarketModeEffect::Boost {
                amount: market_boost(fr, id),
            },
            // Pay a fixed reconfigure cost from the pool if it's there;
            // otherwise fall back to a normal boost.
            FarmEvent::Reconfigure(new_production) => {
                if pool.available(UniformResource::Potato) >= RECONFIGURE_COST {
                    // Farms' delivered potatoes (inflow) pay first; the player's
                    // own stored potatoes top up any shortfall.
                    let (_, from_storage) = pool.claim(
                        LedgerSource::Market,
                        UniformResource::Potato,
                        RECONFIGURE_COST,
                    );
                    MarketModeEffect::Reconfigure {
                        paid: RECONFIGURE_COST,
                        paid_from_storage: from_storage,
                        new_production,
                    }
                } else {
                    MarketModeEffect::Boost {
                        amount: market_boost(fr, id),
                    }
                }
            }
            FarmEvent::Adopt => MarketModeEffect::Adopt,
        };
        farm_effects.insert(
            id,
            CityEffect::Market {
                farm_idx: id,
                travel_cost: cost,
                potato_contributed: potato_contribution(fr, id, cost),
                inedible_contributed: (farm.produced_resource(), farm.inedible_stockpile),
                effect,
            },
        );
    }
    farm_effects
}

/// Compute what the next market run would do on its own — no feeding or traveler
/// in the mix — without mutating state or using RNG. `storage` is a snapshot of
/// the player's pre-existing stock, so a reconfigure's potato cost can draw on it
/// (as it does in real execution) when the market inflow is short. Used by the
/// preview UI and the "…" breakdown. Real execution instead threads a shared
/// `Pool` through `seed_market` + `resolve_market` (see
/// `city_effect::compute_month_effects`) so Eat and a traveler's visit can claim
/// ahead of farms' own market participation.
pub fn compute_market(
    fr: &FarmsResource,
    storage: &HashMap<UniformResource, u32>,
) -> MarketOutcome {
    let mut pool = Pool::new(storage.clone());
    let invited = seed_market(fr, &mut pool);
    let farm_effects = resolve_market(fr, &invited, &mut pool);
    MarketOutcome {
        farm_effects,
        player_gains: pool.gains(),
    }
}

/// Sums `production_capacity()` per produced resource across farms the
/// player currently knows about (fog alpha below `REVEAL_THRESHOLD` at the
/// farm's centroid). Used as a discard-priority tie-break in
/// `resource::distribute_incoming_resources`.
pub fn known_farm_plentifulness(fr: &FarmsResource) -> HashMap<UniformResource, u32> {
    use super::map::{fog_alpha_at, REVEAL_THRESHOLD};

    let mut totals = HashMap::new();
    for farm in &fr.farms {
        if fog_alpha_at(farm.centroid(), &fr.traveler_reveals) < REVEAL_THRESHOLD {
            *totals.entry(farm.produced_resource()).or_insert(0) += farm.production_capacity();
        }
    }
    totals
}

/// Reset per-cycle farm events for the next month (invited or not).
pub fn reset_farm_events(fr: &mut FarmsResource) {
    for farm in fr.farms.iter_mut() {
        farm.event = FarmEvent::Market;
    }
}

/// A month's production, computed by the shared core and applied by `apply_production`.
pub struct ProductionPlan {
    /// Per farm: potatoes to add to its stockpile.
    pub potato_add: Vec<u32>,
    /// Per farm: inedible units to add. For a specialized farm this is its
    /// tool's output, which (until a tool has a non-1:1 conversion ratio)
    /// equals how much input it consumed from its neighbours.
    pub inedible_add: Vec<u32>,
}

/// Compute this month's production for all farms. Specialized farms convert their
/// tool's input, drawn from adjacent Regular farms' production this month, into the
/// tool's output. When neighbours produce a surplus, suppliers are chosen at random
/// (the only use of `rng`; the per-farm totals are deterministic).
pub fn compute_production(fr: &FarmsResource, rng: &mut impl rand::Rng) -> ProductionPlan {
    use rand::seq::SliceRandom as _;

    let n = fr.farms.len();
    let cap: Vec<u32> = fr.farms.iter().map(|f| f.production_capacity()).collect();
    let potato_add = cap.clone();
    let mut inedible_add = cap.clone();

    for s in (0..n).map(FarmId::new) {
        let Some(tool) = fr[s].specialized_tool() else {
            continue;
        };
        let spec = tool.specialization();
        let mut suppliers: Vec<FarmId> = fr
            .neighbors_of(s)
            .iter()
            .copied()
            .filter(|&j| fr[j].production == FarmProduction::Regular(spec.input))
            .collect();
        suppliers.shuffle(rng);

        let mut demand = cap[s.index()];
        let mut consumed = 0;
        for j in suppliers {
            if demand == 0 {
                break;
            }
            let take = demand.min(inedible_add[j.index()]);
            inedible_add[j.index()] -= take;
            demand -= take;
            consumed += take;
        }
        // A specialized farm banks its tool's output instead of its own resource.
        inedible_add[s.index()] = consumed;
    }

    ProductionPlan {
        potato_add,
        inedible_add,
    }
}

/// Apply a `ProductionPlan`: bank production into stockpiles and decay boosts.
pub fn apply_production(fr: &mut FarmsResource, plan: &ProductionPlan) {
    for (i, farm) in fr.farms.iter_mut().enumerate() {
        farm.potato_stockpile = (farm.potato_stockpile + plan.potato_add[i]).min(STOCKPILE_MAX);
        farm.inedible_stockpile =
            (farm.inedible_stockpile + plan.inedible_add[i]).min(STOCKPILE_MAX);
        farm.boost -= farm.boost.signum();
    }
}

/// Human-readable breakdown of what farm `idx` would do this month under the
/// given event, computed with the same full month pipeline (`compute_month_effects`
/// via `inputs`) and `compute_production` as the executor — so feeding and a
/// traveler's claim ahead of the market are reflected here too. Optionally
/// overrides the farm's production type for the preview (used by the Specialize
/// option to show what the farm would produce if it were Specialized).
pub fn farm_breakdown(
    fr: &mut FarmsResource,
    idx: FarmId,
    event: FarmEvent,
    temp_production: Option<FarmProduction>,
    inputs: MonthInputs,
) -> Vec<String> {
    fr.ensure_adjacency();
    fr.ensure_roads();
    let saved_event = fr.farm_event(idx);
    let saved_prod = fr[idx].production;
    fr.set_farm_event(idx, event);
    if let Some(prod) = temp_production {
        fr[idx].production = prod;
    }
    let lines = describe_farm_effect(fr, idx, inputs);
    fr.set_farm_event(idx, saved_event);
    fr[idx].production = saved_prod;
    lines
}

/// The market effect farm `idx` would produce under a hypothetical event,
/// computed with the full month pipeline (so a reconfigure's affordability
/// reflects feeding/traveler contention). `None` if the farm is not currently
/// invited.
pub fn market_effect(
    fr: &mut FarmsResource,
    idx: FarmId,
    event: FarmEvent,
    inputs: MonthInputs,
) -> Option<MarketModeEffect> {
    fr.ensure_adjacency();
    let saved = fr.farm_event(idx);
    fr.set_farm_event(idx, event);
    fr.ensure_roads();
    let effects = inputs.compute(fr);
    let effect = effects.market_effects().get(&idx).and_then(|e| match e {
        CityEffect::Market { effect, .. } => Some(*effect),
        _ => None,
    });
    fr.set_farm_event(idx, saved);
    effect
}

fn describe_farm_effect(fr: &FarmsResource, idx: FarmId, inputs: MonthInputs) -> Vec<String> {
    let mut lines = Vec::new();
    let outcome = inputs.compute(fr);
    let farm = &fr[idx];

    if let Some(CityEffect::Market {
        travel_cost,
        potato_contributed,
        inedible_contributed,
        effect,
        ..
    }) = outcome.market_effects().get(&idx).copied()
    {
        let (res, qty) = *inedible_contributed;
        lines.push(format!(
            "After {} travel cost, contributes {} potatoes and {} {} to the pool",
            travel_cost,
            potato_contributed,
            qty,
            res.label()
        ));
        match *effect {
            MarketModeEffect::Boost { amount } => {
                lines.push(format!("Trades at the market: +{amount} production"));
            }
            MarketModeEffect::Reconfigure {
                paid,
                new_production,
                ..
            } => {
                match new_production {
                    NewProduction::RandomRegular => lines.push(format!(
                        "Spends {paid} potatoes to switch its resource to a different one"
                    )),
                    NewProduction::Tool(tool) => lines.push(format!(
                        "Spends {paid} potatoes to switch to using a {} on nearby resources",
                        tool.label()
                    )),
                }
                if let FarmProduction::Specialized(old_tool) = farm.production {
                    lines.push(format!("Returns the {} to storage", old_tool.label()));
                }
            }
            MarketModeEffect::Adopt => {
                lines.push(format!(
                    "Adopts a family: population +1, production -{ADOPT_PENALTY} (declining)"
                ));
            }
        }
    } else {
        lines.push("Invite the farm to preview market effects.".to_string());
    }

    if let Some(tool) = farm.specialized_tool() {
        let spec = tool.specialization();
        let plan = compute_production(fr, &mut rand::rng());
        // Output produced always equals input consumed (until a tool has a
        // non-1:1 conversion ratio) -- see `ProductionPlan::inedible_add`.
        let output = plan.inedible_add[idx.index()];
        if output > 0 {
            lines.push(format!(
                "Produces {} {} from {} {} taken from adjacent farms (cap {})",
                output,
                spec.output.label(),
                output,
                spec.input.label(),
                farm.production_capacity()
            ));
        } else {
            lines.push(format!(
                "No adjacent {} production available: 0 {}",
                spec.input.label(),
                spec.output.label()
            ));
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_farm(production: FarmProduction, area: f32, inedible: u32) -> FarmData {
        FarmData {
            seed: Vec2::ZERO,
            polygon: Vec::new(),
            area,
            fertility: 1.0,
            production,
            potato_stockpile: 0,
            inedible_stockpile: inedible,
            boost: 0,
            invited: true,
            event: FarmEvent::Market,
        }
    }

    fn farms_with(farms: Vec<FarmData>, neighbors: Vec<Vec<usize>>) -> FarmsResource {
        FarmsResource {
            farms,
            circle_pos: Vec2::ZERO,
            traveler_reveals: Vec::new(),
            market_wants: Default::default(),
            neighbors: neighbors
                .into_iter()
                .map(|v| v.into_iter().map(FarmId::new).collect())
                .collect(),
            road_trips: Vec::new(),
            road_paved: Vec::new(),
            roads: None,
        }
    }

    #[test]
    fn adjacency_detects_shared_edge() {
        use UniformResource::Straw;
        let square = |x: f32| FarmData {
            polygon: vec![
                Vec2::new(x, 0.0),
                Vec2::new(x + 1.0, 0.0),
                Vec2::new(x + 1.0, 1.0),
                Vec2::new(x, 1.0),
            ],
            ..mk_farm(FarmProduction::Regular(Straw), 5.0, 0)
        };
        // a and b share the edge x=1; c is far away and shares nothing.
        let a = square(0.0);
        let b = square(1.0);
        let c = FarmData {
            polygon: vec![
                Vec2::new(50.0, 50.0),
                Vec2::new(51.0, 50.0),
                Vec2::new(51.0, 51.0),
            ],
            ..mk_farm(FarmProduction::Regular(Straw), 5.0, 0)
        };
        let adj = build_adjacency(&[a, b, c]);
        assert_eq!(adj[0], vec![FarmId::new(1)]);
        assert_eq!(adj[1], vec![FarmId::new(0)]);
        assert!(adj[2].is_empty());
    }

    #[test]
    fn pave_fieldstone_routes_returns_unused_budget_once_route_is_paved() {
        use UniformResource::{Fieldstone, Straw};
        let square = |x: f32| FarmData {
            polygon: vec![
                Vec2::new(x, 0.0),
                Vec2::new(x + 1.0, 0.0),
                Vec2::new(x + 1.0, 1.0),
                Vec2::new(x, 1.0),
            ],
            ..mk_farm(FarmProduction::Regular(Straw), 5.0, 0)
        };
        let city_square = square(0.0);
        let mut farm_square = square(1.0);
        farm_square.production = FarmProduction::Regular(Fieldstone);

        let mut fr = farms_with(vec![city_square, farm_square], vec![vec![1], vec![0]]);
        fr.circle_pos = Vec2::new(1.0, 0.0); // the two squares' shared corner
        fr.ensure_roads();

        // The route is a single length-1 edge among the network's 7; at 0.1
        // length-per-fieldstone (a 10x paving cost), fully paving it costs 10.
        let paved_sum = |fr: &FarmsResource| -> f32 { fr.road_paved.iter().sum() };
        let nonzero_edges = |fr: &FarmsResource| fr.road_paved.iter().filter(|&&p| p > 0.0).count();

        let leftover = fr.pave_fieldstone_routes(3);
        assert_eq!(leftover, 0, "a small budget should be fully spent");
        assert_eq!(
            nonzero_edges(&fr),
            1,
            "only the route's one edge is touched"
        );
        assert!((paved_sum(&fr) - 0.3).abs() < 1e-6);

        let leftover = fr.pave_fieldstone_routes(100);
        assert!(
            (paved_sum(&fr) - 1.0).abs() < 1e-6,
            "the edge is now fully paved"
        );
        assert_eq!(
            leftover, 93,
            "once fully paved, the rest of the budget (100 - 7 more to finish) goes unused"
        );
    }

    #[test]
    fn specialized_farm_is_supply_limited() {
        use UniformResource::Timber;
        let s = mk_farm(
            FarmProduction::Specialized(ToolKind::CarpentersTools),
            10.0,
            0,
        ); // capacity 10
        let t = mk_farm(FarmProduction::Regular(Timber), 3.0, 0); // produces 3 timber
        let u = mk_farm(FarmProduction::Regular(Timber), 4.0, 0); // produces 4 timber
        let fr = farms_with(vec![s, t, u], vec![vec![1, 2], vec![0], vec![0]]);
        let plan = compute_production(&fr, &mut rand::rng());
        // Capacity 10 exceeds the 7 timber available, so planks == 7 and neighbours drained.
        assert_eq!(plan.inedible_add[0], 7);
        assert_eq!(plan.inedible_add[1], 0);
        assert_eq!(plan.inedible_add[2], 0);
    }

    #[test]
    fn specialized_farm_is_capacity_limited() {
        use UniformResource::Timber;
        let s = mk_farm(
            FarmProduction::Specialized(ToolKind::CarpentersTools),
            10.0,
            0,
        ); // capacity 10
        let t = mk_farm(FarmProduction::Regular(Timber), 8.0, 0);
        let u = mk_farm(FarmProduction::Regular(Timber), 9.0, 0);
        let fr = farms_with(vec![s, t, u], vec![vec![1, 2], vec![0], vec![0]]);
        let plan = compute_production(&fr, &mut rand::rng());
        // 17 timber available but capacity caps output at 10; 7 timber survives somewhere.
        assert_eq!(plan.inedible_add[0], 10);
        assert_eq!(plan.inedible_add[1] + plan.inedible_add[2], 7);
    }

    #[test]
    fn reroll_pays_pool_or_falls_back_to_boost() {
        use UniformResource::{Potato, Straw};
        // Farm A re-rolls; farm B supplies the potatoes that pay for it. A's own
        // potatoes are burned by the reconfiguration, so they never reach the pool.
        let make = |potato_supply: u32| {
            let mut a = mk_farm(FarmProduction::Regular(Straw), 5.0, 0);
            a.potato_stockpile = 7;
            let mut b = mk_farm(FarmProduction::Regular(Straw), 5.0, 0);
            b.potato_stockpile = potato_supply;
            let mut fr = farms_with(vec![a, b], vec![vec![], vec![]]);
            fr.farms[0].event = FarmEvent::Reconfigure(NewProduction::RandomRegular);
            fr
        };

        // Pool has >= RECONFIGURE_COST potatoes: A pays and re-rolls.
        let outcome = compute_market(&make(RECONFIGURE_COST + 5), &HashMap::new());
        assert!(matches!(
            outcome.farm_effects[&FarmId::new(0)],
            crate::city_effect::CityEffect::Market {
                effect: MarketModeEffect::Reconfigure {
                    paid,
                    new_production: NewProduction::RandomRegular,
                    ..
                },
                ..
            } if paid == RECONFIGURE_COST
        ));

        // A contributed none of its own 7 potatoes; only B's supply is left over,
        // less the RECONFIGURE_COST that A claimed back out of the pool.
        assert_eq!(outcome.player_gains, vec![(Potato, 5)]);

        // Pool has too little potato: A falls back to a normal boost, and its own
        // potatoes are still withheld (the event, not the outcome, decides that).
        let outcome = compute_market(&make(RECONFIGURE_COST - 1), &HashMap::new());
        assert!(matches!(
            outcome.farm_effects[&FarmId::new(0)],
            crate::city_effect::CityEffect::Market {
                effect: MarketModeEffect::Boost { .. },
                ..
            }
        ));
    }

    #[test]
    fn can_adopt_requires_capacity_at_least_eight() {
        use UniformResource::Straw;
        // area 8, no boost: capacity 8, exactly at the threshold -> allowed
        // (the -8 penalty may take it to zero).
        let at_threshold = mk_farm(FarmProduction::Regular(Straw), 8.0, 0);
        assert!(at_threshold.can_adopt());

        // area 7: capacity 7 -> not allowed.
        let below_threshold = mk_farm(FarmProduction::Regular(Straw), 7.0, 0);
        assert!(!below_threshold.can_adopt());
    }

    #[test]
    fn adopt_grows_population_and_applies_declining_penalty() {
        use crate::city::{ConstructedCity, ProposedCity};
        use crate::city_effect::EffectContext;
        use crate::population::Population;
        use rand::SeedableRng;
        use UniformResource::Straw;

        let a = mk_farm(FarmProduction::Regular(Straw), 20.0, 0);
        let mut fr = farms_with(vec![a], vec![vec![]]);
        fr.farms[0].event = FarmEvent::Adopt;

        let effect = compute_market(&fr, &HashMap::new())
            .farm_effects
            .remove(&FarmId::new(0))
            .unwrap();

        let mut constructed = ConstructedCity::new(Vec::new());
        let mut pending = ProposedCity::new();
        let mut population = Population::default();
        let mut rng = rand::rngs::StdRng::seed_from_u64(0);
        effect.apply(&mut EffectContext {
            constructed: &mut constructed,
            pending: &mut pending,
            population: &mut population,
            farms: &mut fr,
            idea_state: &mut crate::idea::IdeaState::default(),
            rng: &mut rng,
        });

        // Population::default() starts with one individual; Adopt adds one more.
        assert_eq!(population.individuals.len(), 2);
        assert_eq!(fr.farms[0].boost, -8);
        assert_eq!(fr.farms[0].production_capacity(), 12); // area 20 - 8

        // The penalty decays by 1/month, same as a positive boost, moving toward 0.
        let plan = compute_production(&fr, &mut rand::rng());
        apply_production(&mut fr, &plan);
        assert_eq!(fr.farms[0].boost, -7);
    }
}
