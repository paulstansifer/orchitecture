use std::f32::consts::FRAC_PI_2;
use std::time::Duration;

use bevy::gltf::Gltf;
use bevy::prelude::*;
use bevy::scene::SceneInstanceReady;

use crate::ortho_camera::{cam_fwd_xz_base, trimetric_camera_basis, WalkCameraState};

/// Clip indices within orcs/human_1.glb.
/// Order: Charge-Punch(0), Idle(1), Left-Punch(2), Run(3), T-Pose(4), Walk(5), Rest(6), T-pose(7).
const IDLE_ANIMATION_INDEX: usize = 1;
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
            },
            SceneRoot(asset_server.load("orcs/human_1.glb#Scene0")),
            Transform::from_xyz(0.0, 0.1, 0.0).with_scale(Vec3::splat(0.5)),
        ))
        .observe(on_orc_scene_ready);
}

fn on_orc_scene_ready(
    trigger: On<SceneInstanceReady>,
    mut orcs: Query<&mut Orc>,
    children_q: Query<&Children>,
    players_q: Query<(), With<AnimationPlayer>>,
    names_q: Query<(Entity, &Name)>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    let orc_entity = trigger.event_target();
    let Ok(mut orc) = orcs.get_mut(orc_entity) else {
        return;
    };

    // The GLB has two scene-root nodes: MaleArm and FemaleArm. Each is the parent
    // of a mesh node and a skeleton root, and each gets an AnimationPlayer from the
    // GLTF loader. Despawn the male root; scope the AnimationPlayer search to FemaleArm.
    let mut male_entity = None;
    let mut female_entity = None;
    for desc in collect_descendants(orc_entity, &children_q) {
        if let Ok((_entity, name)) = names_q.get(desc) {
            match name.as_str() {
                "MaleArm" => male_entity = Some(desc),
                "FemaleArm" => female_entity = Some(desc),
                _ => {}
            }
        }
    }
    if let Some(e) = female_entity {
        commands.entity(e).despawn();
    }

    // Search for AnimationPlayer only within the kept mesh's subtree so we don't
    // accidentally grab the male's player (which will be despawned this frame).
    let search_root = male_entity.unwrap_or(orc_entity);
    let Some(player_entity) = find_descendant_with_component(search_root, &children_q, &players_q)
    else {
        return;
    };

    orc.anim_player = Some(player_entity);
    orc.gltf_handle = Some(asset_server.load("orcs/human_1.glb"));
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

fn collect_descendants(root: Entity, children_q: &Query<&Children>) -> Vec<Entity> {
    let mut result = Vec::new();
    let mut stack = children_q.get(root).map(|c| c.to_vec()).unwrap_or_default();
    while let Some(entity) = stack.pop() {
        result.push(entity);
        if let Ok(children) = children_q.get(entity) {
            stack.extend_from_slice(children);
        }
    }
    result
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

    let speed = 2.25;
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

    let moving = move_dir != Vec3::ZERO;
    if moving {
        transform.translation += move_dir.normalize() * speed * dt;
        transform.look_to(-move_dir, Vec3::Y);
    }

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
                .repeat();
            orc.is_walking = Some(true);
        }
        // Already in the right state.
        _ => {}
    }
}
