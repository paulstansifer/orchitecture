//! Who turns up at the market, and why.
//!
//! Farms are not invited: each one decides for itself whether the trip is
//! worth making, and the city's market stands are the capacity limit on how
//! many of the willing can be accommodated. The player's lever is
//! [`MarketWants`] — a standing advertisement of what the market is buying,
//! which draws the farms producing it from further afield.
//!
//! Everything here is pure and deterministic (no RNG, no ECS): the whole rule
//! lives in [`choose_attendees`], and [`apply_attendance`] is the thin bridge
//! that writes the resulting `FarmData::invited` flags. The rest of the month
//! pipeline (`seed_market`, `resolve_market`, `record_road_trips`,
//! `pave_fieldstone_routes`) reads those flags exactly as it did when the
//! player set them by hand.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::city::ConstructedCity;
use crate::resource::UniformResource;

use super::farmstead::{travel_cost, FarmId, FarmsResource, MARKET_BOOST, STOCKPILE_MAX};
use super::map::{fog_alpha_at, REVEAL_THRESHOLD};

/// How many resources the market can advertise for at once. The scarcity is
/// the point: advertising timber means the stands fill with timber farms and
/// the potato farms stay home.
pub const WANTED_LIMIT: usize = 2;

/// Eagerness bonus for a farm producing something the market advertises for —
/// it expects a better price, so it'll travel further to sell.
pub const WANTED_DRAW: i32 = 8;

/// Extra production boost an advertised producer takes home, on top of
/// `MARKET_BOOST`. Selling into demand really does leave a farm better off;
/// without this the advertisement would be a lie.
pub const WANTED_BOOST: i32 = 2;

/// What the market is currently advertising that it will buy. Persistent
/// across months — the player sets it when the city's needs change, not every
/// month. Never longer than [`WANTED_LIMIT`] (see [`MarketWants::toggle`]).
///
/// Lives on [`FarmsResource`] rather than as a resource of its own: both the
/// attendance rule and `resolve_market`'s premium need it, and both already
/// have the farms in hand.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct MarketWants {
    wanted: Vec<UniformResource>,
}

impl MarketWants {
    pub fn wanted(&self) -> &[UniformResource] {
        &self.wanted
    }

    pub fn is_wanted(&self, res: UniformResource) -> bool {
        self.wanted.contains(&res)
    }

    pub fn is_full(&self) -> bool {
        self.wanted.len() >= WANTED_LIMIT
    }

    /// Adds `res` if there's room, removes it if it's already advertised.
    /// Returns whether anything changed (a no-op when adding past
    /// [`WANTED_LIMIT`]).
    pub fn toggle(&mut self, res: UniformResource) -> bool {
        if let Some(pos) = self.wanted.iter().position(|&r| r == res) {
            self.wanted.remove(pos);
            true
        } else if self.is_full() {
            false
        } else {
            self.wanted.push(res);
            true
        }
    }
}

/// Why a farm is or isn't at this month's market.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttendanceReason {
    /// It's coming, and a stand is reserved for it.
    Attending,
    /// It would come, but every market stand is taken by an eagerer farm.
    /// The city's cue to build another stand.
    NoStall,
    /// It has nothing worth hauling this far, or it's still riding a boost
    /// from its last visit.
    NotWorthTheTrip,
    /// Not revealed yet: the city doesn't know this farm exists, and it
    /// doesn't know about the market either.
    Unknown,
}

/// One farm's decision, as [`choose_attendees`] worked it out.
#[derive(Clone, Copy, Debug)]
pub struct FarmAttendance {
    pub id: FarmId,
    /// How much this farm wants to come, in roughly potato-sized units.
    /// Positive means willing; a stand still has to be free.
    pub eagerness: i32,
    pub reason: AttendanceReason,
}

impl FarmAttendance {
    pub fn attending(&self) -> bool {
        self.reason == AttendanceReason::Attending
    }
}

/// How much of next month's harvest this farm would waste by staying home:
/// stockpiles cap at `STOCKPILE_MAX`, so a nearly-full farm is about to throw
/// away what it grows. This is what makes a full farm desperate rather than
/// merely willing.
fn spoilage(stockpile: u32, production: u32) -> i32 {
    (stockpile + production).saturating_sub(STOCKPILE_MAX) as i32
}

/// How much this farm wants to go to market this month. Every term is in
/// potato-ish units so they can simply be added up:
///
///   * `haul` — what actually reaches market: its potato stockpile less what
///     the trip eats, plus whatever else it has stored.
///   * `spoilage` — production it would waste by not selling (see [`spoilage`]).
///   * `boost_hunger` — a farm already riding a fresh `MARKET_BOOST` gains
///     little and goes *negative* (it stays home a few months while the boost
///     decays); one that's been penalised by an Adopt is very keen indeed.
///   * `wanted` — [`WANTED_DRAW`] if the city is advertising for what it grows.
///   * `- travel` a second time — the trip costs days as well as potatoes,
///     which is what makes paving a road change behaviour rather than just
///     arithmetic.
fn eagerness(fr: &FarmsResource, id: FarmId) -> i32 {
    let farm = &fr[id];
    let travel = travel_cost(fr, id);
    let capacity = farm.production_capacity();

    let haul = farm.potato_stockpile.saturating_sub(travel) + farm.inedible_stockpile;
    let spoilage =
        spoilage(farm.potato_stockpile, capacity) + spoilage(farm.inedible_stockpile, capacity);
    let boost_hunger = MARKET_BOOST - farm.boost;
    let wanted = if fr.market_wants.is_wanted(farm.produced_resource()) {
        WANTED_DRAW
    } else {
        0
    };

    haul as i32 + spoilage + boost_hunger + wanted - travel as i32
}

