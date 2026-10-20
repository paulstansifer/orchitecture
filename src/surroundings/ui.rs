use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::change_guard::Guarded;
use crate::city::{ConstructedCity, ProposedCity};
use crate::resource::{ToolKind, UniformResource};
use crate::resource_icons::SMALL_SIZE;
use crate::ui_util::{icon, FontColors, FontSizes};
use crate::{label, note_label};

use super::attendance::{MAX_TRAVEL_COST, ROOM_CUTOFF};
use super::farmstead::{
    propose_road, road_blocker, specialize, unspecialize, FarmId, FarmsResource,
    SurroundingsState,
};
use super::map::fog_alpha_at;
use super::road_network::{RoadNetwork, RoadSegment};
use super::ui_view::{farm_panel_view, FarmPanelView};

const PIXELS_PER_UNIT: f32 = 8.0;
const CIRCLE_RADIUS: f32 = 18.0;
/// Opacity of an unpaved dirt-road line.
const DIRT_ROAD_ALPHA: u8 = 140;
/// Paved roads (built and proposed): thick, white, and slightly transparent.
const PAVED_ROAD_WIDTH: f32 = 4.0;
const PAVED_ROAD_COLOR: egui::Color32 = egui::Color32::from_rgba_premultiplied(220, 220, 220, 220);
/// How close (in screen pixels) the cursor must be to snap to a corner.
const CORNER_SNAP_PX: f32 = 10.0;
/// How close (in screen pixels) a click must be to a proposed road to cancel it.
const ROAD_CLICK_PX: f32 = 6.0;
const FOG_GRID_STEP_PX: f32 = 20.0;
const PANEL_W: f32 = 110.0;
/// Generous upper bound on a farm info panel's rendered height, used only to
/// decide whether an off-center panel still has a visible sliver on screen.
const PANEL_H_MAX: f32 = 140.0;

