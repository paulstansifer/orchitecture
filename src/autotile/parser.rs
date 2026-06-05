use std::collections::HashMap;

use anyhow::{bail, Context as _};

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
    // TODO: get rid of `.rotation` here. The outermost `Rotation` will continue to be special,
    // but that's okay.
    Atom { name: String, rotation: i32 },
    /// Runtime rotation applied by the autotile matching system (degrees, CW in XZ plane).
    /// Distinct from Atom's rotation, which is author-specified and baked into the mesh file.
    Rotation(i32, Box<MeshSpec>),
    Union(Box<MeshSpec>, Box<MeshSpec>),
    Intersection(Box<MeshSpec>, Box<MeshSpec>),
}

impl MeshSpec {
    /// Wraps this spec in a `Rotation` rather than pushing the angle into atoms,
    /// so that the mesh filename is derived from the unrotated inner spec.
    pub fn rotate(self, by: i32) -> Self {
        let by = by.rem_euclid(360);
        if by == 0 {
            return self;
        }
        match self {
            MeshSpec::Rotation(r, inner) => MeshSpec::Rotation((r + by).rem_euclid(360), inner),
            other => MeshSpec::Rotation(by, Box::new(other)),
        }
    }

    /// Returns the outermost runtime rotation in degrees (0 if none).
    pub fn outer_rotation(&self) -> i32 {
        match self {
            MeshSpec::Rotation(r, _) => *r,
            _ => 0,
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
            (0, 0) => (c / 2, -(r / 2) + 1, 0, AutotileRelSlot::Floor),
            (1, 0) => (c / 2, -(r / 2), 0, AutotileRelSlot::Room),
            (1, 1) => (c / 2 + 1, -(r / 2), 0, AutotileRelSlot::XLoWall),
            _ => panic!("dead slot at ({col}, {row})"),
        },
        PatternType::VNarrow => match (row % 2, col % 2) {
            (0, 0) => (c / 2, -(r / 2), 0, AutotileRelSlot::XLoWall),
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
                pattern_rows.push(vec![]);
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

#[allow(unused)]
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

// TODO: I think `offset` and `is_dead_slot` may be inconsistent with each other!

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

// ─── Filename helpers ─────────────────────────────────────────────────────────

pub fn slot_tag(slot: UnorientedSlot) -> &'static str {
    match slot {
        UnorientedSlot::Wall => "w",
        UnorientedSlot::Room => "ro",
        UnorientedSlot::Floor => "fl",
    }
}

/// Deterministic filename stem for a mesh spec (no extension).
/// Slot is encoded for non-trivial atoms because the pivot translation differs by slot.
pub fn spec_stem(spec: &MeshSpec, slot: UnorientedSlot) -> String {
    match spec {
        MeshSpec::Rotation(_, inner) => spec_stem(inner, slot),
        MeshSpec::Atom { name, rotation: 0 } => name.clone(),
        MeshSpec::Atom { name, rotation } => {
            format!("{name}_{}_r{rotation}", slot_tag(slot))
        }
        MeshSpec::Union(a, b) => {
            format!("union__{}__{}", spec_stem(a, slot), spec_stem(b, slot))
        }
        MeshSpec::Intersection(a, b) => {
            format!("isect__{}__{}", spec_stem(a, slot), spec_stem(b, slot))
        }
    }
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
    fn parse_h_wall_vertical_neighbors() {
        // Dots above and below @ are in the same slot kind (XLoWall at odd col, odd row).
        // Blank lines between rows must be preserved as spacer rows, not skipped.
        let input = "\
== wall: wall ==
H:
  .

  @

  .
--> wall_mesh
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        let checks = pat.relative_checks();
        let north = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::XLoWall,
            cube_offset: (0, 0, -1),
            dest_slot: AutotileRelSlot::XLoWall,
        };
        let south = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::XLoWall,
            cube_offset: (0, 0, 1),
            dest_slot: AutotileRelSlot::XLoWall,
        };
        check!(checks[&north] == '.');
        check!(checks[&south] == '.');
        check!(checks.len() == 2);
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
}
