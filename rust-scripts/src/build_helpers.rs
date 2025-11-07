use std::collections::{HashMap, HashSet};

use godot::builtin::Vector3i;

use crate::sparse3d::{Facing, RelSlot};
use crate::structure::StructureInfo;
use crate::wall_grid::VantageEvaluation;
use crate::{
    sparse3d::{SlotLocation, Sparse3D},
    wall_grid::OfflineCell,
};

#[derive(Clone)]
pub struct Builder {
    map: Sparse3D<OfflineCell>,
    structures: HashMap<String, usize>,
}

impl Builder {
    pub fn new(structures: &Vec<StructureInfo>) -> Self {
        let base_name = |s: &str| s.splitn(2, ".").next().unwrap().to_string();

        Self {
            map: Sparse3D::new(),
            structures: structures
                .iter()
                .enumerate()
                .map(|(i, s)| (base_name(&s.main_mesh), i))
                .collect(),
        }
    }

    pub fn get(self) -> Sparse3D<OfflineCell> {
        self.map
    }

    fn wall(&self) -> OfflineCell {
        OfflineCell {
            id: *self.structures.get("wall").unwrap() as i32,
            facing: Facing::arbitrary(),
            evaluation: None,
        }
    }
    fn flat(&self) -> OfflineCell {
        OfflineCell {
            id: *self.structures.get("floor").unwrap() as i32,
            facing: Facing::arbitrary(),
            evaluation: None,
        }
    }

    pub fn build_box(&mut self, corner_a: Vector3i, corner_b: Vector3i) {
        let min = Vector3i::coord_min(corner_a, corner_b);
        let max = Vector3i::coord_max(corner_a, corner_b);

        for x in min.x..=max.x {
            for z in min.z..=max.z {
                self.map
                    .set(SlotLocation::new(x, min.y, z, RelSlot::Floor), self.flat());
                self.map.set(
                    SlotLocation::new(x, max.y, z, RelSlot::Ceiling),
                    self.flat(),
                );
            }
        }

        for x in min.x..=max.x {
            for y in min.y..=max.y {
                self.map.set(
                    SlotLocation::new(x, y, min.z, RelSlot::ZLoWall),
                    self.wall(),
                );
                self.map.set(
                    SlotLocation::new(x, y, max.z, RelSlot::ZHiWall),
                    self.wall(),
                );
            }
        }

        for y in min.y..=max.y {
            for z in min.z..=max.z {
                self.map.set(
                    SlotLocation::new(min.x, y, z, RelSlot::XLoWall),
                    self.wall(),
                );
                self.map.set(
                    SlotLocation::new(max.x, y, z, RelSlot::XHiWall),
                    self.wall(),
                );
            }
        }
    }

    pub fn build_union_boxes(&mut self, boxes: &[(Vector3i, Vector3i)]) {
        let mut inside_coords: HashSet<Vector3i> = HashSet::new();
        for (a, b) in boxes {
            let min = Vector3i::coord_min(*a, *b);
            let max = Vector3i::coord_max(*a, *b);

            for x in min.x..=max.x {
                for y in min.y..=max.y {
                    for z in min.z..=max.z {
                        inside_coords.insert(Vector3i::new(x, y, z));
                    }
                }
            }
        }

        for coord in inside_coords.iter() {
            for slot in [
                RelSlot::XLoWall,
                RelSlot::XHiWall,
                RelSlot::ZLoWall,
                RelSlot::ZHiWall,
                RelSlot::Floor,
                RelSlot::Ceiling,
            ] {
                let neighbor = *coord + slot.direction_of_neighbor();
                if !inside_coords.contains(&neighbor) {
                    let cell = if slot == RelSlot::Floor || slot == RelSlot::Ceiling {
                        self.flat()
                    } else {
                        self.wall()
                    };
                    self.map
                        .set(SlotLocation::new(coord.x, coord.y, coord.z, slot), cell);
                }
            }
        }
    }

    pub fn build_plane(
        &mut self,
        corner_a: Vector3i,
        corner_b: Vector3i,
        slot: RelSlot,
        obj_name: Option<&str>,
    ) {
        let mut obj = match slot {
            RelSlot::XLoWall | RelSlot::XHiWall => {
                assert!(corner_a.x == corner_b.x);
                self.wall()
            }
            RelSlot::ZLoWall | RelSlot::ZHiWall => {
                assert!(corner_a.z == corner_b.z);
                self.wall()
            }
            RelSlot::Floor | RelSlot::Ceiling => {
                assert!(corner_a.y == corner_b.y);
                self.flat()
            }
            _ => {
                panic!()
            }
        };

        if let Some(name) = obj_name {
            obj.id = *self.structures.get(name).unwrap() as i32;
        }

        let min = Vector3i::coord_min(corner_a, corner_b);
        let max = Vector3i::coord_max(corner_a, corner_b);

        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    self.map.set(SlotLocation::new(x, y, z, slot), obj.clone());
                }
            }
        }
    }

    pub fn wall_off_drops(&mut self, corner_a: Vector3i, corner_b: Vector3i, obj_name: &str) {
        assert!(corner_a.y == corner_b.y);

        let min = Vector3i::coord_min(corner_a, corner_b);
        let max = Vector3i::coord_max(corner_a, corner_b);
        let y = min.y;

        let obj = OfflineCell {
            id: *self.structures.get(obj_name).unwrap() as i32,
            facing: Facing::arbitrary(),
            evaluation: None,
        };

        for x in min.x..=max.x {
            for z in min.z..=max.z {
                for slot in [
                    RelSlot::XLoWall,
                    RelSlot::XHiWall,
                    RelSlot::ZLoWall,
                    RelSlot::ZHiWall,
                ] {
                    let here = SlotLocation::new(x, y, z, RelSlot::Floor);
                    let neighbor = here + slot.direction_of_neighbor();
                    let separator = SlotLocation {
                        rel_slot: slot,
                        ..here
                    };
                    if self.map.get(here).is_some()
                        && self.map.get(neighbor).is_none()
                        && self.map.get(separator).is_none()
                    {
                        self.map.set(separator, obj.clone());
                    }
                }
            }
        }
    }

    pub fn set_vantage(&mut self, loc: Vector3i, symmetry: f32, interest: f32) {
        self.map.set(
            SlotLocation::new(loc.x, loc.y, loc.z, RelSlot::Room),
            OfflineCell {
                id: *self.structures.get("desk").unwrap() as i32,
                facing: Facing::arbitrary(), // doesn't matter, but maybe someday it would
                evaluation: Some(VantageEvaluation { interest, symmetry }),
            },
        );
    }
}
