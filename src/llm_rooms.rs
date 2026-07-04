// This was worth a shot, but they aren't great.

// Explanation of the API for the LLMs:

/*


// Builds a flat plane between `corner_a` and `corner_b` (inclusive), on the `slot` face of the
// cubes. (The coordinates must be equal in the appropriate dimension for `slot`.) If `obj_name` is
// `None`, uses walls or floors, but it can also be "railing" or "doorway".
fn build_plane(
        &mut self,
        corner_a: IVec3,
        corner_b: IVec3,
        slot: RelSlot,
        obj_name: Option<&str>,
    );

// Encloses the volume between corner_a and corner_b (inclusive) in walls and floors/ceilings.
fn build_box(&mut self, corner_a: IVec3, corner_b: IVec3);

// Encloses a volume that's contained within the boxes defined by `boxes`, with no internal walls.
// (This is like a CSG union operation)
fn build_union_boxes(&mut self, boxes: &[(IVec3, IVec3)]);

// For the plane between `corner_a` and `corner_b` (their Y coordinates must be equal), identify
// any floors that are adjacent to a lack of floor, and adds `obj_name` in the wall slot between
// them, if none is present.
fn wall_off_drops(&mut self, corner_a: IVec3, corner_b: IVec3, obj_name: &str)

// Sets a viewpoint with the given aesthetic evaluations.
fn set_vantage(&mut self, loc: IVec3, coherence: f32, interest: f32)
*/
use crate::build_helpers::*;
use crate::city::Cell;
use bevy::math::IVec3;

use crate::eorf::load_structure_info;
use crate::sparse3d::{RelSlot, Sparse3D};

fn v(x: i32, y: i32, z: i32) -> IVec3 {
    IVec3::new(x, y, z)
}

