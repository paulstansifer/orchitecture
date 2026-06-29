use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::resource_icons::SMALL_SIZE;
use crate::ui_util::FontSizes;
use crate::{col_format, label};

use super::farmstead::{
    farm_breakdown, market_effect, preview_market, FarmEvent, FarmProduction, FarmsResource,
    MarketModeEffect, SurroundingsState,
};
use super::map::{fog_alpha_at, REVEAL_THRESHOLD};
use crate::resource::ToolKind;

const PIXELS_PER_UNIT: f32 = 8.0;
const CIRCLE_RADIUS: f32 = 18.0;
const FOG_GRID_STEP_PX: f32 = 20.0;

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

fn build_fog_mesh(
    panel_rect: egui::Rect,
    screen_centre: egui::Pos2,
    viewport_offset: Vec2,
    paths: &[Vec<Vec2>],
) -> egui::Mesh {
    let cols = (panel_rect.width() / FOG_GRID_STEP_PX).ceil() as usize + 1;
    let rows = (panel_rect.height() / FOG_GRID_STEP_PX).ceil() as usize + 1;
    let mut mesh = egui::Mesh::default();
    mesh.reserve_vertices(cols * rows);
    mesh.reserve_triangles((cols - 1) * (rows - 1) * 2);

    for row in 0..rows {
        for col in 0..cols {
            let sx = (panel_rect.min.x + col as f32 * FOG_GRID_STEP_PX).min(panel_rect.max.x);
            let sy = (panel_rect.min.y + row as f32 * FOG_GRID_STEP_PX).min(panel_rect.max.y);
            let map_pos = Vec2::new(
                (sx - screen_centre.x) / PIXELS_PER_UNIT + viewport_offset.x,
                -(sy - screen_centre.y) / PIXELS_PER_UNIT + viewport_offset.y,
            );
            let alpha = fog_alpha_at(map_pos, paths);
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

pub fn enter_surroundings_mode(mut commands: Commands) {
    commands.insert_resource(SurroundingsState {
        viewport_offset: Vec2::ZERO,
        open_farm_menu: None,
    });
}

pub fn exit_surroundings_mode(mut commands: Commands) {
    commands.remove_resource::<SurroundingsState>();
}

pub fn surroundings_ui_system(
    mut contexts: EguiContexts,
    mut farms: ResMut<FarmsResource>,
    mut state: ResMut<SurroundingsState>,
    constructed: Res<crate::world::ConstructedWorld>,
    resource_icons: bevy::prelude::Res<crate::resource_icons::ResourceIcons>,
    mut next_game_mode: ResMut<NextState<crate::game_mode::GameMode>>,
) {
    use crate::game_mode::GameMode;
    use egui::{Color32, Pos2, Sense, Shape, Stroke};

    let icon_textures_sm = resource_icons.texture_ids_small(&mut contexts);
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Market preview for farm boost display.
    farms.ensure_adjacency();
    let preview = preview_market(&*farms);
    // Predicted boost for a farm, if it is invited in `Market` mode.
    let predicted_boost = |i: usize| -> u32 {
        match preview.farm_effects.get(&i).map(|e| e.effect) {
            Some(MarketModeEffect::Boost(t)) => t,
            _ => 0,
        }
    };

    let mut pan_delta: Option<egui::Vec2> = None;
    let mut go_build = false;

    // Collect revealed farm indices and their screen centroids.
    let mut revealed: Vec<(usize, egui::Pos2)> = Vec::new();

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let painter = ui.painter().clone();
            let panel_rect = ui.max_rect();
            let screen_centre = panel_rect.center();

            painter.rect_filled(panel_rect, 0.0, Color32::from_rgb(30, 30, 30));

            let viewport_offset = state.viewport_offset;
            let map_to_screen = |p: Vec2| -> Pos2 {
                let rel = (p - viewport_offset) * PIXELS_PER_UNIT;
                Pos2::new(screen_centre.x + rel.x, screen_centre.y - rel.y)
            };

            let circle_pos = farms.circle_pos;
            let expanded = panel_rect.expand(300.0);

            // ── Pass 1: polygon fills (under fog) ─────────────────────────────
            for (i, farm) in farms.farms.iter().enumerate() {
                let screen_pts: Vec<Pos2> =
                    farm.polygon.iter().map(|&p| map_to_screen(p)).collect();
                if !screen_pts.iter().any(|p| expanded.contains(*p)) {
                    continue;
                }

                let fill = farm_color(farm.fertility);
                let stroke = Stroke::new(1.0, Color32::from_gray(40));
                painter.add(Shape::convex_polygon(screen_pts, fill, stroke));

                let map_centroid = farm.centroid();
                if fog_alpha_at(map_centroid, &farms.traveler_reveals) < REVEAL_THRESHOLD {
                    let centroid = map_to_screen(map_centroid);
                    if panel_rect.contains(centroid) {
                        revealed.push((i, centroid));
                    }
                }
            }

            // ── Fog-of-war mesh ───────────────────────────────────────────────
            let fog_mesh = build_fog_mesh(
                panel_rect,
                screen_centre,
                viewport_offset,
                &farms.traveler_reveals,
            );
            painter.add(egui::Shape::mesh(fog_mesh));

            // ── Navigation circle (above fog) ─────────────────────────────────
            let cs = map_to_screen(circle_pos);
            painter.circle_filled(cs, CIRCLE_RADIUS, Color32::WHITE);
            painter.circle_stroke(cs, CIRCLE_RADIUS, Stroke::new(2.0, Color32::from_gray(40)));
            painter.text(
                Pos2::new(cs.x, cs.y + CIRCLE_RADIUS + 9.0),
                egui::Align2::CENTER_CENTER,
                "Build",
                FontSizes::body(),
                Color32::from_gray(20),
            );

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
    const PANEL_W: f32 = 110.0;

    // Maximum invitees = number of placed market stand stations.
    let market_stand_count = constructed
        .stations
        .iter()
        .position(|s| s.name == "market stand")
        .map_or(0, |idx| {
            constructed
                .placed_stations
                .iter()
                .filter(|ps| ps.station == idx)
                .count()
        });
    let invited_count = farms.farms.iter().filter(|f| f.invited).count();
    let invite_limit_reached = invited_count >= market_stand_count;

    for (i, centroid) in revealed {
        let current_event = farms.farm_event(i);
        let farm = &mut farms.farms[i];

        egui::Area::new(egui::Id::new(("farm_panel", i)))
            .fixed_pos(egui::Pos2::new(centroid.x - PANEL_W / 2.0, centroid.y))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(15, 15, 15, 210))
                    .inner_margin(egui::Margin::same(4))
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        ui.set_max_width(PANEL_W);

                        // Production information: acres + boost (+ predicted_bonus)
                        ui.horizontal(|ui| {
                            label!(
                                ui,
                                format!("{:.0} ac", farm.area),
                                (farm.boost > 0).then_some(format!("+ {}", farm.boost)),
                                (farm.invited && predicted_boost(i) > 0).then_some(col_format!(
                                    preview,
                                    "+ {}",
                                    predicted_boost(i)
                                ))
                            );
                        });

                        // Stockpiles on same line
                        ui.horizontal(|ui| {
                            // Potato stockpile
                            if let Some(&tex) =
                                icon_textures_sm.get(&crate::resource::UniformResource::Potato)
                            {
                                ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                    tex, SMALL_SIZE,
                                )));
                            }
                            label!(ui, format!("{}", farm.potato_stockpile));

                            ui.add_space(4.0);

                            // Inedible resource stockpile (tool output while specialized)
                            if let Some(&tex) = icon_textures_sm.get(&farm.produced_resource()) {
                                ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                    tex, SMALL_SIZE,
                                )));
                            }
                            label!(ui, format!("{}", farm.inedible_stockpile));
                        });

                        // Wanted resource
                        ui.horizontal(|ui| {
                            label!(ui, format!("Wants {}", farm.want_max));
                            if let Some(&tex) = icon_textures_sm.get(&farm.wanted_resource) {
                                ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                    tex, SMALL_SIZE,
                                )));
                            }
                        });

                        ui.horizontal(|ui| {
                            let can_invite = farm.invited || !invite_limit_reached;
                            ui.add_enabled(
                                can_invite,
                                egui::Checkbox::new(
                                    &mut farm.invited,
                                    current_event.checkbox_label(),
                                ),
                            );
                            if ui
                                .add_enabled(farm.invited, egui::Button::new("…"))
                                .clicked()
                            {
                                state.open_farm_menu = Some(i);
                            }
                        });
                    });
            });
    }

    // ── Farm configuration ("…") popup ────────────────────────────────────────
    if let Some(menu_i) = state.open_farm_menu {
        if menu_i >= farms.farms.len() || !farms.farms[menu_i].invited {
            state.open_farm_menu = None;
        } else {
            let current_event = farms.farm_event(menu_i);
            let is_specialized = matches!(
                farms.farms[menu_i].production,
                FarmProduction::Specialized(_)
            );
            let whipsaw_in_storage =
                crate::station::total_tools_of(&constructed, ToolKind::Whipsaw) >= 1;

            // Breakdowns via the same shared compute path.
            let market_lines = farm_breakdown(&mut farms, menu_i, FarmEvent::Market, None);
            let change_lines = farm_breakdown(&mut farms, menu_i, FarmEvent::RerollResource, None);
            let spec_lines = farm_breakdown(
                &mut farms,
                menu_i,
                FarmEvent::Market,
                Some(FarmProduction::Specialized(ToolKind::Whipsaw)),
            );

            let can_change = matches!(
                market_effect(&mut farms, menu_i, FarmEvent::RerollResource),
                Some(MarketModeEffect::Reroll { paid }) if paid > 0
            );
            let can_specialize = !is_specialized && whipsaw_in_storage;

            let render_event_option = |ui: &mut egui::Ui,
                                       chosen: &mut Option<FarmEvent>,
                                       event: FarmEvent,
                                       enabled: bool,
                                       title: &str,
                                       lines: &[String]| {
                let selected = current_event == event;
                if ui
                    .add_enabled(enabled, egui::Button::selectable(selected, title))
                    .clicked()
                {
                    *chosen = Some(event);
                }
                for line in lines {
                    label!(ui, format!("    • {}", line));
                }
                ui.add_space(4.0);
            };

            let mut keep_open = true;
            let mut chosen_event: Option<FarmEvent> = None;
            egui::Window::new("Farm options")
                .id(egui::Id::new(("farm_menu", menu_i)))
                .collapsible(false)
                .resizable(false)
                .open(&mut keep_open)
                .show(ctx, |ui| {
                    render_event_option(
                        ui,
                        &mut chosen_event,
                        FarmEvent::Market,
                        true,
                        "Participate in the market",
                        &market_lines,
                    );
                    render_event_option(
                        ui,
                        &mut chosen_event,
                        FarmEvent::RerollResource,
                        can_change,
                        "Select a different secondary resource",
                        &change_lines,
                    );
                    render_event_option(
                        ui,
                        &mut chosen_event,
                        FarmEvent::Specialize(ToolKind::Whipsaw),
                        can_specialize,
                        "Process nearby timber into beams",
                        &spec_lines,
                    );
                });

            if !keep_open {
                state.open_farm_menu = None;
            }
            if let Some(new_event) = chosen_event {
                farms.set_farm_event(menu_i, new_event);
            }
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
