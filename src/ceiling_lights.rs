use std::collections::{HashMap, HashSet, VecDeque};

use bevy::prelude::*;

use crate::sparse3d::{RelSlot, Sparse3D};
use crate::wall_grid::{Cell, WallGrid};

const GRID_PERIOD: i32 = 6;
const COVERAGE_RADIUS: i32 = 3;
const LIGHT_INTENSITY: f32 = 50_000.0;
const LIGHT_RANGE: f32 = 8.0;

#[derive(Component)]
pub struct CeilingLight;

/// Computes world-space positions for interior ceiling lights.
///
/// For each connected span of ceiling tiles (grouped by Y level, then flood-filled
/// for 4-connectivity), finds the grid phase in 0..GRID_PERIOD × 0..GRID_PERIOD that
/// maximises Chebyshev-3 coverage, places lights at grid points that land on actual
/// ceiling tiles, then greedily patches any tiles still uncovered.
pub fn compute_ceiling_lights(contents: &Sparse3D<Cell>) -> Vec<Vec3> {
    let mut ceilings: Vec<(i32, i32, i32)> = contents
        .iter()
        // HACK: `.iter()` provides `RelSlot`s, but we know that it's always floors, never ceilings.
        .filter(|(loc, _)| loc.rel_slot == RelSlot::Floor)
        .map(|(loc, _)| (loc.cube.x, loc.cube.y, loc.cube.z))
        .collect();

    if ceilings.is_empty() {
        return vec![];
    }

    // Exclude the lowest Y level — those really are floors, for sure.
    let min_y = ceilings.iter().map(|&(_, y, _)| y).min().unwrap();
    ceilings.retain(|&(_, y, _)| y > min_y);

    if ceilings.is_empty() {
        return vec![];
    }

    let mut by_y: HashMap<i32, Vec<(i32, i32)>> = HashMap::new();
    for (x, y, z) in ceilings {
        by_y.entry(y).or_default().push((x, z));
    }

    let mut positions = Vec::new();
    for (y, xz_list) in by_y {
        let tile_set: HashSet<(i32, i32)> = xz_list.into_iter().collect();
        for component in connected_components(&tile_set) {
            positions.extend(lights_for_component(&component, y));
        }
    }

    positions
}

