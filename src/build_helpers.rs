use std::collections::{HashMap, HashSet};

use bevy::math::IVec3;
use enum_derived::Rand;

use crate::city::{ConstrainedScoreExt, VantageEvaluation};
use crate::eorf::{EorfId, EorfInfo};
use crate::materials::BuildMaterialId;
use crate::sparse3d::{Facing, RelSlot, Rotateable, Rotation};
use crate::{
    city::Cell,
    sparse3d::{RelSlotCoord, SlotCoord, Sparse3D},
};
use rand::{prelude::SliceRandom, rngs::StdRng, Rng};

#[derive(Clone)]
pub struct Builder {
    map: Sparse3D<Cell>,
    structures: HashMap<String, usize>,
}

impl Builder {
    pub fn new(structures: &Vec<EorfInfo>) -> Self {
        Self {
            map: Sparse3D::new(),
            structures: structures
                .iter()
                .enumerate()
                .map(|(i, s)| (s.name.clone(), i))
                .collect(),
        }
    }

    pub fn get(self) -> Sparse3D<Cell> {
        self.map
    }

    fn wall(&self) -> Cell {
        Cell {
            id: EorfId(*self.structures.get("wall").unwrap() as u32),
            facing: Facing::arbitrary(),
            evaluation: None,
            build_material: BuildMaterialId::default(),
        }
    }
    fn flat(&self) -> Cell {
        Cell {
            id: EorfId(*self.structures.get("floor").unwrap() as u32),
            facing: Facing::arbitrary(),
            evaluation: None,
            build_material: BuildMaterialId::default(),
        }
    }

    pub fn build_box(&mut self, corner_a: IVec3, corner_b: IVec3) {
        let min = IVec3::min(corner_a, corner_b);
        let max = IVec3::max(corner_a, corner_b);

        for x in min.x..=max.x {
            for z in min.z..=max.z {
                self.map
                    .set(RelSlotCoord::new(x, min.y, z, RelSlot::Floor), self.flat());
                self.map.set(
                    RelSlotCoord::new(x, max.y, z, RelSlot::Ceiling),
                    self.flat(),
                );
            }
        }

        for x in min.x..=max.x {
            for y in min.y..=max.y {
                self.map.set(
                    RelSlotCoord::new(x, y, min.z, RelSlot::ZLoWall),
                    self.wall(),
                );
                self.map.set(
                    RelSlotCoord::new(x, y, max.z, RelSlot::ZHiWall),
                    self.wall(),
                );
            }
        }

        for y in min.y..=max.y {
            for z in min.z..=max.z {
                self.map.set(
                    RelSlotCoord::new(min.x, y, z, RelSlot::XLoWall),
                    self.wall(),
                );
                self.map.set(
                    RelSlotCoord::new(max.x, y, z, RelSlot::XHiWall),
                    self.wall(),
                );
            }
        }
    }

