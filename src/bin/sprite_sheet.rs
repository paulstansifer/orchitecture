//! Sprite-sheet generator — headless mode.
//!
//! Loads `orcs/human_1.glb`, keeps one of its two models (the male), and renders
//! four evenly-spaced phases of the "Walk" animation side by side through an
//! orthographic camera. The result is written to `orcs/human_1_walk.png` with a
//! transparent background.
//!
//! The binary runs fully headless (no window, no Winit). The main loop manually
//! ticks `app.update()` until the scenes and clip are loaded, then settles for a
//! few frames and reads back the GPU render buffer directly.
//!
//! Usage:  cargo run --bin sprite_sheet

use bevy::camera::{RenderTarget, ScalingMode};
use bevy::image::{Image, TextureFormatPixelInfo};
use bevy::prelude::*;
use bevy::render::{
    Extract, Render, RenderApp, RenderSystems,
    render_asset::RenderAssets,
    render_graph::{self, NodeRunError, RenderGraph, RenderGraphContext, RenderLabel},
    render_resource::{
        Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, PollType,
        TexelCopyBufferInfo, TexelCopyBufferLayout, TextureUsages,
    },
    renderer::{RenderContext, RenderDevice, RenderQueue},
    texture::GpuImage,
};
use bevy::scene::SceneInstanceReady;
use bevy::winit::WinitPlugin;
use orchitecture_lib::paths::MANIFEST_DIR;
use std::sync::{Mutex, mpsc};

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

/// Frames to wait after posing the models before capture, to let skinned poses
/// propagate through the render pipeline.
const SETTLE_FRAMES: u32 = 8;
/// Maximum frames to wait for scenes + clip to load before panicking.
const MAX_LOAD_FRAMES: u32 = 600;
/// Minimum number of opaque pixels the final image must contain. The wireframe
/// cube alone produces only a few hundred; all four figures add tens of thousands.
const MIN_CONTENT_PIXELS: usize = 100;

/// Where to write the finished sheet.
fn output_path() -> std::path::PathBuf {
    std::path::Path::new(MANIFEST_DIR).join("orcs/human_1_walk.png")
}

// --- GPU readback snap infrastructure ------------------------------------
//
// Architecture mirrors vector-arena/src/bin/headless/runner.rs:
//
//   setup_image_copier  (Startup, after setup)
//     spawns ImageCopier component with GPU buffer + src image handle
//     spawns SnapCpuImageHandle for the CPU-side readback image
//
//   Render world, each frame:
//     SnapCopyDriver (render graph node) → copies texture to GPU buffer
//     readback_to_channel               → maps buffer, sends bytes via channel
//
//   save_snap() (called from main):
//     drains stale bytes, ticks pipeline, strips row padding, saves PNG

#[derive(Debug, PartialEq, Eq, Clone, Hash, RenderLabel)]
struct SnapCopyLabel;

#[derive(Clone, Component)]
struct ImageCopier {
    buffer: Buffer,
    src_image: Handle<Image>,
}

#[derive(Clone, Default, Resource, Deref, DerefMut)]
struct ExtractedCopiers(Vec<ImageCopier>);

fn extract_copiers(mut commands: Commands, copiers: Extract<Query<&ImageCopier>>) {
    commands.insert_resource(ExtractedCopiers(copiers.iter().cloned().collect()));
}

struct SnapCopyDriver;

impl render_graph::Node for SnapCopyDriver {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let copiers = world.resource::<ExtractedCopiers>();
        let gpu_images = world.resource::<RenderAssets<GpuImage>>();

