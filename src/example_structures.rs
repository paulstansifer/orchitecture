use crate::city::Cell;
use crate::{build_helpers::*, llm_rooms};
use bevy::math::IVec3;

use crate::eorf::load_structure_info;
use crate::sparse3d::{RelSlot, Sparse3D};

fn v(x: i32, y: i32, z: i32) -> IVec3 {
    IVec3::new(x, y, z)
}

pub fn make_structures() -> Vec<Sparse3D<Cell>> {
    let structures = load_structure_info();

    // Plain rectangular room, with minimum height.
    let mut boring_room = Builder::new(&structures);
    boring_room.build_box(v(-3, 0, 0), v(3, 0, 14));
    boring_room.set_vantage(v(0, 0, 2), /*order=*/ 1.0, /*interest=*/ 0.0);

    let mut boring_room_with_alcove = Builder::new(&structures);
    boring_room_with_alcove
        .build_union_boxes(&[(v(-3, 0, 0), v(3, 0, 14)), (v(-2, 0, 2), v(-4, 0, 5))]);
    boring_room_with_alcove.set_vantage(v(0, 0, 2), /*order=*/ 0.7, /*interest=*/ 0.1);

    let mut plus_shaped_room = Builder::new(&structures);
    plus_shaped_room.build_union_boxes(&[(v(-2, 0, -7), v(2, 0, 7)), (v(-7, 0, -2), v(7, 0, 2))]);
    plus_shaped_room.set_vantage(v(0, 0, 0), /*order=*/ 1.0, /*interest=*/ 0.1);

    // Like the plain room, but with pillars along both sides.
    let mut plain_pillar_room = Builder::new(&structures);
    plain_pillar_room.build_box(v(-3, 0, 0), v(3, 0, 14));
    for i in (1..=13).step_by(3) {
        plain_pillar_room.build_box(v(-2, 0, i), v(-2, 0, i));
        plain_pillar_room.build_box(v(2, 0, i), v(2, 0, i));
    }
    plain_pillar_room.set_vantage(v(0, 0, 2), /*order=*/ 0.9, /*interest=*/ 0.25);

    // A taller room with pillars: a little more interesting
    let mut tall_pillar_room = Builder::new(&structures);
    tall_pillar_room.build_box(v(-3, 0, 0), v(3, 3, 14));
    for i in (1..=13).step_by(3) {
        tall_pillar_room.build_box(v(-2, 0, i), v(-2, 3, i));
        tall_pillar_room.build_box(v(2, 0, i), v(2, 3, i));
    }
    tall_pillar_room.set_vantage(v(0, 0, 2), /*order=*/ 0.9, /*interest=*/ 0.4);

    // A corner (made of 3 3x3 squares), and another corner (made of 3 6x6 squares) atop it,
    // with a railing overlooking the gap.
    let mut nested_corners = Builder::new(&structures);
    nested_corners.build_union_boxes(&[
        (v(0, 0, 0), v(5, 0, 2)),
        (v(0, 0, 0), v(2, 0, 5)),
        (v(-5, 1, 0), v(5, 2, 5)),
        (v(-5, 1, 0), v(-1, 2, 11)),
    ]);
    nested_corners.build_plane(v(0, 1, 0), v(0, 1, 5), RelSlot::XLoWall, Some("railing"));
    nested_corners.set_vantage(v(1, 0, 1), /*order=*/ 0.6, /*interest=*/ 0.6);

    let mut gallery = Builder::new(&structures);
    gallery.build_box(v(-3, 0, 0), v(3, 3, 9));

    // Two platforms:
    gallery.build_box(v(-3, 0, 4), v(-2, 0, 9));
    gallery.build_box(v(3, 0, 4), v(2, 0, 9));
    // A bridge connecting the platforms:
    gallery.build_plane(v(-1, 1, 6), v(1, 1, 6), RelSlot::Floor, None);
    // Place railings all around the top surface:
    gallery.wall_off_drops(v(-3, 1, 4), v(3, 1, 9), "railing");
    // Doors into the rooms under the platforms:
    gallery.build_plane(v(-3, 0, 4), v(-3, 0, 4), RelSlot::ZLoWall, Some("doorway"));
    gallery.build_plane(v(3, 0, 4), v(3, 0, 4), RelSlot::ZLoWall, Some("doorway"));
    gallery.build_plane(v(-1, 0, 6), v(-1, 0, 6), RelSlot::XLoWall, Some("doorway"));
    gallery.build_plane(v(1, 0, 6), v(1, 0, 6), RelSlot::XHiWall, Some("doorway"));
    gallery.set_vantage(v(0, 0, 1), /*order=*/ 0.7, /*interest=*/ 0.7);

    let mut res = vec![
        boring_room.get(),
        boring_room_with_alcove.get(),
        plus_shaped_room.get(),
        plain_pillar_room.get(),
        tall_pillar_room.get(),
        nested_corners.get(),
        gallery.get(),
    ];

    res.append(&mut llm_rooms::rooms());

    res
}
