use anyhow::Result;
use bevy::math::IVec3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::city::Cell;
use crate::eorf::{EorfId, EorfInfo};
use crate::materials::BuildMaterialId;
use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, Slot, Sparse3D};

pub fn serialize_slot(
    id: EorfId,
    slot: RelSlot,
    structures: &HashMap<EorfId, EorfInfo>,
) -> Result<char> {
    let structure_info = structures
        .get(&id)
        .ok_or_else(|| anyhow::anyhow!("Structure not found: {:?}", id))?;
    let ch = match slot {
        RelSlot::XLoWall | RelSlot::XHiWall => structure_info.x_char.unwrap_or(' '),
        RelSlot::ZLoWall | RelSlot::ZHiWall | RelSlot::Floor | RelSlot::Ceiling | RelSlot::Room => {
            structure_info.z_char.unwrap_or(' ')
        }
    };
    Ok(ch)
}

pub fn deserialize(c: char, structures: &HashMap<char, EorfId>) -> Result<EorfId> {
    structures
        .get(&c)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Unknown character for deserialization: {}", c))
}

fn extended_serialize_at<T: Serialize>(pos: IVec3, slot: RelSlot, cell: &T) -> Result<String> {
    Ok(format!(
        "({},{},{},{})={}\n",
        pos.x,
        pos.y,
        pos.z,
        serde_json::to_string(&slot)?,
        serde_json::to_string(cell)?
    ))
}

fn extended_deserialize_at<'a, T: Deserialize<'a>>(line: &'a str) -> Result<(IVec3, RelSlot, T)> {
    let parts: Vec<&str> = line.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(anyhow::anyhow!("Invalid extended serialization format"));
    }

    let coords: Vec<&str> = parts[0]
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .collect();

    let pos = IVec3::new(
        coords[0]
            .parse::<i32>()
            .map_err(|e| anyhow::anyhow!("Failed to parse x coordinate: {}", e))?,
        coords[1]
            .parse::<i32>()
            .map_err(|e| anyhow::anyhow!("Failed to parse y coordinate: {}", e))?,
        coords[2]
            .parse::<i32>()
            .map_err(|e| anyhow::anyhow!("Failed to parse z coordinate: {}", e))?,
    );
    let slot: RelSlot = serde_json::from_str(coords[3])
        .map_err(|e| anyhow::anyhow!("Failed to parse slot on line '{}': {}", line, e))?;
    let cell: T = serde_json::from_str(parts[1])
        .map_err(|e| anyhow::anyhow!("Failed to parse cell on line '{}': {}", line, e))?;

    Ok((pos, slot, cell))
}

pub fn serialize_sparse3d(
    grid: &crate::sparse3d::Sparse3D<Cell>,
    f: fn(&Cell, RelSlot, &HashMap<EorfId, EorfInfo>) -> Result<char>,
    structures: &HashMap<EorfId, EorfInfo>,
) -> Result<String> {
    let mut serialized = String::new();
    let (min, max) = grid.bounding_box();
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                for slot in [RelSlot::Room, RelSlot::ZLoWall] {
                    let loc = RelSlotCoord::new(x, y, z, slot);
                    if let Some(value) = grid.get(loc) {
                        serialized.push(f(value, slot, structures)?);
                    } else {
                        serialized.push(' ');
                    }
                }
            }
            serialized.push('\n');
            for x in min.x..=max.x {
                for slot in [RelSlot::XLoWall, RelSlot::Floor] {
                    let loc = RelSlotCoord::new(x, y, z, slot);
                    if let Some(value) = grid.get(loc) {
                        serialized.push(f(value, slot, structures)?);
                    } else {
                        serialized.push(' ');
                    }
                }
            }
            serialized.push('\n');
        }

        serialized.push_str("~~~~~\n");
    }
    serialized.push_str("~*~*~\n");
    for (loc, cell) in grid.iter() {
        if loc.slot == Slot::Room {
            let extended_ser = extended_serialize_at(loc.cube - min, RelSlot::Room, cell)?;
            serialized.push_str(&extended_ser);
        }
    }

    Ok(serialized)
}

