//! Which farms come to this month's market, and why the others don't.
//!
//! There are no invitations: every farm the player can see comes, if it can
//! get a market stand and isn't too far away. A farm further than
//! [`ROOM_CUTOFF`] also needs a guest room (a bedroom in an inn) at least as
//! good as its `required_quality`. Farms claim stands and rooms in priority
//! order, each taking the *worst* room that still satisfies it, so better
//! rooms stay free for pickier farms further out. An invited traveler claims
//! its room before any farm does.
//!
//! Priority comes in three tiers (see [`MarketDemand`]): first the farms whose
//! produce the ongoing construction project still needs, then those whose
//! produce there is storage room for, then everyone else (who still attend if
//! there's space). Within a tier, and when claiming, farms go nearest-first
//! (by road travel cost, ties broken by farm id).
//!
//! The plan is a pure function of the farms, their production, the city's
//! [`MarketVenues`] and [`MarketDemand`], and the traveler, so the map's
//! preview and the month's real execution agree.

use std::collections::{BTreeMap, HashMap};

use bevy::prelude::*;

use crate::city::ConstructedCity;
use crate::city::ProposedCity;
use crate::evaluation::{evaluate_place, QualityInputs};
use crate::materials::MaterialList;
use crate::place::PlacedPlaceId;
use crate::resource::UniformResource;
use crate::surroundings::farmstead::{FarmId, FarmsResource, ProductionPlan};
use crate::surroundings::map::CIRCLE_REVEAL_RADIUS;
use crate::surroundings::road_network::UNPAVED_FACTOR;
use crate::traveler::TravelerState;

/// Farms whose travel cost exceeds this need a guest room to stay in.
pub const ROOM_CUTOFF: f32 = 0.75 * CIRCLE_REVEAL_RADIUS * UNPAVED_FACTOR;
/// Farms whose travel cost exceeds this can't come at all.
pub const MAX_TRAVEL_COST: f32 = 1.5 * CIRCLE_REVEAL_RADIUS * UNPAVED_FACTOR;

/// A guest room and its quality.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuestRoom {
    pub id: PlacedPlaceId,
    pub quality: f32,
}

/// What the city offers market visitors: how many stands, and which guest
/// rooms. Expensive to compute (quality needs flood fills and pathing), so
/// it's kept as a resource refreshed whenever the city changes (see
/// `sync_market_venues`) and recomputed fresh when a month advances.
#[derive(Resource, Clone, Debug, Default)]
pub struct MarketVenues {
    pub stands: usize,
    /// Sorted by quality, worst first (ties by id).
    pub rooms: Vec<GuestRoom>,
}

impl MarketVenues {
    pub fn compute(cw: &ConstructedCity) -> Self {
        let room_ids = crate::place::inn_room_ids(cw);
        let rooms = if room_ids.is_empty() {
            Vec::new()
        } else {
            let inputs = QualityInputs::compute(cw);
            room_ids
                .into_iter()
                .map(|id| GuestRoom {
                    id,
                    quality: evaluate_place(cw, id, &inputs),
                })
                .collect()
        };
        Self::new(crate::place::market_stand_count(cw), rooms)
    }

    pub fn new(stands: usize, mut rooms: Vec<GuestRoom>) -> Self {
        rooms.sort_by(|a, b| a.quality.total_cmp(&b.quality).then(a.id.cmp(&b.id)));
        MarketVenues { stands, rooms }
    }
}

/// Keeps [`MarketVenues`] current; run only when the city changes.
pub fn sync_market_venues(cw: Res<ConstructedCity>, mut venues: ResMut<MarketVenues>) {
    *venues = MarketVenues::compute(&cw);
}

/// What the city would like this month's farms to bring, used to decide who
/// gets a stand first (see [`plan_attendance`]).
#[derive(Clone, Debug, Default)]
pub struct MarketDemand {
    /// Still needed by the ongoing construction project, per resource.
    pub project_need: HashMap<UniformResource, u32>,
    /// Free storage room, per resource (only bins assigned to that resource
    /// count).
    pub storage_room: HashMap<UniformResource, u32>,
}

