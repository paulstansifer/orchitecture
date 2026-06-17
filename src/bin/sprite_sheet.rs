//! Sprite-sheet generator — headless mode.
//!
//! Generates two PNG files:
//!   `orcs/characters_sheet.png` — columns = characters, rows = Y-rotations (0°, 90°, 180°, 270°).
//!   `orcs/structures_sheet.png` — columns = structures, rows = Y-rotations.
//!
//! Each sprite is rendered inside a wireframe unit cube. Cell size is
//! 128×128 px of content plus 10 px of padding on every side (148×148 px total).
//! The image is sized to exactly fit its contents.
//!
//! Usage:  cargo run --bin sprite_sheet

use bevy::camera::{RenderTarget, ScalingMode};
use bevy::image::{Image, TextureFormatPixelInfo};
use bevy::prelude::*;
use bevy::render::{
    render_asset::RenderAssets,
    render_graph::{self, NodeRunError, RenderGraph, RenderGraphContext, RenderLabel},
    render_resource::{
        Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, PollType,
        TexelCopyBufferInfo, TexelCopyBufferLayout,
    },
    renderer::{RenderContext, RenderDevice, RenderQueue},
    texture::GpuImage,
    Extract, Render, RenderApp, RenderSystems,
};
use bevy::scene::SceneInstanceReady;
use bevy::winit::WinitPlugin;
use orchitecture_lib::ortho_camera::{trimetric_camera_basis, PIXELS_PER_UNIT};
use orchitecture_lib::paths::MANIFEST_DIR;
use std::sync::{mpsc, Mutex};

// --- Configuration knobs -------------------------------------------------
/// TODO: this is ad-hoc.
const CELL_TOTAL_PX: u32 = 2 * PIXELS_PER_UNIT - 16;
/// World-space size of one cell.
const CELL_WORLD: f32 = CELL_TOTAL_PX as f32 / PIXELS_PER_UNIT as f32;

/// Number of Y-rotation rows in each sprite sheet (0°, 90°, 180°, 270°).
const N_ROTATIONS: u32 = 4;
/// Number of walk-cycle phase columns in the characters sheet.
const N_WALK_PHASES: u32 = 6;
/// Maximum number of building sprites to include in the structures sheet.
const MAX_BUILDINGS: usize = 16;

/// The GLB to load for the human character, relative to the asset root.
const MODEL_ASSET: &str = "orcs/human_1.glb";
/// Index of the "Walk" clip within the GLB's animation list.
/// Order: Charge-Punch(0), Idle(1), Left-Punch(2), Run(3), T-Pose(4), Walk(5), Rest(6), T-pose(7).
const WALK_ANIMATION_INDEX: usize = 5;
/// Named entity to despawn from the human GLB (we keep HumanMale only).
const DROP_MESH_NAME: &str = "HumanFemale";

/// Y-axis rotation angles for the four rows (radians).
const Y_ROTATIONS: [f32; 4] = [
    0.0,
    std::f32::consts::FRAC_PI_2,
    std::f32::consts::PI,
    3.0 * std::f32::consts::FRAC_PI_2,
];

/// Outward surface inflation for the human model (world units). 0 = disabled.
const BULK_WORLD_UNITS: f32 = 0.15;
const MESH_UNITS_PER_WORLD_UNIT: f32 = 0.66;

/// World-space Y the camera looks at (midpoint of the unit cube height).
const LOOK_HEIGHT: f32 = 0.5;
/// Camera distance (irrelevant for orthographic scale, must clear near plane).
const CAMERA_DISTANCE: f32 = 30.0;

/// Frames to settle after posing before capture.
const SETTLE_FRAMES: u32 = 30;
/// Maximum frames to wait for asset loading before panicking.
const MAX_LOAD_FRAMES: u32 = 600;
/// Minimum opaque pixels in the final image (sanity check).
const MIN_CONTENT_PIXELS: usize = 3000;

fn characters_output_path() -> std::path::PathBuf {
    std::path::Path::new(MANIFEST_DIR).join("orcs/characters_sheet.png")
}

