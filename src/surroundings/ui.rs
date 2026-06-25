use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use super::farmstead::{
    preview_market, run_market, update_wanted_resources, FarmsResource, GameClock,
    SurroundingsState,
};

const PIXELS_PER_UNIT: f32 = 8.0;
const CIRCLE_RADIUS: f32 = 18.0;

// Fog-of-war parameters (all distances in map units unless noted).
const FOG_GRID_STEP_PX: f32 = 20.0;
const PATH_REVEAL_RADIUS: f32 = 12.0;
const FOG_FADE_WIDTH: f32 = 10.0;
const FOG_MAX_ALPHA: u8 = 215;

// Farm UI is shown only when the centroid's fog alpha is below this value.
const REVEAL_THRESHOLD: u8 = 64;

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

fn segment_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.dot(ab);
    if len_sq < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Returns the fog alpha (0 = fully clear, FOG_MAX_ALPHA = fully fogged) for a map-space point.
fn fog_alpha_at(map_pos: Vec2, circle_reveal_radius: f32, path: &[Vec2]) -> u8 {
    let dist_circle = (map_pos.length() - circle_reveal_radius).max(0.0);

    let dist_path = if path.len() >= 2 {
        let raw = path
            .windows(2)
            .map(|seg| segment_dist(map_pos, seg[0], seg[1]))
            .fold(f32::MAX, f32::min);
        (raw - PATH_REVEAL_RADIUS).max(0.0)
    } else {
        f32::MAX
    };

    let dist = dist_circle.min(dist_path);
    let t = (dist / FOG_FADE_WIDTH).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t);
    (t * FOG_MAX_ALPHA as f32) as u8
}

fn build_fog_mesh(
    panel_rect: egui::Rect,
    screen_centre: egui::Pos2,
    viewport_offset: Vec2,
    circle_reveal_radius: f32,
    path: &[Vec2],
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
            let alpha = fog_alpha_at(map_pos, circle_reveal_radius, path);
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
    });
}

pub fn exit_surroundings_mode(mut commands: Commands) {
    commands.remove_resource::<SurroundingsState>();
}