/// HSV (h in degrees, s/v in 0-1) to an sRGB `Color32`, treating the computed
/// RGB directly as gamma-space bytes. (egui's `Hsva` gamma-encodes instead, which
/// would visibly lighten these fills, so we keep the direct conversion.)
fn hsv_to_color32(h: f32, s: f32, v: f32) -> egui::Color32 {
    let h = h % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    egui::Color32::from_rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

fn farm_color(fertility: f32) -> egui::Color32 {
    let t = (fertility - 0.75) / 0.5;
    let hue = 80.0 + t * 40.0;
    let sat = 0.50 + t * 0.30;
    let val = 0.70 - t * 0.15;
    hsv_to_color32(hue, sat, val)
}

/// The map ↔ screen-pixel projection for the Surroundings panel: map-space
/// units are scaled by [`PIXELS_PER_UNIT`] and centered on `screen_centre`,
/// with `offset` panned by dragging; map Y increases upward, screen Y
/// downward, hence the sign flip in [`Self::to_screen`]/[`Self::to_map`].
struct MapView {
    screen_centre: egui::Pos2,
    offset: Vec2,
}

impl MapView {
    fn to_screen(&self, p: Vec2) -> egui::Pos2 {
        let rel = (p - self.offset) * PIXELS_PER_UNIT;
        egui::Pos2::new(self.screen_centre.x + rel.x, self.screen_centre.y - rel.y)
    }

    fn to_map(&self, p: egui::Pos2) -> Vec2 {
        Vec2::new(
            (p.x - self.screen_centre.x) / PIXELS_PER_UNIT + self.offset.x,
            -(p.y - self.screen_centre.y) / PIXELS_PER_UNIT + self.offset.y,
        )
    }
}

fn build_fog_mesh(panel_rect: egui::Rect, view: &MapView, paths: &[Vec<Vec2>]) -> egui::Mesh {
    let cols = (panel_rect.width() / FOG_GRID_STEP_PX).ceil() as usize + 1;
    let rows = (panel_rect.height() / FOG_GRID_STEP_PX).ceil() as usize + 1;
    let mut mesh = egui::Mesh::default();
    mesh.reserve_vertices(cols * rows);
    mesh.reserve_triangles((cols - 1) * (rows - 1) * 2);

    for row in 0..rows {
        for col in 0..cols {
            let sx = (panel_rect.min.x + col as f32 * FOG_GRID_STEP_PX).min(panel_rect.max.x);
            let sy = (panel_rect.min.y + row as f32 * FOG_GRID_STEP_PX).min(panel_rect.max.y);
            let alpha = fog_alpha_at(view.to_map(egui::Pos2::new(sx, sy)), paths);
            mesh.colored_vertex(
                egui::Pos2::new(sx, sy),
                egui::Color32::from_rgba_unmultiplied(160, 164, 180, alpha),
            );
        }
    }

    for row in 0..(rows - 1) {
        for col in 0..(cols - 1) {
            let i = (row * cols + col) as u32;
            let c = cols as u32;
            mesh.add_triangle(i, i + 1, i + c);
            mesh.add_triangle(i + 1, i + c + 1, i + c);
        }
    }
    mesh
}

/// Pass 1 of the map render: polygon fills (under fog), culled to `expanded`.
/// Returns the visible farms' `(id, screen centroid)` — with a panel reach
/// that still overlaps `panel_rect` (see [`PANEL_H_MAX`]) — for the
/// info-panel pass.
fn draw_farm_fills(
    painter: &egui::Painter,
    view: &MapView,
    farms: &FarmsResource,
    expanded: egui::Rect,
    panel_rect: egui::Rect,
) -> Vec<(FarmId, egui::Pos2)> {
    use egui::{Color32, Pos2, Shape, Stroke};

    let mut revealed = Vec::new();
    for (i, farm) in farms.farms.iter().enumerate() {
        let screen_pts: Vec<Pos2> = farm.polygon.iter().map(|&p| view.to_screen(p)).collect();
        if !screen_pts.iter().any(|p| expanded.contains(*p)) {
            continue;
        }

        let fill = farm_color(farm.fertility);
        let stroke = Stroke::new(1.0, Color32::from_gray(40));
        painter.add(Shape::convex_polygon(screen_pts, fill, stroke));

        let id = FarmId::new(i);
        if farms.is_visible(id) {
            let centroid = view.to_screen(farm.centroid());
            // Since the panel is centered on `centroid`, it can still have a
            // visible sliver even once `centroid` itself is off-panel.
            let panel_reach = panel_rect.expand2(egui::Vec2::new(PANEL_W / 2.0, PANEL_H_MAX / 2.0));
            if panel_reach.contains(centroid) {
                revealed.push((id, centroid));
            }
        }
    }
    revealed
}

/// Pass 2 of the map render: unpaved roads (under fog), in brown. Paved
/// roads are drawn separately, from their segments (see [`draw_paved_roads`]).
fn draw_dirt_roads(
    painter: &egui::Painter,
    view: &MapView,
    roads: &RoadNetwork,
    expanded: egui::Rect,
) {
    use egui::{Color32, Stroke};

    let brown = Color32::from_rgba_unmultiplied(120, 80, 40, DIRT_ROAD_ALPHA);
    for edge in roads.edges.iter().filter(|e| !e.paved) {
        let a = view.to_screen(roads.nodes[edge.a]);
        let b = view.to_screen(roads.nodes[edge.b]);
        if expanded.contains(a) || expanded.contains(b) {
            painter.line_segment([a, b], Stroke::new(1.5, brown));
        }
    }
}

/// In road-building mode, a farm panel's frame color: green (no room
/// needed), yellow (needs a room), or red (too far to come), by its travel
/// cost over `network` (which includes proposed roads).
fn distance_color(network: &RoadNetwork, polygon: &[Vec2]) -> egui::Color32 {
    use egui::Color32;
    match network.farm_delivery(polygon).map(|(w, _)| w) {
        Some(cost) if cost <= ROOM_CUTOFF => Color32::from_rgb(60, 200, 60),
        Some(cost) if cost <= MAX_TRAVEL_COST => Color32::from_rgb(230, 200, 40),
        _ => Color32::from_rgb(220, 50, 40),
    }
}

/// Paved roads, solid if built and dashed if only proposed.
fn draw_paved_roads(painter: &egui::Painter, view: &MapView, roads: &[RoadSegment], proposed: bool) {
    use egui::{Shape, Stroke};

    let stroke = Stroke::new(PAVED_ROAD_WIDTH, PAVED_ROAD_COLOR);
    for seg in roads {
        let pts = [view.to_screen(seg.a), view.to_screen(seg.b)];
        if proposed {
            painter.extend(Shape::dashed_line(&pts, stroke, 10.0, 6.0));
        } else {
            painter.line_segment(pts, stroke);
        }
    }
}

/// An arrow from each neighbor supplying a farm with a tool to that farm.
fn draw_supply_arrows(
    painter: &egui::Painter,
    view: &MapView,
    farms: &FarmsResource,
    supplies: &[(FarmId, FarmId, u32)],
) {
    use egui::{Color32, Stroke};

    let stroke = Stroke::new(2.5, Color32::from_rgb(250, 230, 150));
    for &(from, to, _) in supplies {
        if !farms.is_visible(from) || !farms.is_visible(to) {
            continue;
        }
        // Stop short of both centroids, so the arrow doesn't hide under panels.
        let a = farms[from].centroid();
        let b = farms[to].centroid();
        let start = view.to_screen(a.lerp(b, 0.3));
        let end = view.to_screen(a.lerp(b, 0.7));
        painter.arrow(start, end - start, stroke);
    }
}

/// Pass 4 of the map render (after the fog mesh, so it draws above it): the
/// navigation circle back to Build mode. Returns its screen-space centre, for
/// the click hit-test.
fn draw_nav_circle(painter: &egui::Painter, view: &MapView, circle_pos: Vec2) -> egui::Pos2 {
    use egui::{Color32, Pos2, Stroke};

    let cs = view.to_screen(circle_pos);
    painter.circle_filled(cs, CIRCLE_RADIUS, Color32::WHITE);
    painter.circle_stroke(cs, CIRCLE_RADIUS, Stroke::new(2.0, Color32::from_gray(40)));
    painter.text(
        Pos2::new(cs.x, cs.y + CIRCLE_RADIUS + 9.0),
        egui::Align2::CENTER_CENTER,
        "Build",
        FontSizes::body(),
        Color32::from_gray(20),
    );
    cs
}

pub fn enter_surroundings_mode(mut commands: Commands) {
    commands.insert_resource(SurroundingsState::default());
}

pub fn exit_surroundings_mode(mut commands: Commands) {
    commands.remove_resource::<SurroundingsState>();
}

/// The corner road-building would snap to under the cursor, and whether
/// clicking it would do anything.
struct CornerHover {
    node: usize,
    valid: bool,
}

/// Something a farm panel's buttons asked for.
enum FarmAction {
    TogglePause(FarmId),
    Specialize(FarmId),
    Unspecialize(FarmId),
}

#[allow(clippy::too_many_arguments)]
pub fn surroundings_ui_system(
    mut contexts: EguiContexts,
    mut farms: Guarded<FarmsResource>,
    mut state: ResMut<SurroundingsState>,
    mut constructed: Guarded<ConstructedCity>,
    mut pending: ResMut<ProposedCity>,
    sandbox: Res<crate::game_mode::SandboxMode>,
    cache: Res<crate::ui::MonthEffectsCache>,
    resource_icons: bevy::prelude::Res<crate::resource_icons::ResourceIcons>,
    mut next_game_mode: ResMut<NextState<crate::game_mode::GameMode>>,
    // The road network including proposed roads, keyed by every paved and
    // proposed segment, so it's rebuilt only when one of them changes.
    mut display_network: Local<Option<(Vec<RoadSegment>, RoadNetwork)>>,
) {
    use crate::game_mode::GameMode;
    use egui::Sense;

    let icon_textures_sm = resource_icons.texture_ids_small(&mut contexts);
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    // Filled by `crate::ui::update_economy_cache`, which runs first.
    let Some(effects) = cache.0.as_ref() else {
        return;
    };

    let network: Option<&RoadNetwork> = if state.road_mode {
        let key: Vec<RoadSegment> = farms
            .paved_roads
            .iter()
            .chain(&pending.proposed_roads)
            .copied()
            .collect();
        if display_network.as_ref().is_none_or(|(k, _)| *k != key) {
            let net = farms.road_network_with(&pending.proposed_roads);
            *display_network = Some((key, net));
        }
        display_network.as_ref().map(|(_, net)| net)
    } else {
        None
    };

    let mut pan_delta: Option<egui::Vec2> = None;
    let mut go_build = false;
    let mut clicked_at: Option<Vec2> = None;
    let mut right_clicked = false;
    let mut hover: Option<CornerHover> = None;

    // Collect revealed farm ids and their screen centroids.
    let mut revealed: Vec<(FarmId, egui::Pos2)> = Vec::new();
    let mut panel_rect = egui::Rect::NOTHING;

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let painter = ui.painter().clone();
            panel_rect = ui.max_rect();
            let screen_centre = panel_rect.center();

            painter.rect_filled(panel_rect, 0.0, egui::Color32::from_rgb(30, 30, 30));

            let view = MapView {
                screen_centre,
                offset: state.viewport_offset,
            };
            let expanded = panel_rect.expand(300.0);

            revealed = draw_farm_fills(&painter, &view, &farms, expanded, panel_rect);
            if let Some(roads) = farms.roads.as_ref() {
                draw_dirt_roads(&painter, &view, roads, expanded);
            }

            let fog_mesh = build_fog_mesh(panel_rect, &view, &farms.traveler_reveals);
            painter.add(egui::Shape::mesh(fog_mesh));

            draw_paved_roads(&painter, &view, &farms.paved_roads, false);
            draw_paved_roads(&painter, &view, &pending.proposed_roads, true);
            draw_supply_arrows(&painter, &view, &farms, &effects.production.supplies);

            let cs = draw_nav_circle(&painter, &view, farms.circle_pos);

            let response = ui.allocate_rect(panel_rect, Sense::click_and_drag());
            if response.dragged() {
                pan_delta = Some(response.drag_delta());
            }
            right_clicked = response.secondary_clicked();
            let cursor = response.hover_pos().map(|p| view.to_map(p));

            if let (Some(network), Some(cursor)) = (network, cursor) {
                hover = network
                    .nearest_corner_within(cursor, CORNER_SNAP_PX / PIXELS_PER_UNIT)
                    .and_then(|node| {
                        let valid = match state.road_start {
                            None => {
                                road_blocker(&farms, network, node, other_corner(network, node))
                                    .is_none()
                            }
                            Some(start) => road_blocker(
                                &farms,
                                network,
                                network.nearest_node(start),
                                node,
                            )
                            .is_none(),
                        };
                        // Before the first click, only corners a road can
                        // start from are worth pointing out.
                        (valid || state.road_start.is_some())
                            .then_some(CornerHover { node, valid })
                    });

                if let Some(start) = state.road_start {
                    let end = hover.as_ref().map_or(cursor, |h| network.nodes[h.node]);
                    let stroke = egui::Stroke::new(PAVED_ROAD_WIDTH, PAVED_ROAD_COLOR);
                    let pts = [view.to_screen(start), view.to_screen(end)];
                    painter.extend(egui::Shape::dashed_line(&pts, stroke, 10.0, 6.0));
                    if hover.as_ref().is_some_and(|h| h.valid) {
                        let cost = RoadSegment { a: start, b: end }.fieldstone_cost();
                        painter.text(
                            pts[1] + egui::vec2(12.0, -12.0),
                            egui::Align2::LEFT_BOTTOM,
                            format!("HUMANIZE: {cost} fieldstone"),
                            FontSizes::body(),
                            egui::Color32::WHITE,
                        );
                    }
                }
                if let Some(h) = &hover {
                    let color = if h.valid {
                        egui::Color32::WHITE
                    } else {
                        FontColors::problem()
                    };
                    painter.circle_stroke(
                        view.to_screen(network.nodes[h.node]),
                        CORNER_SNAP_PX * 0.8,
                        egui::Stroke::new(2.0, color),
                    );
                }
            }

            if response.clicked() {
                if let Some(ptr) = response.interact_pointer_pos() {
                    let dx = ptr.x - cs.x;
                    let dy = ptr.y - cs.y;
                    if dx * dx + dy * dy <= CIRCLE_RADIUS * CIRCLE_RADIUS && !state.road_mode {
                        go_build = true;
                    } else {
                        clicked_at = Some(view.to_map(ptr));
                    }
                }
            }
        });

    road_toolbar(ctx, panel_rect, &mut state);

    // ── Farm info panels ──────────────────────────────────────────────────────
    let mut actions = Vec::new();
    for (id, centroid) in revealed {
        let Some(view) = farm_panel_view(&farms, &constructed, effects, id) else {
            continue;
        };
        let border = network.map(|n| distance_color(n, &farms[id].polygon));
        if let Some(action) = farm_info_panel(
            ctx,
            panel_rect,
            id,
            centroid,
            &view,
            border,
            &icon_textures_sm,
        ) {
            actions.push(action);
        }
    }

    // ── Apply deferred actions ────────────────────────────────────────────────
    for action in actions {
        match action {
            FarmAction::TogglePause(id) => {
                let farm = &mut farms.mutate()[id];
                farm.paused = !farm.paused;
            }
            FarmAction::Specialize(id) => {
                let coming = effects.attendance.is_coming(id);
                specialize(
                    farms.mutate(),
                    constructed.mutate(),
                    id,
                    ToolKind::CarpentersTools,
                    coming,
                );
            }
            FarmAction::Unspecialize(id) => {
                unspecialize(farms.mutate(), constructed.mutate(), id);
            }
        }
    }

    if state.road_mode && (right_clicked || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
        if state.road_start.is_some() {
            state.road_start = None;
        } else {
            state.road_mode = false;
        }
        state.road_error = None;
    }

    if let Some(at) = clicked_at {
        let mut handled = false;
        if let Some(network) = network {
            match (state.road_start, &hover) {
                (None, Some(h)) if h.valid => {
                    state.road_start = Some(network.nodes[h.node]);
                    state.road_error = None;
                    handled = true;
                }
                (Some(start), Some(h)) => {
                    let a = network.nearest_node(start);
                    let result = propose_road(
                        farms.mutate(),
                        &mut pending,
                        network,
                        a,
                        h.node,
                        sandbox.enabled,
                    );
                    match result {
                        Ok(_) => {
                            state.road_start = None;
                            state.road_error = None;
                        }
                        Err(e) => state.road_error = Some(e),
                    }
                    handled = true;
                }
                (Some(_), None) => {
                    state.road_start = None;
                    handled = true;
                }
                _ => {}
            }
        }
        // Otherwise, clicking a proposed road cancels it.
        if !handled {
            let max_dist = ROAD_CLICK_PX / PIXELS_PER_UNIT;
            if let Some(i) = pending
                .proposed_roads
                .iter()
                .position(|seg| seg.distance_to(at) <= max_dist)
            {
                pending.proposed_roads.remove(i);
            }
        }
    }

    if let Some(delta) = pan_delta {
        state.viewport_offset.x -= delta.x / PIXELS_PER_UNIT;
        state.viewport_offset.y += delta.y / PIXELS_PER_UNIT;
    }
    if go_build {
        next_game_mode.set(GameMode::Build);
    }
}

