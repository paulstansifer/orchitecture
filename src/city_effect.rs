//! Unifies everything that touches the shared monthly resource pool — the
//! per-farm market participation, population feeding, a traveler's visit,
//! and construction's material absorption — behind one small `Effect`
//! vocabulary (`possible`/`apply`/`apply_resource`/`effect_name`/`describe`),
//! so both real execution (`month::advance_month`) and the UI's preview/
//! tooltip can share one computation.
//!
//! ## Sequencing
//! Two pools are tracked per resource for the month: `inflow` (this month's
//! fresh market delivery) and `storage` (pre-existing stock). Ordered
//! consumers claim from `inflow` first, then (if storage-eligible) from
//! what's left of `storage`: Eat, then TravelerVisit, then Construction.
//! Whatever's left of `inflow` after that goes through the existing
//! capacity-contention storage-fill/loss logic
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
    compute_market, known_farm_plentifulness, FarmProduction, FarmsResource, MarketModeEffect,
};
use crate::traveler::{TravelerState, TravelerVisit};

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
/// market delivery, then Eat, then TravelerVisit, then Construction, each
/// claiming from this month's inflow first and pre-existing storage second;
/// whatever's left of the inflow goes through the existing storage-fill/loss
/// logic. Pure — takes everything by shared reference, mutates nothing.
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
    let market = compute_market(farms);
    let mut inflow: HashMap<UniformResource, u32> = market.player_gains.iter().copied().collect();
    let mut storage: HashMap<UniformResource, u32> = place::storage_totals(constructed);

    let mut effects: Vec<CityEffect> = market.farm_effects.into_values().collect();
    // `farm_effects` is a `HashMap`, whose default hasher reseeds on every
    // instantiation — sort here so the tooltip/execution order is canonical
    // (by farm id) rather than shuffling from frame to frame.
    effects.sort_by_key(|e| match e {
        CityEffect::Market { farm_idx, .. } => *farm_idx,
        _ => unreachable!("only Market variants exist at this point"),
    });

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
        effects.push(CityEffect::TravelerVisit(TravelerVisit {
            demands,
            reward: offer.reward.clone(),
            path: offer.path.clone(),
            affordable,
            invited,
        }));
    }

    // 3. Construction: inflow first, then leftover storage.
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

    // 4/5. Leftover inflow -> storage contention -> loss.
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
                just_one_kind: false,
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
}
