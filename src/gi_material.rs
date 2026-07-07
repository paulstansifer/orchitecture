//! Custom global-illumination material.
//!
//! Replaces Bevy's built-in `IrradianceVolume` with an `ExtendedMaterial` over
//! `StandardMaterial`: the base material provides ordinary PBR (sun, fill lights,
//! shadows), and the extension's fragment shader ([assets/static/shaders/gi.wgsl])
//! adds an
//! adjacency-aware trilinear blend of the per-cube sky illuminance computed in
//! [`crate::global_illumination`].
//!
//! The GI texture is an `Rgba8Unorm` 3D texture, one voxel per grid cube:
//! `R` = illuminance, `G`/`B`/`A` = boundary transmission on the cube's −X / −Y / −Z
//! faces (0.0 = wall, 0.5 = window/doorway, 1.0 = open). The shader fetches the
//! 2×2×2 neighborhood with `textureLoad` and interpolates each axis with a weight
//! that the transmission modulates from nearest-neighbor (0.0) to linear (1.0).

use bevy::asset::{Asset, RenderAssetUsages};
use bevy::image::Image;
use bevy::math::Vec4;
use bevy::pbr::{ExtendedMaterial, Material, MaterialExtension, MaterialPlugin, StandardMaterial};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};
use bevy::shader::ShaderRef;

/// How strongly the GI term contributes; multiplies `albedo * illuminance` and is
/// added to the PBR output in linear HDR space. Tune to match the desired interior
/// brightness. (Replaces the old `IRRADIANCE_VOLUME_INTENSITY`.)
pub const GI_INTENSITY: f32 = 1.0;

/// The full material used on building/furniture meshes.
pub type GiMaterial = ExtendedMaterial<StandardMaterial, GiExtension>;

/// Per-material binding of the shared GI volume. All instances point at the same
/// `gi_tex`; `min_cube`/`resolution` describe its placement in world space.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct GiExtension {
    /// xyz = world coordinate of voxel (0,0,0)'s cube; w = GI intensity.
    #[uniform(100)]
    pub min_cube: Vec4,
    /// xyz = voxel resolution (rx, ry, rz); w unused.
    #[uniform(101)]
    pub resolution: Vec4,
    /// One voxel per cube; see module docs for channel layout.
    #[texture(102, dimension = "3d")]
    pub gi_tex: Handle<Image>,
}

impl MaterialExtension for GiExtension {
    fn fragment_shader() -> ShaderRef {
        "assets/static/shaders/gi.wgsl".into()
    }
}

/// A 1×1×1 all-zero GI texture, used until the first real GI computation runs so the
/// `texture_3d` binding is always valid.
pub fn default_gi_image() -> Image {
    Image {
        data: Some(vec![0u8; 4]),
        texture_descriptor: TextureDescriptor {
            label: None,
            size: Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
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

/// Shadow-caster-only material: invisible in the color pass (its fragment shader
/// [assets/static/shaders/shadow_only.wgsl] discards), but still casts shadows,
/// because the light's shadow/prepass uses a separate depth-only pipeline that
/// never runs this fragment.
///
/// Used for cutaway-hidden geometry, which must stay on the camera's render layer
/// to cast a shadow into its view (see `queue_shadows`' camera-layer filter) and
/// therefore can't be hidden with render layers.
#[derive(Asset, TypePath, AsBindGroup, Clone, Default)]
pub struct ShadowOnlyMaterial {}

impl Material for ShadowOnlyMaterial {
    fn fragment_shader() -> ShaderRef {
        "assets/static/shaders/shadow_only.wgsl".into()
    }
}

pub struct GiPlugin;

impl Plugin for GiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<GiMaterial>::default());
        app.add_plugins(MaterialPlugin::<ShadowOnlyMaterial>::default());
    }
}