fn structures_output_path() -> std::path::PathBuf {
    std::path::Path::new(MANIFEST_DIR).join("orcs/structures_sheet.png")
}

// --- GPU readback snap infrastructure ------------------------------------
//
// Architecture mirrors vector-arena/src/bin/headless/runner.rs.

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
            world
                .resource::<RenderQueue>()
                .submit(std::iter::once(encoder.finish()));
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
        render_device
            .poll(PollType::wait_indefinitely())
            .expect("poll failed");
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

#[derive(Clone)]
enum SheetKind {
    Characters,
    Structures,
}

#[derive(Resource, Clone)]
struct SheetConfig {
    kind: SheetKind,
    /// Building names for Structures sheets; empty for Characters.
    buildings: Vec<String>,
}

#[derive(Resource)]
struct Capture {
    /// Animation graph/node/clip; only present for Characters sheets.
    anim: Option<(
        Handle<AnimationGraph>,
        AnimationNodeIndex,
        Handle<AnimationClip>,
    )>,
    image: Handle<Image>,
    img_width: u32,
    img_height: u32,
    /// Total scenes spawned; we wait until `ready` reaches this.
    n_total_scenes: u32,
    ready: u32,
    configured: bool,
}

/// Identifies one sprite in the output grid.
#[derive(Component, Clone, Copy)]
struct SpriteCell {
    _row: u32,
    col: u32,
    is_character: bool,
}

// --- Layout helpers ------------------------------------------------------

/// World-space origin for a sprite cell.
///
/// Layout: columns = items, rows = rotations (row 0 at top = 0°).
fn cell_origin(cam_r: Vec3, cam_u: Vec3, col: u32, row: u32, n_cols: u32) -> Vec3 {
    let sx = (col as f32 + 0.5 - n_cols as f32 / 2.0) * CELL_WORLD;
    let sy = (N_ROTATIONS as f32 / 2.0 - row as f32 - 0.5) * CELL_WORLD;
    // Add an ad-hoc adjustment in y to keep on-screen:
    cam_r * sx + cam_u * sy + Vec3::new(0.0, -0.75, 0.0)
}

// --- Setup ---------------------------------------------------------------

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    config: Res<SheetConfig>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (cam_r, cam_u, cam_forward) = trimetric_camera_basis();

    match config.kind {
        SheetKind::Characters => {
            let n_cols = N_WALK_PHASES;
            let img_width = n_cols * CELL_TOTAL_PX;
            let img_height = N_ROTATIONS * CELL_TOTAL_PX;
            let n_total_scenes = n_cols * N_ROTATIONS;

            let clip: Handle<AnimationClip> = asset_server
                .load(GltfAssetLabel::Animation(WALK_ANIMATION_INDEX).from_asset(MODEL_ASSET));
            let (graph, node) = AnimationGraph::from_clip(clip.clone());
            let graph = graphs.add(graph);

            let mut image = Image::new_target_texture(
                img_width,
                img_height,
                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                None,
            );
            image.texture_descriptor.usage |=
                bevy::render::render_resource::TextureUsages::COPY_SRC;
            let image = images.add(image);

            let human_scene: Handle<Scene> =
                asset_server.load(GltfAssetLabel::Scene(0).from_asset(MODEL_ASSET));

            for (row, &angle) in Y_ROTATIONS.iter().enumerate() {
                for col in 0..N_WALK_PHASES {
                    let origin = cell_origin(cam_r, cam_u, col, row as u32, n_cols);
                    commands.spawn((
                        SceneRoot(human_scene.clone()),
                        Transform::from_translation(origin + Vec3::new(0.5, 0.0, 0.5))
                            .with_rotation(Quat::from_rotation_y(angle))
                            .with_scale(Vec3::splat(0.5)),
                        SpriteCell {
                            _row: row as u32,
                            col,
                            is_character: true,
                        },
                    ));
                    spawn_wireframe_cube(&mut commands, &mut meshes, &mut materials, origin);
                }
            }

            spawn_camera_and_lights(&mut commands, cam_r, cam_u, cam_forward, image.clone());

            commands.insert_resource(Capture {
                anim: Some((graph, node, clip)),
                image,
                img_width,
                img_height,
                n_total_scenes,
                ready: 0,
                configured: false,
            });
        }

        SheetKind::Structures => {
            let n_cols = config.buildings.len() as u32;
            let img_width = n_cols * CELL_TOTAL_PX;
            let img_height = N_ROTATIONS * CELL_TOTAL_PX;
            let n_total_scenes = n_cols * N_ROTATIONS;

            let mut image = Image::new_target_texture(
                img_width,
                img_height,
                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                None,
            );
            image.texture_descriptor.usage |=
                bevy::render::render_resource::TextureUsages::COPY_SRC;
            let image = images.add(image);

            for (col, name) in config.buildings.iter().enumerate() {
                let path = format!("buildables/autotile/{name}.gltf");
                let scene: Handle<Scene> =
                    asset_server.load(GltfAssetLabel::Scene(0).from_asset(path));
                for (row, &angle) in Y_ROTATIONS.iter().enumerate() {
                    let origin = cell_origin(cam_r, cam_u, col as u32, row as u32, n_cols);
                    let mut tform = Transform::from_translation(origin + Vec3::new(0.0, 0.0, 1.0))
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2));
                    tform.rotate_around(
                        origin + Vec3::new(0.5, 0.0, 0.5),
                        Quat::from_rotation_y(angle),
                    );

                    commands.spawn((
                        SceneRoot(scene.clone()),
                        tform,
                        SpriteCell {
                            _row: row as u32,
                            col: col as u32,
                            is_character: false,
                        },
                    ));
                    spawn_wireframe_cube(&mut commands, &mut meshes, &mut materials, origin);
                }
            }

            spawn_camera_and_lights(&mut commands, cam_r, cam_u, cam_forward, image.clone());

            commands.insert_resource(Capture {
                anim: None,
                image,
                img_width,
                img_height,
                n_total_scenes,
                ready: 0,
                configured: false,
            });
        }
    }

    commands.add_observer(|_: On<SceneInstanceReady>, mut capture: ResMut<Capture>| {
        capture.ready += 1;
    });
}

