use bevy::math::Quat;
use bevy::prelude::Transform;

/// Rotation that converts OpenSCAD's Z-up coordinate space to Bevy's Y-up space.
/// Apply at scene spawn time to any SceneRoot loaded from an OpenSCAD-generated mesh.
pub fn zup_to_yup() -> Quat {
    Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
}

/// Returns a copy of `t` with the Z-up→Y-up correction baked into the rotation.
/// Use when spawning any SceneRoot from an OpenSCAD-generated glTF.
pub fn zup_scene_transform(t: Transform) -> Transform {
    Transform {
        rotation: t.rotation * zup_to_yup(),
        ..t
    }
}
