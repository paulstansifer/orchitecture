use std::f32::consts::{FRAC_PI_3, FRAC_PI_4};

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use crate::road::ROAD_WIDTH;

/// Startup system: spawns directional lights, the ground plane, and road meshes.
pub fn spawn_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ambient_light: ResMut<GlobalAmbientLight>,
) {
    // Layers 0 (visible) + 1 (shadow-only hidden geometry) so hidden cells still cast shadows.
    let light_layers = RenderLayers::default().with(1);
    commands.spawn((
        DirectionalLight {
            illuminance: 5_000.0,
            shadows_enabled: true,
            soft_shadow_size: Some(10.0),
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, FRAC_PI_3, -FRAC_PI_4, 0.0)),
        light_layers.clone(),
    ));

    ambient_light.brightness = 100.0;

    // Ground plane (grass).
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(1000.0, 1000.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.5, 0.3),
            ..default()
        })),
        Transform::default(),
    ));

    let dirt = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.38, 0.18),
        ..default()
    });
    let w = ROAD_WIDTH as f32;

    // East-West road: all x, z in [0, ROAD_WIDTH). Slightly above ground to avoid z-fighting.
    let ew_len = 1000.0_f32;
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(ew_len, w))),
        MeshMaterial3d(dirt.clone()),
        Transform::from_translation(Vec3::new(0.0, 0.01, w / 2.0)),
    ));

    // North arm: x in [0, ROAD_WIDTH), z >= ROAD_WIDTH. Raised a touch more to avoid overlap flicker.
    let north_len = 1000.0_f32;
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(w, north_len))),
        MeshMaterial3d(dirt),
        Transform::from_translation(Vec3::new(w / 2.0, 0.01, w + north_len / 2.0)),
    ));

    // It seems like 0.001 is low enough to Z-fight with the terrain, somehow!
}
