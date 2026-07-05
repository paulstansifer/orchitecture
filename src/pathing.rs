//! Route-finding and connectedness over the `Sparse3D<Cell>` grid, backed by
//! `bevy_northstar`.
//!
//! `bevy_northstar`'s `Nav` (`Passable(cost)` / `Impassable` / `Portal`) is a
//! per-*cell* value, but walls/doors in this game live on cube *faces*
//! (`Slot::XLoWall`/`Slot::ZLoWall`). To give a wall/door's `passable` weight a
//! real per-cell cost (rather than a binary open/blocked filter), each `Room`
//! and each X/Z wall boundary gets its own node in a grid whose coordinates are
//! doubled: a room at local index `l` sits at doubled coordinate `2*l + 1`, and
//! the wall on its low face sits at `2*l` (its own `Slot::XLoWall`/`ZLoWall`,
//! shared with the `l-1` neighbor, matching `RelSlot`'s canonicalization).
//! Floor/ceiling boundary nodes are deliberately left at the grid's impassable
//! default: there is no general vertical connectivity, only the explicit
//! `Nav::Portal` edges added for `stairs` below.
use std::collections::HashMap;

use bevy::math::{IVec3, UVec3};
use bevy::prelude::{Commands, Res, Resource};
use bevy_northstar::prelude::{CardinalGrid3d, GridSettingsBuilder, Nav, PathfindArgs, Portal};

use crate::city::{Cell, ConstructedCity};
use crate::eorf::EorfInfo;
use crate::sparse3d::{Facing, Slot, SlotCoord, Sparse3D};

/// Extra cubes of padding added around the built city's bounding box (and
/// always covering `y == 0`, the assumed infinite ground plane) when sizing
/// the navigation grid.
const GRID_PADDING: i32 = 1;

/// Converts a structure's continuous `passable` weight (0.0-1.0) into a
/// northstar `Nav` value. `passable <= 0.0` is impassable; otherwise the cost
/// is the reciprocal, rounded and clamped to at least 1 (fully passable ->
/// cost 1, half passable -> cost 2, ...).
fn passable_to_nav(passable: f32) -> Nav {
    if passable <= 0.0 {
        Nav::Impassable
    } else {
        Nav::Passable((1.0 / passable).round().max(1.0) as u32)
    }
}

/// The world direction a `Facing` points, for horizontal (X/Z) room objects
/// such as stairs. Anchored so the default (`Facing::NegX`) points `-Z`
/// (matching "-Z direction, if unrotated"), then follows the same
/// `Rotation::Clockwise` cycle already used for `RelSlotCoord::rotate`.
fn facing_horizontal_dir(facing: Facing) -> IVec3 {
    match facing {
        Facing::NegX => IVec3::NEG_Z,
        Facing::NegZ => IVec3::X,
        Facing::PosX => IVec3::Z,
        Facing::PosZ => IVec3::NEG_X,
    }
}

/// The boundary `SlotCoord` shared by two horizontally-adjacent cubes `from`
/// and `to` (must differ by exactly one horizontal cardinal step). Mirrors
/// `global_illumination::boundary_transmission`'s traversal, restricted to
/// the X/Z slots pathing cares about (Y crossings are handled separately, via
/// `stairs` portals only).
fn horizontal_boundary_loc(from: IVec3, to: IVec3) -> SlotCoord {
    let delta = to - from;
    if delta == IVec3::X {
        SlotCoord {
            cube: to,
            slot: Slot::XLoWall,
        }
    } else if delta == IVec3::NEG_X {
        SlotCoord {
            cube: from,
            slot: Slot::XLoWall,
        }
    } else if delta == IVec3::Z {
        SlotCoord {
            cube: to,
            slot: Slot::ZLoWall,
        }
    } else if delta == IVec3::NEG_Z {
        SlotCoord {
            cube: from,
            slot: Slot::ZLoWall,
        }
    } else {
        panic!("horizontal_boundary_loc: {from} and {to} are not horizontal cardinal neighbors");
    }
}

/// `Nav` for the boundary between two horizontally-adjacent cubes: the
/// `passable` weight of whatever structure occupies that wall face, or fully
/// open if unoccupied.
fn boundary_nav(contents: &Sparse3D<Cell>, structures: &[EorfInfo], from: IVec3, to: IVec3) -> Nav {
    match contents.get(horizontal_boundary_loc(from, to)) {
        None => Nav::Passable(1),
        Some(cell) => passable_to_nav(structures[cell.id.as_usize()].embedding.passable),
    }
}

