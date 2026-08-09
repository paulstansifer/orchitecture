//! The headless-safe core of the per-frame simulation: the change-detection
//! gated systems that keep derived state (idea progress, place formation,
//! navigation, assignments, work) in step with the city grid.
//!
//! Both the graphical app (`main.rs`) and the headless harness (`headless.rs`)
//! add [`SimulationPlugin`], so the two can't drift. That matters more than
//! usual here: verifying change-detection behavior is a large part of what the
//! harness is *for*, so a system that reached the game but not the harness
//! would silently stop being modeled by the tests meant to cover it.
//!
//! Nothing added here may touch rendering, assets, or egui -- the harness runs
//! on `MinimalPlugins` with no window or GPU.

use bevy::prelude::*;

use crate::city::ConstructedCity;
use crate::idea::{sync_idea_progress, IdeaState};
use crate::pathing::rebuild_navigation_grid;
use crate::place::sync_places_system;
use crate::population::{sync_assignments, Population};
use crate::work::sync_work;

/// Everything [`SimulationPlugin`] schedules, as one set -- so callers can
/// order their own systems against the simulation step as a whole rather than
/// naming its members (which would reintroduce the coupling this plugin
/// exists to remove).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SimulationSystems;

pub struct SimulationPlugin;

impl Plugin for SimulationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // Refreshes the understood-segment cache on `ConstructedCity`
                // before place formation reads it, so learning a segment
                // unlocks a gated place on the same frame.
                sync_idea_progress.run_if(resource_changed::<IdeaState>),
                sync_places_system.run_if(resource_changed::<ConstructedCity>),
                rebuild_navigation_grid.run_if(resource_changed::<ConstructedCity>),
                sync_assignments
                    .run_if(resource_changed::<ConstructedCity>.or(resource_changed::<Population>)),
                sync_work
                    .run_if(resource_changed::<ConstructedCity>.or(resource_changed::<Population>)),
            )
                .chain()
                .in_set(SimulationSystems),
        );
    }
}
