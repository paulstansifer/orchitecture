use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};

use crate::resource::UniformResource;

pub const LARGE_SIZE: [f32; 2] = [24.0, 18.0];
pub const SMALL_SIZE: [f32; 2] = [16.0, 12.0];

#[derive(Resource)]
pub struct ResourceIcons {
    large: HashMap<UniformResource, Handle<Image>>,
    small: HashMap<UniformResource, Handle<Image>>,
    tool_large: Handle<Image>,
    book_large: Handle<Image>,
    unassigned_storage_large: Handle<Image>,
}

impl ResourceIcons {
    pub fn texture_ids_large(
        &self,
        contexts: &mut EguiContexts,
    ) -> HashMap<UniformResource, egui::TextureId> {
        register_all(&self.large, contexts)
    }

    pub fn texture_ids_small(
        &self,
        contexts: &mut EguiContexts,
    ) -> HashMap<UniformResource, egui::TextureId> {
        register_all(&self.small, contexts)
    }

    /// Icon for the Tools rack row. There's no `UniformResource` variant for
    /// tools, so this is kept as its own handle rather than living in `large`.
    pub fn tool_texture_id_large(&self, contexts: &mut EguiContexts) -> egui::TextureId {
        contexts.add_image(EguiTextureHandle::Strong(self.tool_large.clone()))
    }

    /// Icon for the Books bookcase row. Like tools, books are a
    /// `UniqueResource` with no `UniformResource` variant, so it's its own
    /// handle.
    pub fn book_texture_id_large(&self, contexts: &mut EguiContexts) -> egui::TextureId {
        contexts.add_image(EguiTextureHandle::Strong(self.book_large.clone()))
    }

    /// Icon for the uncommitted (undedicated) storage row -- free capacity not
    /// reserved for any particular resource.
    pub fn unassigned_storage_texture_id_large(
        &self,
        contexts: &mut EguiContexts,
    ) -> egui::TextureId {
        contexts.add_image(EguiTextureHandle::Strong(
            self.unassigned_storage_large.clone(),
        ))
    }
}

fn register_all(
    map: &HashMap<UniformResource, Handle<Image>>,
    contexts: &mut EguiContexts,
) -> HashMap<UniformResource, egui::TextureId> {
    map.iter()
        .map(|(&res, handle)| {
            let tex_id = contexts.add_image(EguiTextureHandle::Strong(handle.clone()));
            (res, tex_id)
        })
        .collect()
}

fn load_png(bytes: &[u8], images: &mut Assets<Image>) -> Handle<Image> {
    let mut image = Image::from_buffer(
        bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::nearest(),
        RenderAssetUsages::RENDER_WORLD,
    )
    .expect("failed to decode resource icon PNG");
    premultiply_alpha_srgb(&mut image);
    images.add(image)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// bevy_egui blends with premultiplied alpha, but PNGs decode to straight
/// alpha. Without this, semi-transparent edge pixels keep their full color
/// intensity and get over-blended, making anti-aliased edges look jagged.
/// The premultiply itself is done in linear space, matching the sRGB decode
/// the GPU performs when sampling this (Rgba8UnormSrgb) texture.
fn premultiply_alpha_srgb(image: &mut Image) {
    let Some(data) = image.data.as_mut() else {
        return;
    };
    for pixel in data.chunks_exact_mut(4) {
        let alpha = pixel[3] as f32 / 255.0;
        for channel in &mut pixel[0..3] {
            let linear = srgb_to_linear(*channel as f32 / 255.0);
            let premultiplied = linear_to_srgb(linear * alpha);
            *channel = (premultiplied * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

macro_rules! icon_map {
    ($images:expr, $dir:expr, { $($res:ident => $file:expr),* $(,)? }) => {{
        let mut map = HashMap::new();
        $(
            map.insert(
                UniformResource::$res,
                load_png(include_bytes!(concat!("../assets/generated/sprites/", $dir, "/", $file, ".png")), $images),
            );
        )*
        map
    }};
}

pub fn spawn_resource_icons(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let large = icon_map!(&mut images, "24x18", {
        Potato    => "potato",
        Timber    => "timber",
        Straw     => "straw",
        WoodBeam  => "beam",
        Canvas    => "canvas",
        Fieldstone => "fieldstone",
        Block     => "block",
        Lime      => "lime",
        Plank     => "plank",
    });
    let small = icon_map!(&mut images, "16x12", {
        Potato    => "potato",
        Timber    => "timber",
        Straw     => "straw",
        WoodBeam  => "beam",
        Canvas    => "canvas",
        Fieldstone => "fieldstone",
        Block     => "block",
        Lime      => "lime",
        Plank     => "plank",
    });
    let tool_large = load_png(
        include_bytes!("../assets/generated/sprites/24x18/tool.png"),
        &mut images,
    );
    let book_large = load_png(
        include_bytes!("../assets/generated/sprites/24x18/book.png"),
        &mut images,
    );
    let unassigned_storage_large = load_png(
        include_bytes!("../assets/generated/sprites/24x18/unassigned_storage.png"),
        &mut images,
    );
    commands.insert_resource(ResourceIcons {
        large,
        small,
        tool_large,
        book_large,
        unassigned_storage_large,
    });
}
