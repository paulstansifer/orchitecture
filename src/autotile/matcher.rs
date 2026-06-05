use super::compiler::*;
use super::parser::*;

use crate::sparse3d::{RelSlot, SlotLocation, Sparse3D};
use crate::structure::StructureId;
use crate::wall_grid::Cell;
use bevy::math::IVec3;

// ─── RelSlot conversion ───────────────────────────────────────────────────────

impl From<AutotileRelSlot> for crate::sparse3d::RelSlot {
    fn from(s: AutotileRelSlot) -> Self {
        match s {
            AutotileRelSlot::Room => Self::Room,
            AutotileRelSlot::XHiWall => Self::XHiWall,
            AutotileRelSlot::XLoWall => Self::XLoWall,
            AutotileRelSlot::Floor => Self::Floor,
            AutotileRelSlot::Ceiling => Self::Ceiling,
            AutotileRelSlot::ZHiWall => Self::ZHiWall,
            AutotileRelSlot::ZLoWall => Self::ZLoWall,
        }
    }
}

// ─── Matching ─────────────────────────────────────────────────────────────────

/// Returns the result for the first matching oriented case.
///
/// `char_matches_id` answers "does this neighbor's StructureId satisfy this pattern character?"
/// for any character other than `' '` (wildcard, always true) and `'.'` (empty slot).
/// This does not match the anchor itself! It's expected that we will look at every structure, see
/// which rules use that structure as the anchor, and then call `match_pattern` on them.
pub fn match_pattern<'a>(
    oriented: &'a AutotileOriented,
    grid: &Sparse3D<Cell>,
    anchor: SlotLocation,
    char_matches_id: impl Fn(char, StructureId) -> bool,
) -> Option<&'a AutotileResult> {
    let turn_90 = matches!(anchor.rel_slot, RelSlot::ZLoWall | RelSlot::ZHiWall);
    let cases = if turn_90 {
        &oriented.cases_plus_90
    } else {
        &oriented.cases
    };
    for case in cases {
        let matches = case.checks.iter().all(|(&offset, &ch)| {
            let (cx, cy, cz) = offset.cube_offset;
            let neighbor = anchor.apply_offset(crate::sparse3d::RelSlotOffset {
                origin_slot: offset.origin_slot.into(),
                cube_offset: IVec3::new(cx, cy, cz),
                dest_slot: offset.dest_slot.into(),
            });
            let cell = grid.get(neighbor);
            match ch {
                ' ' => true,
                '.' => cell.is_none(),
                _ => cell.map_or(false, |c| char_matches_id(ch, c.id)),
            }
        });
        if matches {
            return Some(&case.result);
        }
    }
    None
}

/// Map a `RelSlot` to the `UnorientedSlot` category used in autotile rule headers.
pub fn rel_slot_to_unoriented(slot: RelSlot) -> UnorientedSlot {
    match slot {
        RelSlot::Room => UnorientedSlot::Room,
        RelSlot::Floor | RelSlot::Ceiling => UnorientedSlot::Floor,
        _ => UnorientedSlot::Wall,
    }
}

