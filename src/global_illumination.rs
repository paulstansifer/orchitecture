use std::collections::{BinaryHeap, HashMap};

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

use crate::gi_material::{GiMaterial, GI_INTENSITY};
use crate::sparse3d::{Slot, SlotCoord, Sparse3D};
use crate::structure::{StructureInfo, StructureList};
use crate::world::{Cell, ConstructedWorld, MaterialAssets};

const FALLOFF: f32 = 0.30;
// Not sure this works, so not using it yet.
const FALLOFF_DOWNWARD: f32 = 0.30;
/// Half-width of the per-hop falloff noise: effective falloff ∈ [FALLOFF − R, FALLOFF + R].
const FALLOFF_NOISE_RADIUS: f32 = 0.10;

/// Number of independent sky-source contributions tracked per cell.
/// Since the heap is max-ordered, the first MAX_SOURCES to settle at any cell
/// are definitionally the strongest ones, which is what we want.
const MAX_SOURCES: usize = 4;

/// Heap entry ordered by light level (higher = higher priority), with cube
/// and source coordinates as tiebreakers so that `Ord` and `PartialEq` are
/// consistent.
#[derive(PartialEq, Eq)]
struct HeapEntry {
    level_bits: u32,
    cube: IVec3,
    source: IVec3,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.level_bits
            .cmp(&other.level_bits)
            .then_with(|| self.cube.x.cmp(&other.cube.x))
            .then_with(|| self.cube.y.cmp(&other.cube.y))
            .then_with(|| self.cube.z.cmp(&other.cube.z))
            .then_with(|| self.source.x.cmp(&other.source.x))
            .then_with(|| self.source.y.cmp(&other.source.y))
            .then_with(|| self.source.z.cmp(&other.source.z))
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
            "window" | "doorway" => 0.75,
            _ => 1.0,
        },
    }
}

/// Maps a voxel coordinate to a stable value in [0.0, 1.0) via bit-mixing.
fn coord_hash(v: IVec3) -> f32 {
    let mut h = (v.x as u32).wrapping_mul(0x9e3779b9)
        ^ (v.y as u32).wrapping_mul(0x6c62272e)
        ^ (v.z as u32).wrapping_mul(0x517cc1b7);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h as f32 * (1.0 / u32::MAX as f32)
}

