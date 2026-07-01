use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::resource::{ToolKind, UniformResource, UniqueResource};
use crate::surroundings::generate_path_from_pos;
use crate::city::ConstructedCity;

pub const TRAVELER_CAPACITY: usize = 1;

#[derive(Serialize, Deserialize, Clone)]
pub struct TravelerDemand {
    /// One option is chosen at random when rolling an offer.
    pub options: Vec<(UniformResource, std::ops::Range<u16>)>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Traveler {
    pub appear_chance: f32,
    /// Fraction of the view-circle radius.
    pub origin_dist: std::ops::Range<f32>,
    pub demands: Vec<TravelerDemand>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct IndividualTraveler {
    pub config_index: usize,
    /// Resolved demands: one `(resource, quantity)` per `TravelerDemand`.
    pub demands: Vec<(UniformResource, u16)>,
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

        state.current_offer = Some(IndividualTraveler {
            config_index: idx,
            demands,
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

/// Deducts demands from storage (spread across stations), deposits one Tool into
/// the first storage station, and returns the traveler's path.
pub fn accept_traveler(
    offer: &IndividualTraveler,
    constructed: &mut ConstructedCity,
) -> Vec<Vec2> {
    for (res, qty) in &offer.demands {
        crate::station::consume_uniform(constructed, *res, *qty as u32);
    }
    let storage_idx = constructed.placed_stations.iter().position(|ps| {
        constructed
            .stations
            .get(ps.station)
            .is_some_and(|info| info.storage.is_some())
    });
    if let Some(idx) = storage_idx {
        constructed.placed_stations[idx]
            .contents
            .add_unique(UniqueResource::Tool(ToolKind::Whipsaw));
    }

    offer.path.clone()
}
