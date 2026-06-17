//! Sprite-sheet generator.
//!
//! Loads `orcs/human_1.glb`, keeps one of its two models (the male), and renders
//! four evenly-spaced phases of the "Walk" animation side by side through an
//! orthographic camera. The result is written to `orcs/human_1_walk.png` with a
//! transparent background.
//!
//! The trick for getting four poses into a single image is to spawn four copies
//! of the model, each with its own `AnimationPlayer` seeked (and paused) at a
//! different point in the walk cycle, laid out along the camera's right axis so
//! they appear as four cells in one render.

use bevy::camera::{RenderTarget, ScalingMode};
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::scene::SceneInstanceReady;
use bevy::window::ExitCondition;
use orchitecture_lib::paths::MANIFEST_DIR;

// --- Configuration knobs -------------------------------------------------

/// The glb to load, relative to the asset root (the crate manifest dir).
const MODEL_ASSET: &str = "orcs/human_1.glb";
/// Index of the "Walk" clip within the glb's animation list. The animations are,
/// in order: Charge-Punch, Idle, Left-Punch, Run, T-Pose, Walk, Rest, T-pose.
const WALK_ANIMATION_INDEX: usize = 5;
/// The glb contains two models, "HumanMale" and "HumanFemale". We keep the male
/// and despawn the female's mesh node so only one model is rendered.
const DROP_MESH_NAME: &str = "HumanFemale";

/// How much bulk to add to the model, in world units, by pushing the mesh
/// surface outward along its (welded) normals. Set to 0.0 to disable.
///
/// The figure is rendered at root scale 0.5, which halves this in world space.
const BULK_WORLD_UNITS: f32 = 0.15;
/// We inflate the rest-pose positions in mesh space, but the displacement is then
/// reshaped by the skinning transforms before it reaches world space. The overall
/// "7.33 units tall mesh → 1.87 units tall figure" ratio does *not* describe that
/// local scaling — measured empirically, displacing the surface by 0.10 mesh
/// units grows it by ~0.15 world units per side. So the conversion is about 0.66.
const MESH_UNITS_PER_WORLD_UNIT: f32 = 0.66;

/// Number of walk-cycle phases (cells) in the sheet.
const PHASES: u32 = 4;
/// Pixel size of one square cell.
const CELL_SIZE: u32 = 256;

/// Orthographic scale: one world unit corresponds to this many pixels.
const PIXELS_PER_UNIT: f32 = 40.0;
/// Vertical extent the orthographic camera shows, in world units.
const VIEW_HEIGHT: f32 = CELL_SIZE as f32 / PIXELS_PER_UNIT; // 6.4
/// Vertical point the camera aims at.
const LOOK_HEIGHT: f32 = 0.5;
/// Spacing between adjacent copies along the camera's right axis.
const CELL_SPACING: f32 = VIEW_HEIGHT;
/// How far the camera sits from the models (irrelevant to scale under an
/// orthographic projection, just needs to clear the near plane).
const CAMERA_DISTANCE: f32 = 30.0;

/// Frames to wait after posing the models before the first capture attempt, to
/// let the skinned poses propagate. The capture itself retries if it races the
/// render and grabs a blank frame, so this only needs to be modest.
const SETTLE_FRAMES: u32 = 8;

/// Where to write the finished sheet.
fn output_path() -> std::path::PathBuf {
    std::path::Path::new(MANIFEST_DIR).join("orcs/human_1_walk.png")
}

// --- Resources & components ----------------------------------------------

#[derive(Resource)]
struct Capture {
    graph: Handle<AnimationGraph>,
    node: AnimationNodeIndex,
    clip: Handle<AnimationClip>,
    image: Handle<Image>,
    /// Number of `SceneInstanceReady` events seen so far.
    ready: u32,
    /// Whether the players have been posed.
    configured: bool,
    /// Frames elapsed since posing; capture once this passes `SETTLE_FRAMES`.
    settle: u32,
    /// Whether the first screenshot has been requested.
    shot: bool,
    /// How many capture attempts have come back empty (the capture can race the
    /// render and grab a blank frame; we retry until we get content).
    attempts: u32,
}

/// Maximum number of capture retries before giving up and saving whatever we got.
const MAX_CAPTURE_ATTEMPTS: u32 = 30;

