use std::collections::HashMap;
use std::ops::{Index, IndexMut};

use godot::builtin::real_consts::TAU;
use godot::builtin::{Basis, Vector3, Vector3i};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Room,
    XWall,
    YFloor,
    ZWall,
}

impl Slot {
    pub fn basis(self) -> Basis {
        match self {
            Slot::XWall => Basis::IDENTITY,
            Slot::YFloor => Basis::IDENTITY.rotated(Vector3::FORWARD, TAU / 4.0),
            Slot::ZWall => Basis::IDENTITY.rotated(Vector3::UP, TAU / 4.0),
            Slot::Room => Basis::IDENTITY,
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

struct Chunk<T> {
    data: [Option<T>; 256],
}

impl<T> Chunk<T> {
    fn new() -> Self {
        Chunk {
            data: [const { None }; 256],
        }
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

pub struct Sparse3D<T> {
    chunks: HashMap<BigCoordinates, Chunk<T>>,
}

impl<T> Sparse3D<T> {
    pub fn new() -> Self {
        Sparse3D {
            chunks: HashMap::new(),
        }
    }

    fn get_or_create_chunk(&mut self, chunk_coords: BigCoordinates) -> &mut Chunk<T> {
        self.chunks.entry(chunk_coords).or_insert_with(Chunk::new)
    }

    pub fn get(&self, loc: Vector3i, slot: Slot) -> Option<&T> {
        let (bc, sc) = split_coords(loc, slot);
        self.chunks.get(&bc).and_then(|chunk| chunk[sc].as_ref())
    }

    pub fn take(&mut self, loc: Vector3i, slot: Slot) -> Option<T> {
        let (bc, sc) = split_coords(loc, slot);
        self.chunks.get_mut(&bc).and_then(|chunk| {
            let value = chunk[sc].take();
            value
        })
    }

    pub fn get_mut(&mut self, loc: Vector3i, slot: Slot) -> Option<&mut T> {
        let (bc, sc) = split_coords(loc, slot);
        self.chunks
            .get_mut(&bc)
            .and_then(|chunk| chunk[sc].as_mut())
    }

    pub fn set(&mut self, loc: Vector3i, slot: Slot, value: T) {
        let (bc, sc) = split_coords(loc, slot);
        let chunk = self.get_or_create_chunk(bc);
        chunk[sc] = Some(value);
    }
}

impl<T> Index<(Vector3i, Slot)> for Sparse3D<T> {
    type Output = T;

    fn index(&self, (loc, slot): (Vector3i, Slot)) -> &Self::Output {
        self.get(loc, slot).unwrap()
    }
}

impl<T> IndexMut<(Vector3i, Slot)> for Sparse3D<T> {
    fn index_mut(&mut self, (loc, slot): (Vector3i, Slot)) -> &mut Self::Output {
        self.get_mut(loc, slot).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infinite_grid_indexing() {
        let mut grid: Sparse3D<i32> = Sparse3D::new();

        // Set some values
        grid.set(Vector3i::new(1, 2, 3), Slot::XWall, 10);
        grid.set(Vector3i::new(-1, 5, 0), Slot::XWall, 20);
        grid.set(Vector3i::new(4, 0, 0), Slot::XWall, 30); // Different chunk

        // Get the values using indexing
        assert_eq!(grid[(Vector3i::new(1, 2, 3), Slot::XWall)], 10);
        assert_eq!(grid[(Vector3i::new(-1, 5, 0), Slot::XWall)], 20);
        assert_eq!(grid[(Vector3i::new(4, 0, 0), Slot::XWall)], 30);
    }
}
