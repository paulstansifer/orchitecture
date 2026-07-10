//! Unifies everything that touches the shared monthly resource pool — the
//! per-farm market participation, population feeding, a traveler's visit,
//! and construction's material absorption — behind one small `Effect`
//! vocabulary (`possible`/`apply`/`apply_resource`/`effect_name`/`describe`),
//! so both real execution (`month::advance_month`) and the UI's preview/
//! tooltip can share one computation.
//!
//! ## Sequencing
//! Two pools are tracked per resource for the month: `inflow` (this month's
//! fresh delivery) and `storage` (pre-existing stock). Ordered consumers
//! claim from `inflow` first, then (if storage-eligible) from what's left of
//! `storage`: Eat, then TravelerVisit, then farms' own market participation
//! (Boost/Reroll/Specialize/Adopt), then Construction. Eat and TravelerVisit
//! claim directly against the raw pool of invited farms' stockpiles (see
//! `surroundings::farmstead::market_pool`), ahead of farms' own
//! participation, so population feeding and a traveler's demands (and its
//! reward, which joins the same pool) take priority over what farms get to
//! do with their own harvest. Whatever's left of `inflow` after that goes
//! through the existing capacity-contention storage-fill/loss logic
//! (`resource::distribute_incoming_resources`).

use std::collections::HashMap;

use crate::build_ui::remaining_construction_need;
use crate::city::{ConstructedCity, ProposedCity};
use crate::construction::{compute_construction_absorption, Construction};
use crate::materials::MaterialList;
use crate::place;
use crate::population::{Individual, Population};
use crate::resource::{distribute_incoming_resources, ResourceFlow, UniformResource};
use crate::surroundings::farmstead::{
    known_farm_plentifulness, FarmProduction, FarmsResource, MarketModeEffect,
};
use crate::traveler::{ResolvedReward, TravelerState, TravelerVisit};

/// Number of potatoes each individual eats per month.
pub const POTATOES_PER_INDIVIDUAL: u32 = 5;

/// Everything an `Effect::apply()` might need to mutate. Bundled so the
/// trait's `apply()` signature stays uniform across effects with very
/// different needs (a farm's own fields vs. `ProposedCity::resource_progress`
/// vs. `Population`).
pub struct EffectContext<'a> {
    pub constructed: &'a mut ConstructedCity,
    pub pending: &'a mut ProposedCity,
    pub population: &'a mut Population,
    pub farms: &'a mut FarmsResource,
    /// Only consumed by `CityEffect::Market`'s `apply()` in the `Reroll` case.
    pub rng: &'a mut dyn rand::RngCore,
}

/// Something that happens during month-advance and has a resource footprint.
/// `apply_resource` must be a pure read of fields baked in at construction
/// time (by `compute_month_effects`) — no live computation, no side effects —
/// so the same value backs both the read-only preview/tooltip and the real
/// `apply()` mutation.
pub trait Effect {
    /// Whether the relevant checkbox/affordance should be enabled. Not an
    /// "did this succeed" flag — precomputed once, read back here.
    fn possible(&self) -> bool;
    /// Real execution: mutates city state.
    fn apply(&self, ctx: &mut EffectContext);
    /// This effect's net effect on `res` this month. Positive = gained,
    /// negative = consumed.
    fn apply_resource(&self, res: UniformResource) -> i16;
    /// Short tag for the tooltip, e.g. "market", "eat", "traveler", "construction".
    fn effect_name(&self) -> String;
    /// One-line human summary.
    fn describe(&self) -> String;
}

/// Population feeding. Always `possible()`, since it does as much as it can
/// rather than requiring an all-or-nothing affordance.
pub struct Eat {
    pub desired_potato: u32,
    pub granted_potato: u32,
    pub from_storage: u32,
}

impl Eat {
    pub fn possible(&self) -> bool {
        true
    }

