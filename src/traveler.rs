use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::city::ConstructedCity;
use crate::resource::{ToolKind, UniformResource, UniqueResource};
use crate::surroundings::generate_path_from_pos;

pub const TRAVELER_CAPACITY: usize = 1;

#[derive(Serialize, Deserialize, Clone)]
pub struct TravelerDemand {
    /// One option is chosen at random when rolling an offer.
    pub options: Vec<(UniformResource, std::ops::Range<u16>)>,
}

/// What a traveler gives in exchange for their demands being met.
#[derive(Serialize, Deserialize, Clone)]
pub enum TravelerReward {
    Tool(ToolKind),
    Resource(UniformResource, std::ops::Range<u16>),
}

/// A resolved `TravelerReward`, with any quantity range rolled to a concrete value.
#[derive(Serialize, Deserialize, Clone)]
pub enum ResolvedReward {
    Tool(ToolKind),
    Resource(UniformResource, u16),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Traveler {
    pub appear_chance: f32,
    /// Fraction of the view-circle radius.
    pub origin_dist: std::ops::Range<f32>,
    pub demands: Vec<TravelerDemand>,
    pub reward: TravelerReward,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct IndividualTraveler {
    pub config_index: usize,
    /// Resolved demands: one `(resource, quantity)` per `TravelerDemand`.
    pub demands: Vec<(UniformResource, u16)>,
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

/// Roll a new offer for the coming month. Clears the previous offer and `invited` flag.
pub fn roll_traveler_offer(state: &mut TravelerState, view_radius: f32, rng: &mut impl rand::Rng) {
    use std::f32::consts::TAU;

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
        let dist =
            effective_radius * rng.random_range(config.origin_dist.start..config.origin_dist.end);
        let angle: f32 = rng.random_range(0.0..TAU);
        let start = Vec2::new(angle.cos() * dist, angle.sin() * dist);

        let path = generate_path_from_pos(start, rng);

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
            if let Some(&id) = crate::place::storage_ids(constructed).first() {
                constructed.placed_places[id]
                    .contents
                    .add_unique(UniqueResource::Tool(*kind));
            }
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
