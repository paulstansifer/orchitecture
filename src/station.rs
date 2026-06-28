use crate::resource::{Approximation, Inventory, UniformResource};
use crate::sparse3d::{Facing, Slot, SlotCoord};
use crate::structure::StructureList;
use crate::world::{apply_changes, AssembledWorld, Cell, ConstructedWorld, Material};
use bevy::math::IVec3;
use bevy::prelude::{Commands, Res, ResMut};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StationReq {
    /// Name of the required structure, resolved to a `StructureId` when needed.
    pub structure: String,
    pub min: u8,
    pub max: Option<u8>,
    pub worker_visit_weight: f32,
    pub worker_visit_duration: f32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StationId(pub u32);

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StationStorageSpec {
    pub just_one_kind: bool,
    pub accounting: Approximation,
    // max storage space is 20.0 * bins + 10.0 * racks
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StationInfo {
    pub name: String,
    // First requirement is the core structure.
    pub requirements: Vec<StationReq>,
    pub storage: Option<StationStorageSpec>,
}

/// A placed station instance.
pub struct ParticularStation {
    /// Index into `WallGrid.stations`.
    pub station: usize,
    // First location is the core.
    pub structure_locations: Vec<IVec3>,
    pub contents: Inventory,
}

/// Loads the station definitions bundled at compile time.
pub fn load_station_info() -> Vec<StationInfo> {
    let ron_content = include_str!("../buildables/stations.ron");
    ron::from_str(ron_content).unwrap()
}

/// Maximum 2D Manhattan distance (within a single y-layer) for a structure to
/// count as belonging to a station. Tunable: the 4×3 starting room spans ~5.
pub const STATION_DIST: i32 = 6;

fn manhattan2d(a: IVec3, b: IVec3) -> i32 {
    (a.x - b.x).abs() + (a.z - b.z).abs()
}

/// All furniture cubes named `name` within `STATION_DIST` (2D Manhattan, same
/// y-layer) of `origin`. Includes `origin` itself when it qualifies.
fn furniture_of_name_near(cw: &ConstructedWorld, origin: IVec3, name: &str) -> Vec<IVec3> {
    let mut found = Vec::new();
    for dx in -STATION_DIST..=STATION_DIST {
        let zspan = STATION_DIST - dx.abs();
        for dz in -zspan..=zspan {
            let cube = IVec3::new(origin.x + dx, origin.y, origin.z + dz);
            let loc = SlotCoord {
                cube,
                slot: Slot::Room,
            };
            if let Some(cell) = cw.contents.get(loc) {
                let info = &cw.structures[cell.id.as_usize()];
                if info.furniture && info.name == name {
                    found.push(cube);
                }
            }
        }
    }
    found
}

/// True if `core` has at least `min` of every required structure within range.
fn requirements_met(cw: &ConstructedWorld, core: IVec3, station: &StationInfo) -> bool {
    station
        .requirements
        .iter()
        .all(|req| furniture_of_name_near(cw, core, &req.structure).len() >= req.min as usize)
}

/// Choose the core structure for `station_idx` nearest to the clicked `cube`
/// (the cube itself preferred) whose surroundings satisfy every requirement.
fn choose_core(cw: &ConstructedWorld, cube: IVec3, station_idx: usize) -> Option<IVec3> {
    let station = &cw.stations[station_idx];
    let core_name = &station.requirements[0].structure;
    let mut cores = furniture_of_name_near(cw, cube, core_name);
    cores.sort_by_key(|c| (*c != cube, manhattan2d(*c, cube)));
    cores
        .into_iter()
        .find(|core| requirements_met(cw, *core, station))
}

/// Stations (indices into `cw.stations`) that could be formed around the
/// furniture at `cube`.
pub fn valid_stations_for(cw: &ConstructedWorld, cube: IVec3) -> Vec<usize> {
    (0..cw.stations.len())
        .filter(|&idx| choose_core(cw, cube, idx).is_some())
        .collect()
}

/// The placed-station index (into `cw.placed_stations`) that owns the
/// furniture at `cube`, if any.
pub fn station_index_at(cw: &ConstructedWorld, cube: IVec3) -> Option<usize> {
    cw.placed_stations
        .iter()
        .position(|ps| ps.structure_locations.contains(&cube))
}

/// A pre-computed assignment, shared by the panel preview and the commit so the
/// displayed "Pulls {N}" can never disagree with the actual effect.
pub struct AssignmentPlan {
    /// Structure cubes the new station will own.
    pub chosen: Vec<IVec3>,
    /// How many of `chosen` had to be pulled from other stations to meet a min.
    pub pulled: usize,
    /// Indices into `placed_stations` that drop below a min and must be destroyed.
    pub destroy: Vec<usize>,
}

/// Plan assigning structures to a new instance of `station_idx` around `cube`.
/// Prefers unassigned structures; only pulls from other stations to reach `min`.
pub fn plan_assignment(
    cw: &ConstructedWorld,
    cube: IVec3,
    station_idx: usize,
) -> Option<AssignmentPlan> {
    let core = choose_core(cw, cube, station_idx)?;
    let station = &cw.stations[station_idx];

    let mut chosen: Vec<IVec3> = Vec::new();
    // For each donor station, which of its locations we'd take.
    let mut pulled_from: std::collections::HashMap<usize, Vec<IVec3>> =
        std::collections::HashMap::new();

    for req in &station.requirements {
        let max = req.max.map(|m| m as usize).unwrap_or(usize::MAX);
        let min = req.min as usize;

        // Partition reachable structures into unassigned ("free") and those
        // already owned by another station, keeping each owner's index.
        let mut free: Vec<IVec3> = Vec::new();
        let mut assigned: Vec<(IVec3, usize)> = Vec::new();
        for c in furniture_of_name_near(cw, core, &req.structure) {
            if chosen.contains(&c) {
                continue;
            }
            match station_index_at(cw, c) {
                None => free.push(c),
                Some(owner) => assigned.push((c, owner)),
            }
        }

        let mut taken: Vec<IVec3> = free.into_iter().take(max).collect();
        // Only pull from other stations if free ones can't satisfy the minimum.
        for (c, owner) in assigned {
            if taken.len() >= min || taken.len() >= max {
                break;
            }
            pulled_from.entry(owner).or_default().push(c);
            taken.push(c);
        }
        chosen.extend(taken);
    }

    // A donor station is destroyed if, after losing its pulled structures, it no
    // longer meets some minimum.
    let mut destroy = Vec::new();
    for (&ps_idx, pulled_locs) in &pulled_from {
        let ps = &cw.placed_stations[ps_idx];
        let def = &cw.stations[ps.station];
        let still_meets = def.requirements.iter().all(|req| {
            ps.structure_locations
                .iter()
                .filter(|l| !pulled_locs.contains(l))
                .filter(|l| {
                    cw.contents
                        .get(SlotCoord {
                            cube: **l,
                            slot: Slot::Room,
                        })
                        .map(|c| cw.structures[c.id.as_usize()].name == req.structure)
                        .unwrap_or(false)
                })
                .count()
                >= req.min as usize
        });
        if !still_meets {
            destroy.push(ps_idx);
        }
    }
    // Descending so `commit_assignment` can `remove` by index without shifting.
    destroy.sort_unstable_by(|a, b| b.cmp(a));

    let pulled = pulled_from.values().map(Vec::len).sum();
    Some(AssignmentPlan {
        chosen,
        pulled,
        destroy,
    })
}

/// Commit an assignment: create the station, pulling/destroying as planned.
pub fn commit_assignment(cw: &mut ConstructedWorld, cube: IVec3, station_idx: usize) {
    let Some(plan) = plan_assignment(cw, cube, station_idx) else {
        return;
    };

    // Take chosen structures away from any station currently holding them.
    for ps in &mut cw.placed_stations {
        ps.structure_locations.retain(|l| !plan.chosen.contains(l));
    }

    // Destroy donor stations that fell below a minimum. `plan.destroy` is sorted
    // descending so earlier indices stay valid. Their inventory is discarded.
    for idx in &plan.destroy {
        cw.placed_stations.remove(*idx);
    }

    let max_volume = 20.0 * plan.chosen.len() as f32;
    cw.placed_stations.push(ParticularStation {
        station: station_idx,
        structure_locations: plan.chosen,
        contents: Inventory::new(8, max_volume),
    });
}

/// Remove a placed station, discarding its inventory contents.
pub fn unassign_station(cw: &mut ConstructedWorld, idx: usize) {
    if idx < cw.placed_stations.len() {
        cw.placed_stations.remove(idx);
    }
}

fn is_storage(cw: &ConstructedWorld, ps_idx: usize) -> bool {
    cw.stations
        .get(cw.placed_stations[ps_idx].station)
        .is_some_and(|info| info.storage.is_some())
}

/// Total quantity of `res` held across all storage stations.
pub fn total_uniform(cw: &ConstructedWorld, res: UniformResource) -> u32 {
    (0..cw.placed_stations.len())
        .filter(|&i| is_storage(cw, i))
        .flat_map(|i| cw.placed_stations[i].contents.uniform_totals())
        .filter(|(r, _)| *r == res)
        .map(|(_, q)| q as u32)
        .sum()
}

/// Remove `qty` of `res` from storage stations, spreading the deduction across
/// stations in order. Returns `true` and commits if the total held is ≥ `qty`;
/// returns `false` and makes no changes otherwise.
pub fn consume_uniform(cw: &mut ConstructedWorld, res: UniformResource, qty: u32) -> bool {
    if total_uniform(cw, res) < qty {
        return false;
    }
    let mut remaining = qty;
    for i in 0..cw.placed_stations.len() {
        if remaining == 0 {
            break;
        }
        if !is_storage(cw, i) {
            continue;
        }
        let here = cw.placed_stations[i]
            .contents
            .uniform_totals()
            .into_iter()
            .find(|(r, _)| *r == res)
            .map(|(_, q)| q as u32)
            .unwrap_or(0);
        let take = here.min(remaining);
        if take > 0 {
            cw.placed_stations[i]
                .contents
                .subtract_uniform(res, take as u16);
            remaining -= take;
        }
    }
    true
}

/// The starting storage room: a 4×3 area set one cell back from the road's NE
/// inside corner. The E-W road occupies z ∈ [0, 4); the north arm occupies
/// x ∈ [0, 4) for z ≥ 4. Stepping one cell off both road edges puts the room at
/// x ∈ [5, 9), z ∈ [5, 8).
const ROOM_X: std::ops::Range<i32> = 5..9;
const ROOM_Z: std::ops::Range<i32> = 5..8;
const NUM_BINS: usize = 5;

/// Startup system: places the starting storage room with randomly-positioned bins,
/// pre-stocked with potatoes, timber, and canvas. Must run after `spawn_grid`.
pub fn spawn_initial_station(
    mut commands: Commands,
    structure_list: Res<StructureList>,
    mut constructed: ResMut<ConstructedWorld>,
    mut assembled: ResMut<AssembledWorld>,
) {
    let Some(bin_id) = constructed.find_structure_by_name("bin") else {
        return;
    };
    let Some(storage_room_index) = constructed
        .stations
        .iter()
        .position(|s| s.name == "storage room")
    else {
        return;
    };

    // Pick NUM_BINS distinct cells from the 4×3 footprint.
    let mut candidates: Vec<IVec3> = Vec::new();
    for x in ROOM_X {
        for z in ROOM_Z {
            candidates.push(IVec3::new(x, 0, z));
        }
    }
    use rand::seq::SliceRandom;
    let mut rng = rand::rng();
    candidates.shuffle(&mut rng);
    let chosen: Vec<IVec3> = candidates.into_iter().take(NUM_BINS).collect();

    // Place the bins as real cells and spawn their meshes.
    let mut changes: Vec<(SlotCoord, Option<Cell>)> = Vec::new();
    for cube in &chosen {
        let loc = SlotCoord {
            cube: *cube,
            slot: Slot::Room,
        };
        let cell = Cell {
            id: bin_id,
            facing: Facing::default(),
            evaluation: None,
            material: Material::Planks,
        };
        constructed.contents.set(loc, cell.clone());
        changes.push((loc, Some(cell)));
    }

    // Place market stands opposite the stockpile (south of the E-W road at z = -1),
    // with one space between each structure.
    let Some(market_stand_id) = constructed.find_structure_by_name("market stand") else {
        apply_changes(&mut commands, &mut assembled, &structure_list, changes);
        return;
    };
    let Some(market_stand_station_index) = constructed
        .stations
        .iter()
        .position(|s| s.name == "market stand")
    else {
        apply_changes(&mut commands, &mut assembled, &structure_list, changes);
        return;
    };

    let market_stand_positions = [
        IVec3::new(1, 0, -1),
        IVec3::new(3, 0, -1),
        IVec3::new(5, 0, -1),
    ];
    for cube in &market_stand_positions {
        let loc = SlotCoord {
            cube: *cube,
            slot: Slot::Room,
        };
        let cell = Cell {
            id: market_stand_id,
            facing: Facing::default(),
            evaluation: None,
            material: Material::Planks,
        };
        constructed.contents.set(loc, cell.clone());
        changes.push((loc, Some(cell)));
    }

    apply_changes(&mut commands, &mut assembled, &structure_list, changes);

    // Stock the inventory and register the storage room station.
    let mut inv = Inventory::new(8, 20.0 * NUM_BINS as f32);
    inv.add_uniform(UniformResource::Potato, 9);
    inv.add_uniform(UniformResource::Timber, 20);
    inv.add_uniform(UniformResource::Canvas, 10);

    constructed.placed_stations.push(ParticularStation {
        station: storage_room_index,
        structure_locations: chosen,
        contents: inv,
    });

    // Register each market stand as its own station.
    for cube in &market_stand_positions {
        constructed.placed_stations.push(ParticularStation {
            station: market_stand_station_index,
            structure_locations: vec![*cube],
            contents: Inventory::new(8, 20.0),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure::{PlacementStyle, StructureEmbedding, StructureInfo};

    fn bin_structures() -> Vec<StructureInfo> {
        vec![StructureInfo {
            name: "bin".to_string(),
            structure_type: crate::materials::StructureType::Furniture,
            placement_style: PlacementStyle::RoomPlop,
            x_char: None,
            z_char: None,
            embedding: StructureEmbedding {
                tall: 0.0,
                passable: 0.0,
                decorative: 0.0,
                striated: 0.0,
            },
            furniture: true,
        }]
    }

    fn station_def(min: u8, max: Option<u8>) -> StationInfo {
        StationInfo {
            name: "storage room".to_string(),
            requirements: vec![StationReq {
                structure: "bin".to_string(),
                min,
                max,
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            storage: None,
        }
    }

    fn grid_with_bins(def: StationInfo, bins: &[IVec3]) -> ConstructedWorld {
        let mut cw = ConstructedWorld::new(bin_structures());
        cw.road_forbidden_zone = false;
        cw.stations = vec![def];
        let bin_id = cw.find_structure_by_name("bin").unwrap();
        for cube in bins {
            cw.contents.set(
                SlotCoord {
                    cube: *cube,
                    slot: Slot::Room,
                },
                Cell {
                    id: bin_id,
                    facing: Facing::default(),
                    evaluation: None,
                    material: Material::Planks,
                },
            );
        }
        cw
    }

    fn b(x: i32, z: i32) -> IVec3 {
        IVec3::new(x, 0, z)
    }

    #[test]
    fn assigns_all_free_bins_without_pulling() {
        let grid = grid_with_bins(station_def(1, None), &[b(0, 0), b(0, 1), b(0, 2)]);
        let plan = plan_assignment(&grid, b(0, 0), 0).unwrap();
        assert_eq!(plan.chosen.len(), 3);
        assert_eq!(plan.pulled, 0);
        assert!(plan.destroy.is_empty());
    }

    #[test]
    fn unlimited_max_grabs_every_reachable_bin() {
        // A bin 10 apart is outside STATION_DIST (6) and must be excluded.
        let grid = grid_with_bins(station_def(1, None), &[b(0, 0), b(0, 1), b(10, 0)]);
        let plan = plan_assignment(&grid, b(0, 0), 0).unwrap();
        assert!(plan.chosen.contains(&b(0, 0)));
        assert!(plan.chosen.contains(&b(0, 1)));
        assert!(!plan.chosen.contains(&b(10, 0)));
    }

    #[test]
    fn pulls_to_meet_min_and_destroys_starved_donor() {
        // Station needs min 2 bins. An existing station owns two of three bins.
        let mut grid = grid_with_bins(station_def(2, None), &[b(0, 0), b(0, 1), b(0, 2)]);
        grid.placed_stations.push(ParticularStation {
            station: 0,
            structure_locations: vec![b(0, 1), b(0, 2)],
            contents: Inventory::new(8, 40.0),
        });

        // Right-click the free bin and form a new station from it.
        let plan = plan_assignment(&grid, b(0, 0), 0).unwrap();
        assert_eq!(plan.pulled, 1, "should pull exactly one bin to reach min 2");
        assert_eq!(
            plan.destroy,
            vec![0],
            "donor falls below min and is destroyed"
        );

        commit_assignment(&mut grid, b(0, 0), 0);
        assert_eq!(
            grid.placed_stations.len(),
            1,
            "donor destroyed, new one added"
        );
        let new = &grid.placed_stations[0];
        assert!(new.structure_locations.contains(&b(0, 0)));
        assert_eq!(new.structure_locations.len(), 2);
    }

    #[test]
    fn unassign_removes_the_station() {
        let mut grid = grid_with_bins(station_def(1, None), &[b(0, 0)]);
        grid.placed_stations.push(ParticularStation {
            station: 0,
            structure_locations: vec![b(0, 0)],
            contents: Inventory::new(8, 20.0),
        });
        assert_eq!(station_index_at(&grid, b(0, 0)), Some(0));
        unassign_station(&mut grid, 0);
        assert!(grid.placed_stations.is_empty());
        assert_eq!(station_index_at(&grid, b(0, 0)), None);
    }

    #[test]
    fn unused_furniture_not_part_of_any_station() {
        let grid = grid_with_bins(station_def(1, None), &[b(0, 0)]);
        assert_eq!(station_index_at(&grid, b(0, 0)), None);
        assert_eq!(valid_stations_for(&grid, b(0, 0)), vec![0]);
    }
}
