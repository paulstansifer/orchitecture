use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use super::farmstead::{run_market, FarmsResource, GameClock, SurroundingsState};

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
    wall_grid: Res<crate::wall_grid::WallGrid>,
) {
    use crate::game_mode::GameMode;
    use egui::{Color32, FontId, Pos2, Sense, Shape, Stroke};

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Compute circle_reveal_radius early; used both for fog and as the market radius.
    let screen_rect = ctx.screen_rect();
    let short_side = screen_rect.width().min(screen_rect.height());
    let circle_reveal_radius = short_side * 0.45 / PIXELS_PER_UNIT;

    let mut pan_delta: Option<egui::Vec2> = None;
    let mut go_build = false;

    crate::build_ui::resource_sidebar(ctx, &wall_grid);

    // ── Clock panel ───────────────────────────────────────────────────────────
    egui::Area::new(egui::Id::new("clock_panel"))
        .fixed_pos(egui::Pos2::new(8.0, 8.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(20, 20, 20, 200))
                .inner_margin(egui::Margin::same(8))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    let month = clock.month() + 1;
                    let week = clock.week_of_month() + 1;
                    ui.label(
                        egui::RichText::new(format!("Month {month}, Week {week}"))
                            .color(Color32::from_gray(220))
                            .font(FontId::proportional(12.0)),
                    );
                    ui.add_space(4.0);
                    if ui.button("Advance Week").clicked() {
                        let new_month = clock.advance_week();
                        if new_month {
                            for farm in &mut farms.farms {
                                farm.accumulate_monthly();
                            }
                            run_market(&mut *farms, circle_reveal_radius);
                        }
                    }
                });
        });

    // ── Player market goods panel ─────────────────────────────────────────────
    let goods = farms.player_goods.uniform_totals();
    if !goods.is_empty() {
        egui::Area::new(egui::Id::new("player_goods_panel"))
            .fixed_pos(egui::Pos2::new(8.0, 90.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(20, 20, 20, 200))
                    .inner_margin(egui::Margin::same(8))
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        ui.set_max_width(120.0);
                        ui.label(
                            egui::RichText::new("Market Goods")
                                .color(Color32::from_gray(200))
                                .font(FontId::proportional(11.0)),
                        );
                        for (res, qty) in &goods {
                            ui.label(
                                egui::RichText::new(format!("{}: {}", res.label(), qty))
                                    .font(FontId::proportional(10.0))
                                    .color(Color32::from_rgb(200, 180, 100)),
                            );
                        }
                    });
            });
    }

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
                            egui::RichText::new(format!(
                                "Potatoes: {}",
                                farm.potato_stockpile
                            ))
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

                        // Boost (only shown when non-zero)
                        if farm.boost > 0 {
                            ui.label(
                                egui::RichText::new(format!("+{} boost", farm.boost))
                                    .font(FontId::proportional(9.0))
                                    .color(Color32::from_rgb(220, 160, 80)),
                            );
                        }

                        ui.checkbox(&mut farm.invited, "Invite");
                    });
            });
    }

    if let Some(delta) = pan_delta {
        state.viewport_offset.x -= delta.x / PIXELS_PER_UNIT;
        state.viewport_offset.y += delta.y / PIXELS_PER_UNIT;
    }
    if go_build {
        next_game_mode.set(GameMode::Build);
    }
}