/// Some corner other than `node`, so `road_blocker` can be asked whether
/// `node` would work as a road's *start* without a real end in mind yet. (Its
/// only end-specific check is that the two differ.)
fn other_corner(network: &RoadNetwork, node: usize) -> usize {
    if node == 0 {
        1
    } else {
        0
    }
    .min(network.corner_count.saturating_sub(1))
}

/// The road-building toggle and its hint, in the map's top-left corner.
fn road_toolbar(ctx: &egui::Context, panel_rect: egui::Rect, state: &mut SurroundingsState) {
    egui::Area::new(egui::Id::new("road_toolbar"))
        .fixed_pos(panel_rect.min + egui::vec2(8.0, 8.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(15, 15, 15, 210))
                .inner_margin(egui::Margin::same(4))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    if ui
                        .add(egui::Button::selectable(state.road_mode, "HUMANIZE: Pave roads"))
                        .clicked()
                    {
                        state.road_mode = !state.road_mode;
                        state.road_start = None;
                        state.road_error = None;
                    }
                    if state.road_mode {
                        note_label!(
                            ui,
                            "HUMANIZE: Click a corner on the city or a paved road, then any corner. \
                             Right-click or Esc cancels."
                        );
                        if let Some(err) = state.road_error {
                            note_label!(ui, crate::col_format!(problem, "{}", err));
                        }
                    }
                });
        });
}

