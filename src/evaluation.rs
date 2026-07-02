use std::collections::HashMap;

use bevy::math::IVec3;

use crate::city::Cell;
use crate::flood_fill::{flood_fill, has_sky_above};
use crate::sparse3d::{SlotCoord, Sparse3D};
use crate::structure::StructureInfo;

/// Falloff per hop through open air (and through any structure that isn't a
/// window or a doorway).
const FALLOFF: f32 = 0.3;
/// Downward hops don't attenuate: a cube directly under a sky-visible cube is
/// just as "outdoors" as the cube above it.
const FALLOFF_DOWNWARD: f32 = 0.0;
/// Windows let light through but not weather/access: they block outdoorsness
/// entirely.
const FALLOFF_WINDOW: f32 = 1.0;
/// Doors are a partial barrier to outdoorsness.
const FALLOFF_DOORWAY: f32 = 0.7;

/// Returns the per-hop outdoorsness multiplier for the boundary between
/// adjacent cubes `from` and `to`: `0.0` blocks propagation entirely (walls,
/// floors, and windows all fully block outdoorsness), otherwise `1 -
/// falloff`.
fn boundary_multiplier(
    contents: &Sparse3D<Cell>,
    structures: &[StructureInfo],
    from: IVec3,
    to: IVec3,
) -> f32 {
    let falloff = match contents.get(SlotCoord::boundary(from, to)) {
        Some(cell) => match structures[cell.id.as_usize()].name.as_str() {
            "wall" | "floor" | "window" => FALLOFF_WINDOW, // 1.0: fully blocks
            "doorway" => FALLOFF_DOORWAY,
            _ => default_falloff(from, to),
        },
        None => default_falloff(from, to),
    };
    1.0 - falloff
}

fn default_falloff(from: IVec3, to: IVec3) -> f32 {
    if to - from == IVec3::NEG_Y {
        FALLOFF_DOWNWARD
    } else {
        FALLOFF
    }
}

/// Flood-fills "outdoorsness" from sky-visible cube voxels: 1.0 = fully
/// outdoors, falling off towards 0.0 the further a cube is from open sky.
///
/// Seeds all cubes within the grid bounding box (expanded by 1) that have an
/// unobstructed vertical view of the sky (see [`has_sky_above`]), then
/// propagates outward. Each hop through open air multiplies the current
/// level by `1 - FALLOFF` (no falloff going straight down); doorways use a
/// steeper `1 - FALLOFF_DOORWAY`, and windows block outdoorsness entirely
/// (`1 - FALLOFF_WINDOW == 0`) even though they don't block light. Walls and
/// floors block propagation outright.
///
/// Returns a map from cube coordinate to outdoorsness in [0.0, 1.0].
pub fn compute_outdoorsness(
    contents: &Sparse3D<Cell>,
    structures: &[StructureInfo],
) -> HashMap<IVec3, f32> {
    if contents.size() == 0 {
        return HashMap::new();
    }

    let (min_cube, max_cube) = contents.bounding_box();
    let search_min = min_cube - IVec3::ONE;
    let search_max = max_cube + IVec3::ONE;
    let top_y = search_max.y;

    let mut seeds = Vec::new();
    for z in search_min.z..=search_max.z {
        for y in search_min.y..=search_max.y {
            for x in search_min.x..=search_max.x {
                let cube = IVec3::new(x, y, z);
                if has_sky_above(contents, cube, top_y) {
                    seeds.push(cube);
                }
            }
        }
    }

    flood_fill(
        seeds,
        search_min,
        search_max,
        /*max_sources=*/ 3,
        |from, to| boundary_multiplier(contents, structures, from, to),
    )
}

#[cfg(test)]
mod tests {
    use assert2::check;
    use bevy::math::IVec3;

    use crate::build_helpers::Builder;
    use crate::sparse3d::RelSlot;
    use crate::structure::load_structure_info;

    use super::compute_outdoorsness;

    /// A sealed box has opaque walls and a roof on all sides. No outdoorsness
    /// should reach any interior cube.
    #[test]
    fn test_sealed_box_has_no_interior_outdoorsness() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        let contents = builder.get();

        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);

        let interior = IVec3::new(1, 1, 1);
        check!(outdoorsness.get(&interior).copied().unwrap_or(0.0) == 0.0);
    }

    /// An open cube (nothing above it) seeds itself as fully outdoors.
    #[test]
    fn test_open_cube_is_fully_outdoors() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 0, 0),
            RelSlot::Floor,
            None,
        );
        let contents = builder.get();
        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);
        check!(
            outdoorsness
                .get(&IVec3::new(0, 0, 0))
                .copied()
                .unwrap_or(0.0)
                == 1.0
        );
    }

    /// A window in the wall admits light but should not admit any
    /// outdoorsness at all.
    #[test]
    fn test_window_blocks_outdoorsness() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        builder.build_plane(
            IVec3::new(0, 1, 1),
            IVec3::new(0, 1, 1),
            RelSlot::XLoWall,
            Some("window"),
        );
        let contents = builder.get();

        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);
        let at_window = IVec3::new(0, 1, 1);
        check!(outdoorsness.get(&at_window).copied().unwrap_or(0.0) == 0.0);
    }

    /// A doorway admits partial outdoorsness (less than full, but nonzero).
    #[test]
    fn test_doorway_admits_partial_outdoorsness() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        builder.build_plane(
            IVec3::new(0, 1, 1),
            IVec3::new(0, 1, 1),
            RelSlot::XLoWall,
            Some("doorway"),
        );
        let contents = builder.get();

        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);
        let at_doorway = IVec3::new(0, 1, 1);
        let level = outdoorsness.get(&at_doorway).copied().unwrap_or(0.0);
        check!(level > 0.0);
        check!(level < 1.0);
    }

    /// A cube directly beneath a sky-visible cube suffers no falloff, even
    /// after multiple hops straight down. Walled on all sides so the only
    /// route in is the open top, isolating the vertical hop from horizontal
    /// falloff.
    #[test]
    fn test_downward_propagation_has_no_falloff() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 2, 0),
            RelSlot::XLoWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 2, 0),
            RelSlot::XHiWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 2, 0),
            RelSlot::ZLoWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 2, 0),
            RelSlot::ZHiWall,
            None,
        );
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 0, 0),
            RelSlot::Floor,
            None,
        );
        let contents = builder.get();
        let outdoorsness = compute_outdoorsness(&contents, &structure_infos);

        check!(
            outdoorsness
                .get(&IVec3::new(0, 2, 0))
                .copied()
                .unwrap_or(0.0)
                == 1.0
        );
        check!(
            outdoorsness
                .get(&IVec3::new(0, 1, 0))
                .copied()
                .unwrap_or(0.0)
                == 1.0
        );
        check!(
            outdoorsness
                .get(&IVec3::new(0, 0, 0))
                .copied()
                .unwrap_or(0.0)
                == 1.0
        );
    }
}
