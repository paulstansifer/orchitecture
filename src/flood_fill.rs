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
/// coordinates as a tiebreaker so that `Ord` and `PartialEq` are consistent.
#[derive(PartialEq, Eq)]
struct HeapEntry {
    level_bits: u32,
    cube: IVec3,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.level_bits
            .cmp(&other.level_bits)
            .then_with(|| self.cube.x.cmp(&other.cube.x))
            .then_with(|| self.cube.y.cmp(&other.cube.y))
            .then_with(|| self.cube.z.cmp(&other.cube.z))
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

/// Multi-source flood fill over a cubic grid: each cell's value is the
/// strongest (max) level reaching it from any seed.
///
/// Each of `seeds` is planted at full strength `1.0`. Propagation uses a
/// max-priority queue so the strongest frontier is always settled first.
/// Each hop from `from` to `to` multiplies the current level by
/// `multiplier(from, to)`; a multiplier of `0.0` blocks that boundary
/// entirely (e.g. a solid wall), while values in between fold in both a
/// material's transmission and any distance falloff.
///
/// Since every multiplier is in `[0.0, 1.0]`, level is non-increasing along
/// any path, so the max-heap order guarantees the first time a cell is
/// popped, it's been reached by the strongest possible path — no per-source
/// bookkeeping is needed, unlike a screen-blend combine of multiple
/// independent sources (which would double-count a single physical source
/// that happens to reach a cell via more than one seed, e.g. several
/// sky-visible cubes stacked in an unobstructed column).
///
/// Only cubes within `[search_min, search_max]` (inclusive) are visited.
/// Returns a map from cube coordinate to its level in [0.0, 1.0].
pub fn flood_fill<M>(
    seeds: impl IntoIterator<Item = IVec3>,
    search_min: IVec3,
    search_max: IVec3,
    mut multiplier: M,
) -> HashMap<IVec3, f32>
where
    M: FnMut(IVec3, IVec3) -> f32,
{
    let mut settled: HashMap<IVec3, f32> = HashMap::new();
    // BinaryHeap is a max-heap. f32::to_bits() preserves order for non-negative floats.
    let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::new();

    for cube in seeds {
        if settled.insert(cube, 1.0).is_none() {
            heap.push(HeapEntry {
                level_bits: 1.0f32.to_bits(),
                cube,
            });
        }
    }

    while let Some(HeapEntry {
        level_bits, cube, ..
    }) = heap.pop()
    {
        let level = f32::from_bits(level_bits);

        // Discard stale entries: a better (higher) level for this cell was
        // already settled if it doesn't match what we popped.
        if settled.get(&cube) != Some(&level) {
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

            let should_update = match settled.get(&neighbor) {
                Some(&c) => new_level > c,
                None => true,
            };

            if should_update {
                settled.insert(neighbor, new_level);
                heap.push(HeapEntry {
                    level_bits: new_level.to_bits(),
                    cube: neighbor,
                });
            }
        }
    }

    settled
}
