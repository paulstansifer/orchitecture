//! Debug-only tripwire: warns if any of the game's "big bag of data" resources
//! (the ones expensive systems are gated on via `resource_changed`) are being
//! mutated far more often than expected. These resources should only change on
//! genuine player edits/ticks, not every frame — if one crosses the threshold,
//! whatever's gated on it (flood fills, nav-grid rebuilds, place sync, ...) is
//! silently running every frame instead of only when needed.

use bevy::prelude::*;
use std::collections::HashMap;
use std::time::Duration;

use crate::city::ConstructedCity;
use crate::eorf::EorfList;
use crate::idea::IdeaState;
use crate::population::Population;
use crate::surroundings::farmstead::FarmsResource;

/// Frames each tracked resource is expected to change on less than this
/// fraction of, on average. Crossing it gets a `warn!` in the periodic report.
const WARN_THRESHOLD: f64 = 0.10;

const REPORT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Resource)]
pub struct ChangeFrequencyTracker {
    counts: HashMap<&'static str, (u32, u32)>,
    timer: Timer,
}

impl Default for ChangeFrequencyTracker {
    fn default() -> Self {
        Self {
            counts: HashMap::new(),
            timer: Timer::new(REPORT_INTERVAL, TimerMode::Repeating),
        }
    }
}

fn track_resource_change<R: Resource>(
    resource: Res<R>,
    mut tracker: ResMut<ChangeFrequencyTracker>,
) {
    let entry = tracker
        .counts
        .entry(std::any::type_name::<R>())
        .or_insert((0, 0));
    entry.1 += 1;
    if resource.is_changed() {
        entry.0 += 1;
    }
}

fn report_change_frequencies(time: Res<Time>, mut tracker: ResMut<ChangeFrequencyTracker>) {
    if !tracker.timer.tick(time.delta()).just_finished() {
        return;
    }
    for (name, (changed, total)) in tracker.counts.iter_mut() {
        if *total > 0 {
            let ratio = *changed as f64 / *total as f64;
            if ratio > WARN_THRESHOLD {
                warn!(
                    "{name} changed on {:.1}% of frames ({changed}/{total}) over the last \
                     {REPORT_INTERVAL:?} — expected under {:.0}%; whatever's gated on this \
                     resource is probably running every frame",
                    ratio * 100.0,
                    WARN_THRESHOLD * 100.0,
                );
            }
        }
        *changed = 0;
        *total = 0;
    }
}

pub struct ChangeFrequencyDiagnosticsPlugin;

impl Plugin for ChangeFrequencyDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChangeFrequencyTracker>().add_systems(
            Update,
            (
                track_resource_change::<ConstructedCity>,
                track_resource_change::<Population>,
                track_resource_change::<IdeaState>,
                track_resource_change::<EorfList>,
                track_resource_change::<FarmsResource>,
                report_change_frequencies,
            ),
        );
    }
}