/// `Nav` for a Room cell: the `passable` weight of whatever's occupying the
/// Room slot (or fully open if empty), gated by whether there's anything to
/// stand on -- ground level (`y == 0`, the assumed infinite flat plane) or an
/// explicit `Floor` cell.
fn room_nav(contents: &Sparse3D<Cell>, structures: &[EorfInfo], cube: IVec3) -> Nav {
    let grounded = cube.y == 0
        || contents
            .get(SlotCoord {
                cube,
                slot: Slot::Floor,
            })
            .is_some();
    if !grounded {
        return Nav::Impassable;
    }
    match contents.get(SlotCoord {
        cube,
        slot: Slot::Room,
    }) {
        None => Nav::Passable(1),
        Some(cell) => passable_to_nav(structures[cell.id.as_usize()].embedding.passable),
    }
}

/// Translates between world cube coordinates and the doubled, non-negative
/// grid coordinates `bevy_northstar` requires. See the module docs for the
/// doubling scheme.
struct GridSpace {
    /// World cube coordinate mapped to local index `(0, 0, 0)`.
    origin: IVec3,
    /// Number of cubes covered per axis.
    dims: IVec3,
}

impl GridSpace {
    fn local(&self, cube: IVec3) -> Option<IVec3> {
        let l = cube - self.origin;
        if l.x < 0
            || l.y < 0
            || l.z < 0
            || l.x >= self.dims.x
            || l.y >= self.dims.y
            || l.z >= self.dims.z
        {
            None
        } else {
            Some(l)
        }
    }

    fn room_doubled(&self, cube: IVec3) -> Option<UVec3> {
        self.local(cube).map(|l| (2 * l + IVec3::ONE).as_uvec3())
    }

    /// Inverse of `room_doubled`: recovers the world cube for a Room-node
    /// position (odd on every axis), or `None` for a boundary-node position.
    fn cube_of_room(&self, pos: UVec3) -> Option<IVec3> {
        let p = pos.as_ivec3();
        if p.x % 2 == 1 && p.y % 2 == 1 && p.z % 2 == 1 {
            Some((p - IVec3::ONE) / 2 + self.origin)
        } else {
            None
        }
    }

    fn boundary_doubled(&self, loc: SlotCoord) -> Option<UVec3> {
        let axis_offset = match loc.slot {
            Slot::XLoWall => IVec3::new(1, 0, 0),
            Slot::Floor => IVec3::new(0, 1, 0),
            Slot::ZLoWall => IVec3::new(0, 0, 1),
            Slot::Room => panic!("boundary_doubled: Room is not a boundary slot"),
        };
        self.local(loc.cube)
            .map(|l| (2 * l + IVec3::ONE - axis_offset).as_uvec3())
    }

    fn dims_doubled(&self) -> UVec3 {
        (2 * self.dims).as_uvec3()
    }
}

/// The navigation grid over the current `ConstructedCity`, rebuilt whenever
/// the city changes (see `rebuild_navigation_grid`).
#[derive(Resource)]
pub struct NavigationGrid {
    grid: CardinalGrid3d,
    space: GridSpace,
}

impl NavigationGrid {
    /// Finds a walkable route between two Room cells, as a sequence of world
    /// cube coordinates (Room-to-Room hops; the wall-boundary nodes crossed
    /// along the way are not included). `None` if no route exists or either
    /// endpoint is outside the current grid bounds.
    pub fn find_path(&self, from: IVec3, to: IVec3) -> Option<Vec<IVec3>> {
        let start = self.space.room_doubled(from)?;
        let goal = self.space.room_doubled(to)?;
        let path = self.grid.pathfind(&mut PathfindArgs::new(start, goal))?;
        Some(
            path.path()
                .iter()
                .filter_map(|&pos| self.space.cube_of_room(pos))
                .collect(),
        )
    }

    /// Cheaper reachability check than `find_path`: is there any walkable
    /// route between the two Room cells?
    pub fn is_connected(&self, from: IVec3, to: IVec3) -> bool {
        let (Some(start), Some(goal)) =
            (self.space.room_doubled(from), self.space.room_doubled(to))
        else {
            return false;
        };
        self.grid.is_path_viable(start, goal)
    }
}

/// World-cube bounding box the navigation grid should cover: the built
/// city's bounding box, padded by `GRID_PADDING` (so edge rooms have
/// somewhere open to connect to) and always including `y == 0`.
fn grid_bounds(contents: &Sparse3D<Cell>) -> (IVec3, IVec3) {
    let (min, max) = contents.bounding_box();
    let padding = IVec3::splat(GRID_PADDING);
    let min = IVec3::new(min.x, min.y.min(0), min.z) - padding;
    let max = IVec3::new(max.x, max.y.max(0), max.z) + padding;
    (min, max)
}

