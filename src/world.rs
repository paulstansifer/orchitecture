use std::f32::consts::{FRAC_PI_2, FRAC_PI_3, FRAC_PI_4, PI};

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use crate::ceiling_lights::CeilingLight;
use crate::road::ROAD_WIDTH;

pub const SUN_ILLUMINANCE: f32 = 5_000.0;

/// Lighting modes cycled with the X key.
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
pub enum LightingMode {
    #[default]
    CeilingLights,
    /// Single shadowless fill light from the opposite direction of the sun.
    OppositeDir,
    /// Single shadowless fill light from directly above.
    StraightAbove,
    /// Two shadowless fill lights equally spaced around the sun (120° apart).
    TwoLights,
}

impl LightingMode {
    pub fn next(self) -> Self {
        match self {
            LightingMode::CeilingLights => LightingMode::OppositeDir,
            LightingMode::OppositeDir => LightingMode::StraightAbove,
            LightingMode::StraightAbove => LightingMode::TwoLights,
            LightingMode::TwoLights => LightingMode::CeilingLights,
        }
    }
}

/// Marker for non-sun directional lights spawned by lighting mode 2–4.
#[derive(Component)]
pub struct ExtraLight;

pub fn lighting_input_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut lighting_mode: ResMut<LightingMode>,
) {
    if keyboard.just_pressed(KeyCode::KeyX) {
        *lighting_mode = lighting_mode.next();
    }
}

pub fn apply_lighting_mode_system(
    mode: Res<LightingMode>,
    mut commands: Commands,
    mut ceiling_lights: Query<&mut Visibility, With<CeilingLight>>,
    extra_lights: Query<Entity, With<ExtraLight>>,
) {
    if !mode.is_changed() {
        return;
    }

    let ceiling_vis = if *mode == LightingMode::CeilingLights {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut vis in &mut ceiling_lights {
        *vis = ceiling_vis;
    }

    for entity in &extra_lights {
        commands.entity(entity).despawn();
    }

    const FILL: f32 = SUN_ILLUMINANCE / 3.0;

    match *mode {
        LightingMode::CeilingLights => {}
        LightingMode::OppositeDir => {
            commands.spawn((
                DirectionalLight {
                    illuminance: FILL,
                    shadows_enabled: false,
                    ..default()
                },
                Transform::from_rotation(Quat::from_euler(
                    EulerRot::YXZ,
                    FRAC_PI_3 + PI,
                    FRAC_PI_4,
                    0.0,
                )),
                ExtraLight,
            ));
        }
        LightingMode::StraightAbove => {
            commands.spawn((
                DirectionalLight {
                    illuminance: FILL,
                    shadows_enabled: false,
                    ..default()
                },
                Transform::from_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
                ExtraLight,
            ));
        }
        LightingMode::TwoLights => {
            for y_angle in [FRAC_PI_3 + 2.0 * PI / 3.0, FRAC_PI_3 - 2.0 * PI / 3.0] {
                commands.spawn((
                    DirectionalLight {
                        illuminance: FILL,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_rotation(Quat::from_euler(
                        EulerRot::YXZ,
                        y_angle,
                        -FRAC_PI_4,
                        0.0,
                    )),
                    ExtraLight,
                ));
            }
        }
    }
}

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
            illuminance: SUN_ILLUMINANCE,
            shadows_enabled: true,
            soft_shadow_size: None,
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
