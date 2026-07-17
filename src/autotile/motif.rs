use std::collections::HashMap;

use bevy::math::IVec3;

use crate::sparse3d::{Slot, SlotCoord};

use super::parser::{Motif, MotifAxis, MotifId};

/// A single matched occurrence of a `Motif` at a specific world location, as produced by
/// matching a compiled `Motif` rule (e.g. via `evaluate_autotile_rules`) against the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct MotifAtom {
    pub motif: Motif,
    pub loc: SlotCoord,
}

/// A run of adjacent `MotifAtom`s sharing one `MotifId`, collapsed along that motif's axis.
/// `base` is the end of the run with the smaller axis coordinate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotifOccurrence {
    pub id: MotifId,
    pub base: SlotCoord,
    pub axis: MotifAxis,
    pub length: u32,
    pub nonmundanity: f64,
}

/// A single matched `defect` atom. Defects never collapse into `MotifOccurrence`s (each defect
/// is its own rule case, so two defects are never the same `MotifId`); their location is kept so
/// they can later be tallied by line-of-sight rather than by run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefectAtom {
    pub id: MotifId,
    pub loc: SlotCoord,
}

/// Collapses matched `MotifAtom`s into `MotifOccurrence`s (runs of the same `MotifId`, adjacent
/// along that motif's axis) and separately collects `DefectAtom`s. `Motif::Discard` atoms
/// produce neither and are simply dropped.
///
/// Two nonmundane atoms merge when they share a `MotifId` and `Slot`, their coordinates
/// transverse to the axis match exactly, and their axis coordinates differ by exactly 1. A
/// nonmundane atom with axis `None` never merges with anything (always a length-1 occurrence).
pub fn collapse_motif_atoms(atoms: &[MotifAtom]) -> (Vec<MotifOccurrence>, Vec<DefectAtom>) {
    let mut defects = Vec::new();
    let mut occurrences = Vec::new();

    // (id, slot, axis, coordinates transverse to the axis) → axis coordinates seen. `axis` is
    // part of the key (not looked up separately per id) because a single `MotifId` can carry
    // different axes on different atoms -- e.g. `h_corner` fires with axis `Z` alongside one wall
    // of a room corner and axis `X` alongside the other, both sharing the same id. Keying only by
    // id (as a prior version of this function did) let a same-id atom with a *different* axis
    // overwrite which axis a given transverse-coordinate group gets reconstructed with,
    // scrambling that group's cube coordinates.
    let mut groups: HashMap<(MotifId, Slot, MotifAxis, i32, i32), Vec<i32>> = HashMap::new();
    let mut nonmundanity_by_id: HashMap<MotifId, f64> = HashMap::new();

    for atom in atoms {
        match &atom.motif {
            Motif::Discard => {}
            Motif::Defect { id } => defects.push(DefectAtom {
                id: *id,
                loc: atom.loc,
            }),
            Motif::Nonmundane {
                axis,
                nonmundanity,
                id,
                ..
            } => {
                nonmundanity_by_id.insert(*id, *nonmundanity);
                let cube = atom.loc.cube;
                match axis {
                    MotifAxis::None => occurrences.push(MotifOccurrence {
                        id: *id,
                        base: atom.loc,
                        axis: *axis,
                        length: 1,
                        nonmundanity: *nonmundanity,
                    }),
                    MotifAxis::X => groups
                        .entry((*id, atom.loc.slot, *axis, cube.y, cube.z))
                        .or_default()
                        .push(cube.x),
                    MotifAxis::Y => groups
                        .entry((*id, atom.loc.slot, *axis, cube.x, cube.z))
                        .or_default()
                        .push(cube.y),
                    MotifAxis::Z => groups
                        .entry((*id, atom.loc.slot, *axis, cube.x, cube.y))
                        .or_default()
                        .push(cube.z),
                }
            }
        }
    }

    for ((id, slot, axis, t0, t1), mut coords) in groups {
        coords.sort_unstable();
        coords.dedup();
        let nonmundanity = nonmundanity_by_id[&id];

        let mut run_start = 0;
        for i in 1..=coords.len() {
            if i == coords.len() || coords[i] != coords[i - 1] + 1 {
                let base_axis_coord = coords[run_start];
                let length = (i - run_start) as u32;
                let cube = match axis {
                    MotifAxis::X => IVec3::new(base_axis_coord, t0, t1),
                    MotifAxis::Y => IVec3::new(t0, base_axis_coord, t1),
                    MotifAxis::Z => IVec3::new(t0, t1, base_axis_coord),
                    MotifAxis::None => unreachable!("None-axis atoms never enter `groups`"),
                };
                occurrences.push(MotifOccurrence {
                    id,
                    base: SlotCoord { cube, slot },
                    axis,
                    length,
                    nonmundanity,
                });
                run_start = i;
            }
        }
    }

    (occurrences, defects)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sparse3d::Slot;
    use assert2::check;

    #[test]
    fn motifs_autotile_file_parses() {
        let src = include_str!("../../buildables/motifs.autotile");
        let file =
            super::super::parse::<Motif>(src).expect("motifs.autotile should parse as Motif rules");
        check!(file.rules.len() > 0);
    }

    /// Regression test for a real rule from `motifs.autotile` (`empty:room`'s `v_corner` case)
    /// whose `dispatch_anchor` lands on `ZLoWall` (an H tile's "other" wall slot) while `@` sits
    /// on the canonical `XLoWall` slot. `compile_rule` used to assume every wall-family anchor
    /// started `XLoWall`-canonical, which crashed `RelSlotCoord::apply_offset` with a "slot type
    /// mismatch" panic as soon as a real Z-wall structure was matched against this rule.
    #[test]
    fn empty_room_v_corner_rule_does_not_panic_on_z_wall_anchor() {
        use super::super::{
            build_empty_anchor_index, char_matches_name, compile_rule, evaluate_empty_anchor_rules,
        };
        use crate::city::Cell;
        use crate::eorf::EorfId;
        use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, RelSlotCoordOffset, Sparse3D};
        use bevy::math::IVec3;

        let input = "\
==empty:room==
H:
  .@
 w .
  w
--> . 'v_corner' 0.0
";
        let file = super::super::parse::<Motif>(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        check!(pat.anchor_origin_slot() == super::super::AutotileRelSlot::ZLoWall);

        let oriented = compile_rule(&file.rules[0]);
        let names = vec!["wall".to_string()];
        fn char_matches(ch: char, id: EorfId, _facing: Facing) -> bool {
            char_matches_name(ch, ["wall"][id.0 as usize])
        }
        fn no_name_match(_name: &str, _id: EorfId) -> bool {
            false
        }
        let index = build_empty_anchor_index(std::slice::from_ref(&oriented), &names, char_matches);

        fn wall_cell() -> Cell {
            Cell {
                id: EorfId(0),
                facing: Facing::default(),
                evaluation: None,
                build_material: Default::default(),
            }
        }

        // The full "v_corner" pattern needs a second wall (the room's other side) in addition to
        // the dispatch anchor itself, at offset (1,0,0) from the anchor (ZLoWall -> XLoWall).
        let second_wall_offset = RelSlotCoordOffset {
            origin_slot: RelSlot::ZLoWall,
            cube_offset: IVec3::new(1, 0, 0),
            dest_slot: RelSlot::XLoWall,
        };

        // A wall found in a Z-oriented slot must dispatch via `cases_plus_90` without panicking
        // (this used to crash inside `apply_offset` before the fix).
        let anchor_z = RelSlotCoord::new(0, 0, 0, RelSlot::ZLoWall);
        let mut grid_z: Sparse3D<Cell> = Sparse3D::new();
        grid_z.set(anchor_z, wall_cell());
        grid_z.set(anchor_z.apply_offset(second_wall_offset), wall_cell());
        let results_z = evaluate_empty_anchor_rules(
            anchor_z,
            "wall",
            &index,
            |loc| grid_z.get(loc).map(|c| (c.id, c.facing)),
            char_matches,
            no_name_match,
        );
        check!(
            !results_z.is_empty(),
            "expected the Z-wall anchor to match; got none"
        );
        // (The X-wall-anchor orientation of this same `cases`/`cases_plus_90` split is already
        // exercised generically by `matcher::tests::empty_anchor_landing_on_the_other_wall_slot_does_not_panic`.)
    }

    fn atom(id: usize, axis: MotifAxis, nonmundanity: f64, cube: (i32, i32, i32)) -> MotifAtom {
        MotifAtom {
            motif: Motif::Nonmundane {
                axis,
                nonmundanity,
                name: format!("motif_{id}"),
                id: MotifId(id),
            },
            loc: SlotCoord {
                cube: IVec3::new(cube.0, cube.1, cube.2),
                slot: Slot::XLoWall,
            },
        }
    }

    fn defect(id: usize, cube: (i32, i32, i32)) -> MotifAtom {
        MotifAtom {
            motif: Motif::Defect { id: MotifId(id) },
            loc: SlotCoord {
                cube: IVec3::new(cube.0, cube.1, cube.2),
                slot: Slot::XLoWall,
            },
        }
    }

    #[test]
    fn adjacent_atoms_collapse_into_one_occurrence() {
        let atoms = vec![
            atom(1, MotifAxis::X, 0.5, (0, 0, 0)),
            atom(1, MotifAxis::X, 0.5, (1, 0, 0)),
            atom(1, MotifAxis::X, 0.5, (2, 0, 0)),
        ];
        let (occurrences, defects) = collapse_motif_atoms(&atoms);
        check!(defects.is_empty());
        check!(occurrences.len() == 1);
        let occ = occurrences[0];
        check!(occ.id == MotifId(1));
        check!(occ.length == 3);
        check!(occ.nonmundanity == 0.5);
        check!(occ.base.cube == IVec3::new(0, 0, 0));
    }

    #[test]
    fn non_adjacent_atoms_stay_separate() {
        let atoms = vec![
            atom(1, MotifAxis::X, 0.5, (0, 0, 0)),
            atom(1, MotifAxis::X, 0.5, (2, 0, 0)),
        ];
        let (occurrences, _) = collapse_motif_atoms(&atoms);
        check!(occurrences.len() == 2);
        check!(occurrences.iter().all(|o| o.length == 1));
    }

    #[test]
    fn different_ids_never_merge() {
        let atoms = vec![
            atom(1, MotifAxis::X, 0.5, (0, 0, 0)),
            atom(2, MotifAxis::X, 0.9, (1, 0, 0)),
        ];
        let (occurrences, _) = collapse_motif_atoms(&atoms);
        check!(occurrences.len() == 2);
    }

    #[test]
    fn different_slots_never_merge() {
        let mut a = atom(1, MotifAxis::X, 0.5, (0, 0, 0));
        let mut b = atom(1, MotifAxis::X, 0.5, (1, 0, 0));
        a.loc.slot = Slot::Room;
        b.loc.slot = Slot::Floor;
        let (occurrences, _) = collapse_motif_atoms(&[a, b]);
        check!(occurrences.len() == 2);
    }

    #[test]
    fn axis_none_never_merges_even_when_adjacent() {
        let atoms = vec![
            atom(1, MotifAxis::None, 0.5, (0, 0, 0)),
            atom(1, MotifAxis::None, 0.5, (1, 0, 0)),
        ];
        let (occurrences, _) = collapse_motif_atoms(&atoms);
        check!(occurrences.len() == 2);
        check!(occurrences.iter().all(|o| o.length == 1));
    }

    #[test]
    fn adjacency_requires_matching_transverse_coordinates() {
        // Both at x=0 and x=1, but on different Z rows: shouldn't merge.
        let atoms = vec![
            atom(1, MotifAxis::X, 0.5, (0, 0, 0)),
            atom(1, MotifAxis::X, 0.5, (1, 0, 1)),
        ];
        let (occurrences, _) = collapse_motif_atoms(&atoms);
        check!(occurrences.len() == 2);
    }

    #[test]
    fn y_axis_run_collapses() {
        let atoms = vec![
            atom(1, MotifAxis::Y, 0.5, (0, 0, 0)),
            atom(1, MotifAxis::Y, 0.5, (0, 1, 0)),
        ];
        let (occurrences, _) = collapse_motif_atoms(&atoms);
        check!(occurrences.len() == 1);
        check!(occurrences[0].length == 2);
        check!(occurrences[0].base.cube == IVec3::new(0, 0, 0));
    }

    #[test]
    fn defects_are_kept_separately_with_location() {
        let atoms = vec![atom(1, MotifAxis::X, 0.5, (0, 0, 0)), defect(2, (5, 0, 0))];
        let (occurrences, defects) = collapse_motif_atoms(&atoms);
        check!(occurrences.len() == 1);
        check!(defects.len() == 1);
        check!(defects[0].id == MotifId(2));
        check!(defects[0].loc.cube == IVec3::new(5, 0, 0));
    }

    // ── End-to-end: parse → compile → match → collapse ───────────────────────

    #[test]
    fn parse_compile_match_and_collapse_motif_rule() {
        use super::super::{compile, parse};
        use crate::city::Cell;
        use crate::eorf::EorfId;
        use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, Sparse3D};

        // A wall motif that fires whenever there's a wall to the +X side, scoring 0.75 along
        // the pattern's column (X) axis. The middle wall of a run legitimately satisfies both the
        // "+X neighbor" and "-X neighbor" (mirrored) readings of this symmetric pattern at once;
        // `Motif` results are implicitly `(multi)` (see `AutotileResultKind::implicitly_multi`),
        // so both fire, producing two identical atoms at that location that `match_pattern_cases`
        // dedupes back down to one before `collapse_motif_atoms` ever sees them.
        let input = "\
== wall: wall ==
H:
 @ W
--> - 'wall_row' 0.75
--> defect
";
        let file = parse::<Motif>(input).unwrap();
        let oriented = compile(&file);

        fn char_matches(ch: char, id: EorfId, _facing: Facing) -> bool {
            ch == 'W' && id.0 == 0
        }
        fn no_name_match(_name: &str, _id: EorfId) -> bool {
            false
        }

        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        let a0 = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);
        let a1 = RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall);
        let a2 = RelSlotCoord::new(2, 0, 0, RelSlot::XLoWall);
        for loc in [a0, a1, a2] {
            grid.set(
                loc,
                Cell {
                    id: EorfId(0),
                    facing: Default::default(),
                    evaluation: None,
                    build_material: Default::default(),
                },
            );
        }

        let mut atoms = Vec::new();
        for loc in [a0, a1] {
            let results = super::super::evaluate_autotile_rules(
                loc,
                "wall",
                &oriented,
                |l| grid.get(l).map(|c| (c.id, c.facing)),
                char_matches,
                no_name_match,
            )
            .expect("wall has motif rules");
            for motif in results {
                atoms.push(MotifAtom {
                    motif,
                    loc: SlotCoord::from(loc),
                });
            }
        }

        let (occurrences, defects) = collapse_motif_atoms(&atoms);
        check!(defects.is_empty());
        check!(occurrences.len() == 1);
        check!(occurrences[0].length == 2);
        check!(occurrences[0].nonmundanity == 0.75);
    }

    /// `Motif` results are implicitly `(multi)` (`AutotileResultKind::implicitly_multi`), so a
    /// symmetric pattern matching two orientations at once never panics -- unlike the equivalent
    /// `AutotiledMeshes` case (see `matcher::tests::non_multi_ambiguous_match_panics`). Whether
    /// both orientations survive depends on whether they produce *equal* results: identical
    /// results (same case, e.g. `discard`/`defect`, or a `'name'`+axis that happens to coincide)
    /// collapse to one; results that genuinely differ (e.g. different axes, as with the real
    /// `h_corner` rule at a room corner) both survive.
    #[test]
    fn implicitly_multi_dedupes_identical_results_but_keeps_distinct_ones() {
        use super::super::{compile_rule, match_pattern};
        use crate::city::Cell;
        use crate::eorf::EorfId;
        use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, Sparse3D};

        fn char_matches(ch: char, id: EorfId, _facing: Facing) -> bool {
            ch == 'W' && id.0 == 0
        }
        fn no_name_match(_name: &str, _id: EorfId) -> bool {
            false
        }

        // Two neighbors (both walls), so both the "+X neighbor" and "-X neighbor" (mirrored)
        // orientations of this symmetric pattern match at once -- but they both emit the exact
        // same `discard` result, so only one atom should survive.
        let discard_input = "\
== wall: wall ==
H:
 @ W
