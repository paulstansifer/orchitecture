use std::collections::{BinaryHeap, HashMap};

use bevy::asset::RenderAssetUsages;
use bevy::light::IrradianceVolume;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

use crate::sparse3d::{Slot, SlotCoord, Sparse3D};
use crate::structure::{StructureInfo, StructureList};
use crate::wall_grid::{Cell, WallGrid};

const FALLOFF: f32 = 0.40;
const IRRADIANCE_VOLUME_INTENSITY: f32 = 1800.0;

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

#[derive(Component)]
pub struct GlobalIlluminationVolume;

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

/// Packs a flood-filled illuminance map into a Bevy `Image` suitable for
/// `IrradianceVolume::voxels`.
///
/// The image is a 3D `Rgba32Float` texture in Bevy's ambient-cube layout:
/// dimensions `(Rx, 2·Ry, 3·Rz)` where `(Rx, Ry, Rz)` is the voxel resolution.
/// All six directional faces of each voxel receive the same isotropic light level.
///
/// `bounds` is the inclusive range `(min_cube, max_cube)` in world grid coordinates;
/// voxel `(x, y, z)` corresponds to world cube `min_cube + (x, y, z)`.
pub fn illuminance_to_image(
    illuminance: &HashMap<IVec3, f32>,
    (min_cube, max_cube): (IVec3, IVec3),
) -> Image {
    let rx = (max_cube.x - min_cube.x + 1) as u32;
    let ry = (max_cube.y - min_cube.y + 1) as u32;
    let rz = (max_cube.z - min_cube.z + 1) as u32;

    // Texture dimensions for Bevy's ambient-cube irradiance volume format.
    let width = rx;
    let height = 2 * ry;
    let depth = 3 * rz;

    // Rgba8Unorm: 4 bytes per pixel, values in [0, 255] map to [0.0, 1.0] in the shader.
    // This format is universally filterable, unlike Rgba32Float which requires
    // the optional FLOAT32_FILTERABLE feature and silently breaks linear sampling without it.
    const BYTES_PER_PIXEL: usize = 4;
    let mut data = vec![0u8; (width * height * depth) as usize * BYTES_PER_PIXEL];

    // (t_offset, p_offset) for each face: -X, +X, -Y, +Y, -Z, +Z.
    let face_offsets: [(u32, u32); 6] = [
        (0, 0),
        (ry, 0),
        (0, rz),
        (ry, rz),
        (0, 2 * rz),
        (ry, 2 * rz),
    ];

    for z in 0..rz {
        for y in 0..ry {
            for x in 0..rx {
                let world_cube = min_cube + IVec3::new(x as i32, y as i32, z as i32);
                let level = illuminance.get(&world_cube).copied().unwrap_or(0.0);
                let byte = (level.clamp(0.0, 1.0) * 255.0).round() as u8;
                let pixel = [byte, byte, byte, 255u8];

                for &(t_off, p_off) in &face_offsets {
                    let s = x;
                    let t = y + t_off;
                    let p = z + p_off;
                    let pixel_idx = (s + t * width + p * width * height) as usize;
                    data[pixel_idx * BYTES_PER_PIXEL..pixel_idx * BYTES_PER_PIXEL + 4]
                        .copy_from_slice(&pixel);
                }
            }
        }
    }

    Image {
        data: Some(data),
        texture_descriptor: TextureDescriptor {
            label: None,
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: depth,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D3,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        },
        // sampler: bevy::image::ImageSampler::nearest(),
        asset_usage: RenderAssetUsages::RENDER_WORLD,
        ..default()
    }
}

/// Bevy system: despawns old irradiance volumes and spawns one covering the full
/// building bounding box, run whenever `WallGrid` changes.
pub fn update_global_illumination(
    mut commands: Commands,
    wall_grid: Res<WallGrid>,
    structure_list: Res<StructureList>,
    existing: Query<Entity, With<GlobalIlluminationVolume>>,
    mut images: ResMut<Assets<Image>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }

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

    let min_lum = illuminance.values().cloned().fold(f32::INFINITY, f32::min);
    let max_lum = illuminance
        .values()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);
    let dark_count = illuminance.values().filter(|&&v| v < 0.01).count();
    info!(
        "GI: bbox {:?}..{:?}, illuminance {:.3}..{:.3}, {} dark voxels / {} total",
        min_cube,
        max_cube,
        min_lum,
        max_lum,
        dark_count,
        illuminance.len()
    );

    let image = illuminance_to_image(&illuminance, (min_cube, max_cube));
    let handle = images.add(image);

    // The IrradianceVolume is conceptually a 1×1×1 cube; the Transform stretches
    // it to cover the building's bounding box in world space.
    let size = (max_cube - min_cube + IVec3::ONE).as_vec3();
    let center = (min_cube.as_vec3() + (max_cube + IVec3::ONE).as_vec3()) / 2.0;

    commands.spawn((
        IrradianceVolume {
            voxels: handle,
            intensity: IRRADIANCE_VOLUME_INTENSITY,
            affects_lightmapped_meshes: true,
        },
        Transform::from_translation(center).with_scale(size),
        GlobalIlluminationVolume,
    ));
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
}
