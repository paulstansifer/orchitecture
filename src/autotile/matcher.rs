use std::collections::HashMap;

use super::compiler::*;
use super::parser::*;

use crate::eorf::EorfId;
use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, Slot};
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

fn check_condition<F1, F2, F3>(
    cond: &Condition,
    anchor: RelSlotCoord,
    get_cell: &F1,
    char_matches_id: &F2,
    name_matches_id: &F3,
    char_annotations: &HashMap<char, AnnotatedMatcher>,
) -> bool
where
    F1: Fn(RelSlotCoord) -> Option<(EorfId, Facing)>,
    F2: Fn(char, EorfId, Facing) -> bool,
    F3: Fn(&str, EorfId) -> bool,
{
    match cond {
        Condition::Atom(offset, ch) => {
            let (cx, cy, cz) = offset.cube_offset;
            let neighbor = anchor.apply_offset(crate::sparse3d::RelSlotCoordOffset {
                origin_slot: offset.origin_slot.into(),
                cube_offset: IVec3::new(cx, cy, cz),
                dest_slot: offset.dest_slot.into(),
            });
            let cell = get_cell(neighbor);
            match ch {
                ' ' => true,
                '.' => cell.is_none(),
                _ => {
                    if let Some((id, facing)) = cell {
                        if let Some(ann) = char_annotations.get(ch) {
                            let name_ok = name_matches_id(&ann.name, id);
                            let orient_ok = ann.orientation.is_none_or(|o| o == facing);
                            name_ok && orient_ok
                        } else {
                            char_matches_id(*ch, id, facing)
                        }
                    } else {
                        false
                    }
                }
            }
        }
        Condition::Or(conditions) => conditions.iter().any(|c| {
            check_condition(
                c,
                anchor,
                get_cell,
                char_matches_id,
                name_matches_id,
                char_annotations,
            )
        }),
    }
}

/// Returns the result(s) for the first matching oriented case group.
///
/// Orientations are grouped by their source case (see [`OrientedCase::group`]). Groups are
/// tried in priority order; the first group with any matching orientation wins and all later
/// groups are skipped. For an ordinary group only the first matching orientation is returned;
/// for a `(multi)` group every matching orientation is returned (so the same structure can emit
/// a mesh at several orientations at once). Returns an empty vec if no group matches.
///
/// `get_cell` maps a slot location to the `(EorfId, Facing)` occupying it (None = empty).
/// `char_matches_id` answers "does this neighbor satisfy this pattern character?"
/// for any character other than `' '` (wildcard), `'.'` (empty slot), and labeled annotation
/// characters (handled internally via `name_matches_id`).
/// `name_matches_id` answers "does this EorfId represent a structure named `name`?"
/// (used for annotation-style labeled matchers like `1=stairs:90`).
/// This does not match the anchor itself! It's expected that we will look at every structure, see
/// which rules use that structure as the anchor, and then call `match_pattern` on them.
pub fn match_pattern<'a, R: AutotileResultKind>(
    oriented: &'a AutotileOriented<R>,
    get_cell: impl Fn(RelSlotCoord) -> Option<(EorfId, Facing)>,
    anchor: RelSlotCoord,
    char_matches_id: impl Fn(char, EorfId, Facing) -> bool,
    name_matches_id: impl Fn(&str, EorfId) -> bool,
) -> Vec<&'a R> {
    match_pattern_cases(
        oriented,
        &get_cell,
        anchor,
        &char_matches_id,
        &name_matches_id,
    )
    .into_iter()
    .map(|case| &case.result)
    .collect()
}

/// Same matching logic as `match_pattern`, but returns the matched `OrientedCase`s themselves
/// (rather than just their results), so callers that need more than the result — e.g.
/// `evaluate_empty_anchor_rules`, which needs each case's `output_offset` — can get at it.
fn match_pattern_cases<'a, R: AutotileResultKind>(
    oriented: &'a AutotileOriented<R>,
    get_cell: &impl Fn(RelSlotCoord) -> Option<(EorfId, Facing)>,
    anchor: RelSlotCoord,
    char_matches_id: &impl Fn(char, EorfId, Facing) -> bool,
    name_matches_id: &impl Fn(&str, EorfId) -> bool,
) -> Vec<&'a OrientedCase<R>> {
    let turn_90 = matches!(anchor.rel_slot, RelSlot::ZLoWall | RelSlot::ZHiWall);
    let cases = if turn_90 {
        &oriented.cases_plus_90
    } else {
        &oriented.cases
    };

    let case_matches = |case: &OrientedCase<R>| {
        case.checks.iter().all(|cond| {
            check_condition(
                cond,
                anchor,
                get_cell,
                char_matches_id,
                name_matches_id,
                &case.char_annotations,
            )
        })
    };

    let mut i = 0;
    while i < cases.len() {
        let group = cases[i].group;
        let multi = cases[i].multi;
        let mut matched: Vec<&'a OrientedCase<R>> = Vec::new();
        while i < cases.len() && cases[i].group == group {
            if case_matches(&cases[i]) {
                matched.push(&cases[i]);
            }
            i += 1;
        }
        if !matched.is_empty() {
            if R::implicitly_multi() {
                // Every matching orientation is legitimate (e.g. an interior column with floor on
                // both sides, or a room corner, satisfying two orientations of the same case at
                // once) -- but drop exact duplicates (not necessarily adjacent in `matched`) so a
                // coincidence like that doesn't inflate the atom count beyond what a single
                // physical event should produce.
                let mut deduped: Vec<&'a OrientedCase<R>> = Vec::new();
                for m in matched {
                    if !deduped.iter().any(|d| d.result == m.result) {
                        deduped.push(m);
                    }
                }
                matched = deduped;
            } else if !multi {
                assert!(
                    matched.len() == 1,
                    "non-(multi) rule {:?} (slot {:?}, case group {group}) matched {} \
                     orientations at once at anchor {anchor:?}; mark it `(multi)` or make the \
                     orientations mutually exclusive",
                    oriented.subject,
                    oriented.slot,
                    matched.len()
                );
            }
            return matched;
        }
    }
    Vec::new()
}