--> discard
";
        let file = super::super::parse::<Motif>(discard_input).unwrap();
        let oriented = compile_rule(&file.rules[0]);

        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);
        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        for loc in [
            RelSlotCoord::new(-1, 0, 0, RelSlot::XLoWall),
            RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall),
        ] {
            grid.set(
                loc,
                Cell {
                    id: EorfId(0),
                    facing: Default::default(),
                    evaluation: None,
                    build_material: Default::default(),
                },
            );
        }
        let results = match_pattern(
            &oriented,
            |l| grid.get(l).map(|c| (c.id, c.facing)),
            anchor,
            char_matches,
            no_name_match,
        );
        check!(
            results == vec![&Motif::Discard],
            "two orientations emitting the same `discard` should collapse to one; got {results:?}"
        );

        // Now a floor-anchored corner-shaped rule (shaped like the real `h_corner` motif): unlike
        // the wall-anchored case above, a Floor anchor's four compiled rotations all share one
        // case list (never split into `cases_plus_90`), so two of them can match at once with
        // genuinely different axes -- a wall on the +X side (axis Z) and a wall on the +Z side
        // (axis X) of the same floor cell, i.e. an actual room corner. Both are real, distinct
        // motifs and must both survive (mirrors the real `h_corner` rule's fix in
        // `buildables/motifs.autotile`; see `matcher::tests::motif_count_is_rotation_invariant`).
        let corner_input = "\
== floor: floor ==
V:
   w
  @
