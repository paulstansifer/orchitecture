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
    HNarrow,
    V,
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

impl AutotileRelSlotOffset {
    /// For a wall offset, returns the offset for the Room on the far side of that wall from
    /// the anchor. The anchor is assumed to be on a canonical (Lo) wall slot.
    ///
    /// The canonical wall position relative to the anchor is
    /// `wall_vec = -origin.abs + cube_offset + dest.abs`.
    /// The far room is at `wall_vec` when `wall_vec > 0` on the wall axis, or `wall_vec - 1`
    /// otherwise; then `room_cube_offset = far_room_vec + origin.abs`.
    ///
    /// Panics if `dest_slot` is not a wall slot.
    pub fn far_room_offset(self) -> Self {
        let (ox, _oy, oz) = self.origin_slot.absolute_offset();
        let (dx, dy, dz) = self.cube_offset;
        let (ex, _ey, ez) = self.dest_slot.absolute_offset();
        let room_cube_offset = match self.dest_slot {
            AutotileRelSlot::XLoWall | AutotileRelSlot::XHiWall => {
                let wall_x = -ox + dx + ex;
                let far_x = if wall_x > 0 { wall_x } else { wall_x - 1 };
                (far_x + ox, dy, dz)
            }
            AutotileRelSlot::ZLoWall | AutotileRelSlot::ZHiWall => {
                let wall_z = -oz + dz + ez;
                let far_z = if wall_z > 0 { wall_z } else { wall_z - 1 };
                (dx, dy, far_z + oz)
            }
            other => panic!("far_room_offset: not a wall slot: {other:?}"),
        };
        AutotileRelSlotOffset {
            origin_slot: self.origin_slot,
            cube_offset: room_cube_offset,
            dest_slot: AutotileRelSlot::Room,
        }
    }
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

impl AutotileRelSlot {
    /// Cube offset needed to canonicalize this slot (mirrors `RelSlot::absolute_offset`).
    pub fn absolute_offset(self) -> (i32, i32, i32) {
        match self {
            AutotileRelSlot::XHiWall => (1, 0, 0),
            AutotileRelSlot::Ceiling => (0, 1, 0),
            AutotileRelSlot::ZHiWall => (0, 0, 1),
            _ => (0, 0, 0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MeshSpec {
    // TODO: get rid of `.rotation` here. The outermost `Rotation` will continue to be special,
    // but that's okay.
    Atom {
        name: String,
        rotation: i32,
    },
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

/// Annotation attached to a labeled pattern character (e.g. `1=stairs:90`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternAnnotation {
    pub name: String,
    /// Required facing in degrees relative to the rule's base orientation (0° = PosX).
    /// `None` means any facing is accepted.
    pub orientation: Option<i32>,
}

/// One 2D grid of a (possibly multi-layer) pattern. Single-layer patterns have
/// exactly one layer with `t == 0`.
#[derive(Debug, Clone)]
pub struct Layer {
    pub pattern_type: PatternType,
    /// Third-axis coordinate of this layer: Y for the H family, Z for the V family.
    /// In a multi-layer rule the leftmost layer is the highest coordinate.
    pub t: i32,
    pub rows: Vec<Vec<char>>,
}

#[derive(Debug, Clone)]
pub struct Pattern {
    /// Layers stacked along the grid plane's third axis (Y for H, Z for V).
    pub layers: Vec<Layer>,
    pub at_col: usize,
    pub at_row: usize,
    /// Index into `layers` of the layer containing the `@` anchor.
    pub at_layer: usize,
    /// Labeled character → annotation (e.g. `'1'` → stairs at 90°).
    pub annotations: HashMap<char, PatternAnnotation>,
}

/// The spatial axis a pattern type stacks along when layered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerAxis {
    Y,
    Z,
}

fn layer_axis(pt: PatternType) -> LayerAxis {
    match pt {
        PatternType::H | PatternType::HNarrow => LayerAxis::Y,
        PatternType::V | PatternType::VNarrow => LayerAxis::Z,
    }
}

fn is_narrow(pt: PatternType) -> bool {
    matches!(pt, PatternType::HNarrow | PatternType::VNarrow)
}

/// True if both pattern types belong to the same family (H/HNarrow or V/VNarrow).
fn same_family(a: PatternType, b: PatternType) -> bool {
    layer_axis(a) == layer_axis(b)
}

/// Add a layer's third-axis offset `t` to a cube coordinate, on the axis the
/// pattern type stacks along.
fn apply_layer_t(pt: PatternType, t: i32, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
    match layer_axis(pt) {
        LayerAxis::Y => (x, y + t, z),
        LayerAxis::Z => (x, y, z + t),
    }
}

impl Pattern {
    /// The pattern type of the layer containing the `@` anchor.
    pub fn anchor_pattern_type(&self) -> PatternType {
        self.layers[self.at_layer].pattern_type
    }

    /// Returns non-wildcard, non-@ cells as AutotileRelSlotOffset → char, across all layers.
    pub fn relative_checks(&self) -> HashMap<AutotileRelSlotOffset, char> {
        let anchor = &self.layers[self.at_layer];
        let (ax0, ay0, az0, origin_slot) =
            grid_pos_to_3d(anchor.pattern_type, self.at_col, self.at_row);
        let (ax, ay, az) = apply_layer_t(anchor.pattern_type, anchor.t, ax0, ay0, az0);

        let mut map = HashMap::new();
        for layer in &self.layers {
            for (r, row) in layer.rows.iter().enumerate() {
                for (c, &ch) in row.iter().enumerate() {
                    if ch == ' ' || ch == '@' {
                        continue;
                    }
                    let (tx0, ty0, tz0, dest_slot) = grid_pos_to_3d(layer.pattern_type, c, r);
                    let (tx, ty, tz) = apply_layer_t(layer.pattern_type, layer.t, tx0, ty0, tz0);
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
        PatternType::V => match (row % 2, col % 2) {
            (0, 0) => (c / 2, -(r / 2) + 1, 0, AutotileRelSlot::Floor),
            (1, 0) => (c / 2, -(r / 2), 0, AutotileRelSlot::Room),
            (1, 1) => (c / 2 + 1, -(r / 2), 0, AutotileRelSlot::XLoWall),
            _ => panic!("dead slot at ({col}, {row})"),
        },
        PatternType::VNarrow => match (row % 2, col % 2) {
            // The depth-facing wall ("behind" the room) of the same cube as V's room.
            // This is the wall that separates depth-adjacent V layers in multi-layer rules.
            (1, 0) => (c / 2, -(r / 2), 0, AutotileRelSlot::ZLoWall),
            _ => panic!("dead slot at ({col}, {row})"),
        },
        PatternType::HNarrow => match (row % 2, col % 2) {
            // The floor of the same cube as H's room (the R position in H).
            (1, 0) => (c / 2, 0, r / 2, AutotileRelSlot::Floor),
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
                "floor" => UnorientedSlot::Floor,
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
            let result = parse_result(rest.trim()).with_context(|| format!("line {lineno}"))?;
            cases.push(PatternCase {
                pattern: None,
                result,
            });
            i += 1;
            break; // else case is always last
        }

        let (layer_types, annotations, is_pipe) =
            parse_header(line).with_context(|| format!("line {lineno}"))?;
        let layer_ts = assign_layer_ts(&layer_types).with_context(|| format!("line {lineno}"))?;
        let n = layer_types.len();
        let pt_lineno = lineno;
        i += 1;

        let mut layer_rows: Vec<Vec<Vec<char>>> = vec![Vec::new(); n];
        loop {
            if i >= lines.len() {
                bail!("line {pt_lineno}: pattern with no --> result");
            }
            let pline = strip_comment(lines[i]);
            let plineno = i + 1;
            if let Some(rest) = pline.strip_prefix("-->") {
                let pattern = build_pattern(layer_types, layer_ts, layer_rows, slot, annotations)
                    .with_context(|| format!("line {plineno}"))?;
                let result =
                    parse_result(rest.trim()).with_context(|| format!("line {plineno}"))?;
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
                for rows in &mut layer_rows {
                    rows.push(vec![]);
                }
                i += 1;
                continue;
            }
            if is_pipe {
                let segs = split_pipe_line(pline).with_context(|| format!("line {plineno}"))?;
                if segs.len() != n {
                    bail!(
                        "line {plineno}: expected {n} layer segments, got {}",
                        segs.len()
                    );
                }
                for (l, seg) in segs.into_iter().enumerate() {
                    layer_rows[l].push(seg);
                }
            } else {
                if !pline.starts_with(' ') {
                    bail!("line {plineno}: pattern line must start with a space, got: {pline:?}");
                }
                layer_rows[0].push(pline[1..].chars().collect());
            }
            i += 1;
        }
    }

    Ok((cases, i))
}

/// Parse space-separated annotations from a pattern-type header, e.g. `"1=stairs:90 2=railing"`.
fn parse_annotations(s: &str) -> anyhow::Result<HashMap<char, PatternAnnotation>> {
    let mut map = HashMap::new();
    for token in s.split_whitespace() {
        let eq_idx = token
            .find('=')
            .with_context(|| format!("annotation {token:?} missing '='"))?;
        let key_str = &token[..eq_idx];
        if key_str.len() != 1 {
            bail!("annotation key must be a single character, got {key_str:?}");
        }
        let key = key_str.chars().next().unwrap();
        let rest = &token[eq_idx + 1..];
        let annotation = if let Some(colon) = rest.rfind(':') {
            let name = rest[..colon].to_owned();
            let deg_str = &rest[colon + 1..];
            let degrees: i32 = deg_str
                .parse()
                .with_context(|| format!("invalid orientation {deg_str:?}"))?;
            if degrees % 90 != 0 {
                bail!("orientation must be a multiple of 90, got {degrees}");
            }
            PatternAnnotation {
                name,
                orientation: Some(degrees.rem_euclid(360)),
            }
        } else {
            PatternAnnotation {
                name: rest.to_owned(),
                orientation: None,
            }
        };
        map.insert(key, annotation);
    }
    Ok(map)
}

/// Parse a single pattern-type token (no colon), e.g. `"H"`, `"H narrow"`.
fn parse_pattern_type(s: &str) -> anyhow::Result<PatternType> {
    Ok(match s {
        "H" => PatternType::H,
        "H narrow" => PatternType::HNarrow,
        "V" => PatternType::V,
        "V narrow" => PatternType::VNarrow,
        other => bail!("unknown pattern type: {other:?}"),
    })
}

/// Parse a pattern header line into its layer types + annotations. Returns
/// whether the header used pipe (multi-layer) syntax. A bare header like `H:`
/// is a single layer; a pipe header like `|H|H narrow|H|:` lists several.
fn parse_header(
    line: &str,
) -> anyhow::Result<(Vec<PatternType>, HashMap<char, PatternAnnotation>, bool)> {
    let line = line.trim_start();
    let colon = line
        .find(':')
        .with_context(|| format!("unexpected line: {line:?}"))?;
    let spec = line[..colon].trim();
    let annot_str = line[colon + 1..].trim();
    let is_pipe = spec.starts_with('|');
    let types = if is_pipe {
        let inner = spec
            .strip_prefix('|')
            .unwrap()
            .strip_suffix('|')
            .with_context(|| format!("multi-layer header must end with '|': {line:?}"))?;
        let types = inner
            .split('|')
            .map(|s| parse_pattern_type(s.trim()))
            .collect::<anyhow::Result<Vec<_>>>()?;
        if types.is_empty() {
            bail!("empty multi-layer header");
        }
        types
    } else {
        vec![parse_pattern_type(spec)?]
    };
    let annotations = parse_annotations(annot_str)?;
    Ok((types, annotations, is_pipe))
}

/// Split a multi-layer pattern line `|a|b|c|` into per-layer character segments.
/// A leading space (alignment padding) before the first `|` is allowed.
fn split_pipe_line(line: &str) -> anyhow::Result<Vec<Vec<char>>> {
    let line = line.trim_start();
    let inner = line
        .strip_prefix('|')
        .with_context(|| format!("multi-layer pattern line must start with '|': {line:?}"))?
        .strip_suffix('|')
        .with_context(|| format!("multi-layer pattern line must end with '|': {line:?}"))?;
    Ok(inner.split('|').map(|s| s.chars().collect()).collect())
}

/// Assign each layer its third-axis coordinate (Y for H family, Z for V family).
/// Leftmost layer is the highest coordinate; regular layers decrease by 1. A
/// narrow layer is the boundary belonging to the regular above it (its left
/// neighbour); a leading narrow belongs to the cube above the first regular.
/// Errors on two consecutive narrow layers or a multi-layer rule with no regular.
fn assign_layer_ts(types: &[PatternType]) -> anyhow::Result<Vec<i32>> {
    if types.len() == 1 {
        // Single-layer rules (including a lone narrow) live at t = 0.
        return Ok(vec![0]);
    }
    if let Some(&first) = types.first() {
        if !types.iter().all(|&pt| same_family(pt, first)) {
            bail!("all layers in a multi-layer rule must be the same family (all H/H narrow or all V/V narrow)");
        }
    }
    let mut ts = vec![0i32; types.len()];
    let mut running = 0i32;
    let mut last_regular: Option<i32> = None;
    let mut prev_narrow = false;
    let mut leading_narrow: Option<usize> = None;
    for (k, &pt) in types.iter().enumerate() {
        if is_narrow(pt) {
            if prev_narrow {
                bail!("two consecutive narrow layers");
            }
            match last_regular {
                Some(rt) => ts[k] = rt,
                None => leading_narrow = Some(k),
            }
            prev_narrow = true;
        } else {
            ts[k] = running;
            last_regular = Some(running);
            running -= 1;
            prev_narrow = false;
        }
    }
    if last_regular.is_none() {
        bail!("multi-layer pattern has no regular (non-narrow) layer");
    }
    // A leading narrow is the boundary above the first regular (which is at t = 0).
    if let Some(k) = leading_narrow {
        ts[k] = 1;
    }
    Ok(ts)
}

fn build_pattern(
    layer_types: Vec<PatternType>,
    layer_ts: Vec<i32>,
    layer_rows: Vec<Vec<Vec<char>>>,
    slot: UnorientedSlot,
    annotations: HashMap<char, PatternAnnotation>,
) -> anyhow::Result<Pattern> {
    // Find the single '@' across all layers.
    let mut anchor: Option<(usize, usize, usize)> = None; // (layer, col, row)
    for (l, rows) in layer_rows.iter().enumerate() {
        for (r, row) in rows.iter().enumerate() {
            for (c, &ch) in row.iter().enumerate() {
                if ch == '@' {
                    if anchor.is_some() {
                        bail!("pattern has multiple @ characters");
                    }
                    anchor = Some((l, c, r));
                }
            }
        }
    }
    let (at_layer, at_col, at_row) = anchor.context("pattern has no @ character")?;

    let mut layer_rows = layer_rows;

    // Parity padding is driven by the anchor layer, and applied uniformly to all
    // layers (every family member's live cells share the same parity grid).
    let (col_offset, row_offset) = offset(layer_types[at_layer], slot, at_col, at_row)?;

    if col_offset % 2 != 0 {
        for rows in &mut layer_rows {
            for row in rows.iter_mut() {
                row.insert(0, ' ');
            }
        }
    }
    if row_offset % 2 != 0 {
        for rows in &mut layer_rows {
            rows.insert(0, vec![]);
        }
    }

    for (l, rows) in layer_rows.iter().enumerate() {
        let pt = layer_types[l];
        for (r, row) in rows.iter().enumerate() {
            for (c, &ch) in row.iter().enumerate() {
                if ch != ' ' && is_dead_slot(&pt, c, r) {
                    bail!("character {ch:?} in dead slot at layer {l} ({c},{r})");
                }
            }
        }
    }

    let col_shift = if col_offset % 2 != 0 { 1 } else { 0 };
    let row_shift = if row_offset % 2 != 0 { 1 } else { 0 };

    let layers = layer_types
        .into_iter()
        .zip(layer_ts)
        .zip(layer_rows)
        .map(|((pattern_type, t), rows)| Layer {
            pattern_type,
            t,
            rows,
        })
        .collect();

    Ok(Pattern {
        layers,
        at_col: at_col + col_shift,
        at_row: at_row + row_shift,
        at_layer,
        annotations,
    })
}

// The 2x2 repeating grid of slots for each type of pattern
// H:
//   W.
//   RW
// H narrow:
//   ..
//   F.
// V:
//   F.
//   RW
// V narrow:
//   ..
//   W.
/// Offset to apply to a raw pattern to make the anchor appear in the right spot in the 2x2 slot grid
fn offset(
    pt: PatternType,
    slot: UnorientedSlot,
    anchor_col: usize,
    anchor_row: usize,
) -> anyhow::Result<(usize, usize)> {
    // `p_col`/`p_row` of 0 means the anchor should land on an even col/row, 1 means odd.
    let (p_col, p_row) = match (pt, slot) {
        (PatternType::H, UnorientedSlot::Room) => (0, 1),
        (PatternType::H, UnorientedSlot::Wall) => (1, 1),
        (PatternType::H, UnorientedSlot::Floor) => {
            bail!("H patterns have no floor slot; use H narrow")
        }
        (PatternType::HNarrow, UnorientedSlot::Floor) => (0, 1),
        (PatternType::HNarrow, _) => bail!("H narrow patterns only support floor anchors"),
        (PatternType::VNarrow, UnorientedSlot::Wall) => (0, 1),
        (PatternType::VNarrow, _) => bail!("V narrow patterns only support wall anchors"),
        (PatternType::V, UnorientedSlot::Room) => (0, 1),
        // (0,0) would also work, but we need to pick a canonical version:
        (PatternType::V, UnorientedSlot::Wall) => (1, 1),
        // Floor sits at (even, even) in V, which is already (0,0).
        (PatternType::V, UnorientedSlot::Floor) => (0, 0),
    };
    Ok(((anchor_col + p_col) % 2, (anchor_row + p_row) % 2))
}

// TODO: I think `offset` and `is_dead_slot` may be inconsistent with each other!

/// In H patterns the slot layout (per repeating tile) is:
///   (even, even) = W   (even, odd) = R   (odd, odd) = W   (odd, even) = dead
/// In V: same dead/live pattern as H.
/// In V narrow / H narrow: only (even col, odd row) is valid; everything else is dead.
fn is_dead_slot(pt: &PatternType, col: usize, row: usize) -> bool {
    match pt {
        PatternType::H | PatternType::V => col % 2 == 1 && row % 2 == 0,
        PatternType::VNarrow | PatternType::HNarrow => !(col % 2 == 0 && row % 2 == 1),
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
    let spec = parse_mesh_spec(rest).with_context(|| format!("invalid result: {s:?}"))?;
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

// ─── Character predicates ────────────────────────────────────────────────────

/// Returns true if `name` satisfies the structure predicate for pattern char `ch`.
/// `'='` (anchor-relative) is not handled here; the caller is responsible for it.
pub fn char_matches_name(ch: char, name: &str) -> bool {
    matches!(
        (ch, name),
        ('F', "floor")
            | ('W', "wall")
            | ('S', "stairs")
            | ('R', "railing")
            | ('O', "roof")
            | ('w', "wall" | "window" | "doorway")
    )
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
        check!(pat.layers[pat.at_layer].pattern_type == PatternType::H);
        // After parity adjustment, '@' is at (col=3, row=1) = XLoWall at cube (2,0,0).
        check!(pat.at_col == 3);
        check!(pat.at_row == 1);
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

    #[test]
    fn parse_floor_anchor_rule() {
        let input = "\
== rug: floor ==
V:
 @
 R
--> rug_with_room
--> rug_bare
";
        let file = parse(input).unwrap();
        check!(file.rules.len() == 1);
        let rule = &file.rules[0];
        check!(rule.structure_name == "rug");
        check!(rule.slot == UnorientedSlot::Floor);
        check!(rule.cases.len() == 2);

        let case = &rule.cases[0];
        let pat = case.pattern.as_ref().unwrap();
        check!(pat.layers[pat.at_layer].pattern_type == PatternType::V);
        // '@' at grid (0,0) → Floor. With (p_col, p_row)=(0,0) no padding needed.
        check!(pat.at_col == 0);
        check!(pat.at_row == 0);

        let checks = pat.relative_checks();
        // '@' at grid (0,0) → (0,1,0, Floor); 'R' at grid (0,1) → (0,0,0, Room).
        // cube_offset = (0,0,0) − (0,1,0) = (0,−1,0).
        let room_below = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::Floor,
            cube_offset: (0, -1, 0),
            dest_slot: AutotileRelSlot::Room,
        };
        check!(checks[&room_below] == 'R');
        check!(checks.len() == 1);
    }

    /// `V narrow` is wall-anchored: a stack of walls above/below the anchor.
    #[test]
    fn parse_v_narrow_wall_stack() {
        let input = "\
== wall: wall ==
V narrow:
 W

 @

 W
--> wall_mesh
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        check!(pat.layers[pat.at_layer].pattern_type == PatternType::VNarrow);
        // '@' wall at cube (0,-1,0); 'W' above at (0,0,0) → +Y; 'W' below at (0,-2,0) → -Y.
        let checks = pat.relative_checks();
        let above = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::ZLoWall,
            cube_offset: (0, 1, 0),
            dest_slot: AutotileRelSlot::ZLoWall,
        };
        let below = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::ZLoWall,
            cube_offset: (0, -1, 0),
            dest_slot: AutotileRelSlot::ZLoWall,
        };
        check!(checks[&above] == 'W');
        check!(checks[&below] == 'W');
        check!(checks.len() == 2);
    }

    /// `V narrow` only allows wall anchors.
    #[test]
    fn v_narrow_rejects_non_wall_anchor() {
        let input = "\
== rug: floor ==
V narrow:
 @
--> mesh
";
        check!(parse(input).is_err());
    }

    /// `H narrow` is floor-anchored: floors tiling horizontally (XZ plane).
    #[test]
    fn parse_h_narrow_floor_neighbors() {
        let input = "\
== floor: floor ==
H narrow:
 F @ F
--> floor_mesh
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        check!(pat.layers[pat.at_layer].pattern_type == PatternType::HNarrow);
        // '@' floor at cube (1,0,0); 'F' west at (0,0,0) → -X; 'F' east at (2,0,0) → +X.
        let checks = pat.relative_checks();
        let west = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::Floor,
            cube_offset: (-1, 0, 0),
            dest_slot: AutotileRelSlot::Floor,
        };
        let east = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::Floor,
            cube_offset: (1, 0, 0),
            dest_slot: AutotileRelSlot::Floor,
        };
        check!(checks[&west] == 'F');
        check!(checks[&east] == 'F');
        check!(checks.len() == 2);
    }

    /// `H narrow` only allows floor anchors.
    #[test]
    fn h_narrow_rejects_non_floor_anchor() {
        let input = "\
== wall: wall ==
H narrow:
 @
--> mesh
";
        check!(parse(input).is_err());
    }

    // ── Multi-layer rules ─────────────────────────────────────────────────────

    /// Two regular H layers stack in Y; leftmost is the higher level.
    #[test]
    fn parse_multi_layer_h_two_regular() {
        let input = "\
== stack: room ==
|H|H|:
|=|@|
--> stacked
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        check!(pat.layers.len() == 2);
        check!(pat.layers[0].t == 0);
        check!(pat.layers[1].t == -1);
        check!(pat.at_layer == 1);
        // '@' Room at Y=-1; '=' Room directly above at Y=0 → cube_offset (0,+1,0).
        let checks = pat.relative_checks();
        let above = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::Room,
            cube_offset: (0, 1, 0),
            dest_slot: AutotileRelSlot::Room,
        };
        check!(checks[&above] == '=');
        check!(checks.len() == 1);
    }

    /// `|H|H narrow|H|` — a floor separator between two Y-levels.
    #[test]
    fn parse_multi_layer_h_narrow_separator() {
        let input = "\
== stack: room ==
|H|H narrow|H|:
|=|F|@|
--> stacked
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        check!(pat.layers.len() == 3);
        check!(pat.layers[1].t == 0); // narrow shares the upper regular's coord
        check!(pat.layers[2].t == -1);
        let checks = pat.relative_checks();
        // Floor separator (boundary of the cube at Y=0) sits at +1 above the Y=-1 anchor.
        let floor = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::Room,
            cube_offset: (0, 1, 0),
            dest_slot: AutotileRelSlot::Floor,
        };
        let room_above = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::Room,
            cube_offset: (0, 1, 0),
            dest_slot: AutotileRelSlot::Room,
        };
        check!(checks[&floor] == 'F');
        check!(checks[&room_above] == '=');
        check!(checks.len() == 2);
    }

