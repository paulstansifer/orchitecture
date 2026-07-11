//! Pure view-model for the shared monthly resource panel ([`crate::ui`]):
//! [`month_panel_view`] does all the per-resource arithmetic that used to be
//! interleaved with egui calls, so it's headless-testable independent of any
//! rendering. `shared_ui_system` builds a [`MonthPanelView`] once per frame
//! and only does layout/formatting from there.

use std::collections::HashMap;

use crate::city::{ConstructedCity, ProposedCity};
use crate::city_effect::{compute_month_effects, CityEffect, LedgerSource, MonthEffects};
use crate::materials::MaterialList;
use crate::place::PlacedPlaceId;
use crate::population::Population;
use crate::resource::{Precision, UniformResource, UniqueResource, UniqueResourceKind};
use crate::surroundings::farmstead::{FarmProduction, FarmsResource, MarketModeEffect, NewProduction};
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

/// How a single unique item is expected to change this month.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UniqueChange {
    /// Held now, with no expected change.
    Present,
    /// Expected to arrive this month (rendered in the prediction colour).
    Arriving,
    /// Held now but expected to leave this month (rendered struck-through).
    Departing,
}

/// One individual unique item in the resource panel.
#[derive(Clone, Debug)]
pub struct UniqueItemView {
    pub label: String,
    pub change: UniqueChange,
}

/// The unique items of one kind held at one location, with a summary of the
/// current count and this month's net change.
#[derive(Clone, Debug)]
pub struct UniqueLocationGroup {
    pub location: String,
    /// Items physically present now (`Present` + `Departing`).
    pub present: u32,
    /// Net expected change this month (`+arriving − departing`).
    pub delta: i32,
    pub items: Vec<UniqueItemView>,
}

/// All unique items of one kind, grouped by location, with kind-wide totals.
#[derive(Clone, Debug)]
pub struct UniqueTypeGroup {
    pub kind: UniqueResourceKind,
    pub present: u32,
    pub delta: i32,
    pub locations: Vec<UniqueLocationGroup>,
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
    /// Unique resources, grouped type → location → items, with predicted
    /// arrivals/departures for this month.
    pub unique_groups: Vec<UniqueTypeGroup>,
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
        unique_groups: build_unique_groups(constructed, farms, &effects),
        traveler,
    }
}

/// Builds the unique-resource sections: current holdings grouped by kind then
/// location, overlaid with this month's predicted arrivals (prediction colour)
/// and departures (struck-through), derived from `effects`. Pure.
fn build_unique_groups(
    constructed: &ConstructedCity,
    farms: &FarmsResource,
    effects: &MonthEffects,
) -> Vec<UniqueTypeGroup> {
    // kind -> (location id -> its items)
    let mut by_kind: HashMap<UniqueResourceKind, HashMap<PlacedPlaceId, Vec<UniqueItemView>>> =
        HashMap::new();

    // Current holdings.
    for (id, item) in crate::place::stored_unique_items(constructed) {
        by_kind
            .entry(item.kind())
            .or_default()
            .entry(id)
            .or_default()
            .push(UniqueItemView {
                label: item.describe(),
                change: UniqueChange::Present,
            });
    }

    // Collect this month's predicted unique-item movements. Both arrivals and
    // departures land in / draw from the first place storing that kind, matching
    // `place::deposit_tool` / `consume_tool`.
    let mut arrivals: Vec<(UniqueResourceKind, String)> = Vec::new();
    let mut departures: Vec<(UniqueResourceKind, String)> = Vec::new();
    for effect in effects.all() {
        match effect {
            CityEffect::TravelerVisit(t) if t.affordable && t.invited => {
                if let ResolvedReward::Tool(kind) = &t.reward {
                    arrivals.push((
                        UniqueResourceKind::Tool,
                        UniqueResource::Tool(*kind).describe(),
                    ));
                }
            }
            CityEffect::Market {
                farm_idx,
                effect:
                    MarketModeEffect::Reconfigure {
                        paid,
                        new_production,
                    },
                ..
            } if *paid > 0 => {
                // A specialized farm hands its current tool back.
                if let FarmProduction::Specialized(prev) = farms[*farm_idx].production {
                    arrivals.push((
                        UniqueResourceKind::Tool,
                        UniqueResource::Tool(prev).describe(),
                    ));
                }
                // Specializing consumes a tool.
                if let NewProduction::Tool(kind) = new_production {
                    departures.push((
                        UniqueResourceKind::Tool,
                        UniqueResource::Tool(*kind).describe(),
                    ));
                }
            }
            _ => {}
        }
    }

    for (kind, label) in arrivals {
        if let Some(&id) = crate::place::unique_storage_ids(constructed, kind).first() {
            by_kind
                .entry(kind)
                .or_default()
                .entry(id)
                .or_default()
                .push(UniqueItemView {
                    label,
                    change: UniqueChange::Arriving,
                });
        }
    }

    for (kind, label) in departures {
        let locs = crate::place::unique_storage_ids(constructed, kind);
        let loc_map = by_kind.entry(kind).or_default();
        // Prefer to strike a currently-present matching item, in storage order.
        let struck = locs.iter().find_map(|id| {
            loc_map.get_mut(id).and_then(|items| {
                items
                    .iter_mut()
                    .find(|it| it.label == label && it.change == UniqueChange::Present)
                    .map(|it| it.change = UniqueChange::Departing)
            })
        });
        // Nothing present to strike: a same-month arrival is consumed again, so
        // just cancel one arrival (net zero, shown as nothing).
        if struck.is_none() {
            for id in &locs {
                if let Some(items) = loc_map.get_mut(id) {
                    if let Some(pos) = items
                        .iter()
                        .position(|it| it.label == label && it.change == UniqueChange::Arriving)
                    {
                        items.remove(pos);
                        break;
                    }
                }
            }
        }
    }

    // Emit in a stable order: kinds by `ALL`, locations by placement order (id).
    let mut out = Vec::new();
    for &kind in UniqueResourceKind::ALL {
        let Some(loc_map) = by_kind.get(&kind) else {
            continue;
        };
        let mut loc_ids: Vec<PlacedPlaceId> = loc_map.keys().copied().collect();
        loc_ids.sort();

        let mut locations = Vec::new();
        let mut type_present = 0u32;
        let mut type_delta = 0i32;
        for id in loc_ids {
            let items = &loc_map[&id];
            if items.is_empty() {
                continue;
            }
            let arriving = items
                .iter()
                .filter(|it| it.change == UniqueChange::Arriving)
                .count() as i32;
            let departing = items
                .iter()
                .filter(|it| it.change == UniqueChange::Departing)
                .count() as i32;
            let present = items.len() as u32 - arriving as u32;
            let delta = arriving - departing;
            type_present += present;
            type_delta += delta;
            locations.push(UniqueLocationGroup {
                location: location_label(constructed, id),
                present,
                delta,
                items: items.clone(),
            });
        }
        if !locations.is_empty() {
            out.push(UniqueTypeGroup {
                kind,
                present: type_present,
                delta: type_delta,
                locations,
            });
        }
    }
    out
}

