use super::*;
use crate::sparse3d::Sparse3D;
use std::collections::HashSet;

#[derive(Clone, PartialEq, Debug, Eq, Hash)]
struct RotInt(i32);
impl super::Rotateable for RotInt {
    fn rotate(self, _rotation: super::Rotation) -> Self {
        self
    }
}

#[test]
fn test_infinite_grid_indexing() {
    let mut grid: Sparse3D<RotInt> = Sparse3D::new();

    // Set some values
    grid.set(SlotLocation::new(1, 2, 3, RelSlot::XLoWall), RotInt(10));
    grid.set(SlotLocation::new(-1, 5, 0, RelSlot::XLoWall), RotInt(20));
    grid.set(SlotLocation::new(4, 0, 0, RelSlot::XLoWall), RotInt(30)); // Different chunk

    // Get the values using indexing
    assert_eq!(
        grid[SlotLocation::new(1, 2, 3, RelSlot::XLoWall)],
        RotInt(10)
    );
    assert_eq!(
        grid[SlotLocation::new(-1, 5, 0, RelSlot::XLoWall)],
        RotInt(20)
    );
    assert_eq!(
        grid[SlotLocation::new(4, 0, 0, RelSlot::XLoWall)],
        RotInt(30)
    );
}

#[test]
fn test_sparse_3d_iterator() {
    let mut grid: Sparse3D<RotInt> = Sparse3D::new();

    // Set some values
    grid.set(SlotLocation::new(1, 2, 3, RelSlot::XLoWall), RotInt(10));
    grid.set(SlotLocation::new(-1, 5, 0, RelSlot::Floor), RotInt(20));
    grid.set(SlotLocation::new(4, 0, 0, RelSlot::ZLoWall), RotInt(30));

    let items: HashSet<_> = grid.iter().collect();

    let expected: HashSet<_> = vec![
        (SlotLocation::new(1, 2, 3, RelSlot::XLoWall), &RotInt(10)),
        (SlotLocation::new(-1, 5, 0, RelSlot::Floor), &RotInt(20)),
        (SlotLocation::new(4, 0, 0, RelSlot::ZLoWall), &RotInt(30)),
    ]
    .into_iter()
    .collect();

    assert_eq!(items, expected);
}

#[test]
fn test_ray_trace_simple() {
    let mut grid: Sparse3D<RotInt> = Sparse3D::new();
    // Room at (0,0,0) -> "Room A" (1)
    grid.set(SlotLocation::new(0, 0, 0, RelSlot::Room), RotInt(1));
    // Wall at (1,0,0) (XLoWall for (1,0,0) or XHiWall for (0,0,0)) -> "Wall" (2)
    // XLoWall at (1,0,0) is at x=1.
    grid.set(SlotLocation::new(1, 0, 0, RelSlot::XLoWall), RotInt(2));
    // Room at (1,0,0) -> "Room B" (3)
    grid.set(SlotLocation::new(1, 0, 0, RelSlot::Room), RotInt(3));

    // Ray from Center of Rule 0 (0.5, 0.5, 0.5) to Center of Room 1 (1.5, 0.5, 0.5)
    let trace = grid.ray_trace(
        SlotLocation::new(0, 0, 0, RelSlot::Room),
        SlotLocation::new(1, 0, 0, RelSlot::Room),
    );

    // Expected sequence of sets of items found along the ray.
    // We expect at least the rooms and the wall.
    // Let's flatten and check presence.
    let flattened: HashSet<&RotInt> = trace.iter().flatten().cloned().collect();
    assert!(flattened.contains(&RotInt(1)));
    assert!(flattened.contains(&RotInt(2)));
    assert!(flattened.contains(&RotInt(3)));

    // Verify structure roughly
    // Middle of trace should have 3 items (Room A, Wall, Room B)
    let crossing = trace.iter().find(|group| group.len() >= 3);
    assert!(
        crossing.is_some(),
        "Should have a crossing event with multiple items"
    );
}

#[test]
fn test_ray_trace_corner() {
    let mut grid: Sparse3D<RotInt> = Sparse3D::new();
    // 2D Corner: (0,0) to (1,1).
    // Rooms: (0,0), (1,0), (0,1), (1,1).
    // Walls/Floors at x=1, z=1.

    // Ray from (0,0,0) to (1,0,1). (x,z plane).

    let start = SlotLocation::new(0, 0, 0, RelSlot::Room);
    let end = SlotLocation::new(1, 0, 1, RelSlot::Room);

    grid.set(start, RotInt(10)); // Room 0,0
    grid.set(end, RotInt(20)); // Room 1,1

    // Corner is at x=1, z=1. (Ray goes 0.5->1.5? No, 0->1 in coords)
    // Start center: (0.5, 0.5, 0.5)
    // End center: (1.5, 0.5, 1.5)
    // Ray passes through (1.0, 0.5, 1.0).

    // At (1.0, 0.5, 1.0):
    // Should touch Rooms at (0,0,0), (1,0,0), (0,0,1), (1,0,1).
    // Should touch XWall at (1,0,0) (and at z=0,1..).
    // Should touch ZWall at (0,0,1) etc.

    // Let's just set the rooms and verify we hit them all.
    grid.set(SlotLocation::new(1, 0, 0, RelSlot::Room), RotInt(11));
    grid.set(SlotLocation::new(0, 0, 1, RelSlot::Room), RotInt(12));

    let trace = grid.ray_trace(start, end);
    let flattened: HashSet<&RotInt> = trace.iter().flatten().cloned().collect();

    assert!(flattened.contains(&RotInt(10)));
    assert!(flattened.contains(&RotInt(20)));
    assert!(flattened.contains(&RotInt(11)));
    assert!(flattened.contains(&RotInt(12)));
}
