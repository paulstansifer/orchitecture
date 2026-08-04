use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::change_guard::{edit_if_changed, Guarded};
use crate::resource::UniformResource;
use crate::resource_icons::SMALL_SIZE;
use crate::ui_util::{icon, FontSizes};
use crate::{col_format, label, note_label};

use super::farmstead::{
    FarmEvent, FarmId, FarmsResource, MarketModeEffect, SurroundingsState, MARKET_BOOST,
};
use super::map::{fog_alpha_at, REVEAL_THRESHOLD};
use super::ui_view::{farm_menu_view, farm_panel_view, FarmMenuView, FarmPanelView};
use crate::city_effect::{CityEffect, MonthInputs};

const PIXELS_PER_UNIT: f32 = 8.0;
const CIRCLE_RADIUS: f32 = 18.0;
/// Opacity of a fully-developed dirt-road line; scales down with development.
const ROAD_MAX_ALPHA: u8 = 200;
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
/// Returns the revealed farms' `(id, screen centroid)` — farms with fog below
/// [`REVEAL_THRESHOLD`] at their centroid, with a panel reach that still
/// overlaps `panel_rect` (see [`PANEL_H_MAX`]) — for the info-panel pass.
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

        let map_centroid = farm.centroid();
        if fog_alpha_at(map_centroid, &farms.traveler_reveals) < REVEAL_THRESHOLD {
            let centroid = view.to_screen(map_centroid);
            // Since the panel is centered on `centroid`, it can still have a
            // visible sliver even once `centroid` itself is off-panel.
            let panel_reach = panel_rect.expand2(egui::Vec2::new(PANEL_W / 2.0, PANEL_H_MAX / 2.0));
            if panel_reach.contains(centroid) {
                revealed.push((FarmId::new(i), centroid));
            }
        }
    }
    revealed
}