pub fn rooms() -> Vec<Sparse3D<Cell>> {
    let structures = load_structure_info();

    // -- Gemini --

    // Design 1: Sunken Courtyard with Overlook
    // A main room at a lower level (y=0) with an elevated platform/room (y=1) overlooking it.
    // This creates interesting sightlines and a sense of varied elevation.
    let mut sunken_courtyard = Builder::new(&structures);
    sunken_courtyard.build_union_boxes(&[
        // Lower courtyard area: 1 cube high (floor at y=0, ceiling at y=0)
        (v(-5, 0, -5), v(5, 0, 5)),
        // Elevated overlook platform: 1 cube high (floor at y=1, ceiling at y=1)
        // Positioned to one side of the courtyard.
        (v(-2, 1, -5), v(2, 1, -2)),
        // Small room/structure on the overlook
        (v(-1, 1, -4), v(1, 2, -3)), // This part is 2 cubes high, floor at y=1, ceiling at y=2
    ]);
    // Add railings around the overlook platform (on the y=1 plane)
    // The check area covers the extent of potential floors at y=1.
    sunken_courtyard.wall_off_drops(v(-5, 1, -5), v(5, 1, 5), "railing");
    // Vantage point in the lower courtyard, looking towards the overlook.
    sunken_courtyard.set_vantage(v(0, 0, 0), /*coherence=*/ 0.5, /*interest=*/ 0.65);

    // Design 2: Grand Library Hall
    // A tall central hall flanked by multi-level balconies or walkways, suggesting bookshelves.
    // Aims for high coherence and high interest due to verticality and complexity.
    let mut library_hall = Builder::new(&structures);
    library_hall.build_union_boxes(&[
        // Main ground floor area (nave + side aisles): 2 cubes high (floor y=0, ceiling y=1)
        (v(-6, 0, -10), v(6, 1, 10)),
        // Second level walkways (left and right): 1 cube high (floor y=2, ceiling y=2)
        (v(-6, 2, -9), v(-4, 2, 9)), // Left walkway
        (v(4, 2, -9), v(6, 2, 9)),   // Right walkway
        // Third level walkways (left and right): 1 cube high (floor y=4, ceiling y=4)
        (v(-6, 4, -9), v(-4, 4, 9)), // Left walkway
        (v(4, 4, -9), v(6, 4, 9)),   // Right walkway
        // Upper central nave airspace (above ground floor, between walkways):
        // Extends from y=2 up to y=5 (4 cubes high section)
        (v(-3, 2, -10), v(3, 5, 10)),
    ]);
    // Add railings for the second level walkways (on y=2 plane)
    library_hall.wall_off_drops(v(-6, 2, -10), v(6, 2, 10), "railing");
    // Add railings for the third level walkways (on y=4 plane)
    library_hall.wall_off_drops(v(-6, 4, -10), v(6, 4, 10), "railing");
    // Vantage point from the center of the ground floor.
    library_hall.set_vantage(v(0, 0, 0), /*coherence=*/ 0.9, /*interest=*/ 0.85);

    // Design 3: Stepped Pyramid Core
    // A large room containing a central, multi-tiered pyramid structure.
    // The pyramid itself is made of solid blocks, creating walkable steps.
    let mut pyramid_core_room = Builder::new(&structures);
    // Define the outer boundary of the room, 6 cubes high (floor y=0, ceiling y=5)
    pyramid_core_room.build_box(v(-8, 0, -8), v(8, 5, 8));

    // Build the stepped pyramid inside the room using solid boxes.
    // Each box represents a tier of the pyramid.
    // Base of the pyramid (floor at y=0, 1 cube high)
    pyramid_core_room.build_box(v(-4, 0, -4), v(4, 0, 4));
    // Second tier (sits on base, floor at y=1, 1 cube high)
    pyramid_core_room.build_box(v(-3, 1, -3), v(3, 1, 3));
    // Third tier (sits on second tier, floor at y=2, 1 cube high)
    pyramid_core_room.build_box(v(-2, 2, -2), v(2, 2, 2));
    // Fourth tier (sits on third tier, floor at y=3, 1 cube high)
    pyramid_core_room.build_box(v(-1, 3, -1), v(1, 3, 1));
    // Top platform (sits on fourth tier, floor at y=4, 1 cube high)
    pyramid_core_room.build_box(v(0, 4, 0), v(0, 4, 0));

    // Add railings to the edges of each tier of the pyramid.
    // Note: If steps are only 1 unit high, railings might be optional, but can add detail.
    pyramid_core_room.wall_off_drops(v(-4, 0, -4), v(4, 0, 4), "railing"); // Railings for base edge if it's near a drop within the larger room (unlikely with current setup but good practice)
    pyramid_core_room.wall_off_drops(v(-3, 1, -3), v(3, 1, 3), "railing"); // Railings for 2nd tier
    pyramid_core_room.wall_off_drops(v(-2, 2, -2), v(2, 2, 2), "railing"); // Railings for 3rd tier
    pyramid_core_room.wall_off_drops(v(-1, 3, -1), v(1, 3, 1), "railing"); // Railings for 4th tier
                                                                           // No railings needed for the very top platform if it's just 1x1.

    // Vantage point at the base of the room, looking at the pyramid.
    pyramid_core_room.set_vantage(v(0, 0, 6), /*coherence=*/ 0.8, /*interest=*/ 0.7);

    // Design 4: Chasm Crossing
    // Two distinct platforms/areas separated by a chasm, connected by a narrow bridge.
    // High interest due to the perceived danger and the structure of the bridge.
    let mut chasm_crossing = Builder::new(&structures);
    chasm_crossing.build_union_boxes(&[
        // West platform: 2 cubes high (floor y=0, ceiling y=1)
        (v(-10, 0, -5), v(-3, 1, 5)),
        // East platform: 2 cubes high (floor y=0, ceiling y=1)
        (v(3, 0, -5), v(10, 1, 5)),
        // Bridge structure: 1 cube wide, 1 cube high (floor y=0, ceiling y=0)
        // Spans from x=-2 to x=2, at z=0.
        (v(-2, 0, -0), v(2, 0, 0)),
    ]);
    // Add railings along all exposed edges at y=0. This will cover platform edges facing the chasm
    // and the sides of the bridge.
    // The area checked should encompass all y=0 floors.
    chasm_crossing.wall_off_drops(v(-10, 0, -5), v(10, 0, 5), "railing");
    // Vantage point on one platform, looking across towards the bridge and other platform.
    chasm_crossing.set_vantage(v(-8, 0, 0), /*coherence=*/ 0.4, /*interest=*/ 0.8);

    // -- GPT --

    let mut stacked_balconies = Builder::new(&structures);
    stacked_balconies.build_box(v(-5, 0, -5), v(5, 4, 5)); // Main tall box

    // First balcony (floor 1)
    stacked_balconies.build_box(v(-5, 1, -5), v(-3, 1, 5));
    stacked_balconies.build_plane(v(-3, 1, -5), v(-3, 1, 5), RelSlot::XLoWall, Some("railing"));

    // Second balcony (floor 2)
    stacked_balconies.build_box(v(3, 2, -5), v(5, 2, 5));
    stacked_balconies.build_plane(v(3, 2, -5), v(3, 2, 5), RelSlot::XHiWall, Some("railing"));

    // Third balcony (floor 3)
    stacked_balconies.build_box(v(-5, 3, -2), v(-3, 3, 2));
    stacked_balconies.build_plane(v(-3, 3, -2), v(-3, 3, 2), RelSlot::XLoWall, Some("railing"));

    // Add central bridge at top level
    stacked_balconies.build_plane(v(-1, 4, 0), v(1, 4, 0), RelSlot::Floor, None);
    stacked_balconies.wall_off_drops(v(-1, 4, 0), v(1, 4, 0), "railing");

    stacked_balconies.set_vantage(v(0, 0, 0), 0.6, 0.9);

    let mut sunken_cross = Builder::new(&structures);
    // Upper level
    sunken_cross.build_box(v(-7, 0, -7), v(7, 0, 7));

    // Lower sunken cross
    sunken_cross.build_union_boxes(&[(v(-1, -1, -5), v(1, -1, 5)), (v(-5, -1, -1), v(5, -1, 1))]);

    // Walls around the drop with railing
    sunken_cross.wall_off_drops(v(-7, 0, -7), v(7, 0, 7), "railing");

    // Add bridges across the sunken area
    sunken_cross.build_plane(v(-1, 0, 0), v(1, 0, 0), RelSlot::Floor, None);
    sunken_cross.build_plane(v(0, 0, -1), v(0, 0, 1), RelSlot::Floor, None);

    sunken_cross.set_vantage(v(0, 0, 6), 1.0, 0.85);

    let mut diagonal_forest = Builder::new(&structures);
    diagonal_forest.build_box(v(-6, 0, -6), v(6, 3, 6));

    // Pillars placed diagonally
    for i in -2..=2 {
        diagonal_forest.build_box(v(i * 2, 0, i * 2), v(i * 2, 3, i * 2));
        diagonal_forest.build_box(v(i * 2 + 1, 0, i * 2 - 1), v(i * 2 + 1, 3, i * 2 - 1));
    }

    diagonal_forest.set_vantage(v(0, 0, -5), 0.5, 0.95);

    // -- Mistral --

    let mut spiral_room = Builder::new(&structures);
    spiral_room.build_box(v(-5, 0, -5), v(5, 4, 5));

    // Create a spiral staircase
    for i in 0..4 {
        let height = i;
        let radius = 4 - i;
        spiral_room.build_plane(
            v(-radius, height, -radius),
            v(-radius, height, radius),
            RelSlot::Floor,
            None,
        );
        spiral_room.build_plane(
            v(-radius, height, radius),
            v(radius, height, radius),
            RelSlot::Floor,
            None,
        );
        spiral_room.build_plane(
            v(radius, height, radius),
            v(radius, height, -radius),
            RelSlot::Floor,
            None,
        );
        spiral_room.build_plane(
            v(radius, height, -radius),
            v(-radius, height, -radius),
            RelSlot::Floor,
            None,
        );
    }

    spiral_room.set_vantage(v(0, 2, 0), /*coherence=*/ 0.8, /*interest=*/ 0.8);

    let mut checkerboard_room = Builder::new(&structures);
    checkerboard_room.build_box(v(-5, 0, -5), v(5, 3, 5));

    // Create a checkerboard pattern of pillars
    for x in (-4..=4).step_by(2) {
        for z in (-4..=4).step_by(2) {
            checkerboard_room.build_box(v(x, 0, z), v(x, 3, z));
        }
    }

    checkerboard_room.set_vantage(v(0, 1, 0), /*coherence=*/ 0.9, /*interest=*/ 0.7);

    let mut terrace_room = Builder::new(&structures);
    terrace_room.build_box(v(-5, 0, -5), v(5, 4, 5));

    // Create multi-level terraces
    for i in 0..4 {
        let height = i;
        let size = 5 - i;
        terrace_room.build_box(v(-size, height, -size), v(size, height, size));
    }

    terrace_room.set_vantage(v(0, 2, 0), /*coherence=*/ 0.8, /*interest=*/ 0.9);

    let mut concentric_room = Builder::new(&structures);
    concentric_room.build_box(v(-5, 0, -5), v(5, 3, 5));

    // Create concentric circles (approximated with squares)
    for i in 0..3 {
        let size = 5 - i;
        concentric_room.build_box(v(-size, 0, -size), v(size, 3, size));
    }

    concentric_room.set_vantage(v(0, 1, 0), /*coherence=*/ 1.0, /*interest=*/ 0.7);

    let mut labyrinth_room = Builder::new(&structures);
    labyrinth_room.build_box(v(-5, 0, -5), v(5, 3, 5));

    // Create a labyrinth pattern
    for x in (-4..=4).step_by(2) {
        for z in (-4..=4).step_by(2) {
            if (x + z) % 4 == 0 {
                labyrinth_room.build_box(v(x, 0, z), v(x, 3, z));
            }
        }
    }

    labyrinth_room.set_vantage(v(0, 1, 0), /*coherence=*/ 0.6, /*interest=*/ 0.9);

    // A "stacked balconies" atrium: three levels of offset balconies around a central void
    let mut stacked_balconies = Builder::new(&structures);
    // Ground floor ring
    stacked_balconies.build_box(v(-6, 0, -6), v(6, 0, 6));
    stacked_balconies.build_box(v(-4, 0, -4), v(4, 0, 4)); // inner platform
                                                           // First balcony level (offset in X)
    stacked_balconies.build_box(v(-6, 2, -4), v(6, 2, -2));
    stacked_balconies.build_box(v(-6, 2, 2), v(6, 2, 4));
    // Second balcony level (offset in Z)
    stacked_balconies.build_box(v(-4, 4, -6), v(-2, 4, 6));
    stacked_balconies.build_box(v(2, 4, -6), v(4, 4, 6));
    // Railings at every balcony edge
    stacked_balconies.wall_off_drops(v(-6, 2, -4), v(6, 2, 4), "railing");
    stacked_balconies.wall_off_drops(v(-4, 4, -6), v(4, 4, 6), "railing");
    stacked_balconies.set_vantage(
        v(0, 0, -2),
        /*coherence=*/ 0.75,
        /*interest=*/ 0.85,
    );

    // A "spiral terraces" amphitheater descending toward a central stage
    let mut spiral_terraces = Builder::new(&structures);
    // Top ring
    spiral_terraces.build_box(v(-6, 3, -6), v(6, 3, 6));
    // Second ring
    spiral_terraces.build_box(v(-5, 2, -5), v(5, 2, 5));
    // Third ring
    spiral_terraces.build_box(v(-4, 1, -4), v(4, 1, 4));
    // Stage floor
    spiral_terraces.build_box(v(-2, 0, -2), v(2, 0, 2));
    // Railings along each ring drop
    spiral_terraces.wall_off_drops(v(-6, 3, -6), v(6, 3, 6), "railing");
    spiral_terraces.wall_off_drops(v(-5, 2, -5), v(5, 2, 5), "railing");
    spiral_terraces.wall_off_drops(v(-4, 1, -4), v(4, 1, 4), "railing");
    spiral_terraces.set_vantage(v(0, 3, -4), /*coherence=*/ 0.8, /*interest=*/ 0.9);

    // A cross-shaped upper walkway over an open ground floor
    let mut cross_walkway = Builder::new(&structures);
    // Ground floor outer bounds
    cross_walkway.build_box(v(-5, 0, -5), v(5, 0, 5));
    // Cross walkway one level up
    cross_walkway.build_box(v(-1, 2, -5), v(1, 2, 5));
    cross_walkway.build_box(v(-5, 2, -1), v(5, 2, 1));
    // Railings all along the walkway edges
    cross_walkway.wall_off_drops(v(-1, 2, -5), v(1, 2, 5), "railing");
    cross_walkway.wall_off_drops(v(-5, 2, -1), v(5, 2, 1), "railing");
    // Doorways up to the walkway from ground floor corners
    cross_walkway.build_plane(
        v(-5, 0, -1),
        v(-5, 0, -1),
        RelSlot::XLoWall,
        Some("doorway"),
    );
    cross_walkway.build_plane(v(5, 0, 1), v(5, 0, 1), RelSlot::XHiWall, Some("doorway"));
    cross_walkway.set_vantage(v(0, 0, 0), /*coherence=*/ 0.95, /*interest=*/ 0.8);

    // A "bridge over chasm" with two asymmetric viewing decks
    let mut chasm_bridge = Builder::new(&structures);
    // Two facing decks
    chasm_bridge.build_box(v(-6, 0, -2), v(-3, 0, 2));
    chasm_bridge.build_box(v(3, 0, -3), v(6, 0, 3));
    // Central bridge two levels up
    chasm_bridge.build_plane(v(-1, 2, 0), v(1, 2, 0), RelSlot::Floor, None);
    // Railings around decks and bridge
    chasm_bridge.wall_off_drops(v(-6, 0, -2), v(-3, 0, 2), "railing");
    chasm_bridge.wall_off_drops(v(3, 0, -3), v(6, 0, 3), "railing");
    chasm_bridge.wall_off_drops(v(-1, 2, 0), v(1, 2, 0), "railing");
    chasm_bridge.set_vantage(
        v(0, 0, -5),
        /*coherence=*/ 0.65,
        /*interest=*/ 0.85,
    );

    vec![]
}
