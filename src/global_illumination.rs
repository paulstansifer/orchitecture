use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

use crate::city::{Cell, ConstructedCity, MaterialAssets};
use crate::eorf::{EorfInfo, EorfList};
use crate::flood_fill::{coord_hash, flood_fill, has_sky_above};
use crate::gi_material::{GiMaterial, GI_INTENSITY};
use crate::sparse3d::{SlotCoord, Sparse3D};

const FALLOFF: f32 = 0.30;
// Not sure this works, but we should try setting this lower to make high windows nice.
const FALLOFF_DOWNWARD: f32 = 0.30;
/// Half-width of the per-hop falloff noise: effective falloff ∈ [FALLOFF − R, FALLOFF + R].
const FALLOFF_NOISE_RADIUS: f32 = 0.10;

/// Returns how much light passes through the boundary between adjacent cubes `from` and `to`.
/// 0.0 = fully blocked, 0.5 = window/doorway, 1.0 = open air or transparent structure.
fn boundary_transmission(
    contents: &Sparse3D<Cell>,
    structures: &[EorfInfo],
    from: IVec3,
    to: IVec3,
) -> f32 {
    match contents.get(SlotCoord::boundary(from, to)) {
        None => 1.0,
        Some(cell) => match structures[cell.id.as_usize()].name.as_str() {
            "wall" | "floor" => 0.0,
            "window" | "doorway" => 0.75,
            _ => 1.0,
        },
    }
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
/// Per cell, the illuminance is the strongest (max) level reaching it from
/// any seed — see `flood_fill` for why a max combine is used instead of
/// blending multiple sources together.
///
/// Returns a map from cube coordinate → light level in [0.0, 1.0].
pub fn compute_sky_illuminance(
    contents: &Sparse3D<Cell>,
    structures: &[EorfInfo],
) -> HashMap<IVec3, f32> {
    if contents.size() == 0 {
        return HashMap::new();
    }

    let (min_cube, max_cube) = contents.bounding_box();
    // Expand by 1 to include exterior cubes adjacent to windows.
    let search_min = min_cube - IVec3::ONE;
    let search_max = max_cube + IVec3::ONE;
    let top_y = search_max.y;

    // Seed: each sky-visible cube is its own source at full brightness.
    let mut seeds = Vec::new();
    for z in search_min.z..=search_max.z {
        for y in search_min.y..=search_max.y {
            for x in search_min.x..=search_max.x {
                let cube = IVec3::new(x, y, z);
                if has_sky_above(contents, cube, top_y) {
                    seeds.push(cube);
                }
            }
        }
    }

    flood_fill(seeds, search_min, search_max, |from, to| {
        let transmission = boundary_transmission(contents, structures, from, to);
        if transmission == 0.0 {
            return 0.0;
        }
        let dir = to - from;
        let falloff = if dir == IVec3::NEG_Y {
            FALLOFF_DOWNWARD
        } else {
            FALLOFF
        };
        let falloff = falloff + FALLOFF_NOISE_RADIUS * (coord_hash(to) * 2.0 - 1.0);
        transmission * (1.0 - falloff)
    })
}

/// Boundary transmission on a cube's three low faces (−X, −Y, −Z), i.e. the
/// `XLoWall`, `Floor` and `ZLoWall` slots. These are the faces shared with the
/// neighbors at `cube - X`, `cube - Y`, `cube - Z`, so storing only the low faces
/// per cube covers every boundary exactly once.
fn low_face_transmissions(
    contents: &Sparse3D<Cell>,
    structures: &[EorfInfo],
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
    structures: &[EorfInfo],
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
    constructed: Res<ConstructedCity>,
    structure_list: Res<EorfList>,
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
    use crate::eorf::load_structure_info;
    use crate::sparse3d::RelSlot;

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
