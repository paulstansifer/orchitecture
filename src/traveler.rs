use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::city::ConstructedCity;
use crate::resource::{ToolKind, UniformResource};
use crate::surroundings::RoadNetwork;

pub const TRAVELER_CAPACITY: usize = 1;

/// Maximum road-weighted travel cost (`length * factor` summed along the
/// route to the city) a traveler's rejection-sampled origin may lie at --
/// keeps a traveler from materializing somewhere absurdly expensive to reach,
/// without hard-coding a particular shape or distance for "far away".
const MAX_TRAVELER_TRAVEL_COST: f32 = 250.0;

#[derive(Serialize, Deserialize, Clone)]
pub struct TravelerDemand {
    /// One option is chosen at random when rolling an offer.
    pub options: Vec<(UniformResource, std::ops::Range<u32>)>,
}

/// What a traveler gives in exchange for their demands being met.
#[derive(Serialize, Deserialize, Clone)]
pub enum TravelerReward {
    Tool(ToolKind),
    Resource(UniformResource, std::ops::Range<u32>),
}

/// A resolved `TravelerReward`, with any quantity range rolled to a concrete value.
#[derive(Serialize, Deserialize, Clone)]
pub enum ResolvedReward {
    Tool(ToolKind),
    Resource(UniformResource, u32),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Traveler {
    pub appear_chance: f32,
    pub demands: Vec<TravelerDemand>,
    pub reward: TravelerReward,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct IndividualTraveler {
    pub config_index: usize,
    /// Resolved demands: one `(resource, quantity)` per `TravelerDemand`.
    pub demands: Vec<(UniformResource, u32)>,
    pub reward: ResolvedReward,
    /// Path from the traveler's starting position toward the map origin.
    pub path: Vec<Vec2>,
}

#[derive(Resource, Serialize, Deserialize)]
pub struct TravelerState {
    pub configs: Vec<Traveler>,
    pub current_offer: Option<IndividualTraveler>,
    pub invited: bool,
}

pub fn setup_travelers(mut commands: Commands) {
    let ron_content = include_str!("../buildables/travelers.ron");
    let configs: Vec<Traveler> = ron::from_str(ron_content).expect("bad travelers.ron");
    commands.insert_resource(TravelerState {
        configs,
        current_offer: None,
        invited: false,
    });
}

/// Rejection-samples a traveler origin from the road network's own nodes:
/// uniformly at random among those lying both outside the already-visible
/// circle (`view_radius`) and within `MAX_TRAVELER_TRAVEL_COST` of the city
/// by road. Sampling from the network's finite node set (rather than
/// resampling arbitrary map points and hoping one lands in the -- typically
/// thin -- valid annulus) means there's no attempt budget to exhaust and thus
/// no unconstrained fallback: if no node meets both criteria, the search
/// degrades gracefully to the cheapest node outside the circle, and only
/// falls back further (to the single farthest node) if literally every node
/// lies inside `view_radius`, which shouldn't happen on a real map.
fn sample_traveler_origin(view_radius: f32, roads: &RoadNetwork, rng: &mut impl rand::Rng) -> Vec2 {
    let outside_circle: Vec<usize> = (0..roads.nodes.len())
        .filter(|&n| roads.nodes[n].length() > view_radius)
        .collect();

    let in_budget: Vec<usize> = outside_circle
        .iter()
        .copied()
        .filter(|&n| roads.dist_to_city(n) <= MAX_TRAVELER_TRAVEL_COST)
        .collect();

    let node = if !in_budget.is_empty() {
        in_budget[rng.random_range(0..in_budget.len())]
    } else if !outside_circle.is_empty() {
        *outside_circle
            .iter()
            .min_by(|&&a, &&b| roads.dist_to_city(a).total_cmp(&roads.dist_to_city(b)))
            .unwrap()
    } else {
        (0..roads.nodes.len())
            .max_by(|&a, &b| {
                roads.nodes[a]
                    .length_squared()
                    .total_cmp(&roads.nodes[b].length_squared())
            })
            .unwrap_or(roads.city_node)
    };

    roads.nodes[node]
}

/// Roll a new offer for the coming month. Clears the previous offer and `invited` flag.
pub fn roll_traveler_offer(
    state: &mut TravelerState,
    view_radius: f32,
    roads: &RoadNetwork,
    rng: &mut impl rand::Rng,
) {
    state.invited = false;
    state.current_offer = None;

    for (idx, config) in state.configs.iter().enumerate() {
        if rng.random::<f32>() >= config.appear_chance {
            continue;
        }

        let mut demands = Vec::with_capacity(config.demands.len());
        for demand in &config.demands {
            let option_idx = rng.random_range(0..demand.options.len());
            let (resource, ref range) = demand.options[option_idx];
            let qty = rng.random_range(range.start..range.end);
            demands.push((resource, qty));
        }

        let effective_radius = if view_radius > 0.0 { view_radius } else { 30.0 };
        let start = sample_traveler_origin(effective_radius, roads, rng);

        // `start` is already an exact road-network node, so the lowest-cost
        // route to the city begins right at it.
        let path = roads.path_points(roads.nearest_node(start));

        let reward = match &config.reward {
            TravelerReward::Tool(kind) => ResolvedReward::Tool(*kind),
            TravelerReward::Resource(res, range) => {
                ResolvedReward::Resource(*res, rng.random_range(range.start..range.end))
            }
        };

        state.current_offer = Some(IndividualTraveler {
            config_index: idx,
            demands,
            reward,
            path,
        });
        break;
    }
}

/// A traveler's visit this month: its resolved demands (each claimed partly
/// from this month's market inflow, partly from pre-existing storage — see
/// `city_effect::compute_month_effects`) and reward.
///
/// `affordable` reflects whether *all* demands could be fully claimed, and is
/// computed regardless of `invited` — it's what drives the "Invite" checkbox's
/// enabled state, since the player needs to see affordability before deciding
/// to invite. `invited` reflects whether the player actually checked that
/// box. Only when *both* are true were the demands actually claimed
/// (`granted`/`granted_from_storage` are 0 for every demand otherwise), and
/// `apply()`/`apply_resource()` only have an effect in that case.
pub struct TravelerVisit {
    /// `(resource, desired, granted, granted_from_storage)`.
    pub demands: Vec<(UniformResource, u32, u32, u32)>,
    pub reward: ResolvedReward,
    pub path: Vec<Vec2>,
    pub affordable: bool,
    pub invited: bool,
}

impl TravelerVisit {
    fn active(&self) -> bool {
        self.affordable && self.invited
    }

    /// Deducts the storage-backed portion of each demand (the inflow-backed
    /// portion needs no physical action — it was simply never deposited),
    /// deposits a `Tool` reward into the first storage place, and reveals
    /// the traveler's path. A `Resource` reward needs no action here: it was
    /// folded into this month's inflow by `compute_month_effects`, so it's
    /// already been claimed by Construction and/or deposited by the normal
    /// storage-fill/loss pass. No-op unless the visit was both affordable
    /// and invited.
    pub fn apply(
        &self,
        constructed: &mut ConstructedCity,
        farms: &mut crate::surroundings::farmstead::FarmsResource,
    ) {
        if !self.active() {
            return;
        }
        for &(res, _, _granted, from_storage) in &self.demands {
            if from_storage > 0 {
                crate::place::consume_uniform(constructed, res, from_storage);
            }
        }
        if let ResolvedReward::Tool(kind) = &self.reward {
            crate::place::deposit_tool(constructed, *kind);
        }
        farms.traveler_reveals.push(self.path.clone());
    }

    pub fn describe(&self) -> String {
        if !self.invited {
            "A traveler is nearby but hasn't been invited.".to_string()
        } else if self.affordable {
            "A traveler's demands are met; their reward is delivered.".to_string()
        } else {
            "A traveler's demands can't be met this month; they're declined.".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::StdRng, SeedableRng};

    /// A chain of `squares` unit-length squares along the x-axis, city at the
    /// origin corner. Every edge has `trips == 0` (untouched, factor
    /// `FIELD_FACTOR == 5.0`), so the node `n` squares from the city costs
    /// `n * 5.0` to reach -- a predictable cost gradient to sample against.
    fn chain_network(squares: usize) -> RoadNetwork {
        let sq = |x: f32| {
            vec![
                Vec2::new(x, 0.0),
                Vec2::new(x + 1.0, 0.0),
                Vec2::new(x + 1.0, 1.0),
                Vec2::new(x, 1.0),
            ]
        };
        let polygons: Vec<Vec<Vec2>> = (0..squares).map(|i| sq(i as f32)).collect();
        let mut roads = RoadNetwork::build(&polygons, Vec2::ZERO);
        roads.recompute_dist();
        roads
    }

    #[test]
    fn sample_traveler_origin_respects_view_radius_and_travel_cost() {
        // 60 unit edges each costing 5.0: nodes range from 0 up to 300,
        // straddling MAX_TRAVELER_TRAVEL_COST (250), so both acceptance and
        // rejection get exercised.
        let roads = chain_network(60);
        let mut rng = StdRng::seed_from_u64(1);

        for _ in 0..50 {
            let origin = sample_traveler_origin(10.0, &roads, &mut rng);
            assert!(
                origin.length() > 10.0,
                "must lie outside the visible circle: {origin:?}"
            );
            let cost = roads.dist_to_city(roads.nearest_node(origin));
            assert!(
                cost <= MAX_TRAVELER_TRAVEL_COST,
                "cost {cost} exceeds the max travel cost"
            );
        }
    }

    /// A view radius large enough to swallow every affordable node: every
    /// remaining candidate costs more than `MAX_TRAVELER_TRAVEL_COST`, so this
    /// exercises the graceful-degradation fallback. Regression test for a bug
    /// where the old continuous-space rejection sampler, on exhausting its
    /// resampling attempts (which real maps hit ~7% of the time), returned a
    /// fully unconstrained point -- observed in practice as a traveler
    /// appearing at the literal edge of the map.
    #[test]
    fn sample_traveler_origin_falls_back_to_the_cheapest_node_when_none_fit_the_budget() {
        let roads = chain_network(60);
        let view_radius = 55.0;
        let mut rng = StdRng::seed_from_u64(1);

        let expected = (0..roads.nodes.len())
            .filter(|&n| roads.nodes[n].length() > view_radius)
            .min_by(|&a, &b| roads.dist_to_city(a).total_cmp(&roads.dist_to_city(b)))
            .map(|n| roads.nodes[n])
            .unwrap();
        assert!(
            roads.dist_to_city(roads.nearest_node(expected)) > MAX_TRAVELER_TRAVEL_COST,
            "sanity check: the setup should leave no node within budget"
        );

        for _ in 0..10 {
            let origin = sample_traveler_origin(view_radius, &roads, &mut rng);
            assert_eq!(
                origin, expected,
                "must deterministically pick the cheapest option, not an arbitrary point"
            );
        }
    }
}