fn spawn_camera_and_lights(
    commands: &mut Commands,
    cam_r: Vec3,
    cam_u: Vec3,
    cam_forward: Vec3,
    image: Handle<Image>,
) {
    let look_at = Vec3::Y * LOOK_HEIGHT;
    // ad-hoc: bumping by 0.5 pixels X lines up the bars better.
    let cam_pos =
        look_at - cam_forward * CAMERA_DISTANCE + Vec3::new(0.5 / PIXELS_PER_UNIT as f32, 0.0, 0.0);
    let rotation = Quat::from_mat3(&Mat3::from_cols(cam_r, cam_u, cam_r.cross(cam_u)));
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        RenderTarget::Image(image.into()),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: N_ROTATIONS as f32 * CELL_WORLD,
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
            illuminance: 1_000.0,
            ..default()
        },
        Transform::from_xyz(8.0, 12.0, -6.0).looking_at(look_at, Vec3::Y),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 2_000.0,
            ..default()
        },
        Transform::from_xyz(-8.0, 12.0, 6.0).looking_at(look_at, Vec3::Y),
    ));
}

/// Creates the GPU readback buffer and CPU image asset. Must run after `setup`.
fn setup_image_copier(
    mut commands: Commands,
    capture: Res<Capture>,
    render_device: Res<RenderDevice>,
    mut images: ResMut<Assets<Image>>,
) {
    let row_bytes = capture.img_width as usize * 4; // Rgba8UnormSrgb = 4 bytes/pixel
    let padded_row = RenderDevice::align_copy_bytes_per_row(row_bytes);
    let buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some("sprite_sheet_readback"),
        size: (padded_row * capture.img_height as usize) as u64,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    commands.spawn(ImageCopier {
        buffer,
        src_image: capture.image.clone(),
    });

    let cpu_image = Image::new_target_texture(
        capture.img_width,
        capture.img_height,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    );
    commands.spawn(SnapCpuImageHandle(images.add(cpu_image)));
}

// --- Systems -------------------------------------------------------------