/// Marks one of the four model copies and records its phase index.
#[derive(Component, Clone, Copy)]
struct SpriteCopy {
    phase: u32,
}

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: MANIFEST_DIR.to_string(),
                    ..default()
                })
                .set(WindowPlugin {
                    // We render to an off-screen image, so the window is just a
                    // small required surface. Don't exit when it closes; we exit
                    // ourselves once the file is written.
                    primary_window: Some(Window {
                        title: "sprite_sheet".to_string(),
                        resolution: (256u32, 256u32).into(),
                        ..default()
                    }),
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                }),
        )
        .add_systems(Startup, setup)
        .add_systems(Update, (configure_when_ready, capture_when_settled))
        .run();
}

/// Trimetric camera vectors for the requested projection:
///   +X world axis → upper-left at 45° (1:1 rise:run)
///   +Z world axis → upper-right at 1:2 (1 up per 2 right)
///   +Y world axis → straight up
///
/// Derived by requiring the camera right/up vectors to satisfy those angle
/// constraints while staying orthonormal.  The forward vector looks downward
/// (forward.y < 0), so the camera sits above the scene.
///
///   cam_r · X  = -1/√3  (left)    cam_u · X = 1/√3  (up) → 1:1  ✓
///   cam_r · Z  =  2/√6  (right)   cam_u · Z = 1/√6  (up) → 1:2  ✓
///   cam_r · Y  =  0                cam_u · Y = 1/√2  (up) → vertical ✓
fn trimetric_camera_basis() -> (Vec3, Vec3, Vec3) {
    let cam_r = Vec3::new(-1.0 / 3f32.sqrt(), 0.0, 2.0 / 6f32.sqrt());
    let cam_u = Vec3::new(1.0 / 3f32.sqrt(), 1.0 / 2f32.sqrt(), 1.0 / 6f32.sqrt());
    // forward = cam_u × cam_r  (right-hand rule; y < 0 = looks downward)
    let cam_forward = cam_u.cross(cam_r);
    (cam_r, cam_u, cam_forward)
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Build a one-clip animation graph for the Walk animation.
    let clip: Handle<AnimationClip> =
        asset_server.load(GltfAssetLabel::Animation(WALK_ANIMATION_INDEX).from_asset(MODEL_ASSET));
    let (graph, node) = AnimationGraph::from_clip(clip.clone());
    let graph = graphs.add(graph);

    // Off-screen render target: one wide image holding all the cells.
    let mut image = Image::new_target_texture(
        CELL_SIZE * PHASES,
        CELL_SIZE,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    );
    // The screenshot path reads this back; keep the default usages plus what it
    // needs and make the format unambiguous.
    image.texture_descriptor.usage |= bevy::render::render_resource::TextureUsages::COPY_SRC;
    let image = images.add(image);

    let (cam_r, cam_u, cam_forward) = trimetric_camera_basis();

    // Lay the copies out along the camera's right axis so they tile horizontally.
    let center = Vec3::ZERO;
    let span = (PHASES as f32 - 1.0) / 2.0;

    let scene: Handle<Scene> = asset_server.load(GltfAssetLabel::Scene(0).from_asset(MODEL_ASSET));
    for phase in 0..PHASES {
        let offset = cam_r * (phase as f32 - span) * CELL_SPACING;
        commands.spawn((
            SceneRoot(scene.clone()),
            // Scale down by 2; the root scale halves the world-space bulking effect too.
            Transform::from_translation(center + offset).with_scale(Vec3::splat(0.5)),
            SpriteCopy { phase },
        ));
    }

    // Wireframe unit cube at the world origin as a scale reference.
    spawn_wireframe_cube(&mut commands, &mut meshes, &mut materials, Vec3::ZERO);

    // Trimetric orthographic camera, transparent background, no anti-aliasing.
    let look_at = center + Vec3::Y * LOOK_HEIGHT;
    let cam_pos = look_at - cam_forward * CAMERA_DISTANCE;
    // Rotation matrix: local +X = cam_r, local +Y = cam_u, local +Z = cam_r × cam_u (= -forward)
    let rotation = Quat::from_mat3(&Mat3::from_cols(cam_r, cam_u, cam_r.cross(cam_u)));
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        // In Bevy 0.18 the render target is its own component, not a `Camera` field.
        RenderTarget::Image(image.clone().into()),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: VIEW_HEIGHT,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform {
            translation: cam_pos,
            rotation,
            scale: Vec3::ONE,
        },
        Msaa::Off,
        // Fill light so the model isn't a silhouette (AmbientLight is per-camera).
        AmbientLight {
            brightness: 400.0,
            ..default()
        },
    ));

    // Key light so the model isn't flat.
    commands.spawn((
        DirectionalLight {
            illuminance: 8_000.0,
            ..default()
        },
        Transform::from_xyz(8.0, 12.0, 6.0).looking_at(look_at, Vec3::Y),
    ));

    commands.insert_resource(Capture {
        graph,
        node,
        clip,
        image,
        ready: 0,
        configured: false,
        settle: 0,
        shot: false,
        attempts: 0,
    });

    // Count each scene instance as it finishes spawning.
    commands.add_observer(|_: On<SceneInstanceReady>, mut capture: ResMut<Capture>| {
        capture.ready += 1;
    });
}

