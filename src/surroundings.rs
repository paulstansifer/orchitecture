use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use serde::{Deserialize, Serialize};

const PIXELS_PER_UNIT: f32 = 8.0;
const NUM_SEEDS: usize = 200;
const MAP_EXTENT: f32 = 200.0;
const CLIP_BOUNDS: f32 = 300.0;
const CIRCLE_RADIUS: f32 = 14.0;
// 200 seeds over a 400×400 map gives ~800 sq-units per cell on average;
// this scale puts that average at ~6 acres (midpoint of the 2–12 range).
const ACRES_PER_UNIT_SQ: f32 = 0.0075;

// Fog-of-war parameters (all distances in map units unless noted).
const FOG_GRID_STEP_PX: f32 = 20.0; // screen-space pixel spacing between fog mesh vertices
const PATH_REVEAL_RADIUS: f32 = 8.0; // half-width of revealed corridor along the path
const FOG_FADE_WIDTH: f32 = 10.0; // distance over which fog fades in at the edge of a revealed area
const FOG_MAX_ALPHA: u8 = 215; // opacity of fully-fogged regions

#[derive(Serialize, Deserialize)]
pub struct FarmData {
    pub seed: Vec2,
    pub polygon: Vec<Vec2>,
    pub area: f32,
    pub fertility: f32,
    pub farmers: u32,
}

/// Permanent game-state resource: generated once, persists across mode changes, saved/loaded.
#[derive(Resource, Serialize, Deserialize)]
pub struct FarmsResource {
    pub farms: Vec<FarmData>,
    pub circle_pos: Vec2,
    /// Path from a distant point toward the map origin, in map coordinates.
    pub path: Vec<Vec2>,
}

/// Transient resource: exists only while in Surroundings mode.
#[derive(Resource)]
pub struct SurroundingsState {
    pub viewport_offset: Vec2,
}

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
    let t = (fertility - 0.75) / 0.5; // 0.0 to 1.0
    let hue = 80.0 + t * 40.0; // 80° (yellow-green) to 120° (deep green)
    let sat = 0.50 + t * 0.30; // 50% to 80%
    let val = 0.70 - t * 0.15; // 70% to 55%
    hsv_to_color32(hue, sat, val)
}

// Sutherland-Hodgman: keep vertices where dot(v - point, normal) >= 0
fn clip_polygon_by_halfplane(poly: &[Vec2], point: Vec2, normal: Vec2) -> Vec<Vec2> {
    if poly.is_empty() {
        return vec![];
    }
    let mut result = Vec::new();
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let da = (a - point).dot(normal);
        let db = (b - point).dot(normal);
        if da >= 0.0 {
            result.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            result.push(a + t * (b - a));
        }
    }
    result
}

fn polygon_area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    let mut sum = 0.0_f32;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        sum += a.x * b.y - b.x * a.y;
    }
    sum.abs() * 0.5
}

fn voronoi_cell(seed: Vec2, all_seeds: &[Vec2]) -> Vec<Vec2> {
    let b = CLIP_BOUNDS;
    let mut poly = vec![
        Vec2::new(-b, -b),
        Vec2::new(b, -b),
        Vec2::new(b, b),
        Vec2::new(-b, b),
    ];
    for &other in all_seeds {
        if other == seed {
            continue;
        }
        let midpoint = (seed + other) * 0.5;
        let normal = seed - other; // points toward seed's side
        poly = clip_polygon_by_halfplane(&poly, midpoint, normal);
        if poly.is_empty() {
            break;
        }
    }
    poly
}

