use bevy::prelude::*;

use super::farmstead::{FarmData, FarmsResource};
use crate::resource::UniformResource;

const NUM_SEEDS: usize = 200;
const MAP_EXTENT: f32 = 200.0;
pub const CLIP_BOUNDS: f32 = 300.0;

// Fog-of-war parameters (all distances in map units).
pub const CIRCLE_REVEAL_RADIUS: f32 = 35.0;
pub const PATH_REVEAL_RADIUS: f32 = 12.0;
pub const FOG_FADE_WIDTH: f32 = 10.0;
pub const FOG_MAX_ALPHA: u8 = 215;

// Farm UI is shown only when the centroid's fog alpha is below this value.
pub const REVEAL_THRESHOLD: u8 = 64;

fn segment_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.dot(ab);
    if len_sq < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Returns the fog alpha (0 = fully clear, FOG_MAX_ALPHA = fully fogged) for a map-space point.
pub fn fog_alpha_at(map_pos: Vec2, paths: &[Vec<Vec2>]) -> u8 {
    let dist_circle = (map_pos.length() - CIRCLE_REVEAL_RADIUS).max(0.0);

    let dist_path = paths
        .iter()
        .map(|path| {
            if path.len() >= 2 {
                let raw = path
                    .windows(2)
                    .map(|seg| segment_dist(map_pos, seg[0], seg[1]))
                    .fold(f32::MAX, f32::min);
                (raw - PATH_REVEAL_RADIUS).max(0.0)
            } else {
                f32::MAX
            }
        })
        .fold(f32::MAX, f32::min);

    let dist = dist_circle.min(dist_path);
    let t = (dist / FOG_FADE_WIDTH).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t);
    (t * FOG_MAX_ALPHA as f32) as u8
}
// 200 seeds over a 400×400 map gives ~800 sq-units per cell on average;
// this scale puts that average at ~6 acres (midpoint of the 5–10 range).
const ACRES_PER_UNIT_SQ: f32 = 0.0075;

// Sutherland-Hodgman: keep vertices where dot(v - point, normal) >= 0
fn clip_polygon_by_halfplane(poly: &[Vec2], point: Vec2, normal: Vec2) -> Vec<Vec2> {
    if poly.is_empty() {
        return vec![];
    }
    let mut result = Vec::new();
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let da = (a - point).dot(normal);
        let db = (b - point).dot(normal);
        if da >= 0.0 {
            result.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            result.push(a + t * (b - a));
        }
    }
    result
}

fn polygon_area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    let mut sum = 0.0_f32;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        sum += a.x * b.y - b.x * a.y;
    }
    sum.abs() * 0.5
}

/// True area-weighted centroid via the shoelace formula.
fn polygon_centroid(poly: &[Vec2]) -> Vec2 {
    let n = poly.len();
    let mut cx = 0.0_f32;
    let mut cy = 0.0_f32;
    let mut signed_area = 0.0_f32;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let cross = a.x * b.y - b.x * a.y;
        cx += (a.x + b.x) * cross;
        cy += (a.y + b.y) * cross;
        signed_area += cross;
    }
    signed_area *= 0.5;
    if signed_area.abs() < 1e-6 {
        let (sx, sy) = poly
            .iter()
            .fold((0.0_f32, 0.0_f32), |acc, p| (acc.0 + p.x, acc.1 + p.y));
        return Vec2::new(sx / n as f32, sy / n as f32);
    }
    Vec2::new(cx / (6.0 * signed_area), cy / (6.0 * signed_area))
}

/// Lloyd relaxation: moves each seed to the centroid of its Voronoi cell,
/// making cell sizes more uniform across iterations.
fn lloyd_relax(seeds: &mut Vec<Vec2>, steps: usize) {
    for _ in 0..steps {
        let snapshot = seeds.clone();
        for seed in seeds.iter_mut() {
            let cell = voronoi_cell(*seed, &snapshot);
            if !cell.is_empty() {
                *seed = polygon_centroid(&cell);
            }
        }
    }
}

fn voronoi_cell(seed: Vec2, all_seeds: &[Vec2]) -> Vec<Vec2> {
    let b = CLIP_BOUNDS;
    let mut poly = vec![
        Vec2::new(-b, -b),
        Vec2::new(b, -b),
        Vec2::new(b, b),
        Vec2::new(-b, b),
    ];
    for &other in all_seeds {
        if other == seed {
            continue;
        }
        let midpoint = (seed + other) * 0.5;
        let normal = seed - other; // points toward seed's side
        poly = clip_polygon_by_halfplane(&poly, midpoint, normal);
        if poly.is_empty() {
            break;
        }
    }
    poly
}