/// Map a `RelSlot` to the `UnorientedSlot` category used in autotile rule headers.
pub fn rel_slot_to_unoriented(slot: RelSlot) -> UnorientedSlot {
    match slot {
        RelSlot::Room => UnorientedSlot::Room,
        RelSlot::Floor | RelSlot::Ceiling => UnorientedSlot::Floor,
        _ => UnorientedSlot::Wall,
    }
}

/// Map a canonical `Slot` to the `UnorientedSlot` category used in autotile rule headers.
pub fn slot_to_unoriented(slot: Slot) -> UnorientedSlot {
    match slot {
        Slot::Room => UnorientedSlot::Room,
        Slot::Floor => UnorientedSlot::Floor,
        Slot::XLoWall | Slot::ZLoWall => UnorientedSlot::Wall,
    }
}

/// Apply every autotile rule that matches `cell_name` and the slot implied by `loc`,
/// returning one `AutotiledMeshes` per rule (first-match-wins within each rule).
///
/// `get_cell` maps a slot location to `(EorfId, Facing)`; pass
/// `|loc| grid.get(loc).map(|c| (c.id, c.facing))` for real cells, or a closure over
/// `WallGrid::get_proposed_or_real` to include proposed additions.
///
/// Returns `None` when no rules apply to this structure at all (so the caller can
/// fall back to the default mesh).
pub fn evaluate_autotile_rules<R: AutotileResultKind>(
    loc: RelSlotCoord,
    cell_name: &str,
    rules: &[AutotileOriented<R>],
    get_cell: impl Fn(RelSlotCoord) -> Option<(EorfId, Facing)>,
    char_matches: impl Fn(char, EorfId, Facing) -> bool,
    name_matches: impl Fn(&str, EorfId) -> bool,
) -> Option<Vec<R>> {
    let unoriented = rel_slot_to_unoriented(loc.rel_slot);
    let matching: Vec<_> = rules
        .iter()
        .filter(|r| r.subject.structure_name() == Some(cell_name) && r.slot == unoriented)
        .collect();
    if matching.is_empty() {
        return None;
    }
    Some(
        matching
            .iter()
            .flat_map(|rule| {
                let results = match_pattern(rule, &get_cell, loc, &char_matches, &name_matches);
                let results: Vec<R> = if results.is_empty() {
                    // A rule with no matching case (and no else) still occupies a "slot"; emit a
                    // placeholder (if any) so callers see the rule applied (no fallback mesh).
                    R::no_match_placeholder().into_iter().collect()
                } else {
                    results.into_iter().cloned().collect()
                };
                results
            })
            .collect(),
    )
}

// ─── Empty-anchored rule dispatch ────────────────────────────────────────────

/// Index for dispatching `==empty:...==` rules: since their `@` anchor is empty and can't be
/// discovered by iterating occupied cells, they're instead triggered by whatever occupied cell
/// satisfies their dispatch-anchor character (see `Pattern::dispatch_anchor`). This index maps a
/// concrete structure name to the rules whose dispatch-anchor character it satisfies, so
/// per-cell dispatch is a hash lookup rather than a scan of every empty rule. Build once via
/// `build_empty_anchor_index` (structure names and rules are static after startup) and reuse it.
pub struct EmptyAnchorIndex<R> {
    rules: Vec<AutotileOriented<R>>,
    by_name: HashMap<String, Vec<usize>>,
}

/// The dispatch-anchor character of a compiled empty-anchored rule is the same across every
/// orientation: it's the check at cube offset `(0,0,0)` whose origin and destination slot
/// coincide (the anchor's own cell, recentered onto itself). Returns `None` if the rule has no
/// pattern-based case at all (an empty rule with only an else-case has no anchor to dispatch on).
fn dispatch_anchor_char<R: AutotileResultKind>(case: &OrientedCase<R>) -> Option<char> {
    case.checks.iter().find_map(|cond| match cond {
        Condition::Atom(offset, ch) if offset.cube_offset == (0, 0, 0) => Some(*ch),
        _ => None,
    })
}

/// Build the dispatch index for the `Empty`-subject rules among `all_rules`. `names` is the full
/// structure name list (indexed by `EorfId`, as elsewhere in this module); `char_matches_id`
/// answers "does this neighbor satisfy this pattern character?" exactly as in
/// `evaluate_autotile_rules` (facing is irrelevant here since anchor eligibility is name-level,
/// so an arbitrary facing is passed).
pub fn build_empty_anchor_index<R: AutotileResultKind>(
    all_rules: &[AutotileOriented<R>],
    names: &[String],
    char_matches_id: impl Fn(char, EorfId, Facing) -> bool,
) -> EmptyAnchorIndex<R> {
    let rules: Vec<AutotileOriented<R>> = all_rules
        .iter()
        .filter(|r| matches!(r.subject, RuleSubject::Empty))
        .cloned()
        .collect();

    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, rule) in rules.iter().enumerate() {
        let Some(case) = rule.cases.first() else {
            continue;
        };
        let Some(anchor_ch) = dispatch_anchor_char(case) else {
            continue;
        };
        for (i, name) in names.iter().enumerate() {
            let id = EorfId(i as u32);
            let accepted = match case.char_annotations.get(&anchor_ch) {
                Some(ann) => ann.name == *name,
                None => char_matches_id(anchor_ch, id, Facing::PosX),
            };
            if accepted {
                by_name.entry(name.clone()).or_default().push(idx);
            }
        }
    }

    EmptyAnchorIndex { rules, by_name }
}

