//! Pure view-model for the shared monthly resource panel ([`crate::ui`]):
//! [`month_panel_view`] does all the per-resource arithmetic that used to be
//! interleaved with egui calls, so it's headless-testable independent of any
//! rendering. `shared_ui_system` builds a [`MonthPanelView`] once per frame
//! and only does layout/formatting from there.

use std::collections::HashMap;

use crate::city::{ConstructedCity, ProposedCity};
use crate::city_effect::{CityEffect, LedgerSource, MonthEffects};
use crate::materials::MaterialList;
use crate::resource::{Precision, RackContents, UniformResource};
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
    /// Remaining construction need for this resource (0 if none, or in
    /// sandbox) — for `Potato`, this instead holds this month's total food
    /// need (population size × ration).
    pub need: u32,
    /// How much of `need` is being paid off this month — for `Potato`, how
    /// many of the needed potatoes will actually be granted.
    pub applied: u32,
    /// One entry per contributing source this month, for the hover tooltip.
    pub sources: Vec<(LedgerSource, i32)>,
    /// Total capacity from bins dedicated to this resource specifically, if
    /// any — shown alongside `current` as "current/capacity".
    pub dedicated_capacity: Option<u32>,
}

/// The always-shown row measuring free *uncommitted* (shared, undedicated)
/// storage capacity — room not earmarked for any particular resource.
pub struct UncommittedStorageRow {
    /// Free shared capacity right now.
    pub available: u32,
    /// Projected change to `available` this month (negative = this month's
    /// leftover inflow will consume some of it).
    pub delta: i64,
}

/// A category of `UniqueResource` shown in its own storage row. `Tools` and
/// `Rugs` are rack-backed (per-cube dedicated); `Books` are bookcase-backed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UniqueCategory {
    Tools,
    Rugs,
    Books,
}

/// A row for a `UniqueResource` category held in storage — shown when there's
/// any capacity for it, or the player will receive one this month (see
/// `month_panel_view`).
pub struct UniqueResourceRow {
    pub category: UniqueCategory,
    pub current: u32,
    /// Total capacity for this category across all storage, if any.
    pub capacity: Option<u32>,
}

/// This month's traveler offer, if any, and whether it's currently affordable.
pub struct TravelerOfferView {
    pub demands: Vec<(UniformResource, u32)>,
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
    pub uncommitted_storage: UncommittedStorageRow,
    pub rack_rows: Vec<UniqueResourceRow>,
    pub traveler: Option<TravelerOfferView>,
}

fn reward_desc(reward: &ResolvedReward, ideas: &[crate::idea::Idea]) -> String {
    match reward {
        ResolvedReward::Tool(kind) => format!("1 {}", kind.label()),
        ResolvedReward::Resource(res, qty) => format!("{} {}", qty, res.label()),
        // What the book teaches matters more than its title, since a book on
        // something you have no prerequisites for is still worth taking (the
        // segments keep, even if you can't use them yet).
        ResolvedReward::Book {
            idea,
            segments,
            title,
        } => format!(
            "\"{title}\" -- {} segments of {}",
            segments.count_ones(),
            ideas[*idea].name
        ),
    }
}

