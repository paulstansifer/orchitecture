#![allow(dead_code)]
use std::collections::HashMap;
use std::ops::{Index, IndexMut};

use godot::builtin::Vector3i;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Room,
    XWall,
    YFloor,
    ZWall,
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

    pub fn iter(&self) -> impl Iterator<Item = (Vector3i, Slot, &T)> {
        self.chunks.iter().flat_map(|(bc, chunk)| {
            chunk.iter().map(move |(sc, value)| {
                let loc = Vector3i::new(
                    bc.x * 4 + sc.x as i32,
                    bc.y * 4 + sc.y as i32,
                    bc.z * 4 + sc.z as i32,
                );
                (loc, sc.slot, value)
            })
        })
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Vector3i, Slot, &mut T)> {
        self.chunks.iter_mut().flat_map(|(bc, chunk)| {
            chunk.iter_mut().map(move |(sc, value)| {
                let loc = Vector3i::new(
                    bc.x * 4 + sc.x as i32,
                    bc.y * 4 + sc.y as i32,
                    bc.z * 4 + sc.z as i32,
                );
                (loc, sc.slot, value)
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

    // TODO: When we can't summarize as a single character, use a '?', and stick map of coordinates
    // to ID and rotation at the end.
    pub fn serialize(&self, f: fn(&T, Slot) -> char) -> String {
        let mut serialized = String::new();
        let (min, max) = self.bounding_box();
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    for slot in [Slot::Room, Slot::ZWall] {
                        if let Some(value) = self.get(Vector3i::new(x, y, z), slot) {
                            serialized.push(f(value, slot))
                        } else {
                            serialized.push(' ');
                        }
                    }
                }
                serialized.push_str("\n");
                for x in min.x..=max.x {
                    for slot in [Slot::XWall, Slot::YFloor] {
                        if let Some(value) = self.get(Vector3i::new(x, y, z), slot) {
                            serialized.push(f(value, slot))
                        } else {
                            serialized.push(' ');
                        }
                    }
                }
                serialized.push_str("\n");
            }

            serialized.push_str("\n~~~~~~");
        }

        serialized
    }

    pub fn deserialize<F, E>(lines: &str, mut f: F) -> Result<Self, E>
    where
        F: FnMut(char, Slot) -> Result<T, E>,
    {
        let mut grid = Sparse3D::new();
        let mut y = 0;
        let mut z = 0;

        for line in lines.lines() {
            if line.starts_with("~~~~~~") {
                y += 1;
                z = 0;
                continue;
            }
            if line.is_empty() {
                continue;
            }

            let chars: Vec<char> = line.chars().collect();
            let mut x: usize = 0; // Everything gets nonnegative indices, and that's fine.

            while x < chars.len() {
                if chars[x] != ' ' {
                    let slot = match x % 2 {
                        0 => Slot::Room,
                        1 => Slot::ZWall,
                        _ => unreachable!(),
                    };
                    grid.set(Vector3i::new(x as i32 / 2, y, z), slot, f(chars[x], slot)?);
                }
                x += 1;
            }

            if let Some(next_line) = lines.lines().nth(1) {
                let chars: Vec<char> = next_line.chars().collect();
                let mut x = 0;
                while x < chars.len() {
                    if chars[x] != ' ' {
                        let slot = match x % 2 {
                            0 => Slot::XWall,
                            1 => Slot::YFloor,
                            _ => unreachable!(),
                        };
                        grid.set(Vector3i::new(x as i32 / 2, y, z), slot, f(chars[x], slot)?);
                    }
                    x += 1;
                }
            }

            z += 1;
        }

        Ok(grid)
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

        let items: std::collections::HashSet<_> = grid.iter().collect();

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
