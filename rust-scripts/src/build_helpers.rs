use std::collections::{HashMap, HashSet};

use godot::builtin::Vector3i;

use crate::sparse3d::{Facing, RelSlot};
use crate::structure::StructureInfo;
use crate::wall_grid::VantageEvaluation;
use crate::{
    sparse3d::{SlotLocation, Sparse3D},
    wall_grid::OfflineCell,
};
use rand::{Rng,rngs::StdRng,prelude::SliceRandom};

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

pub fn add_noise(
    s: Sparse3D<OfflineCell>,
    structure_info: &[StructureInfo],
    rng: &mut StdRng,
) -> Vec<Sparse3D<OfflineCell>> {
    use crate::sparse3d::RelSlot::{Floor, Room, XLoWall, ZLoWall};
    use crate::wall_grid::VantageEvaluation;

    let mut results: Vec<Sparse3D<OfflineCell>> = Vec::new();

    // Collect all vantages (locations with an evaluation)
    let mut vantages: Vec<(SlotLocation, OfflineCell)> = Vec::new();
    for (loc, cell) in s.iter() {
        if cell.evaluation.is_some() {
            vantages.push((loc, cell.clone()));
        }
    }

    // For each vantage, produce one "messed up" variant
    for (v_loc, v_cell) in vantages.iter() {
        // candidate cube coordinates within Manhattan radius < 4
        let mut candidates: Vec<(i32, i32, i32)> = Vec::new();
        for dx in -3..=3 {
            for dy in 0..=3 { // Only above; we likely can't see below.
                for dz in -3..=3 {
                    let (dx, dy, dz) = (dx as i32, dy as i32, dz as i32);
                    if dx.abs() + dy.abs() + dz.abs() < 4 {
                        candidates.push((v_loc.cube.x + dx, v_loc.cube.y + dy, v_loc.cube.z + dz));
                    }
                }
            }
        }

        candidates.shuffle(rng);

        // We'll select up to 9 coordinates that are visible (ray_trace returns empty)
        let mut selected: Vec<(SlotLocation, RelSlot)> = Vec::new();

        for (cx, cy, cz) in candidates.into_iter() {
            if selected.len() >= 9 {
                break;
            }

            // Choose a random slot kind for this coordinate
            let slot_choices = [Room, XLoWall, ZLoWall, Floor];
            let slot_idx = rng.random_range(0..slot_choices.len());
            let slot = slot_choices[slot_idx];

            let dest = SlotLocation::new(cx, cy, cz, slot);

            // Ray trace from the vantage room center to this slot; only accept if no obstacles
            let vantage_room = SlotLocation::new(v_loc.cube.x, v_loc.cube.y, v_loc.cube.z, Room);
            let obstacles = s.ray_trace(vantage_room, dest);
            if obstacles.is_empty() {
                selected.push((dest, slot));
            }
        }

        // Make a clone and apply modifications
        let mut new_s = s.clone();

        // For each selected dest, pick a replacement (or deletion if present)
        for (dest_loc, slot) in selected.into_iter() {
            // Determine candidate structure IDs matching the placement style for this slot
            let mut candidates_ids: Vec<i32> = Vec::new();

            for (idx, info) in structure_info.iter().enumerate() {
                match slot {
                    Room => {
                        if info.placement_style == crate::structure::PlacementStyle::RoomPlop {
                            candidates_ids.push(idx as i32);
                        }
                    }
                    Floor => {
                        if info.placement_style == crate::structure::PlacementStyle::FloorDrag {
                            candidates_ids.push(idx as i32);
                        }
                    }
                    XLoWall | ZLoWall => {
                        if info.placement_style == crate::structure::PlacementStyle::WallDrag
                            || info.placement_style == crate::structure::PlacementStyle::WallPlop
                        {
                            candidates_ids.push(idx as i32);
                        }
                    }
                    _ => {}
                }
            }

            // If something is already present, allow deletion as one option
            let existing = new_s.get(dest_loc);
            let mut options: Vec<Option<i32>> = Vec::new();
            if existing.is_some() {
                options.push(None);
            }

            for id in candidates_ids.into_iter() {
                if existing.map(|c| c.id) != Some(id) {
                    options.push(Some(id));
                }
            }

            if options.is_empty() {
                // nothing to do here
                continue;
            }

            let idx = rng.random_range(0..options.len());
            let choice = options[idx].clone();

            match choice {
                None => {
                    new_s.take(dest_loc);
                }
                Some(id) => {
                    let new_cell = OfflineCell {
                        id,
                        facing: crate::sparse3d::Facing::default(),
                        evaluation: None,
                    };
                    new_s.set(dest_loc, new_cell);
                }
            }
        }

        // Remove all other vantages (clear evaluations except for this one)
        for (loc, cell) in new_s.iter_mut() {
            if !(loc.cube == v_loc.cube && loc.rel_slot == v_loc.rel_slot) {
                cell.evaluation = None;
            }
        }

        // Edit the affected vantage: set symmetry to 0.25 * original symmetry
        if let Some(orig_eval) = v_cell.evaluation.as_ref() {
            if let Some(cell_mut) = new_s.get_mut(*v_loc) {
                cell_mut.evaluation = Some(VantageEvaluation {
                    symmetry: orig_eval.symmetry * 0.25,
                    interest: orig_eval.interest,
                });
            }
        }

        results.push(new_s);
    }

    results
}


#[test]
fn test_add_noise() {
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use crate::wall_grid::VantageEvaluation;

    let structures = crate::structure::load_structure_info();

    let mut s: Sparse3D<OfflineCell> = Sparse3D::new();

    let v_loc = SlotLocation::new(0, 0, 0, RelSlot::Room);
    s.set(
        v_loc,
        OfflineCell {
            id: 0,
            facing: crate::sparse3d::Facing::NegX,
            evaluation: Some(VantageEvaluation {
                symmetry: 1.0,
                interest: 0.5,
            }),
        },
    );

    // Add a nearby wall so there's something to possibly replace/delete
    s.set(
        SlotLocation::new(1, 0, 0, RelSlot::XLoWall),
        OfflineCell {
            id: 1,
            facing: crate::sparse3d::Facing::NegX,
            evaluation: None,
        },
    );

    let mut rng = StdRng::seed_from_u64(42);

    let res = add_noise(s.clone(), &structures, &mut rng);

    // We had one vantage, so expect one result
    assert_eq!(res.len(), 1);

    let out = &res[0];

    // Ensure all other evaluations are cleared
    for (loc, cell) in out.iter() {
        if !(loc.cube == v_loc.cube && loc.rel_slot == v_loc.rel_slot) {
            assert!(cell.evaluation.is_none());
        }
    }

    // Check symmetry scaled
    if let Some(cell) = out.get(v_loc) {
        if let Some(eval) = &cell.evaluation {
            assert!((eval.symmetry - 0.25).abs() < 1e-6);
        } else {
            panic!("vantage evaluation missing");
        }
    } else {
        panic!("vantage cell missing");
    }
}
