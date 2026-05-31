use std::collections::HashMap;

// These imports are only valid in the main crate (not the build script).
// build.rs sets `cargo:rustc-cfg=autotile_matching` so the main crate sees them.
#[cfg(autotile_matching)]
use crate::sparse3d::{SlotLocation, Sparse3D};
#[cfg(autotile_matching)]
use crate::structure::StructureId;
#[cfg(autotile_matching)]
use crate::wall_grid::Cell;
#[cfg(autotile_matching)]
use bevy::math::IVec3;

// ─── Core types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnorientedSlot {
    Wall,
    Room,
    Floor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternType {
    H,
    VWide,
    VNarrow,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MeshSpec {
    Atom { name: String, rotation: i32 },
    Union(Box<MeshSpec>, Box<MeshSpec>),
    Intersection(Box<MeshSpec>, Box<MeshSpec>),
}

impl MeshSpec {
    pub fn rotate(self, by: i32) -> Self {
        match self {
            MeshSpec::Atom { name, rotation } => MeshSpec::Atom {
                name,
                rotation: (rotation + by).rem_euclid(4),
            },
            MeshSpec::Union(a, b) => {
                MeshSpec::Union(Box::new(a.rotate(by)), Box::new(b.rotate(by)))
            }
            MeshSpec::Intersection(a, b) => {
                MeshSpec::Intersection(Box::new(a.rotate(by)), Box::new(b.rotate(by)))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutotileResult {
    None,
    Mesh { multi: bool, spec: MeshSpec },
}

// ─── Parsed representation ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Pattern {
    pub pattern_type: PatternType,
    pub rows: Vec<Vec<char>>,
    pub at_col: usize,
    pub at_row: usize,
}

impl Pattern {
    /// Returns non-wildcard, non-@ cells as relative (dcol, drow) → char.
    pub fn relative_checks(&self) -> HashMap<(i32, i32), char> {
        let mut map = HashMap::new();
        for (r, row) in self.rows.iter().enumerate() {
            for (c, &ch) in row.iter().enumerate() {
                if ch == ' ' || ch == '@' {
                    continue;
                }
                let dc = c as i32 - self.at_col as i32;
                let dr = r as i32 - self.at_row as i32;
                map.insert((dc, dr), ch);
            }
        }
        map
    }
}

#[derive(Debug, Clone)]
pub struct PatternCase {
    pub pattern: Option<Pattern>,
    pub result: AutotileResult,
}

#[derive(Debug, Clone)]
pub struct AutotileRule {
    pub structure_name: String,
    pub slot: UnorientedSlot,
    pub cases: Vec<PatternCase>,
}

#[derive(Debug, Clone)]
pub struct AutotileFile {
    pub rules: Vec<AutotileRule>,
}

// ─── Compiled / oriented representation ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OrientedCase {
    pub pattern_type: PatternType,
    /// Non-wildcard checks relative to @ (the "anchor"). Empty = else/fallback case.
    pub checks: HashMap<(i32, i32), char>,
    pub result: AutotileResult,
    /// 0=0°, 1=90° CW, 2=180°, 3=270° CW
    pub rotation: u8,
}

#[derive(Debug, Clone)]
pub struct AutotileOriented {
    pub structure_name: String,
    pub slot: UnorientedSlot,
    /// Cases in priority order; else case (empty checks) is last.
    pub cases: Vec<OrientedCase>,
}

// ─── Rotation helpers ─────────────────────────────────────────────────────────

/// Rotate (dcol, drow) by `rot` 90°-CW steps in the XZ plane.
/// 90° CW: (x, z) → (z, -x), so (dc, dr) → (dr, -dc).
pub fn rotate_offset(dc: i32, dr: i32, rot: u8) -> (i32, i32) {
    match rot % 4 {
        0 => (dc, dr),
        1 => (dr, -dc),
        2 => (-dc, -dr),
        3 => (-dr, dc),
        _ => unreachable!(),
    }
}

fn rotate_checks(checks: &HashMap<(i32, i32), char>, rot: u8) -> HashMap<(i32, i32), char> {
    checks
        .iter()
        .map(|(&(dc, dr), &ch)| (rotate_offset(dc, dr, rot), ch))
        .collect()
}

fn rotate_result(result: &AutotileResult, rot: u8) -> AutotileResult {
    match result {
        AutotileResult::None => AutotileResult::None,
        AutotileResult::Mesh { multi, spec } => AutotileResult::Mesh {
            multi: *multi,
            spec: spec.clone().rotate(rot as i32),
        },
    }
}

// ─── Compiler ────────────────────────────────────────────────────────────────

pub fn compile(file: &AutotileFile) -> Vec<AutotileOriented> {
    file.rules.iter().map(compile_rule).collect()
}

pub fn compile_rule(rule: &AutotileRule) -> AutotileOriented {
    let mut cases = Vec::new();

    for case in &rule.cases {
        match &case.pattern {
            None => {
                // Else case: no constraints, matches always. One copy, rotation 0.
                cases.push(OrientedCase {
                    pattern_type: PatternType::H,
                    checks: HashMap::new(),
                    result: case.result.clone(),
                    rotation: 0,
                });
            }
            Some(pattern) => {
                let base_checks = pattern.relative_checks();
                let pt = pattern.pattern_type.clone();

                let mut seen: Vec<HashMap<(i32, i32), char>> = Vec::new();
                for rot in 0u8..4 {
                    let rotated = rotate_checks(&base_checks, rot);
                    if !seen.iter().any(|s| s == &rotated) {
                        seen.push(rotated.clone());
                        cases.push(OrientedCase {
                            pattern_type: pt.clone(),
                            checks: rotated,
                            result: rotate_result(&case.result, rot),
                            rotation: rot,
                        });
                    }
                }
            }
        }
    }

    AutotileOriented {
        structure_name: rule.structure_name.clone(),
        slot: rule.slot.clone(),
        cases,
    }
}

// ─── Matching ─────────────────────────────────────────────────────────────────

/// Returns the result for the first matching oriented case.
///
/// `char_matches_id` answers "does this neighbor's StructureId satisfy this pattern character?"
/// for any character other than `' '` (wildcard, always true) and `'.'` (empty slot).
/// This does not match the anchor itself! It's expected that we will look at every structure, see
/// which rules use that structure as the anchor, and then call `match_pattern` on them.
#[cfg(autotile_matching)]
pub fn match_pattern<'a>(
    oriented: &'a AutotileOriented,
    grid: &Sparse3D<Cell>,
    anchor: SlotLocation,
    char_matches_id: impl Fn(char, StructureId) -> bool,
) -> Option<&'a AutotileResult> {
    for case in &oriented.cases {
        let matches = case.checks.iter().all(|(&(dc, dr), &ch)| {
            let dxyz = match case.pattern_type {
                PatternType::H => IVec3::new(dc, 0, dr),
                PatternType::VWide | PatternType::VNarrow => IVec3::new(dc, dr, 0),
            };
            let neighbor = SlotLocation {
                cube: anchor.cube + dxyz,
                rel_slot: anchor.rel_slot, // This is wrong! Slot type should vary per check!
            };
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

// ─── Parser ───────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum ParseError {
    UnexpectedLine(String),
    MissingAt,
    MultipleAt,
    InvalidSlot(String),
    InvalidResult(String),
    DeadSlotUsed { col: usize, row: usize, ch: char },
    UnterminatedPattern,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnexpectedLine(s) => write!(f, "Unexpected line: {s:?}"),
            ParseError::MissingAt => write!(f, "Pattern has no @ character"),
            ParseError::MultipleAt => write!(f, "Pattern has multiple @ characters"),
            ParseError::InvalidSlot(s) => write!(f, "Invalid slot: {s:?}"),
            ParseError::InvalidResult(s) => write!(f, "Invalid result: {s:?}"),
            ParseError::DeadSlotUsed { col, row, ch } => {
                write!(f, "Character {ch:?} in dead slot at ({col},{row})")
            }
            ParseError::UnterminatedPattern => write!(f, "Pattern with no --> result"),
        }
    }
}

fn strip_comment(line: &str) -> &str {
    if let Some(idx) = line.find('#') {
        line[..idx].trim_end()
    } else {
        line.trim_end()
    }
}

pub fn parse(input: &str) -> Result<AutotileFile, ParseError> {
    let lines: Vec<&str> = input.lines().collect();
    let mut i = 0;
    let mut rules = Vec::new();

    while i < lines.len() {
        let line = strip_comment(lines[i]);
        if line.is_empty() {
            i += 1;
            continue;
        }
        if let Some(inner) = line.strip_prefix("==").and_then(|s| s.strip_suffix("==")) {
            let inner = inner.trim();
            let colon = inner
                .rfind(':')
                .ok_or_else(|| ParseError::UnexpectedLine(line.to_owned()))?;
            let structure_name = inner[..colon].trim().to_owned();
            let slot = match inner[colon + 1..].trim() {
                "wall" => UnorientedSlot::Wall,
                "room" => UnorientedSlot::Room,
                other => return Err(ParseError::InvalidSlot(other.to_owned())),
            };
            i += 1;
            let (cases, new_i) = parse_cases(&lines, i, slot)?;
            i = new_i;
            rules.push(AutotileRule {
                structure_name,
                slot,
                cases,
            });
        } else {
            return Err(ParseError::UnexpectedLine(line.to_owned()));
        }
    }

    Ok(AutotileFile { rules })
}

fn parse_cases(
    lines: &[&str],
    mut i: usize,
    slot: UnorientedSlot,
) -> Result<(Vec<PatternCase>, usize), ParseError> {
    let mut cases = Vec::new();

    loop {
        while i < lines.len() && strip_comment(lines[i]).is_empty() {
            i += 1;
        }
        if i >= lines.len() {
            break;
        }
        let line = strip_comment(lines[i]);

        if line.starts_with("==") {
            break;
        }

        if let Some(rest) = line.strip_prefix("-->") {
            let result = parse_result(rest.trim())?;
            cases.push(PatternCase {
                pattern: None,
                result,
            });
            i += 1;
            break; // else case is always last
        }

        let pt = match line {
            "H:" => PatternType::H,
            "V wide:" => PatternType::VWide,
            "V narrow:" => PatternType::VNarrow,
            _ => return Err(ParseError::UnexpectedLine(line.to_owned())),
        };
        i += 1;

        let mut pattern_rows: Vec<Vec<char>> = Vec::new();
        loop {
            if i >= lines.len() {
                return Err(ParseError::UnterminatedPattern);
            }
            let pline = strip_comment(lines[i]);
            if let Some(rest) = pline.strip_prefix("-->") {
                let pattern = build_pattern(pt, pattern_rows, slot)?;
                let result = parse_result(rest.trim())?;
                cases.push(PatternCase {
                    pattern: Some(pattern),
                    result,
                });
                i += 1;
                break;
            }
            if pline.starts_with("==") {
                return Err(ParseError::UnterminatedPattern);
            }
            if pline.is_empty() {
                i += 1;
                continue;
            }
            // Strip the mandatory leading space
            let content: Vec<char> = pline.strip_prefix(' ').unwrap_or(pline).chars().collect();
            pattern_rows.push(content);
            i += 1;
        }
    }

    Ok((cases, i))
}

fn build_pattern(
    pt: PatternType,
    rows: Vec<Vec<char>>,
    slot: UnorientedSlot,
) -> Result<Pattern, ParseError> {
    let mut at_col = None;
    let mut at_row = None;

    for (r, row) in rows.iter().enumerate() {
        for (c, &ch) in row.iter().enumerate() {
            if ch == '@' {
                if at_col.is_some() {
                    return Err(ParseError::MultipleAt);
                }
                at_col = Some(c);
                at_row = Some(r);
            }
        }
    }

    let at_col = at_col.ok_or(ParseError::MissingAt)?;
    let at_row = at_row.unwrap();

    let mut rows = rows;

    let (col_offset, row_offset) = offset(pt, slot, at_col, at_row);

    if col_offset % 2 != 0 {
        for row in &mut rows {
            row.insert(0, ' ');
        }
    }
    if row_offset % 2 != 0 {
        rows.insert(0, vec![]);
    }

    for (r, row) in rows.iter().enumerate() {
        for (c, &ch) in row.iter().enumerate() {
            if ch != ' ' && is_dead_slot(&pt, c, r) {
                return Err(ParseError::DeadSlotUsed { col: c, row: r, ch });
            }
        }
    }

    Ok(Pattern {
        pattern_type: pt,
        rows,
        at_col,
        at_row,
    })
}

// The 2x2 repeating grid of slots for each type of pattern
// H:
//   W.
//   RW
// V wide:
//   F.
//   RW
// V narrow:
//   W.
//   ..

fn grid_to_slot(pt: PatternType, (row, col): (i32, i32)) -> Option<UnorientedSlot> {
    match (pt, row % 2 == 1, col % 2 == 1) {
        (PatternType::H, false, true) => None,
        (PatternType::H, true, false) => Some(UnorientedSlot::Room),
        (PatternType::H, _, _) => Some(UnorientedSlot::Wall),
        (PatternType::VWide, false, false) => Some(UnorientedSlot::Floor),
        (PatternType::VWide, false, true) => None,
        (PatternType::VWide, true, false) => Some(UnorientedSlot::Room),
        (PatternType::VWide, true, true) => Some(UnorientedSlot::Wall),
        (PatternType::VNarrow, false, false) => Some(UnorientedSlot::Wall),
        (PatternType::VNarrow, _, _) => None,
    }
}

/// Offset to apply to a raw pattern to make it line up with the 2x2 slot grid
fn offset(pt: PatternType, slot: UnorientedSlot, at_col: usize, at_row: usize) -> (usize, usize) {
    // find desired parity:
    let (p_col, p_row) = match (pt, slot) {
        (PatternType::H, UnorientedSlot::Room) => (0, 1),
        (PatternType::H, UnorientedSlot::Wall) => (1, 1),
        (PatternType::VNarrow, _) => (0, 0), // Only wall is valid, though!
        (PatternType::VWide, UnorientedSlot::Room) => (0, 1),
        (PatternType::VWide, UnorientedSlot::Wall) => (1, 1),
        (_, UnorientedSlot::Floor) => panic!("floors as anchors not yet supported!"),
    };
    ((at_col + p_col) % 2, (at_row + p_row) % 2)
}

// TODO: I think `offset` and `is_dead_slot` may be insonsistent with each other!

/// In H patterns the slot layout (per repeating tile) is:
///   (even, even) = W   (even, odd) = R   (odd, odd) = W   (odd, even) = dead
/// In V wide: same dead/live pattern as H.
/// In V narrow: only (even, even) are valid; everything else is dead.
fn is_dead_slot(pt: &PatternType, col: usize, row: usize) -> bool {
    match pt {
        PatternType::H | PatternType::VWide => col % 2 == 1 && row % 2 == 0,
        PatternType::VNarrow => col % 2 == 1 || row % 2 == 1,
    }
}

// ─── Mesh spec parser ─────────────────────────────────────────────────────────

fn parse_result(s: &str) -> Result<AutotileResult, ParseError> {
    if s == "none" {
        return Ok(AutotileResult::None);
    }
    let (multi, rest) = if let Some(r) = s.strip_prefix("(multi)").map(str::trim_start) {
        (true, r)
    } else {
        (false, s)
    };
    let spec = parse_mesh_spec(rest).ok_or_else(|| ParseError::InvalidResult(s.to_owned()))?;
    Ok(AutotileResult::Mesh { multi, spec })
}

/// Recursive-descent parser for mesh specs.
/// Grammar (standard precedence: * binds tighter than +):
///   expr  = term ('+' term)*
///   term  = factor ('*' factor)*
///   factor = '(' expr ')' | ident [':' number]
fn parse_mesh_spec(s: &str) -> Option<MeshSpec> {
    let s = s.trim();
    let (spec, rest) = parse_expr(s)?;
    if rest.trim().is_empty() {
        Some(spec)
    } else {
        None
    }
}

fn parse_expr(s: &str) -> Option<(MeshSpec, &str)> {
    let (mut lhs, mut rest) = parse_term(s)?;
    loop {
        let t = rest.trim_start();
        if let Some(after) = t.strip_prefix('+') {
            let (rhs, r2) = parse_term(after.trim_start())?;
            lhs = MeshSpec::Union(Box::new(lhs), Box::new(rhs));
            rest = r2;
        } else {
            break;
        }
    }
    Some((lhs, rest))
}

fn parse_term(s: &str) -> Option<(MeshSpec, &str)> {
    let (mut lhs, mut rest) = parse_factor(s)?;
    loop {
        let t = rest.trim_start();
        if let Some(after) = t.strip_prefix('*') {
            let (rhs, r2) = parse_factor(after.trim_start())?;
            lhs = MeshSpec::Intersection(Box::new(lhs), Box::new(rhs));
            rest = r2;
        } else {
            break;
        }
    }
    Some((lhs, rest))
}

fn parse_factor(s: &str) -> Option<(MeshSpec, &str)> {
    let s = s.trim_start();
    if let Some(inner) = s.strip_prefix('(') {
        let (spec, rest) = parse_expr(inner)?;
        let rest = rest.trim_start().strip_prefix(')')?;
        return Some((spec, rest));
    }
    // identifier (letters, digits, underscores, hyphens)
    let end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    let name = s[..end].to_owned();
    let rest = &s[end..];
    // optional `:number`
    if let Some(after_colon) = rest.trim_start().strip_prefix(':') {
        let after_colon = after_colon.trim_start();
        let num_end = after_colon
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_colon.len());
        if num_end > 0 {
            let rotation: i32 = after_colon[..num_end].parse().ok()?;
            return Some((MeshSpec::Atom { name, rotation }, &after_colon[num_end..]));
        }
    }
    Some((MeshSpec::Atom { name, rotation: 0 }, rest))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;

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

    // ── Parsing ──────────────────────────────────────────────────────────────

    #[test]
    fn parse_minimal_rule() {
        let input = "\
== wall: wall ==
--> none
";
        let file = parse(input).unwrap();
        check!(file.rules.len() == 1);
        let rule = &file.rules[0];
        check!(rule.structure_name == "wall");
        check!(rule.slot == UnorientedSlot::Wall);
        check!(rule.cases.len() == 1);
        check!(rule.cases[0].pattern.is_none());
        check!(rule.cases[0].result == AutotileResult::None);
    }

    #[test]
    fn parse_h_pattern() {
        let input = "\
== wall: wall ==
H:
 . @ .
--> straight
";
        let file = parse(input).unwrap();
        let rule = &file.rules[0];
        check!(rule.cases.len() == 1);
        let case = &rule.cases[0];
        let pat = case.pattern.as_ref().unwrap();
        check!(pat.pattern_type == PatternType::H);
        check!(pat.at_col == 2); // '@' is at index 2 after stripping leading space
        check!(pat.at_row == 0);
        // The checks should be: col 0 = '.', col 4 = '.'
        let checks = pat.relative_checks();
        check!(checks[&(-1, 1)] == '.');
        check!(checks[&(3, 1)] == '.');
        check!(checks.len() == 2);
    }

    #[test]
    fn parse_mesh_spec_atom() {
        let spec = parse_mesh_spec("my_mesh").unwrap();
        check!(spec == atom("my_mesh"));
    }

    #[test]
    fn parse_mesh_spec_with_rotation() {
        let spec = parse_mesh_spec("my_mesh:2").unwrap();
        check!(spec == atom_r("my_mesh", 2));
    }

    #[test]
    fn parse_mesh_spec_union() {
        let spec = parse_mesh_spec("a + b").unwrap();
        check!(spec == MeshSpec::Union(Box::new(atom("a")), Box::new(atom("b"))));
    }

    #[test]
    fn parse_mesh_spec_precedence() {
        // a + b * c  should parse as  a + (b * c)
        let spec = parse_mesh_spec("a + b * c").unwrap();
        let expected = MeshSpec::Union(
            Box::new(atom("a")),
            Box::new(MeshSpec::Intersection(
                Box::new(atom("b")),
                Box::new(atom("c")),
            )),
        );
        check!(spec == expected);
    }

    #[test]
    fn parse_multi_result() {
        let input = "\
== wall: wall ==
H:
 = @ =
--> (multi) mesh_a
--> mesh_b
";
        let file = parse(input).unwrap();
        let rule = &file.rules[0];
        check!(rule.cases.len() == 2);
        check!(
            rule.cases[0].result
                == AutotileResult::Mesh {
                    multi: true,
                    spec: atom("mesh_a")
                }
        );
        check!(
            rule.cases[1].result
                == AutotileResult::Mesh {
                    multi: false,
                    spec: atom("mesh_b")
                }
        );
    }

    #[test]
    fn parse_two_rules() {
        let input = "\
== wall: wall ==
--> mesh_a

== railing: wall ==
--> mesh_b
";
        let file = parse(input).unwrap();
        check!(file.rules.len() == 2);
        check!(file.rules[0].structure_name == "wall");
        check!(file.rules[1].structure_name == "railing");
    }

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
== wall: wall ==
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
== wall: wall ==
H:
 @ =
--> my_mesh
";
        let file = parse(input).unwrap();
        let oriented = compile_rule(&file.rules[0]);
        // rotation 0: my_mesh:0; rotation 1: my_mesh:1; etc.
        let rots: Vec<u8> = oriented.cases.iter().map(|c| c.rotation).collect();
        check!(rots == vec![0, 1, 2, 3]);
        // Check that result meshes have incremented rotations
        for case in &oriented.cases {
            if let AutotileResult::Mesh {
                spec: MeshSpec::Atom { rotation, .. },
                ..
            } = &case.result
            {
                check!(*rotation == case.rotation as i32);
            }
        }
    }

    // ── Rotation math ─────────────────────────────────────────────────────────

    #[test]
    fn rotate_offset_identity() {
        check!(rotate_offset(3, 1, 0) == (3, 1));
    }

    #[test]
    fn rotate_offset_90cw() {
        // (1, 0) [right] → (0, -1) [up in grid = -Z]
        check!(rotate_offset(1, 0, 1) == (0, -1));
        // (0, 1) [down] → (1, 0) [right]
        check!(rotate_offset(0, 1, 1) == (1, 0));
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

    // ── Matching ──────────────────────────────────────────────────────────────

    #[cfg(autotile_matching)]
    mod matching {
        use super::*;
        use crate::sparse3d::{RelSlot, SlotLocation, Sparse3D};
        use crate::structure::StructureId;
        use crate::wall_grid::Cell;

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
        #[ignore = "Fix in progress"]
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
    }
}
