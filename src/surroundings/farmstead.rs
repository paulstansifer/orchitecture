use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::resource::{Inventory, UniformResource};

const STOCKPILE_MAX: u16 = 100;

#[derive(Serialize, Deserialize)]
pub struct FarmData {
    pub seed: Vec2,
    pub polygon: Vec<Vec2>,
    pub area: f32,
    pub fertility: f32,
    pub farmers: u32,
    pub resource: UniformResource,
    pub stockpile: Inventory,
    pub invited: bool,
}

impl FarmData {
    pub fn surplus_fraction(&self) -> f32 {
        (self.area * self.fertility / (2.0 * self.farmers as f32) - 1.0).max(0.0)
    }

    pub fn stockpile_qty(&self) -> u16 {
        self.stockpile
            .uniform_totals()
            .into_iter()
            .find(|(r, _)| *r == self.resource)
            .map(|(_, q)| q)
            .unwrap_or(0)
    }

    /// Accumulate one month's production into the stockpile.
    ///
    /// `month` is the zero-indexed month since the game started. Production starts at
    /// `surplus * 80` and decreases by `surplus * 10` each month, flooring at `surplus * 20`.
    /// At 50% surplus that's 40 → 35 → … → 10.  Stockpile caps at 100.
    pub fn accumulate_monthly(&mut self, month: u32) {
        let surplus = self.surplus_fraction();
        if surplus <= 0.0 {
            return;
        }
        let factor = (80.0 - month as f32 * 10.0).max(20.0);
        let production = (surplus * factor).floor() as u16;
        if production == 0 {
            return;
        }
        let current = self.stockpile_qty();
        let add = production.min(STOCKPILE_MAX.saturating_sub(current));
        if add > 0 {
            self.stockpile.add_uniform(self.resource, add);
        }
    }
}

/// Permanent game-state resource: generated once, persists across mode changes, saved/loaded.
#[derive(Resource, Serialize, Deserialize)]
pub struct FarmsResource {
    pub farms: Vec<FarmData>,
    pub circle_pos: Vec2,
    /// Path from a distant point toward the map origin, in map coordinates.
    pub path: Vec<Vec2>,
}

/// Transient resource: exists only while in Surroundings mode.
#[derive(Resource)]
pub struct SurroundingsState {
    pub viewport_offset: Vec2,
}

/// In-game calendar: tracks time in weeks (four weeks per month).
#[derive(Resource, Default, Serialize, Deserialize)]
pub struct GameClock {
    pub weeks: u32,
}

impl GameClock {
    pub fn month(&self) -> u32 {
        self.weeks / 4
    }

    pub fn week_of_month(&self) -> u32 {
        self.weeks % 4
    }

    /// Advances by one week. Returns `true` if a new month just began.
    pub fn advance_week(&mut self) -> bool {
        let prev = self.month();
        self.weeks += 1;
        self.month() != prev
    }
}