fn connected_components(tiles: &HashSet<(i32, i32)>) -> Vec<HashSet<(i32, i32)>> {
    let mut unvisited: HashSet<(i32, i32)> = tiles.clone();
    let mut components = Vec::new();

    while let Some(&start) = unvisited.iter().next() {
        let mut component = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(start);
        unvisited.remove(&start);

        while let Some((x, z)) = queue.pop_front() {
            component.insert((x, z));
            for neighbor in [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)] {
                if unvisited.remove(&neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }
        components.push(component);
    }
    components
}

fn lights_for_component(tiles: &HashSet<(i32, i32)>, y: i32) -> Vec<Vec3> {
    let (best_ox, best_oz) = best_grid_phase(tiles);
    let (min_x, max_x, min_z, max_z) = bbox(tiles);

    let x_start = min_x - (min_x - best_ox).rem_euclid(GRID_PERIOD);
    let z_start = min_z - (min_z - best_oz).rem_euclid(GRID_PERIOD);

    let mut light_xz: Vec<(i32, i32)> = Vec::new();
    let mut x = x_start;
    while x <= max_x {
        let mut z = z_start;
        while z <= max_z {
            if tiles.contains(&(x, z)) {
                light_xz.push((x, z));
            }
            z += GRID_PERIOD;
        }
        x += GRID_PERIOD;
    }

    // Mark which tiles are covered by the initial grid lights.
    let mut covered: HashSet<(i32, i32)> = HashSet::new();
    for &(lx, lz) in &light_xz {
        mark_coverage(lx, lz, &mut covered);
    }

    // Greedy fallback: patch any uncovered ceiling tiles.
    let mut uncovered: HashSet<(i32, i32)> = tiles.difference(&covered).cloned().collect();
    while !uncovered.is_empty() {
        let best = *uncovered
            .iter()
            .max_by_key(|&&(cx, cz)| {
                tiles
                    .iter()
                    .filter(|&&(tx, tz)| {
                        uncovered.contains(&(tx, tz))
                            && chebyshev(tx - cx, tz - cz) <= COVERAGE_RADIUS
                    })
                    .count()
            })
            .unwrap();

        light_xz.push(best);
        mark_coverage(best.0, best.1, &mut covered);
        uncovered.retain(|t| !covered.contains(t));
    }

    // Ceiling face is at world y = cube_y + 1.0; hang the light just below it.
    light_xz
        .into_iter()
        .map(|(x, z)| Vec3::new(x as f32 + 0.5, y as f32 + 0.8, z as f32 + 0.5))
        .collect()
}

fn best_grid_phase(tiles: &HashSet<(i32, i32)>) -> (i32, i32) {
    let (min_x, max_x, min_z, max_z) = bbox(tiles);
    let mut best_score = 0usize;
    let mut best = (0, 0);

    for ox in 0..GRID_PERIOD {
        for oz in 0..GRID_PERIOD {
            let mut covered: HashSet<(i32, i32)> = HashSet::new();
            let x_start = min_x - (min_x - ox).rem_euclid(GRID_PERIOD);
            let z_start = min_z - (min_z - oz).rem_euclid(GRID_PERIOD);

            let mut x = x_start;
            while x <= max_x {
                let mut z = z_start;
                while z <= max_z {
                    if tiles.contains(&(x, z)) {
                        mark_coverage(x, z, &mut covered);
                    }
                    z += GRID_PERIOD;
                }
                x += GRID_PERIOD;
            }

            let score = tiles.iter().filter(|t| covered.contains(t)).count();
            if score > best_score {
                best_score = score;
                best = (ox, oz);
            }
        }
    }
    best
}

fn mark_coverage(lx: i32, lz: i32, covered: &mut HashSet<(i32, i32)>) {
    for dx in -COVERAGE_RADIUS..=COVERAGE_RADIUS {
        for dz in -COVERAGE_RADIUS..=COVERAGE_RADIUS {
            covered.insert((lx + dx, lz + dz));
        }
    }
}

fn chebyshev(dx: i32, dz: i32) -> i32 {
    dx.abs().max(dz.abs())
}

fn bbox(tiles: &HashSet<(i32, i32)>) -> (i32, i32, i32, i32) {
    let min_x = tiles.iter().map(|&(x, _)| x).min().unwrap();
    let max_x = tiles.iter().map(|&(x, _)| x).max().unwrap();
    let min_z = tiles.iter().map(|&(_, z)| z).min().unwrap();
    let max_z = tiles.iter().map(|&(_, z)| z).max().unwrap();
    (min_x, max_x, min_z, max_z)
}

/// Bevy system: despawns all ceiling lights and respawns them from the current WallGrid.
///
/// Runs only when WallGrid changes (see main.rs). The visibility system bypasses
/// change detection for its per-frame mutations so this only fires on real edits.
///
/// TODO: When batch construction is added (placing many tiles before committing),
/// defer this recalculation until the batch is finalised rather than running on
/// every individual tile change.
pub fn update_ceiling_lights(
    mut commands: Commands,
    wall_grid: Res<WallGrid>,
    existing: Query<Entity, With<CeilingLight>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    for pos in compute_ceiling_lights(&wall_grid.contents) {
        commands.spawn((
            PointLight {
                color: Color::srgb(1.0, 0.95, 0.8),
                intensity: LIGHT_INTENSITY,
                range: LIGHT_RANGE,
                shadows_enabled: false,
                ..default()
            },
            Transform::from_translation(pos),
            CeilingLight,
        ));
    }
}
