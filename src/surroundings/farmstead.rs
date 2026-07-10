use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::city_effect::{CityEffect, LedgerSource, Pool};
use crate::resource::{ToolKind, UniformResource};

/// Farms within this map-unit radius are considered neighbours for the purpose
/// of updating a farm's wanted resource after a market visit.
const WANTED_UPDATE_RADIUS: f32 = 80.0;

const STOCKPILE_MAX: u32 = 40;

/// Market boundary in map units. A farm at this distance pays 8 potatoes travel cost.
pub const MARKET_RADIUS: f32 = 50.0;

/// How many of a farm's wanted resource the market pool must supply for the farm
/// to re-roll its resource.
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

/// Per-cycle market instruction: what an invited farm does at this month's market.
/// Stored on `FarmData::event`, `#[serde(skip)]` since it resets every advance
/// (see `reset_farm_events`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum FarmEvent {
    /// Participate normally: contribute stockpiles, receive wanted resource for a boost.
    #[default]
    Market,
    /// Spend `RECONFIGURE_COST` of the wanted resource from the pool to permanently
    /// switch the farm's produced resource to a random one (decided when time advances).
    RerollResource,
    /// Spend `RECONFIGURE_COST` to permanently switch to using the given tool for production
    Specialize(ToolKind),
    /// Take on a -10 declining production penalty in exchange for growing the
    /// city's population by one individual.
    Adopt,
}