fn generate_path(rng: &mut impl rand::Rng) -> Vec<Vec2> {
    use std::f32::consts::TAU;
    let start_dist = rng.random_range(110.0..150.0_f32);
    let start_angle: f32 = rng.random_range(0.0..TAU);
    let start = Vec2::new(
        start_angle.cos() * start_dist,
        start_angle.sin() * start_dist,
    );
    let main_dir = -start.normalize(); // points toward origin
    let perp = Vec2::new(-main_dir.y, main_dir.x);

    let mut points = vec![start];
    let num_middle = 4;
    for i in 1..=num_middle {
        let t = i as f32 / (num_middle + 1) as f32;
        let base = start + main_dir * (start_dist * t);
        // Perpendicular deviation shrinks as we approach the centre
        let deviation = rng.random_range(-12.0..12.0_f32) * (1.0 - t * 0.7);
        points.push(base + perp * deviation);
    }
    points.push(Vec2::ZERO);
    points
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
    // Distance outside the central revealed circle
    let dist_circle = (map_pos.length() - circle_reveal_radius).max(0.0);

    // Distance outside the path corridor
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
    // Smoothstep for a soft edge
    let t = t * t * (3.0 - 2.0 * t);
    (t * 255 as f32) as u8
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
                egui::Color32::from_rgba_unmultiplied(80, 82, 90, alpha),
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

pub fn generate_farms(mut commands: Commands) {
    use rand::Rng;
    let mut rng = rand::rng();

    let seeds: Vec<Vec2> = (0..NUM_SEEDS)
        .map(|_| {
            Vec2::new(
                rng.random_range(-MAP_EXTENT..MAP_EXTENT),
                rng.random_range(-MAP_EXTENT..MAP_EXTENT),
            )
        })
        .collect();

    let mut farms = Vec::new();
    for &seed in &seeds {
        let polygon = voronoi_cell(seed, &seeds);
        if polygon.is_empty() {
            continue;
        }
        let area: f32 = (polygon_area(&polygon) * ACRES_PER_UNIT_SQ).clamp(2.0, 12.0);
        let fertility: f32 = rng.random_range(0.75..1.25_f32);
        let farmers = ((area * fertility / 1.8).floor() as u32).max(1);
        farms.push(FarmData {
            seed,
            polygon,
            area,
            fertility,
            farmers,
        });
    }

    let mut circle_pos = Vec2::ZERO;
    let mut best_dist = f32::MAX;
    for farm in &farms {
        for &v in &farm.polygon {
            let d = v.length_squared();
            if d < best_dist {
                best_dist = d;
                circle_pos = v;
            }
        }
    }

    let path = generate_path(&mut rng);

    commands.insert_resource(FarmsResource {
        farms,
        circle_pos,
        path,
    });
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
    farms: Res<FarmsResource>,
    mut state: ResMut<SurroundingsState>,
    mut next_game_mode: ResMut<NextState<crate::game_mode::GameMode>>,
    wall_grid: Res<crate::wall_grid::WallGrid>,
) {
    use crate::game_mode::GameMode;
    use egui::{Color32, FontId, Pos2, Rect, Sense, Shape, Stroke};

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let mut pan_delta: Option<egui::Vec2> = None;
    let mut go_build = false;

    crate::build_ui::resource_sidebar(ctx, &wall_grid);

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let painter = ui.painter().clone();
            let panel_rect = ui.max_rect();
            let screen_centre = panel_rect.center();

            // Dark background so 3D scene behind is hidden
            painter.rect_filled(panel_rect, 0.0, Color32::from_rgb(30, 30, 30));

            let map_to_screen = |p: Vec2| -> Pos2 {
                let rel = (p - state.viewport_offset) * PIXELS_PER_UNIT;
                Pos2::new(screen_centre.x + rel.x, screen_centre.y - rel.y)
            };

            let expanded = panel_rect.expand(300.0);

            for farm in farms.farms.iter() {
                let screen_pts: Vec<Pos2> =
                    farm.polygon.iter().map(|&p| map_to_screen(p)).collect();

                if !screen_pts.iter().any(|p| expanded.contains(*p)) {
                    continue;
                }

                let fill = farm_color(farm.fertility);
                let stroke = Stroke::new(1.0, Color32::from_gray(40));
                painter.add(Shape::convex_polygon(screen_pts.clone(), fill, stroke));

                let centroid = {
                    let (sx, sy) = screen_pts
                        .iter()
                        .fold((0.0_f32, 0.0_f32), |acc, p| (acc.0 + p.x, acc.1 + p.y));
                    Pos2::new(sx / screen_pts.len() as f32, sy / screen_pts.len() as f32)
                };

                if !panel_rect.contains(centroid) {
                    continue;
                }

                painter.text(
                    centroid,
                    egui::Align2::CENTER_CENTER,
                    format!(
                        "{:.0}ac  {}f  ×{:.2}",
                        farm.area, farm.farmers, farm.fertility
                    ),
                    FontId::proportional(10.0),
                    Color32::from_gray(20),
                );

                // Subsistence bar: centre = subsistence level; green right = surplus, red left = deficit
                let bar_w = 70.0_f32;
                let bar_h = 6.0_f32;
                let bar_tl = Pos2::new(centroid.x - bar_w / 2.0, centroid.y + 14.0);
                painter.rect_filled(
                    Rect::from_min_size(bar_tl, egui::vec2(bar_w, bar_h)),
                    0.0,
                    Color32::from_gray(200),
                );

                let surplus = farm.area * farm.fertility / (2.0 * farm.farmers as f32) - 1.0;
                let clamped = surplus.clamp(-0.3, 0.3);
                let cx = bar_tl.x + bar_w / 2.0;
                if clamped >= 0.0 {
                    let fw = (clamped / 0.3) * (bar_w / 2.0);
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(cx, bar_tl.y), egui::vec2(fw, bar_h)),
                        0.0,
                        Color32::from_rgb(60, 160, 60),
                    );
                } else {
                    let fw = (-clamped / 0.3) * (bar_w / 2.0);
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(cx - fw, bar_tl.y), egui::vec2(fw, bar_h)),
                        0.0,
                        Color32::from_rgb(180, 60, 60),
                    );
                }
                painter.line_segment(
                    [Pos2::new(cx, bar_tl.y), Pos2::new(cx, bar_tl.y + bar_h)],
                    Stroke::new(1.0, Color32::from_gray(80)),
                );
            }

            // Fog-of-war mesh (drawn over farms, under the navigation circle)
            {
                let short_side = panel_rect.width().min(panel_rect.height());
                let circle_reveal_radius = short_side * 0.45 / PIXELS_PER_UNIT;
                let fog_mesh = build_fog_mesh(
                    panel_rect,
                    screen_centre,
                    state.viewport_offset,
                    circle_reveal_radius,
                    &farms.path,
                );
                painter.add(egui::Shape::mesh(fog_mesh));
            }

            // Navigation circle (above the fog)
            let cs = map_to_screen(farms.circle_pos);
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

    if let Some(delta) = pan_delta {
        state.viewport_offset.x -= delta.x / PIXELS_PER_UNIT;
        state.viewport_offset.y += delta.y / PIXELS_PER_UNIT;
    }
    if go_build {
        next_game_mode.set(GameMode::Build);
    }
}
