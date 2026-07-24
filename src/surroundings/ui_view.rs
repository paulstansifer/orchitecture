//! Pure view-models for the Surroundings mode UI ([`super::ui`]): each
//! function here does the non-trivial data work — which farm actions are
//! affordable, what a farm's info panel should show — so it's headless-
//! testable independent of egui, mirroring [`crate::ui_view`]'s split for the
//! shared resource panel.

use crate::city_effect::MonthInputs;
use crate::resource::{ToolKind, UniformResource};

use super::farmstead::{
    farm_breakdown, market_effect, FarmData, FarmEvent, FarmId, FarmProduction, FarmsResource,
    MarketModeEffect, NewProduction,
};

/// One choosable action in the "Farm options" popup: its enabled state
/// (whether it's currently affordable/available) and the resource breakdown
/// that would result from choosing it.
pub struct FarmMenuOption {
    pub event: FarmEvent,
    pub selected: bool,
    pub enabled: bool,
    pub title: &'static str,
    pub lines: Vec<String>,
    pub disabled_note: Option<&'static str>,
}

/// Everything the "Farm options" popup needs to render, for a farm already
/// known to be invited.
pub struct FarmMenuView {
    pub options: Vec<FarmMenuOption>,
}

/// Builds the farm menu view-model: each possible next-month action, its
/// resource breakdown (via the shared `farm_breakdown` compute path), and
/// whether it's currently choosable. `farms` is mutated transiently by
/// `farm_breakdown`/`market_effect` (a hypothetical event/production is
/// applied, then restored) but is unchanged once this returns.
pub fn farm_menu_view(
    farms: &mut FarmsResource,
    inputs: MonthInputs,
    menu_i: FarmId,
) -> FarmMenuView {
    let current_event = farms.farm_event(menu_i);
    let is_specialized = matches!(farms[menu_i].production, FarmProduction::Specialized(_));
    let carpenters_tools_in_storage =
        crate::place::total_tools_of(inputs.constructed, ToolKind::CarpentersTools) >= 1;

    // Breakdowns via the same shared full-month compute path.
    let market_lines = farm_breakdown(farms, menu_i, FarmEvent::Market, None, inputs);
    let change_lines = farm_breakdown(
        farms,
        menu_i,
        FarmEvent::Reconfigure(NewProduction::RandomRegular),
        None,
        inputs,
    );
    let spec_lines = farm_breakdown(
        farms,
        menu_i,
        FarmEvent::Market,
        Some(FarmProduction::Specialized(ToolKind::CarpentersTools)),
        inputs,
    );
    let adopt_lines = farm_breakdown(farms, menu_i, FarmEvent::Adopt, None, inputs);

    let change_event = FarmEvent::Reconfigure(NewProduction::RandomRegular);
    let can_change = matches!(
        market_effect(farms, menu_i, change_event, inputs),
        Some(MarketModeEffect::Reconfigure { paid, .. }) if paid > 0
    );
    // Specializing is also a reconfigure, so it must be affordable too (from
    // the market pool or the player's stored potatoes), not just have a
    // spare tool.
    let specialize_event = FarmEvent::Reconfigure(NewProduction::Tool(ToolKind::CarpentersTools));
    let can_specialize = !is_specialized
        && carpenters_tools_in_storage
        && matches!(
            market_effect(farms, menu_i, specialize_event, inputs),
            Some(MarketModeEffect::Reconfigure { paid, .. }) if paid > 0
        );
    let can_adopt = farms[menu_i].can_adopt();

    let options = vec![
        FarmMenuOption {
            event: FarmEvent::Market,
            selected: current_event == FarmEvent::Market,
            enabled: true,
            title: "Participate in the market",
            lines: market_lines,
            disabled_note: None,
        },
        FarmMenuOption {
            event: change_event,
            selected: current_event == change_event,
            enabled: can_change,
            title: "Select a different secondary resource",
            lines: change_lines,
            disabled_note: None,
        },
        FarmMenuOption {
            event: specialize_event,
            selected: current_event == specialize_event,
            enabled: can_specialize,
            title: "Process nearby timber into planks",
            lines: spec_lines,
            disabled_note: None,
        },
        FarmMenuOption {
            event: FarmEvent::Adopt,
            selected: current_event == FarmEvent::Adopt,
            enabled: can_adopt,
            title: "Adopt a family",
            lines: adopt_lines,
            disabled_note: Some("Total production must be at least 8"),
        },
    ];

    FarmMenuView { options }
}

/// Everything a revealed farm's info panel needs to render, besides the live
/// `invited` checkbox binding (the panel binds that directly to
/// `FarmData::invited` so ticking it mutates the farm in place).
pub struct FarmPanelView {
    pub area: f32,
    pub boost: i32,
    /// This month's predicted market boost, shown only while the farm is
    /// invited to a `Market`-boosting event.
    pub predicted_boost: i32,
    pub potato_stockpile: u32,
    pub inedible_stockpile: u32,
    pub produced_resource: UniformResource,
    pub can_invite: bool,
    pub checkbox_label: &'static str,
}