impl FarmEvent {
    pub fn checkbox_label(self) -> &'static str {
        match self {
            FarmEvent::Market => "Invite",
            FarmEvent::RerollResource => "Change",
            FarmEvent::Specialize { .. } => "Specialize",
            FarmEvent::Adopt => "Adopt",
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct FarmData {
    pub seed: Vec2,
    pub polygon: Vec<Vec2>,
    pub area: f32,
    pub fertility: f32,
    /// What this farm produces every month (Regular resource or Specialized beam output).
    pub production: FarmProduction,
    pub wanted_resource: UniformResource,
    pub want_max: u32,
    pub potato_stockpile: u32,
    pub inedible_stockpile: u32,
    pub boost: i32,
    pub invited: bool,
    /// This month's market instruction, if invited. Resets to `Market` every
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

    /// Whether this farm can absorb the "Adopt" action's -10 production penalty
    /// without its capacity hitting zero.
    pub fn can_adopt(&self) -> bool {
        self.production_capacity() > 10
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
    /// Voronoi adjacency: `neighbors[i]` lists the farms sharing an edge with farm `i`.
    /// Rebuilt from polygons when empty (covers fresh generation and loaded saves).
    #[serde(skip)]
    pub neighbors: Vec<Vec<usize>>,
}

/// Two polygon vertices are "the same" corner if within this map-unit distance.
const ADJACENCY_EPS: f32 = 0.01;

/// Build Voronoi adjacency: two cells are neighbours when their polygons share an
/// edge, i.e. they have two vertices in common.
pub fn build_adjacency(farms: &[FarmData]) -> Vec<Vec<usize>> {
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
                neighbors[i].push(j);
                neighbors[j].push(i);
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

    fn neighbors_of(&self, i: usize) -> &[usize] {
        self.neighbors.get(i).map_or(&[], |v| v.as_slice())
    }

    pub fn farm_event(&self, i: usize) -> FarmEvent {
        self.farms[i].event
    }

    pub fn set_farm_event(&mut self, i: usize, event: FarmEvent) {
        self.farms[i].event = event;
    }
}

/// Transient resource: exists only while in Surroundings mode.
#[derive(Resource)]
pub struct SurroundingsState {
    pub viewport_offset: Vec2,
    /// Which farm's "…" configuration menu is open, if any.
    pub open_farm_menu: Option<usize>,
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
    /// `Market` event: the farm's wanted resource was filled by `granted` units,
    /// funded by `potatoes_spent` potatoes from the pool.
    Boost { granted: u32, potatoes_spent: u32 },
    /// `RerollResource` event: paid this much of the wanted resource to re-roll.
    Reroll { paid: u32 },
    /// `Specialize` event: paid this much of the wanted resource to switch to
    /// using the given tool.
    Specialize { paid: u32, tool: ToolKind },
    /// `Adopt` event: population grows by one, production takes a -10 declining penalty.
    Adopt,
}

/// Read-only snapshot of what applying this month's market effects would do
/// given the current state.
pub struct MarketOutcome {
    /// Per invited farm index: its `CityEffect::Market`.
    pub farm_effects: HashMap<usize, crate::city_effect::CityEffect>,
    /// Resources the player will gain (resource, quantity).
    pub player_gains: Vec<(UniformResource, u32)>,
}

fn invited_with_costs(farms: &[FarmData], circle_pos: Vec2) -> Vec<(usize, u32)> {
    farms
        .iter()
        .enumerate()
        .filter(|(_, f)| f.invited)
        .map(|(i, f)| {
            let dist = f.seed.distance(circle_pos);
            let cost = (dist * 8.0 / MARKET_RADIUS.max(1.0)).round() as u32;
            (i, cost)
        })
        .collect()
}

/// Seeds `pool` with every invited farm's stockpiles as this month's market
/// inflow (attributed to `LedgerSource::Market`), and returns each invited
/// farm's `(index, travel_cost)`. The travel cost's worth of potatoes is spent
/// getting to market, so only the remainder is contributed. Seeding happens
/// before any outside claim (Eat, a traveler's visit) so those can take from
/// the pool ahead of farms' own market participation in `resolve_market`.
pub fn seed_market(fr: &FarmsResource, pool: &mut Pool) -> Vec<(usize, u32)> {
    let invited = invited_with_costs(&fr.farms, fr.circle_pos);
    for &(i, cost) in &invited {
        let farm = &fr.farms[i];
        pool.contribute(
            LedgerSource::Market,
            UniformResource::Potato,
            farm.potato_stockpile.saturating_sub(cost),
        );
        pool.contribute(
            LedgerSource::Market,
            farm.produced_resource(),
            farm.inedible_stockpile,
        );
    }
    invited
}

/// Fill a `Market`-event farm's wanted resource from the pool's inflow,
/// spending an equal number of potatoes, and return the resulting `Boost`.
fn take_boost(farm: &FarmData, pool: &mut Pool) -> MarketModeEffect {
    let wanted = farm.wanted_resource;
    let want = pool.inflow_available(wanted).min(farm.want_max);
    let granted = pool.claim_inflow(LedgerSource::Market, wanted, want);
    let potatoes_spent = if granted > 0 {
        pool.claim_inflow(LedgerSource::Market, UniformResource::Potato, granted)
    } else {
        0
    };
    MarketModeEffect::Boost {
        granted,
        potatoes_spent,
    }
}

/// Resolves each invited farm's event (Boost/Reroll/Specialize/Adopt) against
/// `pool`'s inflow, claiming from it as it goes and recording the claims in the
/// pool's ledger. `invited` comes from `seed_market`, which must already have
/// contributed the farms' stockpiles (and any higher-priority claimant must
/// have taken its share) before this runs. Returns each farm's `CityEffect::Market`.
pub fn resolve_market(
    fr: &FarmsResource,
    invited: &[(usize, u32)],
    pool: &mut Pool,
) -> HashMap<usize, CityEffect> {
    let mut farm_effects = HashMap::new();
    for &(i, cost) in invited {
        let farm = &fr.farms[i];
        // Reroll/Specialize pay a fixed reconfigure cost from the pool if it's
        // there; otherwise they fall back to a normal boost.
        let reconfigure = |pool: &mut Pool, made: MarketModeEffect| -> Option<MarketModeEffect> {
            if pool.inflow_available(farm.wanted_resource) >= RECONFIGURE_COST {
                pool.claim_inflow(LedgerSource::Market, farm.wanted_resource, RECONFIGURE_COST);
                Some(made)
            } else {
                None
            }
        };
        let effect = match fr.farm_event(i) {
            FarmEvent::Market => take_boost(farm, pool),
            FarmEvent::RerollResource => reconfigure(
                pool,
                MarketModeEffect::Reroll {
                    paid: RECONFIGURE_COST,
                },
            )
            .unwrap_or_else(|| take_boost(farm, pool)),
            FarmEvent::Specialize(tool) => reconfigure(
                pool,
                MarketModeEffect::Specialize {
                    paid: RECONFIGURE_COST,
                    tool,
                },
            )
            .unwrap_or_else(|| take_boost(farm, pool)),
            FarmEvent::Adopt => MarketModeEffect::Adopt,
        };
        farm_effects.insert(
            i,
            CityEffect::Market {
                farm_idx: i,
                travel_cost: cost,
                potato_contributed: farm.potato_stockpile.saturating_sub(cost),
                wanted_resource: farm.wanted_resource,
                inedible_contributed: (farm.produced_resource(), farm.inedible_stockpile),
                effect,
            },
        );
    }
    farm_effects
}

/// Whatever inflow is left in the pool becomes the player's gains, sorted for a
/// deterministic order.
fn gains_from_pool(inflow: &HashMap<UniformResource, u32>) -> Vec<(UniformResource, u32)> {
    let mut gains: Vec<(UniformResource, u32)> = inflow
        .iter()
        .filter(|(_, &qty)| qty > 0)
        .map(|(&res, &qty)| (res, qty))
        .collect();
    gains.sort_by_key(|&(res, _)| res);
    gains
}

/// Compute what the next market run would do on its own — no feeding, traveler,
/// or storage in the mix — without mutating state or using RNG. Used by the
/// preview UI and the "…" breakdown. Real execution instead threads a shared
/// `Pool` through `seed_market` + `resolve_market` (see
/// `city_effect::compute_month_effects`) so Eat and a traveler's visit can claim
/// ahead of farms' own market participation.
pub fn compute_market(fr: &FarmsResource) -> MarketOutcome {
    let mut pool = Pool::new(HashMap::new());
    let invited = seed_market(fr, &mut pool);
    let farm_effects = resolve_market(fr, &invited, &mut pool);
    MarketOutcome {
        farm_effects,
        player_gains: gains_from_pool(&pool.inflow),
    }
}

/// Thin wrapper kept for call sites that only need a read-only preview.
pub fn preview_market(fr: &FarmsResource) -> MarketOutcome {
    compute_market(fr)
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
    /// Per farm: inedible units to add (the farm's tool output if specialized).
    pub inedible_add: Vec<u32>,
    /// Per specialized farm index: (output produced, input consumed from neighbours).
    pub specialize_info: HashMap<usize, (u32, u32)>,
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
    let mut specialize_info = HashMap::new();

    for s in 0..n {
        let Some(tool) = fr.farms[s].specialized_tool() else {
            continue;
        };
        let spec = tool.specialization();
        let mut suppliers: Vec<usize> = fr
            .neighbors_of(s)
            .iter()
            .copied()
            .filter(|&j| fr.farms[j].production == FarmProduction::Regular(spec.input))
            .collect();
        suppliers.shuffle(rng);

        let mut demand = cap[s];
        let mut consumed = 0;
        for j in suppliers {
            if demand == 0 {
                break;
            }
            let take = demand.min(inedible_add[j]);
            inedible_add[j] -= take;
            demand -= take;
            consumed += take;
        }
        // A specialized farm banks its tool's output instead of its own resource.
        inedible_add[s] = consumed;
        specialize_info.insert(s, (consumed, consumed));
    }

    ProductionPlan {
        potato_add,
        inedible_add,
        specialize_info,
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

/// Human-readable breakdown of what farm `idx` would do this month under the given
/// event, computed with the same `compute_market` / `compute_production` as the executor.
/// Optionally overrides the farm's production type for the preview (used by the Specialize
/// option to show what the farm would produce if it were Specialized).
pub fn farm_breakdown(
    fr: &mut FarmsResource,
    idx: usize,
    event: FarmEvent,
    temp_production: Option<FarmProduction>,
) -> Vec<String> {
    fr.ensure_adjacency();
    let saved_event = fr.farm_event(idx);
    let saved_prod = fr.farms[idx].production;
    fr.set_farm_event(idx, event);
    if let Some(prod) = temp_production {
        fr.farms[idx].production = prod;
    }
    let lines = describe_farm_effect(fr, idx);
    fr.set_farm_event(idx, saved_event);
    fr.farms[idx].production = saved_prod;
    lines
}

/// The market effect farm `idx` would produce under a hypothetical event, computed
/// with the shared `compute_market`. `None` if the farm is not currently invited.
pub fn market_effect(
    fr: &mut FarmsResource,
    idx: usize,
    event: FarmEvent,
) -> Option<MarketModeEffect> {
    fr.ensure_adjacency();
    let saved = fr.farm_event(idx);
    fr.set_farm_event(idx, event);
    let effect = compute_market(fr)
        .farm_effects
        .get(&idx)
        .and_then(|e| match e {
            CityEffect::Market { effect, .. } => Some(*effect),
            _ => None,
        });
    fr.set_farm_event(idx, saved);
    effect
}

fn describe_farm_effect(fr: &FarmsResource, idx: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let outcome = compute_market(fr);
    let farm = &fr.farms[idx];
    let wanted = farm.wanted_resource.label();

    if let Some(CityEffect::Market {
        travel_cost,
        potato_contributed,
        inedible_contributed,
        effect,
        ..
    }) = outcome.farm_effects.get(&idx)
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
            MarketModeEffect::Boost { granted, .. } => {
                if granted > 0 {
                    lines.push(format!(
                        "Receives {} {}: +{} production next month",
                        granted, wanted, granted
                    ));
                } else {
                    lines.push(format!("No {} in the marketplace", wanted));
                }
            }
            MarketModeEffect::Reroll { paid } => {
                lines.push(format!(
                    "Spends {paid} {wanted} to switch its resource to a different one"
                ));
                if let FarmProduction::Specialized(tool) = farm.production {
                    lines.push(format!("Returns the {} to storage", tool.label()));
                }
            }
            MarketModeEffect::Specialize { paid, tool } => {
                lines.push(format!(
                    "Spends {paid} {wanted} to switch to using a {} on nearby resources",
                    tool.label()
                ));
                if let FarmProduction::Specialized(old_tool) = farm.production {
                    lines.push(format!("Returns the {} to storage", old_tool.label()));
                }
            }
            MarketModeEffect::Adopt => {
                lines
                    .push("Adopts a family: population +1, production -10 (declining)".to_string());
            }
        }
    } else {
        lines.push("Invite the farm to preview market effects.".to_string());
    }

    if let Some(tool) = farm.specialized_tool() {
        let spec = tool.specialization();
        let plan = compute_production(fr, &mut rand::rng());
        let (output, consumed) = plan.specialize_info.get(&idx).copied().unwrap_or((0, 0));
        if output > 0 {
            lines.push(format!(
                "Produces {} {} from {} {} taken from adjacent farms (cap {})",
                output,
                spec.output.label(),
                consumed,
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

/// After each market visit every participating farm refreshes its wanted resource:
/// pick a random nearby farm and adopt what it produces, so wanted resources track
/// systematic shifts in local production.
pub fn update_wanted_resources(fr: &mut FarmsResource) {
    use rand::Rng as _;
    let mut rng = rand::rng();
    let snapshot: Vec<(Vec2, UniformResource)> = fr
        .farms
        .iter()
        .map(|f| (f.seed, f.produced_resource()))
        .collect();

    let invited_indices: Vec<usize> = fr
        .farms
        .iter()
        .enumerate()
        .filter(|(_, f)| f.invited)
        .map(|(i, _)| i)
        .collect();

    for i in invited_indices {
        let (farm_pos, own_resource) = snapshot[i];
        let candidates: Vec<UniformResource> = snapshot
            .iter()
            .enumerate()
            .filter(|(j, (pos, res))| {
                *j != i && *res != own_resource && pos.distance(farm_pos) <= WANTED_UPDATE_RADIUS
            })
            .map(|(_, (_, res))| *res)
            .collect();

        fr.farms[i].wanted_resource = if !candidates.is_empty() {
            candidates[rng.random_range(0..candidates.len())]
        } else {
            let others: Vec<UniformResource> = UniformResource::inedible_farmables()
                .iter()
                .copied()
                .filter(|&r| r != own_resource)
                .collect();
            others[rng.random_range(0..others.len())]
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_farm(
        production: FarmProduction,
        wanted: UniformResource,
        area: f32,
        inedible: u32,
    ) -> FarmData {
        FarmData {
            seed: Vec2::ZERO,
            polygon: Vec::new(),
            area,
            fertility: 1.0,
            production,
            wanted_resource: wanted,
            want_max: 5,
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
            neighbors,
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
            ..mk_farm(FarmProduction::Regular(Straw), Straw, 5.0, 0)
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
            ..mk_farm(FarmProduction::Regular(Straw), Straw, 5.0, 0)
        };
        let adj = build_adjacency(&[a, b, c]);
        assert_eq!(adj[0], vec![1]);
        assert_eq!(adj[1], vec![0]);
        assert!(adj[2].is_empty());
    }

    #[test]
    fn specialized_farm_is_supply_limited() {
        use UniformResource::{Straw, Timber};
        let s = mk_farm(
            FarmProduction::Specialized(ToolKind::Whipsaw),
            Timber,
            10.0, // capacity 10
            0,
        );
        let t = mk_farm(FarmProduction::Regular(Timber), Straw, 3.0, 0); // produces 3 timber
        let u = mk_farm(FarmProduction::Regular(Timber), Straw, 4.0, 0); // produces 4 timber
        let fr = farms_with(vec![s, t, u], vec![vec![1, 2], vec![0], vec![0]]);
        let plan = compute_production(&fr, &mut rand::rng());
        // Capacity 10 exceeds the 7 timber available, so beams == 7 and neighbours drained.
        assert_eq!(plan.inedible_add[0], 7);
        assert_eq!(plan.inedible_add[1], 0);
        assert_eq!(plan.inedible_add[2], 0);
        assert_eq!(plan.specialize_info[&0], (7, 7));
    }

    #[test]
    fn specialized_farm_is_capacity_limited() {
        use UniformResource::{Straw, Timber};
        let s = mk_farm(
            FarmProduction::Specialized(ToolKind::Whipsaw),
            Timber,
            10.0, // capacity 10
            0,
        );
        let t = mk_farm(FarmProduction::Regular(Timber), Straw, 8.0, 0);
        let u = mk_farm(FarmProduction::Regular(Timber), Straw, 9.0, 0);
        let fr = farms_with(vec![s, t, u], vec![vec![1, 2], vec![0], vec![0]]);
        let plan = compute_production(&fr, &mut rand::rng());
        // 17 timber available but capacity caps output at 10; 7 timber survives somewhere.
        assert_eq!(plan.inedible_add[0], 10);
        assert_eq!(plan.inedible_add[1] + plan.inedible_add[2], 7);
        assert_eq!(plan.specialize_info[&0], (10, 10));
    }

    #[test]
    fn reroll_pays_pool_or_falls_back_to_boost() {
        use UniformResource::{Straw, Timber};
        // Farm A re-rolls (wants Timber); farm B supplies Timber into the pool.
        let make = |timber_supply: u32| {
            let a = mk_farm(FarmProduction::Regular(Straw), Timber, 5.0, 0);
            let b = mk_farm(FarmProduction::Regular(Timber), Straw, 5.0, timber_supply);
            let mut fr = farms_with(vec![a, b], vec![vec![], vec![]]);
            fr.farms[0].event = FarmEvent::RerollResource;
            fr
        };

        // Pool has >= RECONFIGURE_COST timber: A pays and re-rolls.
        let outcome = compute_market(&make(RECONFIGURE_COST + 5));
        assert!(matches!(
            outcome.farm_effects[&0],
            crate::city_effect::CityEffect::Market {
                effect: MarketModeEffect::Reroll { paid },
                ..
            } if paid == RECONFIGURE_COST
        ));

        // Pool has too little timber: A falls back to a normal boost.
        let outcome = compute_market(&make(RECONFIGURE_COST - 1));
        assert!(matches!(
            outcome.farm_effects[&0],
            crate::city_effect::CityEffect::Market {
                effect: MarketModeEffect::Boost { .. },
                ..
            }
        ));
    }

    #[test]
    fn can_adopt_requires_capacity_above_ten() {
        use UniformResource::Straw;
        // area 10, no boost: capacity 10, exactly at the boundary -> not allowed.
        let at_boundary = mk_farm(FarmProduction::Regular(Straw), Straw, 10.0, 0);
        assert!(!at_boundary.can_adopt());

        // area 11: capacity 11 -> allowed.
        let above_boundary = mk_farm(FarmProduction::Regular(Straw), Straw, 11.0, 0);
        assert!(above_boundary.can_adopt());
    }

    #[test]
    fn adopt_grows_population_and_applies_declining_penalty() {
        use crate::city::{ConstructedCity, ProposedCity};
        use crate::city_effect::EffectContext;
        use crate::population::Population;
        use rand::SeedableRng;
        use UniformResource::Straw;

        let a = mk_farm(FarmProduction::Regular(Straw), Straw, 20.0, 0);
        let mut fr = farms_with(vec![a], vec![vec![]]);
        fr.farms[0].event = FarmEvent::Adopt;

        let effect = compute_market(&fr).farm_effects.remove(&0).unwrap();

        let mut constructed = ConstructedCity::new(Vec::new());
        let mut pending = ProposedCity::new();
        let mut population = Population::default();
        let mut rng = rand::rngs::StdRng::seed_from_u64(0);
        effect.apply(&mut EffectContext {
            constructed: &mut constructed,
            pending: &mut pending,
            population: &mut population,
            farms: &mut fr,
            rng: &mut rng,
        });

        // Population::default() starts with one individual; Adopt adds one more.
        assert_eq!(population.individuals.len(), 2);
        assert_eq!(fr.farms[0].boost, -10);
        assert_eq!(fr.farms[0].production_capacity(), 10);

        // The penalty decays by 1/month, same as a positive boost, moving toward 0.
        let plan = compute_production(&fr, &mut rand::rng());
        apply_production(&mut fr, &plan);
        assert_eq!(fr.farms[0].boost, -9);
    }
}