    /// `|V|V narrow|V|` — V layers stack in Z, separated by the depth-facing wall.
    #[test]
    fn parse_multi_layer_v_narrow_separator() {
        let input = "\
== stack: room ==
|V|V narrow|V|:
|=|W|@|
--> stacked
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        check!(pat.layers.len() == 3);
        check!(pat.layers[0].t == 0);
        check!(pat.layers[1].t == 0);
        check!(pat.layers[2].t == -1);
        let checks = pat.relative_checks();
        // Anchor Room at Z=-1; separator ZLoWall and the room copy sit at +1 in Z.
        let wall = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::Room,
            cube_offset: (0, 0, 1),
            dest_slot: AutotileRelSlot::ZLoWall,
        };
        let room_over = AutotileRelSlotOffset {
            origin_slot: AutotileRelSlot::Room,
            cube_offset: (0, 0, 1),
            dest_slot: AutotileRelSlot::Room,
        };
        check!(checks[&wall] == 'W');
        check!(checks[&room_over] == '=');
        check!(checks.len() == 2);
    }

    /// Layers must be a single family.
    #[test]
    fn multi_layer_mixed_family_rejected() {
        let input = "\
== stack: room ==
|H|V|:
|@|=|
--> m
";
        check!(parse(input).is_err());
    }

