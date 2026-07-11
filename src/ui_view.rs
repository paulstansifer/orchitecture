//! Pure view-model for the shared monthly resource panel ([`crate::ui`]):
//! [`month_panel_view`] does all the per-resource arithmetic that used to be
//! interleaved with egui calls, so it's headless-testable independent of any
//! rendering. `shared_ui_system` builds a [`MonthPanelView`] once per frame
//! and only does layout/formatting from there.

use std::collections::HashMap;

use crate::city::{ConstructedCity, ProposedCity};
use crate::city_effect::{compute_month_effects, LedgerSource};
use crate::materials::MaterialList;
use crate::population::Population;
use crate::resource::{Precision, UniformResource};
use crate::surroundings::farmstead::FarmsResource;
use crate::traveler::{ResolvedReward, TravelerState};

/// Whether "Advance Month" is offered, and if so, whether it's currently
/// blocked by unpaid construction cost.
pub enum AdvanceState {
    /// There's a project, invited farms, or an invited traveler this month.
    Active { blocked: bool },
    /// Nothing is pending; the UI should ask "Wait anyways?" before offering
    /// to advance.
    NothingPending,
}

/// One row of the resource grid: current storage, this month's projected
/// change, and remaining construction need.
pub struct ResourceRow {
    pub resource: UniformResource,
    pub current: u32,
    pub precision: Precision,
    /// Net change to storage this month (positive = grows, negative = shrinks).
    pub storage_delta: i64,
    /// Units lost to a full store this month.
    pub lost: u32,
    /// Remaining construction need for this resource (0 if none, or in sandbox).
    pub need: u32,
    /// How much of `need` is being paid off this month.
    pub applied: u32,
    /// One entry per contributing source this month, for the hover tooltip.
    pub sources: Vec<(LedgerSource, i32)>,
}

/// This month's traveler offer, if any, and whether it's currently affordable.
pub struct TravelerOfferView {
    pub demands: Vec<(UniformResource, u16)>,
    pub reward_desc: String,
    pub affordable: bool,
}

/// Everything the shared resource panel needs to render, computed once per
/// frame from the game resources.
pub struct MonthPanelView {
    pub month: u32,
    pub advance: AdvanceState,
    pub has_project: bool,
    pub construction_progress: Option<f32>,
    pub market_stand_count: usize,
    pub invited_count: usize,
    pub has_storage: bool,
    pub rows: Vec<ResourceRow>,
    pub tool_count: u32,
    pub traveler: Option<TravelerOfferView>,
}

fn reward_desc(reward: &ResolvedReward) -> String {
    match reward {
        ResolvedReward::Tool(kind) => format!("1 {}", kind.label()),
        ResolvedReward::Resource(res, qty) => format!("{} {}", qty, res.label()),
    }
}

