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

    // (id, slot, coordinates transverse to the axis) → axis coordinates seen.
    let mut groups: HashMap<(MotifId, Slot, i32, i32), Vec<i32>> = HashMap::new();
    let mut nonmundanity_by_id: HashMap<MotifId, f64> = HashMap::new();
    let mut axis_by_id: HashMap<MotifId, MotifAxis> = HashMap::new();

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
            } => {
                nonmundanity_by_id.insert(*id, *nonmundanity);
                axis_by_id.insert(*id, *axis);
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
                        .entry((*id, atom.loc.slot, cube.y, cube.z))
                        .or_default()
                        .push(cube.x),
                    MotifAxis::Y => groups
                        .entry((*id, atom.loc.slot, cube.x, cube.z))
                        .or_default()
                        .push(cube.y),
                    MotifAxis::Z => groups
                        .entry((*id, atom.loc.slot, cube.x, cube.y))
                        .or_default()
                        .push(cube.z),
                }
            }
        }
    }

    for ((id, slot, t0, t1), mut coords) in groups {
        coords.sort_unstable();
        coords.dedup();
        let axis = axis_by_id[&id];
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

    fn atom(id: usize, axis: MotifAxis, nonmundanity: f64, cube: (i32, i32, i32)) -> MotifAtom {
        MotifAtom {
            motif: Motif::Nonmundane {
                axis,
                nonmundanity,
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
        // the pattern's column (X) axis.
        let input = "\
== wall: wall ==
H:
 @ W
--> - ok 0.75
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
}
