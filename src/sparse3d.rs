#![allow(dead_code)]
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::{Index, IndexMut};

use bevy::math::IVec3;
use enum_derived::Rand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Rand)]
enum Slot {
    Room,
    XWall,
    YFloor,
    ZWall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotLocation {
    pub cube: IVec3,
    pub rel_slot: RelSlot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Rand)]
pub enum RelSlot {
    Room,
    XHiWall,
    XLoWall,
    Floor,
    Ceiling,
    ZHiWall,
    ZLoWall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Rand)]
pub enum Rotation {
    Clockwise,
    OneEighty,
    CounterClockwise,
}

pub trait Rotateable {
    fn rotate(self, rotation: Rotation) -> Self;
}

// Direction a Room object can face
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Facing {
    #[default]
    NegX = 0, // TODO: verify that these are right; I picked them arbitrarily!
    NegZ = 1,
    PosX = 2,
    PosZ = 3,
}

impl Facing {
    pub fn from_number(n: u8) -> Facing {
        match n % 4 {
            0 => Facing::NegX,
            1 => Facing::NegZ,
            2 => Facing::PosX,
            3 => Facing::PosZ,
            _ => unreachable!(),
        }
    }

    pub fn arbitrary() -> Facing {
        Facing::from_number(0)
    }
}

impl Rotateable for Facing {
    fn rotate(self, rotation: Rotation) -> Self {
        let current_val = self as u8;
        let rotated_val = match rotation {
            Rotation::Clockwise => current_val + 1,
            Rotation::OneEighty => current_val + 2,
            Rotation::CounterClockwise => current_val + 3,
        };
        Facing::from_number(rotated_val)
    }
}

impl RelSlot {
    fn absolute_offset(self) -> IVec3 {
        match self {
            RelSlot::XHiWall => IVec3::new(1, 0, 0),
            RelSlot::Ceiling => IVec3::new(0, 1, 0),
            RelSlot::ZHiWall => IVec3::new(0, 0, 1),
            _ => IVec3::ZERO,
        }
    }

    fn as_absolute_slot(self) -> Slot {
        match self {
            RelSlot::XLoWall | RelSlot::XHiWall => Slot::XWall,
            RelSlot::Floor | RelSlot::Ceiling => Slot::YFloor,
            RelSlot::ZLoWall | RelSlot::ZHiWall => Slot::ZWall,
            RelSlot::Room => Slot::Room,
        }
    }

    fn from_absolute_slot(slot: Slot) -> Self {
        match slot {
            Slot::XWall => RelSlot::XLoWall,
            Slot::YFloor => RelSlot::Floor,
            Slot::ZWall => RelSlot::ZLoWall,
            Slot::Room => RelSlot::Room,
        }
    }

    pub fn direction_of_neighbor(self) -> IVec3 {
        match self {
            RelSlot::XHiWall => IVec3::new(1, 0, 0),
            RelSlot::Ceiling => IVec3::new(0, 1, 0),
            RelSlot::ZHiWall => IVec3::new(0, 0, 1),
            RelSlot::XLoWall => IVec3::new(-1, 0, 0),
            RelSlot::Floor => IVec3::new(0, -1, 0),
            RelSlot::ZLoWall => IVec3::new(0, 0, -1),
            RelSlot::Room => panic!(),
        }
    }
}

impl Rotateable for RelSlot {
    fn rotate(self, rotation: Rotation) -> Self {
        match self {
            RelSlot::Room | RelSlot::Floor | RelSlot::Ceiling => self,
            RelSlot::XLoWall => match rotation {
                Rotation::Clockwise => RelSlot::ZLoWall,
                Rotation::OneEighty => RelSlot::XHiWall,
                Rotation::CounterClockwise => RelSlot::ZHiWall,
            },
            RelSlot::XHiWall => match rotation {
                Rotation::Clockwise => RelSlot::ZHiWall,
                Rotation::OneEighty => RelSlot::XLoWall,
                Rotation::CounterClockwise => RelSlot::ZLoWall,
            },
            RelSlot::ZLoWall => match rotation {
                Rotation::Clockwise => RelSlot::XHiWall,
                Rotation::OneEighty => RelSlot::ZHiWall,
                Rotation::CounterClockwise => RelSlot::XLoWall,
            },
            RelSlot::ZHiWall => match rotation {
                Rotation::Clockwise => RelSlot::XLoWall,
                Rotation::OneEighty => RelSlot::ZLoWall,
                Rotation::CounterClockwise => RelSlot::XHiWall,
            },
        }
    }
}

