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

        serialized.push_str("\n~~~~~~");
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

    for line in lines.lines() {
        if line.starts_with("~~~~~~") {
            y += 1;
            z = 0;
            continue;
        }
        if line.is_empty() {
            continue;
        }

        let chars: Vec<char> = line.chars().collect();
        let mut x: usize = 0; // Everything gets nonnegative indices, and that's fine.

        while x < chars.len() {
            if chars[x] != ' ' {
                let slot = match x % 2 {
                    0 => Slot::Room,
                    1 => Slot::ZWall,
                    _ => unreachable!(),
                };
                grid.set(Vector3i::new(x as i32 / 2, y, z), slot, f(chars[x], slot)?);
            }
            x += 1;
        }

        if let Some(next_line) = lines.lines().nth(1) {
            let chars: Vec<char> = next_line.chars().collect();
            let mut x = 0;
            while x < chars.len() {
                if chars[x] != ' ' {
                    let slot = match x % 2 {
                        0 => Slot::XWall,
                        1 => Slot::YFloor,
                        _ => unreachable!(),
                    };
                    grid.set(Vector3i::new(x as i32 / 2, y, z), slot, f(chars[x], slot)?);
                }
                x += 1;
            }
        }

        z += 1;
    }

    Ok(grid)
}
