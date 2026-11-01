//! Chimneys: the column of walls rising from the wall a hearth is built into.
//!
//! A stopgap until the autotile system can express "every consecutive wall above X" (its
//! patterns are fixed-size, so they can't follow an unbounded stack). `chimney_of` is plain grid
//! logic shared by the visuals (`chimney_display_system`) and by `hearth_problem`, which keeps a
//! hearth without a working chimney out of place formation (see `place::furniture_problem`).

use bevy::prelude::*;

use crate::city::{structure_transform, Cell, ConstructedCity, GridCellMarker};
use crate::eorf::{EorfInfo, EorfList};
use crate::flood_fill::has_sky_above;
use crate::sparse3d::{Facing, Slot, SlotCoord, Sparse3D};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chimney {
    /// The hearth's own (room-slot) location.
    pub hearth: SlotCoord,
    pub facing: Facing,
    /// The wall the hearth is built into.
    pub back_wall: SlotCoord,
    /// The unbroken run of walls directly above `back_wall`, bottom to top.
    pub flue: Vec<SlotCoord>,
    /// The first wall slot above the flue that isn't a wall: where the chimney comes out.
    pub top: SlotCoord,
}

/// The wall slot a hearth facing `facing` in `cube` is built into. This has to agree with the
/// `1=hearth:90` / `1@` rule for `wall_around_hearth` in `structures.autotile`: that rule puts
/// the wall on the hearth's +X side when it faces PosZ, i.e. one step "behind" its facing.
pub fn hearth_back_wall(cube: IVec3, facing: Facing) -> SlotCoord {
    let behind = match Facing::from_number(facing as u8 + 3) {
        Facing::NegX => IVec3::NEG_X,
        Facing::NegZ => IVec3::NEG_Z,
        Facing::PosX => IVec3::X,
        Facing::PosZ => IVec3::Z,
    };
    SlotCoord::boundary(cube, cube + behind)
}

fn name_at<'a>(
    contents: &Sparse3D<Cell>,
    eorfs: &'a [EorfInfo],
    loc: SlotCoord,
) -> Option<&'a str> {
    contents
        .get(loc)
        .map(|c| eorfs[c.id.as_usize()].name.as_str())
}

fn is_hearth(contents: &Sparse3D<Cell>, eorfs: &[EorfInfo], loc: SlotCoord) -> bool {
    loc.slot == Slot::Room && name_at(contents, eorfs, loc) == Some("hearth")
}

/// The chimney of the hearth at `hearth`, or `None` if there's no hearth there or it isn't built
/// into a wall (in which case it has no chimney at all). Only cells named `"wall"` count, both
/// behind the hearth and in the flue; a window, doorway, or anything else stops the flue there.
pub fn chimney_of(
    contents: &Sparse3D<Cell>,
    eorfs: &[EorfInfo],
    hearth: SlotCoord,
) -> Option<Chimney> {
    if !is_hearth(contents, eorfs, hearth) {
        return None;
    }
    let facing = contents.get(hearth)?.facing;
    let back_wall = hearth_back_wall(hearth.cube, facing);
    if name_at(contents, eorfs, back_wall) != Some("wall") {
        return None;
    }
    let above = |loc: SlotCoord| SlotCoord {
        cube: loc.cube + IVec3::Y,
        ..loc
    };
    let mut flue = Vec::new();
    let mut top = above(back_wall);
    while name_at(contents, eorfs, top) == Some("wall") {
        flue.push(top);
        top = above(top);
    }
    Some(Chimney {
        hearth,
        facing,
        back_wall,
        flue,
        top,
    })
}

/// Every hearth's chimney in `contents`, in a stable order.
pub fn find_chimneys(contents: &Sparse3D<Cell>, eorfs: &[EorfInfo]) -> Vec<Chimney> {
    let mut chimneys: Vec<Chimney> = contents
        .iter()
        .filter_map(|(loc, _)| chimney_of(contents, eorfs, loc))
        .collect();
    // `Sparse3D::iter` goes chunk by chunk in hash order; keep the result stable so callers
    // can compare it from frame to frame.
    chimneys.sort_by_key(|c| c.hearth.cube.to_array());
    chimneys
}

