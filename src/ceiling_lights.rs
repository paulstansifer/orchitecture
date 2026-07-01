use std::collections::{HashMap, HashSet, VecDeque};

use std::f32::consts::FRAC_PI_2;

use bevy::math::IVec3;
use bevy::prelude::*;

use crate::city::{Cell, ConstructedCity};
use crate::sparse3d::{Slot, SlotCoord, Sparse3D};
use crate::structure::{Structure, StructureList};

#[allow(dead_code)]
const GRID_PERIOD: i32 = 5;
#[allow(dead_code)]
const COVERAGE_RADIUS: i32 = 3;
#[allow(dead_code)]
const LIGHT_INTENSITY: f32 = 150_000.0;
#[allow(dead_code)]
const LIGHT_RANGE: f32 = 8.0;

#[allow(dead_code)]
#[derive(Component)]
pub struct CeilingLight;

#[derive(Component)]
pub struct WindowLight;

/// Returns true if no Floor tile exists above (cx, cy, cz) within 30 levels.
fn has_sky_above(contents: &Sparse3D<Cell>, cx: i32, cy: i32, cz: i32) -> bool {
    for dy in 1..=30 {
        if contents
            .get(SlotCoord {
                cube: IVec3::new(cx, cy + dy, cz),
                slot: Slot::Floor,
            })
            .is_some()
        {
            return false;
        }
    }
    true
}

/// Returns (position, look_direction) for each exterior window: exactly the side
/// with a clear path to the sky is the exterior; the light is placed just inside,
/// facing inward.
fn compute_window_lights(contents: &Sparse3D<Cell>, structures: &[Structure]) -> Vec<(Vec3, Vec3)> {
    let mut results = Vec::new();

    for (loc, cell) in contents.iter() {
        if !matches!(loc.slot, Slot::XLoWall | Slot::ZLoWall) {
            continue;
        }
        if structures[cell.id.as_usize()].info.name != "window" {
            continue;
        }

        // Cubes on each side of this wall, and the inward direction for each.
        let (neg_cube, pos_cube, pos_dir) = match loc.slot {
            Slot::XLoWall => (
                IVec3::new(loc.cube.x - 1, loc.cube.y, loc.cube.z),
                loc.cube,
                Vec3::X,
            ),
            Slot::ZLoWall => (
                IVec3::new(loc.cube.x, loc.cube.y, loc.cube.z - 1),
                loc.cube,
                Vec3::Z,
            ),
            _ => unreachable!(),
        };

        let sky_neg = has_sky_above(contents, neg_cube.x, loc.cube.y, neg_cube.z);
        let sky_pos = has_sky_above(contents, pos_cube.x, loc.cube.y, pos_cube.z);

        // Only exterior windows: exactly one side open to sky.
        let look_dir = match (sky_neg, sky_pos) {
            (true, false) => pos_dir,  // exterior -side, interior +side
            (false, true) => -pos_dir, // exterior +side, interior -side
            _ => continue,
        };

        // World position: at the wall surface, centered in YZ (or YX), 0.1 inside.
        let offset = 0.1_f32;
        let pos = match loc.slot {
            Slot::XLoWall => {
                let wall_x = loc.cube.x as f32;
                let ix = if look_dir.x > 0.0 {
                    wall_x + offset
                } else {
                    wall_x - offset
                };
                Vec3::new(ix, loc.cube.y as f32 + 0.5, loc.cube.z as f32 + 0.5)
            }
            Slot::ZLoWall => {
                let wall_z = loc.cube.z as f32;
                let iz = if look_dir.z > 0.0 {
                    wall_z + offset
                } else {
                    wall_z - offset
                };
                Vec3::new(loc.cube.x as f32 + 0.5, loc.cube.y as f32 + 0.5, iz)
            }
            _ => unreachable!(),
        };

        results.push((pos, look_dir));
    }

    results
}

/// Bevy system: despawns and respawns window-based radiosity spotlights.
/// Runs when ConstructedWorld changes.
pub fn update_window_lights(
    mut commands: Commands,
    constructed: Res<ConstructedCity>,
    structure_list: Res<StructureList>,
    existing: Query<Entity, With<WindowLight>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    for (pos, look_dir) in compute_window_lights(&constructed.contents, &structure_list.structures)
    {
        commands.spawn((
            SpotLight {
                color: Color::srgb(0.95, 0.95, 0.9),
                intensity: 50_000.0,
                range: 10.0,
                inner_angle: 20.0_f32.to_radians(),
                outer_angle: 85.0_f32.to_radians(),
                shadows_enabled: false,
                ..default()
            },
            Transform::from_translation(pos).looking_to(look_dir, Vec3::Y),
            WindowLight,
        ));
    }
}

