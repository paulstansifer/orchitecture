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
    let image = Image::from_buffer(
        bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::nearest(),
        RenderAssetUsages::RENDER_WORLD,
    )
    .expect("failed to decode resource icon PNG");
    images.add(image)
}

macro_rules! icon_map {
    ($images:expr, $dir:expr, { $($res:ident => $file:expr),* $(,)? }) => {{
        let mut map = HashMap::new();
        $(
            map.insert(
                UniformResource::$res,
                load_png(include_bytes!(concat!("../sprites/pngs/", $dir, "/", $file, ".png")), $images),
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
    });
    commands.insert_resource(ResourceIcons { large, small });
}