/// A location label for a unique-resource group: the place kind's name plus its
/// grid coordinate, to disambiguate multiple places of the same kind.
fn location_label(constructed: &ConstructedCity, id: PlacedPlaceId) -> String {
    let name = crate::place::place_name(constructed, id);
    let loc = crate::place::place_location(constructed, id);
    format!("{name} ({}, {}, {})", loc.x, loc.y, loc.z)
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

    /// A market holding one tool, plus an invited traveler bringing another,
    /// yields one `Tool` group: present 1, delta +1, with an `Arriving` item.
    #[test]
    fn market_tool_holdings_and_incoming() {
        use crate::place::{FulfilledPorf, ParentRestriction, ParticularPlace, Place};
        use crate::resource::{Inventory, ToolKind, UniqueResource, UniqueResourceKind};
        use crate::traveler::{IndividualTraveler, ResolvedReward};
        use bevy::math::IVec3;

        let mut cw = ConstructedCity::new(Vec::new());
        cw.places = vec![Place {
            name: "market".to_string(),
            requirements: vec![],
            storage: None,
            stores_unique: vec![UniqueResourceKind::Tool],
            quality_factors: vec![],
            assignable_for: None,
        }];
        let mut inv = Inventory::new(0.0);
        inv.add_unique(UniqueResource::Tool(ToolKind::Whipsaw));
        cw.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![FulfilledPorf::Furniture(IVec3::ZERO)],
            contents: inv,
            restriction: ParentRestriction::Unrestricted,
        });

        let traveler_state = TravelerState {
            configs: Vec::new(),
            current_offer: Some(IndividualTraveler {
                config_index: 0,
                demands: Vec::new(),
                reward: ResolvedReward::Tool(ToolKind::Whipsaw),
                path: Vec::new(),
            }),
            invited: true,
        };

        let farms = farms_with(vec![]);
        let pending = crate::city::ProposedCity::new();
        let pop = Population {
            individuals: vec![],
        };
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

        assert_eq!(view.unique_groups.len(), 1);
        let group = &view.unique_groups[0];
        assert_eq!(group.kind, UniqueResourceKind::Tool);
        assert_eq!(group.present, 1);
        assert_eq!(group.delta, 1);
        assert_eq!(group.locations.len(), 1);
        let loc = &group.locations[0];
        assert_eq!(loc.present, 1);
        assert_eq!(loc.delta, 1);
        assert_eq!(
            loc.items
                .iter()
                .filter(|i| i.change == UniqueChange::Arriving)
                .count(),
            1
        );
        assert_eq!(
            loc.items
                .iter()
                .filter(|i| i.change == UniqueChange::Present)
                .count(),
            1
        );
    }
}