--> . 'h_corner_like' 0.0
";
        let corner_file = super::super::parse::<Motif>(corner_input).unwrap();
        let corner_oriented = compile_rule(&corner_file.rules[0]);

        let floor_anchor = RelSlotCoord::new(0, 0, 0, RelSlot::Floor);
        let mut corner_grid: Sparse3D<Cell> = Sparse3D::new();
        for loc in [
            RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall),
            RelSlotCoord::new(0, 0, 1, RelSlot::ZLoWall),
        ] {
            corner_grid.set(
                loc,
                Cell {
                    id: EorfId(0),
                    facing: Default::default(),
                    evaluation: None,
                    build_material: Default::default(),
                },
            );
        }
        fn wall_matches(ch: char, id: EorfId, _facing: Facing) -> bool {
            ch == 'w' && id.0 == 0
        }
        let corner_results = match_pattern(
            &corner_oriented,
            |l| corner_grid.get(l).map(|c| (c.id, c.facing)),
            floor_anchor,
            wall_matches,
            no_name_match,
        );
        check!(
            corner_results.len() == 2,
            "both distinct-axis orientations at a real corner should survive; got {corner_results:?}"
        );
        check!(
            corner_results.iter().any(|r| matches!(
                r,
                Motif::Nonmundane {
                    axis: MotifAxis::Z,
                    ..
                }
            )),
            "expected a Z-axis result among {corner_results:?}"
        );
        check!(
            corner_results.iter().any(|r| matches!(
                r,
                Motif::Nonmundane {
                    axis: MotifAxis::X,
                    ..
                }
            )),
            "expected an X-axis result among {corner_results:?}"
        );
    }
}