/// Once all scenes (and the Walk clip for characters) are loaded, finish setup.
fn configure_when_ready(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    clips: Res<Assets<AnimationClip>>,
    mut players: Query<(Entity, &mut AnimationPlayer)>,
    parents: Query<&ChildOf>,
    cells: Query<&SpriteCell>,
    names: Query<(Entity, &Name)>,
    mesh_nodes: Query<(Entity, &Name, &Mesh3d)>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
) {
    if capture.configured || capture.ready < capture.n_total_scenes {
        return;
    }

    // For structure sheets there is no animation to configure.
    let Some((ref graph, node, ref clip)) = capture.anim.clone() else {
        capture.configured = true;
        return;
    };

    let Some(clip_asset) = clips.get(clip) else {
        return;
    };
    let clip_duration = clip_asset.duration();

    for (entity, mut player) in &mut players {
        let Some(cell) = find_cell(entity, &parents, &cells) else {
            continue;
        };
        if !cell.is_character {
            continue;
        }
        let t = cell.col as f32 / N_WALK_PHASES as f32 * clip_duration;
        let active = player.play(node);
        active.seek_to(t);
        active.pause();
        commands
            .entity(entity)
            .insert(AnimationGraphHandle(graph.clone()));
    }

    // Remove the unused female model from every human scene.
    for (entity, name) in &names {
        if name.as_str() == DROP_MESH_NAME {
            commands.entity(entity).despawn();
        }
    }

    // Inflate only character meshes.
    if BULK_WORLD_UNITS != 0.0 {
        let amount = BULK_WORLD_UNITS * MESH_UNITS_PER_WORLD_UNIT;
        let mut done = std::collections::HashSet::new();
        for (mesh_entity, name, mesh3d) in &mesh_nodes {
            if name.as_str() == DROP_MESH_NAME || !done.insert(mesh3d.0.id()) {
                continue;
            }
            let Some(cell) = find_cell(mesh_entity, &parents, &cells) else {
                continue;
            };
            if !cell.is_character {
                continue;
            }
            if let Some(mesh) = mesh_assets.get_mut(&mesh3d.0) {
                inflate_mesh(mesh, amount);
            }
        }
    }

    capture.configured = true;
}

// --- Pure helpers --------------------------------------------------------

/// Spawns a wireframe unit cube (0..1 on each axis) centred at `base`.
fn spawn_wireframe_cube(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    base: Vec3,
) {
    let radius = 0.5 / PIXELS_PER_UNIT as f32;
    let material = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        emissive: Color::srgb(0.0, 0.0, 1.0).into(),
        ..default()
    });
    let edges: &[(Vec3, Vec3)] = &[
        (Vec3::new(0., 0., 0.), Vec3::new(1., 0., 0.)),
        (Vec3::new(0., 0., 0.), Vec3::new(0., 0., 1.)),
        (Vec3::new(1., 0., 0.), Vec3::new(1., 0., 1.)),
        (Vec3::new(0., 0., 1.), Vec3::new(1., 0., 1.)),
        (Vec3::new(0., 1., 0.), Vec3::new(1., 1., 0.)),
        (Vec3::new(0., 1., 0.), Vec3::new(0., 1., 1.)),
        (Vec3::new(1., 1., 0.), Vec3::new(1., 1., 1.)),
        (Vec3::new(0., 1., 1.), Vec3::new(1., 1., 1.)),
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
        let mesh = meshes.add(Cylinder {
            radius,
            half_height: len * 0.5,
        });
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform {
                translation: mid,
                rotation: Quat::from_rotation_arc(Vec3::Y, dir),
                scale: Vec3::ONE,
            },
        ));
    }
}

/// Walk up the parent chain to find the `SpriteCell` an entity belongs to.
fn find_cell(
    mut entity: Entity,
    parents: &Query<&ChildOf>,
    cells: &Query<&SpriteCell>,
) -> Option<SpriteCell> {
    loop {
        if let Ok(cell) = cells.get(entity) {
            return Some(*cell);
        }
        entity = parents.get(entity).ok()?.parent();
    }
}

