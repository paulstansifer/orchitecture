use godot::builtin::Vector3i;

use crate::sparse3d::Slot;

// HACK: we should be passed this information
const DESK_ID: i32 = 0;
const DOORWAY_ID: i32 = 1;
const FLOOR_ID: i32 = 2;
const WALL_ID: i32 = 3;
const RAILING_ID: i32 = 4;

pub fn serialize_slot(id: i32, slot: Slot) -> char {
    let idx = if slot == Slot::XWall { 0 } else { 1 };
    match id {
        DESK_ID => 'D',
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
        '-' | '|' => WALL_ID,
        '#' => FLOOR_ID,
        '=' | ':' => DOORWAY_ID,
        '…' | '⋮' => RAILING_ID,
        _ => panic!(),
    }
}

pub fn serialize_sparse3d<T>(
    grid: &crate::sparse3d::Sparse3D<T>,
    f: fn(&T, Slot) -> char,
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
                        serialized.push(f(value, slot))
                    } else {
                        serialized.push(' ');
                    }
                }
            }
            serialized.push_str("\n");
        }

        serialized.push_str("~~~~~~\n");
    }

    serialized
}

pub fn deserialize_sparse3d<T, F, E>(
    lines: &str,
    mut f: F,
) -> Result<crate::sparse3d::Sparse3D<T>, E>
where
    F: FnMut(char, Slot) -> Result<T, E>,
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

        if line.starts_with("~~~~~~") {
            y += 1;
            z = 0;
            continue;
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
