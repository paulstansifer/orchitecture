//! Shared mesh-raycast helper for turning a cursor ray into a `SlotCoord`, plus
//! the hover-highlight ring and the rotating-stripe selected-item ring that use
//! it. Grid cells are spawned as `SceneRoot`s (`city::apply_changes`), so the
//! actual pickable meshes are descendants; a hit is resolved back to its
//! owning cell by walking `ChildOf` up to the entity carrying `GridCellMarker`.

use std::collections::HashSet;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::window::PrimaryWindow;
use bevy_egui::input::EguiWantsInput;
use bevy_picking::mesh_picking::ray_cast::RayMeshHit;
use bevy_picking::prelude::{MeshRayCast, MeshRayCastSettings};

use crate::camera::{cursor_to_viewport, GameCamera};
use crate::city::{slot_center, ConstructedCity, GridCellMarker, PlaceHighlight};
use crate::cutaway::CutawayHidden;
use crate::sparse3d::{Slot, SlotCoord};

/// Casts `ray` against real scene geometry and resolves the hit back to the
/// `SlotCoord` of the `GridCellMarker`-tagged ancestor that owns it. `filter`
/// should exclude any non-cell overlay geometry (hover/selection rings) that
/// might otherwise sit in front of the real mesh and swallow the hit.
pub fn raycast_grid_cell(
    ray: Ray3d,
    ray_cast: &mut MeshRayCast,
    child_of_q: &Query<&ChildOf>,
    cell_markers_q: &Query<&GridCellMarker>,
    filter: &impl Fn(Entity) -> bool,
) -> Option<SlotCoord> {
    let settings = MeshRayCastSettings::default().with_filter(filter);
    let (hit_entity, _) = ray_cast.cast_ray(ray, &settings).first()?;
    let mut node = *hit_entity;
    loop {
        if let Ok(marker) = cell_markers_q.get(node) {
            return Some(marker.loc);
        }
        node = child_of_q.get(node).ok()?.0;
    }
}

/// Marker components whose entities (or descendants thereof) should never be
/// hit by a grid-cell ray cast:
/// - `HoverHighlightMarker`/`PlaceHighlightMarker`: this crate's own overlay
///   geometry, which sits in front of the real mesh it's marking -- otherwise
///   hit-testing would flicker between the overlay (no `GridCellMarker`
///   ancestor, so no hit) and the real mesh frame to frame, and right-clicks
///   landing on an overlay would silently do nothing.
/// - `CutawayHidden`: cells hidden by the cutaway view. They stay fully
///   present as geometry (only their *material* is swapped to
///   `ShadowOnlyMaterial`, so they keep casting shadows -- see
///   `cutaway::sync_cutaway_shadow_material`), so without this exclusion a
///   ray would stop on an invisible wall/ceiling instead of reaching the
///   furniture the user can actually see and is trying to click.
pub type ExcludeFromPickQuery<'w, 's> = Query<
    'w,
    's,
    (),
    Or<(
        With<HoverHighlightMarker>,
        With<PlaceHighlightMarker>,
        With<CutawayHidden>,
    )>,
>;

/// Walks a candidate entity's `ChildOf` ancestors (inclusive) and reports
/// whether any of them match `ExcludeFromPickQuery` -- `CutawayHidden` in
/// particular is set on the `GridCellMarker` root, not the mesh leaves a ray
/// actually hits.
pub fn is_pick_excluded(
    entity: Entity,
    exclude_q: &ExcludeFromPickQuery,
    child_of_q: &Query<&ChildOf>,
) -> bool {
    let mut node = entity;
    loop {
        if exclude_q.contains(node) {
            return true;
        }
        match child_of_q.get(node) {
            Ok(child_of) => node = child_of.0,
            Err(_) => return false,
        }
    }
}

/// Casts `ray` against real scene geometry, excluding whatever `is_pick_excluded`
/// would drop (overlay geometry, cutaway-hidden cells) plus every entity in
/// `extra_excluded` (e.g. the picker's own model, which shouldn't stop its own
/// click-to-move ray). Returns the closest hit, if any.
pub fn cast_ray_excluding<'a>(
    ray: Ray3d,
    ray_cast: &'a mut MeshRayCast,
    extra_excluded: &HashSet<Entity>,
    exclude_q: &ExcludeFromPickQuery,
    child_of_q: &Query<&ChildOf>,
) -> Option<&'a RayMeshHit> {
    let filter = |entity: Entity| {
        !extra_excluded.contains(&entity) && !is_pick_excluded(entity, exclude_q, child_of_q)
    };
    let settings = MeshRayCastSettings::default().with_filter(&filter);
    ray_cast
        .cast_ray(ray, &settings)
        .first()
        .map(|(_, hit)| hit)
}