/// Computes world-space positions for interior ceiling lights.
///
/// For each connected span of ceiling tiles (grouped by Y level, then flood-filled
/// for 4-connectivity), finds the grid phase in 0..GRID_PERIOD × 0..GRID_PERIOD that
/// maximises Chebyshev-3 coverage, places lights at grid points that land on actual
/// ceiling tiles, then greedily patches any tiles still uncovered.
pub fn compute_ceiling_lights(contents: &Sparse3D<Cell>) -> Vec<Vec3> {
    let mut ceilings: Vec<(i32, i32, i32)> = contents
        .iter()
        .filter(|(loc, _)| loc.slot == Slot::Floor)
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

    // Hang the light just below the ceiling, in the middle
    light_xz
        .into_iter()
        .map(|(x, z)| Vec3::new(x as f32 + 0.5, y as f32 - 0.05, z as f32 + 0.5))
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
/// Not currently wired into the app — kept for future use.
#[allow(dead_code)]
pub fn update_ceiling_lights(
    mut commands: Commands,
    constructed: Res<ConstructedCity>,
    existing: Query<Entity, With<CeilingLight>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    for pos in compute_ceiling_lights(&constructed.contents) {
        // SpotLight pointing straight down: the exterior top of the ceiling tile is
        // above the light and therefore outside the cone, so it receives no illumination.
        commands.spawn((
            SpotLight {
                color: Color::srgb(1.0, 0.95, 0.8),
                intensity: LIGHT_INTENSITY,
                range: LIGHT_RANGE,
                // Span almost all the way out to 180°, almost full brightness the whole time.
                inner_angle: FRAC_PI_2 - 0.1,
                outer_angle: FRAC_PI_2 - 0.05,
                shadows_enabled: false,
                ..default()
            },
            // looking_to(NEG_Y, X) makes the spotlight's local −Z face world −Y (downward).
            Transform::from_translation(pos).looking_to(Vec3::NEG_Y, Vec3::X),
            CeilingLight,
        ));
    }
}

#[cfg(test)]
mod tests {
    use assert2::check;

    use super::*;

    use bevy::math::IVec3;

    use crate::build_helpers::Builder;
    use crate::sparse3d::RelSlot;
    use crate::structure::load_structure_info;

    /// Three floor planes — Y=1 (ground, excluded by min-y rule), Y=2 (interior
    /// ceiling), Y=3 (roof).  Lights should appear only at Y=2 and Y=3 levels.
    #[test]
    fn test_compute_ceiling_lights_three_levels() {
        let structures = load_structure_info();
        let mut builder = Builder::new(&structures);
        builder.build_plane(
            IVec3::new(0, 1, 0),
            IVec3::new(2, 1, 2),
            RelSlot::Floor,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 2, 0),
            IVec3::new(2, 2, 2),
            RelSlot::Floor,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 3, 0),
            IVec3::new(2, 3, 2),
            RelSlot::Floor,
            None,
        );

        let contents = builder.get();
        let lights = compute_ceiling_lights(&contents);

        check!(!lights.is_empty());

        // No light at ground level (world Y ≈ 0.8).
        let at_ground = lights.iter().filter(|l| (l.y - 0.8).abs() < 0.1).count();
        check!(at_ground == 0);

        // At least one light for the interior ceiling (Y=2 → world Y ≈ 1.95).
        let at_y2 = lights.iter().filter(|l| (l.y - 1.95).abs() < 0.1).count();
        check!(at_y2 >= 1);

        // At least one light for the roof level (Y=3 → world Y ≈ 2.95).
        let at_y3 = lights.iter().filter(|l| (l.y - 2.95).abs() < 0.1).count();
        check!(at_y3 >= 1);

        // Sanity: all lights sit at one of the two expected world-Y heights.
        for l in &lights {
            let near_y2 = (l.y - 1.95).abs() < 0.1;
            let near_y3 = (l.y - 2.95).abs() < 0.1;
            check!(near_y2 || near_y3);
        }
    }

    /// Flat plane (floor at Y=0 only after min-y exclusion: nothing).
    /// No interior ceiling exists, so no lights should be placed.
    #[test]
    fn test_no_lights_flat_plane() {
        let structures = load_structure_info();
        let mut builder = Builder::new(&structures);
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(4, 0, 4),
            RelSlot::Floor,
            None,
        );

        let lights = compute_ceiling_lights(&builder.get());
        check!(lights.is_empty());
    }
}
