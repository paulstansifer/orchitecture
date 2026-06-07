use std::collections::HashMap;

use super::parser::*;

// ─── Compiled / oriented representation ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OrientedCase {
    pub pattern_type: PatternType,
    /// Non-wildcard checks relative to @ (the "anchor"). Empty = else/fallback case.
    pub checks: HashMap<AutotileRelSlotOffset, char>,
    pub result: AutotileResult,
}

#[derive(Debug, Clone)]
pub struct AutotileOriented {
    pub structure_name: String,
    pub slot: UnorientedSlot,
    /// Cases in priority order; else case (empty checks) is last.
    pub cases: Vec<OrientedCase>,
    // If `slot` is Wall, this will be 90-degree-rotated versions of `cases`.
    // Otherwise, it'll be empty
    pub cases_plus_90: Vec<OrientedCase>,
}

// ─── Rotation helpers ─────────────────────────────────────────────────────────

/// Rotate (dcol, drow) by `rot` 90°-CW steps in the XZ plane.
/// CW matches sparse3d's Rotation::Clockwise: (x, z) → (-z, x), so (dc, dr) → (-dr, dc).
pub fn rotate_offset(dc: i32, dr: i32, rot: u8) -> (i32, i32) {
    match rot % 4 {
        0 => (dc, dr),
        1 => (-dr, dc),
        2 => (-dc, -dr),
        3 => (dr, -dc),
        _ => unreachable!(),
    }
}

fn rotate_autotile_rel_slot(slot: AutotileRelSlot, rot: u8) -> AutotileRelSlot {
    match rot % 4 {
        0 => slot,
        1 => match slot {
            AutotileRelSlot::XLoWall => AutotileRelSlot::ZLoWall,
            AutotileRelSlot::XHiWall => AutotileRelSlot::ZHiWall,
            AutotileRelSlot::ZLoWall => AutotileRelSlot::XHiWall,
            AutotileRelSlot::ZHiWall => AutotileRelSlot::XLoWall,
            other => other,
        },
        2 => match slot {
            AutotileRelSlot::XLoWall => AutotileRelSlot::XHiWall,
            AutotileRelSlot::XHiWall => AutotileRelSlot::XLoWall,
            AutotileRelSlot::ZLoWall => AutotileRelSlot::ZHiWall,
            AutotileRelSlot::ZHiWall => AutotileRelSlot::ZLoWall,
            other => other,
        },
        3 => match slot {
            AutotileRelSlot::XLoWall => AutotileRelSlot::ZHiWall,
            AutotileRelSlot::XHiWall => AutotileRelSlot::ZLoWall,
            AutotileRelSlot::ZLoWall => AutotileRelSlot::XLoWall,
            AutotileRelSlot::ZHiWall => AutotileRelSlot::XHiWall,
            other => other,
        },
        _ => unreachable!(),
    }
}

fn rotate_slot_offset(offset: AutotileRelSlotOffset, rot: u8) -> AutotileRelSlotOffset {
    let (x, y, z) = offset.cube_offset;
    let (rx, rz) = rotate_offset(x, z, rot);
    AutotileRelSlotOffset {
        origin_slot: rotate_autotile_rel_slot(offset.origin_slot, rot),
        cube_offset: (rx, y, rz),
        dest_slot: rotate_autotile_rel_slot(offset.dest_slot, rot),
    }
}

fn rotate_checks(
    checks: &HashMap<AutotileRelSlotOffset, char>,
    rot: u8,
) -> HashMap<AutotileRelSlotOffset, char> {
    checks
        .iter()
        .map(|(&offset, &ch)| (rotate_slot_offset(offset, rot), ch))
        .collect()
}

fn rotate_result(result: &AutotileResult, rot: u8) -> AutotileResult {
    match result {
        AutotileResult::None => AutotileResult::None,
        AutotileResult::Mesh { multi, spec } => AutotileResult::Mesh {
            multi: *multi,
            spec: spec.clone().rotate(rot as i32 * 90),
        },
    }
}

// ─── Compiler ────────────────────────────────────────────────────────────────

pub fn compile(file: &AutotileFile) -> Vec<AutotileOriented> {
    file.rules.iter().map(compile_rule).collect()
}