pub fn surroundings_ui_system(
    mut contexts: EguiContexts,
    mut farms: ResMut<FarmsResource>,
    mut state: ResMut<SurroundingsState>,
    mut clock: ResMut<GameClock>,
    mut next_game_mode: ResMut<NextState<crate::game_mode::GameMode>>,
    mut wall_grid: ResMut<crate::wall_grid::WallGrid>,
) {
    use crate::game_mode::GameMode;
    use egui::{Color32, FontId, Pos2, Sense, Shape, Stroke};

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Market preview: what would happen if we ran the market right now.
    let preview = preview_market(&*farms);

    // Gains map for quick lookup in the resource panel.
    let gains_map: std::collections::HashMap<crate::resource::UniformResource, u32> =
        preview.player_gains.iter().copied().collect();

    // Current station resource totals (sorted canonically).
    let station_totals = crate::build_ui::station_resource_totals(&*wall_grid);

    // All resources that appear in current storage OR in preview gains, sorted canonically.
    let mut rhs_resources: Vec<crate::resource::UniformResource> =
        station_totals.iter().map(|(r, _, _)| *r).collect();
    for (r, _) in &preview.player_gains {
        if !rhs_resources.contains(r) {
            rhs_resources.push(*r);
        }
    }
    rhs_resources.sort();

    let mut pan_delta: Option<egui::Vec2> = None;
    let mut go_advance_month = false;
    let mut go_walk = false;
    let mut go_build = false;

    // ── Right-hand sidebar ────────────────────────────────────────────────────
    egui::SidePanel::right("resources")
        .min_width(130.0)
        .show(ctx, |ui| {
            let month = clock.month() + 1;
            ui.label(
                egui::RichText::new(format!("Month {}", month))
                    .color(Color32::from_gray(220))
                    .font(FontId::proportional(13.0)),
            );
            ui.add_space(2.0);
            if ui.button("Advance Month").clicked() {
                go_advance_month = true;
            }
            ui.separator();
            ui.heading("Resources");
            if rhs_resources.is_empty() {
                ui.label("(none)");
            }
            for res in &rhs_resources {
                let (current, rounded_down) = station_totals
                    .iter()
                    .find(|(r, _, _)| r == res)
                    .map(|(_, q, d)| (*q, *d))
                    .unwrap_or((0, false));
                let gain = *gains_map.get(res).unwrap_or(&0);
                let prefix = if rounded_down { "> " } else { "" };
                let text = if gain > 0 {
                    format!("{}{}: {} + {}", prefix, res.label(), current, gain)
                } else {
                    format!("{}{}: {}", prefix, res.label(), current)
                };
                ui.label(egui::RichText::new(text).color(if gain > 0 {
                    Color32::from_rgb(160, 220, 140)
                } else {
                    Color32::from_gray(200)
                }));
            }

            // Mode buttons pushed to the bottom of the panel.
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                if ui.button("Build").clicked() {
                    go_build = true;
                }
                if ui.button("Walk Around").clicked() {
                    go_walk = true;
                }
            });
        });

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

            let short_side = panel_rect.width().min(panel_rect.height());
            let circle_reveal_radius = short_side * 0.45 / PIXELS_PER_UNIT;
            let path = farms.path.clone();
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
                if fog_alpha_at(map_centroid, circle_reveal_radius, &path) < REVEAL_THRESHOLD {
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
                circle_reveal_radius,
                &path,
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
                FontId::proportional(11.0),
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
    let market_stand_count = wall_grid
        .stations
        .iter()
        .position(|s| s.name == "market stand")
        .map_or(0, |idx| {
            wall_grid
                .placed_stations
                .iter()
                .filter(|ps| ps.station == idx)
                .count()
        });
    let invited_count = farms.farms.iter().filter(|f| f.invited).count();
    let invite_limit_reached = invited_count >= market_stand_count;

    for (i, centroid) in revealed {
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

                        ui.label(
                            egui::RichText::new(format!("{:.0} ac", farm.area))
                                .font(FontId::proportional(10.0))
                                .color(Color32::from_gray(210)),
                        );

                        // Potato stockpile
                        ui.label(
                            egui::RichText::new(format!("Potatoes: {}", farm.potato_stockpile))
                                .font(FontId::proportional(9.0))
                                .color(Color32::from_rgb(220, 210, 120)),
                        );

                        // Inedible resource stockpile
                        ui.label(
                            egui::RichText::new(format!(
                                "{}: {}",
                                farm.resource.label(),
                                farm.inedible_stockpile
                            ))
                            .font(FontId::proportional(9.0))
                            .color(Color32::from_rgb(160, 200, 140)),
                        );

                        // Wanted resource
                        ui.label(
                            egui::RichText::new(format!(
                                "Wants {} {}",
                                farm.want_max,
                                farm.wanted_resource.label()
                            ))
                            .font(FontId::proportional(9.0))
                            .color(Color32::from_rgb(140, 170, 220)),
                        );

                        // Current boost (non-zero means extra production this month)
                        if farm.boost > 0 {
                            ui.label(
                                egui::RichText::new(format!("+{} boost", farm.boost))
                                    .font(FontId::proportional(9.0))
                                    .color(Color32::from_rgb(220, 160, 80)),
                            );
                        }

                        // Market preview: predicted boost from next market run
                        if farm.invited {
                            if let Some(&t) = preview.farm_boosts.get(&i) {
                                if t > 0 {
                                    ui.label(
                                        egui::RichText::new(format!("+{}", t))
                                            .font(FontId::proportional(9.0))
                                            .color(Color32::from_rgb(80, 220, 80)),
                                    );
                                }
                            }
                        }

                        let can_invite = farm.invited || !invite_limit_reached;
                        ui.add_enabled(
                            can_invite,
                            egui::Checkbox::new(&mut farm.invited, "Invite"),
                        );
                    });
            });
    }

    // ── Apply deferred actions ────────────────────────────────────────────────
    if let Some(delta) = pan_delta {
        state.viewport_offset.x -= delta.x / PIXELS_PER_UNIT;
        state.viewport_offset.y += delta.y / PIXELS_PER_UNIT;
    }
    if go_advance_month {
        clock.advance_month();
        for farm in &mut farms.farms {
            farm.accumulate_monthly();
        }
        let gains = run_market(&mut *farms);
        update_wanted_resources(&mut *farms);
        // Uncheck all invites for the next month.
        for farm in &mut farms.farms {
            farm.invited = false;
        }
        // Deposit gains into the first available storage station; silently drop if none.
        if !gains.is_empty() {
            let storage_idx = wall_grid.placed_stations.iter().position(|ps| {
                wall_grid
                    .stations
                    .get(ps.station)
                    .map_or(false, |info| info.storage.is_some())
            });
            if let Some(idx) = storage_idx {
                for (res, qty) in gains {
                    wall_grid.placed_stations[idx]
                        .contents
                        .add_uniform(res, qty as u16);
                }
            }
        }
    }
    if go_walk {
        next_game_mode.set(GameMode::Walk);
    }
    if go_build {
        next_game_mode.set(GameMode::Build);
    }
}