/// Spawns a wireframe unit cube (0..1 on each axis) out of thin cylinders.
/// The cylinder radius is chosen so each edge appears ~1 pixel wide at
/// `PIXELS_PER_UNIT` scale (radius = 0.5 / PIXELS_PER_UNIT).
fn spawn_wireframe_cube(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    base: Vec3,
) {
    let radius = 0.5 / PIXELS_PER_UNIT;
    let material = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        ..default()
    });

    // 12 edges: (start, end) in unit cube local coords
    let edges: &[(Vec3, Vec3)] = &[
        // bottom face
        (Vec3::new(0., 0., 0.), Vec3::new(1., 0., 0.)),
        (Vec3::new(0., 0., 0.), Vec3::new(0., 0., 1.)),
        (Vec3::new(1., 0., 0.), Vec3::new(1., 0., 1.)),
        (Vec3::new(0., 0., 1.), Vec3::new(1., 0., 1.)),
        // top face
        (Vec3::new(0., 1., 0.), Vec3::new(1., 1., 0.)),
        (Vec3::new(0., 1., 0.), Vec3::new(0., 1., 1.)),
        (Vec3::new(1., 1., 0.), Vec3::new(1., 1., 1.)),
        (Vec3::new(0., 1., 1.), Vec3::new(1., 1., 1.)),
        // vertical edges
        (Vec3::new(0., 0., 0.), Vec3::new(0., 1., 0.)),
        (Vec3::new(1., 0., 0.), Vec3::new(1., 1., 0.)),
        (Vec3::new(0., 0., 1.), Vec3::new(0., 1., 1.)),
        (Vec3::new(1., 0., 1.), Vec3::new(1., 1., 1.)),
    ];

    for &(a, b) in edges {
        let a = a + base;
        let b = b + base;
        let mid = (a + b) * 0.5;
        let len = (b - a).length();
        let dir = (b - a) / len;
        let rotation = Quat::from_rotation_arc(Vec3::Y, dir);
        let mesh = meshes.add(Cylinder {
            radius,
            half_height: len * 0.5,
        });
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform {
                translation: mid,
                rotation,
                scale: Vec3::ONE,
            },
        ));
    }
}

/// Once all copies have spawned, pose each copy's `AnimationPlayer` at its phase
/// and drop the unwanted model.
fn configure_when_ready(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    clips: Res<Assets<AnimationClip>>,
    mut players: Query<(Entity, &mut AnimationPlayer)>,
    parents: Query<&ChildOf>,
    copies: Query<&SpriteCopy>,
    names: Query<(Entity, &Name)>,
    mesh_nodes: Query<(&Name, &Mesh3d)>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
) {
    if capture.configured || capture.ready < PHASES {
        return;
    }
    let Some(clip) = clips.get(&capture.clip) else {
        return; // clip not loaded yet
    };
    let duration = clip.duration();

    for (entity, mut player) in &mut players {
        let Some(copy) = find_copy(entity, &parents, &copies) else {
            continue;
        };
        // Evenly spaced phases across the cycle: 0, 1/4, 2/4, 3/4 of the duration.
        let t = duration * (copy.phase as f32) / (PHASES as f32);
        let active = player.play(capture.node);
        active.seek_to(t);
        active.pause();
        commands
            .entity(entity)
            .insert(AnimationGraphHandle(capture.graph.clone()));
    }

    // Keep only the chosen model: despawn the other mesh node wherever it appears.
    for (entity, name) in &names {
        if name.as_str() == DROP_MESH_NAME {
            commands.entity(entity).despawn();
        }
    }

    // Optionally bulk up the kept model. All four copies share one mesh asset, so
    // inflate it once (a `HashSet` guards against doing it per-copy).
    if BULK_WORLD_UNITS != 0.0 {
        let amount = BULK_WORLD_UNITS * MESH_UNITS_PER_WORLD_UNIT;
        let mut done = std::collections::HashSet::new();
        for (name, mesh3d) in &mesh_nodes {
            if name.as_str() == DROP_MESH_NAME || !done.insert(mesh3d.0.id()) {
                continue;
            }
            if let Some(mesh) = mesh_assets.get_mut(&mesh3d.0) {
                inflate_mesh(mesh, amount);
            }
        }
    }

    capture.configured = true;
}

