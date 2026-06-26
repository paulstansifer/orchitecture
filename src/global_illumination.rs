use std::collections::{BinaryHeap, HashMap};

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

use crate::gi_material::{GiMaterial, GI_INTENSITY};
use crate::sparse3d::{Slot, SlotCoord, Sparse3D};
use crate::structure::{StructureInfo, StructureList};
use crate::wall_grid::{Cell, MaterialAssets, WallGrid};

const FALLOFF: f32 = 0.40;

/// Heap entry ordered by light level (higher = higher priority) with cube
/// coordinates as a tiebreaker so that `Ord` and `PartialEq` are consistent.
#[derive(PartialEq, Eq)]
struct HeapEntry {
    level_bits: u32,
    cube: IVec3,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.level_bits
            .cmp(&other.level_bits)
            .then_with(|| self.cube.x.cmp(&other.cube.x))
            .then_with(|| self.cube.y.cmp(&other.cube.y))
            .then_with(|| self.cube.z.cmp(&other.cube.z))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Returns true if no Floor cell exists above `cube` within the grid's bounding box.
fn has_sky_above(contents: &Sparse3D<Cell>, cube: IVec3, top_y: i32) -> bool {
    for y in (cube.y + 1)..=top_y {
        if contents
            .get(SlotCoord {
                cube: IVec3::new(cube.x, y, cube.z),
                slot: Slot::Floor,
            })
            .is_some()
        {
            return false;
        }
    }
    true
}

/// Returns how much light passes through the boundary between adjacent cubes `from` and `to`.
/// 0.0 = fully blocked, 0.5 = window/doorway, 1.0 = open air or transparent structure.
fn boundary_transmission(
    contents: &Sparse3D<Cell>,
    structures: &[StructureInfo],
    from: IVec3,
    to: IVec3,
) -> f32 {
    let delta = to - from;
    let boundary_loc = if delta == IVec3::X {
        SlotCoord {
            cube: to,
            slot: Slot::XLoWall,
        }
    } else if delta == IVec3::NEG_X {
        SlotCoord {
            cube: from,
            slot: Slot::XLoWall,
        }
    } else if delta == IVec3::Y {
        SlotCoord {
            cube: to,
            slot: Slot::Floor,
        }
    } else if delta == IVec3::NEG_Y {
        SlotCoord {
            cube: from,
            slot: Slot::Floor,
        }
    } else if delta == IVec3::Z {
        SlotCoord {
            cube: to,
            slot: Slot::ZLoWall,
        }
    } else {
        SlotCoord {
            cube: from,
            slot: Slot::ZLoWall,
        }
    };

    match contents.get(boundary_loc) {
        None => 1.0,
        Some(cell) => match structures[cell.id.as_usize()].name.as_str() {
            "wall" | "floor" => 0.0,
            "window" | "doorway" => 0.5,
            _ => 1.0,
        },
    }
}

/// Flood-fills sky illuminance from sky-visible cube voxels.
///
/// Seeds all cubes within the grid bounding box (expanded by 1) that have an
/// unobstructed vertical view of the sky, then propagates outward using a
/// max-priority queue so the brightest frontier is always settled first.
///
/// Each hop multiplies the current level by `(1 - FALLOFF) * transmission`:
/// - open air: × 0.85
/// - window/doorway: × 0.85 × 0.5 = 0.425
/// - wall/floor: blocked (× 0.0)
///
/// Returns a map from cube coordinate → light level in [0.0, 1.0].
pub fn compute_sky_illuminance(
    contents: &Sparse3D<Cell>,
    structures: &[StructureInfo],
) -> HashMap<IVec3, f32> {
    if contents.size() == 0 {
        return HashMap::new();
    }

    let (min_cube, max_cube) = contents.bounding_box();
    // Expand by 1 to include exterior cubes adjacent to windows.
    let search_min = min_cube - IVec3::ONE;
    let search_max = max_cube + IVec3::ONE;
    let top_y = search_max.y;

    let mut illuminance: HashMap<IVec3, f32> = HashMap::new();
    // BinaryHeap is a max-heap. f32::to_bits() preserves order for non-negative floats.
    let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::new();

    for z in search_min.z..=search_max.z {
        for y in search_min.y..=search_max.y {
            for x in search_min.x..=search_max.x {
                let cube = IVec3::new(x, y, z);
                if has_sky_above(contents, cube, top_y) {
                    illuminance.insert(cube, 1.0);
                    heap.push(HeapEntry {
                        level_bits: 1.0f32.to_bits(),
                        cube,
                    });
                }
            }
        }
    }

    const DIRS: [IVec3; 6] = [
        IVec3::X,
        IVec3::NEG_X,
        IVec3::Y,
        IVec3::NEG_Y,
        IVec3::Z,
        IVec3::NEG_Z,
    ];

    while let Some(HeapEntry { level_bits, cube }) = heap.pop() {
        let level = f32::from_bits(level_bits);
        // Discard stale heap entries superseded by a higher-level path.
        if illuminance.get(&cube).copied().unwrap_or(0.0) > level {
            continue;
        }

        for dir in DIRS {
            let neighbor = cube + dir;
            if neighbor.x < search_min.x
                || neighbor.x > search_max.x
                || neighbor.y < search_min.y
                || neighbor.y > search_max.y
                || neighbor.z < search_min.z
                || neighbor.z > search_max.z
            {
                continue;
            }

            let transmission = boundary_transmission(contents, structures, cube, neighbor);
            if transmission == 0.0 {
                continue;
            }

            let new_level = level * (1.0 - FALLOFF) * transmission;
            let current = illuminance.get(&neighbor).copied().unwrap_or(0.0);
            if new_level > current {
                illuminance.insert(neighbor, new_level);
                heap.push(HeapEntry {
                    level_bits: new_level.to_bits(),
                    cube: neighbor,
                });
            }
        }
    }

    // for (_, v) in &illuminance {
    //     print!("{v:.2} ");
    // }
    // println!();

    illuminance
}

/// Boundary transmission on a cube's three low faces (−X, −Y, −Z), i.e. the
/// `XLoWall`, `Floor` and `ZLoWall` slots. These are the faces shared with the
/// neighbors at `cube - X`, `cube - Y`, `cube - Z`, so storing only the low faces
/// per cube covers every boundary exactly once.
fn low_face_transmissions(
    contents: &Sparse3D<Cell>,
    structures: &[StructureInfo],
    cube: IVec3,
) -> [f32; 3] {
    [
        boundary_transmission(contents, structures, cube - IVec3::X, cube),
        boundary_transmission(contents, structures, cube - IVec3::Y, cube),
        boundary_transmission(contents, structures, cube - IVec3::Z, cube),
    ]
}

/// Packs per-cube illuminance and low-face transmissions into a 3D `Rgba8Unorm`
/// texture, one voxel per cube (size `(Rx, Ry, Rz)`).
///
/// Channels: `R` = illuminance, `G`/`B`/`A` = transmission on the cube's −X / −Y / −Z
/// faces (0 = wall, 0.5 = window/doorway, 1 = open). Sampled with `textureLoad` and
/// interpolated manually in [shaders/gi.wgsl], so the format need not be filterable.
///
/// `bounds` is the inclusive range `(min_cube, max_cube)` in world grid coordinates;
/// voxel `(x, y, z)` corresponds to world cube `min_cube + (x, y, z)`.
pub fn gi_to_image(
    illuminance: &HashMap<IVec3, f32>,
    contents: &Sparse3D<Cell>,
    structures: &[StructureInfo],
    (min_cube, max_cube): (IVec3, IVec3),
) -> Image {
    let rx = (max_cube.x - min_cube.x + 1) as u32;
    let ry = (max_cube.y - min_cube.y + 1) as u32;
    let rz = (max_cube.z - min_cube.z + 1) as u32;

    const BYTES_PER_PIXEL: usize = 4;
    let mut data = vec![0u8; (rx * ry * rz) as usize * BYTES_PER_PIXEL];

    let quantize = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;

    for z in 0..rz {
        for y in 0..ry {
            for x in 0..rx {
                let world_cube = min_cube + IVec3::new(x as i32, y as i32, z as i32);
                let level = illuminance.get(&world_cube).copied().unwrap_or(0.0);
                let [tx, ty, tz] = low_face_transmissions(contents, structures, world_cube);
                let pixel = [quantize(level), quantize(tx), quantize(ty), quantize(tz)];

                let pixel_idx = (x + y * rx + z * rx * ry) as usize;
                data[pixel_idx * BYTES_PER_PIXEL..pixel_idx * BYTES_PER_PIXEL + 4]
                    .copy_from_slice(&pixel);
            }
        }
    }

    Image {
        data: Some(data),
        texture_descriptor: TextureDescriptor {
            label: None,
            size: Extent3d {
                width: rx,
                height: ry,
                depth_or_array_layers: rz,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D3,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        },
        asset_usage: RenderAssetUsages::RENDER_WORLD,
        ..default()
    }
}

/// Bevy system: recomputes the GI volume texture and rebinds it (along with the
/// volume bounds) on every building material, run whenever `WallGrid` changes.
pub fn update_global_illumination(
    wall_grid: Res<WallGrid>,
    structure_list: Res<StructureList>,
    material_assets: Res<MaterialAssets>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GiMaterial>>,
) {
    let contents = &wall_grid.contents;
    if contents.size() == 0 {
        return;
    }

    let structure_infos: Vec<_> = structure_list
        .structures
        .iter()
        .map(|s| s.info.clone())
        .collect();
    let illuminance = compute_sky_illuminance(contents, &structure_infos);
    if illuminance.is_empty() {
        return;
    }

    let (min_cube, max_cube) = contents.bounding_box();

    let image = gi_to_image(
        &illuminance,
        contents,
        &structure_infos,
        (min_cube, max_cube),
    );
    let handle = images.add(image);

    let resolution = (max_cube - min_cube + IVec3::ONE).as_vec3();
    let min_cube = min_cube.as_vec3();

    // All building materials share the one GI volume; rebind it on each.
    for material_handle in material_assets.all() {
        if let Some(material) = materials.get_mut(material_handle) {
            material.extension.min_cube = min_cube.extend(GI_INTENSITY);
            material.extension.resolution = resolution.extend(0.0);
            material.extension.gi_tex = handle.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use assert2::check;
    use bevy::math::IVec3;

    use crate::build_helpers::Builder;
    use crate::sparse3d::RelSlot;
    use crate::structure::load_structure_info;

    use super::compute_sky_illuminance;

    /// A sealed box has opaque walls and a roof on all sides.  No sky light
    /// should reach any interior cube.
    #[test]
    fn test_sealed_box_has_no_interior_illuminance() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        let contents = builder.get();

        let illuminance = compute_sky_illuminance(&contents, &structure_infos);

        // The centre of the box is fully enclosed; it should receive no light.
        let interior = IVec3::new(1, 1, 1);
        check!(illuminance.get(&interior).copied().unwrap_or(0.0) == 0.0);
    }

    /// Punching a single window into the X-low face of the sealed box should
    /// let sky light enter: the cell just behind the window and the one further
    /// in must both have positive illuminance.
    #[test]
    fn test_window_admits_light() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        // Replace one wall cell with a window on the X-low face.
        builder.build_plane(
            IVec3::new(0, 1, 1),
            IVec3::new(0, 1, 1),
            RelSlot::XLoWall,
            Some("window"),
        );
        let contents = builder.get();

        let illuminance = compute_sky_illuminance(&contents, &structure_infos);

        // The cell immediately inside the window.
        let at_window = IVec3::new(0, 1, 1);
        check!(illuminance.get(&at_window).copied().unwrap_or(0.0) > 0.0);

        // One step deeper into the box.
        let deeper = IVec3::new(1, 1, 1);
        check!(illuminance.get(&deeper).copied().unwrap_or(0.0) > 0.0);
    }

    /// The per-cube low-face transmissions feed the shader's adjacency-aware blend:
    /// a solid wall reports 0 (no bleed), a window reports 0.5.
    #[test]
    fn test_low_face_transmissions() {
        use super::low_face_transmissions;

        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(2, 2, 2));
        builder.build_plane(
            IVec3::new(0, 1, 1),
            IVec3::new(0, 1, 1),
            RelSlot::XLoWall,
            Some("window"),
        );
        let contents = builder.get();

        // Cube (0,1,1): its −X face is now a window (0.5); its −Y/−Z faces are open
        // interior air (1.0) since neighbors within the box share no wall there.
        let [tx, _ty, _tz] =
            low_face_transmissions(&contents, &structure_infos, IVec3::new(0, 1, 1));
        check!(tx == 0.5);

        // Cube (1,1,1): its −X face is the wall shared with (0,1,1)? No — that
        // boundary (XLoWall of cube 1) is open interior, so 1.0. Confirm an actual
        // box wall reads 0: cube (0,1,1)'s −Z face is the box's Z-low wall.
        let [_, _, tz_wall] =
            low_face_transmissions(&contents, &structure_infos, IVec3::new(1, 1, 0));
        check!(tz_wall == 0.0);
    }
}
