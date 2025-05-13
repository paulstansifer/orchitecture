use crate::build_helpers::*;
use crate::wall_grid::OfflineCell;
use godot::builtin::Vector3i;

use crate::sparse3d::{RelSlot, Sparse3D};
use crate::structure::{self, load_structure_info};

fn v(x: i32, y: i32, z: i32) -> Vector3i {
    Vector3i::new(x, y, z)
}

pub fn make_structures() -> Vec<Sparse3D<OfflineCell>> {
    let structures = load_structure_info();

    // Plain rectangular room, with minimum height.
    let mut boring_room = Builder::new(&structures);
    boring_room.build_box(v(-3, 0, 0), v(3, 0, 14));
    boring_room.set_vantage(v(0, 0, 2), /*symmetry=*/ 1.0, /*interest=*/ 0.0);

    let mut plus_shaped_room = Builder::new(&structures);
    plus_shaped_room.build_union_boxes(&[(v(-2, 0, -7), v(2, 0, 7)), (v(-7, 0, -2), v(7, 0, 2))]);
    plus_shaped_room.set_vantage(v(0, 0, 0), /*symmetry=*/ 1.0, /*interest=*/ 0.1);

    // Like the plain room, but with pillars along both sides.
    let mut plain_pillar_room = Builder::new(&structures);
    plain_pillar_room.build_box(v(-3, 0, 0), v(3, 0, 14));
    for i in (1..13).step_by(3) {
        plain_pillar_room.build_box(v(-2, 0, i), v(-2, 0, i));
        plain_pillar_room.build_box(v(2, 0, i), v(2, 0, i));
    }
    plain_pillar_room.set_vantage(v(0, 0, 2), /*symmetry=*/ 0.9, /*interest=*/ 0.25);

    // A taller room with pillars: a little more interesting
    let mut tall_pillar_room = Builder::new(&structures);
    tall_pillar_room.build_box(v(-3, 0, 0), v(3, 3, 14));
    for i in (1..13).step_by(3) {
        tall_pillar_room.build_box(v(-2, 0, i), v(-2, 3, i));
        tall_pillar_room.build_box(v(2, 0, i), v(2, 3, i));
    }
    tall_pillar_room.set_vantage(v(0, 0, 2), /*symmetry=*/ 0.9, /*interest=*/ 0.4);

    // A corner (made of 3 3x3 squares), and another corner (made of 3 6x6 squares) atop it,
    // with a railing overlooking the gap.
    let mut nested_corners = Builder::new(&structures);
    nested_corners.build_union_boxes(&[
        (v(0, 0, 0), v(5, 0, 2)),
        (v(0, 0, 0), v(2, 0, 5)),
        (v(0, 1, 0), v(11, 2, 5)),
        (v(11, 1, 0), v(5, 2, 11)),
    ]);
    nested_corners.build_plane(v(0, 1, 5), v(5, 1, 5), RelSlot::ZHiWall, Some("railing"));
    nested_corners.set_vantage(v(1, 0, 1), /*symmetry=*/ 0.6, /*interest=*/ 0.6);

    vec![
        boring_room.get(),
        plus_shaped_room.get(),
        plain_pillar_room.get(),
        tall_pillar_room.get(),
        nested_corners.get(),
    ]
}