pub fn deserialize_sparse3d<'a, T, F, E>(
    lines: &'a str,
    mut f: F,
    structures_by_char: &HashMap<char, EorfId>,
) -> Result<crate::sparse3d::Sparse3D<T>, E>
where
    F: FnMut(char, RelSlot, &HashMap<char, EorfId>) -> Result<T, E>,
    T: Deserialize<'a> + Serialize,
    E: From<anyhow::Error>,
{
    let mut grid = crate::sparse3d::Sparse3D::new();
    let mut y = 0;
    let mut z = 0;

    let mut lines_it = lines.lines();

    loop {
        let line = match lines_it.next() {
            Some(line) => line,
            None => break,
        };

        if line.starts_with("~~~~~") {
            y += 1;
            z = 0;
            continue;
        }

        if line.starts_with("~*~*~") {
            // Start of evaluated cells data
            for line in lines_it {
                let (pos, slot, cell) = extended_deserialize_at(line).map_err(E::from)?;
                let loc = RelSlotCoord::new(pos.x, pos.y, pos.z, slot);
                grid.set(loc, cell);
            }
            break;
        }

        let mut top_line = line.chars().collect::<Vec<_>>();
        let mut bottom_line = lines_it.next().unwrap_or("").chars().collect::<Vec<_>>();
        if top_line.contains(&'#') || top_line.contains(&'|') {
            panic!("Invalid room/zwall line: '{:?}'", top_line);
        }
        if bottom_line.contains(&'V') || bottom_line.contains(&'-') {
            panic!("Invalid room/xwall line: '{:?}'", bottom_line);
        }

        while top_line.len() < bottom_line.len() {
            top_line.push(' ');
        }
        while bottom_line.len() < top_line.len() {
            bottom_line.push(' ');
        }

        for x in 0..top_line.len() / 2 {
            let room_ch = top_line[x * 2];
            let zwall_ch = top_line[x * 2 + 1];
            let xwall_ch = bottom_line[x * 2];
            let floor_ch = bottom_line[x * 2 + 1];

            for (ch, slot) in [
                (zwall_ch, RelSlot::ZLoWall),
                (room_ch, RelSlot::Room),
                (floor_ch, RelSlot::Floor),
                (xwall_ch, RelSlot::XLoWall),
            ] {
                if ch != ' ' {
                    let loc = RelSlotCoord::new(x as i32, y, z, slot);
                    grid.set(loc, f(ch, slot, structures_by_char)?);
                }
            }
        }

        z += 1;
    }

    Ok(grid)
}

pub fn serialize(contents: &Sparse3D<Cell>, structures: &[EorfInfo]) -> Result<Vec<u8>> {
    let mut structures_by_id = HashMap::new();
    for (id, info) in structures.iter().enumerate() {
        structures_by_id.insert(EorfId(id as u32), info.clone());
    }
    let serialized = serialize_sparse3d(
        contents,
        |cell, slot, structures| serialize_slot(cell.id, slot, structures),
        &structures_by_id,
    )?;
    Ok(serialized.into_bytes())
}

