//! The Ideas window: a scrollable view of the idea DAG, one 50-segment bar per
//! idea, with dependency lines from each prerequisite down to its dependents.
//!
//! Layout is intentionally one-idea-per-row rather than a free-form graph:
//! rows come out of [`crate::idea_view::idea_tree_view`] topologically sorted
//! and indented by depth, so a dependent is always below and to the right of
//! everything it needs, and a connector never has to route upward. Connectors
//! run in a reserved gutter to the left of every bar, so they never cross the
//! rows they pass.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::city::ConstructedCity;
use crate::idea::IdeaState;
use crate::idea_view::{idea_tree_view, IdeaRow, SegmentState};
use crate::ui_util::{FontColors, FontSizes};
use crate::{heading_label, note_label};

/// Whether the Ideas window is showing. Toggled from the resource panel's
/// "Ideas" button (see `ui::shared_ui_system`).
#[derive(Resource, Default)]
pub struct IdeaWindowState {
    pub open: bool,
}

/// Height of a segment bar.
const BAR_HEIGHT: f32 = 14.0;
/// Horizontal indent per level of DAG depth.
const DEPTH_INDENT: f32 = 12.0;
/// Gap between adjacent segments. Segments are drawn at least 1px wide, so a
/// narrow window degrades to a solid bar rather than to nothing.
const SEGMENT_GAP: f32 = 1.0;
/// Width reserved at the left of every row for dependency connectors. Bars
/// start after it, so a connector routed inside it can never cross a bar.
const LANE_GUTTER: f32 = 20.0;
/// Horizontal spacing between connector lanes within the gutter.
const LANE_WIDTH: f32 = 5.0;

pub fn idea_ui_system(
    mut contexts: EguiContexts,
    mut window: ResMut<IdeaWindowState>,
    idea_state: Res<IdeaState>,
    constructed: Res<ConstructedCity>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let view = idea_tree_view(
        &constructed.ideas,
        &constructed.idea_deps,
        &idea_state.learned,
        &constructed.understood,
    );

    // `Window::open` needs its own bool: `window` is a `ResMut`, and handing
    // egui a `&mut` into it would mark the resource changed every frame.
    let mut open = true;
    egui::Window::new("Ideas")
        .open(&mut open)
        .default_width(440.0)
        .default_height(420.0)
        .resizable(true)
        .show(ctx, |ui| {
            legend(ui);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                let gutter_left = ui.max_rect().left();
                // Each row's bar rect, so a dependent can connect back to its
                // prerequisites; topological order guarantees every
                // prerequisite's rect is already recorded. `lanes` tracks how
                // far down the gutter each connector lane is already occupied.
                let mut bars: Vec<egui::Rect> = Vec::with_capacity(view.rows.len());
                let mut lanes: Vec<f32> = Vec::new();
                for row in &view.rows {
                    let rect = idea_row(ui, row);
                    for &prereq in &row.prereqs {
                        connector(ui, gutter_left, bars[prereq], rect, &mut lanes);
                    }
                    bars.push(rect);
                }
                ui.add_space(4.0);
            });
        });

    if !open {
        window.open = false;
    }
}

fn legend(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        for (color, text) in [
            (FontColors::understood(), "understood"),
            (FontColors::learned_only(), "in a book"),
            (FontColors::unknown(), "unread"),
        ] {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 0.0, color);
            note_label!(ui, text);
        }
    });
}

/// Draws one idea's heading, summary, and segment bar. Returns the bar's rect.
fn idea_row(ui: &mut egui::Ui, row: &IdeaRow) -> egui::Rect {
    ui.add_space(4.0);
    let mut bar_rect = egui::Rect::NOTHING;
    ui.horizontal(|ui| {
        ui.add_space(LANE_GUTTER + row.depth as f32 * DEPTH_INDENT);
        ui.vertical(|ui| {
            heading_label!(ui, row.name.clone());
            note_label!(ui, row.summary());
            bar_rect = segment_bar(ui, row);
        });
    });
    bar_rect
}

/// Routes a prerequisite-to-dependent connector as three segments: left out of
/// the prerequisite's bar into a free lane, straight down that lane, then right
/// into the dependent's bar. Lanes are assigned greedily to the first one whose
/// occupied extent has already been passed, so two connectors only share a lane
/// when their vertical spans don't overlap.
fn connector(
    ui: &egui::Ui,
    gutter_left: f32,
    from: egui::Rect,
    to: egui::Rect,
    lanes: &mut Vec<f32>,
) {
    let (top, bottom) = (from.bottom(), to.top());
    let lane = match lanes.iter().position(|&occupied_to| occupied_to <= top) {
        Some(lane) => lane,
        None => {
            lanes.push(top);
            lanes.len() - 1
        }
    };
    lanes[lane] = bottom;

    // More concurrent edges than the gutter has room for is a cosmetic problem
    // (two connectors overlap), not a correctness one -- so wrap rather than
    // letting a lane escape the gutter and cross the bars.
    let max_lanes = (LANE_GUTTER / LANE_WIDTH).floor().max(1.0) as usize;
    let x = gutter_left + (lane % max_lanes) as f32 * LANE_WIDTH + LANE_WIDTH * 0.5;

    let stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(95));
    let painter = ui.painter();
    painter.line_segment([egui::pos2(from.left(), top), egui::pos2(x, top)], stroke);
    painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], stroke);
    painter.line_segment(
        [egui::pos2(x, bottom), egui::pos2(to.left(), bottom)],
        stroke,
    );
}

/// Paints `row`'s segments across the available width and returns the rect they
/// occupy.
fn segment_bar(ui: &mut egui::Ui, row: &IdeaRow) -> egui::Rect {
    let width = ui.available_width().max(row.segments.len() as f32);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, BAR_HEIGHT), egui::Sense::hover());

    let stride = rect.width() / row.segments.len() as f32;
    let segment_width = (stride - SEGMENT_GAP).max(1.0);
    let painter = ui.painter();
    for (i, state) in row.segments.iter().enumerate() {
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + i as f32 * stride, rect.top()),
                egui::vec2(segment_width, rect.height()),
            ),
            0.0,
            match state {
                SegmentState::Understood => FontColors::understood(),
                SegmentState::LearnedOnly => FontColors::learned_only(),
                SegmentState::Unknown => FontColors::unknown(),
            },
        );
    }

    let (understood, pending) = (row.understood, row.pending);
    let percent = (row.progress() * 100.0).round() as u32;
    response.on_hover_ui(|ui| {
        ui.label(
            egui::RichText::new(format!("{understood} understood ({percent}%)"))
                .font(FontSizes::body())
                .color(FontColors::understood()),
        );
        if pending > 0 {
            ui.label(
                egui::RichText::new(format!("{pending} read but not yet understood"))
                    .font(FontSizes::body())
                    .color(FontColors::learned_only()),
            );
            ui.label(
                egui::RichText::new(
                    "(a segment needs the same segment of every idea it depends on)",
                )
                .font(FontSizes::small())
                .italics(),
            );
        }
    });

    rect
}