impl SlotLocation {
    pub fn new(x: i32, y: i32, z: i32, rel_slot: RelSlot) -> Self {
        SlotLocation {
            cube: IVec3::new(x, y, z),
            rel_slot,
        }
    }

    fn split_location(&self) -> (BigCoordinates, SmallCoordinates) {
        let slot = self.rel_slot.as_absolute_slot();
        let abs_loc = self.cube + self.rel_slot.absolute_offset();
        let big_coords = BigCoordinates {
            x: abs_loc.x.div_euclid(4),
            y: abs_loc.y.div_euclid(4),
            z: abs_loc.z.div_euclid(4),
        };

        let small_coords = SmallCoordinates {
            x: abs_loc.x.rem_euclid(4) as u8,
            y: abs_loc.y.rem_euclid(4) as u8,
            z: abs_loc.z.rem_euclid(4) as u8,
            slot,
        };
        (big_coords, small_coords)
    }

    fn get_center(&self) -> (f64, f64, f64) {
        let base = (
            self.cube.x as f64 + 0.5,
            self.cube.y as f64 + 0.5,
            self.cube.z as f64 + 0.5,
        );
        match self.rel_slot {
            RelSlot::Room => base,
            RelSlot::XLoWall => (base.0 - 0.5, base.1, base.2),
            RelSlot::XHiWall => (base.0 + 0.5, base.1, base.2),
            RelSlot::Floor => (base.0, base.1 - 0.5, base.2),
            RelSlot::Ceiling => (base.0, base.1 + 0.5, base.2),
            RelSlot::ZLoWall => (base.0, base.1, base.2 - 0.5),
            RelSlot::ZHiWall => (base.0, base.1, base.2 + 0.5),
        }
    }
}

impl Rotateable for SlotLocation {
    fn rotate(self, rotation: Rotation) -> Self {
        let new_coord = match rotation {
            Rotation::Clockwise => IVec3::new(-self.cube.z, self.cube.y, self.cube.x),
            Rotation::CounterClockwise => IVec3::new(self.cube.z, self.cube.y, -self.cube.x),
            Rotation::OneEighty => IVec3::new(-self.cube.x, self.cube.y, -self.cube.z),
        };
        SlotLocation {
            cube: new_coord,
            rel_slot: self.rel_slot.rotate(rotation),
        }
    }
}

impl std::ops::Add<IVec3> for SlotLocation {
    type Output = Self;