    pub fn apply_resource(&self, res: UniformResource) -> i16 {
        if res == UniformResource::Potato {
            -(self.granted_potato as i16)
        } else {
            0
        }
    }

    pub fn apply(&self, ctx: &mut EffectContext) {
        if self.from_storage > 0 {
            place::consume_uniform(ctx.constructed, UniformResource::Potato, self.from_storage);
        }
        let mut remaining = self.granted_potato;
        for individual in &mut ctx.population.individuals {
            if remaining >= POTATOES_PER_INDIVIDUAL {
                individual.fed_this_month = true;
                remaining -= POTATOES_PER_INDIVIDUAL;
            } else {
                individual.fed_this_month = false;
            }
        }
    }

    pub fn effect_name(&self) -> String {
        "eat".to_string()
    }

    pub fn describe(&self) -> String {
        format!(
            "Population eats {} of {} potatoes needed.",
            self.granted_potato, self.desired_potato
        )
    }
}

/// One instance per invited farm's market participation, plus the three
/// singleton kinds. There is no separate `FarmMarketEffect` type — the
/// `Market` variant carries everything a farm's effect needs directly.
pub enum CityEffect {
    Market {
        farm_idx: usize,
        travel_cost: u32,
        potato_contributed: u32,
        wanted_resource: UniformResource,
        inedible_contributed: (UniformResource, u32),
        effect: MarketModeEffect,
    },
    Eat(Eat),
    TravelerVisit(TravelerVisit),
    Construction(Construction),
}

impl Effect for CityEffect {
    fn possible(&self) -> bool {
        match self {
            CityEffect::Market { .. } => true,
            CityEffect::Eat(e) => e.possible(),
            CityEffect::TravelerVisit(t) => t.possible(),
            CityEffect::Construction(c) => c.possible(),
        }
    }

    fn apply(&self, ctx: &mut EffectContext) {
        match self {
            CityEffect::Market {
                farm_idx, effect, ..
            } => {
                let i = *farm_idx;
                match effect {
                    MarketModeEffect::Boost { granted, .. } => {
                        ctx.farms.farms[i].boost = *granted as i32
                    }
                    MarketModeEffect::Adopt => {
                        ctx.farms.farms[i].boost -= 10;
                        ctx.population.individuals.push(Individual::default());
                    }
                    MarketModeEffect::Reroll { paid } => {
                        ctx.farms.farms[i].boost = 0;
                        if *paid > 0 {
                            if let FarmProduction::Specialized(prev) = ctx.farms.farms[i].production
                            {
                                place::deposit_tool(ctx.constructed, prev);
                            }
                            let current = ctx.farms.farms[i].produced_resource();
                            let options: Vec<UniformResource> =
                                UniformResource::inedible_farmables()
                                    .iter()
                                    .copied()
                                    .filter(|&r| r != current)
                                    .collect();
                            use rand::Rng as _;
                            ctx.farms.farms[i].production = FarmProduction::Regular(
                                options[ctx.rng.random_range(0..options.len())],
                            );
                        }
                    }
                    MarketModeEffect::Specialize { paid, tool } => {
                        ctx.farms.farms[i].boost = 0;
                        if *paid > 0 {
                            if let FarmProduction::Specialized(prev) = ctx.farms.farms[i].production
                            {
                                place::deposit_tool(ctx.constructed, prev);
                            }
                            ctx.farms.farms[i].production = FarmProduction::Specialized(*tool);
                        }
                    }
                }
                // Invited farms sold their stockpiles into the pool.
                ctx.farms.farms[i].potato_stockpile = 0;
                ctx.farms.farms[i].inedible_stockpile = 0;
            }
            CityEffect::Eat(e) => e.apply(ctx),
            CityEffect::TravelerVisit(t) => t.apply(ctx.constructed, ctx.farms),
            CityEffect::Construction(c) => c.apply(ctx.pending, ctx.constructed),
        }
    }

