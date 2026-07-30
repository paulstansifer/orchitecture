//! The "Starter town" button in the bottom bar: replaces the whole map with a
//! small pre-furnished settlement (a storage room, a bookroom, three
//! bedrooms, and some open-air market stands), for quickly getting into a
//! populated-looking city without building everything by hand.

use bevy::math::IVec3;

use crate::build_helpers::Builder;
use crate::city::Cell;
use crate::eorf::EorfInfo;
use crate::sparse3d::{RelSlot, Sparse3D};

fn v(x: i32, y: i32, z: i32) -> IVec3 {
    IVec3::new(x, y, z)
}

/// A box building with a single door on its south (min-Z) wall at `door_x`.
fn build_box_with_door(builder: &mut Builder, corner_a: IVec3, corner_b: IVec3, door_x: i32) {
    builder.build_box(corner_a, corner_b);
    let min = IVec3::min(corner_a, corner_b);
    builder.build_plane(
        v(door_x, min.y, min.z),
        v(door_x, min.y, min.z),
        RelSlot::ZLoWall,
        Some("doorway"),
    );
}

/// Builds the starter town: a storage room (5 bins, 3 racks), a bookroom (2
/// bookcases, 1 chair), three bedrooms (1 pallet each) -- each in its own box
/// building with a door -- plus 4 open-air market stands.
pub fn build_starter_town(eorfs: &[EorfInfo]) -> Sparse3D<Cell> {
    let mut builder = Builder::new(eorfs);

    // Storage room: 5 bins, 3 racks.
    build_box_with_door(&mut builder, v(0, 0, 0), v(4, 0, 3), 3);
    for x in 0..=4 {
        builder.room_plop(v(x, 0, 1), "bin");
    }
    for x in 1..=3 {
        builder.room_plop(v(x, 0, 2), "rack");
    }

    // Bookroom: 2 bookcases, 1 chair.
    build_box_with_door(&mut builder, v(7, 0, 0), v(11, 0, 3), 11);
    builder.room_plop(v(8, 0, 1), "bookcase");
    builder.room_plop(v(10, 0, 1), "bookcase");
    builder.build_plane(v(9, 0, 1), v(9, 0, 1), RelSlot::XHiWall, Some("chair"));

    // Three bedrooms, each with a pallet.
    for base_x in [13, 16, 19] {
        build_box_with_door(
            &mut builder,
            v(base_x, 0, 0),
            v(base_x + 2, 0, 2),
            base_x + 1,
        );
        builder.room_plop(v(base_x + 1, 0, 1), "pallet");
    }

    for x in [3, 5, 7, 9] {
        builder.room_plop(v(x, 0, -3), "market stand");
    }

    // Slightly fancier building (TODO: add more, and maybe turn it into an inn)
    build_box_with_door(&mut builder, v(15, 0, 8), v(20, 2, 14), 17);
    builder.build_plane(v(15,3,8), v(20,3,14), RelSlot::Floor, Some("roof"));

    builder.build_plane(v(15,1,11), v(19,1,14), RelSlot::Floor, Some("floor"));
    builder.wall_off_drops(v(16,1,11), v(19,1,14), "railing");
    builder.room_plop(v(15,0,10), "stairs");

    build_box_with_door(&mut builder, v(17, 1, 12), v(18, 1, 14), 17);


    builder.get()
}