/// Builds the shared resource panel's view model. Pure: takes everything by
/// shared reference and mutates nothing. `month` is the 1-indexed month
/// number to display (i.e. `clock.month() + 1`).
#[allow(clippy::too_many_arguments)]
pub fn month_panel_view(
    month: u32,
    // This month's effects (market participation, feeding, a traveler's visit,
    // construction absorption), computed once per frame by
    // `crate::ui::update_economy_cache` and shared by every consumer. Same
    // computation `advance_month` uses to actually apply things.
    effects: &MonthEffects,
    farms: &FarmsResource,
    constructed: &ConstructedCity,
    pending: &ProposedCity,
    traveler_state: &TravelerState,
    material_list: &MaterialList,
    sandbox_enabled: bool,
) -> MonthPanelView {
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
    let dedicated_ceilings = crate::place::storage_dedicated_ceilings(constructed);

    let eat = effects.effects.iter().find_map(|e| match e {
        CityEffect::Eat(eat) => Some(eat),
        _ => None,
    });

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
    // Anything this month's effects move in or out of storage (e.g. a
    // workshop's freshly produced output, not yet held) gets a row so its
    // projected delta is visible.
    for &res in UniformResource::ALL {
        if !resources.contains(&res) && effects.storage_delta(res) != 0 {
            resources.push(res);
        }
    }
    // Potatoes are the population's food, so their row (and its need/applied
    // as this month's food need/granted) is always shown.
    if !resources.contains(&UniformResource::Potato) {
        resources.push(UniformResource::Potato);
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
            let (need, applied) = if res == UniformResource::Potato {
                let eat = eat.expect("Eat effect is always present");
                (eat.desired_potato, eat.granted_potato)
            } else {
                let need = remaining_need_map.get(&res).copied().unwrap_or(0);
                // Construction consumes (negative net); flip its sign for the
                // "applied this month" figure.
                let applied =
                    (-effects.ledger.net_for(LedgerSource::Construction, res)).max(0) as u32;
                (need, applied)
            };
            let lost = effects.leftover.get(&res).map(|f| f.lost).unwrap_or(0);
            let storage_delta = effects.storage_delta(res);
            let sources = effects.ledger.sources_touching(res).collect();
            let dedicated_capacity = dedicated_ceilings.get(&res).map(|&c| c.round() as u32);
            ResourceRow {
                resource: res,
                current,
                precision,
                storage_delta,
                lost,
                need,
                applied,
                sources,
                dedicated_capacity,
            }
        })
        .collect();

    // Uncommitted (shared, undedicated) storage capacity: how much is free
    // right now, and how much this month's projected leftover inflow will
    // consume, mirroring `ResourceRow::storage_delta`'s "preview" math.
    let shared_ceiling = crate::place::storage_shared_ceiling(constructed);
    let storage_totals_now = crate::place::storage_totals(constructed);
    let uncommitted_now = crate::place::uncommitted_free_capacity(
        shared_ceiling,
        &dedicated_ceilings,
        &storage_totals_now,
    );
    let mut storage_totals_after = storage_totals_now.clone();
    for &res in UniformResource::ALL {
        let delta = effects.storage_delta(res);
        let entry = storage_totals_after.entry(res).or_insert(0);
        *entry = (*entry as i64 + delta).max(0) as u32;
    }
    let uncommitted_after = crate::place::uncommitted_free_capacity(
        shared_ceiling,
        &dedicated_ceilings,
        &storage_totals_after,
    );
    let uncommitted_storage = UncommittedStorageRow {
        available: uncommitted_now.round() as u32,
        delta: uncommitted_after.round() as i64 - uncommitted_now.round() as i64,
    };

    // Tools/Rugs/Books rows: shown when the player has any storage capacity
    // for that category, or will receive one this month. Tools and Rugs are
    // rack-backed; Books are bookcase-backed.
    let tools_incoming = effects.tools_incoming(farms);
    let rack_rows: Vec<UniqueResourceRow> = [
        UniqueCategory::Tools,
        UniqueCategory::Rugs,
        UniqueCategory::Books,
    ]
    .into_iter()
    .filter_map(|category| {
        // The nominal ceiling dedicated to this category -- used only to
        // decide whether the row appears at all (e.g. once a rack has been
        // built), since it stays stable regardless of how full the place
        // currently is.
        let ceiling = match category {
            UniqueCategory::Tools => crate::place::rack_capacity(constructed, RackContents::Tools),
            UniqueCategory::Rugs => crate::place::rack_capacity(constructed, RackContents::Rugs),
            UniqueCategory::Books => crate::place::book_capacity(constructed),
        };
        let incoming = category == UniqueCategory::Tools && tools_incoming;
        if ceiling <= 0.0 && !incoming {
            return None;
        }
        let current = match category {
            UniqueCategory::Tools => crate::place::total_tool_count(constructed),
            UniqueCategory::Rugs => crate::place::total_rug_count(constructed),
            UniqueCategory::Books => crate::place::total_book_count(constructed),
        };
        // The capacity actually shown is bounded by free room right now, not
        // just the nominal ceiling: a rack sharing its place's volume with
        // Bulk goods (e.g. the starting wagon) can have less real headroom
        // than its dedicated ceiling once that shared volume fills up.
        // Regression: the ceiling alone read as "20 tools free" even when
        // the wagon's Bulk contents had already used up the shared volume,
        // leaving no actual room for a tool.
        let free = match category {
            UniqueCategory::Tools => {
                crate::place::rack_free_capacity(constructed, RackContents::Tools)
            }
            UniqueCategory::Rugs => {
                crate::place::rack_free_capacity(constructed, RackContents::Rugs)
            }
            UniqueCategory::Books => ceiling - current as f32,
        };
        let capacity = current as f32 + free;
        Some(UniqueResourceRow {
            category,
            current,
            capacity: (ceiling > 0.0).then_some(capacity.round() as u32),
        })
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
            reward_desc: reward_desc(&offer.reward, &constructed.ideas),
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
        uncommitted_storage,
        rack_rows,
        traveler,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::city::ConstructedCity;
    use crate::city_effect::compute_month_effects;
    use crate::materials::MaterialList;
    use crate::population::{Individual, Population};
    use crate::resource::UniformResource::*;
    use crate::surroundings::farmstead::{FarmData, FarmEvent, FarmsResource};
    use bevy::math::Vec2;

    /// Test helper: run the month pipeline, then build the panel view from it —
    /// mirroring how `update_economy_cache` + `shared_ui_system` split the work
    /// in the real app.
    #[allow(clippy::too_many_arguments)]
    fn panel_view(
        month: u32,
        farms: &FarmsResource,
        constructed: &ConstructedCity,
        pending: &ProposedCity,
        population: &Population,
        traveler_state: &TravelerState,
        material_list: &MaterialList,
        sandbox_enabled: bool,
    ) -> MonthPanelView {
        let effects = compute_month_effects(
            farms,
            constructed,
            pending,
            population,
            traveler_state,
            material_list,
            sandbox_enabled,
        );
        month_panel_view(
            month,
            &effects,
            farms,
            constructed,
            pending,
            traveler_state,
            material_list,
            sandbox_enabled,
        )
    }

    fn mk_farm(invited: bool) -> FarmData {
        FarmData {
            seed: Vec2::ZERO,
            polygon: Vec::new(),
            area: 5.0,
            fertility: 1.0,
            production: crate::surroundings::farmstead::FarmProduction::Regular(Straw),
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

        let view = panel_view(
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
        // The potato row is always shown, even with nothing else pending.
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.rows[0].resource, UniformResource::Potato);
        // No racks exist and nothing is incoming, so no Tools/Rugs/Books rows.
        assert!(view.rack_rows.is_empty());
    }

    /// A rack gives the Tools row a capacity, even with zero tools currently
    /// held -- mirroring how a dedicated bin shows up for a `UniformResource`.
    #[test]
    fn rack_capacity_alone_makes_the_tools_row_appear() {
        use crate::place::{
            FulfilledPorf, ParentRestriction, ParticularPlace, Place, PlaceReq, Porf,
        };
        use crate::resource::Inventory;
        use bevy::math::IVec3;

        use crate::eorf::{EorfInfo, FurnitureOrElement, PlacementStyle, StructureEmbedding};
        use crate::materials::BuildMaterialId;
        use crate::resource::StorageKind;
        use crate::sparse3d::{Facing, Slot, SlotCoord};

        let rack = EorfInfo {
            name: "rack".to_string(),
            placement_style: PlacementStyle::RoomPlop,
            x_char: None,
            z_char: None,
            embedding: StructureEmbedding {
                tall: 0.0,
                passable: 0.0,
                decorative: 0.0,
                striated: 0.0,
                temporary: 1.0,
            },
            kind: FurnitureOrElement::Furniture(vec![]),
            vantage_evaluated: false,
            storage_capacity: vec![(StorageKind::Rack, 10.0)],
            placeable: true,
            slots: vec![],
        };
        let mut cw = ConstructedCity::new(vec![rack]);
        let rack_id = cw.find_structure_by_name("rack").unwrap();
        cw.contents.set(
            SlotCoord {
                cube: IVec3::ZERO,
                slot: Slot::Room,
            },
            crate::city::Cell {
                id: rack_id,
                facing: Facing::default(),
                evaluation: None,
                build_material: BuildMaterialId::default(),
            },
        );
        cw.places = vec![Place {
            name: "shelving".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("rack".to_string()),
                min: 1,
                max: None,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            public_storage: true,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        }];
        cw.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![FulfilledPorf::Furniture(SlotCoord {
                cube: IVec3::ZERO,
                slot: Slot::Room,
            })],
            contents: Inventory::new([(StorageKind::Rack, 10.0)]),
            restriction: ParentRestriction::Unrestricted,
        });

        let farms = farms_with(vec![]);
        let pending = crate::city::ProposedCity::new();
        let pop = Population {
            individuals: vec![],
        };
        let traveler_state = no_traveler();
        let material_list = MaterialList::default();

        let view = panel_view(
            1,
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        assert_eq!(view.rack_rows.len(), 1);
        assert_eq!(view.rack_rows[0].category, UniqueCategory::Tools);
        assert_eq!(view.rack_rows[0].current, 0);
        assert_eq!(view.rack_rows[0].capacity, Some(10));
    }

    /// Regression test for a bug where a place like the starting wagon --
    /// which dedicates part of its volume to `Rack` and part to `Bulk` --
    /// tracked both in one shared `Inventory` pool, so Bulk goods filling
    /// that pool left no real headroom for tools even though the Tools row
    /// still showed the full nominal ceiling. `Inventory` now tracks a
    /// separate capacity per `StorageKind`, so Bulk contents can never eat
    /// into Rack room (or vice versa): filling Bulk to and past its own
    /// capacity leaves Rack space fully free.
    #[test]
    fn bulk_contents_never_reduce_rack_capacity() {
        use crate::place::{
            FulfilledPorf, ParentRestriction, ParticularPlace, Place, PlaceReq, Porf,
        };
        use crate::resource::{Inventory, RackContents};
        use bevy::math::IVec3;

        use crate::eorf::{EorfInfo, FurnitureOrElement, PlacementStyle, StructureEmbedding};
        use crate::materials::BuildMaterialId;
        use crate::resource::StorageKind;
        use crate::sparse3d::{Facing, Slot, SlotCoord};

        // A wagon-like furniture: 8 Bulk + 2 Rack.
        let wagon = EorfInfo {
            name: "wagon".to_string(),
            placement_style: PlacementStyle::RoomPlop,
            x_char: None,
            z_char: None,
            embedding: StructureEmbedding {
                tall: 0.0,
                passable: 0.0,
                decorative: 0.0,
                striated: 0.0,
                temporary: 1.0,
            },
            kind: FurnitureOrElement::Furniture(vec![]),
            vantage_evaluated: false,
            storage_capacity: vec![(StorageKind::Bulk, 8.0), (StorageKind::Rack, 2.0)],
            placeable: false,
            slots: vec![],
        };
        let mut cw = ConstructedCity::new(vec![wagon]);
        let wagon_id = cw.find_structure_by_name("wagon").unwrap();
        let loc = SlotCoord {
            cube: IVec3::ZERO,
            slot: Slot::Room,
        };
        cw.contents.set(
            loc,
            crate::city::Cell {
                id: wagon_id,
                facing: Facing::default(),
                evaluation: None,
                build_material: BuildMaterialId::default(),
            },
        );
        cw.places = vec![Place {
            name: "camp".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("wagon".to_string()),
                min: 1,
                max: None,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            public_storage: true,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        }];
        let mut contents = Inventory::new([(StorageKind::Bulk, 8.0), (StorageKind::Rack, 2.0)]);
        // Fill Bulk to its own 8-unit capacity -- if it shared a pool with
        // Rack, this would leave no room for tools.
        contents.add_uniform(UniformResource::Plank, 8);
        cw.placed_places.insert(ParticularPlace {
            place: 0,
            fulfillments: vec![FulfilledPorf::Furniture(loc)],
            contents,
            restriction: ParentRestriction::Unrestricted,
        });
        assert_eq!(
            crate::place::rack_free_capacity(&cw, RackContents::Tools),
            2.0,
            "a full Bulk pool must not eat into Rack's own capacity"
        );

        let farms = farms_with(vec![]);
        let pending = crate::city::ProposedCity::new();
        let pop = Population {
            individuals: vec![],
        };
        let traveler_state = no_traveler();
        let material_list = MaterialList::default();

        let view = panel_view(
            1,
            &farms,
            &cw,
            &pending,
            &pop,
            &traveler_state,
            &material_list,
            false,
        );

        assert_eq!(view.rack_rows.len(), 1);
        assert_eq!(view.rack_rows[0].category, UniqueCategory::Tools);
        assert_eq!(view.rack_rows[0].current, 0);
        assert_eq!(
            view.rack_rows[0].capacity,
            Some(2),
            "the Tools row must show the full 2-unit Rack capacity, unaffected by Bulk being full"
        );
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

        let view = panel_view(
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