/// Flood-fills sky illuminance from sky-visible cube voxels.
///
/// Seeds all cubes within the grid bounding box (expanded by 1) that have an
/// unobstructed vertical view of the sky — each such cube is an independent
/// light source identified by its position. Propagates outward using a
/// max-priority queue so the brightest frontier is always settled first.
///
/// Each hop multiplies the current level by `(1 - falloff) * transmission` where
/// `falloff` is `FALLOFF` ± `FALLOFF_NOISE_RADIUS`, hashed from the destination
/// voxel's coordinates for stable, view-independent variation:
/// - open air: × ≈0.60  (range 0.50–0.70)
/// - window/doorway: × ≈0.30  (range 0.25–0.35)
/// - wall/floor: blocked (× 0.0)
///
/// Per cell, the top `MAX_SOURCES` contributions (by level) are retained; the
/// max-heap order guarantees these are the strongest. The final illuminance is
/// the screen blend of all retained contributions: `1 − ∏(1 − cᵢ)`.
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

    // Per cell: the settled contributions from the strongest MAX_SOURCES sky sources.
    // Key: cube coordinate → (source position → contribution level).
    let mut contributions: HashMap<IVec3, HashMap<IVec3, f32>> = HashMap::new();
    // BinaryHeap is a max-heap. f32::to_bits() preserves order for non-negative floats.
    let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::new();

    // Seed: each sky-visible cube is its own source at full brightness.
    for z in search_min.z..=search_max.z {
        for y in search_min.y..=search_max.y {
            for x in search_min.x..=search_max.x {
                let cube = IVec3::new(x, y, z);
                if has_sky_above(contents, cube, top_y) {
                    contributions.entry(cube).or_default().insert(cube, 1.0);
                    heap.push(HeapEntry {
                        level_bits: 1.0f32.to_bits(),
                        cube,
                        source: cube,
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

    while let Some(HeapEntry {
        level_bits,
        cube,
        source,
    }) = heap.pop()
    {
        let level = f32::from_bits(level_bits);

        // Discard stale entries: a better path for this (source, cube) pair was
        // already settled if the stored level is strictly higher than what we popped.
        let settled = contributions
            .get(&cube)
            .and_then(|m| m.get(&source))
            .copied()
            .unwrap_or(0.0);
        if settled > level {
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
            let falloff = if dir == IVec3::NEG_Y {
                FALLOFF_DOWNWARD
            } else {
                FALLOFF
            };

            let falloff = falloff + FALLOFF_NOISE_RADIUS * (coord_hash(neighbor) * 2.0 - 1.0);
            let new_level = level * (1.0 - falloff) * transmission;

            // Decide whether to update (source, neighbor): either improve an existing
            // contribution or add a new one if the cell still has room.
            let current_for_source = contributions
                .get(&neighbor)
                .and_then(|m| m.get(&source))
                .copied();

            let should_update = match current_for_source {
                Some(c) => new_level > c,
                None => {
                    let count = contributions.get(&neighbor).map_or(0, |m| m.len());
                    count < MAX_SOURCES
                }
            };

            if should_update {
                contributions
                    .entry(neighbor)
                    .or_default()
                    .insert(source, new_level);
                heap.push(HeapEntry {
                    level_bits: new_level.to_bits(),
                    cube: neighbor,
                    source,
                });
            }
        }
    }

    // Combine per-cell contributions with the screen blend: 1 − ∏(1 − cᵢ).
    contributions
        .into_iter()
        .map(|(cube, source_map)| {
            let screen = 1.0 - source_map.values().fold(1.0_f32, |acc, &v| acc * (1.0 - v));
            (cube, screen)
        })
        .collect()
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
/// volume bounds) on every building material, run whenever `ConstructedWorld` changes.
pub fn update_global_illumination(
    constructed: Res<ConstructedWorld>,
    structure_list: Res<StructureList>,
    material_assets: Res<MaterialAssets>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GiMaterial>>,
) {
    let contents = &constructed.contents;
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

    // Expand by 1 to match `compute_sky_illuminance`'s search range, so the exterior
    // ring of cubes is in the texture: this gives boundary walls their outside-facing
    // illuminance and stops interior light from bleeding out (the exterior cube also
    // carries the boundary wall's transmission on its low face). See `boundary_transmission`.
    let (min_cube, max_cube) = contents.bounding_box();
    let min_cube = min_cube - IVec3::ONE;
    let max_cube = max_cube + IVec3::ONE;

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

    /// An open cube (nothing above it in the grid) seeds itself as a sky source
    /// and should report full illuminance.
    #[test]
    fn test_open_cube_has_full_illuminance() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        // Place a single floor cell; nothing is above it → sky is visible.
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 0, 0),
            RelSlot::Floor,
            None,
        );
        let contents = builder.get();
        let illuminance = compute_sky_illuminance(&contents, &structure_infos);
        check!(
            illuminance
                .get(&IVec3::new(0, 0, 0))
                .copied()
                .unwrap_or(0.0)
                == 1.0
        );
    }

    /// Light level decreases as it propagates away from the sky through open air.
    /// Check that a cube one hop into a tunnel is dimmer than the entrance.
    #[test]
    fn test_illuminance_decreases_with_depth() {
        let structure_infos = load_structure_info();
        let mut builder = Builder::new(&structure_infos);
        // Build a sealed 1×1×4 tunnel: floor/ceiling at y=0 and y=1, walls on
        // the sides, open entrance at z=0 (no ZLoWall there).
        builder.build_box(IVec3::new(0, 0, 0), IVec3::new(0, 1, 3));
        // Remove the entrance wall so light can enter from z=0 face.
        builder.build_plane(
            IVec3::new(0, 0, 0),
            IVec3::new(0, 0, 0),
            RelSlot::ZLoWall,
            Some("doorway"),
        );
        let contents = builder.get();
        let illuminance = compute_sky_illuminance(&contents, &structure_infos);

        let entrance = illuminance
            .get(&IVec3::new(0, 0, 0))
            .copied()
            .unwrap_or(0.0);
        let deep = illuminance
            .get(&IVec3::new(0, 0, 3))
            .copied()
            .unwrap_or(0.0);
        check!(entrance > deep);
    }

    /// `gi_to_image` should produce a texture whose dimensions match the supplied
    /// bounding box (inclusive, so size = max - min + 1 on each axis).
    #[test]
    fn test_gi_to_image_dimensions() {
        use super::gi_to_image;
        use std::collections::HashMap;

        let structure_infos = load_structure_info();
        let contents = crate::sparse3d::Sparse3D::new();
        let illuminance: HashMap<bevy::math::IVec3, f32> = HashMap::new();

        let min = IVec3::new(0, 0, 0);
        let max = IVec3::new(3, 4, 2); // 4×5×3 voxels

        let image = gi_to_image(&illuminance, &contents, &structure_infos, (min, max));
        let desc = &image.texture_descriptor;
        check!(desc.size.width == 4);
        check!(desc.size.height == 5);
        check!(desc.size.depth_or_array_layers == 3);
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
        check!(tx == 0.75);

        // Cube (1,1,1): its −X face is the wall shared with (0,1,1)? No — that
        // boundary (XLoWall of cube 1) is open interior, so 1.0. Confirm an actual
        // box wall reads 0: cube (0,1,1)'s −Z face is the box's Z-low wall.
        let [_, _, tz_wall] =
            low_face_transmissions(&contents, &structure_infos, IVec3::new(1, 1, 0));
        check!(tz_wall == 0.0);
    }
}
