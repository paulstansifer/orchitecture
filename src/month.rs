//! The month-advance sequence: the single source of truth for what happens when
//! the player (or the headless harness) advances the game clock by one month.
//!
//! [`advance_month`] is deliberately ECS-free — it takes and mutates the game
//! resources directly and returns a [`MonthOutcome`] describing what happened.
//! Callers are responsible for any ECS follow-up (reflecting completed
//! construction into geometry via
//! [`crate::construction::apply_construction_completion`]) and for turning the
//! outcome into user-facing text or log lines. [`advance_month_system`] is the
//! in-game ECS adapter that does both, driven by [`AdvanceMonthRequested`]
//! messages from the UI.

use bevy::prelude::{Commands, Message, MessageReader, Res, ResMut};

use crate::city::{Cell, CityMut, ConstructedCity, ProposedCity, ViewableWorld};
use crate::city_effect::{compute_month_effects, EffectContext};
use crate::construction::{remaining_construction_need, tick_construction};
use crate::materials::MaterialList;
use crate::place;
use crate::population::Population;
use crate::resource::UniformResource;
use crate::sparse3d::SlotCoord;
use crate::surroundings::farmstead::{
    apply_production, compute_production, reset_farm_events, FarmsResource, GameClock,
};
use crate::surroundings::map::CIRCLE_REVEAL_RADIUS;
use crate::traveler::{roll_traveler_offer, TravelerState};

/// Outcome of one month advance, for callers to report or apply.
pub struct MonthOutcome {
    /// This month's market inflow, before any claims (construction, storage,
    /// etc.) — i.e. what farms delivered to the pool.
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
/// Sequence: compute this month's effects (market participation per invited
/// farm, population feeding, a traveler's visit, construction's material
/// absorption — see [`crate::city_effect`]) against the current state, apply
/// every one of them, then produce next month's stockpiles, clear farm
/// invites, roll a fresh traveler offer, deposit whatever's left of this
/// month's inflow into storage, and tick construction (which completes once
/// fully paid, unconditionally in sandbox mode).
#[allow(clippy::too_many_arguments)]
pub fn advance_month(
    clock: &mut GameClock,
    farms: &mut FarmsResource,
    constructed: &mut ConstructedCity,
    pending: &mut ProposedCity,
    population: &mut Population,
    traveler_state: &mut TravelerState,
    idea_state: &mut crate::idea::IdeaState,
    material_list: &MaterialList,
    sandbox_enabled: bool,
    rng: &mut impl rand::Rng,
) -> MonthOutcome {
    clock.advance_month();
    farms.ensure_adjacency();
    farms.ensure_roads();

    let effects = compute_month_effects(
        farms,
        constructed,
        pending,
        population,
        traveler_state,
        material_list,
        sandbox_enabled,
    );

    {
        // Reborrow (not move) so `rng` is still usable below.
        let rng: &mut dyn rand::RngCore = &mut *rng;
        let mut ctx = EffectContext {
            constructed,
            pending,
            population,
            farms,
            idea_state,
            rng,
        };
        for effect in &effects.effects {
            effect.apply(&mut ctx);
        }
    }

    // Roads develop where traffic flowed this month: every invited farm's
    // delivery route, plus an accepted traveler's route. Must run before the
    // `invited` flags are cleared below.
    let traveler_start = effects
        .traveler_visit()
        .filter(|t| t.invited && t.affordable)
        .and_then(|t| t.path.first().copied());
    farms.record_road_trips(traveler_start);
    // Half of whatever Fieldstone survived market transactions paves its own
    // delivery routes; also must run before `invited` is cleared below.
    let paving_leftover = farms.pave_fieldstone_routes(effects.fieldstone_for_paving);

    // Invited farms' per-cycle events are spent; reset for next month.
    reset_farm_events(farms);

    let plan = compute_production(farms, rng);
    apply_production(farms, &plan);
    for farm in &mut farms.farms {
        farm.invited = false;
    }

    let traveler_accepted = effects
        .traveler_visit()
        .filter(|t| t.invited)
        .map(|t| t.affordable);
    traveler_state.invited = false;
    if let Some(roads) = farms.roads.as_ref() {
        roll_traveler_offer(
            traveler_state,
            CIRCLE_REVEAL_RADIUS,
            roads,
            &constructed.ideas,
            rng,
        );
    }

    // Whatever wasn't claimed by any effect above is stored (if capacity
    // allows) or was already accounted as lost.
    for (res, flow) in &effects.leftover {
        if flow.stored > 0 {
            place::deposit_uniform_with_capacity(constructed, *res, flow.stored);
        }
    }
    // Fieldstone earmarked for paving but not actually spent (every candidate
    // route was already fully paved) still goes to the player, same as any
    // other unclaimed inflow.
    if paving_leftover > 0 {
        place::deposit_uniform_with_capacity(
            constructed,
            UniformResource::Fieldstone,
            paving_leftover,
        );
    }

    // Construction completes once fully paid off (sandbox mode bypasses the
    // resource requirement, as it always has).
    let fully_paid = sandbox_enabled
        || remaining_construction_need(pending, &constructed.eorfs, material_list).is_empty();
    let construction_changes = tick_construction(pending, constructed, fully_paid, material_list);

    let mut market_gains = effects.player_gains;
    if paving_leftover > 0 {
        match market_gains
            .iter_mut()
            .find(|(res, _)| *res == UniformResource::Fieldstone)
        {
            Some((_, qty)) => *qty += paving_leftover,
            None => {
                market_gains.push((UniformResource::Fieldstone, paving_leftover));
                market_gains.sort_by_key(|&(res, _)| res);
            }
        }
    }

    MonthOutcome {
        market_gains,
        traveler_accepted,
        construction_changes,
    }
}

/// Sent by the UI when the player clicks "Advance Month".
#[derive(Message)]
pub struct AdvanceMonthRequested;

/// ECS adapter for [`advance_month`]: consumes [`AdvanceMonthRequested`]
/// (advancing at most one month per frame, however many messages arrived) and
/// performs the construction follow-up that `advance_month` leaves to its
/// caller.
pub fn advance_month_system(
    mut requests: MessageReader<AdvanceMonthRequested>,
    mut commands: Commands,
    mut clock: ResMut<GameClock>,
    mut farms: ResMut<FarmsResource>,
    world: CityMut,
    mut viewable: ResMut<ViewableWorld>,
    structure_list: Res<crate::eorf::EorfList>,
    mut population: ResMut<Population>,
    mut traveler_state: ResMut<TravelerState>,
    mut idea_state: ResMut<crate::idea::IdeaState>,
    material_list: Res<MaterialList>,
    sandbox: Res<crate::game_mode::SandboxMode>,
) {
    if requests.read().count() == 0 {
        return;
    }
    let CityMut {
        mut constructed,
        mut pending,
        mut assembled,
    } = world;

    let mut rng = rand::rng();
    let outcome = advance_month(
        &mut clock,
        &mut farms,
        &mut constructed,
        &mut pending,
        &mut population,
        &mut traveler_state,
        &mut idea_state,
        &material_list,
        sandbox.enabled,
        &mut rng,
    );
    if let Some(real_changes) = outcome.construction_changes {
        crate::construction::apply_construction_completion(
            &mut commands,
            &mut assembled,
            &mut viewable,
            &structure_list,
            real_changes,
        );
    }
}
