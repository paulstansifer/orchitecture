use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::resource::{Inventory, UniformResource};

const STOCKPILE_MAX: u32 = 40;

#[derive(Serialize, Deserialize)]
pub struct FarmData {
    pub seed: Vec2,
    pub polygon: Vec<Vec2>,
    pub area: f32,
    pub fertility: f32,              // kept only for map coloring
    pub resource: UniformResource,   // inedible resource this farm produces
    pub wanted_resource: UniformResource, // resource this farm wants from others
    pub want_max: u32,               // how many of wanted_resource it wants (>= 3)
    pub potato_stockpile: u32,       // accumulated potatoes, max 40
    pub inedible_stockpile: u32,     // accumulated inedible resource, max 40
    pub boost: u32,                  // extra production per month; declines by 1 each month
    pub invited: bool,
}

impl FarmData {
    pub fn centroid(&self) -> Vec2 {
        let n = self.polygon.len() as f32;
        let (sx, sy) = self
            .polygon
            .iter()
            .fold((0.0_f32, 0.0_f32), |acc, p| (acc.0 + p.x, acc.1 + p.y));
        Vec2::new(sx / n, sy / n)
    }

    pub fn base_production(&self) -> u32 {
        self.area.round() as u32
    }

    /// Accumulate one month's production into both stockpiles and decay the boost.
    pub fn accumulate_monthly(&mut self) {
        let prod = self.base_production() + self.boost;
        self.potato_stockpile = (self.potato_stockpile + prod).min(STOCKPILE_MAX);
        self.inedible_stockpile = (self.inedible_stockpile + prod).min(STOCKPILE_MAX);
        self.boost = self.boost.saturating_sub(1);
    }
}

/// Permanent game-state resource: generated once, persists across mode changes, saved/loaded.
#[derive(Resource, Serialize, Deserialize)]
pub struct FarmsResource {
    pub farms: Vec<FarmData>,
    pub circle_pos: Vec2,
    /// Path from a distant point toward the map origin, in map coordinates.
    pub path: Vec<Vec2>,
    /// Resources accumulated from monthly market exchanges.
    pub player_goods: Inventory,
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

/// Run the monthly market for invited farms.
///
/// Each invited farm contributes (potato_stockpile − travel_cost) potatoes and its
/// entire inedible stockpile to a shared pool. Travel cost scales linearly with
/// distance from circle_pos so that a farm at market_radius units away costs 8.
///
/// Then each invited farm takes up to want_max of its wanted resource (call the
/// amount t) plus t potatoes from the pool. The boost for that farm is set to t.
/// Whatever remains in the pool goes into player_goods.
pub fn run_market(fr: &mut FarmsResource, market_radius: f32) {
    let circle_pos = fr.circle_pos;

    // Collect invited indices and per-farm travel costs.
    let invited: Vec<(usize, u32)> = fr
        .farms
        .iter()
        .enumerate()
        .filter(|(_, f)| f.invited)
        .map(|(i, f)| {
            let dist = f.centroid().distance(circle_pos);
            let cost = (dist * 8.0 / market_radius.max(1.0)).round() as u32;
            (i, cost)
        })
        .collect();

    // Build the shared pool.
    let mut potato_pool: u32 = 0;
    let mut inedible_pool: HashMap<UniformResource, u32> = HashMap::new();

    for &(i, cost) in &invited {
        let farm = &mut fr.farms[i];
        potato_pool += farm.potato_stockpile.saturating_sub(cost);
        *inedible_pool.entry(farm.resource).or_insert(0) += farm.inedible_stockpile;
        farm.potato_stockpile = 0;
        farm.inedible_stockpile = 0;
    }

    // Each farm takes its wanted resource (and matching potatoes) from the pool.
    for &(i, _) in &invited {
        let wanted = fr.farms[i].wanted_resource;
        let want_max = fr.farms[i].want_max;
        let available = *inedible_pool.get(&wanted).unwrap_or(&0);
        let t = available.min(want_max);
        if t > 0 {
            *inedible_pool.get_mut(&wanted).unwrap() -= t;
            let take_potatoes = potato_pool.min(t);
            potato_pool -= take_potatoes;
        }
        fr.farms[i].boost = t;
    }

    // Remaining pool → player_goods.
    if potato_pool > 0 {
        fr.player_goods
            .add_uniform(UniformResource::Potato, potato_pool as u16);
    }
    for (res, qty) in inedible_pool {
        if qty > 0 {
            fr.player_goods.add_uniform(res, qty as u16);
        }
    }
}
