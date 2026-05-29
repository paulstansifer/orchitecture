use bevy::math::IVec3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::sparse3d::{Facing, RelSlot, SlotLocation, Sparse3D};
use crate::structure::{StructureId, StructureInfo};
use crate::wall_grid::Cell;

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
                    let loc = SlotLocation::new(x, y, z, slot);
                    if let Some(value) = grid.get(loc) {
                        serialized.push(f(value, slot, structures))
                    } else {
                        serialized.push(' ');
                    }
                }
            }
            serialized.push_str("\n");
            for x in min.x..=max.x {
                for slot in [RelSlot::XLoWall, RelSlot::Floor] {
                    let loc = SlotLocation::new(x, y, z, slot);
                    if let Some(value) = grid.get(loc) {
                        serialized.push(f(value, slot, structures));
                    } else {
                        serialized.push(' ');
                    }
                }
            }
            serialized.push_str("\n");
        }

        serialized.push_str("~~~~~\n");
    }
    serialized.push_str("~*~*~\n");
    for (loc, cell) in grid.iter() {
        if loc.rel_slot == RelSlot::Room {
            let extended_ser = extended_serialize_at(loc.cube - min, loc.rel_slot, cell);
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
                let loc = SlotLocation::new(pos.x, pos.y, pos.z, slot);
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
                    let loc = SlotLocation::new(x as i32, y, z, slot);
                    grid.set(loc, f(ch, slot, structures_by_char)?);
                }
            }
        }

        z += 1;
    }

    Ok(grid)
}

pub fn save(contents: &Sparse3D<Cell>, structures: &[StructureInfo], path: &std::path::PathBuf) {
    let mut structures_by_id = HashMap::new();
    for (id, info) in structures.iter().enumerate() {
        structures_by_id.insert(StructureId(id as u32), info.clone());
    }
    let serialized = serialize_sparse3d(
        contents,
        |cell, slot, structures| serialize_slot(cell.id, slot, structures),
        &structures_by_id,
    );
    std::fs::write(path, serialized).unwrap();
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