/// Apply every autotile rule that matches `cell_name` and the slot implied by `loc`,
/// returning one `AutotileResult` per rule (first-match-wins within each rule).
///
/// Returns `None` when no rules apply to this structure at all (so the caller can
/// fall back to the default mesh).
pub fn evaluate_autotile_rules(
    loc: SlotLocation,
    cell_name: &str,
    rules: &[AutotileOriented],
    grid: &Sparse3D<Cell>,
    char_matches: impl Fn(char, StructureId) -> bool,
) -> Option<Vec<AutotileResult>> {
    let unoriented = rel_slot_to_unoriented(loc.rel_slot);
    let matching: Vec<_> = rules
        .iter()
        .filter(|r| r.structure_name == cell_name && r.slot == unoriented)
        .collect();
    if matching.is_empty() {
        return None;
    }
    Some(
        matching
            .iter()
            .map(|rule| {
                match_pattern(rule, grid, loc, &char_matches)
                    .cloned()
                    .unwrap_or(AutotileResult::None)
            })
            .collect(),
    )
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;
    use crate::sparse3d::{RelSlot, SlotLocation, Sparse3D};
    use crate::structure::StructureId;
    use crate::wall_grid::Cell;

    fn atom(name: &str) -> MeshSpec {
        MeshSpec::Atom {
            name: name.to_owned(),
            rotation: 0,
        }
    }
    fn atom_r(name: &str, r: i32) -> MeshSpec {
        MeshSpec::Atom {
            name: name.to_owned(),
            rotation: r,
        }
    }

    // Test-local ID convention: 0=wall, 1=floor, 2=stairs, 3=railing
    fn wall_cell() -> Cell {
        Cell {
            id: StructureId(0),
            facing: Default::default(),
            evaluation: None,
        }
    }

    fn test_char_matches(ch: char, id: StructureId) -> bool {
        match ch {
            'W' => id.0 == 0,
            'F' => id.0 == 1,
            'S' => id.0 == 2,
            'R' => id.0 == 3,
            _ => false,
        }
    }

    #[test]
    fn else_case_always_matches() {
        let input = "\
== wall: wall ==
--> none
";
        let file = parse(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let grid: Sparse3D<Cell> = Sparse3D::new();
        let anchor = SlotLocation::new(0, 0, 0, RelSlot::XLoWall);
        let result = match_pattern(&oriented, &grid, anchor, test_char_matches);
        check!(result == Some(&AutotileResult::None));
    }

    #[test]
    fn pattern_matches_correct_neighbor() {
        // One wall across a room from another wall.
        let input = "\
== wall: wall ==
H:
 @ W
--> wall_across
--> none
";
        let file = parse(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let anchor = SlotLocation::new(0, 0, 0, RelSlot::XLoWall);

        // Grid with a wall at (1, 0, 0) relative to anchor
        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(SlotLocation::new(1, 0, 0, RelSlot::XLoWall), wall_cell());
        let result = match_pattern(&oriented, &grid, anchor, test_char_matches);

        check!(
            result
                == Some(&AutotileResult::Mesh {
                    multi: false,
                    spec: atom("wall_across")
                })
        );

        // Empty grid: should fall through to else
        let empty_grid: Sparse3D<Cell> = Sparse3D::new();
        let result2 = match_pattern(&oriented, &empty_grid, anchor, test_char_matches);
        check!(result2 == Some(&AutotileResult::None));
    }

    /// Verify that `rotate_autotile_rel_slot` agrees with sparse3d's slot rotation:
    /// an XLoWall→XLoWall neighbor pattern (cases) should produce a matching
    /// ZLoWall→ZLoWall neighbor pattern (cases_plus_90) after one CW turn.
    #[test]
    fn wall_slot_rotation_consistent_with_sparse3d() {
        // Wall with a wall neighbor in the +X direction (both XLoWall slots).
        let input = "\
== wall: wall ==
H:
 @ W
--> wall_across
--> none
";
        let file = parse(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);

        let anchor_x = SlotLocation::new(0, 0, 0, RelSlot::XLoWall);
        let anchor_z = SlotLocation::new(0, 0, 0, RelSlot::ZLoWall);

        // Base case (cases): XLoWall anchor, neighbor at XLoWall(1,0,0).
        let mut grid_x: Sparse3D<Cell> = Sparse3D::new();
        grid_x.set(SlotLocation::new(1, 0, 0, RelSlot::XLoWall), wall_cell());
        let r_x = match_pattern(&oriented, &grid_x, anchor_x, test_char_matches).unwrap();
        check!(r_x == &AutotileResult::Mesh { multi: false, spec: atom("wall_across") });

        // CW-rotated case (cases_plus_90): ZLoWall anchor.
        // sparse3d CW moves the XLoWall neighbor at (1,0,0) to ZLoWall at (0,0,1),
        // and the XLoWall anchor type becomes ZLoWall.
        let mut grid_z: Sparse3D<Cell> = Sparse3D::new();
        grid_z.set(SlotLocation::new(0, 0, 1, RelSlot::ZLoWall), wall_cell());
        let r_z = match_pattern(&oriented, &grid_z, anchor_z, test_char_matches).unwrap();
        check!(r_z == &AutotileResult::Mesh { multi: false, spec: atom("wall_across") },
            "ZLoWall anchor with ZLoWall neighbor at (0,0,1): expected wall_across, got {r_z:?}");

        // The 180°-rotated case: neighbor on the −Z side of the ZLoWall anchor.
        // That is the counterpart to an XLoWall anchor with neighbor at (−1,0,0).
        // It should also match wall_across, but via the rot=2 case (mesh rotation=2).
        let mut grid_z_back: Sparse3D<Cell> = Sparse3D::new();
        grid_z_back.set(SlotLocation::new(0, 0, -1, RelSlot::ZLoWall), wall_cell());
        let r_z_back = match_pattern(&oriented, &grid_z_back, anchor_z, test_char_matches).unwrap();
        if let AutotileResult::Mesh { spec, .. } = r_z_back {
            check!(
                spec.outer_rotation() == 180 && spec_stem(spec, UnorientedSlot::Wall) == "wall_across",
                "ZLoWall neighbor at (0,0,-1): expected wall_across with rotation 180, got {spec:?}"
            );
        } else {
            panic!("ZLoWall neighbor at (0,0,-1): expected Mesh result, got {r_z_back:?}");
        }
    }

    /// Verify that the rotation numbering in `compile_rule` is consistent with the
    /// rotation directions in `sparse3d`.
    ///
    /// sparse3d's Clockwise transform is (x,y,z)→(-z,y,x), so a +X neighbor at (1,0,0)
    /// moves to (0,0,1) after one CW turn.  The autotile compiled rule's rotation-1 case
    /// must match that same configuration — and its mesh result must carry rotation=1.
    #[test]
    fn rotation_consistent_with_sparse3d() {
        // Asymmetric room pattern: anchor desk has a matching neighbor in the +X direction.
        let input = "\
== desk: room ==
H:
 @ =
--> my_mesh
";
        let file = parse(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);

        fn desk_cell() -> Cell {
            Cell {
                id: StructureId(1),
                facing: Default::default(),
                evaluation: None,
            }
        }
        fn is_desk(ch: char, id: StructureId) -> bool {
            ch == '=' && id.0 == 1
        }
        fn mesh_rotation(r: &AutotileResult) -> i32 {
            match r {
                AutotileResult::Mesh { spec, .. } => spec.outer_rotation(),
                other => panic!("expected mesh, got {other:?}"),
            }
        }

        let anchor = SlotLocation::new(0, 0, 0, RelSlot::Room);

        // No neighbor: nothing matches (rule has no else case).
        let empty: Sparse3D<Cell> = Sparse3D::new();
        check!(match_pattern(&oriented, &empty, anchor, is_desk).is_none());

        // rot=0: neighbor in +X direction → Room(1,0,0).
        let mut g0: Sparse3D<Cell> = Sparse3D::new();
        g0.set(SlotLocation::new(1, 0, 0, RelSlot::Room), desk_cell());
        let r0 = match_pattern(&oriented, &g0, anchor, is_desk).unwrap();
        check!(mesh_rotation(r0) == 0, "rot=0: expected mesh rotation 0, got {}", mesh_rotation(r0));

        // Rotating the world CW (sparse3d Clockwise: (x,y,z)→(-z,y,x)) moves
        // the +X neighbor (1,0,0) to (0,0,1).  The autotile rot=1 (CW) case must
        // match this configuration and return mesh rotation=90°.
        let mut g1: Sparse3D<Cell> = Sparse3D::new();
        g1.set(SlotLocation::new(0, 0, 1, RelSlot::Room), desk_cell());
        let r1 = match_pattern(&oriented, &g1, anchor, is_desk).unwrap();
        check!(mesh_rotation(r1) == 90, "rot=1 (CW): expected mesh rotation 90, got {}", mesh_rotation(r1));

        // 180° (sparse3d OneEighty: (x,y,z)→(-x,y,-z)) moves (1,0,0) to (-1,0,0).
        let mut g2: Sparse3D<Cell> = Sparse3D::new();
        g2.set(SlotLocation::new(-1, 0, 0, RelSlot::Room), desk_cell());
        let r2 = match_pattern(&oriented, &g2, anchor, is_desk).unwrap();
        check!(mesh_rotation(r2) == 180, "rot=2 (180°): expected mesh rotation 180, got {}", mesh_rotation(r2));

        // CCW / 270° CW (sparse3d CounterClockwise: (x,y,z)→(z,y,-x)) moves
        // (1,0,0) to (0,0,-1).  The autotile rot=3 case must match.
        let mut g3: Sparse3D<Cell> = Sparse3D::new();
        g3.set(SlotLocation::new(0, 0, -1, RelSlot::Room), desk_cell());
        let r3 = match_pattern(&oriented, &g3, anchor, is_desk).unwrap();
        check!(mesh_rotation(r3) == 270, "rot=3 (CCW): expected mesh rotation 270, got {}", mesh_rotation(r3));
    }

    // ── Column autotile tests ─────────────────────────────────────────────────

    fn all_rules() -> Vec<AutotileOriented> {
        let src = include_str!("../../buildables/structures.autotile");
        compile(&parse(src).unwrap())
    }

    fn stems_from_results(results: &[AutotileResult]) -> Vec<String> {
        results
            .iter()
            .filter_map(|r| {
                if let AutotileResult::Mesh { spec, .. } = r {
                    Some(spec_stem(spec, UnorientedSlot::Wall))
                } else {
                    None
                }
            })
            .collect()
    }

    // IDs used in column tests: 0 = floor, 1 = column
    fn col_cell() -> Cell {
        Cell { id: StructureId(1), facing: Default::default(), evaluation: None }
    }

    fn col_char_matches(ch: char, id: StructureId) -> bool {
        match ch {
            '=' => id.0 == 1, // column (same structure as anchor)
            'F' => id.0 == 0, // floor
            _ => false,
        }
    }

    /// A lone column with no neighbours falls through to the else case in both
    /// stanzas and produces two `column_floating` meshes.
    #[test]
    fn column_single_produces_two_floating() {
        let rules = all_rules();
        let anchor = SlotLocation::new(1, 0, 0, RelSlot::XLoWall);
        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(anchor, col_cell());

        let results = evaluate_autotile_rules(anchor, "column", &rules, &grid, col_char_matches)
            .expect("column has autotile rules");
        let stems = stems_from_results(&results);

        check!(stems.len() == 2, "expected 2 stems, got {stems:?}");
        check!(stems.contains(&"isect__column_floating__top".to_string()), "stems={stems:?}");
        check!(stems.contains(&"isect__column_floating__bottom".to_string()), "stems={stems:?}");
    }

    /// Z-wall variant of `column_single_produces_two_floating`: the else case must
    /// appear in `cases_plus_90` so that ZLoWall anchors also get the fallback mesh.
    #[test]
    fn column_single_zwall_produces_two_floating() {
        let rules = all_rules();
        let anchor = SlotLocation::new(1, 0, 0, RelSlot::ZLoWall);
        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(anchor, col_cell());

        let results = evaluate_autotile_rules(anchor, "column", &rules, &grid, col_char_matches)
            .expect("column has autotile rules");
        let stems = stems_from_results(&results);

        check!(stems.len() == 2, "expected 2 stems for ZLoWall column, got {stems:?}");
        check!(stems.contains(&"isect__column_floating__top".to_string()), "stems={stems:?}");
        check!(stems.contains(&"isect__column_floating__bottom".to_string()), "stems={stems:?}");
    }

    /// Two columns stacked vertically with no floors: the outer ends get
    /// `column_floating` (no neighbour beyond them) and the joint gets
    /// `column_middle` on both sides.
    #[test]
    fn column_stacked_produces_four_meshes() {
        let rules = all_rules();
        let anchor_lo = SlotLocation::new(1, 0, 0, RelSlot::XLoWall);
        let anchor_hi = SlotLocation::new(1, 1, 0, RelSlot::XLoWall);
        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(anchor_lo, col_cell());
        grid.set(anchor_hi, col_cell());

        let results_lo = evaluate_autotile_rules(anchor_lo, "column", &rules, &grid, col_char_matches)
            .expect("column has autotile rules");
        let results_hi = evaluate_autotile_rules(anchor_hi, "column", &rules, &grid, col_char_matches)
            .expect("column has autotile rules");

        let stems_lo = stems_from_results(&results_lo);
        let stems_hi = stems_from_results(&results_hi);
        let all: Vec<_> = stems_lo.iter().chain(stems_hi.iter()).cloned().collect();

        check!(all.len() == 4, "expected 4 stems total, got {all:?}");
        for expected in &[
            "isect__column_floating__top",    // bottom column, stanza 1: no column below
            "isect__column_middle__bottom",   // bottom column, stanza 2: column above present
            "isect__column_middle__top",      // top column,    stanza 1: column below present
            "isect__column_floating__bottom", // top column,    stanza 2: no column above
        ] {
            check!(all.contains(&expected.to_string()), "missing {expected}; got {all:?}");
        }
    }
}
