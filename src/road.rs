use bevy::math::IVec3;

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
        if z == 0 || z == ROAD_WIDTH - 1 { 3 } else { 4 }
    } else if in_north {
        if x == 0 || x == ROAD_WIDTH - 1 { 3 } else { 4 }
    } else {
        0
    }
}

pub fn is_in_road_forbidden_zone(x: i32, y: i32, z: i32) -> bool {
    y < road_forbidden_height(x, z)
}

/// Offset applied to buildings when loading from file, placing them in the
/// no-road semiplane (z < 0, south of the E-W road).
pub const BUILDING_LOAD_OFFSET: IVec3 = IVec3::new(0, 0, -6);
