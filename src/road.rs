use bevy::math::IVec3;

use crate::sparse3d::{Slot, SlotCoord};

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
    let in_ew = (0..ROAD_WIDTH).contains(&z);
    let in_north = (0..ROAD_WIDTH).contains(&x) && z >= ROAD_WIDTH;

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
pub fn is_in_road_forbidden_zone(loc: SlotCoord) -> bool {
    let x = loc.cube.x;
    let y = loc.cube.y;
    let z = loc.cube.z;

    let cube_forbidden = |cx: i32, cy: i32, cz: i32| cy < road_forbidden_height(cx, cz);

    match loc.slot {
        Slot::Room | Slot::Floor => cube_forbidden(x, y, z),
        // Walls are between two cubes; forbidden only if both sides are inside the zone.
        Slot::ZLoWall => cube_forbidden(x, y, z - 1) && cube_forbidden(x, y, z),
        Slot::XLoWall => cube_forbidden(x - 1, y, z) && cube_forbidden(x, y, z),
    }
}

/// Offset applied to buildings when loading from file, placing them in the
/// no-road semiplane (z < 0, south of the E-W road).
pub const BUILDING_LOAD_OFFSET: IVec3 = IVec3::new(0, 0, -6);

#[cfg(test)]
mod tests {
    use assert2::check;
    use bevy::math::IVec3;

    use crate::sparse3d::{Slot, SlotCoord};

    use super::{is_in_road_forbidden_zone, road_forbidden_height, ROAD_WIDTH};

    fn loc(x: i32, y: i32, z: i32, slot: Slot) -> SlotCoord {
        SlotCoord {
            cube: IVec3::new(x, y, z),
            slot,
        }
    }

    #[test]
    fn road_forbidden_zone() {
        // ── road_forbidden_height ──────────────────────────────────────────

        // South of EW road and fully outside: no restriction
        check!(road_forbidden_height(0, -1) == 0);
        check!(road_forbidden_height(-5, 10) == 0);

        // EW road boundary rows (z == 0 and z == ROAD_WIDTH-1): lower ceiling
        check!(road_forbidden_height(0, 0) == 3);
        check!(road_forbidden_height(5, ROAD_WIDTH - 1) == 3);

        // EW road interior rows (1 <= z <= ROAD_WIDTH-2): full ceiling
        check!(road_forbidden_height(0, 1) == 4);
        check!(road_forbidden_height(0, ROAD_WIDTH - 2) == 4);

        // North arm boundary columns (x == 0 or x == ROAD_WIDTH-1): lower ceiling
        check!(road_forbidden_height(0, ROAD_WIDTH) == 3);
        check!(road_forbidden_height(ROAD_WIDTH - 1, ROAD_WIDTH + 2) == 3);

        // North arm interior columns: full ceiling
        check!(road_forbidden_height(1, ROAD_WIDTH) == 4);
        check!(road_forbidden_height(ROAD_WIDTH - 2, ROAD_WIDTH + 5) == 4);

        // ── Room and Floor: same cube-level logic ──────────────────────────

        // Below height limit inside EW road interior: forbidden
        check!(is_in_road_forbidden_zone(loc(0, 0, 1, Slot::Room)));
        check!(is_in_road_forbidden_zone(loc(0, 0, 1, Slot::Floor)));

        // At the height limit: allowed (y must be strictly less than height)
        check!(!is_in_road_forbidden_zone(loc(0, 4, 1, Slot::Room)));
        check!(!is_in_road_forbidden_zone(loc(0, 4, 1, Slot::Floor)));

        // South of EW road: always allowed
        check!(!is_in_road_forbidden_zone(loc(0, 0, -1, Slot::Room)));

        // ── ZLoWall: boundary between cube(z-1) and cube(z) ───────────────

        // Wall at z=0: one side is south (z=-1, outside) → only one cube
        // is forbidden → wall is NOT forbidden
        check!(!is_in_road_forbidden_zone(loc(0, 0, 0, Slot::ZLoWall)));

        // Wall at z=2 (inside EW interior on both sides): both cubes forbidden
        check!(is_in_road_forbidden_zone(loc(0, 0, 2, Slot::ZLoWall)));

        // Above height limit: allowed
        check!(!is_in_road_forbidden_zone(loc(0, 4, 1, Slot::ZLoWall)));

        // ── XLoWall: boundary between cube(x-1) and cube(x) ──────────────

        // Wall at x=0 inside north arm: x-1=-1 is outside the arm → NOT forbidden
        check!(!is_in_road_forbidden_zone(loc(
            0,
            0,
            ROAD_WIDTH + 1,
            Slot::XLoWall
        )));

        // Wall at x=2 inside north arm: both x=1 and x=2 are interior → IS forbidden
        check!(is_in_road_forbidden_zone(loc(
            2,
            0,
            ROAD_WIDTH + 1,
            Slot::XLoWall
        )));
    }
}
