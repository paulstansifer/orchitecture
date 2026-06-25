use bevy::prelude::*;

use super::farmstead::{FarmData, FarmsResource};
use crate::resource::{Inventory, UniformResource};

const NUM_SEEDS: usize = 200;
const MAP_EXTENT: f32 = 200.0;
pub const CLIP_BOUNDS: f32 = 300.0;
// 200 seeds over a 400×400 map gives ~800 sq-units per cell on average;
// this scale puts that average at ~6 acres (midpoint of the 5–10 range).
const ACRES_PER_UNIT_SQ: f32 = 0.0075;

// Inedible farmable resources (potatoes are produced separately by all farms).
const FARMABLE: &[UniformResource] = &[
    UniformResource::Canvas,
    UniformResource::Thatch,
    UniformResource::Timber,
    UniformResource::Block,
    UniformResource::Fieldstone,
];

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
        let (sx, sy) = poly.iter().fold((0.0_f32, 0.0_f32), |acc, p| (acc.0 + p.x, acc.1 + p.y));
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

fn generate_path(rng: &mut impl rand::Rng) -> Vec<Vec2> {
    use std::f32::consts::TAU;
    let start_dist = rng.random_range(110.0..150.0_f32);
    let start_angle: f32 = rng.random_range(0.0..TAU);
    let start = Vec2::new(
        start_angle.cos() * start_dist,
        start_angle.sin() * start_dist,
    );
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
    use rand::Rng;
    let mut rng = rand::rng();

    let mut seeds: Vec<Vec2> = (0..NUM_SEEDS)
        .map(|_| {
            Vec2::new(
                rng.random_range(-MAP_EXTENT..MAP_EXTENT),
                rng.random_range(-MAP_EXTENT..MAP_EXTENT),
            )
        })
        .collect();

    // Relax seeds toward their Voronoi centroids so cell sizes are naturally
    // closer to the 5–10 acre target before clamping.
    lloyd_relax(&mut seeds, 5);

    let mut farms = Vec::new();
    for (i, &seed) in seeds.iter().enumerate() {
        let polygon = voronoi_cell(seed, &seeds);
        if polygon.is_empty() {
            continue;
        }
        let area: f32 = (polygon_area(&polygon) * ACRES_PER_UNIT_SQ).clamp(5.0, 10.0);
        let fertility: f32 = rng.random_range(0.75..1.25_f32);
        let res_idx = i % FARMABLE.len();
        let resource = FARMABLE[res_idx];
        // wanted_resource is a different inedible resource
        let wanted_offset = rng.random_range(1..FARMABLE.len());
        let wanted_resource = FARMABLE[(res_idx + wanted_offset) % FARMABLE.len()];
        let want_max = (area.round() as u32).max(3);
        farms.push(FarmData {
            seed,
            polygon,
            area,
            fertility,
            resource,
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

    let path = generate_path(&mut rng);

    commands.insert_resource(FarmsResource {
        farms,
        circle_pos,
        path,
        player_goods: Inventory::new(8, 10000.0),
    });
}