/// Pass 2 of the map render: roads (under fog) — dirt fades in brown as it
/// develops; a paved fraction (from the city-side end) is solid gray instead.
fn draw_roads(
    painter: &egui::Painter,
    view: &MapView,
    roads: &crate::surroundings::RoadNetwork,
    expanded: egui::Rect,
) {
    use egui::{Color32, Stroke};

    for edge in &roads.edges {
        let dev = edge.development();
        let paved = edge.paved.clamp(0.0, 1.0);
        if dev <= 0.0 && paved <= 0.0 {
            continue;
        }
        let (near, far) = if roads.dist_to_city(edge.a) <= roads.dist_to_city(edge.b) {
            (edge.a, edge.b)
        } else {
            (edge.b, edge.a)
        };
        let near_map = roads.nodes[near];
        let far_map = roads.nodes[far];
        let near_pt = view.to_screen(near_map);
        let far_pt = view.to_screen(far_map);
        if !expanded.contains(near_pt) && !expanded.contains(far_pt) {
            continue;
        }
        let split = near_map.lerp(far_map, paved);
        let split_pt = view.to_screen(split);

        if paved > 0.0 {
            let gray = Color32::from_rgba_unmultiplied(150, 150, 150, ROAD_MAX_ALPHA);
            painter.line_segment([near_pt, split_pt], Stroke::new(2.0, gray));
        }
        if paved < 1.0 && dev > 0.0 {
            let alpha = (dev * ROAD_MAX_ALPHA as f32) as u8;
            let brown = Color32::from_rgba_unmultiplied(120, 80, 40, alpha);
            let width = 1.0 + dev * 1.5;
            painter.line_segment([split_pt, far_pt], Stroke::new(width, brown));
        }
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
    commands.insert_resource(SurroundingsState {
        viewport_offset: Vec2::ZERO,
        open_farm_menu: None,
    });
}

pub fn exit_surroundings_mode(mut commands: Commands) {
    commands.remove_resource::<SurroundingsState>();
}

#[allow(clippy::too_many_arguments)]
pub fn surroundings_ui_system(
    mut contexts: EguiContexts,
    mut farms: Guarded<FarmsResource>,
    mut state: ResMut<SurroundingsState>,
    constructed: Res<crate::city::ConstructedCity>,
    pending: Res<crate::city::ProposedCity>,
    population: Res<crate::population::Population>,
    traveler_state: Res<crate::traveler::TravelerState>,
    material_list: Res<crate::materials::MaterialList>,
    sandbox: Res<crate::game_mode::SandboxMode>,
    cache: Res<crate::ui::MonthEffectsCache>,
    resource_icons: bevy::prelude::Res<crate::resource_icons::ResourceIcons>,
    mut next_game_mode: ResMut<NextState<crate::game_mode::GameMode>>,
) {
    use crate::game_mode::GameMode;
    use egui::Sense;

    let icon_textures_sm = resource_icons.texture_ids_small(&mut contexts);
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Everything the full month pipeline needs, so the farm-config popup's
    // previews reflect feeding and a traveler's claim ahead of the market (see
    // `MonthInputs`). The popup runs hypotheticals (a farm temporarily mutated),
    // so it can't use the shared cache below.
    let inputs = MonthInputs {
        constructed: &constructed,
        pending: &pending,
        population: &population,
        traveler_state: &traveler_state,
        material_list: &material_list,
        sandbox_enabled: sandbox.enabled,
    };

    // Market preview for farm boost display: read from the shared cache that
    // `crate::ui::update_economy_cache` fills once per frame (before this
    // system), rather than recomputing the whole month pipeline here.
    let preview_market = cache
        .0
        .as_ref()
        .map(|e| e.market_effects())
        .unwrap_or_default();
    // Predicted boost for a farm, if it is invited in `Market` mode.
    let predicted_boost = |id: FarmId| -> i32 {
        match preview_market.get(&id).copied() {
            Some(CityEffect::Market {
                effect: MarketModeEffect::Boost,
                ..
            }) => MARKET_BOOST,
            _ => 0,
        }
    };

    let mut pan_delta: Option<egui::Vec2> = None;
    let mut go_build = false;

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
                draw_roads(&painter, &view, roads, expanded);
            }

            let fog_mesh = build_fog_mesh(panel_rect, &view, &farms.traveler_reveals);
            painter.add(egui::Shape::mesh(fog_mesh));

            let cs = draw_nav_circle(&painter, &view, farms.circle_pos);

            let response = ui.allocate_rect(panel_rect, Sense::click_and_drag());
            if response.dragged() {
                pan_delta = Some(response.drag_delta());
            }
            if response.clicked() {
                if let Some(ptr) = response.interact_pointer_pos() {
                    let dx = ptr.x - cs.x;
                    let dy = ptr.y - cs.y;
                    if dx * dx + dy * dy <= CIRCLE_RADIUS * CIRCLE_RADIUS {
                        go_build = true;
                    }
                }
            }
        });

    // ── Farm info panels ──────────────────────────────────────────────────────

    // Maximum invitees = number of market stand furniture placed across all
    // market places.
    let invite_limit_reached =
        farms.invited_count() >= crate::place::market_stand_count(&constructed);

    for (id, centroid) in revealed {
        let current_event = farms.farm_event(id);
        let panel_view = farm_panel_view(
            &farms[id],
            current_event,
            predicted_boost(id),
            invite_limit_reached,
        );
        // The checkbox binds `&mut bool` every frame this panel is shown,
        // regardless of whether the player clicks it -- render against a
        // scratch copy and only touch the real resource if it actually
        // changed (see `edit_if_changed`).
        let current_invited = farms[id].invited;
        let mut opened = None;
        if let Some(new_invited) = edit_if_changed(&current_invited, |invited| {
            opened = farm_info_panel(
                ctx,
                panel_rect,
                id,
                centroid,
                &panel_view,
                invited,
                &icon_textures_sm,
            );
        }) {
            farms.mutate()[id].invited = new_invited;
        }
        if let Some(opened) = opened {
            state.open_farm_menu = Some(opened);
        }
    }

    // ── Farm configuration ("…") popup ────────────────────────────────────────
    if let Some(menu_i) = state.open_farm_menu {
        if menu_i.index() >= farms.farms.len() || !farms[menu_i].invited {
            state.open_farm_menu = None;
        } else {
            farm_menu_ui(ctx, &mut farms, inputs, menu_i, &mut state);
        }
    }

    // ── Apply deferred actions ────────────────────────────────────────────────
    if let Some(delta) = pan_delta {
        state.viewport_offset.x -= delta.x / PIXELS_PER_UNIT;
        state.viewport_offset.y += delta.y / PIXELS_PER_UNIT;
    }
    if go_build {
        next_game_mode.set(GameMode::Build);
    }
}