/// Whether the city knows this farm exists. Unrevealed farms don't attend:
/// otherwise one would announce itself by turning up, which is the traveler
/// reveals' job.
fn revealed(fr: &FarmsResource, id: FarmId) -> bool {
    fog_alpha_at(fr[id].centroid(), &fr.traveler_reveals) < REVEAL_THRESHOLD
}

/// Works out, for every farm, whether it's coming to this month's market.
/// The willing (positive eagerness) compete for `stands` places, eagerest
/// first, ties broken by `FarmId` so the result is deterministic.
///
/// Returns one entry per farm, in `FarmId` order.
pub fn choose_attendees(fr: &FarmsResource, stands: usize) -> Vec<FarmAttendance> {
    let mut attendance: Vec<FarmAttendance> = (0..fr.farms.len())
        .map(FarmId::new)
        .map(|id| {
            if !revealed(fr, id) {
                return FarmAttendance {
                    id,
                    eagerness: 0,
                    reason: AttendanceReason::Unknown,
                };
            }
            let eagerness = eagerness(fr, id);
            FarmAttendance {
                id,
                eagerness,
                reason: if eagerness > 0 {
                    // Provisionally: capacity is settled below.
                    AttendanceReason::NoStall
                } else {
                    AttendanceReason::NotWorthTheTrip
                },
            }
        })
        .collect();

    // Eagerest first; `FarmId` breaks ties so a given state always produces
    // the same market.
    let mut willing: Vec<FarmId> = attendance
        .iter()
        .filter(|a| a.reason == AttendanceReason::NoStall)
        .map(|a| a.id)
        .collect();
    willing.sort_by_key(|&id| (-attendance[id.index()].eagerness, id));

    for id in willing.into_iter().take(stands) {
        attendance[id.index()].reason = AttendanceReason::Attending;
    }

    attendance
}

/// Runs [`choose_attendees`] against the city's market stand count and writes
/// the result into `FarmData::invited`.
///
/// Idempotent, and deliberately written so an unchanged decision doesn't touch
/// the farms at all — it runs every frame from `ui::update_economy_cache`, and
/// there's no reason for a decision that didn't change to look like a write.
pub fn apply_attendance(fr: &mut FarmsResource, constructed: &ConstructedCity) {
    let stands = crate::place::market_stand_count(constructed);
    let attendance = choose_attendees(fr, stands);
    for entry in attendance {
        let farm = &mut fr[entry.id];
        if farm.invited != entry.attending() {
            farm.invited = entry.attending();
        }
    }
}

