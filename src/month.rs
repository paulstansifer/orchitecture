//! The month-advance sequence: the single source of truth for what happens when
//! the player (or the headless harness) advances the game clock by one month.
//!
//! This is deliberately ECS-free — it takes and mutates the game resources
//! directly and returns a [`MonthOutcome`] describing what happened. Callers are
//! responsible for any ECS follow-up (reflecting completed construction into
//! geometry via [`crate::construction::apply_construction_completion`]) and for
//! turning the outcome into user-facing text or log lines.

use crate::build_ui::{construction_cost, place_resource_totals};
use crate::city::{Cell, ConstructedCity, ProposedCity};
use crate::construction::tick_construction;
use crate::materials::MaterialList;
use crate::place;
use crate::population::{Individual, Population};
use crate::resource::UniformResource;
use crate::sparse3d::SlotCoord;
use crate::surroundings::farmstead::{
    apply_production, compute_production, preview_market, run_market, update_wanted_resources,
    FarmsResource, GameClock,
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
/// traveler, roll a fresh traveler offer, deposit market gains into storage, feed
/// the population, then tick construction (deducting its cost on completion when
/// not in sandbox mode).
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

    if !market_gains.is_empty() {
        if let Some(&id) = place::storage_ids(constructed).first() {
            for (res, qty) in &market_gains {
                constructed.placed_places[id]
                    .contents
                    .add_uniform(*res, *qty as u16);
            }
        }
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

    // Construction: compute cost up front (it depends on the still-pending
    // proposals), tick progress, and deduct the cost if it completed.
    let cost = if pending.num_changes() > 0 && !sandbox_enabled {
        construction_cost(&pending.proposed_changes, &constructed.eorfs, material_list)
    } else {
        vec![]
    };
    let construction_changes =
        tick_construction(pending, constructed, population.individuals.len());
    if construction_changes.is_some() && !sandbox_enabled {
        for (res, qty) in &cost {
            place::consume_uniform(constructed, *res, *qty);
        }
    }

    MonthOutcome {
        market_gains,
        traveler_accepted,
        construction_changes,
    }
}
