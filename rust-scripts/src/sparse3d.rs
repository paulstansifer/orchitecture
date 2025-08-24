#![allow(dead_code)]
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::{Index, IndexMut};

use godot::builtin::Vector3i;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum Slot {
    Room,
    XWall,
    YFloor,
    ZWall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotLocation {
    pub cube: Vector3i,
    pub rel_slot: RelSlot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelSlot {
    Room,
    XHiWall,
    XLoWall,
    Floor,
    Ceiling,
    ZHiWall,
    ZLoWall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rotation {
    Clockwise,
    OneEighty,
    CounterClockwise,
}

impl RelSlot {
    fn absolute_offset(self) -> Vector3i {
        match self {
            RelSlot::XHiWall => Vector3i::new(1, 0, 0),
            RelSlot::Ceiling => Vector3i::new(0, 1, 0),
            RelSlot::ZHiWall => Vector3i::new(0, 0, 1),
            _ => Vector3i::ZERO,
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

    pub fn direction_of_neighbor(self) -> Vector3i {
        match self {
            RelSlot::XHiWall => Vector3i::new(1, 0, 0),
            RelSlot::Ceiling => Vector3i::new(0, 1, 0),
            RelSlot::ZHiWall => Vector3i::new(0, 0, 1),
            RelSlot::XLoWall => Vector3i::new(-1, 0, 0),
            RelSlot::Floor => Vector3i::new(0, -1, 0),
            RelSlot::ZLoWall => Vector3i::new(0, 0, -1),
            RelSlot::Room => panic!(),
        }
    }
    pub fn rotate(self, rotation: Rotation) -> Self {
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
                Rotation::Clockwise => RelSlot::XLoWall,
                Rotation::OneEighty => RelSlot::ZHiWall,
                Rotation::CounterClockwise => RelSlot::XHiWall,
            },
            RelSlot::ZHiWall => match rotation {
                Rotation::Clockwise => RelSlot::XHiWall,
                Rotation::OneEighty => RelSlot::ZLoWall,
                Rotation::CounterClockwise => RelSlot::XLoWall,
            },
        }
    }
}

impl SlotLocation {
    pub fn new(x: i32, y: i32, z: i32, rel_slot: RelSlot) -> Self {
        SlotLocation {
            cube: Vector3i::new(x, y, z),
            rel_slot,
        }
    }

    fn split_location(&self) -> (BigCoordinates, SmallCoordinates) {
        let slot = match self.rel_slot {
            RelSlot::Room => Slot::Room,
            RelSlot::XHiWall | RelSlot::XLoWall => Slot::XWall,
            RelSlot::Floor | RelSlot::Ceiling => Slot::YFloor,
            RelSlot::ZHiWall | RelSlot::ZLoWall => Slot::ZWall,
        };
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

    fn rotate(&self, rotation: Rotation) -> Self {
        let new_coord = match rotation {
            Rotation::Clockwise => Vector3i::new(-self.cube.z, self.cube.y, self.cube.x),
            Rotation::CounterClockwise => Vector3i::new(self.cube.z, self.cube.y, -self.cube.x),
            Rotation::OneEighty => Vector3i::new(-self.cube.x, -self.cube.y, -self.cube.z),
        };

        SlotLocation {
            cube: new_coord,
            rel_slot: self.rel_slot.rotate(rotation),
        }
    }
}

impl std::ops::Add<Vector3i> for SlotLocation {
    type Output = Self;

    fn add(self, other: Vector3i) -> Self {
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

fn split_coords(loc: Vector3i, slot: Slot) -> (BigCoordinates, SmallCoordinates) {
    let big_coords = BigCoordinates {
        x: loc.x.div_euclid(4),
        y: loc.y.div_euclid(4),
        z: loc.z.div_euclid(4),
    };

    let small_coords = SmallCoordinates {
        x: loc.x.rem_euclid(4) as u8,
        y: loc.y.rem_euclid(4) as u8,
        z: loc.z.rem_euclid(4) as u8,
        slot,
    };

    (big_coords, small_coords)
}

fn combine_coords(bc: BigCoordinates, sc: SmallCoordinates) -> Vector3i {
    Vector3i::new(
        bc.x * 4 + sc.x as i32,
        bc.y * 4 + sc.y as i32,
        bc.z * 4 + sc.z as i32,
    )
}

#[derive(Debug)]
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

#[derive(Debug)]
pub struct Sparse3D<T> {
    chunks: HashMap<BigCoordinates, Chunk<T>>,
}

impl<T> PartialEq for Sparse3D<T>
where
    T: PartialEq,
{
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
        self.chunks.get_mut(&bc).and_then(|chunk| {
            let value = chunk[sc].take();
            value
        })
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

    pub fn bounding_box(&self) -> (Vector3i, Vector3i) {
        if self.chunks.is_empty() {
            return (Vector3i::ZERO, Vector3i::ZERO);
        }

        let mut min = Vector3i::new(i32::MAX, i32::MAX, i32::MAX);
        let mut max = Vector3i::new(i32::MIN, i32::MIN, i32::MIN);

        for (bc, chunk) in &self.chunks {
            for (sc, _) in chunk.iter() {
                let coord = combine_coords(*bc, sc);
                min = Vector3i::coord_min(min, coord);
                max = Vector3i::coord_max(max, coord);
            }
        }
        (min, max)
    }
}

impl<T: Clone> Sparse3D<T> {
    pub fn rotate(&self, rotation: Rotation) -> Self {
        let mut rotated = Sparse3D::<T>::new();
        for (loc, value) in self.iter() {
            rotated.set(loc.rotate(rotation), value.clone());
        }
        rotated
    }
}

impl<T> Index<SlotLocation> for Sparse3D<T> {
    type Output = T;

    fn index(&self, loc: SlotLocation) -> &Self::Output {
        self.get(loc).unwrap()
    }
}

impl<T> IndexMut<SlotLocation> for Sparse3D<T> {
    fn index_mut(&mut self, loc: SlotLocation) -> &mut Self::Output {
        self.get_mut(loc).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sparse3d::Sparse3D;
    use std::collections::HashSet;

    #[test]
    fn test_infinite_grid_indexing() {
        let mut grid: Sparse3D<i32> = Sparse3D::new();

        // Set some values
        grid.set(SlotLocation::new(1, 2, 3, RelSlot::XLoWall), 10);
        grid.set(SlotLocation::new(-1, 5, 0, RelSlot::XLoWall), 20);
        grid.set(SlotLocation::new(4, 0, 0, RelSlot::XLoWall), 30); // Different chunk

        // Get the values using indexing
        assert_eq!(grid[SlotLocation::new(1, 2, 3, RelSlot::XLoWall)], 10);
        assert_eq!(grid[SlotLocation::new(-1, 5, 0, RelSlot::XLoWall)], 20);
        assert_eq!(grid[SlotLocation::new(4, 0, 0, RelSlot::XLoWall)], 30);
    }

    #[test]
    fn test_sparse_3d_iterator() {
        let mut grid: Sparse3D<i32> = Sparse3D::new();

        // Set some values
        grid.set(SlotLocation::new(1, 2, 3, RelSlot::XLoWall), 10);
        grid.set(SlotLocation::new(-1, 5, 0, RelSlot::Floor), 20);
        grid.set(SlotLocation::new(4, 0, 0, RelSlot::ZLoWall), 30);

        let items: HashSet<_> = grid.iter().collect();

        let expected: HashSet<_> = vec![
            (SlotLocation::new(1, 2, 3, RelSlot::XLoWall), &10),
            (SlotLocation::new(-1, 5, 0, RelSlot::Floor), &20),
            (SlotLocation::new(4, 0, 0, RelSlot::ZLoWall), &30),
        ]
        .into_iter()
        .collect();

        assert_eq!(items, expected);
    }
}