/// Bundles the raycast machinery into a single `SystemParam` so callers (like
/// `building_input_system`, which is already near Bevy's 16-parameter limit)
/// only spend one parameter slot on mesh-picking.
#[derive(SystemParam)]
pub struct GridRaycast<'w, 's> {
    ray_cast: MeshRayCast<'w, 's>,
    child_of_q: Query<'w, 's, &'static ChildOf>,
    cell_markers_q: Query<'w, 's, &'static GridCellMarker>,
    exclude_q: ExcludeFromPickQuery<'w, 's>,
}

impl GridRaycast<'_, '_> {
    pub fn cast(&mut self, ray: Ray3d) -> Option<SlotCoord> {
        let filter = |entity: Entity| !is_pick_excluded(entity, &self.exclude_q, &self.child_of_q);
        raycast_grid_cell(
            ray,
            &mut self.ray_cast,
            &self.child_of_q,
            &self.cell_markers_q,
            &filter,
        )
    }
}

/// Builds the cursor's world-space ray for the primary window/camera, or
/// `None` if the cursor is outside the window or the camera isn't ready.
pub fn cursor_ray(
    windows: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<GameCamera>>,
) -> Option<Ray3d> {
    let window = windows.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, camera_transform) = camera_q.single().ok()?;
    camera
        .viewport_to_world(camera_transform, cursor_to_viewport(window, camera, cursor))
        .ok()
}

/// Non-uniform scale that stretches a flat, ground-lying ring mesh (see
/// `ring_transform`) into an oval spanning a wall slot's run direction
/// (mirrors `city::protrude_axis`'s choice of the *normal* axis: the run
/// direction is the other horizontal axis). Room/Floor markers stay circular.
///
/// The ring mesh is rotated -90° about X to lie flat, which leaves local X
/// mapped to world X and local Y mapped to world Z -- so stretching along
/// world X scales local X, and stretching along world Z scales local Y.
fn ring_stretch_scale(slot: Slot) -> Vec3 {
    const STRETCH: f32 = 1.8;
    match slot {
        Slot::ZLoWall => Vec3::new(STRETCH, 1.0, 1.0),
        Slot::XLoWall => Vec3::new(1.0, STRETCH, 1.0),
        Slot::Room | Slot::Floor => Vec3::ONE,
    }
}

/// Flat-on-the-ground transform for a ring/hover marker: rotates the `Circle`
/// mesh (authored in the XY plane, normal +Z) to lie on the ground plane (XZ,
/// normal +Y), 0.2 units above the cube's floor, stretched for wall slots.
fn ring_transform(loc: SlotCoord) -> Transform {
    let mut center = slot_center(loc);
    center.y = loc.cube.y as f32 + 0.2;
    Transform::from_translation(center)
        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
        .with_scale(ring_stretch_scale(loc.slot))
}

/// Shared ring-mask material: an annulus (not a filled disc) computed in the
/// fragment shader from the mesh's UVs, so a single flat `Circle` mesh serves
/// both the solid hover ring and the striped selection ring.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct RingMaterial {
    /// x = elapsed time (drives the striped variant's rotation), y = striped
    /// flag (nonzero selects the rotating blue/white stripes and ignores
    /// `color`), z/w unused.
    #[uniform(0)]
    pub params: Vec4,
    /// Solid ring color (rgb) and alpha, used when `params.y == 0`. Shares
    /// binding 0 with `params` -- `AsBindGroup` merges same-index uniform
    /// fields into a single combined buffer, matching the WGSL struct below.
    #[uniform(0)]
    pub color: Vec4,
}