impl MarketDemand {
    pub fn compute(
        constructed: &ConstructedCity,
        pending: &ProposedCity,
        material_list: &MaterialList,
        sandbox_enabled: bool,
    ) -> Self {
        let project_need = if pending.num_changes() > 0 && !sandbox_enabled {
            crate::construction::remaining_construction_need(
                pending,
                &constructed.eorfs,
                material_list,
            )
            .into_iter()
            .collect()
        } else {
            HashMap::new()
        };
        let storage_room = UniformResource::ALL
            .iter()
            .map(|&res| {
                let room = crate::place::storage_free_capacity(constructed, res);
                (res, room.max(0.0).floor() as u32)
            })
            .filter(|&(_, room)| room > 0)
            .collect();
        MarketDemand {
            project_need,
            storage_room,
        }
    }
}

/// One reason a visible farm isn't coming.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Absence {
    TooFar,
    NoStandsRemain,
    NoRoomsRemain,
}

impl Absence {
    /// A glyph from egui's built-in fonts.
    pub fn icon(self) -> &'static str {
        match self {
            Absence::TooFar => "👣",
            Absence::NoStandsRemain => "⛺",
            Absence::NoRoomsRemain => "🏠",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Absence::TooFar => "HUMANIZE: Too far away",
            Absence::NoStandsRemain => "HUMANIZE: No market stands remain",
            Absence::NoRoomsRemain => "No rooms remain.",
        }
    }
}

/// One visible farm's place in this month's plan.
#[derive(Clone, Debug, PartialEq)]
pub struct FarmAttendance {
    /// Road travel cost to the city; `None` if the farm has no route at all.
    pub travel_cost: Option<f32>,
    /// Whether it's far enough out to need a room.
    pub needs_room: bool,
    /// Everything stopping it from coming. Empty means it's coming.
    pub absences: Vec<Absence>,
    /// The guest room it's staying in, if it's coming and needed one.
    pub room: Option<PlacedPlaceId>,
}

impl FarmAttendance {
    pub fn coming(&self) -> bool {
        self.absences.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttendancePlan {
    /// Every visible farm, by id. Fogged farms aren't listed at all.
    pub farms: BTreeMap<FarmId, FarmAttendance>,
    /// How many market stands the city has.
    pub stands: usize,
    /// Whether the current traveler (if any) could get a room it's happy with.
    pub traveler_has_room: bool,
    /// The room an invited traveler is staying in.
    pub traveler_room: Option<PlacedPlaceId>,
}

impl AttendancePlan {
    pub fn get(&self, id: FarmId) -> Option<&FarmAttendance> {
        self.farms.get(&id)
    }

    pub fn is_coming(&self, id: FarmId) -> bool {
        self.get(id).is_some_and(|a| a.coming())
    }

    /// Farms coming this month, ascending by id.
    pub fn coming(&self) -> impl Iterator<Item = FarmId> + '_ {
        self.farms
            .iter()
            .filter(|(_, a)| a.coming())
            .map(|(&id, _)| id)
    }

    pub fn coming_count(&self) -> usize {
        self.coming().count()
    }
}

/// Which tier of [`plan_attendance`]'s priority order a farm is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    /// Brings something the construction project still needs.
    Project,
    /// Brings something there's storage room for.
    Storage,
    Other,
}