/// The resources it's worth offering to advertise for: potatoes (every farm
/// grows them) plus whatever the farms the city knows about actually produce.
pub fn advertisable_resources(fr: &FarmsResource) -> Vec<UniformResource> {
    let mut resources: Vec<UniformResource> =
        crate::surroundings::farmstead::known_farm_plentifulness(fr)
            .into_keys()
            .collect();
    if !resources.contains(&UniformResource::Potato) {
        resources.push(UniformResource::Potato);
    }
    resources.sort();
    resources
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::UniformResource::{Straw, Timber};
    use crate::surroundings::farmstead::{
        apply_production, compute_production, FarmData, FarmProduction,
    };

    /// A farm at `distance` from the city (which sits at the origin), with a
    /// polygon well inside the fog reveal circle unless `distance` says
    /// otherwise. No road network is built, so `travel_cost` falls back to the
    /// straight-line estimate — which is exactly what we want to control here.
    fn mk_farm(
        distance: f32,
        production: FarmProduction,
        potatoes: u32,
        inedible: u32,
    ) -> FarmData {
        FarmData {
            seed: Vec2::new(distance, 0.0),
            polygon: Vec::new(),
            area: 10.0,
            fertility: 1.0,
            production,
            potato_stockpile: potatoes,
            inedible_stockpile: inedible,
            boost: 0,
            invited: false,
            event: Default::default(),
        }
    }

    fn farms_with(farms: Vec<FarmData>) -> FarmsResource {
        let n = farms.len();
        FarmsResource {
            farms,
            circle_pos: Vec2::ZERO,
            traveler_reveals: Vec::new(),
            market_wants: MarketWants::default(),
            neighbors: vec![Vec::new(); n],
            road_trips: Vec::new(),
            road_paved: Vec::new(),
            roads: None,
        }
    }

    fn attending(fr: &FarmsResource, stands: usize) -> Vec<usize> {
        choose_attendees(fr, stands)
            .iter()
            .filter(|a| a.attending())
            .map(|a| a.id.index())
            .collect()
    }

    /// The farm with something to sell beats the one that has nothing, even
    /// though the empty one is closer.
    #[test]
    fn a_full_farm_outbids_an_empty_one_for_a_stand() {
        let fr = farms_with(vec![
            mk_farm(2.0, FarmProduction::Regular(Straw), 0, 0),
            mk_farm(20.0, FarmProduction::Regular(Straw), 38, 30),
        ]);
        assert_eq!(attending(&fr, 1), vec![1]);
    }

    /// Stands are the capacity limit: the rest are willing but have nowhere to
    /// stand, which is what tells the player to build more.
    #[test]
    fn stands_cap_attendance_and_the_rest_report_no_stall() {
        let farms = (0..5)
            .map(|i| mk_farm(5.0, FarmProduction::Regular(Straw), 20 + i, 10))
            .collect();
        let fr = farms_with(farms);

        let attendance = choose_attendees(&fr, 2);
        let attending: Vec<usize> = attendance
            .iter()
            .filter(|a| a.attending())
            .map(|a| a.id.index())
            .collect();
        // The two fullest (stockpiles ascend with the index).
        assert_eq!(attending, vec![3, 4]);
        assert!(attendance
            .iter()
            .filter(|a| !a.attending())
            .all(|a| a.reason == AttendanceReason::NoStall));
    }

    /// With no stands at all, nobody comes however eager they are.
    #[test]
    fn no_stands_means_no_market() {
        let fr = farms_with(vec![mk_farm(2.0, FarmProduction::Regular(Straw), 40, 40)]);
        assert!(attending(&fr, 0).is_empty());
    }

    /// The rotation that makes this whole mechanic work: a farm that just sold
    /// has nothing left and a fresh boost, so it stays home — and comes back
    /// on its own once the boost decays and the stockpiles refill.
    #[test]
    fn a_farm_that_just_sold_sits_out_and_returns_later() {
        let mut fr = farms_with(vec![mk_farm(5.0, FarmProduction::Regular(Straw), 0, 0)]);
        // As `CityEffect::Market` leaves a farm that attended: stockpiles sold
        // into the pool, boost raised.
        fr.farms[0].boost = MARKET_BOOST;

        assert!(
            attending(&fr, 3).is_empty(),
            "a farm with empty stores and a fresh boost has no reason to travel"
        );

        // Left alone, it grows back into wanting the market again.
        let mut months = 0;
        while attending(&fr, 3).is_empty() {
            months += 1;
            assert!(
                months < 12,
                "a farm should want the market again within a year"
            );
            let plan = compute_production(&fr, &mut rand::rng());
            apply_production(&mut fr, &plan);
        }
    }

    /// Advertising is the player's lever: it pulls a distant producer of what
    /// the city needs in ahead of a nearer farm growing something else.
    #[test]
    fn advertising_draws_a_distant_producer_past_a_nearer_farm() {
        let mut fr = farms_with(vec![
            mk_farm(4.0, FarmProduction::Regular(Straw), 12, 8),
            mk_farm(18.0, FarmProduction::Regular(Timber), 12, 8),
        ]);

        assert_eq!(
            attending(&fr, 1),
            vec![0],
            "without advertising, the nearer farm wins the stand"
        );

        assert!(fr.market_wants.toggle(Timber));
        assert_eq!(
            attending(&fr, 1),
            vec![1],
            "advertising for timber should be worth the longer trip"
        );
    }

    /// A farm the city hasn't discovered yet doesn't turn up, however eager
    /// the arithmetic would make it: revealing farms is the travelers' job.
    #[test]
    fn an_unrevealed_farm_never_attends() {
        // Far outside `CIRCLE_REVEAL_RADIUS`, with no traveler reveals.
        let mut fr = farms_with(vec![mk_farm(200.0, FarmProduction::Regular(Straw), 40, 40)]);
        let attendance = choose_attendees(&fr, 3);
        assert_eq!(attendance[0].reason, AttendanceReason::Unknown);

        // Reveal it with a traveler's path and it joins in.
        fr.traveler_reveals
            .push(vec![Vec2::ZERO, Vec2::new(200.0, 0.0)]);
        assert_eq!(attending(&fr, 3), vec![0]);
    }

    /// `WANTED_LIMIT` is what makes advertising a choice rather than a
    /// free-for-all.
    #[test]
    fn market_wants_is_capped_and_toggles() {
        use crate::resource::UniformResource::{Canvas, Potato};
        let mut wants = MarketWants::default();
        assert!(wants.toggle(Straw));
        assert!(wants.toggle(Timber));
        assert!(wants.is_full());
        assert!(!wants.toggle(Canvas), "a third want must not fit");
        assert!(!wants.is_wanted(Canvas));

        assert!(
            wants.toggle(Straw),
            "toggling an advertised resource drops it"
        );
        assert!(!wants.is_wanted(Straw));
        assert!(wants.toggle(Potato));
        assert!(wants.is_wanted(Potato));
    }
}
