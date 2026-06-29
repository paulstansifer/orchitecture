use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

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
/// Not stored on `FarmData`; lives in `FarmsResource::farm_events` and resets each advance.
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
}

impl FarmEvent {
    pub fn checkbox_label(self) -> &'static str {
        match self {
            FarmEvent::Market => "Invite",
            FarmEvent::RerollResource => "Change",
            FarmEvent::Specialize{..} => "Specialize",
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
    pub boost: u32,
    pub invited: bool,
}

impl FarmData {
    pub fn centroid(&self) -> Vec2 {
        self.seed
    }

    pub fn base_production(&self) -> u32 {
        self.area.round() as u32
    }

    pub fn production_capacity(&self) -> u32 {
        self.base_production() + self.boost
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
    /// Per-cycle market event, parallel to `farms`. Defaults to Market; reset by `run_market`.
    #[serde(skip)]
    pub farm_events: Vec<FarmEvent>,
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
    /// Ensure the adjacency cache and `farm_events` vec are populated (no-op once built).
    pub fn ensure_adjacency(&mut self) {
        if self.neighbors.len() != self.farms.len() {
            self.neighbors = build_adjacency(&self.farms);
        }
        if self.farm_events.len() != self.farms.len() {
            self.farm_events.resize(self.farms.len(), FarmEvent::Market);
        }
    }

    fn neighbors_of(&self, i: usize) -> &[usize] {
        self.neighbors.get(i).map_or(&[], |v| v.as_slice())
    }

    pub fn farm_event(&self, i: usize) -> FarmEvent {
        self.farm_events.get(i).copied().unwrap_or(FarmEvent::Market)
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

/// The market effect a single invited farm produces, given its event. Computed by
/// the shared `compute_market` and consumed by both the preview UI and the executor.
#[derive(Clone, Copy)]
pub struct FarmMarketEffect {
    pub travel_cost: u32,
    pub potato_contributed: u32,
    pub inedible_contributed: (UniformResource, u32),
    pub effect: MarketModeEffect,
}

#[derive(Clone, Copy)]
pub enum MarketModeEffect {
    /// `Market` event: the farm's wanted resource was filled, granting this boost.
    Boost(u32),
    /// `RerollResource` event: paid this much of the wanted resource to re-roll.
    Reroll { paid: u32 },
    /// `Specialize` event: switch production to use the given tool
    Specialize { paid: u32, tool: ToolKind },
}

/// Read-only snapshot of what `run_market` would do given the current state.
pub struct MarketOutcome {
    /// Per invited farm index: its market effect.
    pub farm_effects: HashMap<usize, FarmMarketEffect>,
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

fn pool_totals(
    farms: &[FarmData],
    invited: &[(usize, u32)],
) -> (u32, HashMap<UniformResource, u32>) {
    let mut potato_pool: u32 = 0;
    let mut inedible_pool: HashMap<UniformResource, u32> = HashMap::new();
    for &(i, cost) in invited {
        potato_pool += farms[i].potato_stockpile.saturating_sub(cost);
        *inedible_pool
            .entry(farms[i].produced_resource())
            .or_insert(0) += farms[i].inedible_stockpile;
    }
    (potato_pool, inedible_pool)
}

/// Fill a `Market`-event farm's wanted resource from the pool, consuming an equal
/// number of potatoes, and return the boost granted.
fn take_boost(
    farm: &FarmData,
    potato_pool: &mut u32,
    inedible_pool: &mut HashMap<UniformResource, u32>,
) -> u32 {
    let wanted = farm.wanted_resource;
    let available = *inedible_pool.get(&wanted).unwrap_or(&0);
    let t = available.min(farm.want_max);
    if t > 0 {
        *inedible_pool.get_mut(&wanted).unwrap() -= t;
        let take_potatoes = (*potato_pool).min(t);
        *potato_pool -= take_potatoes;
    }
    t
}

fn gains_from_pool(
    potato_pool: u32,
    inedible_pool: HashMap<UniformResource, u32>,
) -> Vec<(UniformResource, u32)> {
    let mut gains = Vec::new();
    if potato_pool > 0 {
        gains.push((UniformResource::Potato, potato_pool));
    }
    for (res, qty) in inedible_pool {
        if qty > 0 {
            gains.push((res, qty));
        }
    }
    gains
}

/// Compute what the next market run would do, without mutating state or using RNG.
/// Shared by the preview UI, the "…" breakdown, and the executor `run_market`.
pub fn compute_market(fr: &FarmsResource) -> MarketOutcome {
    let invited = invited_with_costs(&fr.farms, fr.circle_pos);
    // TODO: copy-and-mutate inside a simulation
    let (mut potato_pool, mut inedible_pool) = pool_totals(&fr.farms, &invited);

    let mut farm_effects = HashMap::new();
    for &(i, cost) in &invited {
        let farm = &fr.farms[i];
        let event = fr.farm_event(i);
        let effect = match event {
            FarmEvent::Market => {
                MarketModeEffect::Boost(take_boost(farm, &mut potato_pool, &mut inedible_pool))
            }
            FarmEvent::RerollResource => {
                let wanted = farm.wanted_resource;
                if *inedible_pool.get(&wanted).unwrap_or(&0) >= RECONFIGURE_COST {
                    *inedible_pool.get_mut(&wanted).unwrap() -= RECONFIGURE_COST;
                    MarketModeEffect::Reroll {
                        paid: RECONFIGURE_COST,
                    }
                } else {
                    // Can't afford the re-roll: fall back to a normal boost.
                    MarketModeEffect::Boost(take_boost(farm, &mut potato_pool, &mut inedible_pool))
                }
            }
            FarmEvent::Specialize(tool) => {
                MarketModeEffect::Specialize { paid: RECONFIGURE_COST, tool }
            }
        };
        farm_effects.insert(
            i,
            FarmMarketEffect {
                travel_cost: cost,
                potato_contributed: farm.potato_stockpile.saturating_sub(cost),
                inedible_contributed: (farm.produced_resource(), farm.inedible_stockpile),
                effect,
            },
        );
    }

    let player_gains = gains_from_pool(potato_pool, inedible_pool);
    MarketOutcome {
        farm_effects,
        player_gains,
    }
}

/// Thin wrapper kept for call sites that only need a read-only preview.
pub fn preview_market(fr: &FarmsResource) -> MarketOutcome {
    compute_market(fr)
}

/// Run the monthly market for invited farms, applying the effects computed by the
/// shared `compute_market`. Returns the resources the player gains and the tools that
/// must be returned to storage (Whipsaws freed when a Specialized farm re-rolls).
pub fn run_market(
    fr: &mut FarmsResource,
    rng: &mut impl rand::Rng,
) -> (Vec<(UniformResource, u32)>, Vec<ToolKind>) {
    let outcome = compute_market(fr);
    let effects: Vec<(usize, MarketModeEffect)> = outcome
        .farm_effects
        .iter()
        .map(|(&i, e)| (i, e.effect))
        .collect();

    // Invited farms sold their stockpiles into the pool.
    for &(i, _) in &effects {
        fr.farms[i].potato_stockpile = 0;
        fr.farms[i].inedible_stockpile = 0;
    }

    let mut tools_to_return: Vec<ToolKind> = Vec::new();
    for (i, effect) in effects {
        match effect {
            MarketModeEffect::Boost(t) => fr.farms[i].boost = t,
            MarketModeEffect::Reroll { paid } => {
                fr.farms[i].boost = 0;
                if paid > 0 {
                    // If this farm was Specialized, its tool is freed when it reverts.
                    if let FarmProduction::Specialized(prev_tool) = fr.farms[i].production {
                        tools_to_return.push(prev_tool);
                    }
                    let current_res = fr.farms[i].produced_resource();
                    let options: Vec<UniformResource> = UniformResource::inedible_farmables()
                        .iter()
                        .copied()
                        .filter(|&r| r != current_res)
                        .collect();
                    fr.farms[i].production =
                        FarmProduction::Regular(options[rng.random_range(0..options.len())]);
                }
            }
            MarketModeEffect::Specialize { paid, tool } => {
                fr.farms[i].boost = 0;
                if paid > 0 {
                    // If this farm was Specialized, its tool is freed when it reverts.
                    if let FarmProduction::Specialized(prev_tool) = fr.farms[i].production {
                        tools_to_return.push(prev_tool);
                    }

                    let current_res = fr.farms[i].produced_resource();
                    fr.farms[i].production =
                        FarmProduction::Specialized(tool);
                }

            }
        }
    }

    // Reset per-cycle events for the next month.
    for event in fr.farm_events.iter_mut() {
        *event = FarmEvent::Market;
    }

    (outcome.player_gains, tools_to_return)
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
        farm.boost = farm.boost.saturating_sub(1);
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
    if fr.farm_events.len() > idx {
        fr.farm_events[idx] = event;
    }
    if let Some(prod) = temp_production {
        fr.farms[idx].production = prod;
    }
    let lines = describe_farm_effect(fr, idx);
    if fr.farm_events.len() > idx {
        fr.farm_events[idx] = saved_event;
    }
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
    let saved = fr.farm_event(idx);
    if fr.farm_events.len() > idx {
        fr.farm_events[idx] = event;
    }
    let effect = compute_market(fr).farm_effects.get(&idx).map(|e| e.effect);
    if fr.farm_events.len() > idx {
        fr.farm_events[idx] = saved;
    }
    effect
}

fn describe_farm_effect(fr: &FarmsResource, idx: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let outcome = compute_market(fr);
    let farm = &fr.farms[idx];
    let wanted = farm.wanted_resource.label();

    if let Some(eff) = outcome.farm_effects.get(&idx) {
        let (res, qty) = eff.inedible_contributed;
        lines.push(format!(
            "After {} travel cost, contributes {} potatoes and {} {} to the pool",
            eff.travel_cost,
            eff.potato_contributed,
            qty,
            res.label()
        ));
        match eff.effect {
            MarketModeEffect::Boost(t) => {
                if t > 0 {
                    lines.push(format!(
                        "Receives {} {}: +{} production next month",
                        t, wanted, t
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
    let snapshot: Vec<(Vec2, UniformResource)> =
        fr.farms.iter().map(|f| (f.seed, f.produced_resource())).collect();

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
        }
    }

    fn farms_with(farms: Vec<FarmData>, neighbors: Vec<Vec<usize>>) -> FarmsResource {
        let n = farms.len();
        FarmsResource {
            farms,
            circle_pos: Vec2::ZERO,
            traveler_reveals: Vec::new(),
            neighbors,
            farm_events: vec![FarmEvent::Market; n],
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
            fr.farm_events[0] = FarmEvent::RerollResource;
            fr
        };

        // Pool has >= RECONFIGURE_COST timber: A pays and re-rolls.
        let outcome = compute_market(&make(RECONFIGURE_COST + 5));
        assert!(matches!(
            outcome.farm_effects[&0].effect,
            MarketModeEffect::Reroll { paid } if paid == RECONFIGURE_COST
        ));

        // Pool has too little timber: A falls back to a normal boost.
        let outcome = compute_market(&make(RECONFIGURE_COST - 1));
        assert!(matches!(
            outcome.farm_effects[&0].effect,
            MarketModeEffect::Boost(_)
        ));
    }
}