        for copier in copiers.iter() {
            let src = match gpu_images.get(&copier.src_image) {
                Some(s) => s,
                None => continue,
            };
            let mut encoder = render_context
                .render_device()
                .create_command_encoder(&CommandEncoderDescriptor::default());

            let (bx, _) = src.texture_format.block_dimensions();
            let block_size = src.texture_format.block_copy_size(None).unwrap();
            let padded = RenderDevice::align_copy_bytes_per_row(
                (src.size.width as usize / bx as usize) * block_size as usize,
            );

            encoder.copy_texture_to_buffer(
                src.texture.as_image_copy(),
                TexelCopyBufferInfo {
                    buffer: &copier.buffer,
                    layout: TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(std::num::NonZero::new(padded as u32).unwrap().into()),
                        rows_per_image: None,
                    },
                },
                src.size,
            );
            world.resource::<RenderQueue>().submit(std::iter::once(encoder.finish()));
        }
        Ok(())
    }
}

#[derive(Resource)]
struct SnapBytesTx(mpsc::Sender<Vec<u8>>);

#[derive(Resource)]
struct SnapBytesRx(Mutex<mpsc::Receiver<Vec<u8>>>);

impl SnapBytesRx {
    fn try_recv(&self) -> Option<Vec<u8>> {
        self.0.lock().unwrap().try_recv().ok()
    }
}

fn readback_to_channel(
    copiers: Res<ExtractedCopiers>,
    render_device: Res<RenderDevice>,
    tx: Res<SnapBytesTx>,
) {
    for copier in copiers.iter() {
        let slice = copier.buffer.slice(..);
        let (s, r) = mpsc::channel();
        slice.map_async(MapMode::Read, move |res| {
            s.send(res).ok();
        });
        render_device.poll(PollType::wait_indefinitely()).expect("poll failed");
        r.recv().ok();
        let _ = tx.0.send(slice.get_mapped_range().to_vec());
        copier.buffer.unmap();
    }
}

#[derive(Component)]
struct SnapCpuImageHandle(Handle<Image>);

struct SnapPlugin;

impl Plugin for SnapPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        app.insert_resource(SnapBytesRx(Mutex::new(rx)));

        let render_app = app.sub_app_mut(RenderApp);
        {
            let mut graph = render_app.world_mut().resource_mut::<RenderGraph>();
            graph.add_node(SnapCopyLabel, SnapCopyDriver);
            graph.add_node_edge(bevy::render::graph::CameraDriverLabel, SnapCopyLabel);
        }
        render_app
            .insert_resource(SnapBytesTx(tx))
            .add_systems(ExtractSchedule, extract_copiers)
            .add_systems(Render, readback_to_channel.after(RenderSystems::Render));
    }
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
}

/// Marks one of the four model copies and records its phase index.
#[derive(Component, Clone, Copy)]
struct SpriteCopy {
    phase: u32,
}

fn main() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: MANIFEST_DIR.to_string(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(),
    )
    .add_plugins(SnapPlugin)
    .add_systems(Startup, (setup, setup_image_copier).chain())
    .add_systems(Update, configure_when_ready);

    app.finish();

    // Tick until scenes and clip are configured (up to MAX_LOAD_FRAMES).
    let mut frame = 0u32;
    loop {
        app.update();
        frame += 1;
        if app.world().resource::<Capture>().configured {
            break;
        }
        assert!(
            frame < MAX_LOAD_FRAMES,
            "scenes/clip not ready after {MAX_LOAD_FRAMES} frames — asset loading failed"
        );
    }

    // Give the render pipeline time to process the new poses.
    for _ in 0..SETTLE_FRAMES {
        app.update();
    }

    save_snap(&mut app);

    std::process::exit(0);
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
            Transform::from_translation(center + offset).with_scale(Vec3::splat(0.5)),
            SpriteCopy { phase },
        ));
    }

    // Wireframe unit cube at the world origin as a scale reference.
    spawn_wireframe_cube(&mut commands, &mut meshes, &mut materials, Vec3::ZERO);

    // Trimetric orthographic camera, transparent background, no anti-aliasing.
    let look_at = center + Vec3::Y * LOOK_HEIGHT;
    let cam_pos = look_at - cam_forward * CAMERA_DISTANCE;
    let rotation = Quat::from_mat3(&Mat3::from_cols(cam_r, cam_u, cam_r.cross(cam_u)));
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
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
        AmbientLight {
            brightness: 400.0,
            ..default()
        },
    ));

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
    });

    // Count each scene instance as it finishes spawning.
    commands.add_observer(|_: On<SceneInstanceReady>, mut capture: ResMut<Capture>| {
        capture.ready += 1;
    });
}

