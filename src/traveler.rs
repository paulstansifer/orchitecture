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

/// Returns true if the player can afford all of the traveler's demands after
/// market preview gains are applied.
pub fn can_afford_traveler(
    offer: &IndividualTraveler,
    station_totals: &[(UniformResource, u32, crate::resource::Precision)],
    preview_gains: &[(UniformResource, u32)],
) -> bool {
    let available = |res: UniformResource| -> u32 {
        let stored = station_totals
            .iter()
            .find(|(r, _, _)| *r == res)
            .map_or(0, |(_, q, _)| *q);
        let gain = preview_gains
            .iter()
            .find(|(r, _)| *r == res)
            .map_or(0, |(_, q)| *q);
        stored + gain
    };
    offer
        .demands
        .iter()
        .all(|(res, qty)| available(*res) >= *qty as u32)
}

/// Deducts demands from storage (spread across stations), deposits the traveler's
/// reward into the first storage place, and returns the traveler's path.
pub fn accept_traveler(offer: &IndividualTraveler, constructed: &mut ConstructedCity) -> Vec<Vec2> {
    for (res, qty) in &offer.demands {
        crate::place::consume_uniform(constructed, *res, *qty as u32);
    }
    if let Some(&id) = crate::place::storage_ids(constructed).first() {
        let contents = &mut constructed.placed_places[id].contents;
        match &offer.reward {
            ResolvedReward::Tool(kind) => contents.add_unique(UniqueResource::Tool(*kind)),
            ResolvedReward::Resource(res, qty) => contents.add_uniform(*res, *qty),
        }
    }

    offer.path.clone()
}
