use std::collections::{HashSet, VecDeque};
use std::f32::consts::FRAC_PI_2;
use std::time::Duration;

use bevy::gltf::Gltf;
use bevy::prelude::*;
use bevy::scene::SceneInstanceReady;
use bevy::window::PrimaryWindow;
use bevy_egui::input::EguiWantsInput;
use bevy_picking::prelude::{MeshRayCast, MeshRayCastSettings};

use crate::camera::GameCamera;
use crate::ortho_camera::{cam_fwd_xz_base, trimetric_camera_basis, WalkCameraState};
use crate::pathing::NavigationGrid;

/// Clip indices within assets/static/orcs/orc1_mesh2motion.glb.
/// Order: Idle Listening(0), Sitting_Enter(1), Sprint_Loop(2), Walk_Loop(3).
const IDLE_ANIMATION_INDEX: usize = 0;
const WALK_ANIMATION_INDEX: usize = 3;

const BLEND_DURATION: Duration = Duration::from_millis(150);

#[derive(Component)]
pub struct Orc {
    /// The child entity that holds `AnimationPlayer`; set by `on_orc_scene_ready`.
    anim_player: Option<Entity>,
    /// Handle for the parent Gltf asset; set by `on_orc_scene_ready`.
    gltf_handle: Option<Handle<Gltf>>,
    /// Set by `setup_orc_animation` once the Gltf asset is confirmed loaded.
    idle_node: Option<AnimationNodeIndex>,
    walk_node: Option<AnimationNodeIndex>,
    /// None = not yet started; Some(false) = idle; Some(true) = walking.
    is_walking: Option<bool>,
    /// Remaining waypoints (room cubes, including floor/`y`) toward a
    /// click-to-move destination; drained front-to-back as the orc arrives at
    /// each one. Cleared by manual (IJKL) movement.
    nav_path: VecDeque<IVec3>,
}

pub fn spawn_orc(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands
        .spawn((
            Orc {
                anim_player: None,
                gltf_handle: None,
                idle_node: None,
                walk_node: None,
                is_walking: None,
                nav_path: VecDeque::new(),
            },
            SceneRoot(asset_server.load("assets/static/orcs/orc1_mesh2motion.glb#Scene0")),
            Transform::from_xyz(0.0, 0.1, 0.0).with_scale(Vec3::splat(0.5)),
        ))
        .observe(on_orc_scene_ready);
}

fn on_orc_scene_ready(
    trigger: On<SceneInstanceReady>,
    mut orcs: Query<&mut Orc>,
    children_q: Query<&Children>,
    players_q: Query<(), With<AnimationPlayer>>,
    asset_server: Res<AssetServer>,
) {
    let orc_entity = trigger.event_target();
    let Ok(mut orc) = orcs.get_mut(orc_entity) else {
        return;
    };

    let Some(player_entity) = find_descendant_with_component(orc_entity, &children_q, &players_q)
    else {
        return;
    };

    orc.anim_player = Some(player_entity);
    orc.gltf_handle = Some(asset_server.load("assets/static/orcs/orc1_mesh2motion.glb"));
}

/// Polls each frame until the Gltf asset is loaded, then builds the AnimationGraph
/// and inserts it onto the player entity.
pub fn setup_orc_animation(
    mut orcs: Query<&mut Orc>,
    gltf_assets: Res<Assets<Gltf>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut commands: Commands,
) {
    for mut orc in &mut orcs {
        // Skip if already set up or not yet scene-ready.
        if orc.idle_node.is_some() || orc.anim_player.is_none() {
            continue;
        }
        let (Some(player_entity), Some(gltf_handle)) = (orc.anim_player, orc.gltf_handle.clone())
        else {
            continue;
        };
        let Some(gltf) = gltf_assets.get(&gltf_handle) else {
            continue;
        };
        let (Some(idle_clip), Some(walk_clip)) = (
            gltf.animations.get(IDLE_ANIMATION_INDEX).cloned(),
            gltf.animations.get(WALK_ANIMATION_INDEX).cloned(),
        ) else {
            continue;
        };

        let (mut graph, idle_node) = AnimationGraph::from_clip(idle_clip);
        let walk_node = graph.add_clip(walk_clip, 1.0, graph.root);
        let graph_handle = graphs.add(graph);

        commands.entity(player_entity).insert((
            AnimationGraphHandle(graph_handle),
            AnimationTransitions::new(),
        ));
        orc.idle_node = Some(idle_node);
        orc.walk_node = Some(walk_node);
    }
}

fn find_descendant_with_component(
    entity: Entity,
    children_q: &Query<&Children>,
    marker_q: &Query<(), With<AnimationPlayer>>,
) -> Option<Entity> {
    if marker_q.contains(entity) {
        return Some(entity);
    }
    for &child in children_q.get(entity).ok()? {
        if let Some(found) = find_descendant_with_component(child, children_q, marker_q) {
            return Some(found);
        }
    }
    None
}

pub fn despawn_orc(mut commands: Commands, orcs: Query<Entity, With<Orc>>) {
    for entity in &orcs {
        commands.entity(entity).despawn();
    }
}

/// Collects `entity` and every descendant into `out`, so a ray-cast filter
/// can exclude an entire scene-spawned hierarchy (e.g. the orc's own GLTF
/// model) rather than just its root.
fn collect_with_descendants(
    entity: Entity,
    children_q: &Query<&Children>,
    out: &mut HashSet<Entity>,
) {
    out.insert(entity);
    let Ok(children) = children_q.get(entity) else {
        return;
    };
    for &child in children {
        collect_with_descendants(child, children_q, out);
    }
}

