use bevy::prelude::*;

#[derive(Default)]
pub struct Individual {
    pub home: Option<usize>,
    pub fed_this_month: bool,
}

impl Individual {
    pub fn shelter(&self) -> f32 {
        if self.home.is_some() {
            1.0
        } else {
            0.25
        }
    }

    pub fn food(&self) -> f32 {
        if self.fed_this_month {
            1.0
        } else {
            0.25
        }
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
