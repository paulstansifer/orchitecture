use bevy::math::IVec3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::city::Cell;
use crate::materials::BuildMaterialId;
use crate::sparse3d::{Facing, RelSlot, RelSlotCoord, Slot, Sparse3D};
use crate::structure::{StructureId, StructureInfo};

pub fn serialize_slot(
    id: StructureId,
    slot: RelSlot,
    structures: &HashMap<StructureId, StructureInfo>,
) -> char {
    let structure_info = structures.get(&id).unwrap();
    match slot {
        RelSlot::XLoWall | RelSlot::XHiWall => structure_info.x_char.unwrap_or(' '),
        RelSlot::ZLoWall | RelSlot::ZHiWall | RelSlot::Floor | RelSlot::Ceiling | RelSlot::Room => {
            structure_info.z_char.unwrap_or(' ')
        }
    }
}

pub fn deserialize(c: char, structures: &HashMap<char, StructureId>) -> StructureId {
    *structures
        .get(&c)
        .unwrap_or_else(|| panic!("Unknown character for deserialization: {}", c))
}

fn extended_serialize_at<T: Serialize>(pos: IVec3, slot: RelSlot, cell: &T) -> String {
    format!(
        "({},{},{},{})={}\n",
        pos.x,
        pos.y,
        pos.z,
        serde_json::to_string(&slot).unwrap(),
        serde_json::to_string(cell).unwrap()
    )
}

fn extended_deserialize_at<'a, T: Deserialize<'a>>(line: &'a str) -> (IVec3, RelSlot, T) {
    let parts: Vec<&str> = line.splitn(2, '=').collect();
    if parts.len() != 2 {
        panic!("Invalid extended serialization format");
    }

    let coords: Vec<&str> = parts[0]
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .collect();

    let pos = IVec3::new(
        coords[0].parse::<i32>().unwrap(),
        coords[1].parse::<i32>().unwrap(),
        coords[2].parse::<i32>().unwrap(),
    );
    let slot: RelSlot = serde_json::from_str(coords[3]).unwrap();
    let cell: T = serde_json::from_str(parts[1]).unwrap();

    (pos, slot, cell)
}

pub fn serialize_sparse3d(
    grid: &crate::sparse3d::Sparse3D<Cell>,
    f: fn(&Cell, RelSlot, &HashMap<StructureId, StructureInfo>) -> char,
    structures: &HashMap<StructureId, StructureInfo>,
) -> String {
    let mut serialized = String::new();
    let (min, max) = grid.bounding_box();
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                for slot in [RelSlot::Room, RelSlot::ZLoWall] {
                    let loc = RelSlotCoord::new(x, y, z, slot);
                    if let Some(value) = grid.get(loc) {
                        serialized.push(f(value, slot, structures))
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
                        serialized.push(f(value, slot, structures));
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
            let extended_ser = extended_serialize_at(loc.cube - min, RelSlot::Room, cell);
            serialized.push_str(&extended_ser);
        }
    }

    serialized
}

pub fn deserialize_sparse3d<'a, T, F, E>(
    lines: &'a str,
    mut f: F,
    structures_by_char: &HashMap<char, StructureId>,
) -> Result<crate::sparse3d::Sparse3D<T>, E>
where
    F: FnMut(char, RelSlot, &HashMap<char, StructureId>) -> Result<T, E>,
    T: Deserialize<'a> + Serialize,
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
                let (pos, slot, cell) = extended_deserialize_at(line);
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

pub fn serialize(contents: &Sparse3D<Cell>, structures: &[StructureInfo]) -> Vec<u8> {
    let mut structures_by_id = HashMap::new();
    for (id, info) in structures.iter().enumerate() {
        structures_by_id.insert(StructureId(id as u32), info.clone());
    }
    serialize_sparse3d(
        contents,
        |cell, slot, structures| serialize_slot(cell.id, slot, structures),
        &structures_by_id,
    )
    .into_bytes()
}

pub fn save(contents: &Sparse3D<Cell>, structures: &[StructureInfo], path: &std::path::PathBuf) {
    std::fs::write(path, serialize(contents, structures)).unwrap();
}