impl Material for RingMaterial {
    fn fragment_shader() -> ShaderRef {
        "assets/static/shaders/selection_ring.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

#[derive(Resource)]
pub struct RingMeshAssets {
    mesh: Handle<Mesh>,
}

pub fn spawn_ring_mesh_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    // A flat disc lying in the XZ plane; the shader masks it down to a ring.
    let mesh = meshes.add(Circle::new(0.45).mesh().resolution(32));
    commands.insert_resource(RingMeshAssets { mesh });
}

// ---------------------------------------------------------------------------
// Hover highlight: a solid amber ring on the furniture cell under the cursor.
// ---------------------------------------------------------------------------

#[derive(Resource, Default, PartialEq)]
pub struct HoveredFurniture(pub Option<SlotCoord>);

#[derive(Component)]
pub struct HoverHighlightMarker;

/// Raycasts from the cursor every frame to find the hovered furniture cell,
/// and (re)spawns the ring only when the hovered cell changes.
pub fn update_hover_highlight(
    mut commands: Commands,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    egui_wants_input: Res<EguiWantsInput>,
    mut ray_cast: MeshRayCast,
    child_of_q: Query<&ChildOf>,
    cell_markers_q: Query<&GridCellMarker>,
    exclude_q: ExcludeFromPickQuery,
    constructed: Res<ConstructedCity>,
    mut hovered: ResMut<HoveredFurniture>,
    mesh_assets: Option<Res<RingMeshAssets>>,
    mut materials: ResMut<Assets<RingMaterial>>,
    existing: Query<Entity, With<HoverHighlightMarker>>,
) {
    let filter = |entity: Entity| !is_pick_excluded(entity, &exclude_q, &child_of_q);
    let new_hover = (!egui_wants_input.wants_any_pointer_input())
        .then(|| cursor_ray(&windows, &camera_q))
        .flatten()
        .and_then(|ray| {
            raycast_grid_cell(ray, &mut ray_cast, &child_of_q, &cell_markers_q, &filter)
        })
        .filter(|loc| {
            constructed
                .contents
                .get(*loc)
                .is_some_and(|cell| constructed.eorfs[cell.id.as_usize()].is_furniture())
        });

    if hovered.0 == new_hover {
        return;
    }
    hovered.0 = new_hover;

    for entity in &existing {
        commands.entity(entity).despawn();
    }
    let Some(mesh_assets) = mesh_assets else {
        return;
    };
    if let Some(loc) = hovered.0 {
        let mat = materials.add(RingMaterial {
            params: Vec4::ZERO,
            // Warm amber, distinct from the selection ring's blue/white.
            color: Vec4::new(1.0, 0.7, 0.15, 0.55),
        });
        commands.spawn((
            Mesh3d(mesh_assets.mesh.clone()),
            MeshMaterial3d(mat),
            ring_transform(loc),
            HoverHighlightMarker,
        ));
    }
}

// ---------------------------------------------------------------------------
// Selected-item ring: a flat, slowly-rotating blue/white-striped ring 0.2
// units above the floor, for each cube in `PlaceHighlight`. Replaces the
// former translucent cyan cuboid.
// ---------------------------------------------------------------------------

#[derive(Component)]
pub struct PlaceHighlightMarker;

/// Despawns prior selection-ring overlays and spawns one per highlighted
/// furniture cube whenever the highlight set changes.
pub fn update_selection_ring(
    mut commands: Commands,
    highlight: Res<PlaceHighlight>,
    mesh_assets: Option<Res<RingMeshAssets>>,
    mut materials: ResMut<Assets<RingMaterial>>,
    existing: Query<Entity, With<PlaceHighlightMarker>>,
) {
    if !highlight.is_changed() {
        return;
    }
    let Some(mesh_assets) = mesh_assets else {
        return;
    };
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    for &loc in &highlight.0 {
        let mat = materials.add(RingMaterial {
            params: Vec4::new(0.0, 1.0, 0.0, 0.0),
            color: Vec4::ZERO,
        });
        commands.spawn((
            Mesh3d(mesh_assets.mesh.clone()),
            MeshMaterial3d(mat),
            ring_transform(loc),
            PlaceHighlightMarker,
        ));
    }
}

/// Advances the rotating-stripe animation on every live selection-ring material.
pub fn animate_selection_rings(
    time: Res<Time>,
    ring_q: Query<&MeshMaterial3d<RingMaterial>, With<PlaceHighlightMarker>>,
    mut materials: ResMut<Assets<RingMaterial>>,
) {
    for handle in &ring_q {
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.params.x = time.elapsed_secs();
        }
    }
}