/// Like `evaluate_autotile_rules`, but for `==empty:...==` rules: `loc`/`cell_name` describe the
/// *occupied* cell serving as the dispatch anchor (found via `index`), not `@` itself, so results
/// aren't necessarily recorded at `loc` — each returned pair is `(output location, result)`,
/// where the output location is `loc` shifted by the matched case's `output_offset`.
pub fn evaluate_empty_anchor_rules<R: AutotileResultKind>(
    loc: RelSlotCoord,
    cell_name: &str,
    index: &EmptyAnchorIndex<R>,
    get_cell: impl Fn(RelSlotCoord) -> Option<(EorfId, Facing)>,
    char_matches_id: impl Fn(char, EorfId, Facing) -> bool,
    name_matches_id: impl Fn(&str, EorfId) -> bool,
) -> Vec<(RelSlotCoord, R)> {
    let Some(rule_indices) = index.by_name.get(cell_name) else {
        return Vec::new();
    };
    rule_indices
        .iter()
        .flat_map(|&i| {
            let rule = &index.rules[i];
            match_pattern_cases(rule, &get_cell, loc, &char_matches_id, &name_matches_id)
                .into_iter()
                .map(|case| {
                    let output_loc = match case.output_offset {
                        Some(offset) => {
                            let (cx, cy, cz) = offset.cube_offset;
                            loc.apply_offset(crate::sparse3d::RelSlotCoordOffset {
                                origin_slot: offset.origin_slot.into(),
                                cube_offset: IVec3::new(cx, cy, cz),
                                dest_slot: offset.dest_slot.into(),
                            })
                        }
                        None => loc,
                    };
                    (output_loc, case.result.clone())
                })
        })
        .collect()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autotile::test_helpers::*;
    use crate::city::Cell;
    use crate::eorf::EorfId;
    use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, Sparse3D};
    use assert2::check;

    // Test-local ID convention: 0=wall, 1=floor, 2=stairs, 3=railing

    fn test_char_matches(ch: char, id: EorfId, _facing: Facing) -> bool {
        const NAMES: [&str; 4] = ["wall", "floor", "stairs", "railing"];
        char_matches_name(ch, NAMES[id.0 as usize])
    }

    fn no_name_match(_name: &str, _id: EorfId) -> bool {
        false
    }

    #[test]
    fn else_case_always_matches() {
        let input = "\
== wall: wall ==
--> none
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let grid: Sparse3D<Cell> = Sparse3D::new();
        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);
        let result = match_pattern(
            &oriented,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            test_char_matches,
            no_name_match,
        );
        check!(result == vec![&AutotiledMeshes::None]);
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
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);

        // Grid with a wall at (1, 0, 0) relative to anchor
        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall), wall_cell());
        let result = match_pattern(
            &oriented,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            test_char_matches,
            no_name_match,
        );

        check!(
            result
                == vec![&AutotiledMeshes::Mesh {
                    spec: atom("wall_across").rotate(180)
                }]
        );

        // Empty grid: should fall through to else
        let empty_grid: Sparse3D<Cell> = Sparse3D::new();
        let result2 = match_pattern(
            &oriented,
            |loc| empty_grid.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            test_char_matches,
            no_name_match,
        );
        check!(result2 == vec![&AutotiledMeshes::None]);
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
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);

        let anchor_x = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);
        let anchor_z = RelSlotCoord::new(0, 0, 0, RelSlot::ZLoWall);

        // Base case (cases): XLoWall anchor, neighbor at XLoWall(1,0,0).
        let mut grid_x: Sparse3D<Cell> = Sparse3D::new();
        grid_x.set(RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall), wall_cell());
        let r_x = match_pattern(
            &oriented,
            |loc| grid_x.get(loc).map(|c| (c.id, c.facing)),
            anchor_x,
            test_char_matches,
            no_name_match,
        )[0];
        check!(
            r_x == &AutotiledMeshes::Mesh {
                spec: atom("wall_across").rotate(180)
            }
        );

        // CW-rotated case (cases_plus_90): ZLoWall anchor.
        // sparse3d CW moves the XLoWall neighbor at (1,0,0) to ZLoWall at (0,0,1),
        // and the XLoWall anchor type becomes ZLoWall.
        let mut grid_z: Sparse3D<Cell> = Sparse3D::new();
        grid_z.set(RelSlotCoord::new(0, 0, 1, RelSlot::ZLoWall), wall_cell());
        let r_z = match_pattern(
            &oriented,
            |loc| grid_z.get(loc).map(|c| (c.id, c.facing)),
            anchor_z,
            test_char_matches,
            no_name_match,
        )[0];
        check!(
            r_z == &AutotiledMeshes::Mesh {
                spec: atom("wall_across")
            },
            "ZLoWall anchor with ZLoWall neighbor at (0,0,1): expected wall_across, got {r_z:?}"
        );

        // The 180°-rotated case: neighbor on the −Z side of the ZLoWall anchor.
        // That is the counterpart to an XLoWall anchor with neighbor at (−1,0,0).
        // It should also match wall_across, but via the rot=2 case (mesh rotation=2).
        let mut grid_z_back: Sparse3D<Cell> = Sparse3D::new();
        grid_z_back.set(RelSlotCoord::new(0, 0, -1, RelSlot::ZLoWall), wall_cell());
        let r_z_back = match_pattern(
            &oriented,
            |loc| grid_z_back.get(loc).map(|c| (c.id, c.facing)),
            anchor_z,
            test_char_matches,
            no_name_match,
        )[0];
        if let AutotiledMeshes::Mesh { spec, .. } = r_z_back {
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
== table: room ==
H:
 @ =
--> my_mesh
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);

        fn is_table(ch: char, id: EorfId, _facing: Facing) -> bool {
            ch == '=' && id.0 == 1
        }

        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::Room);

        // No neighbor: nothing matches (rule has no else case).
        let empty: Sparse3D<Cell> = Sparse3D::new();
        check!(match_pattern(
            &oriented,
            |loc| empty.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            is_table,
            no_name_match,
        )
        .is_empty());

        // rot=0: neighbor in +X direction → Room(1,0,0).
        // Due to the +2 rotation offset in compile_rule (needed to align with
        // wall_grid::cell_transform), this case produces mesh rotation 180°.
        let mut g0: Sparse3D<Cell> = Sparse3D::new();
        g0.set(RelSlotCoord::new(1, 0, 0, RelSlot::Room), table_cell());
        let r0 = match_pattern(
            &oriented,
            |loc| g0.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            is_table,
            no_name_match,
        )[0];
        check!(
            mesh_rotation(r0) == 180,
            "rot=0: expected mesh rotation 180, got {}",
            mesh_rotation(r0)
        );

        // Rotating the world CW (sparse3d Clockwise: (x,y,z)→(-z,y,x)) moves
        // the +X neighbor (1,0,0) to (0,0,1).  The autotile rot=1 (CW) case must
        // match this configuration.  With the +2 offset, it carries mesh rotation=270°.
        let mut g1: Sparse3D<Cell> = Sparse3D::new();
        g1.set(RelSlotCoord::new(0, 0, 1, RelSlot::Room), table_cell());
        let r1 = match_pattern(
            &oriented,
            |loc| g1.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            is_table,
            no_name_match,
        )[0];
        check!(
            mesh_rotation(r1) == 270,
            "rot=1 (CW): expected mesh rotation 270, got {}",
            mesh_rotation(r1)
        );

        // 180° (sparse3d OneEighty: (x,y,z)→(-x,y,-z)) moves (1,0,0) to (-1,0,0).
        // rot=2 with +2 offset → (2+2)*90=360°=0°.
        let mut g2: Sparse3D<Cell> = Sparse3D::new();
        g2.set(RelSlotCoord::new(-1, 0, 0, RelSlot::Room), table_cell());
        let r2 = match_pattern(
            &oriented,
            |loc| g2.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            is_table,
            no_name_match,
        )[0];
        check!(
            mesh_rotation(r2) == 0,
            "rot=2 (180°): expected mesh rotation 0, got {}",
            mesh_rotation(r2)
        );

        // CCW / 270° CW (sparse3d CounterClockwise: (x,y,z)→(z,y,-x)) moves
        // (1,0,0) to (0,0,-1).  rot=3 with +2 offset → (3+2)*90=450°=90°.
        let mut g3: Sparse3D<Cell> = Sparse3D::new();
        g3.set(RelSlotCoord::new(0, 0, -1, RelSlot::Room), table_cell());
        let r3 = match_pattern(
            &oriented,
            |loc| g3.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            is_table,
            no_name_match,
        )[0];
        check!(
            mesh_rotation(r3) == 90,
            "rot=3 (CCW): expected mesh rotation 90, got {}",
            mesh_rotation(r3)
        );
    }

    // ── `==empty:...==` dispatch ──────────────────────────────────────────────

    /// An empty-anchored rule fires when its dispatch-anchor structure ('W' here) is present and
    /// `@`'s own position is actually empty, recording the result at `@`'s offset from the
    /// anchor rather than at the anchor's own location. It must NOT fire when something occupies
    /// `@`'s position -- the injected `.` check should block it.
    #[test]
    fn empty_anchor_rule_dispatches_via_neighbor_structure() {
        let input = "\
== empty: wall ==
H:
 @ W
--> hit
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let names = vec![
            "wall".to_string(),
            "floor".to_string(),
            "stairs".to_string(),
            "railing".to_string(),
        ];
        let index =
            build_empty_anchor_index(std::slice::from_ref(&oriented), &names, test_char_matches);

        // A wall at XLoWall(0,0,0) serves as the dispatch anchor; '@' (and thus the output) is at
        // XLoWall(-1,0,0), which must be empty for the rule to fire.
        let anchor_loc = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);
        let output_loc = RelSlotCoord::new(-1, 0, 0, RelSlot::XLoWall);
        // A single wall is symmetric under 180°, so both the west ('@ W') and mirrored east
        // ('W @') readings of the pattern would match the same wall at once on an otherwise-empty
        // grid; a second wall east of the anchor blocks the east reading's own "`@` is empty"
        // check, leaving only the west orientation -- disambiguating without weakening what this
        // test is actually about (west-side dispatch and its '.' blocking check).
        let east_blocker_loc = RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall);

        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(anchor_loc, wall_cell());
        grid.set(east_blocker_loc, wall_cell());
        let results = evaluate_empty_anchor_rules(
            anchor_loc,
            "wall",
            &index,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            test_char_matches,
            no_name_match,
        );
        check!(results.len() == 1, "expected one match; got {results:?}");
        check!(results[0].0 == output_loc);
        check!(matches!(results[0].1, AutotiledMeshes::Mesh { .. }));

        // If the output position is occupied too, the injected '.' check must block that specific
        // (west) orientation from matching -- and the east reading stays blocked by its own
        // blocker wall, so no orientation should match at all.
        let mut grid_blocked = grid.clone();
        grid_blocked.set(output_loc, wall_cell());
        let blocked = evaluate_empty_anchor_rules(
            anchor_loc,
            "wall",
            &index,
            |loc| grid_blocked.get(loc).map(|c| (c.id, c.facing)),
            test_char_matches,
            no_name_match,
        );
        check!(
            blocked.is_empty(),
            "both orientations should be blocked once their '@' positions are occupied; got {blocked:?}"
        );

        // No wall present at all: the index shouldn't even be consulted for an unrelated name.
        let none_results = evaluate_empty_anchor_rules(
            anchor_loc,
            "floor",
            &index,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            test_char_matches,
            no_name_match,
        );
        check!(none_results.is_empty());
    }

    /// Regression test: an empty-anchored rule's `dispatch_anchor` can land on the "other" wall
    /// slot within an H tile (`ZLoWall`, e.g. a room's north wall) while `@` sits on the
    /// canonical `XLoWall` slot (e.g. the same room's east wall). `compile_rule` used to assume
    /// every wall-family anchor started out `XLoWall`-canonical (guaranteed for named rules, since
    /// `offset()` pads `@` there, but never enforced for `dispatch_anchor`), which crashed
    /// `RelSlotCoord::apply_offset` with a "slot type mismatch" panic once a real Z-wall anchor
    /// was matched against.
    #[test]
    fn empty_anchor_landing_on_the_other_wall_slot_does_not_panic() {
        // `(multi)`: on an otherwise-empty grid, this diagonal pattern's sole non-dispatch
        // constraint (the injected "`@` is empty" check) is satisfied by more than one of its
        // compiled orientations at once -- harmless here since the test only cares that matching
        // completes without panicking, not which single orientation "wins".
        let input = "\
== empty: wall ==
H:
 W
  @
--> (multi) hit
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        // Confirm the reproduction: '@' canonically lands on XLoWall, but the dispatch anchor
        // ('W') is on the OTHER wall slot of the tile (ZLoWall).
        check!(pat.anchor_origin_slot() == AutotileRelSlot::ZLoWall);

        let oriented = compile_rule(&file.rules[0]);
        let names = vec!["wall".to_string()];
        let index =
            build_empty_anchor_index(std::slice::from_ref(&oriented), &names, test_char_matches);

        // A wall placed at a Z-oriented slot must be found via `cases_plus_90` without panicking.
        let anchor_loc_z = RelSlotCoord::new(0, 0, 0, RelSlot::ZLoWall);
        let mut grid_z: Sparse3D<Cell> = Sparse3D::new();
        grid_z.set(anchor_loc_z, wall_cell());
        let results_z = evaluate_empty_anchor_rules(
            anchor_loc_z,
            "wall",
            &index,
            |loc| grid_z.get(loc).map(|c| (c.id, c.facing)),
            test_char_matches,
            no_name_match,
        );
        check!(
            !results_z.is_empty(),
            "expected the Z-wall anchor to match; got none"
        );

        // The same rule must still work when the anchor is found as an X-oriented wall instead
        // (via `cases`).
        let anchor_loc_x = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);
        let mut grid_x: Sparse3D<Cell> = Sparse3D::new();
        grid_x.set(anchor_loc_x, wall_cell());
        let results_x = evaluate_empty_anchor_rules(
            anchor_loc_x,
            "wall",
            &index,
            |loc| grid_x.get(loc).map(|c| (c.id, c.facing)),
            test_char_matches,
            no_name_match,
        );
        check!(
            !results_x.is_empty(),
            "expected the X-wall anchor to match; got none"
        );
    }

    // ── (multi) tests ─────────────────────────────────────────────────────────

    fn is_table(ch: char, id: EorfId, _facing: Facing) -> bool {
        ch == '=' && id.0 == 1
    }

    /// A `(multi)` case with neighbours on two sides (+X and −X) matches in two orientations,
    /// and both meshes are emitted (mesh rotations 0 and 180 — see `rotation_consistent`).
    #[test]
    fn multi_emits_all_matching_orientations() {
        let input = "\
== table: room ==
H:
 @ =
--> (multi) my_mesh
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::Room);

        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(RelSlotCoord::new(1, 0, 0, RelSlot::Room), table_cell());
        grid.set(RelSlotCoord::new(-1, 0, 0, RelSlot::Room), table_cell());

        let results = match_pattern(
            &oriented,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            is_table,
            no_name_match,
        );
        check!(
            mesh_rotations(&results) == vec![0, 180],
            "expected both orientations; got {results:?}"
        );
    }

    /// Without `(multi)`, a genuinely ambiguous two-sided configuration (both orientations of a
    /// symmetric pattern matching at once) is a hard error rather than a silent, rotation-order-
    /// dependent pick of "the first one" -- that silent pick isn't rotation-covariant (see
    /// `motif_count_is_rotation_invariant`'s bug write-up), so it's better caught here. Mark the
    /// case `(multi)` if every matching orientation should legitimately fire.
    #[test]
    #[should_panic(expected = "matched 2 orientations")]
    fn non_multi_ambiguous_match_panics() {
        let input = "\
== table: room ==
H:
 @ =
--> my_mesh
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::Room);

        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(RelSlotCoord::new(1, 0, 0, RelSlot::Room), table_cell());
        grid.set(RelSlotCoord::new(-1, 0, 0, RelSlot::Room), table_cell());

        match_pattern(
            &oriented,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            is_table,
            no_name_match,
        );
    }

    /// A `(multi)` match stops later cases from contributing: the else case is skipped once the
    /// multi group matches.
    #[test]
    fn multi_skips_later_cases() {
        let input = "\
== table: room ==
H:
 @ =
--> (multi) my_mesh
--> fallback
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::Room);

        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(RelSlotCoord::new(1, 0, 0, RelSlot::Room), table_cell());

        let results = match_pattern(
            &oriented,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            is_table,
            no_name_match,
        );
        // One matching orientation, and the `fallback` else case is not appended.
        check!(
            results.len() == 1,
            "expected only the multi result; got {results:?}"
        );
        check!(
            matches!(results[0], AutotiledMeshes::Mesh { spec } if spec_stem(spec, UnorientedSlot::Room) == "my_mesh"),
            "expected my_mesh, got {:?}",
            results[0]
        );
    }

    // ── Column autotile tests ─────────────────────────────────────────────────

    fn all_rules() -> Vec<AutotileOriented<AutotiledMeshes>> {
        let src = include_str!("../../buildables/structures.autotile");
        compile(&parse(src).unwrap())
    }

    fn stems_from_results(results: &[AutotiledMeshes]) -> Vec<String> {
        results
            .iter()
            .filter_map(|r| {
                if let AutotiledMeshes::Mesh { spec, .. } = r {
                    Some(spec_stem(spec, UnorientedSlot::Wall))
                } else {
                    None
                }
            })
            .collect()
    }

    // IDs used in column tests: 0 = floor, 1 = column
    fn col_char_matches(ch: char, id: EorfId, _facing: Facing) -> bool {
        const NAMES: [&str; 2] = ["floor", "column"];
        match ch {
            '=' => id.0 == 1,
            other => char_matches_name(other, NAMES[id.0 as usize]),
        }
    }

    /// A lone column with no neighbours falls through to the else case in both
    /// stanzas and produces two `column_floating` meshes.
    #[test]
    fn column_single_produces_two_floating() {
        let rules = all_rules();
        let anchor = RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall);
        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(anchor, col_cell());

        let results = evaluate_autotile_rules(
            anchor,
            "column",
            &rules,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            col_char_matches,
            no_name_match,
        )
        .expect("column has autotile rules");
        let stems = stems_from_results(&results);

        check!(stems.len() == 2, "expected 2 stems, got {stems:?}");
        check!(
            stems.contains(&"isect__column_floating__u_top".to_string()),
            "stems={stems:?}"
        );
        check!(
            stems.contains(&"isect__column_floating__u_bottom".to_string()),
            "stems={stems:?}"
        );
    }

    /// Z-wall variant of `column_single_produces_two_floating`: the else case must
    /// appear in `cases_plus_90` so that ZLoWall anchors also get the fallback mesh.
    #[test]
    fn column_single_zwall_produces_two_floating() {
        let rules = all_rules();
        let anchor = RelSlotCoord::new(1, 0, 0, RelSlot::ZLoWall);
        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(anchor, col_cell());

        let results = evaluate_autotile_rules(
            anchor,
            "column",
            &rules,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            col_char_matches,
            no_name_match,
        )
        .expect("column has autotile rules");
        let stems = stems_from_results(&results);

        check!(
            stems.len() == 2,
            "expected 2 stems for ZLoWall column, got {stems:?}"
        );
        check!(
            stems.contains(&"isect__column_floating__u_top".to_string()),
            "stems={stems:?}"
        );
        check!(
            stems.contains(&"isect__column_floating__u_bottom".to_string()),
            "stems={stems:?}"
        );
    }

    /// Two columns stacked vertically with no floors: the outer ends get
    /// `column_floating` (no neighbour beyond them) and the joint gets
    /// `column_middle` on both sides.
    #[test]
    fn column_stacked_produces_four_meshes() {
        let rules = all_rules();
        let anchor_lo = RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall);
        let anchor_hi = RelSlotCoord::new(1, 1, 0, RelSlot::XLoWall);
        let mut grid: Sparse3D<Cell> = Sparse3D::new();
        grid.set(anchor_lo, col_cell());
        grid.set(anchor_hi, col_cell());

        let results_lo = evaluate_autotile_rules(
            anchor_lo,
            "column",
            &rules,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            col_char_matches,
            no_name_match,
        )
        .expect("column has autotile rules");
        let results_hi = evaluate_autotile_rules(
            anchor_hi,
            "column",
            &rules,
            |loc| grid.get(loc).map(|c| (c.id, c.facing)),
            col_char_matches,
            no_name_match,
        )
        .expect("column has autotile rules");

        let stems_lo = stems_from_results(&results_lo);
        let stems_hi = stems_from_results(&results_hi);
        let all: Vec<_> = stems_lo.iter().chain(stems_hi.iter()).cloned().collect();

        check!(all.len() == 4, "expected 4 stems total, got {all:?}");
        for expected in &[
            "isect__column_floating__u_top", // bottom column, stanza 1: no column below
            "isect__column_middle__u_bottom", // bottom column, stanza 2: column above present
            "isect__column_middle__u_top",   // top column,    stanza 1: column below present
            "isect__column_floating__u_bottom", // top column,    stanza 2: no column above
        ] {
            check!(
                all.contains(&expected.to_string()),
                "missing {expected}; got {all:?}"
            );
        }
    }

    // ── 'r' condition tests ───────────────────────────────────────────────────

    /// `'r'` in a wall position matches when there's a wall there, or when there's a roof
    /// (now a Floor-slot structure) on the FAR side of that wall from the anchor. A roof in
    /// the near cube (between the anchor and the wall) must NOT match.
    #[test]
    fn r_condition_matches_wall_or_roof() {
        // Pattern `r @`: 'r' is one wall-slot to the LEFT of '@'.
        // After parse: '@' at XLoWall cube (2,0,0), 'r' at XLoWall cube (1,0,0).
        // Relative to anchor: wall_offset = XLoWall at cube_offset (-1,0,0).
        //
        // XLoWall(-1) sits between cube(-2) and cube(-1).
        // Anchor XLoWall(0) sits between cube(-1) and cube(0).
        // → cube(-1) is the NEAR cube (shared with anchor), cube(-2) is the FAR cube.
        let input = "\
== wall: wall ==
H:
 r @
--> hit
--> none
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);
        let wall_loc = RelSlotCoord::new(-1, 0, 0, RelSlot::XLoWall);
        let far_floor = RelSlotCoord::new(-2, 0, 0, RelSlot::Floor);
        let near_floor = RelSlotCoord::new(-1, 0, 0, RelSlot::Floor);

        // EorfId 0 = wall, 1 = roof (local convention)
        fn char_matches(ch: char, id: EorfId, _: Facing) -> bool {
            const NAMES: [&str; 2] = ["wall", "roof"];
            char_matches_name(ch, NAMES[id.0 as usize])
        }

        // Case 1: wall present at wall_loc → should match 'hit'
        let mut grid = Sparse3D::new();
        grid.set(wall_loc, make_cell(0));
        let result = match_pattern(
            &oriented,
            |l| grid.get(l).map(|c| (c.id, c.facing)),
            anchor,
            char_matches,
            no_name_match,
        );
        check!(
            matches!(result[0], AutotiledMeshes::Mesh { .. }),
            "wall present should match; got {result:?}"
        );

        // Case 2: roof in far cube's floor → should match 'hit'
        let mut grid2 = Sparse3D::new();
        grid2.set(far_floor, make_cell(1));
        let result2 = match_pattern(
            &oriented,
            |l| grid2.get(l).map(|c| (c.id, c.facing)),
            anchor,
            char_matches,
            no_name_match,
        );
        check!(
            matches!(result2[0], AutotiledMeshes::Mesh { .. }),
            "roof in far floor should match; got {result2:?}"
        );

        // Case 3: roof in NEAR floor only → should NOT satisfy 'r', falls through to 'none'
        let mut grid3 = Sparse3D::new();
        grid3.set(near_floor, make_cell(1));
        let result3 = match_pattern(
            &oriented,
            |l| grid3.get(l).map(|c| (c.id, c.facing)),
            anchor,
            char_matches,
            no_name_match,
        );
        check!(
            result3 == vec![&AutotiledMeshes::None],
            "roof in near floor should not match; got {result3:?}"
        );

        // Case 4: neither wall nor roof → should fall through to 'none'
        let result4 = match_pattern(
            &oriented,
            |l| Sparse3D::<Cell>::new().get(l).map(|c| (c.id, c.facing)),
            anchor,
            char_matches,
            no_name_match,
        );
        check!(
            result4 == vec![&AutotiledMeshes::None],
            "neither wall nor roof should fall through; got {result4:?}"
        );
    }

    /// The roof-rule form of `'r'`: an `H narrow` floor anchor with `'r'` in one of its (new)
    /// wall slots. It matches when that wall exists, or when a roof occupies the floor beyond
    /// that wall (at the same level as the anchor floor).
    #[test]
    fn r_condition_h_narrow_floor_anchor() {
        // Pattern `r@`: 'r' is the −X wall of the anchor's cube. After parse the anchor floor is
        // cube (1,0,0) and 'r' is XLoWall(1,0,0); evaluated at anchor (0,0,0,Floor) the wall sits
        // at XLoWall(0,0,0) and the floor beyond it at Floor(-1,0,0).
        let input = "\
== roof: floor ==
H narrow:
 r@
--> hit
--> none
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::Floor);
        let wall_loc = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);
        let far_floor = RelSlotCoord::new(-1, 0, 0, RelSlot::Floor);

        // EorfId 0 = wall, 1 = roof (local convention)
        fn char_matches(ch: char, id: EorfId, _: Facing) -> bool {
            const NAMES: [&str; 2] = ["wall", "roof"];
            char_matches_name(ch, NAMES[id.0 as usize])
        }

        // Wall present beyond the anchor → match.
        let mut grid = Sparse3D::new();
        grid.set(wall_loc, make_cell(0));
        let result = match_pattern(
            &oriented,
            |l| grid.get(l).map(|c| (c.id, c.facing)),
            anchor,
            char_matches,
            no_name_match,
        );
        check!(
            matches!(result[0], AutotiledMeshes::Mesh { .. }),
            "wall present should match; got {result:?}"
        );

        // Roof on the floor beyond the wall (same level) → match.
        let mut grid2 = Sparse3D::new();
        grid2.set(far_floor, make_cell(1));
        let result2 = match_pattern(
            &oriented,
            |l| grid2.get(l).map(|c| (c.id, c.facing)),
            anchor,
            char_matches,
            no_name_match,
        );
        check!(
            matches!(result2[0], AutotiledMeshes::Mesh { .. }),
            "roof on far floor should match; got {result2:?}"
        );

        // Neither → fall through to 'none'.
        let result3 = match_pattern(
            &oriented,
            |l| Sparse3D::<Cell>::new().get(l).map(|c| (c.id, c.facing)),
            anchor,
            char_matches,
            no_name_match,
        );
        check!(
            result3 == vec![&AutotiledMeshes::None],
            "neither wall nor roof should fall through; got {result3:?}"
        );
    }

    // ── Annotation orientation tests ──────────────────────────────────────────

    /// `1=stairs:90` fires only when the stairs in the adjacent Room face PosZ (90°).
    /// A stairs facing any other direction falls through to the else case.
    #[test]
    fn annotation_orientation_check() {
        // In the H: pattern, @1 with a wall rule puts @ at XLoWall(1,0,0) and
        // 1 at Room(1,0,0) — the room in the same cube as the anchor wall.
        // `1=stairs:90` means "stairs" name + Facing::PosZ (0°=PosX, 90°=PosZ CW).
        let input = "\
== railing: wall ==
H: 1=stairs:90
 @1
--> stair_railing:90
--> none
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);

        let anchor = RelSlotCoord::new(0, 0, 0, RelSlot::XLoWall);
        // The '1' position: Room at the same cube as the XLoWall anchor.
        let room_loc = RelSlotCoord::new(0, 0, 0, RelSlot::Room);

        // ID 2 = stairs in this test
        fn name_is_stairs(name: &str, id: EorfId) -> bool {
            name == "stairs" && id.0 == 2
        }

        // stairs facing PosZ (= 90° from PosX) → should match stair_railing
        let mut grid_match = Sparse3D::new();
        grid_match.set(room_loc, stairs_cell(Facing::PosZ));
        let result = match_pattern(
            &oriented,
            |loc| grid_match.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            |_ch, _id, _facing| false,
            name_is_stairs,
        );
        check!(
            matches!(result[0], AutotiledMeshes::Mesh { .. }),
            "stairs facing PosZ should match; got {result:?}"
        );

        // stairs facing NegX → should fall through to else (none)
        let mut grid_miss = Sparse3D::new();
        grid_miss.set(room_loc, stairs_cell(Facing::NegX));
        let result2 = match_pattern(
            &oriented,
            |loc| grid_miss.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            |_ch, _id, _facing| false,
            name_is_stairs,
        );
        check!(
            result2 == vec![&AutotiledMeshes::None],
            "stairs facing NegX should not match annotation; got {result2:?}"
        );

        // no structure at room position → should fall through to else (none)
        let empty_grid: Sparse3D<Cell> = Sparse3D::new();
        let result3 = match_pattern(
            &oriented,
            |loc| empty_grid.get(loc).map(|c| (c.id, c.facing)),
            anchor,
            |_ch, _id, _facing| false,
            name_is_stairs,
        );
        check!(
            result3 == vec![&AutotiledMeshes::None],
            "empty room should not match annotation; got {result3:?}"
        );
    }

    /// Multiple labeled matchers in one rule (`1=stairs:90 2=stairs`) — verify
    /// both are stored and the orientation-less matcher accepts any facing.
    #[test]
    fn annotation_multiple_labels() {
        // Two annotations: '1' requires stairs facing PosZ; '2' requires stairs at any facing.
        // Pattern: @12 (three adjacent room slots after the anchor wall).
        // We put '1' and '2' at separate room slots in the same row.
        let input = "\
== railing: wall ==
H: 1=stairs:90 2=stairs
 @1
--> stair_railing:90
--> none
";
        let file = parse::<AutotiledMeshes>(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();

        check!(pat.annotations.len() == 2);

        let ann1 = pat.annotations.get(&'1').unwrap();
        check!(ann1.name == "stairs");
        check!(ann1.orientation == Some(90));

        let ann2 = pat.annotations.get(&'2').unwrap();
        check!(ann2.name == "stairs");
        check!(ann2.orientation == None);
    }

    // ── Motif count invariance under rotation ─────────────────────────────────

    /// Runs every rule in `rules` over every occupied cell of `grid` and collapses the results,
    /// mirroring `qnn::translate::visible_motifs_and_defects` but without its vantage-based
    /// visibility filtering (we want every motif in the structure, not just the visible ones).
    fn all_motifs(
        grid: &Sparse3D<Cell>,
        rules: &[AutotileOriented<Motif>],
        names: &[String],
    ) -> (
        Vec<crate::autotile::MotifOccurrence>,
        Vec<crate::autotile::DefectAtom>,
    ) {
        use crate::autotile::{collapse_motif_atoms, MotifAtom};
        use crate::sparse3d::SlotCoord;

        let empty_index = build_empty_anchor_index(rules, names, |ch, id, _facing| {
            char_matches_name(ch, &names[id.as_usize()])
        });

        let char_matches = |ch: char, id: EorfId, _facing: Facing, anchor_name: &str| match ch {
            '=' => names[id.as_usize()] == anchor_name,
            other => char_matches_name(other, &names[id.as_usize()]),
        };

        let mut atoms = Vec::new();
        for (loc, cell) in grid.iter() {
            let anchor_name = names[cell.id.as_usize()].clone();
            let rel_loc: RelSlotCoord = loc.into();

            if let Some(results) = evaluate_autotile_rules(
                rel_loc,
                &anchor_name,
                rules,
                |l| grid.get(l).map(|c| (c.id, c.facing)),
                |ch, id, facing| char_matches(ch, id, facing, &anchor_name),
                |name, id| names[id.as_usize()] == name,
            ) {
                for motif in results {
                    if matches!(motif, Motif::Discard) {
                        continue;
                    }
                    atoms.push(MotifAtom { motif, loc });
                }
            }

            for (out_loc, motif) in evaluate_empty_anchor_rules(
                rel_loc,
                &anchor_name,
                &empty_index,
                |l| grid.get(l).map(|c| (c.id, c.facing)),
                |ch, id, facing| char_matches(ch, id, facing, &anchor_name),
                |name, id| names[id.as_usize()] == name,
            ) {
                if matches!(motif, Motif::Discard) {
                    continue;
                }
                atoms.push(MotifAtom {
                    motif,
                    loc: SlotCoord::from(out_loc),
                });
            }
        }

        collapse_motif_atoms(&atoms)
    }

    /// `layered_offices` generates a certain number of motifs; the same structure rotated 90°
    /// should generate the same number, since rotation shouldn't change what's structurally
    /// present -- only its orientation in space.
    #[test]
    fn motif_count_is_rotation_invariant() {
        let structures = crate::eorf::load_structure_info();
        let names: Vec<String> = structures.iter().map(|s| s.name.clone()).collect();

        let src = include_str!("../../buildables/motifs.autotile");
        let file = parse::<Motif>(src).expect("motifs.autotile should parse");
        let rules = compile(&file);

        let grid = crate::serialization::load_from_str(
            include_str!("../../assets/static/training/layered_offices.txt"),
            &structures,
        )
        .expect("layered_offices.txt should load");
        let rotated_grid = {
            use crate::sparse3d::Rotateable;
            grid.clone().rotate(crate::sparse3d::Rotation::Clockwise)
        };

        let (occurrences, defects) = all_motifs(&grid, &rules, &names);
        let (rotated_occurrences, rotated_defects) = all_motifs(&rotated_grid, &rules, &names);

        let count = occurrences.len() + defects.len();
        let rotated_count = rotated_occurrences.len() + rotated_defects.len();
        check!(
            count == rotated_count,
            "expected the same motif count under rotation; got {count} (occurrences={}, defects={}) \
             vs {rotated_count} (occurrences={}, defects={})",
            occurrences.len(),
            defects.len(),
            rotated_occurrences.len(),
            rotated_defects.len()
        );
    }
}
