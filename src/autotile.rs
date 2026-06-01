use std::collections::HashMap;

use anyhow::{bail, Context as _};

// These imports are only valid in the main crate (not the build script).
// build.rs sets `cargo:rustc-cfg=autotile_matching` so the main crate sees them.
#[cfg(autotile_matching)]
use crate::sparse3d::{SlotLocation, Sparse3D, RelSlot};
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

/// This matches a type in sparse3d, but we need this at build time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AutotileRelSlotOffset {
    /// When using relative slots, we need the origin, because slots are shared between two adjacent cubes
    pub origin_slot: AutotileRelSlot,
    pub cube_offset: (i32, i32, i32),
    pub dest_slot: AutotileRelSlot,
}

/// This matches a type in sparse3d, but we need this at build time.
/// Every `RelSlot` other than `Room` is shared with an adjacent cube.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutotileRelSlot {
    Room,
    XHiWall,
    /// Equivalent to the XLoWall of +(1,0,0)
    XLoWall,
    Floor,
    /// Equivalent to the Ceiling of +(0,-1,1)
    Ceiling,
    ZHiWall,
    /// Equivalent to the ZLoWall of +(0,0,1)
    ZLoWall,
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
    /// Returns non-wildcard, non-@ cells as AutotileRelSlotOffset → char.
    ///
    /// `at_col`/`at_row` store the pre-insert `@` position; the actual `@` in
    /// `self.rows` may be shifted by one column/row due to parity adjustment in
    /// `build_pattern`. We scan for the real `@` to get the correct origin slot.
    pub fn relative_checks(&self) -> HashMap<AutotileRelSlotOffset, char> {
        let (at_col_actual, at_row_actual) = self
            .rows
            .iter()
            .enumerate()
            .find_map(|(r, row)| {
                row.iter()
                    .enumerate()
                    .find(|(_, &ch)| ch == '@')
                    .map(|(c, _)| (c, r))
            })
            .expect("pattern has no @");

        let (ax, ay, az, origin_slot) =
            grid_pos_to_3d(self.pattern_type, at_col_actual, at_row_actual);

        let mut map = HashMap::new();
        for (r, row) in self.rows.iter().enumerate() {
            for (c, &ch) in row.iter().enumerate() {
                if ch == ' ' || ch == '@' {
                    continue;
                }
                let (tx, ty, tz, dest_slot) = grid_pos_to_3d(self.pattern_type, c, r);
                map.insert(
                    AutotileRelSlotOffset {
                        origin_slot,
                        cube_offset: (tx - ax, ty - ay, tz - az),
                        dest_slot,
                    },
                    ch,
                );
            }
        }
        map
    }
}

