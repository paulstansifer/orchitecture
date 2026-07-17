//! Unifies everything that touches the shared monthly resource pool — the
//! per-farm market participation, population feeding, a traveler's visit, and
//! construction's material absorption — by threading a single [`Pool`] through
//! them and recording every movement in one [`MonthLedger`]. That ledger is the
//! single source of truth behind the UI's per-resource tooltip and
//! [`MonthEffects::storage_delta`]; the `CityEffect`s themselves only carry out
//! the structural side effects a ledger entry can't express (setting a boost,
//! growing the population, rerolling production, committing construction). Both
//! real execution (`month::advance_month`) and the UI preview share the one
//! `compute_month_effects` computation.
//!
//! ## Sequencing
//! The [`Pool`] tracks two amounts per resource for the month: `inflow` (this
//! month's fresh delivery — invited farms' stockpiles up front, plus a
//! traveler's reward) and `storage` (a snapshot of pre-existing stock). Ordered
//! claimants take from `inflow` first, then (if storage-eligible) from what's
//! left of `storage`: Eat, then TravelerVisit, then farms' own market
//! participation (Boost/Reroll/Specialize/Adopt), then Construction. Because Eat
//! and the traveler claim before farms resolve their own boosts, population
//! feeding and a traveler's demands (and its reward, which joins the same pool)
//! take priority over what farms get to do with their own harvest. Whatever's
//! left of `inflow` after that goes through the existing capacity-contention
//! storage-fill/loss logic (`resource::distribute_incoming_resources`).

use std::collections::HashMap;

use crate::city::{ConstructedCity, ProposedCity};
use crate::construction::{
    compute_construction_absorption, remaining_construction_need, Construction,
};
use crate::materials::MaterialList;
use crate::place;
use crate::population::{Individual, Population};
use crate::resource::{distribute_incoming_resources, ResourceFlow, UniformResource};
use crate::surroundings::farmstead::{
    known_farm_plentifulness, FarmId, FarmProduction, FarmsResource, MarketModeEffect,
    NewProduction,
};
use crate::traveler::{ResolvedReward, TravelerState, TravelerVisit};

/// Number of potatoes each individual eats per month.
pub const POTATOES_PER_INDIVIDUAL: u32 = 5;

/// Which monthly actor a resource movement is attributed to. Drives the
/// tooltip's per-line tag (formerly `Effect::effect_name`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LedgerSource {
    Market,
    Eat,
    Traveler,
    Construction,
}

impl LedgerSource {
    pub fn tag(self) -> &'static str {
        match self {
            LedgerSource::Market => "market",
            LedgerSource::Eat => "eat",
            LedgerSource::Traveler => "traveler",
            LedgerSource::Construction => "construction",
        }
    }
}

/// Display order for the per-resource tooltip: the same order the effects are
/// resolved in (Eat and the traveler get first dibs, then farms' own market
/// participation, then construction).
const LEDGER_SOURCE_ORDER: [LedgerSource; 4] = [
    LedgerSource::Eat,
    LedgerSource::Traveler,
    LedgerSource::Market,
    LedgerSource::Construction,
];

/// One resource movement this month. `net` is the signed change to the
/// player's economy (+ gained / joined the pool, − consumed); `storage_draw`
/// is how much of a consumption came out of *pre-existing* storage (as opposed
/// to this month's inflow, which needs no physical withdrawal).
pub struct LedgerEntry {
    pub source: LedgerSource,
    pub resource: UniformResource,
    pub net: i32,
    pub storage_draw: u32,
}

/// The single accounting record of a month's resource movements — the one
/// source of truth behind the tooltip deltas and `MonthEffects::storage_delta`,
/// built as `compute_month_effects` threads claims through a [`Pool`].
#[derive(Default)]
pub struct MonthLedger {
    pub entries: Vec<LedgerEntry>,
}