/// Creates the GPU buffer for readback and spawns the `ImageCopier` and CPU image
/// used by `save_snap`. Must run after `setup` so `Capture.image` is available.
fn setup_image_copier(
    mut commands: Commands,
    capture: Res<Capture>,
    render_device: Res<RenderDevice>,
    mut images: ResMut<Assets<Image>>,
) {
    let width = CELL_SIZE * PHASES;
    let height = CELL_SIZE;
    let row_bytes = width as usize * 4; // Rgba8UnormSrgb = 4 bytes/pixel
    let padded = RenderDevice::align_copy_bytes_per_row(row_bytes);
    let buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some("sprite_sheet_readback"),
        size: (padded * height as usize) as u64,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    commands.spawn(ImageCopier {
        buffer,
        src_image: capture.image.clone(),
    });

    // CPU-side image used only for format conversion in save_snap.
    let cpu_image = Image::new_target_texture(
        width,
        height,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    );
    let cpu_handle = images.add(cpu_image);
    commands.spawn(SnapCpuImageHandle(cpu_handle));
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

/// Reads back the rendered frame from the GPU buffer and saves it as a PNG.
///
/// Drains any stale bytes from previous frames, ticks once to produce a fresh
/// frame, then reads back. Panics if no bytes arrive or if the saved image has
/// too few opaque pixels (indicating the humans didn't render).
fn save_snap(app: &mut App) {
    // Drain stale readback bytes.
    while app.world().resource::<SnapBytesRx>().try_recv().is_some() {}

    // Tick to produce a fresh rendered frame.
    app.update();

    // Read back with retries in case the GPU pipeline needs more time.
    let mut data = None;
    for _ in 0..8 {
        if let Some(d) = app.world().resource::<SnapBytesRx>().try_recv() {
            data = Some(d);
            break;
        }
        app.update();
    }
    let data = data.expect("GPU readback timed out — render pipeline produced no bytes");

    // De-pad rows (GPU row stride is aligned to 256 bytes; our rows may be shorter).
    let width = CELL_SIZE * PHASES;
    let height = CELL_SIZE;
    let mut cpu_q = app.world_mut().query::<&SnapCpuImageHandle>();
    let cpu_handle = cpu_q
        .single(app.world())
        .expect("SnapCpuImageHandle missing")
        .0
        .clone();

    let mut images = app.world_mut().resource_mut::<Assets<Image>>();
    let img = images.get_mut(&cpu_handle).unwrap();
    let row_bytes =
        img.width() as usize * img.texture_descriptor.format.pixel_size().unwrap();
    let aligned = RenderDevice::align_copy_bytes_per_row(row_bytes);
    img.data = Some(if row_bytes == aligned {
        data
    } else {
        data.chunks(aligned)
            .take(height as usize)
            .flat_map(|row| &row[..row_bytes.min(row.len())])
            .cloned()
            .collect()
    });

    let rgba = img
        .clone()
        .try_into_dynamic()
        .expect("image conversion failed")
        .to_rgba8();

    // Assert that the human figures actually rendered.
    let opaque_pixels = rgba.pixels().filter(|p| p.0[3] != 0).count();
    assert!(
        opaque_pixels >= MIN_CONTENT_PIXELS,
        "output has only {opaque_pixels} opaque pixels (need ≥ {MIN_CONTENT_PIXELS}); \
         human figures did not render — increase SETTLE_FRAMES or check animation setup"
    );

    let path = output_path();
    rgba.save(&path)
        .unwrap_or_else(|e| panic!("failed to write sprite sheet to {}: {e}", path.display()));
    println!("Wrote {} ({width}×{height}, {opaque_pixels} opaque px)", path.display());
}
