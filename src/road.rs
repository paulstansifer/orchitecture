use bevy::math::IVec3;

use crate::sparse3d::{RelSlot, SlotLocation};

/// Width of each road arm in grid units.
pub const ROAD_WIDTH: i32 = 4;

/// The T-intersection occupies:
///   - East-West road: z in [0, ROAD_WIDTH) for all x
///   - North arm:      x in [0, ROAD_WIDTH) for z >= ROAD_WIDTH
///
/// The no-road semiplane is z < 0 (south of the E-W road).

/// Returns the lowest y at which construction is permitted above this (x, z) column.
/// y values strictly below the returned value are in the forbidden zone.
/// Returns 0 for columns outside the road (no restriction).
pub fn road_forbidden_height(x: i32, z: i32) -> i32 {
    let in_ew = z >= 0 && z < ROAD_WIDTH;
    let in_north = x >= 0 && x < ROAD_WIDTH && z >= ROAD_WIDTH;

    if in_ew {
        if z == 0 || z == ROAD_WIDTH - 1 {
            3
        } else {
            4
        }
    } else if in_north {
        if x == 0 || x == ROAD_WIDTH - 1 {
            3
        } else {
            4
        }
    } else {
        0
    }
}

/// Returns true if `loc` is inside the road's forbidden zone, taking slot geometry
/// into account so that boundary slots (walls flush with the road edge, floors/ceilings
/// flush with the vertical limit) are permitted.
pub fn is_in_road_forbidden_zone(loc: SlotLocation) -> bool {
    // This implementation probably could be simplified.
    let x = loc.cube.x;
    let y = loc.cube.y;
    let z = loc.cube.z;

    let cube_forbidden = |cx: i32, cy: i32, cz: i32| cy < road_forbidden_height(cx, cz);

    match loc.rel_slot {
        RelSlot::Room | RelSlot::Floor => cube_forbidden(x, y, z),
        // Ceiling surface is at y+1; flush when y+1 == limit.
        RelSlot::Ceiling => y + 1 < road_forbidden_height(x, z),
        // Walls are between two cubes; forbidden only if both sides are inside the zone.
        RelSlot::ZLoWall => cube_forbidden(x, y, z - 1) && cube_forbidden(x, y, z),
        RelSlot::ZHiWall => cube_forbidden(x, y, z) && cube_forbidden(x, y, z + 1),
        RelSlot::XLoWall => cube_forbidden(x - 1, y, z) && cube_forbidden(x, y, z),
        RelSlot::XHiWall => cube_forbidden(x, y, z) && cube_forbidden(x + 1, y, z),
    }
}

/// Offset applied to buildings when loading from file, placing them in the
/// no-road semiplane (z < 0, south of the E-W road).
pub const BUILDING_LOAD_OFFSET: IVec3 = IVec3::new(0, 0, -6);
