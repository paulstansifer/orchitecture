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

pub struct Sparse3DIterator<'a, T> {
    big_coords_iter: std::collections::hash_map::Iter<'a, BigCoordinates, Chunk<T>>,
    current_chunk: Option<(&'a BigCoordinates, &'a Chunk<T>)>,
    small_coords_index: usize,
}

impl<'a, T> Sparse3DIterator<'a, T> {
    fn new(chunks: &'a HashMap<BigCoordinates, Chunk<T>>) -> Self {
        Sparse3DIterator {
            big_coords_iter: chunks.iter(),
            current_chunk: None,
            small_coords_index: 0,
        }
    }
}

impl<'a, T> Iterator for Sparse3DIterator<'a, T> {
    type Item = (Vector3i, Slot, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current_chunk.is_none() {
                self.current_chunk = self.big_coords_iter.next();
                self.small_coords_index = 0;

                if self.current_chunk.is_none() {
                    return None;
                }
            }

            let (big_coords, chunk) = self.current_chunk.unwrap();
            if self.small_coords_index >= 256 {
                self.current_chunk = None;
                continue;
            }

            let slot = match self.small_coords_index % 4 {
                0 => Slot::Room,
                1 => Slot::XWall,
                2 => Slot::YFloor,
                3 => Slot::ZWall,
                _ => unreachable!(),
            };
            let x = ((self.small_coords_index / 4) % 4) as u8;
            let y = ((self.small_coords_index / 16) % 4) as u8;
            let z = (self.small_coords_index / 64) as u8;

            self.small_coords_index += 1;

            let small_coords = SmallCoordinates { x, y, z, slot };

            if let Some(value) = &chunk[small_coords] {
                let loc = Vector3i::new(
                    big_coords.x * 4 + x as i32,
                    big_coords.y * 4 + y as i32,
                    big_coords.z * 4 + z as i32,
                );
                return Some((loc, slot, value));
            }
        }
    }
}

impl<'a, T> IntoIterator for &'a Sparse3D<T> {
    type Item = (Vector3i, Slot, &'a T);
    type IntoIter = Sparse3DIterator<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        Sparse3DIterator::new(&self.chunks)
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

    #[test]
    fn test_sparse_3d_iterator() {
        let mut grid: Sparse3D<i32> = Sparse3D::new();

        // Set some values
        grid.set(Vector3i::new(1, 2, 3), Slot::XWall, 10);
        grid.set(Vector3i::new(-1, 5, 0), Slot::YFloor, 20);
        grid.set(Vector3i::new(4, 0, 0), Slot::ZWall, 30);

        let items: std::collections::HashSet<_> = (&grid).into_iter().collect();

        let expected: std::collections::HashSet<_> = vec![
            (Vector3i::new(1, 2, 3), Slot::XWall, &10),
            (Vector3i::new(-1, 5, 0), Slot::YFloor, &20),
            (Vector3i::new(4, 0, 0), Slot::ZWall, &30),
        ]
        .into_iter()
        .collect();

        assert_eq!(items, expected);
    }
}