pub fn load_from_str(content: &str, structures: &[StructureInfo]) -> Sparse3D<Cell> {
    let mut structures_by_char = HashMap::new();
    for (id, info) in structures.iter().enumerate() {
        if let Some(c) = info.x_char {
            structures_by_char.insert(c, StructureId(id as u32));
        }
        if let Some(c) = info.z_char {
            structures_by_char.insert(c, StructureId(id as u32));
        }
    }
    deserialize_sparse3d(
        content,
        |c, _slot, map| {
            let id = deserialize(c, map);
            Ok::<Cell, ()>(Cell {
                id,
                facing: Facing::NegX,
                evaluation: None,
                material: crate::city::Material::default(),
                build_material: BuildMaterialId::default(),
            })
        },
        &structures_by_char,
    )
    .unwrap()
}

pub fn load(path: &std::path::PathBuf, structures: &[StructureInfo]) -> Sparse3D<Cell> {
    let content = std::fs::read_to_string(path).unwrap();
    load_from_str(&content, structures)
}

#[cfg(test)]
mod tests {
    use assert2::check;
    use std::collections::HashMap;

    use crate::city::{Cell, Material};
    use crate::sparse3d::{Facing, RelSlot, RelSlotCoord};
    use crate::structure::{PlacementStyle, StructureEmbedding, StructureId, StructureInfo};

    use super::{deserialize, load_from_str, serialize};

    fn make_structures() -> Vec<StructureInfo> {
        vec![
            StructureInfo {
                name: "wall".to_string(),
                structure_type: crate::materials::StructureType::WallLike,
                placement_style: PlacementStyle::WallDrag,
                x_char: Some('|'),
                z_char: Some('-'),
                embedding: StructureEmbedding {
                    tall: 0.0,
                    passable: 0.0,
                    decorative: 0.0,
                    striated: 0.0,
                },
                furniture: None,
            },
            StructureInfo {
                name: "floor".to_string(),
                structure_type: crate::materials::StructureType::GroundFloorLike,
                placement_style: PlacementStyle::FloorDrag,
                x_char: Some('/'),
                z_char: Some('.'),
                embedding: StructureEmbedding {
                    tall: 0.0,
                    passable: 1.0,
                    decorative: 0.0,
                    striated: 0.0,
                },
                furniture: None,
            },
        ]
    }

    fn cell(id: u32) -> Cell {
        Cell {
            id: StructureId(id),
            facing: Facing::NegX,
            evaluation: None,
            material: Material::default(),
            build_material: crate::materials::BuildMaterialId::default(),
        }
    }

    #[test]
    fn round_trip_single_zwall() {
        let structures = make_structures();
        let mut grid = super::super::sparse3d::Sparse3D::new();
        let loc = RelSlotCoord::new(0, 0, 0, RelSlot::ZLoWall);
        grid.set(loc, cell(0));

        let bytes = serialize(&grid, &structures);
        let restored = load_from_str(std::str::from_utf8(&bytes).unwrap(), &structures);

        check!(restored.size() == 1);
        check!(restored.get(loc).map(|c| c.id) == Some(StructureId(0)));
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

        let bytes = serialize(&grid, &structures);
        let restored = load_from_str(std::str::from_utf8(&bytes).unwrap(), &structures);

        check!(restored.size() == 3);
        check!(restored.get(zwall).map(|c| c.id) == Some(StructureId(0)));
        check!(restored.get(xwall).map(|c| c.id) == Some(StructureId(0)));
        check!(restored.get(floor).map(|c| c.id) == Some(StructureId(1)));
    }

    #[test]
    fn round_trip_multi_level() {
        let structures = make_structures();
        let mut grid = super::super::sparse3d::Sparse3D::new();
        let lo = RelSlotCoord::new(0, 0, 0, RelSlot::ZLoWall);
        let hi = RelSlotCoord::new(0, 2, 0, RelSlot::ZLoWall);
        grid.set(lo, cell(0));
        grid.set(hi, cell(0));

        let bytes = serialize(&grid, &structures);
        let restored = load_from_str(std::str::from_utf8(&bytes).unwrap(), &structures);

        check!(restored.get(lo).is_some());
        check!(restored.get(hi).is_some());
    }

    #[test]
    #[should_panic(expected = "Unknown character for deserialization")]
    fn deserialize_panics_on_unknown_char() {
        let map: HashMap<char, StructureId> = HashMap::new();
        deserialize('X', &map);
    }

    #[test]
    fn deserialize_returns_correct_id_for_known_char() {
        let mut map = HashMap::new();
        map.insert('A', StructureId(7));
        check!(deserialize('A', &map) == StructureId(7));
    }
}