    /// Two consecutive narrow layers are not allowed.
    #[test]
    fn multi_layer_consecutive_narrow_rejected() {
        let input = "\
== stack: room ==
|H|H narrow|H narrow|H|:
|@| | |=|
--> m
";
        check!(parse(input).is_err());
    }

    // ── Annotation parsing ────────────────────────────────────────────────────

    /// The user's motivating example: `H: 1=stairs:90` annotates `'1'` as stairs
    /// facing 90° (relative to the rule's base orientation).
    #[test]
    fn parse_annotation_with_orientation() {
        let input = "\
== railing: wall ==
H: 1=stairs:90
 @1
--> stair_railing:90
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        check!(pat.annotations.len() == 1);
        let ann = pat.annotations.get(&'1').unwrap();
        check!(ann.name == "stairs");
        check!(ann.orientation == Some(90));
    }

    /// An annotation without a colon-angle means "any facing".
    #[test]
    fn parse_annotation_without_orientation() {
        let input = "\
== railing: wall ==
H: 1=railing
 @1
--> railing_mesh
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        let ann = pat.annotations.get(&'1').unwrap();
        check!(ann.name == "railing");
        check!(ann.orientation == None);
    }

    /// Multiple annotations on the same header line are all stored.
    #[test]
    fn parse_multiple_annotations() {
        let input = "\
== railing: wall ==
H: 1=stairs:90 2=stairs:0
 @1
--> stair_railing:90
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        check!(pat.annotations.len() == 2);
        check!(pat.annotations.get(&'1').unwrap().orientation == Some(90));
        check!(pat.annotations.get(&'2').unwrap().orientation == Some(0));
    }

    /// Patterns with no annotation still compile fine (empty map, not an error).
    #[test]
    fn parse_pattern_without_annotation_has_empty_map() {
        let input = "\
== wall: wall ==
H:
 @
--> mesh
";
        let file = parse(input).unwrap();
        let pat = file.rules[0].cases[0].pattern.as_ref().unwrap();
        check!(pat.annotations.is_empty());
    }
}