/// Why the hearth at `loc` doesn't work, or `None` if it does (or isn't a hearth). A hearth works
/// when it's built into a wall and its chimney comes out into open air: nothing occupies the slot
/// it comes out of, and no floor or roof is above it on either side. (A floor level with the
/// chimney top doesn't count; the chimney rises past it.)
pub fn hearth_problem(
    contents: &Sparse3D<Cell>,
    eorfs: &[EorfInfo],
    loc: SlotCoord,
) -> Option<String> {
    if !is_hearth(contents, eorfs, loc) {
        return None;
    }
    let Some(chimney) = chimney_of(contents, eorfs, loc) else {
        return Some("A hearth must be embedded in a wall".to_string());
    };
    if let Some(blocker) = name_at(contents, eorfs, chimney.top) {
        return Some(format!("Chimney blocked by {blocker}."));
    }
    let (_, max) = contents.bounding_box();
    // A wall slot lies between its own cube and the one on its low side.
    let low_side = match chimney.top.slot {
        Slot::XLoWall => IVec3::NEG_X,
        _ => IVec3::NEG_Z,
    };
    let cube = chimney.top.cube;
    if !(has_sky_above(contents, cube, max.y) && has_sky_above(contents, cube + low_side, max.y)) {
        return Some("Chimney must reach open air.".to_string());
    }
    None
}

// ─── Display ──────────────────────────────────────────────────────────────────

/// The chimney meshes, loaded straight from the per-`.wings` copies `build.rs` always writes
/// to `generated/autotile/atomic/`, since no autotile rule refers to them.
#[derive(Resource)]
pub struct ChimneyAssets {
    flue: Handle<Scene>,
    top: Handle<Scene>,
}

pub fn load_chimney_assets(asset_server: Res<AssetServer>, mut commands: Commands) {
    let load = |name: &str| {
        asset_server.load(format!(
            "assets/generated/autotile/atomic/{name}.gltf#Scene0"
        ))
    };
    commands.insert_resource(ChimneyAssets {
        flue: load("chimney"),
        top: load("chimney_top"),
    });
}

/// What `chimney_display_system` last drew, and the entities it drew it with.
#[derive(Default)]
pub struct DrawnChimneys {
    chimneys: Vec<Chimney>,
    entities: Vec<Entity>,
}