/// Builds the shared resource panel's view model. Pure: takes everything by
/// shared reference and mutates nothing. `month` is the 1-indexed month
/// number to display (i.e. `clock.month() + 1`).
#[allow(clippy::too_many_arguments)]
pub fn month_panel_view(
    month: u32,
    farms: &FarmsResource,
    constructed: &ConstructedCity,
    pending: &ProposedCity,
    population: &Population,
    traveler_state: &TravelerState,
    material_list: &MaterialList,
    sandbox_enabled: bool,
) -> MonthPanelView {
    // This month's effects (market participation, feeding, a traveler's
    // visit, construction absorption) — computed once and shared by the
    // resource preview, the per-resource tooltip, and the traveler
    // checkbox's affordability. Same computation `advance_month` uses to
    // actually apply things, just not mutated here.
    let effects = compute_month_effects(
        farms,
        constructed,
        pending,
        population,
        traveler_state,
        material_list,
        sandbox_enabled,
    );
    let station_totals = crate::place::place_resource_totals(constructed);

    // Remaining construction need (non-sandbox only).
    let remaining_need: Vec<(UniformResource, u32)> = if pending.num_changes() > 0
        && !sandbox_enabled
    {
        crate::construction::remaining_construction_need(pending, &constructed.eorfs, material_list)
    } else {
        vec![]
    };
    let remaining_need_map: HashMap<UniformResource, u32> =
        remaining_need.iter().copied().collect();

    let has_storage = !crate::place::storage_ids(constructed).is_empty();
    let blocked_construction = crate::construction::construction_blocked(&remaining_need);

    let mut resources: Vec<UniformResource> = station_totals.iter().map(|(r, _, _)| *r).collect();
    for (r, _) in &effects.player_gains {
        if !resources.contains(r) {
            resources.push(*r);
        }
    }
    for (r, _) in &remaining_need {
        if !resources.contains(r) {
            resources.push(*r);
        }
    }
    resources.sort();

    let rows = resources
        .into_iter()
        .map(|res| {
            let (current, precision) = station_totals
                .iter()
                .find(|(r, _, _)| *r == res)
                .map(|(_, q, p)| (*q, *p))
                .unwrap_or((0, Precision::Exact));
            let need = remaining_need_map.get(&res).copied().unwrap_or(0);
            let lost = effects.leftover.get(&res).map(|f| f.lost).unwrap_or(0);
            let storage_delta = effects.storage_delta(res);
            // Construction consumes (negative net); flip its sign for the
            // "applied this month" figure.
            let applied = (-effects.ledger.net_for(LedgerSource::Construction, res)).max(0) as u32;
            let sources = effects.ledger.sources_touching(res).collect();
            ResourceRow {
                resource: res,
                current,
                precision,
                storage_delta,
                lost,
                need,
                applied,
                sources,
            }
        })
        .collect();

    let has_project = pending.num_changes() > 0;
    let has_farms_invited = farms.invited_count() > 0;
    let advance = if has_project || has_farms_invited || traveler_state.invited {
        AdvanceState::Active {
            blocked: has_project && blocked_construction,
        }
    } else {
        AdvanceState::NothingPending
    };

    let traveler = traveler_state
        .current_offer
        .as_ref()
        .map(|offer| TravelerOfferView {
            demands: offer.demands.clone(),
            reward_desc: reward_desc(&offer.reward),
            affordable: effects.traveler_affordable(),
        });

    MonthPanelView {
        month,
        advance,
        has_project,
        construction_progress: crate::construction::construction_progress_fraction(
            pending,
            &constructed.eorfs,
            material_list,
        ),
        market_stand_count: crate::place::market_stand_count(constructed),
        invited_count: farms.invited_count(),
        has_storage,
        rows,
        tool_count: crate::place::total_tool_count(constructed),
        traveler,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::city::ConstructedCity;
    use crate::materials::MaterialList;
    use crate::population::Individual;
    use crate::resource::UniformResource::*;
    use crate::surroundings::farmstead::{FarmData, FarmEvent, FarmsResource};
    use bevy::math::Vec2;

    fn mk_farm(invited: bool) -> FarmData {
        FarmData {
            seed: Vec2::ZERO,
            polygon: Vec::new(),
            area: 5.0,
            fertility: 1.0,
            production: crate::surroundings::farmstead::FarmProduction::Regular(Straw),
            wanted_resource: Timber,
            want_max: 5,
            potato_stockpile: 10,
            inedible_stockpile: 4,
            boost: 0,
            invited,
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

    fn no_traveler() -> TravelerState {
        TravelerState {
            configs: Vec::new(),
            current_offer: None,
            invited: false,
        }
    }

    /// With no project, no invited farms, and no traveler, the panel should
    /// ask "Wait?" rather than offer to advance directly.
    #[test]
    fn nothing_pending_means_wait() {
        let farms = farms_with(vec![mk_farm(false)]);
        let cw = ConstructedCity::new(Vec::new());
        let pending = crate::city::ProposedCity::new();
        let pop = Population {
            individuals: vec![],
        };
        let traveler_state = no_traveler();
        let material_list = MaterialList::default();

        let view = month_panel_view(
            1,
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        assert!(matches!(view.advance, AdvanceState::NothingPending));
        assert!(view.rows.is_empty());
    }

    /// An invited farm makes the panel active and reports the market gain in
    /// the resource row for the produced resource.
    #[test]
    fn invited_farm_reports_market_row() {
        let farms = farms_with(vec![mk_farm(true)]);
        let cw = ConstructedCity::new(Vec::new());
        let pending = crate::city::ProposedCity::new();
        let pop = Population {
            individuals: vec![Individual::default()],
        };
        let traveler_state = no_traveler();
        let material_list = MaterialList::default();

        let view = month_panel_view(
            3,
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        assert_eq!(view.month, 3);
        assert!(matches!(
            view.advance,
            AdvanceState::Active { blocked: false }
        ));
        assert_eq!(view.invited_count, 1);
        // No storage places exist, so nothing is retained; the row for the
        // farm's wanted/produced resources should still show up.
        assert!(view.rows.iter().any(|r| r.resource == Straw));
    }
}
