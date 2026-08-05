use std::collections::HashMap;

use super::parser::*;
use crate::sparse3d::Facing;

// ─── Structure info for category generation ──────────────────────────────────

#[derive(Clone, Debug)]
pub struct StructureInfo {
    pub name: String,
}

// ─── Compiled / oriented representation ──────────────────────────────────────

/// Compiled form of a labeled pattern character's annotation (e.g. `1=stairs:90`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotatedMatcher {
    pub name: String,
    /// Required facing after this case's rotation has been applied. `None` = any facing.
    pub orientation: Option<Facing>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    Atom(AutotileRelSlotOffset, char),
    Or(Vec<Condition>),
}

#[derive(Debug, Clone)]
pub struct OrientedCase<R> {
    pub pattern_type: PatternType,
    /// Non-wildcard checks relative to @ (the "anchor"). Empty = else/fallback case.
    pub checks: Vec<Condition>,
    pub result: R,
    /// Labeled character → compiled annotation (empty when the pattern has no annotations).
    pub char_annotations: HashMap<char, AnnotatedMatcher>,
    /// Index of the source (parser) case this orientation was expanded from. All orientations
    /// of one source case share a group, and a group's orientations are contiguous.
    pub group: usize,
    /// `(multi)`: when one orientation in this group matches, every matching orientation emits
    /// its mesh. See [`super::parser::PatternCase::multi`].
    pub multi: bool,
    /// For `==empty:...==` rules: the offset from the dispatch anchor (`checks`' origin) to `@`,
    /// where the result should actually be recorded. `None` means "record at the dispatch
    /// anchor's own location" — always true for ordinary (non-empty-anchored) rules, and also
    /// used as a fallback for an empty-anchored rule's else-case, which has no `@` of its own.
    pub output_offset: Option<AutotileRelSlotOffset>,
}