    pub fn build_union_boxes(&mut self, boxes: &[(IVec3, IVec3)]) {
        let mut inside_coords: HashSet<IVec3> = HashSet::new();
        for (a, b) in boxes {
            let min = IVec3::min(*a, *b);
            let max = IVec3::max(*a, *b);

            for x in min.x..=max.x {
                for y in min.y..=max.y {
                    for z in min.z..=max.z {
                        inside_coords.insert(IVec3::new(x, y, z));
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
                        .set(RelSlotCoord::new(coord.x, coord.y, coord.z, slot), cell);
                }
            }
        }
    }

    pub fn build_plane(
        &mut self,
        corner_a: IVec3,
        corner_b: IVec3,
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
            obj.id = EorfId(*self.structures.get(name).unwrap() as u32);
        }

        let min = IVec3::min(corner_a, corner_b);
        let max = IVec3::max(corner_a, corner_b);

        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    self.map.set(RelSlotCoord::new(x, y, z, slot), obj.clone());
                }
            }
        }
    }

    pub fn wall_off_drops(&mut self, corner_a: IVec3, corner_b: IVec3, obj_name: &str) {
        assert!(corner_a.y == corner_b.y);

        let min = IVec3::min(corner_a, corner_b);
        let max = IVec3::max(corner_a, corner_b);
        let y = min.y;

        let obj = Cell {
            id: EorfId(*self.structures.get(obj_name).unwrap() as u32),
            facing: Facing::arbitrary(),
            evaluation: None,
            build_material: BuildMaterialId::default(),
        };

        for x in min.x..=max.x {
            for z in min.z..=max.z {
                for slot in [
                    RelSlot::XLoWall,
                    RelSlot::XHiWall,
                    RelSlot::ZLoWall,
                    RelSlot::ZHiWall,
                ] {
                    let here = RelSlotCoord::new(x, y, z, RelSlot::Floor);
                    let neighbor = here + slot.direction_of_neighbor();
                    let separator = RelSlotCoord {
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

    pub fn set_vantage(
        &mut self,
        loc: IVec3,
        coherence: impl Into<crate::city::ConstrainedScore>,
        interest: impl Into<crate::city::ConstrainedScore>,
    ) {
        self.map.set(
            RelSlotCoord::new(loc.x, loc.y, loc.z, RelSlot::Room),
            Cell {
                id: EorfId(*self.structures.get("desk").unwrap() as u32),
                facing: Facing::arbitrary(), // doesn't matter, but maybe someday it would
                evaluation: Some(VantageEvaluation {
                    interest: Some(interest.into()),
                    coherence: Some(coherence.into()),
                }),
                build_material: BuildMaterialId::default(),
            },
        );
    }
}

pub fn make_boring_room(structures: &Vec<EorfInfo>, rng: &mut StdRng) -> (Sparse3D<Cell>, String) {
    let mut builder = Builder::new(structures);
    let x_size = rng.random_range(1..7);
    let y_height = rng.random_range(1..4);
    let z_size = rng.random_range(1..7);
    builder.build_box(IVec3::new(0, 0, 0), IVec3::new(x_size, y_height, z_size));
    let door_loc = IVec3::new(rng.random_range(0..=x_size), 0, 0);
    builder.build_plane(door_loc, door_loc, RelSlot::ZLoWall, Some("doorway"));
    builder.set_vantage(
        IVec3::new(rng.random_range(0..x_size), 0, rng.random_range(0..z_size)),
        1.0,
        0.0,
    );
    let result = builder.get();
    (
        result.rotate(Rotation::rand()),
        format!("boring-{x_size}x{z_size}"),
    )
}

pub fn add_noise(
    s: Sparse3D<Cell>,
    structure_info: &[EorfInfo],
    rng: &mut StdRng,
) -> Vec<Sparse3D<Cell>> {
    use crate::city::VantageEvaluation;
    use crate::sparse3d::RelSlot::{Floor, Room, XLoWall, ZLoWall};

    let mut results: Vec<Sparse3D<Cell>> = Vec::new();

    // Collect all vantages (locations with an evaluation)
    let mut vantages: Vec<(SlotCoord, Cell)> = Vec::new();
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
            for dy in 0..=3 {
                // Only above; we likely can't see below.
                for dz in -3..=3 {
                    let (dx, dy, dz) = (dx as i32, dy as i32, dz as i32);
                    if dx.abs() + dy.abs() + dz.abs() < 4 && (dx != 0 || dy != 0 || dz != 0) {
                        candidates.push((v_loc.cube.x + dx, v_loc.cube.y + dy, v_loc.cube.z + dz));
                    }
                }
            }
        }

        candidates.shuffle(rng);

        // We'll select up to 9 coordinates that are visible (ray_trace returns empty)
        let mut selected: Vec<(RelSlotCoord, RelSlot)> = Vec::new();

        for (cx, cy, cz) in candidates.into_iter() {
            if selected.len() >= 13 {
                break;
            }

            let slot = RelSlot::rand();
            let dest = RelSlotCoord::new(cx, cy, cz, slot);

            // Ray trace from the vantage room center to this slot; only accept if no obstacles
            let vantage_room = RelSlotCoord::new(v_loc.cube.x, v_loc.cube.y, v_loc.cube.z, Room);
            let obstacles = s.ray_trace(vantage_room, dest);
            if obstacles.len() > 1 {
                selected.push((dest, slot));
            } else {
                // println!("Obstacles: {:?}", obstacles);
            }
        }
        assert!(
            selected.len() > 3,
            "Need to find at least a couple things to edit"
        );

        // Make a clone and apply modifications
        let mut new_s = s.clone();

        // For each selected dest, pick a replacement (or deletion if present)
        for (dest_loc, slot) in selected.into_iter() {
            // Determine candidate structure IDs matching the placement style for this slot
            let mut candidates_ids: Vec<EorfId> = Vec::new();

            for (idx, info) in structure_info.iter().enumerate() {
                match slot {
                    Room => {
                        if info.placement_style == crate::eorf::PlacementStyle::RoomPlop {
                            candidates_ids.push(EorfId(idx as u32));
                        }
                    }
                    Floor => {
                        if info.placement_style == crate::eorf::PlacementStyle::FloorDrag {
                            candidates_ids.push(EorfId(idx as u32));
                        }
                    }
                    XLoWall | ZLoWall => {
                        if info.placement_style == crate::eorf::PlacementStyle::WallDrag
                            || info.placement_style == crate::eorf::PlacementStyle::WallPlop
                        {
                            candidates_ids.push(EorfId(idx as u32));
                        }
                    }
                    _ => {}
                }
            }

            // If something is already present, allow deletion as one option
            let existing = new_s.get(dest_loc);
            let mut options: Vec<Option<EorfId>> = Vec::new();
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
            let choice = options[idx];

            match choice {
                None => {
                    new_s.take(dest_loc);
                }
                Some(id) => {
                    let new_cell = Cell {
                        id,
                        facing: crate::sparse3d::Facing::default(),
                        evaluation: None,
                        build_material: BuildMaterialId::default(),
                    };
                    new_s.set(dest_loc, new_cell);
                }
            }
        }

        // Remove all other vantages (clear evaluations except for this one)
        for (loc, cell) in new_s.iter_mut() {
            if !(loc.cube == v_loc.cube && loc.slot == v_loc.slot) {
                cell.evaluation = None;
            }
        }

        // Noisy room should score at least 0.3 worse on coherence than the original.
        if let Some(orig_eval) = v_cell.evaluation.as_ref() {
            if let Some(cell_mut) = new_s.get_mut(*v_loc) {
                cell_mut.evaluation = Some(VantageEvaluation {
                    coherence: orig_eval.coherence.subtract(0.3).unbound_lower(),
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
    use crate::city::{ConstrainedScore, VantageEvaluation};
    use assert2::{assert, check};
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    let structures = crate::eorf::load_structure_info();
    let desk_id = crate::eorf::find_structure_by_name(&structures, "desk").unwrap();
    let doorway_id = crate::eorf::find_structure_by_name(&structures, "doorway").unwrap();

    let mut s: Sparse3D<Cell> = Sparse3D::new();

    let v_loc = RelSlotCoord::new(0, 0, 0, RelSlot::Room);
    s.set(
        v_loc,
        Cell {
            id: desk_id,
            facing: crate::sparse3d::Facing::NegX,
            evaluation: Some(VantageEvaluation {
                coherence: Some(ConstrainedScore::Exact(1.0)),
                interest: Some(ConstrainedScore::Exact(0.5)),
            }),
            build_material: BuildMaterialId::default(),
        },
    );

    // Add a nearby wall so there's something to possibly replace/delete
    s.set(
        RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall),
        Cell {
            id: doorway_id,
            facing: crate::sparse3d::Facing::NegX,
            evaluation: None,
            build_material: BuildMaterialId::default(),
        },
    );

    let mut rng = StdRng::seed_from_u64(42);

    let res = add_noise(s.clone(), &structures, &mut rng);

    check!(res.len() == 1); // We had one vantage.

    let out = &res[0];

    for (loc, cell) in out.iter() {
        if loc != SlotCoord::from(v_loc) {
            check!(cell.evaluation.is_none(), "other score should be cleared");
        }
    }

    assert!(let Some(cell) = out.get(v_loc));
    assert!(let Some(eval) = &cell.evaluation);
    // original coherence 1.0, subtract 0.3, unbound_lower → AtMost { at_most: 0.7 }
    check!(eval.coherence == Some(ConstrainedScore::AtMost { at_most: 0.7 }));
}
