//! Pure view-models for the Surroundings mode UI ([`super::ui`]): each
//! function here does the non-trivial data work — what a farm's info panel
//! should show, which of its actions are available — so it's headless-
//! testable independent of egui, mirroring [`crate::ui_view`]'s split for the
//! shared resource panel.

use crate::city::ConstructedCity;
use crate::city_effect::MonthEffects;
use crate::resource::{ToolKind, UniformResource};

use super::attendance::Absence;
use super::farmstead::{specialize_blocker, unspecialize_blocker, FarmId, FarmsResource};

/// A button's availability: `blocker` explains why it's disabled.
pub struct ActionView {
    pub blocker: Option<&'static str>,
}

impl ActionView {
    pub fn enabled(&self) -> bool {
        self.blocker.is_none()
    }
}

/// Everything a visible farm's info panel needs to render.
pub struct FarmPanelView {
    pub coming: bool,
    pub potatoes: u32,
    pub produced: (UniformResource, u32),
    pub absences: Vec<Absence>,
    /// The guest-room quality the farm needs, if it's far enough out to need one.
    pub required_quality: Option<f32>,
    pub tool: Option<ToolKind>,
    /// Giving the farm carpenter's tools (shown while it has no tool).
    pub specialize: ActionView,
    /// Taking its tool back (shown while it has one).
    pub unspecialize: ActionView,
    /// Why it isn't coming, one line each (empty if it is): shown in the
    /// panel's collapsed "will not attend" section.
    pub reasons: Vec<String>,
}

/// Builds farm `id`'s info-panel view-model from this month's `effects` (its
/// attendance and production). `None` for a farm the player can't see.
pub fn farm_panel_view(
    fr: &FarmsResource,
    cw: &ConstructedCity,
    effects: &MonthEffects,
    id: FarmId,
) -> Option<FarmPanelView> {
    let attendance = effects.attendance.get(id)?;
    let farm = &fr[id];
    let coming = attendance.coming();
    let required_quality = attendance.needs_room.then_some(farm.required_quality);

    let mut reasons: Vec<String> = attendance
        .absences
        .iter()
        .map(|a| a.label().to_string())
        .collect();
    if let (false, Some(q)) = (coming, required_quality) {
        reasons.push(format!("HUMANIZE: Needs a room of quality {q:.2}"));
    }

    let tool = ToolKind::CarpentersTools;
    Some(FarmPanelView {
        coming,
        potatoes: effects.production.potatoes[id.index()],
        produced: (
            farm.produced_resource(),
            effects.production.inedible[id.index()],
        ),
        absences: attendance.absences.clone(),
        required_quality,
        tool: farm.tool,
        specialize: ActionView {
            blocker: specialize_blocker(fr, cw, id, tool, coming),
        },
        unspecialize: ActionView {
            blocker: unspecialize_blocker(fr, cw, id),
        },
        reasons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::city::ProposedCity;
    use crate::city_effect::compute_month_effects;
    use crate::materials::MaterialList;
    use crate::population::Population;
    use crate::resource::UniformResource::Straw;
    use crate::surroundings::farmstead::tests::{farms_with, mk_farm};
    use crate::surroundings::farmstead::FarmData;
    use crate::surroundings::attendance::MarketVenues;
    use crate::traveler::TravelerState;
    use bevy::math::Vec2;

    /// A single unit-square farm right next to the city.
    fn one_farm() -> FarmsResource {
        let farm = FarmData {
            seed: Vec2::new(0.5, 0.5),
            polygon: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(0.0, 1.0),
            ],
            ..mk_farm(Straw, 10.0)
        };
        let mut fr = farms_with(vec![farm], vec![vec![]]);
        fr.ensure_roads();
        fr
    }

    fn effects(fr: &FarmsResource, cw: &ConstructedCity, stands: usize) -> MonthEffects {
        compute_month_effects(
            fr,
            cw,
            &ProposedCity::new(),
            &Population::default(),
            &TravelerState {
                configs: Vec::new(),
                current_offer: None,
                invited: false,
            },
            &MarketVenues::new(stands, vec![]),
            &MaterialList::default(),
            false,
        )
    }

    #[test]
    fn a_coming_farm_shows_its_production() {
        let fr = one_farm();
        let cw = ConstructedCity::new(Vec::new());
        let view = farm_panel_view(
            &fr,
            &cw,
            &effects(&fr, &cw, 1),
            FarmId::new(0),
        )
        .expect("the farm is visible");
        assert!(view.coming);
        assert_eq!(view.potatoes, fr.farms[0].production_capacity());
        assert_eq!(view.produced, (Straw, fr.farms[0].production_capacity()));
        assert_eq!(view.required_quality, None, "right next to the city");
        // No tool in storage (and no potatoes), so it can't be given one.
        assert!(!view.specialize.enabled());
        assert!(!view.unspecialize.enabled(), "it has no tool to take back");
    }

    #[test]
    fn a_farm_without_a_stand_explains_why_and_cant_be_given_a_tool() {
        let fr = one_farm();
        let cw = ConstructedCity::new(Vec::new());
        let view = farm_panel_view(
            &fr,
            &cw,
            &effects(&fr, &cw, 0),
            FarmId::new(0),
        )
        .unwrap();
        assert!(!view.coming);
        assert_eq!(view.absences, vec![Absence::NoStandsRemain]);
        assert!(view.reasons.iter().any(|l| l.contains("stands")));
        assert_eq!(
            view.specialize.blocker,
            Some("HUMANIZE: Only farms coming to market can be given a tool")
        );
    }
}