/// Builds a farm's info-panel view-model. `predicted_boost` is this month's
/// market-boost preview (0 if not applicable); `invite_limit_reached` is
/// whether every market stand slot is already taken by some other farm.
pub fn farm_panel_view(
    farm: &FarmData,
    current_event: FarmEvent,
    predicted_boost: i32,
    invite_limit_reached: bool,
) -> FarmPanelView {
    FarmPanelView {
        area: farm.area,
        boost: farm.boost,
        predicted_boost,
        potato_stockpile: farm.potato_stockpile,
        inedible_stockpile: farm.inedible_stockpile,
        produced_resource: farm.produced_resource(),
        can_invite: farm.invited || !invite_limit_reached,
        checkbox_label: current_event.checkbox_label(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::city::{ConstructedCity, ProposedCity};
    use crate::materials::MaterialList;
    use crate::population::Population;
    use crate::resource::UniformResource::Straw;
    use crate::surroundings::farmstead::FarmProduction;
    use crate::traveler::TravelerState;
    use bevy::math::Vec2;

    fn mk_farm(invited: bool, production: FarmProduction) -> FarmData {
        FarmData {
            seed: Vec2::ZERO,
            polygon: Vec::new(),
            area: 10.0,
            fertility: 1.0,
            production,
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
            road_trips: Vec::new(),
            road_paved: Vec::new(),
            roads: None,
        }
    }

    fn no_traveler() -> TravelerState {
        TravelerState {
            configs: Vec::new(),
            current_offer: None,
            invited: false,
        }
    }

    /// An invited, non-specialized, sufficiently-productive farm should offer
    /// every option except Specialize (no carpenter's tools in storage).
    #[test]
    fn invited_regular_farm_offers_market_and_change_but_not_specialize() {
        let mut farms = farms_with(vec![mk_farm(true, FarmProduction::Regular(Straw))]);
        let constructed = ConstructedCity::new(Vec::new());
        let pending = ProposedCity::new();
        let population = Population::default();
        let traveler_state = no_traveler();
        let material_list = MaterialList::default();
        let inputs = MonthInputs {
            constructed: &constructed,
            pending: &pending,
            population: &population,
            traveler_state: &traveler_state,
            material_list: &material_list,
            sandbox_enabled: false,
        };

        let view = farm_menu_view(&mut farms, inputs, FarmId::new(0));

        assert_eq!(view.options.len(), 4);
        assert!(view.options[0].enabled, "Market is always enabled");
        assert!(view.options[0].selected, "Market is the default event");
        // No carpenter's tools anywhere, so Specialize must stay disabled
        // regardless of affordability.
        assert!(!view.options[2].enabled);
        // Adopt requires production capacity >= 8; area 10 with no boost
        // qualifies.
        assert!(view.options[3].enabled);
    }

    /// A farm already `Specialized` can't specialize again.
    #[test]
    fn already_specialized_farm_cannot_specialize_again() {
        let mut farms = farms_with(vec![mk_farm(
            true,
            FarmProduction::Specialized(ToolKind::CarpentersTools),
        )]);
        let constructed = ConstructedCity::new(Vec::new());
        let pending = ProposedCity::new();
        let population = Population::default();
        let traveler_state = no_traveler();
        let material_list = MaterialList::default();
        let inputs = MonthInputs {
            constructed: &constructed,
            pending: &pending,
            population: &population,
            traveler_state: &traveler_state,
            material_list: &material_list,
            sandbox_enabled: false,
        };

        let view = farm_menu_view(&mut farms, inputs, FarmId::new(0));

        assert!(!view.options[2].enabled);
    }

    /// A farm too small to adopt (area well under `ADOPT_PENALTY`) has Adopt
    /// disabled, with the explanatory note attached.
    #[test]
    fn small_farm_cannot_adopt() {
        let mut farm = mk_farm(true, FarmProduction::Regular(Straw));
        farm.area = 2.0;
        let mut farms = farms_with(vec![farm]);
        let constructed = ConstructedCity::new(Vec::new());
        let pending = ProposedCity::new();
        let population = Population::default();
        let traveler_state = no_traveler();
        let material_list = MaterialList::default();
        let inputs = MonthInputs {
            constructed: &constructed,
            pending: &pending,
            population: &population,
            traveler_state: &traveler_state,
            material_list: &material_list,
            sandbox_enabled: false,
        };

        let view = farm_menu_view(&mut farms, inputs, FarmId::new(0));

        assert!(!view.options[3].enabled);
        assert_eq!(
            view.options[3].disabled_note,
            Some("Total production must be at least 8")
        );
    }

    /// The farm panel view surfaces the checkbox label for the current event
    /// and gates `can_invite` on the market-stand limit.
    #[test]
    fn farm_panel_view_reflects_invite_limit() {
        let farm = mk_farm(false, FarmProduction::Regular(Straw));
        let view = farm_panel_view(&farm, FarmEvent::Market, 4, true);
        assert!(!view.can_invite, "limit reached and not already invited");
        assert_eq!(view.checkbox_label, "Invite");

        let invited_farm = mk_farm(true, FarmProduction::Regular(Straw));
        let view = farm_panel_view(&invited_farm, FarmEvent::Market, 4, true);
        assert!(
            view.can_invite,
            "an already-invited farm keeps its checkbox enabled even at the limit"
        );
    }
}
