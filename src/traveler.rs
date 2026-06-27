use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::resource::{UniqueResource, UniformResource};
use crate::surroundings::generate_path_from_pos;
use crate::world::ConstructedWorld;

pub const TRAVELER_CAPACITY: usize = 1;

#[derive(Serialize, Deserialize, Clone)]
pub struct TravelerConfig {
    pub appear_chance: f32,
    pub origin_dist_min: f32,
    pub origin_dist_max: f32,
    pub potato_demand_min: u16,
    pub potato_demand_max: u16,
    pub other_demand_min: u16,
    pub other_demand_max: u16,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TravelerOffer {
    pub config_index: usize,
    pub potato_demand: u16,
    /// Random non-potato farmable resource the traveler demands.
    pub other_resource: UniformResource,
    pub other_demand: u16,
    /// Path from the traveler's starting position toward the map origin.
    pub path: Vec<Vec2>,
}

#[derive(Resource, Serialize, Deserialize)]
pub struct TravelerState {
    pub configs: Vec<TravelerConfig>,
    pub current_offer: Option<TravelerOffer>,
    pub invited: bool,
}

pub fn setup_travelers(mut commands: Commands) {
    let ron_content = include_str!("../buildables/travelers.ron");
    let configs: Vec<TravelerConfig> = ron::from_str(ron_content).expect("bad travelers.ron");
    commands.insert_resource(TravelerState {
        configs,
        current_offer: None,
        invited: false,
    });
    // Roll an initial offer so the sidebar isn't empty from the first month.
    // We use a default view radius since Surroundings mode hasn't been entered yet.
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

        let potato_demand =
            rng.random_range(config.potato_demand_min..=config.potato_demand_max);

        // Pick a random non-potato farmable resource.
        let farmables = UniformResource::inedible_farmables();
        let other_resource = farmables[rng.random_range(0..farmables.len())];

        let other_demand =
            rng.random_range(config.other_demand_min..=config.other_demand_max);

        // Effective radius is 0 (default) when player hasn't visited Surroundings yet;
        // fall back to a reasonable map-unit distance so the path is still plausible.
        let effective_radius = if view_radius > 0.0 { view_radius } else { 30.0 };
        let dist = effective_radius
            * rng.random_range(config.origin_dist_min..config.origin_dist_max);
        let angle: f32 = rng.random_range(0.0..TAU);
        let start = Vec2::new(angle.cos() * dist, angle.sin() * dist);

        let path = generate_path_from_pos(start, rng);

        state.current_offer = Some(TravelerOffer {
            config_index: idx,
            potato_demand,
            other_resource,
            other_demand,
            path,
        });
        // Only one offer at a time (TRAVELER_CAPACITY = 1).
        break;
    }
}

/// Returns true if the player can afford the traveler's demands after the market
/// preview gains are applied.
pub fn can_afford_traveler(
    offer: &TravelerOffer,
    station_totals: &[(UniformResource, u32, bool)],
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
    available(UniformResource::Potato) >= offer.potato_demand as u32
        && available(offer.other_resource) >= offer.other_demand as u32
}

/// Deducts demands from the first storage station, deposits one Tool, and
/// returns the traveler's path for appending to `FarmsResource.extra_paths`.
pub fn accept_traveler(offer: &TravelerOffer, constructed: &mut ConstructedWorld) -> Vec<Vec2> {
    // Find the first storage station index.
    let storage_idx = constructed.placed_stations.iter().position(|ps| {
        constructed
            .stations
            .get(ps.station)
            .map_or(false, |info| info.storage.is_some())
    });

    if let Some(idx) = storage_idx {
        let inv = &mut constructed.placed_stations[idx].contents;
        inv.subtract_uniform(UniformResource::Potato, offer.potato_demand);
        inv.subtract_uniform(offer.other_resource, offer.other_demand);
        inv.add_unique(UniqueResource::Tool);
    }

    offer.path.clone()
}
