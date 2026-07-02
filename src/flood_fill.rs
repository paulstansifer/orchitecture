use std::collections::{BinaryHeap, HashMap};

use bevy::math::IVec3;

use crate::city::Cell;
use crate::sparse3d::{Slot, SlotCoord, Sparse3D};

/// Returns true if no `Floor` cell exists above `cube` within the grid's
/// bounding box, i.e. `cube` has an unobstructed vertical view of the sky.
/// Handy as a seed predicate for flood fills that propagate from sky-visible
/// cubes (sky illuminance, "outdoorsness", etc).
pub fn has_sky_above(contents: &Sparse3D<Cell>, cube: IVec3, top_y: i32) -> bool {
    for y in (cube.y + 1)..=top_y {
        if contents
            .get(SlotCoord {
                cube: IVec3::new(cube.x, y, cube.z),
                slot: Slot::Floor,
            })
            .is_some()
        {
            return false;
        }
    }
    true
}

/// Heap entry ordered by level (higher = higher priority), with cube
/// and source coordinates as tiebreakers so that `Ord` and `PartialEq` are
/// consistent.
#[derive(PartialEq, Eq)]
struct HeapEntry {
    level_bits: u32,
    cube: IVec3,
    source: IVec3,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.level_bits
            .cmp(&other.level_bits)
            .then_with(|| self.cube.x.cmp(&other.cube.x))
            .then_with(|| self.cube.y.cmp(&other.cube.y))
            .then_with(|| self.cube.z.cmp(&other.cube.z))
            .then_with(|| self.source.x.cmp(&other.source.x))
            .then_with(|| self.source.y.cmp(&other.source.y))
            .then_with(|| self.source.z.cmp(&other.source.z))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Maps a voxel coordinate to a stable value in [0.0, 1.0) via bit-mixing.
/// Handy for adding per-cell jitter to a `falloff` closure without needing
/// to thread an RNG through the fill.
pub fn coord_hash(v: IVec3) -> f32 {
    let mut h = (v.x as u32).wrapping_mul(0x9e3779b9)
        ^ (v.y as u32).wrapping_mul(0x6c62272e)
        ^ (v.z as u32).wrapping_mul(0x517cc1b7);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h as f32 * (1.0 / u32::MAX as f32)
}

const DIRS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

/// Multi-source flood fill over a cubic grid, with per-cell top-`max_sources`
/// contribution tracking and a screen-blend combine (`1 − ∏(1 − cᵢ)`).
///
/// Each of `seeds` is planted at full strength `1.0` and tracked as its own
/// independent source. Propagation uses a max-priority queue so the
/// strongest frontier is always settled first. Each hop from `from` to `to`
/// multiplies the current level by `multiplier(from, to)`; a multiplier of
/// `0.0` blocks that boundary entirely (e.g. a solid wall), while values in
/// between fold in both a material's transmission and any distance falloff.
///
/// Per cell, only the strongest `max_sources` contributions (by level) are
/// retained; the max-heap order guarantees that the first `max_sources`
/// contributions settled for any cell are the strongest ones. The final
/// value per cube is the screen blend of its retained contributions.
///
/// Only cubes within `[search_min, search_max]` (inclusive) are visited.
/// Returns a map from cube coordinate to its combined value in [0.0, 1.0].
pub fn flood_fill<M>(
    seeds: impl IntoIterator<Item = IVec3>,
    search_min: IVec3,
    search_max: IVec3,
    max_sources: usize,
    mut multiplier: M,
) -> HashMap<IVec3, f32>
where
    M: FnMut(IVec3, IVec3) -> f32,
{
    // Per cell: the settled contributions from the strongest `max_sources` sources.
    // Key: cube coordinate → (source position → contribution level).
    let mut contributions: HashMap<IVec3, HashMap<IVec3, f32>> = HashMap::new();
    // BinaryHeap is a max-heap. f32::to_bits() preserves order for non-negative floats.
    let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::new();

    for cube in seeds {
        contributions.entry(cube).or_default().insert(cube, 1.0);
        heap.push(HeapEntry {
            level_bits: 1.0f32.to_bits(),
            cube,
            source: cube,
        });
    }

    while let Some(HeapEntry {
        level_bits,
        cube,
        source,
    }) = heap.pop()
    {
        let level = f32::from_bits(level_bits);

        // Discard stale entries: a better path for this (source, cube) pair was
        // already settled if the stored level is strictly higher than what we popped.
        let settled = contributions
            .get(&cube)
            .and_then(|m| m.get(&source))
            .copied()
            .unwrap_or(0.0);
        if settled > level {
            continue;
        }

        for dir in DIRS {
            let neighbor = cube + dir;
            if neighbor.x < search_min.x
                || neighbor.x > search_max.x
                || neighbor.y < search_min.y
                || neighbor.y > search_max.y
                || neighbor.z < search_min.z
                || neighbor.z > search_max.z
            {
                continue;
            }

            let m = multiplier(cube, neighbor);
            if m == 0.0 {
                continue;
            }
            let new_level = level * m;

            // Decide whether to update (source, neighbor): either improve an existing
            // contribution or add a new one if the cell still has room.
            let current_for_source = contributions
                .get(&neighbor)
                .and_then(|m| m.get(&source))
                .copied();

            let should_update = match current_for_source {
                Some(c) => new_level > c,
                None => {
                    let count = contributions.get(&neighbor).map_or(0, |m| m.len());
                    count < max_sources
                }
            };

            if should_update {
                contributions
                    .entry(neighbor)
                    .or_default()
                    .insert(source, new_level);
                heap.push(HeapEntry {
                    level_bits: new_level.to_bits(),
                    cube: neighbor,
                    source,
                });
            }
        }
    }

    // Combine per-cell contributions with the screen blend: 1 − ∏(1 − cᵢ).
    contributions
        .into_iter()
        .map(|(cube, source_map)| {
            let screen = 1.0 - source_map.values().fold(1.0_f32, |acc, &v| acc * (1.0 - v));
            (cube, screen)
        })
        .collect()
}
