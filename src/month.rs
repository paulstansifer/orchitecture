//! The month-advance sequence: the single source of truth for what happens when
//! the player (or the headless harness) advances the game clock by one month.
//!
//! This is deliberately ECS-free — it takes and mutates the game resources
//! directly and returns a [`MonthOutcome`] describing what happened. Callers are
//! responsible for any ECS follow-up (reflecting completed construction into
//! geometry via [`crate::construction::apply_construction_completion`]) and for
//! turning the outcome into user-facing text or log lines.

use std::collections::HashMap;

use crate::build_ui::{place_resource_totals, remaining_construction_need};
use crate::city::{Cell, ConstructedCity, ProposedCity};
use crate::construction::tick_construction;
use crate::materials::MaterialList;
use crate::place;
use crate::population::{Individual, Population};
use crate::resource::{distribute_incoming_resources, UniformResource};
use crate::sparse3d::SlotCoord;
use crate::surroundings::farmstead::{
    apply_production, compute_production, known_farm_plentifulness, preview_market, run_market,
    update_wanted_resources, FarmsResource, GameClock,
};
use crate::surroundings::map::CIRCLE_REVEAL_RADIUS;
use crate::traveler::{accept_traveler, can_afford_traveler, roll_traveler_offer, TravelerState};

/// Number of potatoes each individual eats per month.
const POTATOES_PER_INDIVIDUAL: u32 = 5;

/// Outcome of one month advance, for callers to report or apply.
pub struct MonthOutcome {
    /// Resources deposited into storage from the market this month.
    pub market_gains: Vec<(UniformResource, u32)>,
    /// Traveler resolution: `None` if none was invited, `Some(true)` if accepted,
    /// `Some(false)` if invited but unaffordable (declined).
    pub traveler_accepted: Option<bool>,
    /// `Some(real_changes)` when construction completed this month; feed this to
    /// [`crate::construction::apply_construction_completion`] if you have an ECS.
    pub construction_changes: Option<Vec<(SlotCoord, Option<Cell>)>>,
}

/// Advances the game by one month, mutating the game resources in place.
///
/// Sequence: run the market (depositing returned tools and growing population),
/// produce next month's stockpiles, clear farm invites, resolve any invited
/// traveler, roll a fresh traveler offer, distribute market gains (construction
/// payment first, then storage, then loss), feed the population, then tick
/// construction (which completes once fully paid, unconditionally in sandbox
/// mode).
#[allow(clippy::too_many_arguments)]
pub fn advance_month(
    clock: &mut GameClock,
    farms: &mut FarmsResource,
    constructed: &mut ConstructedCity,
    pending: &mut ProposedCity,
    population: &mut Population,
    traveler_state: &mut TravelerState,
    material_list: &MaterialList,
    sandbox_enabled: bool,
    rng: &mut impl rand::Rng,
) -> MonthOutcome {
    clock.advance_month();
    farms.ensure_adjacency();

    // Snapshot the market preview before mutating farms; the traveler
    // affordability check below reads its predicted player gains.
    let preview = preview_market(farms);

    let (market_gains, tools_to_return, population_growth) = run_market(farms, rng);
    for tool in &tools_to_return {
        place::deposit_tool(constructed, *tool);
    }
    for _ in 0..population_growth {
        population.individuals.push(Individual::default());
    }

    let plan = compute_production(farms, rng);
    apply_production(farms, &plan);
    update_wanted_resources(farms);
    for farm in &mut farms.farms {
        farm.invited = false;
    }

    // Traveler resolution: deduct demands, deposit the reward, reveal their path.
    let traveler_accepted = if traveler_state.invited {
        traveler_state.current_offer.take().map(|offer| {
            let station_totals = place_resource_totals(constructed);
            if can_afford_traveler(&offer, &station_totals, &preview.player_gains) {
                let new_path = accept_traveler(&offer, constructed);
                farms.traveler_reveals.push(new_path);
                true
            } else {
                false
            }
        })
    } else {
        None
    };
    traveler_state.invited = false;
    roll_traveler_offer(traveler_state, CIRCLE_REVEAL_RADIUS, rng);

    // Resource distribution: construction payment first (bounded by each
    // resource's own construction rate and its remaining need), then storage
    // (bounded by free volume), then loss. Mirrors the
    // compute_market/preview_market/run_market split: this reuses the same
    // pure `distribute_incoming_resources` used by ui.rs's preview.
    let remaining_need: HashMap<_, _> = if pending.num_changes() > 0 && !sandbox_enabled {
        remaining_construction_need(pending, &constructed.eorfs, material_list)
            .into_iter()
            .collect()
    } else {
        HashMap::new()
    };
    let storage_snapshot = place::storage_totals(constructed);
    let storage_free_capacity: HashMap<UniformResource, f32> = market_gains
        .iter()
        .map(|(res, _)| (*res, place::storage_free_capacity(constructed, *res)))
        .collect();
    let known_farm_output = known_farm_plentifulness(farms);

    let flows = distribute_incoming_resources(
        &market_gains,
        &remaining_need,
        &storage_snapshot,
        &storage_free_capacity,
        &known_farm_output,
    );
    for (res, flow) in &flows {
        if flow.applied_to_construction > 0 {
            *pending.resource_progress.entry(*res).or_insert(0) += flow.applied_to_construction;
        }
        if flow.stored > 0 {
            place::deposit_uniform_with_capacity(constructed, *res, flow.stored);
        }
        // flow.lost is simply dropped.
    }

    // Feed the population: each individual eats a fixed number of potatoes.
    for individual in &mut population.individuals {
        individual.fed_this_month = false;
    }
    for individual in &mut population.individuals {
        if place::consume_uniform(
            constructed,
            UniformResource::Potato,
            POTATOES_PER_INDIVIDUAL,
        ) {
            individual.fed_this_month = true;
        }
    }

    // Construction completes once fully paid off (sandbox mode bypasses the
    // resource requirement, as it always has).
    let fully_paid = sandbox_enabled
        || remaining_construction_need(pending, &constructed.eorfs, material_list).is_empty();
    let construction_changes = tick_construction(pending, constructed, fully_paid);

    MonthOutcome {
        market_gains,
        traveler_accepted,
        construction_changes,
    }
}
