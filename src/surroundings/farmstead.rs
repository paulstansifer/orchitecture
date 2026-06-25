use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::resource::{Inventory, UniformResource};

/// Farms within this map-unit radius are considered neighbours for the purpose
/// of updating a farm's wanted resource after a market visit.
const WANTED_UPDATE_RADIUS: f32 = 80.0;

const STOCKPILE_MAX: u32 = 40;

/// Market boundary in map units. A farm at this distance pays 8 potatoes travel cost.
pub const MARKET_RADIUS: f32 = 50.0;

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
        // After Lloyd relaxation the seed converges to the polygon centroid.
        self.seed
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

/// Read-only snapshot of what `run_market` would do given the current state.
pub struct MarketPreview {
    /// Maps farm index → predicted boost each farm will receive.
    pub farm_boosts: HashMap<usize, u32>,
    /// Resources the player will gain (resource, quantity).
    pub player_gains: Vec<(UniformResource, u32)>,
}

fn invited_with_costs(farms: &[FarmData], circle_pos: Vec2) -> Vec<(usize, u32)> {
    farms
        .iter()
        .enumerate()
        .filter(|(_, f)| f.invited)
        .map(|(i, f)| {
            let dist = f.seed.distance(circle_pos);
            let cost = (dist * 8.0 / MARKET_RADIUS.max(1.0)).round() as u32;
            (i, cost)
        })
        .collect()
}

fn pool_totals(
    farms: &[FarmData],
    invited: &[(usize, u32)],
) -> (u32, HashMap<UniformResource, u32>) {
    let mut potato_pool: u32 = 0;
    let mut inedible_pool: HashMap<UniformResource, u32> = HashMap::new();
    for &(i, cost) in invited {
        potato_pool += farms[i].potato_stockpile.saturating_sub(cost);
        *inedible_pool.entry(farms[i].resource).or_insert(0) += farms[i].inedible_stockpile;
    }
    (potato_pool, inedible_pool)
}

fn distribute(
    farms: &[FarmData],
    invited: &[(usize, u32)],
    potato_pool: &mut u32,
    inedible_pool: &mut HashMap<UniformResource, u32>,
) -> HashMap<usize, u32> {
    let mut farm_boosts = HashMap::new();
    for &(i, _) in invited {
        let wanted = farms[i].wanted_resource;
        let want_max = farms[i].want_max;
        let available = *inedible_pool.get(&wanted).unwrap_or(&0);
        let t = available.min(want_max);
        if t > 0 {
            *inedible_pool.get_mut(&wanted).unwrap() -= t;
            let take_potatoes = (*potato_pool).min(t);
            *potato_pool -= take_potatoes;
        }
        farm_boosts.insert(i, t);
    }
    farm_boosts
}

fn gains_from_pool(
    potato_pool: u32,
    inedible_pool: HashMap<UniformResource, u32>,
) -> Vec<(UniformResource, u32)> {
    let mut gains = Vec::new();
    if potato_pool > 0 {
        gains.push((UniformResource::Potato, potato_pool));
    }
    for (res, qty) in inedible_pool {
        if qty > 0 {
            gains.push((res, qty));
        }
    }
    gains
}

/// Compute what the next market run would produce without mutating any state.
pub fn preview_market(fr: &FarmsResource) -> MarketPreview {
    let invited = invited_with_costs(&fr.farms, fr.circle_pos);
    let (mut potato_pool, mut inedible_pool) = pool_totals(&fr.farms, &invited);
    let farm_boosts = distribute(&fr.farms, &invited, &mut potato_pool, &mut inedible_pool);
    let player_gains = gains_from_pool(potato_pool, inedible_pool);
    MarketPreview {
        farm_boosts,
        player_gains,
    }
}

/// Run the monthly market for invited farms.
///
/// Each invited farm contributes (potato_stockpile − travel_cost) potatoes and its
/// entire inedible stockpile to a shared pool. Travel cost scales linearly with
/// distance from circle_pos so that a farm at MARKET_RADIUS units away costs 8.
///
/// Then each invited farm takes up to want_max of its wanted resource (call the
/// amount t) plus t potatoes from the pool. The boost for that farm is set to t.
/// Whatever remains in the pool goes into player_goods.
pub fn run_market(fr: &mut FarmsResource) {
    let invited = invited_with_costs(&fr.farms, fr.circle_pos);
    let (mut potato_pool, mut inedible_pool) = pool_totals(&fr.farms, &invited);

    for &(i, _) in &invited {
        fr.farms[i].potato_stockpile = 0;
        fr.farms[i].inedible_stockpile = 0;
    }

    let farm_boosts = distribute(&fr.farms, &invited, &mut potato_pool, &mut inedible_pool);
    for (i, t) in &farm_boosts {
        fr.farms[*i].boost = *t;
    }

    for (res, qty) in gains_from_pool(potato_pool, inedible_pool) {
        fr.player_goods.add_uniform(res, qty as u16);
    }
}

/// After each market visit every participating farm refreshes its wanted resource:
/// pick a random nearby farm and adopt what it produces, so wanted resources track
/// systematic shifts in local production.
pub fn update_wanted_resources(fr: &mut FarmsResource) {
    use rand::Rng as _;
    let mut rng = rand::rng();
    let snapshot: Vec<(Vec2, UniformResource)> =
        fr.farms.iter().map(|f| (f.seed, f.resource)).collect();

    let invited_indices: Vec<usize> = fr
        .farms
        .iter()
        .enumerate()
        .filter(|(_, f)| f.invited)
        .map(|(i, _)| i)
        .collect();

    for i in invited_indices {
        let (farm_pos, own_resource) = snapshot[i];
        let candidates: Vec<UniformResource> = snapshot
            .iter()
            .enumerate()
            .filter(|(j, (pos, res))| {
                *j != i && *res != own_resource && pos.distance(farm_pos) <= WANTED_UPDATE_RADIUS
            })
            .map(|(_, (_, res))| *res)
            .collect();

        fr.farms[i].wanted_resource = if !candidates.is_empty() {
            candidates[rng.random_range(0..candidates.len())]
        } else {
            let others: Vec<UniformResource> = UniformResource::inedible_farmables()
                .iter()
                .copied()
                .filter(|&r| r != own_resource)
                .collect();
            others[rng.random_range(0..others.len())]
        };
    }
}