/// Renders one revealed farm's info panel at `centroid`. `invited` binds the
/// checkbox directly to the farm's live state, since the view-model is
/// read-only. Returns `Some(id)` if the "…" button was clicked (requesting
/// the farm menu popup open for it).
fn farm_info_panel(
    ctx: &egui::Context,
    panel_rect: egui::Rect,
    id: FarmId,
    centroid: egui::Pos2,
    view: &FarmPanelView,
    invited: &mut bool,
    icon_textures_sm: &HashMap<UniformResource, egui::TextureId>,
) -> Option<FarmId> {
    let mut open_menu = None;
    egui::Area::new(egui::Id::new(("farm_panel", id)))
        .fixed_pos(centroid)
        .pivot(egui::Align2::CENTER_CENTER)
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_clip_rect(panel_rect);
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(15, 15, 15, 210))
                .inner_margin(egui::Margin::same(4))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.set_max_width(PANEL_W);

                    // Production information: acres + boost (+ predicted_bonus)
                    ui.horizontal(|ui| {
                        label!(
                            ui,
                            format!("{:.0} ac", view.area),
                            (view.boost != 0).then_some(format!("{:+}", view.boost)),
                            (*invited && view.predicted_boost > 0).then_some(col_format!(
                                preview,
                                "+ {}",
                                view.predicted_boost
                            ))
                        );
                    });

                    // Stockpiles on same line
                    ui.horizontal(|ui| {
                        if let Some(&tex) = icon_textures_sm.get(&UniformResource::Potato) {
                            icon(ui, tex, SMALL_SIZE);
                        }
                        label!(ui, format!("{}", view.potato_stockpile));

                        ui.add_space(4.0);

                        // Inedible resource stockpile (tool output while specialized)
                        if let Some(&tex) = icon_textures_sm.get(&view.produced_resource) {
                            icon(ui, tex, SMALL_SIZE);
                        }
                        label!(ui, format!("{}", view.inedible_stockpile));
                    });

                    ui.horizontal(|ui| {
                        ui.add_enabled(
                            view.can_invite,
                            egui::Checkbox::new(invited, view.checkbox_label),
                        );
                        if ui.add_enabled(*invited, egui::Button::new("…")).clicked() {
                            open_menu = Some(id);
                        }
                    });
                });
        });
    open_menu
}

/// The "Farm options" popup for `menu_i` (already known to be invited),
/// rendering the choices computed by [`farm_menu_view`].
fn farm_menu_ui(
    ctx: &egui::Context,
    farms: &mut Guarded<'_, FarmsResource>,
    inputs: MonthInputs,
    menu_i: FarmId,
    state: &mut SurroundingsState,
) {
    // `farm_menu_view` mutates `farms` transiently (hypothetical previews,
    // restored before it returns) -- never a real net change, so this must
    // never mark the resource changed even though it needs `&mut` access.
    let FarmMenuView { options } = farm_menu_view(farms.bypass(), inputs, menu_i);

    let mut keep_open = true;
    let mut chosen_event: Option<FarmEvent> = None;
    egui::Window::new("Farm options")
        .id(egui::Id::new(("farm_menu", menu_i)))
        .collapsible(false)
        .resizable(false)
        .open(&mut keep_open)
        .show(ctx, |ui| {
            for option in &options {
                if ui
                    .add_enabled(
                        option.enabled,
                        egui::Button::selectable(option.selected, option.title),
                    )
                    .clicked()
                {
                    chosen_event = Some(option.event);
                }
                if !option.enabled {
                    if let Some(note) = option.disabled_note {
                        note_label!(ui, note);
                    }
                }
                for line in &option.lines {
                    label!(ui, format!("    • {}", line));
                }
                ui.add_space(4.0);
            }
        });

    if !keep_open {
        state.open_farm_menu = None;
    }
    if let Some(new_event) = chosen_event {
        farms.mutate().set_farm_event(menu_i, new_event);
    }
}