pub fn compile_rule(rule: &AutotileRule) -> AutotileOriented {
    let mut cases = Vec::new();
    let mut cases_plus_90 = Vec::new();

    for case in &rule.cases {
        match &case.pattern {
            None => {
                // Else case: no constraints, matches always. One copy.
                let oc = OrientedCase {
                    pattern_type: PatternType::H,
                    checks: HashMap::new(),
                    result: case.result.clone(),
                };
                if rule.slot == UnorientedSlot::Wall {
                    cases_plus_90.push(oc.clone());
                }
                cases.push(oc);
            }
            Some(pattern) => {
                let base_checks = pattern.relative_checks();
                let pt = pattern.pattern_type.clone();

                let mut seen: Vec<HashMap<AutotileRelSlotOffset, char>> = Vec::new();
                for rot in 0u8..4 {
                    if rule.slot == UnorientedSlot::Wall && rot % 2 == 1 {
                        // A particular wall is only symmetric in 180 degrees, not 90
                        // (But we need to store a 90-offset rotation for Z walls; the
                        // patterns are constructed for X walls.)
                        continue;
                    }
                    // The rotation of `result` in these is kind of bizarre; it's just how to make this match up correctly with
                    // `wall_grid::cell_transform`.
                    let rotated = rotate_checks(&base_checks, rot);
                    if !seen.iter().any(|s| s == &rotated) {
                        seen.push(rotated.clone());
                        cases.push(OrientedCase {
                            pattern_type: pt.clone(),
                            checks: rotated,
                            result: rotate_result(&case.result, rot + 2),
                        });

                        if rule.slot == UnorientedSlot::Wall {
                            let rotated_plus_90 = rotate_checks(&base_checks, rot + 1);
                            cases_plus_90.push(OrientedCase {
                                pattern_type: pt.clone(),
                                checks: rotated_plus_90,
                                result: rotate_result(&case.result, rot),
                            });
                        }
                    }
                }
            }
        }
    }

    AutotileOriented {
        structure_name: rule.structure_name.clone(),
        slot: rule.slot,
        cases,
        cases_plus_90,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;

    // ── Compilation / orientation expansion ──────────────────────────────────

    #[test]
    fn symmetric_pattern_produces_one_orientation() {
        // A pattern symmetric under all 4 rotations: only @ (no constraints).
        // Each case: pattern is just "@" with no other checks → all rotations identical.
        let input = "\
== wall: wall ==
H:
 @
--> mesh_a
";
        let file = parse(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        // All 4 rotations produce the same empty check-set, so only 1 is kept.
        check!(oriented.cases.len() == 1);
    }

    #[test]
    fn asymmetric_pattern_produces_four_orientations() {
        // Asymmetric: something only to the right of @.
        let input = "\
== desk: room ==
H:
 @ =
--> mesh_a
";
        let file = parse(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        check!(oriented.cases.len() == 4);
    }

    #[test]
    #[ignore = "fix after matching works"]
    fn two_fold_symmetric_pattern_produces_two_orientations() {
        // Something on both sides of @ symmetrically (left+right).
        // Rotation by 180° maps it back to itself; 90° and 270° are a second distinct set.
        let input = "\
== wall: wall ==
H:
 = @ =
--> mesh_a
";
        let file = parse(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        check!(oriented.cases.len() == 2);
    }

    #[test]
    fn rotation_propagates_to_mesh() {
        let input = "\
== desk: room ==
H:
 @ =
--> my_mesh
";
        let file = parse(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        // All 4 orientations must carry distinct mesh rotations covering {0,90,180,270}.
        let mut rotations: Vec<i32> = oriented
            .cases
            .iter()
            .map(|c| match &c.result {
                AutotileResult::Mesh { spec, .. } => spec.outer_rotation(),
                other => panic!("expected Mesh, got {other:?}"),
            })
            .collect();
        rotations.sort();
        check!(rotations == vec![0, 90, 180, 270]);
    }

    // ── Rotation math ─────────────────────────────────────────────────────────

    #[test]
    fn rotate_offset_identity() {
        check!(rotate_offset(3, 1, 0) == (3, 1));
    }

    #[test]
    fn rotate_offset_90cw() {
        // Matches sparse3d Clockwise: (x,z) → (-z,x).
        // (1, 0) [+X] → (0, 1) [+Z]
        check!(rotate_offset(1, 0, 1) == (0, 1));
        // (0, 1) [+Z] → (-1, 0) [-X]
        check!(rotate_offset(0, 1, 1) == (-1, 0));
    }

    #[test]
    fn rotate_offset_180() {
        check!(rotate_offset(2, 1, 2) == (-2, -1));
    }

    #[test]
    fn rotate_offset_four_times_is_identity() {
        let (dc, dr) = (3, -2);
        let mut p = (dc, dr);
        for _ in 0..4 {
            p = rotate_offset(p.0, p.1, 1);
        }
        check!(p == (dc, dr));
    }

    #[test]
    fn rotate_offset_compose() {
        // rot=2 must equal two rot=1 steps; rot=3 must equal three.
        for (dc, dr) in [(1, 0), (0, 1), (-1, 0), (0, -1), (2, 3), (-1, 2)] {
            let step1 = rotate_offset(dc, dr, 1);
            let step2 = rotate_offset(step1.0, step1.1, 1);
            let step3 = rotate_offset(step2.0, step2.1, 1);
            check!(
                step2 == rotate_offset(dc, dr, 2),
                "({dc},{dr}): 90°×2 ≠ 180°"
            );
            check!(
                step3 == rotate_offset(dc, dr, 3),
                "({dc},{dr}): 90°×3 ≠ 270°"
            );
        }
    }

    #[test]
    fn rotate_autotile_rel_slot_compose() {
        use AutotileRelSlot::*;
        for slot in [Room, XHiWall, XLoWall, Floor, Ceiling, ZHiWall, ZLoWall] {
            let step1 = rotate_autotile_rel_slot(slot, 1);
            let step2 = rotate_autotile_rel_slot(step1, 1);
            let step3 = rotate_autotile_rel_slot(step2, 1);
            let step4 = rotate_autotile_rel_slot(step3, 1);
            check!(
                step2 == rotate_autotile_rel_slot(slot, 2),
                "{slot:?}: 90°×2 ≠ 180°"
            );
            check!(
                step3 == rotate_autotile_rel_slot(slot, 3),
                "{slot:?}: 90°×3 ≠ 270°"
            );
            check!(step4 == slot, "{slot:?}: 90°×4 ≠ identity");
        }
    }
}