/// Plans this month's attendance. `fr`'s road network should already be
/// built (`FarmsResource::ensure_roads`); without one every farm is too far.
/// `production` is what each farm would bring if it came.
///
/// Farms are considered tier by tier ([`Tier`]), nearest first within a tier.
/// A farm that attends draws down the project need, and then the storage room,
/// for what it brings, so later farms are only prioritized for what's still
/// wanted.
pub fn plan_attendance(
    fr: &FarmsResource,
    production: &ProductionPlan,
    venues: &MarketVenues,
    demand: &MarketDemand,
    traveler: &TravelerState,
) -> AttendancePlan {
    let mut room_free = vec![true; venues.rooms.len()];
    // The worst free room at least `quality` good, claiming it if `claim`.
    let mut find_room = |quality: f32, claim: bool| -> Option<PlacedPlaceId> {
        let i = (0..venues.rooms.len())
            .find(|&i| room_free[i] && venues.rooms[i].quality >= quality)?;
        if claim {
            room_free[i] = false;
        }
        Some(venues.rooms[i].id)
    };

    let mut plan = AttendancePlan {
        stands: venues.stands,
        ..Default::default()
    };
    if let Some(offer) = &traveler.current_offer {
        plan.traveler_has_room = find_room(offer.required_quality, false).is_some();
        if traveler.invited {
            plan.traveler_room = find_room(offer.required_quality, true);
        }
    }

    let mut order: Vec<(FarmId, Option<f32>)> = (0..fr.farms.len())
        .map(FarmId::new)
        .filter(|&id| fr.is_visible(id))
        .map(|id| (id, fr.travel_cost(id)))
        .collect();
    order.sort_by(|(a_id, a), (b_id, b)| {
        let key = |c: &Option<f32>| c.unwrap_or(f32::INFINITY);
        key(a).total_cmp(&key(b)).then(a_id.cmp(b_id))
    });

    let brings = |id: FarmId| -> [(UniformResource, u32); 2] {
        [
            (UniformResource::Potato, production.potatoes[id.index()]),
            (fr[id].produced_resource(), production.inedible[id.index()]),
        ]
    };
    let mut project_need = demand.project_need.clone();
    let mut storage_room = demand.storage_room.clone();
    let wanted = |wants: &HashMap<UniformResource, u32>, id: FarmId| {
        brings(id)
            .iter()
            .any(|(res, qty)| *qty > 0 && wants.get(res).is_some_and(|&w| w > 0))
    };

    let mut stands_left = venues.stands;
    for tier in [Tier::Project, Tier::Storage, Tier::Other] {
        for &(id, travel_cost) in &order {
            if plan.farms.contains_key(&id) {
                continue;
            }
            let in_tier = match tier {
                Tier::Project => wanted(&project_need, id),
                Tier::Storage => wanted(&storage_room, id),
                Tier::Other => true,
            };
            if !in_tier {
                continue;
            }

            let farm = &fr[id];
            let too_far = travel_cost.is_none_or(|c| c > MAX_TRAVEL_COST);
            let needs_room = !too_far && travel_cost.is_some_and(|c| c > ROOM_CUTOFF);

            let mut absences = Vec::new();
            if too_far {
                absences.push(Absence::TooFar);
            }
            if stands_left == 0 {
                absences.push(Absence::NoStandsRemain);
            }
            if needs_room && find_room(farm.required_quality, false).is_none() {
                absences.push(Absence::NoRoomsRemain);
            }

            let room = if absences.is_empty() {
                stands_left -= 1;
                // Whatever the project doesn't take is left over for storage.
                for (res, qty) in brings(id) {
                    let to_project = project_need.get(&res).copied().unwrap_or(0).min(qty);
                    if let Some(need) = project_need.get_mut(&res) {
                        *need -= to_project;
                    }
                    if let Some(room) = storage_room.get_mut(&res) {
                        *room = room.saturating_sub(qty - to_project);
                    }
                }
                if needs_room {
                    find_room(farm.required_quality, true)
                } else {
                    None
                }
            } else {
                None
            };
            plan.farms.insert(
                id,
                FarmAttendance {
                    travel_cost,
                    needs_room,
                    absences,
                    room,
                },
            );
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::UniformResource::{Canvas, Straw, Timber};
    use crate::surroundings::farmstead::compute_production;
    use crate::surroundings::farmstead::tests::{farms_with, mk_farm};
    use crate::surroundings::farmstead::FarmData;

    /// A row of unit-tall farms stretching east from the city at the
    /// origin, `widths[i]` map units wide each. Farm `i`'s cheapest route
    /// runs along the bottom border from its west corner, so its travel cost
    /// is `UNPAVED_FACTOR` times its west edge's x (except farm 0, whose
    /// west corners are the city and one unit-long hop above it). A traveler
    /// reveal along the row makes them all visible.
    fn row_of_farms(widths: &[f32]) -> FarmsResource {
        let mut x = 0.0;
        let farms: Vec<FarmData> = widths
            .iter()
            .map(|&width| {
                let farm = FarmData {
                    seed: Vec2::new(x + width / 2.0, 0.5),
                    polygon: vec![
                        Vec2::new(x, 0.0),
                        Vec2::new(x + width, 0.0),
                        Vec2::new(x + width, 1.0),
                        Vec2::new(x, 1.0),
                    ],
                    ..mk_farm(Straw, 5.0)
                };
                x += width;
                farm
            })
            .collect();
        let mut fr = farms_with(farms, vec![]);
        fr.traveler_reveals = vec![vec![Vec2::new(0.0, 0.5), Vec2::new(x, 0.5)]];
        fr.ensure_adjacency();
        fr.ensure_roads();
        fr
    }

    fn no_traveler() -> TravelerState {
        TravelerState {
            configs: Vec::new(),
            current_offer: None,
            invited: false,
        }
    }

    fn room(id: u32, quality: f32) -> GuestRoom {
        GuestRoom {
            id: PlacedPlaceId::new_for_test(id),
            quality,
        }
    }

    /// Plans attendance with all farms producing normally.
    fn plan(
        fr: &FarmsResource,
        venues: &MarketVenues,
        demand: &MarketDemand,
        traveler: &TravelerState,
    ) -> AttendancePlan {
        plan_attendance(fr, &compute_production(fr), venues, demand, traveler)
    }

    fn plan_no_demand(fr: &FarmsResource, venues: &MarketVenues) -> AttendancePlan {
        plan(fr, venues, &MarketDemand::default(), &no_traveler())
    }

    #[test]
    fn nearest_farms_take_the_stands() {
        let fr = row_of_farms(&[5.0; 4]);
        let plan = plan_no_demand(&fr, &MarketVenues::new(2, vec![]));
        let coming: Vec<usize> = plan.coming().map(|id| id.index()).collect();
        assert_eq!(coming, vec![0, 1]);
        assert_eq!(
            plan.get(FarmId::new(2)).unwrap().absences,
            vec![Absence::NoStandsRemain]
        );
    }

    #[test]
    fn farms_with_what_the_project_needs_come_first() {
        let mut fr = row_of_farms(&[5.0; 4]);
        fr.farms[2].resource = Canvas;
        fr.farms[3].resource = Canvas;
        let demand = MarketDemand {
            project_need: [(Canvas, 1)].into(),
            storage_room: HashMap::new(),
        };
        // One stand: the nearest farm that grows canvas wins it, even though
        // farms 0 and 1 are closer.
        let plan = plan(&fr, &MarketVenues::new(1, vec![]), &demand, &no_traveler());
        assert_eq!(plan.coming().collect::<Vec<_>>(), vec![FarmId::new(2)]);
    }

    #[test]
    fn project_need_is_used_up_by_earlier_farms() {
        let mut fr = row_of_farms(&[5.0; 4]);
        for farm in &mut fr.farms {
            farm.resource = Canvas;
        }
        fr.farms[0].resource = Straw;
        let need = fr.farms[1].production_capacity();
        let demand = MarketDemand {
            project_need: [(Canvas, need)].into(),
            storage_room: HashMap::new(),
        };
        // Farm 1 alone covers the need, so the next stand goes to the nearest
        // farm that's not otherwise wanted (farm 0), not to farm 2.
        let plan = plan(&fr, &MarketVenues::new(2, vec![]), &demand, &no_traveler());
        assert_eq!(
            plan.coming().collect::<Vec<_>>(),
            vec![FarmId::new(0), FarmId::new(1)]
        );
    }

    #[test]
    fn storage_room_ranks_below_the_project_but_above_nothing() {
        let mut fr = row_of_farms(&[5.0; 4]);
        fr.farms[2].resource = Canvas;
        fr.farms[3].resource = Timber;
        let demand = MarketDemand {
            project_need: [(Timber, 1)].into(),
            storage_room: [(Canvas, 100), (Timber, 100)].into(),
        };
        let plan_for = |stands| {
            plan(
                &fr,
                &MarketVenues::new(stands, vec![]),
                &demand,
                &no_traveler(),
            )
        };
        // Project first (Timber, farm 3), then storage (Canvas, farm 2).
        assert_eq!(
            plan_for(1).coming().collect::<Vec<_>>(),
            vec![FarmId::new(3)]
        );
        assert_eq!(
            plan_for(2).coming().collect::<Vec<_>>(),
            vec![FarmId::new(2), FarmId::new(3)]
        );
        // Farms contributing nothing still attend if there's space.
        assert_eq!(plan_for(4).coming_count(), 4);
    }

    #[test]
    fn farms_without_storage_for_their_resource_still_attend() {
        let fr = row_of_farms(&[5.0; 2]);
        let plan = plan_no_demand(&fr, &MarketVenues::new(10, vec![]));
        assert_eq!(plan.coming_count(), 2);
    }

    #[test]
    fn distant_farms_take_the_worst_room_that_satisfies_them() {
        // Farms 1 and 2 are past the room cutoff but within the hard cutoff.
        let unit = ROOM_CUTOFF / UNPAVED_FACTOR;
        let mut fr = row_of_farms(&[1.1 * unit, 0.4 * unit, 1.0]);
        assert!(fr.travel_cost(FarmId::new(1)).unwrap() > ROOM_CUTOFF);
        assert!(fr.travel_cost(FarmId::new(2)).unwrap() <= MAX_TRAVEL_COST);
        fr.farms[1].required_quality = 0.5;
        fr.farms[2].required_quality = 0.8;
        let plan_for =
            |fr: &FarmsResource, rooms| plan_no_demand(fr, &MarketVenues::new(10, rooms));

        let plan = plan_for(&fr, vec![room(1, 0.9), room(2, 0.6), room(3, 0.3)]);
        let near = plan.get(FarmId::new(0)).unwrap();
        assert!(!near.needs_room);
        assert!(near.coming());
        // Farm 1 settles for the 0.6 room, leaving the 0.9 room for farm 2.
        assert_eq!(
            plan.get(FarmId::new(1)).unwrap().room,
            Some(room(2, 0.0).id)
        );
        assert_eq!(
            plan.get(FarmId::new(2)).unwrap().room,
            Some(room(1, 0.0).id)
        );

        // With only the 0.9 room, farm 1 (nearer) takes it and farm 2 can't come.
        let plan = plan_for(&fr, vec![room(1, 0.9)]);
        assert_eq!(
            plan.get(FarmId::new(1)).unwrap().room,
            Some(room(1, 0.0).id)
        );
        assert_eq!(
            plan.get(FarmId::new(2)).unwrap().absences,
            vec![Absence::NoRoomsRemain]
        );
    }

    #[test]
    fn farms_past_the_hard_cutoff_are_too_far() {
        let unit = MAX_TRAVEL_COST / UNPAVED_FACTOR;
        let fr = row_of_farms(&[0.5 * unit, 0.6 * unit, 1.0]);
        let plan = plan_no_demand(&fr, &MarketVenues::new(10, vec![]));
        assert_eq!(
            plan.get(FarmId::new(2)).unwrap().absences,
            vec![Absence::TooFar]
        );
    }
}
