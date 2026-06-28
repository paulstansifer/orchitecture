use bevy::prelude::*;

use crate::resource::UniformResource;
use crate::world::ConstructedWorld;

pub struct Individual {
    pub home: Option<usize>,
    pub fed_this_month: bool,
}

impl Default for Individual {
    fn default() -> Self {
        Individual {
            home: None,
            fed_this_month: false,
        }
    }
}

impl Individual {
    pub fn shelter(&self) -> f32 {
        if self.home.is_some() { 1.0 } else { 0.25 }
    }

    pub fn food(&self) -> f32 {
        if self.fed_this_month { 1.0 } else { 0.25 }
    }

    pub fn inspiration(&self) -> f32 {
        1.0
    }

    pub fn morale(&self) -> f32 {
        self.shelter() * self.food() * self.inspiration()
    }
}

#[derive(Resource)]
pub struct Population {
    pub individuals: Vec<Individual>,
}

impl Default for Population {
    fn default() -> Self {
        Population {
            individuals: vec![Individual::default()],
        }
    }
}

pub fn spawn_population(mut commands: Commands) {
    commands.insert_resource(Population::default());
}

/// Subtract `qty` potatoes from the first storage station that holds at least that many.
/// Returns true and commits the deduction; returns false and changes nothing if no station qualifies.
pub fn try_consume_potatoes(constructed: &mut ConstructedWorld, qty: u16) -> bool {
    for i in 0..constructed.placed_stations.len() {
        let is_storage = constructed
            .stations
            .get(constructed.placed_stations[i].station)
            .map_or(false, |info| info.storage.is_some());
        if !is_storage {
            continue;
        }
        let potato_qty = constructed.placed_stations[i]
            .contents
            .uniform_totals()
            .into_iter()
            .find(|(r, _)| *r == UniformResource::Potato)
            .map(|(_, q)| q)
            .unwrap_or(0);
        if potato_qty >= qty {
            constructed.placed_stations[i]
                .contents
                .subtract_uniform(UniformResource::Potato, qty);
            return true;
        }
    }
    false
}