/// Pushes a mesh's surface outward along its normals by `amount` (in mesh-local
/// units), making the model bulkier.
///
/// The glb stores duplicated vertices at UV/shading seams (595 verts for an
/// 816-triangle mesh), each with its own facet normal. Displacing every vertex
/// along its own normal would tear the surface apart at those seams, so we first
/// "weld" coincident vertices — summing the normals that share a position — and
/// move all of them along that shared direction. The original normals are left
/// untouched, so shading is unchanged.
fn inflate_mesh(mesh: &mut Mesh, amount: f32) {
    use bevy::mesh::VertexAttributeValues::Float32x3;

    let Some(Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        return;
    };
    let positions = positions.clone();
    let Some(Float32x3(normals)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL) else {
        return;
    };

    // Quantize positions so coincident-but-not-bit-identical verts weld together.
    let key = |p: &[f32; 3]| {
        [
            (p[0] * 1e4).round() as i64,
            (p[1] * 1e4).round() as i64,
            (p[2] * 1e4).round() as i64,
        ]
    };
    let mut welded: std::collections::HashMap<[i64; 3], Vec3> = std::collections::HashMap::new();
    for (p, n) in positions.iter().zip(normals.iter()) {
        *welded.entry(key(p)).or_insert(Vec3::ZERO) += Vec3::from_array(*n);
    }

    let mut new_positions = positions.clone();
    for (i, p) in positions.iter().enumerate() {
        let dir = welded[&key(p)].normalize_or_zero();
        new_positions[i] = (Vec3::from_array(*p) + dir * amount).to_array();
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, new_positions);
}

/// Walk up the parent chain to find which `SpriteCopy` an entity belongs to.
fn find_copy(
    mut entity: Entity,
    parents: &Query<&ChildOf>,
    copies: &Query<&SpriteCopy>,
) -> Option<SpriteCopy> {
    loop {
        if let Ok(copy) = copies.get(entity) {
            return Some(*copy);
        }
        entity = parents.get(entity).ok()?.parent();
    }
}

/// After the posed models have had a few frames to render, capture the image and
/// write it out, then exit.
fn capture_when_settled(mut commands: Commands, mut capture: ResMut<Capture>) {
    if !capture.configured || capture.shot {
        return;
    }
    if capture.settle < SETTLE_FRAMES {
        capture.settle += 1;
        return;
    }
    capture.shot = true;
    request_screenshot(&mut commands, capture.image.clone());
}

/// Spawns a one-shot screenshot of the off-screen render target, observed by
/// [`on_capture`].
fn request_screenshot(commands: &mut Commands, image: Handle<Image>) {
    commands
        .spawn(Screenshot(RenderTarget::Image(image.into())))
        .observe(on_capture);
}

/// Handles a captured frame: if it's blank (the capture raced the render), retry
/// on the next frame; otherwise save it (preserving alpha, unlike Bevy's
/// `save_to_disk`) and exit.
fn on_capture(
    captured: On<ScreenshotCaptured>,
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    mut exit: MessageWriter<AppExit>,
) {
    let rgba = match captured.image.clone().try_into_dynamic() {
        Ok(dyn_img) => dyn_img.to_rgba8(),
        Err(e) => {
            error!("Could not convert captured screenshot: {e}");
            exit.write(AppExit::error());
            return;
        }
    };

    let has_content = rgba.pixels().any(|p| p.0[3] != 0);
    if !has_content && capture.attempts < MAX_CAPTURE_ATTEMPTS {
        // The render hadn't landed in the captured buffer yet; try again.
        capture.attempts += 1;
        let image = capture.image.clone();
        request_screenshot(&mut commands, image);
        return;
    }

    let path = output_path();
    match rgba.save(&path) {
        Ok(()) => info!("Wrote sprite sheet to {}", path.display()),
        Err(e) => error!("Failed to write sprite sheet: {e}"),
    }
    exit.write(AppExit::Success);
}
