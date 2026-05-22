use std::f32::consts::{FRAC_PI_3, FRAC_PI_4};

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

/// Startup system: spawns four directional lights and the ground plane.
pub fn spawn_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ambient_light: ResMut<GlobalAmbientLight>,
) {
    // Four lights may be costly; reconsider if performance is bad.
    // Layers 0 (visible) + 1 (shadow-only hidden geometry) so hidden cells still cast shadows.
    let light_layers = RenderLayers::default().with(1);
    commands.spawn((
        DirectionalLight {
            illuminance: 5_000.0,
            // We should try `TemporalAntiAliasing` to reduce the noise.
            shadows_enabled: true,
            soft_shadow_size: Some(10.0),
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, FRAC_PI_3, -FRAC_PI_4, 0.0)),
        light_layers.clone(),
    ));

    ambient_light.brightness = 100.0;

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(100.0, 100.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.5, 0.3),
            ..default()
        })),
        Transform::default(),
    ));
}