fn build_navigation_grid(city: &ConstructedCity) -> NavigationGrid {
    let contents = &city.contents;
    let structures = &city.eorfs;
    let (min, max) = grid_bounds(contents);
    let space = GridSpace {
        origin: min,
        dims: max - min + IVec3::ONE,
    };
    let doubled = space.dims_doubled();

    let settings = GridSettingsBuilder::new_3d(doubled.x, doubled.y, doubled.z)
        .default_impassable()
        .build();
    let mut grid: CardinalGrid3d = CardinalGrid3d::new(&settings);

    // Pass 1: base Room and X/Z-boundary nav for every cube in range. Floor
    // boundary nodes are intentionally left at the grid's impassable default.
    let mut base_room_nav: HashMap<IVec3, Nav> = HashMap::new();
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                let cube = IVec3::new(x, y, z);
                let nav = room_nav(contents, structures, cube);
                base_room_nav.insert(cube, nav);
                if let Some(pos) = space.room_doubled(cube) {
                    grid.set_nav(pos, nav);
                }
                if let Some(pos) = space.boundary_doubled(SlotCoord {
                    cube,
                    slot: Slot::XLoWall,
                }) {
                    grid.set_nav(
                        pos,
                        boundary_nav(contents, structures, cube - IVec3::X, cube),
                    );
                }
                if let Some(pos) = space.boundary_doubled(SlotCoord {
                    cube,
                    slot: Slot::ZLoWall,
                }) {
                    grid.set_nav(
                        pos,
                        boundary_nav(contents, structures, cube - IVec3::Z, cube),
                    );
                }
            }
        }
    }

    // Pass 2: special-case `stairs` cells -- block 3 of their 4 horizontal
    // boundaries and add a vertical `Nav::Portal` connection.
    if let Some(stairs_id) = city.find_structure_by_name("stairs") {
        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    let cube = IVec3::new(x, y, z);
                    let Some(cell) = contents.get(SlotCoord {
                        cube,
                        slot: Slot::Room,
                    }) else {
                        continue;
                    };
                    if cell.id != stairs_id {
                        continue;
                    }
                    let Some(room_pos) = space.room_doubled(cube) else {
                        continue;
                    };

                    let dir = facing_horizontal_dir(cell.facing);
                    for candidate_dir in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z] {
                        if candidate_dir == dir {
                            continue; // Leave the entry side as normally computed in pass 1.
                        }
                        if let Some(pos) = space
                            .boundary_doubled(horizontal_boundary_loc(cube, cube + candidate_dir))
                        {
                            grid.set_nav(pos, Nav::Impassable);
                        }
                    }

                    let landing_cube = cube + dir + IVec3::Y;
                    let (Some(&Nav::Passable(stairs_cost)), Some(&Nav::Passable(landing_cost))) =
                        (base_room_nav.get(&cube), base_room_nav.get(&landing_cube))
                    else {
                        continue; // No floor to land on (or out of grid range): stairs go nowhere.
                    };
                    let Some(landing_pos) = space.room_doubled(landing_cube) else {
                        continue;
                    };

                    grid.set_nav(
                        room_pos,
                        Nav::Portal(Portal::to(landing_pos, stairs_cost, true)),
                    );
                    grid.set_nav(
                        landing_pos,
                        Nav::Portal(Portal::to(room_pos, landing_cost, true)),
                    );
                }
            }
        }
    }

    grid.build();
    NavigationGrid { grid, space }
}

/// Rebuilds the navigation grid whenever the committed city changes, mirroring
/// the `update_global_illumination.run_if(resource_changed::<ConstructedCity>)`
/// pattern used for global illumination.
pub fn rebuild_navigation_grid(mut commands: Commands, city: Res<ConstructedCity>) {
    commands.insert_resource(build_navigation_grid(&city));
}

#[cfg(test)]
mod tests {
    use assert2::check;
    use bevy::math::IVec3;

    use crate::city::{Cell, ConstructedCity};
    use crate::eorf::{find_structure_by_name, load_structure_info};
    use crate::materials::BuildMaterialId;
    use crate::sparse3d::{Facing, Slot, SlotCoord};

    use super::*;

    #[test]
    fn passable_to_nav_thresholds_and_scales() {
        check!(matches!(passable_to_nav(0.0), Nav::Impassable));
        check!(matches!(passable_to_nav(-1.0), Nav::Impassable));
        check!(matches!(passable_to_nav(1.0), Nav::Passable(1)));
        check!(matches!(passable_to_nav(0.5), Nav::Passable(2)));
        check!(matches!(passable_to_nav(0.95), Nav::Passable(1)));
        check!(matches!(passable_to_nav(0.8), Nav::Passable(1)));
    }

    #[test]
    fn facing_horizontal_dir_follows_clockwise_cycle() {
        check!(facing_horizontal_dir(Facing::NegX) == IVec3::NEG_Z);
        check!(facing_horizontal_dir(Facing::NegZ) == IVec3::X);
        check!(facing_horizontal_dir(Facing::PosX) == IVec3::Z);
        check!(facing_horizontal_dir(Facing::PosZ) == IVec3::NEG_X);
    }