/// Draws a `chimney` on every wall of each hearth's flue and a `chimney_top` above it. Real
/// cells only (proposed hearths and walls don't get chimney ghosts).
///
/// The chimney meshes are authored in the hearth mesh's frame, so each piece reuses the hearth's
/// own transform, lifted a whole number of floors. Each is tagged with the `GridCellMarker` of a
/// wall in the stack (the top with the highest one), so it takes that wall's build material and
/// is hidden along with it by the cutaway.
pub fn chimney_display_system(
    mut commands: Commands,
    constructed: Res<ConstructedCity>,
    structure_list: Res<EorfList>,
    assets: Res<ChimneyAssets>,
    mut drawn: Local<DrawnChimneys>,
) {
    let chimneys = find_chimneys(&constructed.contents, &constructed.eorfs);
    if chimneys == drawn.chimneys {
        return;
    }
    for e in drawn.entities.drain(..) {
        commands.entity(e).despawn();
    }
    for chimney in &chimneys {
        let Some(hearth) = constructed.contents.get(chimney.hearth) else {
            continue;
        };
        let base = structure_transform(
            &structure_list,
            hearth.id,
            Slot::Room,
            chimney.facing,
            chimney.hearth.cube,
        );
        let mut spawn = |handle: &Handle<Scene>, floors: usize, loc: SlotCoord| {
            let mut transform = base;
            transform.translation.y += floors as f32;
            commands
                .spawn((SceneRoot(handle.clone()), transform, GridCellMarker { loc }))
                .id()
        };
        for (i, &wall) in chimney.flue.iter().enumerate() {
            let e = spawn(&assets.flue, i + 1, wall);
            drawn.entities.push(e);
        }
        let top_owner = chimney.flue.last().copied().unwrap_or(chimney.back_wall);
        let e = spawn(&assets.top, chimney.flue.len() + 1, top_owner);
        drawn.entities.push(e);
    }
    drawn.chimneys = chimneys;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eorf::{find_structure_by_name, load_structure_info};
    use assert2::check;

    fn cell(structures: &[EorfInfo], name: &str, facing: Facing) -> Cell {
        Cell {
            id: find_structure_by_name(structures, name).unwrap(),
            facing,
            evaluation: None,
            build_material: Default::default(),
        }
    }

    #[test]
    fn back_wall_matches_the_wall_around_hearth_rule() {
        // Facing PosZ, the rule puts the wall on the hearth's +X side.
        let cube = IVec3::new(2, 0, 3);
        check!(hearth_back_wall(cube, Facing::PosZ) == SlotCoord::boundary(cube, cube + IVec3::X));
        // And it turns with the hearth.
        check!(hearth_back_wall(cube, Facing::PosX) == SlotCoord::boundary(cube, cube - IVec3::Z));
        check!(hearth_back_wall(cube, Facing::NegZ) == SlotCoord::boundary(cube, cube - IVec3::X));
        check!(hearth_back_wall(cube, Facing::NegX) == SlotCoord::boundary(cube, cube + IVec3::Z));
    }

    #[test]
    fn flue_follows_consecutive_walls_up_from_the_back_wall() {
        let structures = load_structure_info();
        let mut contents = Sparse3D::new();
        let hearth = SlotCoord {
            cube: IVec3::new(0, 0, 0),
            slot: Slot::Room,
        };
        contents.set(hearth, cell(&structures, "hearth", Facing::PosZ));
        let back = hearth_back_wall(hearth.cube, Facing::PosZ);
        let up = |n: i32| SlotCoord {
            cube: back.cube + IVec3::Y * n,
            ..back
        };
        for n in 0..3 {
            contents.set(up(n), cell(&structures, "wall", Facing::default()));
        }
        // A gap, then more wall: not part of this chimney.
        contents.set(up(4), cell(&structures, "wall", Facing::default()));

        let chimneys = find_chimneys(&contents, &structures);
        check!(chimneys.len() == 1);
        check!(chimneys[0].back_wall == back);
        check!(chimneys[0].flue == vec![up(1), up(2)]);
        check!(chimneys[0].top == up(3));
    }

    /// A hearth facing PosZ at the origin, with a wall behind it, and that wall's slot `n`
    /// floors up.
    fn walled_hearth(
        structures: &[EorfInfo],
    ) -> (Sparse3D<Cell>, SlotCoord, impl Fn(i32) -> SlotCoord) {
        let mut contents = Sparse3D::new();
        let hearth = SlotCoord {
            cube: IVec3::new(0, 0, 0),
            slot: Slot::Room,
        };
        contents.set(hearth, cell(structures, "hearth", Facing::PosZ));
        let back = hearth_back_wall(hearth.cube, Facing::PosZ);
        contents.set(back, cell(structures, "wall", Facing::default()));
        let up = move |n: i32| SlotCoord {
            cube: back.cube + IVec3::Y * n,
            ..back
        };
        (contents, hearth, up)
    }

    #[test]
    fn a_hearth_without_a_wall_behind_it_has_no_chimney_and_does_not_work() {
        let structures = load_structure_info();
        let (mut contents, hearth, up) = walled_hearth(&structures);
        contents.take(up(0));
        // Walls above don't help.
        contents.set(up(1), cell(&structures, "wall", Facing::default()));
        check!(chimney_of(&contents, &structures, hearth).is_none());
        check!(find_chimneys(&contents, &structures).is_empty());
        check!(hearth_problem(&contents, &structures, hearth).is_some());
    }

    #[test]
    fn a_window_stops_the_flue_and_blocks_the_chimney() {
        let structures = load_structure_info();
        let (mut contents, hearth, up) = walled_hearth(&structures);
        contents.set(up(1), cell(&structures, "window", Facing::default()));
        contents.set(up(2), cell(&structures, "wall", Facing::default()));

        let chimney = chimney_of(&contents, &structures, hearth).unwrap();
        check!(chimney.flue.is_empty());
        check!(chimney.top == up(1));
        check!(hearth_problem(&contents, &structures, hearth).is_some());
    }

    #[test]
    fn a_chimney_under_a_roof_does_not_work() {
        let structures = load_structure_info();
        let (mut contents, hearth, _) = walled_hearth(&structures);
        check!(hearth_problem(&contents, &structures, hearth) == None);

        // A floor level with the chimney top doesn't block it...
        let floor_at = |cube: IVec3| SlotCoord {
            cube,
            slot: Slot::Floor,
        };
        contents.set(
            floor_at(IVec3::new(0, 1, 0)),
            cell(&structures, "floor", Facing::default()),
        );
        check!(hearth_problem(&contents, &structures, hearth) == None);
        // ...but one above it, on either side of the wall, does.
        for side in [IVec3::new(0, 2, 0), IVec3::new(1, 2, 0)] {
            let mut covered = contents.clone();
            covered.set(
                floor_at(side),
                cell(&structures, "floor", Facing::default()),
            );
            check!(
                hearth_problem(&covered, &structures, hearth).is_some(),
                "covered at {side}"
            );
        }
    }
}