/// Inflates a skinned mesh outward along welded normals.
fn inflate_mesh(mesh: &mut Mesh, amount: f32) {
    use bevy::mesh::VertexAttributeValues::Float32x3;
    let Some(Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        return;
    };
    let positions = positions.clone();
    let Some(Float32x3(normals)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL) else {
        return;
    };
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

// --- Capture -------------------------------------------------------------

fn save_snap(app: &mut App, path: &std::path::Path) {
    while app.world().resource::<SnapBytesRx>().try_recv().is_some() {}
    app.update();
    let mut data = None;
    for _ in 0..8 {
        if let Some(d) = app.world().resource::<SnapBytesRx>().try_recv() {
            data = Some(d);
            break;
        }
        app.update();
    }
    let data = data.expect("GPU readback timed out — render pipeline produced no bytes");

    let (img_width, img_height) = {
        let c = app.world().resource::<Capture>();
        (c.img_width, c.img_height)
    };

    let mut cpu_q = app.world_mut().query::<&SnapCpuImageHandle>();
    let cpu_handle = cpu_q
        .single(app.world())
        .expect("SnapCpuImageHandle missing")
        .0
        .clone();

    let mut images = app.world_mut().resource_mut::<Assets<Image>>();
    let img = images.get_mut(&cpu_handle).unwrap();
    let row_bytes = img.width() as usize * img.texture_descriptor.format.pixel_size().unwrap();
    let aligned = RenderDevice::align_copy_bytes_per_row(row_bytes);
    img.data = Some(if row_bytes == aligned {
        data
    } else {
        data.chunks(aligned)
            .take(img_height as usize)
            .flat_map(|row| &row[..row_bytes.min(row.len())])
            .cloned()
            .collect()
    });

    let rgba = img
        .clone()
        .try_into_dynamic()
        .expect("image conversion failed")
        .to_rgba8();

    let opaque_pixels = rgba.pixels().filter(|p| p.0[3] != 0).count();
    assert!(
        opaque_pixels >= MIN_CONTENT_PIXELS,
        "output has only {opaque_pixels} opaque pixels (need ≥ {MIN_CONTENT_PIXELS}); \
         sprites did not render — increase SETTLE_FRAMES or check scene setup"
    );

    rgba.save(path)
        .unwrap_or_else(|e| panic!("failed to write sprite sheet: {e}"));
    println!(
        "Wrote {} ({}×{}, {opaque_pixels} opaque px)",
        path.display(),
        img_width,
        img_height
    );
}

// --- Main ----------------------------------------------------------------

fn find_matching_buildings() -> Vec<String> {
    let base = std::path::Path::new(MANIFEST_DIR);
    let scad_names: std::collections::HashSet<String> = std::fs::read_dir(base.join("buildables"))
        .expect("buildables/ not found")
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? == "scad" {
                Some(p.file_stem()?.to_str()?.to_string())
            } else {
                None
            }
        })
        .collect();

    let mut matches: Vec<String> = std::fs::read_dir(base.join("buildables/autotile"))
        .expect("buildables/autotile/ not found")
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? == "gltf" {
                let name = p.file_stem()?.to_str()?.to_string();
                scad_names.contains(&name).then_some(name)
            } else {
                None
            }
        })
        .collect();

    matches.sort();
    matches.truncate(MAX_BUILDINGS);
    matches
}

fn run_sheet(config: SheetConfig) {
    let output_path = match config.kind {
        SheetKind::Characters => characters_output_path(),
        SheetKind::Structures => structures_output_path(),
    };

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
    .insert_resource(config)
    .add_systems(Startup, (setup, setup_image_copier).chain())
    .add_systems(Update, configure_when_ready);

    app.finish();

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

    for _ in 0..SETTLE_FRAMES {
        app.update();
    }

    save_snap(&mut app, &output_path);
}

fn main() {
    let buildings = find_matching_buildings();
    println!(
        "Building sprites: {} ({})",
        buildings.len(),
        buildings.join(", ")
    );

    run_sheet(SheetConfig {
        kind: SheetKind::Characters,
        buildings: vec![],
    });
    run_sheet(SheetConfig {
        kind: SheetKind::Structures,
        buildings,
    });

    std::process::exit(0);
}