impl MonthLedger {
    fn record(
        &mut self,
        source: LedgerSource,
        resource: UniformResource,
        net: i32,
        storage_draw: u32,
    ) {
        if net != 0 || storage_draw != 0 {
            self.entries.push(LedgerEntry {
                source,
                resource,
                net,
                storage_draw,
            });
        }
    }

    /// Net economic change `source` made to `res` this month (the old
    /// per-effect `apply_resource`).
    pub fn net_for(&self, source: LedgerSource, res: UniformResource) -> i32 {
        self.entries
            .iter()
            .filter(|e| e.source == source && e.resource == res)
            .map(|e| e.net)
            .sum()
    }

    /// Total of `res` drawn from pre-existing storage across all sources.
    pub fn storage_draw_for(&self, res: UniformResource) -> u32 {
        self.entries
            .iter()
            .filter(|e| e.resource == res)
            .map(|e| e.storage_draw)
            .sum()
    }

    /// Each source that moved `res` this month and its net, in display order —
    /// drives the per-resource tooltip lines.
    pub fn sources_touching(
        &self,
        res: UniformResource,
    ) -> impl Iterator<Item = (LedgerSource, i32)> + '_ {
        LEDGER_SOURCE_ORDER
            .into_iter()
            .map(move |source| (source, self.net_for(source, res)))
            .filter(|&(_, net)| net != 0)
    }
}

/// This month's resource flow: `inflow` is fresh delivery (farm stockpiles,
/// then a traveler's reward), `storage` is a snapshot of pre-existing stock.
/// Ordered claimants take from `inflow` first and `storage` second; every
/// movement is recorded in `ledger`. Folds together the old `MarketPool`
/// pass-through and the separate potato/inedible pool threading.
pub struct Pool {
    pub inflow: HashMap<UniformResource, u32>,
    pub storage: HashMap<UniformResource, u32>,
    pub ledger: MonthLedger,
}

impl Pool {
    pub fn new(storage: HashMap<UniformResource, u32>) -> Self {
        Pool {
            inflow: HashMap::new(),
            storage,
            ledger: MonthLedger::default(),
        }
    }

    /// Add fresh delivery to the inflow pool, attributed to `source`.
    pub fn contribute(&mut self, source: LedgerSource, res: UniformResource, qty: u32) {
        if qty == 0 {
            return;
        }
        *self.inflow.entry(res).or_insert(0) += qty;
        self.ledger.record(source, res, qty as i32, 0);
    }

    /// Inflow available for `res` right now (not counting pre-existing storage).
    pub fn inflow_available(&self, res: UniformResource) -> u32 {
        self.inflow.get(&res).copied().unwrap_or(0)
    }

    /// Everything available for `res` right now, inflow plus pre-existing storage.
    pub fn available(&self, res: UniformResource) -> u32 {
        self.inflow_available(res) + self.storage.get(&res).copied().unwrap_or(0)
    }

    /// Claim up to `desired` of `res`, from inflow first then storage, recording
    /// the consumption. Returns `(granted, granted_from_storage)`.
    pub fn claim(
        &mut self,
        source: LedgerSource,
        res: UniformResource,
        desired: u32,
    ) -> (u32, u32) {
        let from_inflow = desired.min(self.inflow_available(res));
        if from_inflow > 0 {
            *self.inflow.get_mut(&res).unwrap() -= from_inflow;
        }
        let from_storage =
            (desired - from_inflow).min(self.storage.get(&res).copied().unwrap_or(0));
        if from_storage > 0 {
            *self.storage.get_mut(&res).unwrap() -= from_storage;
        }
        let granted = from_inflow + from_storage;
        self.ledger
            .record(source, res, -(granted as i32), from_storage);
        (granted, from_storage)
    }

