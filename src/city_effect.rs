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
//! participation (Boost/Reroll/Specialize/Adopt), then Construction, then
//! Workshops. Because Eat and the traveler claim before farms resolve their own
//! boosts, population feeding and a traveler's demands (and its reward, which
//! joins the same pool) take priority over what farms get to do with their own
//! harvest. Workshops resolve last, after construction, so they process only
//! what those claimants left behind; their outputs join the same pool but are
//! capped at the free storage capacity for the output (they block on back
//! pressure rather than overproduce into loss). Whatever's left of `inflow`
//! after that goes through the existing capacity-contention storage-fill/loss
//! logic (`resource::distribute_incoming_resources`).

use std::collections::{BTreeMap, HashMap};

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
    NewProduction, ADOPT_PENALTY,
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
    /// Half of whatever Fieldstone survives market transactions is spent
    /// paving its own delivery routes — see `pave_fieldstone_routes`.
    Paving,
    Construction,
    /// Staffed workshops converting stored/inflow inputs into outputs, resolved
    /// after construction — see `Workshop` and `compute_workshop_effect`.
    Workshop,
}

impl LedgerSource {
    /// Every variant. `LEDGER_SOURCE_ORDER` must be a permutation of this (see
    /// `ledger_source_order_is_complete`); the exhaustive `tag()` match below
    /// forces this list to be updated whenever a variant is added.
    pub const ALL: [LedgerSource; 6] = [
        LedgerSource::Market,
        LedgerSource::Eat,
        LedgerSource::Traveler,
        LedgerSource::Paving,
        LedgerSource::Construction,
        LedgerSource::Workshop,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            LedgerSource::Market => "market",
            LedgerSource::Eat => "eat",
            LedgerSource::Traveler => "traveler",
            LedgerSource::Paving => "paving",
            LedgerSource::Construction => "construction",
            LedgerSource::Workshop => "workshop",
        }
    }
}