fn two_lowest_indices(vals: &[f32]) -> (usize, usize) {
    assert!(vals.len() >= 2);
    let (mut first, mut second) = if vals[0] <= vals[1] { (0, 1) } else { (1, 0) };
    for i in 2..vals.len() {
        if vals[i] < vals[first] {
            second = first;
            first = i;
        } else if vals[i] < vals[second] {
            second = i;
        }
    }
    (first, second)
}

/// Generates a winding path from `start` toward the map origin.
pub fn generate_path_from_pos(start: Vec2, rng: &mut impl rand::Rng) -> Vec<Vec2> {
    let start_dist = start.length();
    let main_dir = -start.normalize(); // points toward origin
    let perp = Vec2::new(-main_dir.y, main_dir.x);

    let mut points = vec![start];
    let num_middle = 4;
    for i in 1..=num_middle {
        let t = i as f32 / (num_middle + 1) as f32;
        let base = start + main_dir * (start_dist * t);
        // Perpendicular deviation shrinks as we approach the centre
        let deviation = rng.random_range(-12.0..12.0_f32) * (1.0 - t * 0.7);
        points.push(base + perp * deviation);
    }
    points.push(Vec2::ZERO);
    points
}

pub fn generate_farms(mut commands: Commands) {
    use rand::Rng as _;
    let mut rng = rand::rng();

    let mut seeds: Vec<Vec2> = (0..NUM_SEEDS)
        .map(|_| {
            Vec2::new(
                rng.random_range(-MAP_EXTENT..MAP_EXTENT),
                rng.random_range(-MAP_EXTENT..MAP_EXTENT),
            )
        })
        .collect();

    // One Lloyd step removes degenerate slivers without killing size variance;
    // more steps converge toward equal-area cells (~6 ac each), which is too uniform.
    lloyd_relax(&mut seeds, 1);

    // Phase 1: compute Voronoi cells and areas, then sort by distance from
    // the map centre so resource assignment radiates outward.
    let mut pre_farms: Vec<(Vec2, Vec<Vec2>, f32)> = seeds
        .iter()
        .filter_map(|&seed| {
            let polygon = voronoi_cell(seed, &seeds);
            if polygon.is_empty() {
                return None;
            }
            let area = (polygon_area(&polygon) * ACRES_PER_UNIT_SQ).clamp(5.0, 10.0);
            Some((seed, polygon, area))
        })
        .collect();
    pre_farms.sort_by(|a, b| {
        a.0.length_squared()
            .partial_cmp(&b.0.length_squared())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Phase 2: assign resources by acreage balance — always pick one of the
    // two types with the fewest total acres assigned so far.
    let farmable = UniformResource::inedible_farmables();
    let mut resource_acres = vec![0.0f32; farmable.len()];
    let mut farms = Vec::new();

    for (seed, polygon, area) in pre_farms {
        let (i0, i1) = two_lowest_indices(&resource_acres);
        let res_idx = if rng.random_range(0..2usize) == 0 {
            i0
        } else {
            i1
        };
        let resource = farmable[res_idx];
        resource_acres[res_idx] += area;

        let fertility = rng.random_range(0.75..1.25_f32);
        let wanted_offset = rng.random_range(1..farmable.len());
        let wanted_resource = farmable[(res_idx + wanted_offset) % farmable.len()];
        let want_max = (area.round() as u32).max(3);

        farms.push(FarmData {
            seed,
            polygon,
            area,
            fertility,
            production: crate::surroundings::farmstead::FarmProduction::Regular(resource),
            wanted_resource,
            want_max,
            potato_stockpile: 0,
            inedible_stockpile: 0,
            boost: 0,
            invited: false,
        });
    }

    let mut circle_pos = Vec2::ZERO;
    let mut best_dist = f32::MAX;
    for farm in &farms {
        for &v in &farm.polygon {
            let d = v.length_squared();
            if d < best_dist {
                best_dist = d;
                circle_pos = v;
            }
        }
    }

    let neighbors = crate::surroundings::farmstead::build_adjacency(&farms);
    let n = farms.len();
    commands.insert_resource(FarmsResource {
        farms,
        circle_pos,
        traveler_reveals: Vec::new(),
        neighbors,
        farm_events: vec![crate::surroundings::farmstead::FarmEvent::Market; n],
    });
}