pub fn save(
    contents: &Sparse3D<Cell>,
    structures: &[EorfInfo],
    path: &std::path::PathBuf,
) -> Result<()> {
    let bytes = serialize(contents, structures)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

pub fn load_from_str(content: &str, structures: &[EorfInfo]) -> Result<Sparse3D<Cell>> {
    let mut structures_by_char = HashMap::new();
    for (id, info) in structures.iter().enumerate() {
        if let Some(c) = info.x_char {
            structures_by_char.insert(c, EorfId(id as u32));
        }
        if let Some(c) = info.z_char {
            structures_by_char.insert(c, EorfId(id as u32));
        }
    }
    deserialize_sparse3d(
        content,
        |c, _slot, map| {
            let id = deserialize(c, map)?;
            Ok::<Cell, anyhow::Error>(Cell {
                id,
                facing: Facing::NegX,
                evaluation: None,
                build_material: BuildMaterialId::default(),
            })
        },
        &structures_by_char,
    )
}

pub fn load(path: &std::path::PathBuf, structures: &[EorfInfo]) -> Result<Sparse3D<Cell>> {
    let content = std::fs::read_to_string(path)?;
    load_from_str(&content, structures)
}

#[cfg(test)]
mod tests {
    use assert2::check;
    use std::collections::HashMap;

    use crate::city::Cell;
    use crate::eorf::{EorfId, EorfInfo, PlacementStyle, StructureEmbedding};
    use crate::sparse3d::{Facing, RelSlot, RelSlotCoord};

    use super::{deserialize, load_from_str, serialize};

    fn make_structures() -> Vec<EorfInfo> {
        vec![
            EorfInfo {
                name: "wall".to_string(),
                placement_style: PlacementStyle::WallDrag,
                x_char: Some('|'),
                z_char: Some('-'),
                embedding: StructureEmbedding {
                    tall: 0.0,
                    passable: 0.0,
                    decorative: 0.0,
                    striated: 0.0,
                },
                kind: crate::eorf::FurnitureOrElement::Element(
                    crate::materials::ElementType::WallLike,
                ),
                vantage_evaluated: false,
            },
            EorfInfo {
                name: "floor".to_string(),
                placement_style: PlacementStyle::FloorDrag,
                x_char: Some('/'),
                z_char: Some('.'),
                embedding: StructureEmbedding {
                    tall: 0.0,
                    passable: 1.0,
                    decorative: 0.0,
                    striated: 0.0,
                },
                kind: crate::eorf::FurnitureOrElement::Element(
                    crate::materials::ElementType::GroundFloorLike,
                ),
                vantage_evaluated: false,
            },
        ]
    }

    fn cell(id: u32) -> Cell {
        Cell {
            id: EorfId(id),
            facing: Facing::NegX,
            evaluation: None,
            build_material: crate::materials::BuildMaterialId::default(),
        }
    }

    #[test]
    fn extended_section_parses_constrained_score_variants() {
        use crate::city::{ConstrainedScore, VantageEvaluation};
        use std::collections::HashMap;

        let structures_by_char: HashMap<char, EorfId> = HashMap::new();
        let content = "  \n  \n~~~~~\n~*~*~\n(0,0,0,\"Room\")={\"id\":0,\"facing\":\"NegX\",\"evaluation\":{\"coherence\":{\"at_most\":0.5},\"interest\":{\"at_least\":0.2}},\"build_material\":0}\n";

        let grid = super::deserialize_sparse3d::<Cell, _, anyhow::Error>(
            content,
            |c, _slot, map| deserialize(c, map).map(|id| cell_with_id(id)),
            &structures_by_char,
        )
        .unwrap();

        let loc = RelSlotCoord::new(0, 0, 0, RelSlot::Room);
        let evaluation = grid.get(loc).unwrap().evaluation.clone();
        check!(
            evaluation
                == Some(VantageEvaluation {
                    coherence: Some(ConstrainedScore::AtMost { at_most: 0.5 }),
                    interest: Some(ConstrainedScore::AtLeast { at_least: 0.2 }),
                })
        );
    }

    fn cell_with_id(id: EorfId) -> Cell {
        Cell {
            id,
            facing: Facing::NegX,
            evaluation: None,
            build_material: crate::materials::BuildMaterialId::default(),
        }
    }

    #[test]
    fn round_trip_single_zwall() {
        let structures = make_structures();
        let mut grid = super::super::sparse3d::Sparse3D::new();
        let loc = RelSlotCoord::new(0, 0, 0, RelSlot::ZLoWall);
        grid.set(loc, cell(0));

        let bytes = serialize(&grid, &structures).unwrap();
        let restored = load_from_str(std::str::from_utf8(&bytes).unwrap(), &structures).unwrap();

        check!(restored.size() == 1);
        check!(restored.get(loc).map(|c| c.id) == Some(EorfId(0)));
    }

    #[test]
    fn round_trip_multiple_slots() {
        let structures = make_structures();
        let mut grid = super::super::sparse3d::Sparse3D::new();
        let zwall = RelSlotCoord::new(0, 0, 0, RelSlot::ZLoWall);
        let xwall = RelSlotCoord::new(1, 0, 0, RelSlot::XLoWall);
        let floor = RelSlotCoord::new(0, 0, 0, RelSlot::Floor);
        grid.set(zwall, cell(0));
        grid.set(xwall, cell(0));
        grid.set(floor, cell(1));

        let bytes = serialize(&grid, &structures).unwrap();
        let restored = load_from_str(std::str::from_utf8(&bytes).unwrap(), &structures).unwrap();

        check!(restored.size() == 3);
        check!(restored.get(zwall).map(|c| c.id) == Some(EorfId(0)));
        check!(restored.get(xwall).map(|c| c.id) == Some(EorfId(0)));
        check!(restored.get(floor).map(|c| c.id) == Some(EorfId(1)));
    }

    #[test]
    fn round_trip_multi_level() {
        let structures = make_structures();
        let mut grid = super::super::sparse3d::Sparse3D::new();
        let lo = RelSlotCoord::new(0, 0, 0, RelSlot::ZLoWall);
        let hi = RelSlotCoord::new(0, 2, 0, RelSlot::ZLoWall);
        grid.set(lo, cell(0));
        grid.set(hi, cell(0));

        let bytes = serialize(&grid, &structures).unwrap();
        let restored = load_from_str(std::str::from_utf8(&bytes).unwrap(), &structures).unwrap();

        check!(restored.get(lo).is_some());
        check!(restored.get(hi).is_some());
    }

    #[test]
    fn deserialize_returns_error_on_unknown_char() {
        let map: HashMap<char, EorfId> = HashMap::new();
        let result = deserialize('X', &map);
        check!(result.is_err());
    }

    #[test]
    fn deserialize_returns_correct_id_for_known_char() {
        let mut map = HashMap::new();
        map.insert('A', EorfId(7));
        check!(deserialize('A', &map).unwrap() == EorfId(7));
    }
}