    /// Claim up to `desired` of `res` from inflow only (market boosts never
    /// touch pre-existing storage), recording it. Returns the amount granted.
    pub fn claim_inflow(
        &mut self,
        source: LedgerSource,
        res: UniformResource,
        desired: u32,
    ) -> u32 {
        let take = desired.min(self.inflow_available(res));
        if take > 0 {
            *self.inflow.get_mut(&res).unwrap() -= take;
            self.ledger.record(source, res, -(take as i32), 0);
        }
        take
    }
}

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

/// Population feeding. Does as much as it can rather than requiring an
/// all-or-nothing affordance.
pub struct Eat {
    pub desired_potato: u32,
    pub granted_potato: u32,
    pub from_storage: u32,
}

impl Eat {
    pub fn apply(&self, ctx: &mut EffectContext) {
        if self.from_storage > 0 {
            place::consume_uniform(ctx.constructed, UniformResource::Potato, self.from_storage);
        }
        // Prorate any shortfall evenly across the population rather than
        // fully feeding some individuals and starving others.
        let fraction = if self.desired_potato == 0 {
            1.0
        } else {
            (self.granted_potato as f32 / self.desired_potato as f32).min(1.0)
        };
        for individual in &mut ctx.population.individuals {
            individual.fed_fraction = fraction;
        }
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
        farm_idx: FarmId,
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

impl CityEffect {
    /// Real execution: mutates city state. The resource accounting lives in the
    /// [`MonthLedger`] instead (built by `compute_month_effects`); this only
    /// carries out the structural side effects a ledger entry can't express
    /// (setting a boost, growing the population, rerolling production, depositing
    /// a tool, committing construction, zeroing sold stockpiles).
    pub fn apply(&self, ctx: &mut EffectContext) {
        match self {
            CityEffect::Market {
                farm_idx, effect, ..
            } => {
                let i = *farm_idx;
                match effect {
                    MarketModeEffect::Boost { granted, .. } => ctx.farms[i].boost = *granted as i32,
                    MarketModeEffect::Adopt => {
                        ctx.farms[i].boost -= 10;
                        ctx.population.individuals.push(Individual::default());
                    }
                    MarketModeEffect::Reconfigure {
                        paid,
                        new_production,
                    } => {
                        ctx.farms[i].boost = 0;
                        if *paid > 0 {
                            if let FarmProduction::Specialized(prev) = ctx.farms[i].production {
                                place::deposit_tool(ctx.constructed, prev);
                            }
                            ctx.farms[i].production = match new_production {
                                NewProduction::RandomRegular => {
                                    let current = ctx.farms[i].produced_resource();
                                    let options: Vec<UniformResource> =
                                        UniformResource::inedible_farmables()
                                            .iter()
                                            .copied()
                                            .filter(|&r| r != current)
                                            .collect();
                                    use rand::Rng as _;
                                    FarmProduction::Regular(
                                        options[ctx.rng.random_range(0..options.len())],
                                    )
                                }
                                NewProduction::Tool(tool) => FarmProduction::Specialized(*tool),
                            };
                        }
                    }
                }
                // Invited farms sold their stockpiles into the pool.
                ctx.farms[i].potato_stockpile = 0;
                ctx.farms[i].inedible_stockpile = 0;
            }
            CityEffect::Eat(e) => e.apply(ctx),
            CityEffect::TravelerVisit(t) => t.apply(ctx.constructed, ctx.farms),
            CityEffect::Construction(c) => c.apply(ctx.pending, ctx.constructed),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            CityEffect::Market { effect, .. } => match effect {
                MarketModeEffect::Boost { granted, .. } if *granted > 0 => {
                    format!("A farm receives {granted} of its wanted resource.")
                }
                MarketModeEffect::Boost { .. } => {
                    "A farm found none of its wanted resource at market.".to_string()
                }
                MarketModeEffect::Reconfigure {
                    new_production: NewProduction::RandomRegular,
                    ..
                } => "A farm re-rolls its produced resource.".to_string(),
                MarketModeEffect::Reconfigure {
                    new_production: NewProduction::Tool(tool),
                    ..
                } => format!("A farm specializes using a {}.", tool.label()),
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
    /// The single accounting record of every resource movement this month.
    pub ledger: MonthLedger,
}

impl MonthEffects {
    pub fn all(&self) -> impl Iterator<Item = &CityEffect> {
        self.effects.iter()
    }

    /// Net change to the storage pool for `res` this month: leftover inflow
    /// that gets deposited there (a `Resource` reward joins that same inflow
    /// pool -- see `compute_month_effects` -- so it's already reflected in
    /// `leftover`), minus whatever any source drew from pre-existing storage to
    /// cover a shortfall. Positive = storage grows; negative = storage shrinks.
    pub fn storage_delta(&self, res: UniformResource) -> i64 {
        let stored = self
            .leftover
            .get(&res)
            .map(|f| f.stored as i64)
            .unwrap_or(0);
        stored - self.ledger.storage_draw_for(res) as i64
    }

    /// This month's traveler visit, if a traveler is offering one.
    pub fn traveler_visit(&self) -> Option<&TravelerVisit> {
        self.effects.iter().find_map(|e| match e {
            CityEffect::TravelerVisit(t) => Some(t),
            _ => None,
        })
    }

    /// Whether the traveler's demands can be met from this month's pool.
    /// `false` when there is no traveler this month.
    pub fn traveler_affordable(&self) -> bool {
        self.traveler_visit().is_some_and(|t| t.affordable)
    }
}

/// Computes the full set of this month's effects, in claim-priority order:
/// Eat, then TravelerVisit, then market delivery (farms' own participation),
/// then Construction. Every resource movement is threaded through a single
/// [`Pool`] — claimants take from this month's inflow first and pre-existing
/// storage second, and each claim/contribution is recorded in the pool's
/// [`MonthLedger`]. Eat and a traveler's visit claim ahead of farms' own
/// Boost/Reroll/Specialize/Adopt participation, so population feeding and a
/// traveler's demands (and its reward, which joins the same pool) take priority
/// over what farms get to do with their own harvest. Pure — takes everything by
/// shared reference, mutates nothing outside its own `Pool`.
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
    use crate::surroundings::farmstead::{resolve_market, seed_market};

    let mut pool = Pool::new(place::storage_totals(constructed));
    // Invited farms deliver their stockpiles into the pool up front, so the
    // claimants below can take from that inflow in priority order.
    let invited = seed_market(farms, &mut pool);

    let mut effects: Vec<CityEffect> = Vec::new();

    // 1. Eat.
    let desired_potato = population.individuals.len() as u32 * POTATOES_PER_INDIVIDUAL;
    let (granted_potato, potato_from_storage) =
        pool.claim(LedgerSource::Eat, UniformResource::Potato, desired_potato);
    effects.push(CityEffect::Eat(Eat {
        desired_potato,
        granted_potato,
        from_storage: potato_from_storage,
    }));

    // 2. TravelerVisit. `affordable` is computed regardless of `invited` (it
    // drives the "Invite" checkbox), but resources are only actually claimed
    // when the visit is both affordable and invited.
    if let Some(offer) = &traveler_state.current_offer {
        let affordable = offer
            .demands
            .iter()
            .all(|&(res, qty)| pool.available(res) >= qty as u32);
        let invited_traveler = traveler_state.invited;
        let demands = offer
            .demands
            .iter()
            .map(|&(res, qty)| {
                if invited_traveler && affordable {
                    let (granted, from_storage) =
                        pool.claim(LedgerSource::Traveler, res, qty as u32);
                    (res, qty as u32, granted, from_storage)
                } else {
                    (res, qty as u32, 0, 0)
                }
            })
            .collect();
        // A `Resource` reward joins the same pool (rather than being deposited
        // directly), so it's available -- ahead of farms' own market
        // participation -- to Construction, and any remainder falls through to
        // the normal storage-fill/loss handling below. This also means a reward
        // is never silently lost for lack of a storage room to receive it: it
        // either gets used immediately or is accounted as lost like any other
        // leftover.
        if invited_traveler && affordable {
            if let ResolvedReward::Resource(res, qty) = &offer.reward {
                pool.contribute(LedgerSource::Traveler, *res, *qty as u32);
            }
        }
        effects.push(CityEffect::TravelerVisit(TravelerVisit {
            demands,
            reward: offer.reward.clone(),
            path: offer.path.clone(),
            affordable,
            invited: invited_traveler,
        }));
    }

    // 3. Market: farms' own Boost/Reroll/Specialize/Adopt participation,
    // against whatever's left of the pool's inflow after Eat and the traveler.
    // `resolve_market` returns a `BTreeMap`, so this is already in canonical
    // (by farm id) order rather than shuffling from frame to frame.
    effects.extend(resolve_market(farms, &invited, &mut pool).into_values());

    // Whatever inflow farms left in the pool is the player's gains, and this
    // month's inflow for Construction and the leftover-distribution pass below.
    let player_gains: Vec<(UniformResource, u32)> = {
        let mut gains: Vec<(UniformResource, u32)> = pool
            .inflow
            .iter()
            .filter(|(_, &q)| q > 0)
            .map(|(&r, &q)| (r, q))
            .collect();
        gains.sort_by_key(|&(r, _)| r);
        gains
    };

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
            compute_construction_absorption(&remaining_need, &mut pool.inflow, &mut pool.storage);
        for (&res, &applied) in &construction.applied {
            let from_storage = construction.from_storage.get(&res).copied().unwrap_or(0);
            pool.ledger.record(
                LedgerSource::Construction,
                res,
                -(applied as i32),
                from_storage,
            );
        }
        effects.push(CityEffect::Construction(construction));
    }

    // 5/6. Leftover inflow -> storage contention -> loss.
    let incoming_leftover: Vec<(UniformResource, u32)> = pool
        .inflow
        .iter()
        .filter(|(_, &q)| q > 0)
        .map(|(&r, &q)| (r, q))
        .collect();
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
        player_gains,
        leftover,
        ledger: pool.ledger,
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
            event: FarmEvent::Market,
        }
    }

    fn farms_with(farms: Vec<FarmData>) -> FarmsResource {
        let n = farms.len();
        FarmsResource {
            farms,
            circle_pos: Vec2::ZERO,
            traveler_reveals: Vec::new(),
            neighbors: vec![Vec::new(); n],
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

    fn market_boost_granted(effects: &MonthEffects, idx: FarmId) -> u32 {
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
            market_boost_granted(&effects, FarmId::new(1)),
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

        // The ledger records each source's storage draw: Eat drew 3 Potato,
        // the traveler drew 2 Straw for its demand (and its 7-Timber reward
        // joined the inflow, hence the Timber leftover above), and Construction
        // drew 1 Straw. `storage_delta` is `leftover.stored − storage_draw`.
        let ledger = MonthLedger {
            entries: vec![
                LedgerEntry {
                    source: LedgerSource::Eat,
                    resource: Potato,
                    net: -10,
                    storage_draw: 3,
                },
                LedgerEntry {
                    source: LedgerSource::Traveler,
                    resource: Straw,
                    net: -5,
                    storage_draw: 2,
                },
                LedgerEntry {
                    source: LedgerSource::Traveler,
                    resource: Timber,
                    net: 7,
                    storage_draw: 0,
                },
                LedgerEntry {
                    source: LedgerSource::Construction,
                    resource: Straw,
                    net: -4,
                    storage_draw: 1,
                },
            ],
        };
        let effects = MonthEffects {
            effects: vec![],
            player_gains: Vec::new(),
            leftover,
            ledger,
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
    fn ledger_conserves_resources() {
        // A farm delivers 10 potato + 8 straw; the city holds 20 potato in
        // storage and has 3 mouths to feed (needs 15). Eat drains the 10 potato
        // of inflow and tops up 5 from storage; the straw falls through to
        // storage. For every resource, the ledger's net inflow change plus what
        // was drawn from pre-existing storage must equal what ended up in
        // leftover (stored + lost) — nothing is created or destroyed.
        let mut farm = mk_farm(10);
        farm.inedible_stockpile = 8; // produces Straw (see `mk_farm`)
        let farms = farms_with(vec![farm]);

        let mut inv = Inventory::new(100.0);
        inv.add_uniform(Potato, 20);
        let cw = grid_with_storage(inv);
        let pending = ProposedCity::new();
        let pop = population(3); // desired = 15 potatoes

        let material_list = MaterialList::default();
        let effects = compute_month_effects(
            &farms,
            &cw,
            &pending,
            &pop,
            &no_traveler(),
            &material_list,
            false,
        );

        for &res in UniformResource::ALL {
            let net: i64 = effects
                .ledger
                .entries
                .iter()
                .filter(|e| e.resource == res)
                .map(|e| e.net as i64)
                .sum();
            let storage_draw = effects.ledger.storage_draw_for(res) as i64;
            let leftover = effects
                .leftover
                .get(&res)
                .map(|f| (f.stored + f.lost) as i64)
                .unwrap_or(0);
            assert_eq!(
                net + storage_draw,
                leftover,
                "conservation failed for {res:?}"
            );
        }

        // Spot-check the interesting flows.
        assert_eq!(effects.ledger.net_for(LedgerSource::Eat, Potato), -15);
        assert_eq!(effects.ledger.storage_draw_for(Potato), 5);
    }

    #[test]
    fn eat_falls_back_to_pre_existing_storage_when_inflow_short() {
        let farms = farms_with(vec![]); // no inflow at all
        let mut inv = Inventory::new(100.0);
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
        // A declined traveler touches nothing in the ledger.
        assert_eq!(effects.ledger.net_for(LedgerSource::Traveler, Timber), 0);
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
        // Affordable (drives the checkbox), but inactive since it wasn't invited,
        // so it claimed nothing from the pool.
        assert!(t.affordable);
        assert_eq!(effects.ledger.net_for(LedgerSource::Traveler, Potato), 0);
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
            crate::construction::remaining_construction_need(&pending, &cw.eorfs, &material_list),
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
        assert!(crate::construction::remaining_construction_need(
            &pending,
            &cw.eorfs,
            &material_list
        )
        .is_empty());

        // The 2 leftover reward units (5 - 3) were lost, not deposited,
        // since there's still no storage room -- but that's now accounted
        // for as a normal loss instead of silently vanishing.
        assert_eq!(effects.leftover.get(&Plank).map(|f| f.lost), Some(2));
    }

    /// A potato shortfall is prorated evenly across the population, rather
    /// than fully feeding some individuals and starving others.
    #[test]
    fn eat_prorates_shortfall_evenly() {
        use rand::{rngs::StdRng, SeedableRng};

        let mut cw = ConstructedCity::new(Vec::new());
        let mut pending = crate::city::ProposedCity::new();
        let mut population = population(4); // needs 4 * 5 = 20 potato
        let mut farms = farms_with(vec![]);
        let mut rng = StdRng::seed_from_u64(0);

        let eat = Eat {
            desired_potato: 20,
            granted_potato: 10, // half of what's needed
            from_storage: 0,
        };
        let mut ctx = EffectContext {
            constructed: &mut cw,
            pending: &mut pending,
            population: &mut population,
            farms: &mut farms,
            rng: &mut rng,
        };
        eat.apply(&mut ctx);

        // Every individual gets the same half-satisfied ration, instead of
        // half the population being fully fed and the other half starved.
        for individual in &population.individuals {
            assert_eq!(individual.fed_fraction, 0.5);
            assert_eq!(individual.food(), 0.625); // 0.25 + 0.75 * 0.5
        }
    }
}