/// Map a pattern-grid position to a 3D cube coordinate + canonical slot type.
///
/// For H patterns: col = X, row = Z, Y = 0.
///   (even_row, even_col) → ZLoWall at (col/2, 0, row/2)
///   (odd_row,  even_col) → Room    at (col/2, 0, row/2)
///   (odd_row,  odd_col)  → XLoWall at (col/2+1, 0, row/2)
///
/// For VWide patterns: col = X, row = Y, Z = 0.
///   (even_row, even_col) → Floor   at (col/2, row/2, 0)
///   (odd_row,  even_col) → Room    at (col/2, row/2, 0)
///   (odd_row,  odd_col)  → XLoWall at (col/2+1, row/2, 0)
///
/// For VNarrow patterns: col = X, row = Y, Z = 0; only (even, even) are valid.
///   (even_row, even_col) → XLoWall at (col/2, row/2, 0)
fn grid_pos_to_3d(pt: PatternType, col: usize, row: usize) -> (i32, i32, i32, AutotileRelSlot) {
    let c = col as i32;
    let r = row as i32;
    match pt {
        PatternType::H => match (row % 2, col % 2) {
            (0, 0) => (c / 2, 0, r / 2, AutotileRelSlot::ZLoWall),
            (1, 0) => (c / 2, 0, r / 2, AutotileRelSlot::Room),
            (1, 1) => (c / 2 + 1, 0, r / 2, AutotileRelSlot::XLoWall),
            _ => panic!("dead slot at ({col}, {row})"),
        },
        PatternType::VWide => match (row % 2, col % 2) {
            (0, 0) => (c / 2, r / 2, 0, AutotileRelSlot::Floor),
            (1, 0) => (c / 2, r / 2, 0, AutotileRelSlot::Room),
            (1, 1) => (c / 2 + 1, r / 2, 0, AutotileRelSlot::XLoWall),
            _ => panic!("dead slot at ({col}, {row})"),
        },
        PatternType::VNarrow => match (row % 2, col % 2) {
            (0, 0) => (c / 2, r / 2, 0, AutotileRelSlot::XLoWall),
            _ => panic!("dead slot at ({col}, {row})"),
        },
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
    pub checks: HashMap<AutotileRelSlotOffset, char>,
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
    let mut cases_plus_90 = Vec::new();

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

                let mut seen: Vec<HashMap<AutotileRelSlotOffset, char>> = Vec::new();
                for rot in 0u8..4 {
                    if rule.slot == UnorientedSlot::Wall && rot % 2 == 1 {
                        // A particular wall is only symmetric in 180 degrees, not 90
                        // (But we need to store a 90-offset rotation for Z walls; the
                        // patterns are constructed for X walls.)
                        continue;
                    }
                    let rotated = rotate_checks(&base_checks, rot);
                    if !seen.iter().any(|s| s == &rotated) {
                        seen.push(rotated.clone());
                        cases.push(OrientedCase {
                            pattern_type: pt.clone(),
                            checks: rotated,
                            result: rotate_result(&case.result, rot),
                            rotation: rot,
                        });

                        if rule.slot == UnorientedSlot::Wall {
                            let rotated_plus_90 = rotate_checks(&base_checks, rot + 1);
                            cases_plus_90.push(OrientedCase {
                                checks: rotated_plus_90,
                                ..cases.last().unwrap().clone()
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

// ─── Matching ─────────────────────────────────────────────────────────────────

#[cfg(autotile_matching)]
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

// ─── Parser ───────────────────────────────────────────────────────────────────

fn strip_comment(line: &str) -> &str {
    if let Some(idx) = line.find('#') {
        line[..idx].trim_end()
    } else {
        line.trim_end()
    }
}

pub fn parse(input: &str) -> anyhow::Result<AutotileFile> {
    let lines: Vec<&str> = input.lines().collect();
    let mut i = 0;
    let mut rules = Vec::new();

    while i < lines.len() {
        let line = strip_comment(lines[i]);
        let lineno = i + 1;
        if line.is_empty() {
            i += 1;
            continue;
        }
        if let Some(inner) = line.strip_prefix("==").and_then(|s| s.strip_suffix("==")) {
            let inner = inner.trim();
            let colon = inner
                .rfind(':')
                .with_context(|| format!("line {lineno}: unexpected line: {line:?}"))?;
            let structure_name = inner[..colon].trim().to_owned();
            let slot = match inner[colon + 1..].trim() {
                "wall" => UnorientedSlot::Wall,
                "room" => UnorientedSlot::Room,
                other => bail!("line {lineno}: invalid slot: {other:?}"),
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
            bail!("line {lineno}: unexpected line: {line:?}");
        }
    }

    Ok(AutotileFile { rules })
}

fn parse_cases(
    lines: &[&str],
    mut i: usize,
    slot: UnorientedSlot,
) -> anyhow::Result<(Vec<PatternCase>, usize)> {
    let mut cases = Vec::new();

    loop {
        while i < lines.len() && strip_comment(lines[i]).is_empty() {
            i += 1;
        }
        if i >= lines.len() {
            break;
        }
        let line = strip_comment(lines[i]);
        let lineno = i + 1;

        if line.starts_with("==") {
            break;
        }

        if let Some(rest) = line.strip_prefix("-->") {
            let result = parse_result(rest.trim())
                .with_context(|| format!("line {lineno}"))?;
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
            _ => bail!("line {lineno}: unexpected line: {line:?}"),
        };
        let pt_lineno = lineno;
        i += 1;

        let mut pattern_rows: Vec<Vec<char>> = Vec::new();
        loop {
            if i >= lines.len() {
                bail!("line {pt_lineno}: pattern with no --> result");
            }
            let pline = strip_comment(lines[i]);
            let plineno = i + 1;
            if let Some(rest) = pline.strip_prefix("-->") {
                let pattern = build_pattern(pt, pattern_rows, slot)
                    .with_context(|| format!("line {plineno}"))?;
                let result = parse_result(rest.trim())
                    .with_context(|| format!("line {plineno}"))?;
                cases.push(PatternCase {
                    pattern: Some(pattern),
                    result,
                });
                i += 1;
                break;
            }
            if pline.starts_with("==") {
                bail!("line {plineno}: pattern with no --> result");
            }
            if pline.is_empty() {
                i += 1;
                continue;
            }
            if !pline.starts_with(' ') {
                bail!("line {plineno}: pattern line must start with a space, got: {pline:?}");
            }
            let content: Vec<char> = pline[1..].chars().collect();
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
) -> anyhow::Result<Pattern> {
    let mut at_col = None;
    let mut at_row = None;

    for (r, row) in rows.iter().enumerate() {
        for (c, &ch) in row.iter().enumerate() {
            if ch == '@' {
                if at_col.is_some() {
                    bail!("pattern has multiple @ characters");
                }
                at_col = Some(c);
                at_row = Some(r);
            }
        }
    }

    let at_col = at_col.context("pattern has no @ character")?;
    let at_row = at_row.unwrap();

    let mut rows = rows;

    let (col_offset, row_offset) = offset(pt, slot, at_col, at_row)?;

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
                bail!("character {ch:?} in dead slot at ({c},{r})");
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

#[allow(unused)] // Maybe we'll need this?
/// From a grid location, determine what slot it represents
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

/// Offset to apply to a raw pattern to make the anchor appear in the right spot in the 2x2 slot grid
fn offset(
    pt: PatternType,
    slot: UnorientedSlot,
    anchor_col: usize,
    anchor_row: usize,
) -> anyhow::Result<(usize, usize)> {
    let (p_col, p_row) = match (pt, slot) {
        (PatternType::H, UnorientedSlot::Room) => (0, 1),
        (PatternType::H, UnorientedSlot::Wall) => (1, 1),
        (PatternType::VNarrow, _) => (0, 0), // Only wall is valid, though!
        (PatternType::VWide, UnorientedSlot::Room) => (0, 1),
        // (0,0) would also work, but we need to pick a canonical version:
        (PatternType::VWide, UnorientedSlot::Wall) => (1, 1),
        (_, UnorientedSlot::Floor) => bail!("floors as anchors are not yet supported"),
    };
    Ok(((anchor_col + p_col) % 2, (anchor_row + p_row) % 2))
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

fn parse_result(s: &str) -> anyhow::Result<AutotileResult> {
    if s == "none" {
        return Ok(AutotileResult::None);
    }
    let (multi, rest) = if let Some(r) = s.strip_prefix("(multi)").map(str::trim_start) {
        (true, r)
    } else {
        (false, s)
    };
    let spec = parse_mesh_spec(rest)
        .with_context(|| format!("invalid result: {s:?}"))?;
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
        // After parity adjustment, '@' is at (col=3, row=1) = XLoWall at cube (2,0,0).
        // '.' at (col=1, row=1) = XLoWall at cube (1,0,0): offset (-1,0,0).
        // '.' at (col=5, row=1) = XLoWall at cube (3,0,0): offset (+1,0,0).
        let checks = pat.relative_checks();
        let lo = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::XLoWall,
            cube_offset: (-1, 0, 0),
            dest_slot: AutotileRelSlot::XLoWall,
        };
        let hi = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::XLoWall,
            cube_offset: (1, 0, 0),
            dest_slot: AutotileRelSlot::XLoWall,
        };
        check!(checks[&lo] == '.');
        check!(checks[&hi] == '.');
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

    // ── grid_to_slot / offset consistency ────────────────────────────────────

    #[test]
    fn grid_to_slot_and_offset_consistent() {
        // All valid (pt, slot) anchor combinations (Floor panics, VNarrow only supports Wall).
        let valid_combos = [
            (PatternType::H, UnorientedSlot::Wall),
            (PatternType::H, UnorientedSlot::Room),
            (PatternType::VNarrow, UnorientedSlot::Wall),
            (PatternType::VWide, UnorientedSlot::Wall),
            (PatternType::VWide, UnorientedSlot::Room),
        ];

        for (pt, slot) in valid_combos {
            for anchor_col in 0usize..2 {
                for anchor_row in 0usize..2 {
                    let (col_off, row_off) = offset(pt, slot, anchor_col, anchor_row).unwrap();
                    // After inserting col_off leading columns / row_off leading rows,
                    // the anchor lands at (anchor_col + col_off, anchor_row + row_off).
                    // What matters for slot identity is the parity.
                    let target_col = (anchor_col + col_off) % 2;
                    let target_row = (anchor_row + row_off) % 2;
                    let result = grid_to_slot(pt, (target_row as i32, target_col as i32));
                    check!(
                        result == Some(slot),
                        "pt={:?} slot={:?} anchor=({},{}) → target parity=({},{}) → {:?}",
                        pt,
                        slot,
                        anchor_col,
                        anchor_row,
                        target_col,
                        target_row,
                        result
                    );
                }
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
            check!(step2 == rotate_offset(dc, dr, 2), "({dc},{dr}): 90°×2 ≠ 180°");
            check!(step3 == rotate_offset(dc, dr, 3), "({dc},{dr}): 90°×3 ≠ 270°");
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
            check!(step2 == rotate_autotile_rel_slot(slot, 2), "{slot:?}: 90°×2 ≠ 180°");
            check!(step3 == rotate_autotile_rel_slot(slot, 3), "{slot:?}: 90°×3 ≠ 270°");
            check!(step4 == slot,                              "{slot:?}: 90°×4 ≠ identity");
        }
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
            check!(
                r_z_back == &AutotileResult::Mesh { multi: false, spec: atom_r("wall_across", 2) },
                "ZLoWall neighbor at (0,0,-1): expected wall_across:2, got {r_z_back:?}"
            );
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
                    AutotileResult::Mesh {
                        spec: MeshSpec::Atom { rotation, .. },
                        ..
                    } => *rotation,
                    other => panic!("expected mesh atom, got {other:?}"),
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
            // match this configuration and return mesh rotation=1.
            let mut g1: Sparse3D<Cell> = Sparse3D::new();
            g1.set(SlotLocation::new(0, 0, 1, RelSlot::Room), desk_cell());
            let r1 = match_pattern(&oriented, &g1, anchor, is_desk).unwrap();
            check!(mesh_rotation(r1) == 1, "rot=1 (CW): expected mesh rotation 1, got {}", mesh_rotation(r1));

            // 180° (sparse3d OneEighty: (x,y,z)→(-x,y,-z)) moves (1,0,0) to (-1,0,0).
            let mut g2: Sparse3D<Cell> = Sparse3D::new();
            g2.set(SlotLocation::new(-1, 0, 0, RelSlot::Room), desk_cell());
            let r2 = match_pattern(&oriented, &g2, anchor, is_desk).unwrap();
            check!(mesh_rotation(r2) == 2, "rot=2 (180°): expected mesh rotation 2, got {}", mesh_rotation(r2));

            // CCW / 270° CW (sparse3d CounterClockwise: (x,y,z)→(z,y,-x)) moves
            // (1,0,0) to (0,0,-1).  The autotile rot=3 case must match.
            let mut g3: Sparse3D<Cell> = Sparse3D::new();
            g3.set(SlotLocation::new(0, 0, -1, RelSlot::Room), desk_cell());
            let r3 = match_pattern(&oriented, &g3, anchor, is_desk).unwrap();
            check!(mesh_rotation(r3) == 3, "rot=3 (CCW): expected mesh rotation 3, got {}", mesh_rotation(r3));
        }
    }
}