/// Renders one visible farm's info panel at `centroid`, with its frame drawn
/// in `border` if given, returning whatever its buttons asked for.
fn farm_info_panel(
    ctx: &egui::Context,
    panel_rect: egui::Rect,
    id: FarmId,
    centroid: egui::Pos2,
    view: &FarmPanelView,
    border: Option<egui::Color32>,
    icon_textures_sm: &HashMap<UniformResource, egui::TextureId>,
) -> Option<FarmAction> {
    let mut action = None;
    egui::Area::new(egui::Id::new(("farm_panel", id)))
        .fixed_pos(centroid)
        .pivot(egui::Align2::CENTER_CENTER)
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_clip_rect(panel_rect);
            let stroke = border.map_or(egui::Stroke::NONE, |c| egui::Stroke::new(2.5, c));
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(15, 15, 15, 210))
                .stroke(stroke)
                .inner_margin(egui::Margin::same(4))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.set_max_width(PANEL_W);

                    // What it produces, then whether it's coming (and why not).
                    ui.horizontal(|ui| {
                        if let Some(&tex) = icon_textures_sm.get(&UniformResource::Potato) {
                            icon(ui, tex, SMALL_SIZE);
                        }
                        label!(ui, format!("{}", view.potatoes));
                        if let Some(&tex) = icon_textures_sm.get(&view.produced.0) {
                            icon(ui, tex, SMALL_SIZE);
                        }
                        label!(ui, format!("{}", view.produced.1));
                    });

                    if view.coming {
                        label!(ui, "attending market");
                    } else {
                        egui::CollapsingHeader::new(
                            egui::RichText::new("will not attend").font(FontSizes::body()),
                        )
                        .id_salt(("will_not_attend", id))
                        .default_open(false)
                        .show(ui, |ui| {
                            for reason in &view.reasons {
                                note_label!(ui, reason.as_str());
                            }
                        });
                    }

                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::selectable(view.paused, "⏸"))
                            .on_hover_text("HUMANIZE: Stay home from market")
                            .clicked()
                        {
                            action = Some(FarmAction::TogglePause(id));
                        }
                        if view.tool.is_some() {
                            if ui
                                .add_enabled(
                                    view.unspecialize.enabled(),
                                    egui::Button::selectable(true, "🔨"),
                                )
                                .on_hover_text("HUMANIZE: Take the carpenter's tools back (free)")
                                .on_disabled_hover_text(view.unspecialize.blocker.unwrap_or(""))
                                .clicked()
                            {
                                action = Some(FarmAction::Unspecialize(id));
                            }
                        } else if ui
                            .add_enabled(view.specialize.enabled(), egui::Button::new("🔨"))
                            .on_hover_text(format!(
                                "HUMANIZE: Give carpenter's tools ({} potatoes): turns neighbors' \
                                 timber into planks",
                                super::farmstead::SPECIALIZE_COST
                            ))
                            .on_disabled_hover_text(view.specialize.blocker.unwrap_or(""))
                            .clicked()
                        {
                            action = Some(FarmAction::Specialize(id));
                        }
                    });
                });
        });
    action
}