    fn apply_resource(&self, res: UniformResource) -> i16 {
        match self {
            CityEffect::Market {
                potato_contributed,
                wanted_resource,
                inedible_contributed,
                effect,
                ..
            } => {
                let mut delta: i32 = 0;
                if res == UniformResource::Potato {
                    delta += *potato_contributed as i32;
                    if let MarketModeEffect::Boost { potatoes_spent, .. } = effect {
                        delta -= *potatoes_spent as i32;
                    }
                }
                let (produced, qty) = *inedible_contributed;
                if res == produced {
                    delta += qty as i32;
                }
                if res == *wanted_resource {
                    delta -= match effect {
                        MarketModeEffect::Boost { granted, .. } => *granted as i32,
                        MarketModeEffect::Reroll { paid } => *paid as i32,
                        MarketModeEffect::Specialize { paid, .. } => *paid as i32,
                        MarketModeEffect::Adopt => 0,
                    };
                }
                delta.clamp(i16::MIN as i32, i16::MAX as i32) as i16
            }
            CityEffect::Eat(e) => e.apply_resource(res),
            CityEffect::TravelerVisit(t) => t.apply_resource(res),
            CityEffect::Construction(c) => c.apply_resource(res),
        }
    }

    fn effect_name(&self) -> String {
        match self {
            CityEffect::Market { .. } => "market".to_string(),
            CityEffect::Eat(e) => e.effect_name(),
            CityEffect::TravelerVisit(t) => t.effect_name(),
            CityEffect::Construction(c) => c.effect_name(),
        }
    }

    fn describe(&self) -> String {
        match self {
            CityEffect::Market { effect, .. } => match effect {
                MarketModeEffect::Boost { granted, .. } if *granted > 0 => {
                    format!("A farm receives {granted} of its wanted resource.")
                }
                MarketModeEffect::Boost { .. } => {
                    "A farm found none of its wanted resource at market.".to_string()
                }
                MarketModeEffect::Reroll { .. } => {
                    "A farm re-rolls its produced resource.".to_string()
                }
                MarketModeEffect::Specialize { tool, .. } => {
                    format!("A farm specializes using a {}.", tool.label())
                }
                MarketModeEffect::Adopt => {
                    "A farm adopts a family, growing the population.".to_string()
                }
            },
            CityEffect::Eat(e) => e.describe(),
            CityEffect::TravelerVisit(t) => t.describe(),
            CityEffect::Construction(c) => c.describe(),
        }
    }
}

/// Precomputed outcome of `compute_month_effects`, threaded through both the
/// read-only preview (ui.rs) and the real execution (month.rs).
pub struct MonthEffects {
    /// One `Market` entry per invited farm, plus `Eat`, an optional
    /// `TravelerVisit`, and an optional `Construction`.
    pub effects: Vec<CityEffect>,
    /// This month's market inflow, before any claims — i.e. what the old
    /// `player_gains` meant.
    pub player_gains: Vec<(UniformResource, u32)>,
    /// Whatever's left of `player_gains` after every effect above has
    /// claimed what it needs: stored (if capacity allows) or lost.
    pub leftover: HashMap<UniformResource, ResourceFlow>,
}

impl MonthEffects {
    pub fn all(&self) -> impl Iterator<Item = &CityEffect> {
        self.effects.iter()
    }

    /// Net change to the storage pool for `res` this month: leftover inflow
    /// that gets deposited there (a `Resource` reward joins that same
    /// inflow pool -- see `compute_month_effects` -- so it's already
    /// reflected in `leftover` rather than added here), minus whatever Eat,
    /// TravelerVisit, or Construction drew from pre-existing storage to
    /// cover a shortfall. Positive = storage grows; negative = storage
    /// shrinks.
    pub fn storage_delta(&self, res: UniformResource) -> i64 {
        let mut delta = self
            .leftover
            .get(&res)
            .map(|f| f.stored as i64)
            .unwrap_or(0);
        for effect in &self.effects {
            match effect {
                CityEffect::Eat(e) if res == UniformResource::Potato => {
                    delta -= e.from_storage as i64;
                }
                CityEffect::TravelerVisit(t) if t.affordable && t.invited => {
                    for &(demand_res, _, _, from_storage) in &t.demands {
                        if demand_res == res {
                            delta -= from_storage as i64;
                        }
                    }
                }
                CityEffect::Construction(c) => {
                    delta -= *c.from_storage.get(&res).unwrap_or(&0) as i64;
                }
                _ => {}
            }
        }
        delta
    }
}