#[derive(Debug, Clone)]
pub struct AutotileOriented<R> {
    pub subject: RuleSubject,
    pub slot: UnorientedSlot,
    /// Cases in priority order; else case (empty checks) is last.
    pub cases: Vec<OrientedCase<R>>,
    // If `slot` is Wall, this will be 90-degree-rotated versions of `cases`.
    // Otherwise, it'll be empty
    pub cases_plus_90: Vec<OrientedCase<R>>,
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

/// A canonicalized `AutotileRelSlotOffset`: the slot it lands on, and the cube offset from the
/// checked cell's origin cube to its own -- see `canonical_offset_key`.
type CanonicalOffsetKey = (AutotileRelSlot, (i32, i32, i32));

/// A dedup key for a whole (rotated) check set -- see `canonical_checks_key`.
type CanonicalChecks = HashMap<CanonicalOffsetKey, char>;

/// A location key for `AutotileRelSlotOffset` that's independent of the arbitrary Lo/Hi choice
/// of `origin_slot`/`dest_slot` -- e.g. `XLoWall` at cube offset `(1,0,0)` and `XHiWall` at cube
/// offset `(0,0,0)` name the same physical wall (see `AutotileRelSlot::XLoWall`'s doc comment),
/// and `rotate_slot_offset` can produce either spelling for the same real check depending on
/// rotation. Mirrors the canonicalization `RelSlotCoord::apply_offset` performs at match time, so
/// two checks that always resolve to the same cell dedup together even when their Lo/Hi spelling
/// differs.
fn canonical_offset_key(offset: AutotileRelSlotOffset) -> CanonicalOffsetKey {
    let (ox, oy, oz) = offset.origin_slot.absolute_offset();
    let (mut cx, mut cy, mut cz) = offset.cube_offset;
    cx -= ox;
    cy -= oy;
    cz -= oz;
    let dest_slot = match offset.dest_slot {
        AutotileRelSlot::XHiWall => {
            cx += 1;
            AutotileRelSlot::XLoWall
        }
        AutotileRelSlot::ZHiWall => {
            cz += 1;
            AutotileRelSlot::ZLoWall
        }
        AutotileRelSlot::Ceiling => {
            cy += 1;
            AutotileRelSlot::Floor
        }
        other => other,
    };
    (dest_slot, (cx, cy, cz))
}

/// A dedup key for a whole (rotated) check set: canonicalizes each offset (see
/// `canonical_offset_key`) so that two check sets naming the same physical cells compare equal
/// even if they disagree on Lo/Hi spelling.
fn canonical_checks_key(checks: &HashMap<AutotileRelSlotOffset, char>) -> CanonicalChecks {
    checks
        .iter()
        .map(|(&offset, &ch)| (canonical_offset_key(offset), ch))
        .collect()
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

/// Convert a raw `(offset → char)` map into `Vec<Condition>`, expanding special chars.
/// `'r'` expands to `Or([Atom(wall_offset, 'W'), Atom(far_floor_offset, 'O')])`;
/// see `AutotileRelSlotOffset::far_floor_offset` for the geometry.
fn checks_to_conditions(checks: &HashMap<AutotileRelSlotOffset, char>) -> Vec<Condition> {
    checks
        .iter()
        .map(|(&offset, &ch)| match ch {
            'r' => Condition::Or(vec![
                Condition::Atom(offset, 'W'),
                Condition::Atom(offset.far_floor_offset(), 'O'),
            ]),
            _ => Condition::Atom(offset, ch),
        })
        .collect()
}

/// Convert annotation degrees (0° = PosX) to a `Facing` value.
fn degrees_to_facing(degrees: i32) -> Facing {
    // PosX = 2; each 90° CW step adds 1 mod 4
    Facing::from_number((2 + degrees / 90).rem_euclid(4) as u8)
}

/// Rotate a `Facing` by `rot` 90°-CW steps.
fn rotate_facing(f: Facing, rot: u8) -> Facing {
    Facing::from_number(f as u8 + rot)
}

fn rotate_annotations(
    annotations: &HashMap<char, PatternAnnotation>,
    rot: u8,
) -> HashMap<char, AnnotatedMatcher> {
    annotations
        .iter()
        .map(|(&ch, ann)| {
            let orientation = ann
                .orientation
                .map(|deg| rotate_facing(degrees_to_facing(deg), rot));
            (
                ch,
                AnnotatedMatcher {
                    name: ann.name.clone(),
                    orientation,
                },
            )
        })
        .collect()
}

// ─── Compiler ────────────────────────────────────────────────────────────────

pub fn compile<R: AutotileResultKind>(file: &AutotileFile<R>) -> Vec<AutotileOriented<R>> {
    file.rules.iter().map(compile_rule).collect()
}

/// Extract all mesh names (stems) from the compiled rules, organized by structure name.
pub fn structure_to_meshes(file: &AutotileFile<AutotiledMeshes>) -> HashMap<String, Vec<String>> {
    let mut mapping: HashMap<String, Vec<String>> = HashMap::new();

    for rule in &file.rules {
        // Empty-anchored rules aren't associated with a placeable structure's mesh category.
        let Some(structure_name) = rule.subject.structure_name() else {
            continue;
        };
        let meshes = mapping.entry(structure_name.to_owned()).or_default();

        for case in &rule.cases {
            if let AutotiledMeshes::Mesh { spec } = &case.result {
                let mesh_name = spec_stem(spec, rule.slot);
                if !meshes.contains(&mesh_name) {
                    meshes.push(mesh_name);
                }
            }
        }
    }

    mapping
}

/// Generate structure_categories.json, mapping all mesh names (both standalone and
/// from autotile rules) to their category (elements or furniture).
pub fn generate_structure_categories_json(
    file: &AutotileFile<AutotiledMeshes>,
    elements: &[StructureInfo],
    furniture: &[StructureInfo],
) -> String {
    let mut categories: HashMap<String, String> = HashMap::new();
    let structure_meshes = structure_to_meshes(file);

    // Map element structures and their meshes
    for info in elements {
        if !info.name.starts_with("u_") {
            categories.insert(info.name.clone(), "elements".to_string());
        }
        // Also map any meshes belonging to this structure
        if let Some(meshes) = structure_meshes.get(&info.name) {
            for mesh in meshes {
                if !mesh.starts_with("u_") {
                    categories.insert(mesh.clone(), "elements".to_string());
                }
            }
        }
    }

    // Map furniture structures and their meshes
    for info in furniture {
        if !info.name.starts_with("u_") {
            categories.insert(info.name.clone(), "furniture".to_string());
        }
        // Also map any meshes belonging to this structure
        if let Some(meshes) = structure_meshes.get(&info.name) {
            for mesh in meshes {
                if !mesh.starts_with("u_") {
                    categories.insert(mesh.clone(), "furniture".to_string());
                }
            }
        }
    }

    // Generate JSON
    let mut json = String::from("{\n");
    let mut first = true;

    for (name, category) in &categories {
        if !first {
            json.push_str(",\n");
        }
        first = false;
        json.push_str(&format!("  \"{}\": \"{}\"", name, category));
    }

    json.push_str("\n}\n");
    json
}

pub fn compile_rule<R: AutotileResultKind>(rule: &AutotileRule<R>) -> AutotileOriented<R> {
    let mut cases = Vec::new();
    let mut cases_plus_90 = Vec::new();

    for (group, case) in rule.cases.iter().enumerate() {
        match &case.pattern {
            None => {
                // Else case: no constraints, matches always. One copy.
                let oc = OrientedCase {
                    pattern_type: PatternType::H,
                    checks: vec![],
                    result: case.result.clone(),
                    char_annotations: HashMap::new(),
                    group,
                    multi: case.multi,
                    output_offset: None,
                };
                if rule.slot == UnorientedSlot::Wall {
                    cases_plus_90.push(oc.clone());
                }
                cases.push(oc);
            }
            Some(pattern) => {
                let (base_checks, base_output_offset) = pattern.relative_checks_with_output();
                let base_annotations = &pattern.annotations;
                let pt = pattern.anchor_pattern_type();

                // H/H-narrow patterns have two wall positions per tile (`ZLoWall`/`XLoWall`).
                // Ordinary rules always land their anchor on `XLoWall` (via `offset()`'s
                // padding), but an empty-anchored rule's `dispatch_anchor` isn't padded that way
                // and may land on either -- so route each orientation by the *actual* resulting
                // family (checked fresh per rotation) rather than assuming `rule.slot == Wall`
                // means "anchor starts X-family". `cases` (X-family/non-wall) and `cases_plus_90`
                // (Z-family) are deduplicated independently, since a symmetric pattern can still
                // need one entry in each.
                let anchor_origin_slot = pattern.anchor_origin_slot();
                let anchor_is_wall = matches!(
                    anchor_origin_slot,
                    AutotileRelSlot::XLoWall
                        | AutotileRelSlot::XHiWall
                        | AutotileRelSlot::ZLoWall
                        | AutotileRelSlot::ZHiWall
                );

                let mut seen: Vec<CanonicalChecks> = Vec::new();
                let mut seen_plus_90: Vec<CanonicalChecks> = Vec::new();
                for rot in 0u8..4 {
                    let rotated = rotate_checks(&base_checks, rot);
                    let canonical = canonical_checks_key(&rotated);
                    let is_z_family = anchor_is_wall
                        && matches!(
                            rotate_autotile_rel_slot(anchor_origin_slot, rot),
                            AutotileRelSlot::ZLoWall | AutotileRelSlot::ZHiWall
                        );

                    if !is_z_family {
                        if !seen.iter().any(|s| s == &canonical) {
                            seen.push(canonical);
                            cases.push(OrientedCase {
                                pattern_type: pt,
                                checks: checks_to_conditions(&rotated),
                                result: case.result.rotate(rot, CaseList::Base),
                                char_annotations: rotate_annotations(base_annotations, rot),
                                group,
                                multi: case.multi,
                                output_offset: base_output_offset
                                    .map(|o| rotate_slot_offset(o, rot)),
                            });
                        }
                    } else if !seen_plus_90.iter().any(|s| s == &canonical) {
                        seen_plus_90.push(canonical);
                        cases_plus_90.push(OrientedCase {
                            pattern_type: pt,
                            checks: checks_to_conditions(&rotated),
                            result: case.result.rotate(rot, CaseList::Plus90),
                            char_annotations: rotate_annotations(base_annotations, rot),
                            group,
                            multi: case.multi,
                            output_offset: base_output_offset.map(|o| rotate_slot_offset(o, rot)),
                        });
                    }
                }
            }
        }
    }

    debug_assert_contiguous_groups(&cases);
    debug_assert_contiguous_groups(&cases_plus_90);

    AutotileOriented {
        subject: rule.subject.clone(),
        slot: rule.slot,
        cases,
        cases_plus_90,
    }
}

/// `matcher::match_pattern_cases` assumes a group's orientations are never split up (see
/// `OrientedCase::group`'s doc comment) so it can scan for a group's boundary just by watching
/// `group` change. Nothing in the types enforces that here, so check it explicitly right where
/// the invariant is established, instead of leaving it as an unenforced doc comment.
fn debug_assert_contiguous_groups<R>(cases: &[OrientedCase<R>]) {
    if cfg!(debug_assertions) {
        let mut seen_groups = std::collections::HashSet::new();
        let mut prev_group = None;
        for case in cases {
            if prev_group != Some(case.group) {
                assert!(
                    seen_groups.insert(case.group),
                    "case group {} is split across non-contiguous runs",
                    case.group
                );
                prev_group = Some(case.group);
            }
        }
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
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        // All 4 rotations produce the same empty check-set, so only 1 is kept.
        check!(oriented.cases.len() == 1);
    }

    #[test]
    fn asymmetric_pattern_produces_four_orientations() {
        // Asymmetric: something only to the right of @.
        let input = "\
== table: room ==
H:
 @ =
--> mesh_a
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        check!(oriented.cases.len() == 4);
    }

    #[test]
    fn multi_layer_pure_vertical_is_symmetric() {
        // A multi-layer rule whose only check is directly above (X=Z=0) is
        // unchanged by XZ rotation, so all 4 orientations collapse to one. This
        // confirms the dedup still works once patterns carry a layer (Y) axis.
        let input = "\
== stack: room ==
|H|H|:
|=|@|
--> mesh_a
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        check!(oriented.cases.len() == 1);
    }

    #[test]
    fn multi_layer_xz_asymmetric_produces_four_orientations() {
        // A multi-layer rule with a horizontal (XZ) neighbour is still rotated
        // into 4 distinct orientations.
        let input = "\
== stack: room ==
|H|H|:
|   |@ =|
--> mesh_a
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
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
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        check!(oriented.cases.len() == 2);
    }

    // ── `==empty:...==` output_offset ─────────────────────────────────────────

    /// For an empty-anchored rule, every compiled case must carry an `output_offset` (the vector
    /// from the dispatch anchor back to `@`), rotated consistently with `checks`.
    #[test]
    fn empty_anchor_output_offset_present_and_rotates() {
        let input = "\
== empty: wall ==
H:
 @ W
--> mesh_a
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);

        for case in oriented.cases.iter().chain(oriented.cases_plus_90.iter()) {
            check!(
                case.output_offset.is_some(),
                "case missing output_offset: {case:?}"
            );
        }

        // rot=0: 'W' (the dispatch anchor) is at cube_offset (1,0,0) from '@' as authored, so
        // recentered on 'W', '@' sits at cube_offset (-1,0,0).
        let rot0 = &oriented.cases[0];
        check!(rot0.output_offset.unwrap().cube_offset == (-1, 0, 0));
    }

    #[test]
    fn rotation_propagates_to_mesh() {
        let input = "\
== table: room ==
H:
 @ =
--> my_mesh
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        // All 4 orientations must carry distinct mesh rotations covering {0,90,180,270}.
        let mut rotations: Vec<i32> = oriented
            .cases
            .iter()
            .map(|c| match &c.result {
                AutotiledMeshes::Mesh { spec, .. } => spec.outer_rotation(),
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

    // ── Annotation compilation ────────────────────────────────────────────────

    /// A rule that cares about the orientation of one of its matchers.
    #[test]
    fn annotation_compiled_into_oriented_cases() {
        let input = "\
== railing: wall ==
H: 1=stairs:90
 @1
--> stair_railing:90
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);

        let orientations_in_cases: Vec<Option<Facing>> = oriented
            .cases
            .iter()
            .map(|c| c.char_annotations.get(&'1').and_then(|a| a.orientation))
            .collect();

        // rot=0 → PosZ; rot=2 → NegZ (PosZ rotated 180°).
        check!(
            orientations_in_cases.contains(&Some(Facing::PosZ)),
            "cases missing PosZ annotation; got {orientations_in_cases:?}"
        );
        check!(
            orientations_in_cases.contains(&Some(Facing::NegZ)),
            "cases missing NegZ annotation; got {orientations_in_cases:?}"
        );

        let orientations_plus_90: Vec<Option<Facing>> = oriented
            .cases_plus_90
            .iter()
            .map(|c| c.char_annotations.get(&'1').and_then(|a| a.orientation))
            .collect();

        // rot=0+1=1 → NegX; rot=2+1=3 → PosX.
        check!(
            orientations_plus_90.contains(&Some(Facing::NegX)),
            "cases_plus_90 missing NegX annotation; got {orientations_plus_90:?}"
        );
        check!(
            orientations_plus_90.contains(&Some(Facing::PosX)),
            "cases_plus_90 missing PosX annotation; got {orientations_plus_90:?}"
        );
    }

    /// An annotation with no orientation compiles to `None` in every rotated case.
    #[test]
    fn annotation_without_orientation_stays_none_after_rotation() {
        let input = "\
== railing: wall ==
H: 1=stairs
 @1
--> stair_railing:90
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);

        for case in oriented.cases.iter().chain(oriented.cases_plus_90.iter()) {
            let ann = case.char_annotations.get(&'1').unwrap();
            check!(
                ann.orientation.is_none(),
                "orientation-less annotation should stay None after rotation"
            );
        }
    }
}