    fn test_city() -> ConstructedCity {
        ConstructedCity::new(load_structure_info())
    }

    fn place(city: &mut ConstructedCity, cube: IVec3, slot: Slot, name: &str, facing: Facing) {
        let id = find_structure_by_name(&city.eorfs, name).unwrap();
        city.contents.set(
            SlotCoord { cube, slot },
            Cell {
                id,
                facing,
                evaluation: None,
                build_material: BuildMaterialId::default(),
            },
        );
    }

    #[test]
    fn open_gap_between_ground_rooms_is_connected() {
        let city = test_city();
        let nav = build_navigation_grid(&city);
        // Two ground-level rooms with no wall between them: open passage.
        check!(nav.is_connected(IVec3::new(0, 0, 0), IVec3::new(1, 0, 0)));
    }

    /// Encloses `(0, 0, 0)` with real walls on its west, south and north
    /// faces, so its only possible exit is the east face at `(1, 0, 0)`'s
    /// `XLoWall` -- making `is_connected` results attributable solely to
    /// that one boundary, regardless of how much open padding surrounds it.
    fn enclose_except_east(city: &mut ConstructedCity) {
        place(
            city,
            IVec3::new(0, 0, 0),
            Slot::XLoWall,
            "wall",
            Facing::NegX,
        );
        place(
            city,
            IVec3::new(0, 0, 0),
            Slot::ZLoWall,
            "wall",
            Facing::NegX,
        );
        place(
            city,
            IVec3::new(0, 0, 1),
            Slot::ZLoWall,
            "wall",
            Facing::NegX,
        );
    }

    #[test]
    fn wall_blocks_but_doorway_connects() {
        let mut city = test_city();
        enclose_except_east(&mut city);
        place(
            &mut city,
            IVec3::new(1, 0, 0),
            Slot::XLoWall,
            "wall",
            Facing::NegX,
        );
        let nav = build_navigation_grid(&city);
        check!(!nav.is_connected(IVec3::new(0, 0, 0), IVec3::new(1, 0, 0)));

        let mut city = test_city();
        enclose_except_east(&mut city);
        place(
            &mut city,
            IVec3::new(1, 0, 0),
            Slot::XLoWall,
            "doorway",
            Facing::NegX,
        );
        let nav = build_navigation_grid(&city);
        check!(nav.is_connected(IVec3::new(0, 0, 0), IVec3::new(1, 0, 0)));
    }

    #[test]
    fn stairs_connect_two_levels_in_facing_direction() {
        let mut city = test_city();
        // Floors for the cube above the ground-level stairs and its landing.
        place(
            &mut city,
            IVec3::new(0, 1, 0),
            Slot::Floor,
            "floor",
            Facing::NegX,
        );
        place(
            &mut city,
            IVec3::new(0, 1, -1),
            Slot::Floor,
            "floor",
            Facing::NegX,
        );
        // Stairs at (0,0,0), unrotated: connects to (0,0,-1) and up to (0,1,-1).
        place(
            &mut city,
            IVec3::new(0, 0, 0),
            Slot::Room,
            "stairs",
            Facing::NegX,
        );

        let nav = build_navigation_grid(&city);
        check!(nav.is_connected(IVec3::new(0, 0, 0), IVec3::new(0, 1, -1)));
    }

    #[test]
    fn stairs_block_all_horizontal_boundaries_but_the_facing_one() {
        let mut city = test_city();
        let cube = IVec3::new(0, 0, 0);
        place(&mut city, cube, Slot::Room, "stairs", Facing::NegX);
        let nav = build_navigation_grid(&city);

        // Facing::NegX -> entry is -Z: the other 3 horizontal boundaries are
        // forced impassable regardless of the (unoccupied) wall data there.
        for loc in [
            SlotCoord {
                cube,
                slot: Slot::XLoWall,
            },
            SlotCoord {
                cube: cube + IVec3::X,
                slot: Slot::XLoWall,
            },
            SlotCoord {
                cube: cube + IVec3::Z,
                slot: Slot::ZLoWall,
            },
        ] {
            let pos = nav.space.boundary_doubled(loc).unwrap();
            check!(matches!(nav.grid.nav(pos), Some(Nav::Impassable)));
        }

        // The facing (-Z) boundary is left as normally computed: open.
        let entry_pos = nav
            .space
            .boundary_doubled(SlotCoord {
                cube,
                slot: Slot::ZLoWall,
            })
            .unwrap();
        check!(matches!(nav.grid.nav(entry_pos), Some(Nav::Passable(1))));
    }

    #[test]
    fn upper_room_without_floor_is_unreachable() {
        let city = test_city();
        let nav = build_navigation_grid(&city);
        check!(!nav.is_connected(IVec3::new(0, 0, 0), IVec3::new(0, 1, 0)));
    }
}