/// Left-click in walk mode: ray-casts from the cursor into the scene and, if
/// it hits something, paths the orc there via the navigation grid. Ignores
/// hits on the orc's own model, and defers to egui when it wants the click.
pub fn orc_click_to_move_system(
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    egui_wants_input: Res<EguiWantsInput>,
    children_q: Query<&Children>,
    nav_grid: Option<Res<NavigationGrid>>,
    mut ray_cast: MeshRayCast,
    mut orcs: Query<(Entity, &Transform, &mut Orc)>,
) {
    if !mouse_button.just_pressed(MouseButton::Left) || egui_wants_input.wants_any_pointer_input() {
        return;
    }
    let Some(nav_grid) = nav_grid else {
        return;
    };
    let Ok((orc_entity, transform, mut orc)) = orcs.single_mut() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera_q.single() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor) else {
        return;
    };

    let mut excluded = HashSet::new();
    collect_with_descendants(orc_entity, &children_q, &mut excluded);
    let filter = |e: Entity| !excluded.contains(&e);
    let settings = MeshRayCastSettings::default().with_filter(&filter);
    let Some((_, hit)) = ray_cast.cast_ray(ray, &settings).first() else {
        return;
    };

    // `cell_transform` places every slot's mesh origin at exactly `cube.y`,
    // so rounding a hit's world position recovers its cube uniformly whether
    // it's the ground, a floor, or a room interior.
    let to = hit.point.round().as_ivec3();
    let from = transform.translation.round().as_ivec3();
    if let Some(path) = nav_grid.find_path(from, to) {
        orc.nav_path = path.into();
    }
}

pub fn orc_input_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    walk_state: Res<WalkCameraState>,
    mut orcs: Query<(&mut Transform, &mut Orc)>,
    mut player_q: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    let Ok((mut transform, mut orc)) = orcs.single_mut() else {
        return;
    };

    let (cam_r_base, _, _) = trimetric_camera_basis();
    let cam_fwd = cam_fwd_xz_base();
    let rot = Quat::from_rotation_y(walk_state.camera_direction as f32 * FRAC_PI_2);
    let cam_r = rot * cam_r_base;
    let cam_fwd_xz = rot * cam_fwd;

    let speed = 2.0;
    let dt = time.delta_secs();
    let mut move_dir = Vec3::ZERO;

    if keyboard.pressed(KeyCode::KeyI) {
        move_dir += cam_fwd_xz;
    }
    if keyboard.pressed(KeyCode::KeyK) {
        move_dir -= cam_fwd_xz;
    }
    if keyboard.pressed(KeyCode::KeyJ) {
        move_dir -= cam_r;
    }
    if keyboard.pressed(KeyCode::KeyL) {
        move_dir += cam_r;
    }

    let key_moving = move_dir != Vec3::ZERO;
    if key_moving {
        orc.nav_path.clear(); // Manual input cancels a pending click-to-move destination.

        // Round movement to nearest cardinal or diagonal direction (8 directions total).
        let angle = move_dir.z.atan2(move_dir.x);
        let frac_pi_4 = std::f32::consts::FRAC_PI_4;
        let rounded_angle = (angle / frac_pi_4).round() * frac_pi_4;
        let rounded_dir = Vec3::new(rounded_angle.cos(), 0.0, rounded_angle.sin());

        transform.translation += rounded_dir * speed * dt;
        transform.look_to(-rounded_dir, Vec3::Y);
    } else if let Some(&next) = orc.nav_path.front() {
        // `Orc` walks at `floor_y + 0.1` (see `spawn_orc`), so waypoints keep that offset.
        let target = Vec3::new(next.x as f32, next.y as f32 + 0.1, next.z as f32);
        let to_target = target - transform.translation;
        let dist = to_target.length();
        const ARRIVE_EPS: f32 = 0.05;
        if dist < ARRIVE_EPS {
            orc.nav_path.pop_front();
        } else {
            let dir = to_target / dist;
            transform.translation += dir * (speed * dt).min(dist);
            let horizontal = Vec3::new(dir.x, 0.0, dir.z);
            if horizontal.length_squared() > 1e-6 {
                transform.look_to(-horizontal.normalize(), Vec3::Y);
            }
        }
    }
    let moving = key_moving || !orc.nav_path.is_empty();

    let (Some(player_entity), Some(idle_node), Some(walk_node)) =
        (orc.anim_player, orc.idle_node, orc.walk_node)
    else {
        return;
    };
    let Ok((mut player, mut transitions)) = player_q.get_mut(player_entity) else {
        return;
    };

    match (moving, orc.is_walking) {
        // First frame after animation is ready, or key released: hold idle pose (frame 0).
        (false, None) | (false, Some(true)) => {
            transitions
                .play(&mut player, idle_node, BLEND_DURATION)
                .set_speed(0.0);
            orc.is_walking = Some(false);
        }
        // Key pressed: transition to walk.
        (true, None) | (true, Some(false)) => {
            transitions
                .play(&mut player, walk_node, BLEND_DURATION)
                .set_speed(2.5)
                .repeat();
            orc.is_walking = Some(true);
        }
        // Already in the right state.
        _ => {}
    }
}