/// Display order for the per-resource tooltip: the same order the effects are
/// resolved in (Eat and the traveler get first dibs, then farms' own market
/// participation, then road paving, then construction).
const LEDGER_SOURCE_ORDER: [LedgerSource; LedgerSource::ALL.len()] = [
    LedgerSource::Eat,
    LedgerSource::Traveler,
    LedgerSource::Market,
    LedgerSource::Paving,
    LedgerSource::Construction,
    LedgerSource::Workshop,
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

    /// Whatever nonzero inflow remains, as the player's gains — sorted by
    /// resource for a deterministic order.
    pub fn gains(&self) -> Vec<(UniformResource, u32)> {
        let mut gains: Vec<(UniformResource, u32)> = self
            .inflow
            .iter()
            .filter(|(_, &q)| q > 0)
            .map(|(&r, &q)| (r, q))
            .collect();
        gains.sort_by_key(|&(r, _)| r);
        gains
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
    /// Only written by `CityEffect::TravelerVisit`'s `apply()`, when the
    /// traveler brought a book.
    pub idea_state: &'a mut crate::idea::IdeaState,
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

/// Staffed workshops' monthly production, resolved after construction. The
/// resource accounting (inputs drawn, outputs produced) lives in the
/// [`MonthLedger`] via [`compute_workshop_effect`]; `apply` only carries out the
/// physical withdrawal of inputs taken from pre-existing storage (the inflow
/// portion needs no withdrawal, and outputs are deposited by the shared
/// leftover pass, exactly like a traveler's `Resource` reward).
pub struct Workshop {
    /// `(input resource, amount)` to physically withdraw from storage in `apply`.
    pub input_draws: Vec<(UniformResource, u32)>,
    /// `(input, output, qty)` conversions this month, for `describe`.
    pub conversions: Vec<(UniformResource, UniformResource, u32)>,
}

impl Workshop {
    pub fn apply(&self, constructed: &mut ConstructedCity) {
        for (res, qty) in &self.input_draws {
            if *qty > 0 {
                place::consume_uniform(constructed, *res, *qty);
            }
        }
    }

    pub fn describe(&self) -> String {
        let parts: Vec<String> = self
            .conversions
            .iter()
            .map(|(input, output, qty)| {
                format!("{qty} {} → {qty} {}", input.label(), output.label())
            })
            .collect();
        format!("Workshops process {}.", parts.join(", "))
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
        inedible_contributed: (UniformResource, u32),
        effect: MarketModeEffect,
    },
    Eat(Eat),
    TravelerVisit(TravelerVisit),
    Construction(Construction),
    Workshop(Workshop),
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
                    MarketModeEffect::Boost { amount } => ctx.farms[i].boost += amount,
                    MarketModeEffect::Adopt => {
                        ctx.farms[i].boost -= ADOPT_PENALTY;
                        ctx.population.individuals.push(Individual::default());
                    }
                    MarketModeEffect::Reconfigure {
                        paid,
                        paid_from_storage,
                        new_production,
                    } => {
                        ctx.farms[i].boost = 0;
                        // Physically withdraw the portion of the cost that came
                        // out of the player's stored potatoes; the inflow portion
                        // was farms' own delivery and needs no withdrawal.
                        if *paid_from_storage > 0 {
                            place::consume_uniform(
                                ctx.constructed,
                                UniformResource::Potato,
                                *paid_from_storage,
                            );
                        }
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
            CityEffect::TravelerVisit(t) => t.apply(ctx.constructed, ctx.farms, ctx.idea_state),
            CityEffect::Construction(c) => c.apply(ctx.pending, ctx.constructed),
            CityEffect::Workshop(w) => w.apply(ctx.constructed),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            CityEffect::Market { effect, .. } => match effect {
                MarketModeEffect::Boost { amount } => {
                    format!("A farm trades at the market: +{amount} production.")
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
            CityEffect::Workshop(w) => w.describe(),
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
    /// Fieldstone diverted to road paving this month (half of whatever
    /// survived market transactions). Applied to the road network separately
    /// via `FarmsResource::pave_fieldstone_routes`.
    pub fieldstone_for_paving: u32,
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

    /// Per-farm market participation, projected out of this month's effects and
    /// keyed by farm — the same shape `farmstead::compute_market` produces in
    /// isolation, but reflecting the full monthly pipeline (feeding and a
    /// traveler claim ahead of farms, so a reconfigure's affordability here
    /// accounts for that contention). Drives the farm previews.
    pub fn market_effects(&self) -> BTreeMap<FarmId, &CityEffect> {
        self.effects
            .iter()
            .filter_map(|e| match e {
                CityEffect::Market { farm_idx, .. } => Some((*farm_idx, e)),
                _ => None,
            })
            .collect()
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

    /// Whether this month's effects will deposit a tool into rack storage --
    /// a farm respecializing away from a tool returns its previous one, or an
    /// invited-and-affordable traveler's reward is a tool. Read-only preview
    /// of what `CityEffect::apply` would do, for the UI's "will receive any"
    /// row-visibility check (see `place::deposit_tool`).
    pub fn tools_incoming(&self, farms: &FarmsResource) -> bool {
        self.effects.iter().any(|e| match e {
            CityEffect::Market {
                farm_idx,
                effect: MarketModeEffect::Reconfigure { paid, .. },
                ..
            } => *paid > 0 && matches!(farms[*farm_idx].production, FarmProduction::Specialized(_)),
            CityEffect::TravelerVisit(t) => {
                t.invited && t.affordable && matches!(t.reward, ResolvedReward::Tool(_))
            }
            _ => false,
        })
    }
}

/// Resolve every staffed workshop's monthly production against `pool` (after
/// construction has taken its share), recording each input claim and output
/// contribution in the ledger under [`LedgerSource::Workshop`]. Inputs are taken
/// inflow-first then storage; outputs rejoin the pool's inflow (so they flow
/// through the same leftover storage-contention as any other gain).
///
/// Workshops **block on back pressure**: each is capped so its output never
/// exceeds the free storage capacity for that resource, and only the
/// correspondingly reduced input is consumed. A workshop whose output has
/// nowhere to go therefore does nothing (and keeps its input) rather than
/// producing something that would be lost. Since a unit of input and its output
/// are both one volume, this is exactly what keeps the prediction and the
/// physical month in step. Returns the `Workshop` effect carrying the storage
/// draws to physically withdraw, or `None` if nothing was produced.
fn compute_workshop_effect(
    constructed: &ConstructedCity,
    population: &Population,
    pool: &mut Pool,
) -> Option<Workshop> {
    use crate::place::WorkEffect;

    let mut input_draws: Vec<(UniformResource, u32)> = Vec::new();
    let mut conversions: Vec<(UniformResource, UniformResource, u32)> = Vec::new();
    // Free storage room remaining for each output resource, decremented as
    // workshops fill it (so several workshops sharing an output can't each
    // assume the whole capacity).
    let mut output_room: HashMap<UniformResource, u32> = HashMap::new();

    for (id, pp) in constructed.placed_places.iter() {
        let Some(effect) = &constructed.places[pp.place].work else {
            continue;
        };
        let staffing = crate::work::workplace_staffing(&population.individuals, id);
        if staffing <= 0.0 {
            continue;
        }
        match effect {
            WorkEffect::TodoEffect => {}
            WorkEffect::ToolEffect => {
                let Some(kind) = crate::work::installed_tool_in(constructed, id) else {
                    continue;
                };
                // An idea-gated workshop can exist while still understood too
                // poorly to be worth anything; `None` shouldn't be reachable
                // (a locked place can't form), but treat it as idle if it is.
                let Some(efficiency) = place::gate_efficiency(constructed, pp.place) else {
                    continue;
                };
                let spec = kind.specialization();
                let desired = (kind.rate() * staffing * efficiency).round() as u32;
                let room = output_room.entry(spec.output).or_insert_with(|| {
                    place::storage_free_capacity(constructed, spec.output).floor() as u32
                });
                let want = desired.min(*room);
                if want == 0 {
                    continue;
                }
                let (granted, from_storage) = pool.claim(LedgerSource::Workshop, spec.input, want);
                if granted == 0 {
                    continue;
                }
                *room -= granted;
                pool.contribute(LedgerSource::Workshop, spec.output, granted);
                input_draws.push((spec.input, from_storage));
                conversions.push((spec.input, spec.output, granted));
            }
        }
    }

    (!conversions.is_empty()).then_some(Workshop {
        input_draws,
        conversions,
    })
}

/// Everything `compute_month_effects` needs besides the farms, bundled so
/// preview callers (which run the month pipeline speculatively, sometimes with
/// a farm temporarily mutated) can thread it as one value instead of a long
/// argument list. Farms stay a separate argument since previews mutate-and-
/// restore them.
#[derive(Clone, Copy)]
pub struct MonthInputs<'a> {
    pub constructed: &'a ConstructedCity,
    pub pending: &'a ProposedCity,
    pub population: &'a Population,
    pub traveler_state: &'a TravelerState,
    pub material_list: &'a MaterialList,
    pub sandbox_enabled: bool,
}

impl MonthInputs<'_> {
    /// Run the full month pipeline for `farms` under these inputs.
    pub fn compute(&self, farms: &FarmsResource) -> MonthEffects {
        compute_month_effects(
            farms,
            self.constructed,
            self.pending,
            self.population,
            self.traveler_state,
            self.material_list,
            self.sandbox_enabled,
        )
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

    // A snapshot of pre-existing storage. `compute_month_effects` never mutates
    // `constructed`, so this is stable for the whole computation and is reused
    // by the leftover-distribution pass below rather than re-scanned.
    let storage_snapshot = place::storage_totals(constructed);
    let mut pool = Pool::new(storage_snapshot.clone());
    // Attending farms deliver their stockpiles into the pool up front, so the
    // claimants below can take from that inflow in priority order.
    let invited = seed_market(farms, &mut pool);

    // What the construction plan is still short of — the resources the market
    // pays a premium for, which is also what drew some of these farms in (see
    // `surroundings::attendance`).
    let demand = crate::surroundings::attendance::construction_demand(MonthInputs {
        constructed,
        pending,
        population,
        traveler_state,
        material_list,
        sandbox_enabled,
    });

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
        // A `Resource` reward is never lost for lack of storage room (see
        // below), but `Tool` and `Book` rewards are deposited directly into
        // rack and bookcase storage by `TravelerVisit::apply`, which silently
        // no-ops if there's no room. Fold that into `affordable` so the
        // "Invite" checkbox (which `affordable` drives) isn't enabled for a
        // reward the city has nowhere to put.
        let reward_deliverable = match &offer.reward {
            ResolvedReward::Tool(_) => {
                place::rack_free_capacity(constructed, crate::resource::RackContents::Tools) >= 1.0
            }
            ResolvedReward::Resource(..) => true,
            ResolvedReward::Book { .. } => {
                place::book_free_capacity(constructed) >= place::book_volume()
            }
        };
        let affordable = reward_deliverable
            && offer
                .demands
                .iter()
                .all(|&(res, qty)| pool.available(res) >= qty);
        let invited_traveler = traveler_state.invited;
        let demands = offer
            .demands
            .iter()
            .map(|&(res, qty)| {
                if invited_traveler && affordable {
                    let (granted, from_storage) = pool.claim(LedgerSource::Traveler, res, qty);
                    (res, qty, granted, from_storage)
                } else {
                    (res, qty, 0, 0)
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
                pool.contribute(LedgerSource::Traveler, *res, *qty);
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
    effects.extend(resolve_market(farms, &invited, &mut pool, &demand).into_values());

    // Half of whatever Fieldstone survived market transactions is diverted to
    // paving its own delivery routes, before the player ever sees it as a gain
    // or spends it on construction/storage. `pave_fieldstone_routes` (called
    // separately, once the pool's effects are applied) does the actual paving.
    let fieldstone_for_paving = pool.inflow_available(UniformResource::Fieldstone) / 2;
    if fieldstone_for_paving > 0 {
        pool.claim(
            LedgerSource::Paving,
            UniformResource::Fieldstone,
            fieldstone_for_paving,
        );
    }

    // Whatever inflow farms left in the pool is the player's gains, and this
    // month's inflow for Construction and the leftover-distribution pass below.
    let player_gains = pool.gains();

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

    // 4.5. Workshops: staffed workplaces convert inputs (from whatever inflow
    // and storage the claimants above left behind) into outputs that rejoin the
    // pool, so both movements show up in the ledger/prediction and the outputs
    // flow through the same storage contention as any other gain.
    if let Some(workshop) = compute_workshop_effect(constructed, population, &mut pool) {
        effects.push(CityEffect::Workshop(workshop));
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
        &storage_snapshot,
        &storage_free_capacity,
        place::storage_overall_free_capacity(constructed),
        &known_farm_output,
    );

    MonthEffects {
        effects,
        player_gains,
        leftover,
        ledger: pool.ledger,
        fieldstone_for_paving,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::city::ConstructedCity;
    use crate::place::{ParentRestriction, ParticularPlace};
    use crate::population::Individual;
    use crate::resource::{Approximation, Inventory, StorageKind, UniformResource::*};
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
            road_trips: Vec::new(),
            road_paved: Vec::new(),
            roads: None,
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
            public_storage: true,
            accounting: Some(Approximation {
                digits: 2,
                max: 999,
            }),
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
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
    fn ledger_source_order_is_complete() {
        // The tooltip's display order must cover every ledger source exactly
        // once — otherwise a source's movements would silently never appear.
        assert_eq!(LEDGER_SOURCE_ORDER.len(), LedgerSource::ALL.len());
        for source in LedgerSource::ALL {
            assert!(
                LEDGER_SOURCE_ORDER.contains(&source),
                "{source:?} missing from LEDGER_SOURCE_ORDER"
            );
        }
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
            fieldstone_for_paving: 0,
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

    /// A "bin" furniture providing `cap` units of `Bulk` storage.
    fn bin_eorf(cap: f32) -> crate::eorf::EorfInfo {
        use crate::eorf::{EorfInfo, FurnitureOrElement, PlacementStyle, StructureEmbedding};
        use crate::resource::StorageKind;
        EorfInfo {
            name: "bin".to_string(),
            placement_style: PlacementStyle::RoomPlop,
            x_char: None,
            z_char: None,
            embedding: StructureEmbedding {
                tall: 0.0,
                passable: 0.0,
                decorative: 0.0,
                striated: 0.0,
                temporary: 0.0,
            },
            kind: FurnitureOrElement::Furniture(vec![]),
            vantage_evaluated: false,
            storage_capacity: vec![(StorageKind::Bulk, cap)],
            placeable: true,
            slots: vec![],
        }
    }

    /// A city with a `bin`-backed storage room (capacity `cap`, holding
    /// `timber`) and a single full-time carpenter's workshop. Returns the city,
    /// a matching population, and the workshop's id.
    fn city_with_workshop(
        cap: f32,
        timber: u32,
    ) -> (ConstructedCity, Population, crate::place::PlacedPlaceId) {
        use crate::city::Cell;
        use crate::eorf::EorfId;
        use crate::materials::BuildMaterialId;
        use crate::place::{FulfilledPorf, Place, WorkEffect};
        use crate::resource::{ToolKind, UniqueResource};
        use crate::sparse3d::{Facing, Slot, SlotCoord};
        use bevy::math::IVec3;

        let mut cw = ConstructedCity::new(vec![bin_eorf(cap)]);
        cw.places = vec![
            crate::place::Place {
                name: "storage room".to_string(),
                requirements: vec![],
                public_storage: true,
                accounting: None,
                quality_factors: vec![],
                assignable_for: None,
                work: None,
                gate: None,
            },
            Place {
                name: "carpenter's workshop".to_string(),
                requirements: vec![],
                public_storage: false,
                accounting: None,
                quality_factors: vec![],
                assignable_for: None,
                work: Some(WorkEffect::ToolEffect),
                gate: None,
            },
        ];

        // A bin at the origin, backing a storage room holding `timber` Timber.
        let bin_cube = IVec3::new(0, 0, 0);
        cw.contents.set(
            SlotCoord {
                cube: bin_cube,
                slot: Slot::Room,
            },
            Cell {
                id: EorfId(0),
                facing: Facing::default(),
                evaluation: None,
                build_material: BuildMaterialId::default(),
            },
        );
        let mut inv = Inventory::new([(StorageKind::Bulk, cap)]);
        inv.add_uniform(Timber, timber);
        cw.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![FulfilledPorf::Furniture(SlotCoord {
                cube: bin_cube,
                slot: Slot::Room,
            })],
            contents: inv,
            restriction: ParentRestriction::Unrestricted,
        });

        // A workshop cored on a table with carpenter's tools installed.
        let core = IVec3::new(5, 0, 5);
        let workshop_id = cw.placed_places.insert(ParticularPlace {
            place: 1,
            fulfillments: vec![FulfilledPorf::Furniture(SlotCoord {
                cube: core,
                slot: Slot::Room,
            })],
            contents: Inventory::new([]),
            restriction: ParentRestriction::Unrestricted,
        });
        cw.furniture_slots.insert(
            core,
            vec![Some(UniqueResource::Tool(ToolKind::CarpentersTools))],
        );

        let mut pop = population(1);
        pop.individuals[0].work_jobs = vec![(workshop_id, 1.0)];
        (cw, pop, workshop_id)
    }

    fn workshop_effects(cw: &ConstructedCity, pop: &Population) -> MonthEffects {
        compute_month_effects(
            &farms_with(vec![]),
            cw,
            &ProposedCity::new(),
            pop,
            &no_traveler(),
            &MaterialList::default(),
            false,
        )
    }

    #[test]
    fn staffed_workshop_records_conversion_in_ledger() {
        // Storage capacity 100, holding 10 Timber: plenty of room for the output.
        let (cw, pop, _) = city_with_workshop(100.0, 10);
        let effects = workshop_effects(&cw, &pop);

        // A full-time carpenter converts rate(4.0) Timber into Plank, recorded
        // in the ledger under the Workshop source (Timber drawn from storage,
        // Plank deposited via the leftover pass).
        assert_eq!(effects.ledger.net_for(LedgerSource::Workshop, Timber), -4);
        assert_eq!(effects.ledger.net_for(LedgerSource::Workshop, Plank), 4);
        assert_eq!(effects.ledger.storage_draw_for(Timber), 4);
        assert_eq!(effects.storage_delta(Timber), -4);
        assert_eq!(effects.storage_delta(Plank), 4);
        // The Workshop effect resolves after Construction (none here, so last).
        assert!(matches!(
            effects.effects.last(),
            Some(CityEffect::Workshop(_))
        ));
    }

    #[test]
    fn workshop_blocks_on_back_pressure() {
        // Storage is full (capacity 10, all Timber): there's no room for any
        // Plank, so the workshop produces nothing and keeps all its Timber.
        let (cw, pop, _) = city_with_workshop(10.0, 10);
        let effects = workshop_effects(&cw, &pop);

        assert_eq!(effects.ledger.net_for(LedgerSource::Workshop, Timber), 0);
        assert_eq!(effects.ledger.net_for(LedgerSource::Workshop, Plank), 0);
        assert_eq!(effects.storage_delta(Timber), 0);
        assert_eq!(effects.storage_delta(Plank), 0);
    }

    /// Sets `cw`'s understood segments for `idea` to `count`, i.e. drives the
    /// gating idea's progress to an exact fraction.
    fn understand(cw: &mut ConstructedCity, idea: &str, count: u32) {
        let idx = crate::idea::idea_by_name(&cw.ideas, idea).expect("known idea");
        cw.understood[idx] = if count == 0 { 0 } else { (1u64 << count) - 1 };
    }

    /// Gates the workshop the way `places.ron` does, then checks that output
    /// tracks the ramp. `rate() * staffing` is 4, so the ramp is directly
    /// legible in the ledger: 0 at the threshold, 4 at full understanding.
    #[test]
    fn a_gated_workshop_scales_its_output_with_understanding() {
        use crate::place::IdeaGate;

        // (understood segments of Specialization, expected Plank output)
        for (understood_segments, expected) in [(25, 0), (38, 2), (44, 3), (50, 4)] {
            let (mut cw, pop, _) = city_with_workshop(100.0, 10);
            cw.places[1].gate = Some(IdeaGate {
                idea: "Specialization".to_string(),
                unlock_at: 0.5,
                full_at: 1.0,
            });
            understand(&mut cw, "Specialization", understood_segments);

            let effects = workshop_effects(&cw, &pop);
            assert_eq!(
                effects.ledger.net_for(LedgerSource::Workshop, Plank),
                expected,
                "{understood_segments}/50 understood should yield {expected} Plank"
            );
            assert_eq!(
                effects.ledger.net_for(LedgerSource::Workshop, Timber),
                -expected,
                "a workshop should only consume what it actually converts"
            );
        }
    }

    /// Belt-and-braces: `sync_places` won't let a locked workshop exist in the
    /// first place, but if one somehow does, it must sit idle rather than
    /// producing at full rate.
    #[test]
    fn a_locked_workshop_produces_nothing() {
        use crate::place::IdeaGate;

        let (mut cw, pop, _) = city_with_workshop(100.0, 10);
        cw.places[1].gate = Some(IdeaGate {
            idea: "Specialization".to_string(),
            unlock_at: 0.5,
            full_at: 1.0,
        });
        understand(&mut cw, "Specialization", 10); // 20%, well below the gate

        let effects = workshop_effects(&cw, &pop);
        assert_eq!(effects.ledger.net_for(LedgerSource::Workshop, Plank), 0);
        assert_eq!(effects.storage_delta(Timber), 0);
    }

    #[test]
    fn workshop_partially_blocks_when_room_is_tight() {
        // Capacity 12 holding 10 Timber leaves room for only 2 Plank, so the
        // workshop converts 2 (not the full 4) and consumes just 2 Timber.
        let (cw, pop, _) = city_with_workshop(12.0, 10);
        let effects = workshop_effects(&cw, &pop);

        assert_eq!(effects.ledger.net_for(LedgerSource::Workshop, Timber), -2);
        assert_eq!(effects.ledger.net_for(LedgerSource::Workshop, Plank), 2);
        assert_eq!(effects.storage_delta(Timber), -2);
        assert_eq!(effects.storage_delta(Plank), 2);
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

        let mut inv = Inventory::new([(StorageKind::Bulk, 100.0)]);
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
        let mut inv = Inventory::new([(StorageKind::Bulk, 100.0)]);
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

    /// Regression test for a bug where a `Tool` reward's "Invite" checkbox
    /// was enabled purely on demand affordability, ignoring whether there was
    /// any rack room to actually receive the tool: `TravelerVisit::apply`
    /// deposits `Tool` rewards via `place::deposit_tool`, which silently
    /// no-ops without room, so the reward vanished with no warning. Unlike a
    /// `Resource` reward (see `plank_reward_pays_off_pending_furniture_with_no_storage_room_yet`
    /// below), a `Tool` reward has no fallback path into this month's pool, so
    /// `affordable` -- and thus the checkbox -- must account for storage room.
    #[test]
    fn tool_reward_is_unaffordable_without_rack_room() {
        use crate::resource::ToolKind;

        let farms = farms_with(vec![]);
        let cw = ConstructedCity::new(Vec::new()); // no storage at all
        let pending = ProposedCity::new();
        let pop = population(0);

        let mut traveler_state = no_traveler();
        traveler_state.invited = true;
        traveler_state.current_offer = Some(IndividualTraveler {
            config_index: 0,
            demands: vec![], // trivially affordable
            reward: ResolvedReward::Tool(ToolKind::CarpentersTools),
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
        assert!(
            !t.affordable,
            "no rack storage exists to receive the tool, so the checkbox must be disabled"
        );
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

    /// A reconfiguring farm can cover the 10-potato cost from the player's
    /// stored potatoes when this month's market inflow is short, and executing
    /// the effect physically withdraws that portion from storage.
    #[test]
    fn reconfigure_pays_from_stored_potatoes_when_inflow_short() {
        use rand::{rngs::StdRng, SeedableRng};

        // One invited farm, set to re-roll. A reconfiguring farm contributes no
        // potatoes to the pool, so this month's potato inflow is 0.
        let mut farm = mk_farm(0);
        farm.event = FarmEvent::Reconfigure(NewProduction::RandomRegular);
        let mut farms = farms_with(vec![farm]);

        // The player has 20 stored potatoes to draw on.
        let mut inv = Inventory::new([(StorageKind::Bulk, 100.0)]);
        inv.add_uniform(Potato, 20);
        let mut cw = grid_with_storage(inv);
        let pending = ProposedCity::new();
        let pop = population(0); // no eating, so nothing claims potatoes first

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

        let market = effects
            .effects
            .iter()
            .find_map(|e| match e {
                CityEffect::Market { effect, .. } => Some(*effect),
                _ => None,
            })
            .expect("a Market effect should be present");
        assert!(
            matches!(
                market,
                MarketModeEffect::Reconfigure {
                    paid: 10,
                    paid_from_storage: 10,
                    ..
                }
            ),
            "the whole 10-potato cost should be drawn from storage",
        );

        // Executing the effect withdraws those 10 potatoes from storage.
        let mut population = pop;
        let mut rng = StdRng::seed_from_u64(0);
        for effect in effects.all() {
            effect.apply(&mut EffectContext {
                constructed: &mut cw,
                pending: &mut ProposedCity::new(),
                population: &mut population,
                farms: &mut farms,
                idea_state: &mut crate::idea::IdeaState::default(),
                rng: &mut rng,
            });
        }
        assert_eq!(
            crate::place::storage_totals(&cw).get(&Potato).copied(),
            Some(10),
            "20 stored - 10 paid = 10 remaining",
        );
    }

    /// With neither market inflow nor stored potatoes enough to cover the cost,
    /// a reconfigure falls back to an ordinary boost.
    #[test]
    fn reconfigure_falls_back_to_boost_when_unaffordable() {
        let mut farm = mk_farm(0);
        farm.event = FarmEvent::Reconfigure(NewProduction::RandomRegular);
        let farms = farms_with(vec![farm]);

        let mut inv = Inventory::new([(StorageKind::Bulk, 100.0)]);
        inv.add_uniform(Potato, 5); // not enough, even from storage
        let cw = grid_with_storage(inv);
        let pending = ProposedCity::new();
        let pop = population(0);

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

        let market = effects.effects.iter().find_map(|e| match e {
            CityEffect::Market { effect, .. } => Some(*effect),
            _ => None,
        });
        assert!(matches!(market, Some(MarketModeEffect::Boost { .. })));
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
            idea_state: &mut crate::idea::IdeaState::default(),
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
