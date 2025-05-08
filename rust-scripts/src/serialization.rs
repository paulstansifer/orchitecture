use godot::builtin::Vector3i;
use serde::{Deserialize, Serialize};

use crate::sparse3d::Slot;
use crate::wall_grid::Cell;
use crate::wall_grid::OfflineCell;
// HACK: we should be passed this information
const DESK_ID: i32 = 0;
const DOORWAY_ID: i32 = 1;
const FLOOR_ID: i32 = 2;
const WALL_ID: i32 = 3;
const RAILING_ID: i32 = 4;
const STAIRS_ID: i32 = 7;

pub fn cell_needs_extended(c: &Cell) -> bool {
    c.evaluation.is_some()
}

pub fn offline_cell_needs_extended(c: &OfflineCell) -> bool {
    c.evaluation.is_some()
}

pub fn serialize_slot(id: i32, slot: Slot) -> char {
    let idx = if slot == Slot::XWall { 0 } else { 1 };
    match id {
        DESK_ID => 'D',
        STAIRS_ID => 'S',
        WALL_ID => ['-', '|'][idx],
        FLOOR_ID => ['#', '#'][idx],
        DOORWAY_ID => ['=', ':'][idx],
        RAILING_ID => ['…', '⋮'][idx],
        _ => panic!(),
    }
}

pub fn deserialize(c: char) -> i32 {
    match c {
        'D' => DESK_ID,
        'S' => STAIRS_ID,
        '-' | '|' => WALL_ID,
        '#' => FLOOR_ID,
        '=' | ':' => DOORWAY_ID,
        '…' | '⋮' => RAILING_ID,
        _ => panic!(),
    }
}

fn extended_serialize_at<T: Serialize>(pos: Vector3i, slot: Slot, cell: &T) -> String {
    format!(
        "({},{},{},{})={}\n",
        pos.x,
        pos.y,
        pos.z,
        serde_json::to_string(&slot).unwrap(),
        serde_json::to_string(cell).unwrap()
    )
}

fn extended_deserialize_at<'a, T: Deserialize<'a>>(line: &'a str) -> (Vector3i, Slot, T) {
    let parts: Vec<&str> = line.split('=').collect();
    if parts.len() != 2 {
        panic!("Invalid extended serialization format");
    }

    let coords: Vec<&str> = parts[0]
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .collect();

    let pos = Vector3i::new(
        coords[0].parse::<i32>().unwrap(),
        coords[1].parse::<i32>().unwrap(),
        coords[2].parse::<i32>().unwrap(),
    );
    let slot: Slot = serde_json::from_str(coords[3]).unwrap();
    let cell: T = serde_json::from_str(parts[1]).unwrap();

    (pos, slot, cell)
}

pub fn serialize_sparse3d(
    grid: &crate::sparse3d::Sparse3D<Cell>,
    f: fn(&Cell, Slot) -> char,
    cell_needs_extended: fn(&Cell) -> bool,
) -> String {
    let mut serialized = String::new();
    let (min, max) = grid.bounding_box();
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                for slot in [Slot::Room, Slot::ZWall] {
                    if let Some(value) = grid.get(Vector3i::new(x, y, z), slot) {
                        serialized.push(f(value, slot))
                    } else {
                        serialized.push(' ');
                    }
                }
            }
            serialized.push_str("\n");
            for x in min.x..=max.x {
                for slot in [Slot::XWall, Slot::YFloor] {
                    if let Some(value) = grid.get(Vector3i::new(x, y, z), slot) {
                        serialized.push(f(value, slot));
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
    for (pos, slot, cell) in grid.iter() {
        if cell_needs_extended(cell) {
            let extended_ser = extended_serialize_at(pos - min, slot, cell);
            serialized.push_str(&extended_ser);
        }
    }

    serialized
}

pub fn deserialize_sparse3d<'a, T, F, E>(
    lines: &'a str,
    mut f: F,
) -> Result<crate::sparse3d::Sparse3D<T>, E>
where
    F: FnMut(char, Slot) -> Result<T, E>,
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
                grid.set(pos, slot, cell);
            }
            break;
        }

        let top_line = line.chars().collect::<Vec<_>>();
        let bottom_line = lines_it
            .next()
            .expect("Lines must come in pairs")
            .chars()
            .collect::<Vec<_>>();
        assert!(top_line.len() == bottom_line.len());

        for x in 0..top_line.len() / 2 {
            let zwall_ch = top_line[x * 2 + 1];
            let room_ch = top_line[x * 2];
            let floor_ch = bottom_line[x * 2 + 1];
            let xwall_ch = bottom_line[x * 2];

            for (ch, slot) in [
                (zwall_ch, Slot::ZWall),
                (room_ch, Slot::Room),
                (floor_ch, Slot::YFloor),
                (xwall_ch, Slot::XWall),
            ] {
                if ch != ' ' {
                    grid.set(Vector3i::new(x as i32, y, z), slot, f(ch, slot)?);
                }
            }
        }

        z += 1;
    }

    Ok(grid)
}