/// Claims up to `desired` of `res`, from `inflow` first and `storage` second,
/// mutating both to subtract what's claimed. Returns `(granted, granted_from_storage)`.
fn claim(
    res: UniformResource,
    desired: u32,
    inflow: &mut HashMap<UniformResource, u32>,
    storage: &mut HashMap<UniformResource, u32>,
) -> (u32, u32) {
    let from_inflow = desired.min(inflow.get(&res).copied().unwrap_or(0));
    if from_inflow > 0 {
        *inflow.get_mut(&res).unwrap() -= from_inflow;
    }
    let from_storage = (desired - from_inflow).min(storage.get(&res).copied().unwrap_or(0));
    if from_storage > 0 {
        *storage.get_mut(&res).unwrap() -= from_storage;
    }
    (from_inflow + from_storage, from_storage)
}

/// Computes the full set of this month's effects, in claim-priority order:
/// Eat, then TravelerVisit, then market delivery (farms' own participation),
/// then Construction, each claiming from this month's inflow first and
/// pre-existing storage second; whatever's left of the inflow goes through
/// the existing storage-fill/loss logic. Eat and a traveler's visit claim
/// directly against the raw pool of invited farms' stockpiles (see
/// `market_pool`), ahead of farms' own Boost/Reroll/Specialize/Adopt
/// participation, so population feeding and a traveler's demands (and its
/// reward, which joins the same pool) take priority over what farms get to
/// do with their own harvest. Pure — takes everything by shared reference,
/// mutates nothing.
#[allow(clippy::too_many_arguments)]
pub fn compute_month_effects(
    farms: &FarmsResource,
    constructed: &ConstructedCity,
    pending: &ProposedCity,
    population: &Population,
    traveler_state: &TravelerState,
    material_list: &MaterialList,
    sandbox_enabled: bool,
) -> MonthEffects {
    use crate::surroundings::farmstead::{market_pool, resolve_market, MarketPool};

    let pool = market_pool(farms);
    let mut inflow: HashMap<UniformResource, u32> = pool.inedible.clone();
    inflow.insert(UniformResource::Potato, pool.potato);
    let mut storage: HashMap<UniformResource, u32> = place::storage_totals(constructed);

    let mut effects: Vec<CityEffect> = Vec::new();

    // 1. Eat.
    let desired_potato = population.individuals.len() as u32 * POTATOES_PER_INDIVIDUAL;
    let (granted_potato, potato_from_storage) = claim(
        UniformResource::Potato,
        desired_potato,
        &mut inflow,
        &mut storage,
    );
    effects.push(CityEffect::Eat(Eat {
        desired_potato,
        granted_potato,
        from_storage: potato_from_storage,
    }));

    // 2. TravelerVisit. `affordable` is computed regardless of `invited` (it
    // drives the "Invite" checkbox), but resources are only actually claimed
    // when the visit is both affordable and invited.
    if let Some(offer) = &traveler_state.current_offer {
        let affordable = offer.demands.iter().all(|&(res, qty)| {
            inflow.get(&res).copied().unwrap_or(0) + storage.get(&res).copied().unwrap_or(0)
                >= qty as u32
        });
        let invited = traveler_state.invited;
        let demands = offer
            .demands
            .iter()
            .map(|&(res, qty)| {
                if invited && affordable {
                    let (granted, from_storage) = claim(res, qty as u32, &mut inflow, &mut storage);
                    (res, qty as u32, granted, from_storage)
                } else {
                    (res, qty as u32, 0, 0)
                }
            })
            .collect();
        // A `Resource` reward joins the same pool (rather than being
        // deposited directly), so it's available -- ahead of farms' own
        // market participation -- to Construction, and any remainder falls
        // through to the normal storage-fill/loss handling below. This also
        // means a reward is never silently lost for lack of a storage room
        // to receive it: it either gets used immediately or is accounted as
        // lost like any other leftover.
        if invited && affordable {
            if let ResolvedReward::Resource(res, qty) = &offer.reward {
                *inflow.entry(*res).or_insert(0) += *qty as u32;
            }
        }
        effects.push(CityEffect::TravelerVisit(TravelerVisit {
            demands,
            reward: offer.reward.clone(),
            path: offer.path.clone(),
            affordable,
            invited,
        }));
    }

    // 3. Market: farms' own Boost/Reroll/Specialize/Adopt participation,
    // against whatever's left of the pool after Eat and a traveler's visit.
    let potato_after = inflow.remove(&UniformResource::Potato).unwrap_or(0);
    let market = resolve_market(
        farms,
        MarketPool {
            invited: pool.invited,
            potato: potato_after,
            inedible: inflow,
        },
    );
    let mut farm_effects: Vec<CityEffect> = market.farm_effects.into_values().collect();
    // `farm_effects` is a `HashMap`, whose default hasher reseeds on every
    // instantiation — sort here so the tooltip/execution order is canonical
    // (by farm id) rather than shuffling from frame to frame.
    farm_effects.sort_by_key(|e| match e {
        CityEffect::Market { farm_idx, .. } => *farm_idx,
        _ => unreachable!("only Market variants exist at this point"),
    });
    effects.extend(farm_effects);

    // Whatever's left after farms took their share becomes this month's
    // inflow for Construction and the leftover-distribution pass below.
    let mut inflow: HashMap<UniformResource, u32> = market.player_gains.iter().copied().collect();

    // 4. Construction: inflow first, then leftover storage.
    if pending.num_changes() > 0 {
        let remaining_need: HashMap<UniformResource, u32> = if sandbox_enabled {
            HashMap::new()
        } else {
            remaining_construction_need(pending, &constructed.eorfs, material_list)
                .into_iter()
                .collect()
        };
        let construction =
            compute_construction_absorption(&remaining_need, &mut inflow, &mut storage);
        effects.push(CityEffect::Construction(construction));
    }

    // 5/6. Leftover inflow -> storage contention -> loss.
    let incoming_leftover: Vec<(UniformResource, u32)> =
        inflow.into_iter().filter(|&(_, q)| q > 0).collect();
    let known_farm_output = known_farm_plentifulness(farms);
    let storage_free_capacity: HashMap<UniformResource, f32> = incoming_leftover
        .iter()
        .map(|&(r, _)| (r, place::storage_free_capacity(constructed, r)))
        .collect();
    let leftover = distribute_incoming_resources(
        &incoming_leftover,
        &place::storage_totals(constructed),
        &storage_free_capacity,
        place::storage_overall_free_capacity(constructed),
        &known_farm_output,
    );

    MonthEffects {
        effects,
        player_gains: market.player_gains,
        leftover,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::city::ConstructedCity;
    use crate::place::{ParentRestriction, ParticularPlace, PlaceStorageSpec};
    use crate::population::Individual;
    use crate::resource::{Approximation, Inventory, UniformResource::*};
    use crate::surroundings::farmstead::{FarmData, FarmEvent};
    use crate::traveler::{IndividualTraveler, ResolvedReward};
    use bevy::prelude::Vec2;

    fn mk_farm(potato: u32) -> FarmData {
        FarmData {
            seed: Vec2::ZERO,
            polygon: Vec::new(),
            area: 5.0,
            fertility: 1.0,
            production: FarmProduction::Regular(Straw),
            wanted_resource: Timber,
            want_max: 5,
            potato_stockpile: potato,
            inedible_stockpile: 0,
            boost: 0,
            invited: true,
        }
    }

    fn farms_with(farms: Vec<FarmData>) -> FarmsResource {
        let n = farms.len();
        FarmsResource {
            farms,
            circle_pos: Vec2::ZERO,
            traveler_reveals: Vec::new(),
            neighbors: vec![Vec::new(); n],
            farm_events: vec![FarmEvent::Market; n],
        }
    }

    fn population(n: usize) -> Population {
        Population {
            individuals: (0..n).map(|_| Individual::default()).collect(),
        }
    }

    fn no_traveler() -> TravelerState {
        TravelerState {
            configs: Vec::new(),
            current_offer: None,
            invited: false,
        }
    }

    fn grid_with_storage(inv: Inventory) -> ConstructedCity {
        let mut cw = ConstructedCity::new(Vec::new());
        cw.places = vec![crate::place::Place {
            name: "storage room".to_string(),
            requirements: vec![],
            storage: Some(PlaceStorageSpec {
                accounting: Approximation {
                    digits: 2,
                    max: 999,
                },
            }),
            quality_factors: vec![],
            assignable_for: None,
        }];
        cw.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![],
            contents: inv,
            restriction: ParentRestriction::Unrestricted,
        });
        cw
    }

    fn eat_of(effects: &MonthEffects) -> &Eat {
        effects
            .effects
            .iter()
            .find_map(|e| match e {
                CityEffect::Eat(eat) => Some(eat),
                _ => None,
            })
            .expect("Eat effect should always be present")
    }

    fn traveler_of(effects: &MonthEffects) -> &TravelerVisit {
        effects
            .effects
            .iter()
            .find_map(|e| match e {
                CityEffect::TravelerVisit(t) => Some(t),
                _ => None,
            })
            .expect("a TravelerVisit effect should be present")
    }

    fn market_boost_granted(effects: &MonthEffects, idx: usize) -> u32 {
        effects
            .effects
            .iter()
            .find_map(|e| match e {
                CityEffect::Market {
                    farm_idx,
                    effect: MarketModeEffect::Boost { granted, .. },
                    ..
                } if *farm_idx == idx => Some(*granted),
                _ => None,
            })
            .expect("farm should have a Boost market effect")
    }

    #[test]
    fn traveler_outranks_farms_own_market_participation() {
        // Farm 0 produces Straw and contributes 10 to the shared pool. Farm 1
        // wants Straw and would happily Boost with all 10 if nothing else
        // claimed it first. A traveler also wants 5 Straw -- it should get
        // its full demand, leaving only the remainder (5) for farm 1's own
        // Boost, even though farms are resolved as `CityEffect::Market`.
        let mut supplier = mk_farm(0);
        supplier.production = FarmProduction::Regular(Straw);
        supplier.inedible_stockpile = 10;
        let mut wanter = mk_farm(0);
        wanter.wanted_resource = Straw;
        wanter.want_max = 10;

        let farms = farms_with(vec![supplier, wanter]);
        let cw = ConstructedCity::new(Vec::new());
        let pending = ProposedCity::new();
        let pop = population(0);

        let mut traveler_state = no_traveler();
        traveler_state.invited = true;
        traveler_state.current_offer = Some(IndividualTraveler {
            config_index: 0,
            demands: vec![(Straw, 5)],
            reward: ResolvedReward::Resource(Potato, 1),
            path: Vec::new(),
        });

        let material_list = MaterialList::default();
        let effects = compute_month_effects(
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        let t = traveler_of(&effects);
        assert!(t.affordable, "traveler should get first dibs on Straw");
        assert_eq!(
            t.demands,
            vec![(Straw, 5, 5, 0)],
            "traveler's full demand should be granted from inflow"
        );
        assert_eq!(
            market_boost_granted(&effects, 1),
            5,
            "farm 1 should only get the 5 Straw left over after the traveler's claim"
        );
    }

    #[test]
    fn eat_outranks_traveler_for_potato_when_scarce() {
        let farms = farms_with(vec![mk_farm(10)]); // exactly enough for Eat, not both
        let cw = ConstructedCity::new(Vec::new());
        let pending = ProposedCity::new();
        let pop = population(2); // desired = 10 potatoes

        let mut traveler_state = no_traveler();
        traveler_state.invited = true;
        traveler_state.current_offer = Some(IndividualTraveler {
            config_index: 0,
            demands: vec![(Potato, 10)],
            reward: ResolvedReward::Resource(Timber, 1),
            path: Vec::new(),
        });

        let material_list = MaterialList::default();
        let effects = compute_month_effects(
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        assert_eq!(eat_of(&effects).granted_potato, 10);
        assert!(!traveler_of(&effects).affordable);
    }

    #[test]
    fn storage_delta_combines_leftover_reward_and_storage_draws() {
        // A `Resource` reward joins this month's inflow (see
        // `compute_month_effects`), so its fate is already folded into
        // `leftover` by the time `storage_delta` runs -- here, 6 units of
        // other leftover inflow plus the 7-unit Timber reward, all stored.
        let mut leftover = HashMap::new();
        leftover.insert(
            Timber,
            ResourceFlow {
                stored: 13,
                lost: 4,
            },
        );

        let effects = MonthEffects {
            effects: vec![
                CityEffect::Eat(Eat {
                    desired_potato: 10,
                    granted_potato: 10,
                    from_storage: 3,
                }),
                CityEffect::TravelerVisit(TravelerVisit {
                    demands: vec![(Straw, 5, 5, 2)],
                    reward: ResolvedReward::Resource(Timber, 7),
                    path: Vec::new(),
                    affordable: true,
                    invited: true,
                }),
                CityEffect::Construction(crate::construction::Construction {
                    applied: HashMap::from([(Straw, 4)]),
                    from_storage: HashMap::from([(Straw, 1)]),
                }),
            ],
            player_gains: Vec::new(),
            leftover,
        };

        // Timber: leftover deposit already includes the traveler reward.
        assert_eq!(effects.storage_delta(Timber), 13);
        // Potato: Eat drew 3 from storage.
        assert_eq!(effects.storage_delta(Potato), -3);
        // Straw: traveler demand drew 2, construction drew 1.
        assert_eq!(effects.storage_delta(Straw), -3);
        // Untouched resource.
        assert_eq!(effects.storage_delta(Fieldstone), 0);
    }

    #[test]
    fn eat_falls_back_to_pre_existing_storage_when_inflow_short() {
        let farms = farms_with(vec![]); // no inflow at all
        let mut inv = Inventory::new(8, 100.0);
        inv.add_uniform(Potato, 20);
        let cw = grid_with_storage(inv);
        let pending = ProposedCity::new();
        let pop = population(2); // desired = 10 potatoes

        let traveler_state = no_traveler();
        let material_list = MaterialList::default();
        let effects = compute_month_effects(
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        let eat = eat_of(&effects);
        assert_eq!(eat.granted_potato, 10);
        assert_eq!(eat.from_storage, 10);
    }

    #[test]
    fn declined_traveler_leaves_resources_untouched() {
        let farms = farms_with(vec![]); // no market inflow
        let cw = ConstructedCity::new(Vec::new()); // no storage either
        let pending = ProposedCity::new();
        let pop = population(0);

        let mut traveler_state = no_traveler();
        traveler_state.invited = true;
        traveler_state.current_offer = Some(IndividualTraveler {
            config_index: 0,
            demands: vec![(Timber, 10)],
            reward: ResolvedReward::Resource(Plank, 1),
            path: Vec::new(),
        });

        let material_list = MaterialList::default();
        let effects = compute_month_effects(
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        let t = traveler_of(&effects);
        assert!(!t.affordable);
        assert!(t.demands.iter().all(|&(_, _, granted, _)| granted == 0));
        assert_eq!(t.apply_resource(Timber), 0);
    }

    #[test]
    fn traveler_not_invited_is_never_active_even_if_affordable() {
        let farms = farms_with(vec![mk_farm(20)]);
        let cw = ConstructedCity::new(Vec::new());
        let pending = ProposedCity::new();
        let pop = population(0);

        let mut traveler_state = no_traveler();
        traveler_state.invited = false; // never checked the box
        traveler_state.current_offer = Some(IndividualTraveler {
            config_index: 0,
            demands: vec![(Potato, 5)],
            reward: ResolvedReward::Resource(Timber, 1),
            path: Vec::new(),
        });

        let material_list = MaterialList::default();
        let effects = compute_month_effects(
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        let t = traveler_of(&effects);
        // Affordable (drives the checkbox), but inactive since it wasn't invited.
        assert!(t.affordable);
        assert!(t.possible());
        assert_eq!(t.apply_resource(Potato), 0);
    }

    /// A brand-new city has no storage room yet (the very first one, "bin",
    /// itself costs Plank — see `buildables/furniture.ron`). A traveler's
    /// `Resource` reward must therefore be usable the same month it arrives,
    /// not just deposited into pre-existing storage: otherwise Plank could
    /// never be bootstrapped at all. Regression test for a bug where the
    /// reward was deposited directly into "the first storage place" and
    /// silently discarded when none existed yet.
    #[test]
    fn plank_reward_pays_off_pending_furniture_with_no_storage_room_yet() {
        use crate::eorf::load_structure_info;

        let infos = load_structure_info();
        let bin_id = crate::eorf::find_structure_by_name(&infos, "bin").expect("bin exists");
        let mut cw = ConstructedCity::new(infos);
        cw.road_forbidden_zone = false;
        assert!(crate::place::storage_ids(&cw).is_empty(), "no storage yet");

        let mut pending = crate::city::ProposedCity::new();
        pending.room_plop(
            &cw,
            bevy::prelude::Vec3::ZERO,
            0,
            Some(bin_id),
            crate::materials::BuildMaterialId::default(),
        );
        let material_list = MaterialList::default();
        assert_eq!(
            crate::build_ui::remaining_construction_need(&pending, &cw.eorfs, &material_list),
            vec![(Plank, 3)],
            "a bin costs exactly 3 plank"
        );

        let farms = farms_with(vec![]);
        let pop = Population {
            individuals: vec![],
        };
        let mut traveler_state = no_traveler();
        traveler_state.invited = true;
        traveler_state.current_offer = Some(IndividualTraveler {
            config_index: 0,
            demands: vec![], // trivially affordable
            reward: ResolvedReward::Resource(Plank, 5),
            path: Vec::new(),
        });

        let effects = compute_month_effects(
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        let construction = effects
            .effects
            .iter()
            .find_map(|e| match e {
                CityEffect::Construction(c) => Some(c.clone()),
                _ => None,
            })
            .expect("a Construction effect should be present");
        // Absorbed straight from this month's inflow (the reward), with no
        // storage room to draw from.
        assert_eq!(construction.applied.get(&Plank), Some(&3));
        assert_eq!(construction.from_storage.get(&Plank), None);

        construction.apply(&mut pending, &mut cw);
        assert!(
            crate::build_ui::remaining_construction_need(&pending, &cw.eorfs, &material_list)
                .is_empty()
        );

        // The 2 leftover reward units (5 - 3) were lost, not deposited,
        // since there's still no storage room -- but that's now accounted
        // for as a normal loss instead of silently vanishing.
        assert_eq!(effects.leftover.get(&Plank).map(|f| f.lost), Some(2));
    }
}