    fn add(self, other: IVec3) -> Self {
        SlotLocation {
            cube: self.cube + other,
            rel_slot: self.rel_slot,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct BigCoordinates {
    x: i32,
    y: i32,
    z: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SmallCoordinates {
    x: u8,
    y: u8,
    z: u8,
    slot: Slot,
}

fn combine_coords(bc: BigCoordinates, sc: SmallCoordinates) -> IVec3 {
    IVec3::new(
        bc.x * 4 + sc.x as i32,
        bc.y * 4 + sc.y as i32,
        bc.z * 4 + sc.z as i32,
    )
}

#[derive(Debug, Clone)]
struct Chunk<T> {
    data: [Option<T>; 256],
}

impl<T> Chunk<T> {
    fn new() -> Self {
        Chunk {
            data: [const { None }; 256],
        }
    }
    fn iter(&self) -> impl Iterator<Item = (SmallCoordinates, &T)> {
        self.data.iter().enumerate().filter_map(|(i, item)| {
            item.as_ref().map(|value| {
                let slot = match i % 4 {
                    0 => Slot::Room,
                    1 => Slot::XWall,
                    2 => Slot::YFloor,
                    3 => Slot::ZWall,
                    _ => unreachable!(),
                };
                let x = ((i / 4) % 4) as u8;
                let y = ((i / 16) % 4) as u8;
                let z = (i / 64) as u8;
                (SmallCoordinates { x, y, z, slot }, value)
            })
        })
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = (SmallCoordinates, &mut T)> {
        self.data.iter_mut().enumerate().filter_map(|(i, item)| {
            item.as_mut().map(|value| {
                let slot = match i % 4 {
                    0 => Slot::Room,
                    1 => Slot::XWall,
                    2 => Slot::YFloor,
                    3 => Slot::ZWall,
                    _ => unreachable!(),
                };
                let x = ((i / 4) % 4) as u8;
                let y = ((i / 16) % 4) as u8;
                let z = (i / 64) as u8;
                (SmallCoordinates { x, y, z, slot }, value)
            })
        })
    }

    fn size(&self) -> usize {
        self.data.iter().filter(|item| item.is_some()).count()
    }
}

impl<T> Index<SmallCoordinates> for Chunk<T> {
    type Output = Option<T>;

    fn index(&self, sc: SmallCoordinates) -> &Self::Output {
        let index = sc.slot as usize + sc.x as usize * 4 + sc.y as usize * 16 + sc.z as usize * 64;
        &self.data[index]
    }
}
impl<T> IndexMut<SmallCoordinates> for Chunk<T> {
    fn index_mut(&mut self, sc: SmallCoordinates) -> &mut Self::Output {
        let index = sc.slot as usize + sc.x as usize * 4 + sc.y as usize * 16 + sc.z as usize * 64;
        &mut self.data[index]
    }
}

#[derive(Debug, Clone)]
pub struct Sparse3D<T> {
    chunks: HashMap<BigCoordinates, Chunk<T>>,
}

impl<T: PartialEq> PartialEq for Sparse3D<T> {
    fn eq(&self, other: &Self) -> bool {
        if self.size() != other.size() {
            return false;
        }
        for (loc, value) in self.iter() {
            if other.get(loc) != Some(value) {
                return false;
            }
        }

        true
    }
}

impl<T> Sparse3D<T> {
    pub fn new() -> Self {
        Sparse3D {
            chunks: HashMap::new(),
        }
    }

    pub fn size(&self) -> usize {
        self.chunks.iter().map(|(_, chunk)| chunk.size()).sum()
    }

    fn get_or_create_chunk(&mut self, chunk_coords: BigCoordinates) -> &mut Chunk<T> {
        self.chunks.entry(chunk_coords).or_insert_with(Chunk::new)
    }

    pub fn get(&self, loc: SlotLocation) -> Option<&T> {
        let (bc, sc) = loc.split_location();
        self.chunks.get(&bc).and_then(|chunk| chunk[sc].as_ref())
    }

    pub fn take(&mut self, loc: SlotLocation) -> Option<T> {
        let (bc, sc) = loc.split_location();
        self.chunks.get_mut(&bc).and_then(|chunk| chunk[sc].take())
    }

    pub fn get_mut(&mut self, loc: SlotLocation) -> Option<&mut T> {
        let (bc, sc) = loc.split_location();
        self.chunks
            .get_mut(&bc)
            .and_then(|chunk| chunk[sc].as_mut())
    }

    pub fn set(&mut self, loc: SlotLocation, value: T) {
        let (bc, sc) = loc.split_location();
        let chunk = self.get_or_create_chunk(bc);
        chunk[sc] = Some(value);
    }

    pub fn iter(&self) -> impl Iterator<Item = (SlotLocation, &T)> {
        self.chunks.iter().flat_map(|(bc, chunk)| {
            chunk.iter().map(move |(sc, value)| {
                let loc = SlotLocation::new(
                    bc.x * 4 + sc.x as i32,
                    bc.y * 4 + sc.y as i32,
                    bc.z * 4 + sc.z as i32,
                    RelSlot::from_absolute_slot(sc.slot),
                );
                (loc, value)
            })
        })
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (SlotLocation, &mut T)> {
        self.chunks.iter_mut().flat_map(|(bc, chunk)| {
            chunk.iter_mut().map(move |(sc, value)| {
                let loc = SlotLocation::new(
                    bc.x * 4 + sc.x as i32,
                    bc.y * 4 + sc.y as i32,
                    bc.z * 4 + sc.z as i32,
                    RelSlot::from_absolute_slot(sc.slot),
                );
                (loc, value)
            })
        })
    }

    pub fn translate(self, offset: IVec3) -> Self
    where
        T: Clone,
    {
        let mut new_grid = Sparse3D::new();
        for (loc, cell) in self.iter() {
            let new_loc = SlotLocation {
                cube: loc.cube + offset,
                rel_slot: loc.rel_slot,
            };
            new_grid.set(new_loc, cell.clone());
        }
        new_grid
    }

    pub fn bounding_box(&self) -> (IVec3, IVec3) {
        if self.chunks.is_empty() {
            return (IVec3::ZERO, IVec3::ZERO);
        }

        let mut min = IVec3::new(i32::MAX, i32::MAX, i32::MAX);
        let mut max = IVec3::new(i32::MIN, i32::MIN, i32::MIN);

        for (bc, chunk) in &self.chunks {
            for (sc, _) in chunk.iter() {
                let coord = combine_coords(*bc, sc);
                min = min.min(coord);
                max = max.max(coord);
            }
        }
        (min, max)
    }

    pub fn ray_trace(&self, origin: SlotLocation, destination: SlotLocation) -> Vec<Vec<&T>> {
        let p0 = origin.get_center();
        let p1 = destination.get_center();
        let dir = (p1.0 - p0.0, p1.1 - p0.1, p1.2 - p0.2);

        // 1. Identify all t values where the ray crosses an integer plane (x, y, or z)
        let mut ts = Vec::new();
        ts.push(0.0);
        ts.push(1.0);

        let epsilon = 1e-9;

        // Helper to find intersections for one dimension
        let mut add_intersections = |start: f64, d: f64| {
            if d.abs() > epsilon {
                let min_val = start.min(start + d);
                let max_val = start.max(start + d);

                let first_int = min_val.ceil() as i32;
                let last_int = max_val.floor() as i32;

                for k in first_int..=last_int {
                    let t = (k as f64 - start) / d;
                    if t >= -epsilon && t <= 1.0 + epsilon {
                        ts.push(t.clamp(0.0, 1.0));
                    }
                }
            } else if (start.round() - start).abs() < epsilon {
                // parallel to plane and lies on an integer coordinate?
                // technically "intersects" everywhere, but we don't need to add specific 't's for this
                // because the segment checks will handle it.
            }
        };

        add_intersections(p0.0, dir.0);
        add_intersections(p0.1, dir.1);
        add_intersections(p0.2, dir.2);

        ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ts.dedup_by(|a, b| (*a - *b).abs() < epsilon);

        let mut result = Vec::new();

        // 2. Iterate through points and segments
        let mut prev_t: Option<f64> = None;

        for &t in &ts {
            // Segment before this point
            if let Some(pt) = prev_t {
                if (t - pt) > epsilon {
                    let t_mid = (pt + t) / 2.0;
                    let p_mid = (
                        p0.0 + t_mid * dir.0,
                        p0.1 + t_mid * dir.1,
                        p0.2 + t_mid * dir.2,
                    );
                    let items = self.collect_at_point(p_mid, epsilon);
                    if !items.is_empty() {
                        result.push(items);
                    }
                }
            }

            // The point itself
            let p_curr = (p0.0 + t * dir.0, p0.1 + t * dir.1, p0.2 + t * dir.2);
            let items = self.collect_at_point(p_curr, epsilon);
            if !items.is_empty() {
                result.push(items);
            }

            prev_t = Some(t);
        }

        result
    }

    fn collect_at_point(&self, p: (f64, f64, f64), epsilon: f64) -> Vec<&T> {
        let mut items = Vec::new();

        // Helper to get candidate indices for a coordinate
        let get_candidates = |val: f64| -> Vec<i32> {
            if (val.round() - val).abs() < epsilon {
                vec![val.round() as i32 - 1, val.round() as i32]
            } else {
                vec![val.floor() as i32]
            }
        };

        let xs = get_candidates(p.0);
        let ys = get_candidates(p.1);
        let zs = get_candidates(p.2);

        // Check Rooms
        for &x in &xs {
            for &y in &ys {
                for &z in &zs {
                    if let Some(val) = self.get(SlotLocation::new(x, y, z, RelSlot::Room)) {
                        items.push(val);
                    }
                }
            }
        }

        // Check XWalls
        // For XWall, x must be closely integer. The wall is exactly at that integer.
        // So we don't look at `xs` (which has 2 neighbors), we look at exactly `round(p.0)`.
        if (p.0.round() - p.0).abs() < epsilon {
            let x_idx = p.0.round() as i32;
            for &y in &ys {
                for &z in &zs {
                    if let Some(val) = self.get(SlotLocation::new(x_idx, y, z, RelSlot::XLoWall)) {
                        items.push(val);
                    }
                }
            }
        }

        // Check YFloors
        if (p.1.round() - p.1).abs() < epsilon {
            let y_idx = p.1.round() as i32;
            for &x in &xs {
                for &z in &zs {
                    if let Some(val) = self.get(SlotLocation::new(x, y_idx, z, RelSlot::Floor)) {
                        items.push(val);
                    }
                }
            }
        }

        // Check ZWalls
        if (p.2.round() - p.2).abs() < epsilon {
            let z_idx = p.2.round() as i32;
            for &x in &xs {
                for &y in &ys {
                    if let Some(val) = self.get(SlotLocation::new(x, y, z_idx, RelSlot::ZLoWall)) {
                        items.push(val);
                    }
                }
            }
        }

        // Dedup items if necessary?
        // Logic might add same item multiple times?
        // e.g. Room check is distinct. Wall checks are distinct.
        // But do Wall checks overlap with Room checks? No, different RelSlot.
        // Result is Vec<&T>. Pointers.
        // We shouldn't have duplicates because the slots are distinct keys in the map.

        items
    }
}

impl<T: Rotateable + Clone> Rotateable for Sparse3D<T> {
    fn rotate(self, rotation: Rotation) -> Self {
        let mut rotated = Sparse3D::<T>::new();
        for (loc, value) in self.iter() {
            rotated.set(loc.rotate(rotation), value.clone().rotate(rotation));
        }
        rotated
    }
}

impl<T: Rotateable> Index<SlotLocation> for Sparse3D<T> {
    type Output = T;

    fn index(&self, loc: SlotLocation) -> &Self::Output {
        self.get(loc).unwrap()
    }
}

impl<T: Rotateable> IndexMut<SlotLocation> for Sparse3D<T> {
    fn index_mut(&mut self, loc: SlotLocation) -> &mut Self::Output {
        self.get_mut(loc).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use assert2::check;

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

        check!(grid[SlotLocation::new(1, 2, 3, RelSlot::XLoWall)] == RotInt(10));
        check!(grid[SlotLocation::new(-1, 5, 0, RelSlot::XLoWall)] == RotInt(20));
        check!(grid[SlotLocation::new(4, 0, 0, RelSlot::XLoWall)] == RotInt(30));
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

        check!(items == expected);
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

        let flattened: HashSet<&RotInt> = trace.iter().flatten().cloned().collect();
        check!(flattened.contains(&RotInt(1)));
        check!(flattened.contains(&RotInt(2)));
        check!(flattened.contains(&RotInt(3)));
        // Verify structure roughly
        // Middle of trace should have 3 items (Room A, Wall, Room B)
        let crossing = trace.iter().find(|group| group.len() >= 3);
        check!(crossing.is_some());
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

        check!(flattened.contains(&RotInt(10)));
        check!(flattened.contains(&RotInt(20)));
        check!(flattened.contains(&RotInt(11)));
        check!(flattened.contains(&RotInt(12)));
    }
}
