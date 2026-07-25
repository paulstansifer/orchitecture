//! The Ideas window: a scrollable view of the idea DAG, one 50-segment bar per
//! idea, with dependency lines from each prerequisite down to its dependents.
//!
//! Layout is intentionally one-idea-per-row rather than a free-form graph:
//! rows come out of [`crate::idea_view::idea_tree_view`] topologically sorted
//! and indented by depth, so a dependent is always below and to the right of
//! everything it needs, and a connector never has to route upward. Connectors
//! run in a reserved gutter to the left of every bar, so they never cross the
//! rows they pass.
//!
//! The window opens itself two ways besides the panel button: clicking the book
//! a traveler is offering (which also *focuses* that idea — see
//! [`IdeaHighlight`]), and gaining knowledge, which pops the window up and
//! cross-fades the affected segments from their old colors to their new ones
//! (see [`announce_new_knowledge`]).

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::city::ConstructedCity;
use crate::idea::{compute_understood, IdeaState, SEGMENTS};
use crate::idea_view::{idea_tree_view, IdeaRow, SegmentState};
use crate::ui_util::{FontColors, FontSizes};
use crate::{heading_label, note_label};

/// How long the knowledge-gain cross-fade runs.
const REVEAL_SECS: f32 = 1.2;
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

/// An idea to call out, with a line of context explaining why. Set by clicking
/// the book a traveler is offering; the idea gets a rounded rectangle drawn
/// around it and `note` displayed inside.
#[derive(Clone, PartialEq)]
pub struct IdeaHighlight {
    /// Index into `ConstructedCity::ideas`.
    pub idea: usize,
    pub note: String,
}

/// A knowledge gain in progress: the masks as they stood *before* it, plus when
/// it started. Everything the cross-fade needs is derivable from that — each
/// segment's old color comes from the old masks, its new one from the current
/// ones.
pub struct Reveal {
    prev_learned: Vec<u64>,
    prev_understood: Vec<u64>,
    started: f32,
}

impl Reveal {
    /// What `segment` of `idea` looked like before this gain.
    fn prev_state(&self, idea: usize, segment: usize) -> Option<SegmentState> {
        let mask = 1u64 << segment;
        let (learned, understood) = (
            self.prev_learned.get(idea)?,
            self.prev_understood.get(idea)?,
        );
        Some(if understood & mask != 0 {
            SegmentState::Understood
        } else if learned & mask != 0 {
            SegmentState::LearnedOnly
        } else {
            SegmentState::Unknown
        })
    }
}

/// Whether the Ideas window is showing, plus anything it's currently calling
/// out. Toggled from the resource panel's "Ideas" button (see
/// `ui::shared_ui_system`).
#[derive(Resource, Default)]
pub struct IdeaWindowState {
    pub open: bool,
    pub focus: Option<IdeaHighlight>,
    pub reveal: Option<Reveal>,
}

/// The masks as of the last time we looked, so a change can be spotted.
#[derive(Default)]
pub struct KnowledgeSnapshot {
    learned: Vec<u64>,
    understood: Vec<u64>,
}

/// Pops the Ideas window up whenever the city learns something, and arms the
/// cross-fade that shows what changed.
///
/// Understanding is recomputed here rather than read from
/// `ConstructedCity::understood`, so this doesn't depend on having run after
/// `idea::sync_idea_progress` — the cache can lag by a frame, and announcing
/// against a stale one would either miss the newly-understood segments or
/// restart the fade a frame later.
pub fn announce_new_knowledge(
    mut snapshot: Local<KnowledgeSnapshot>,
    mut window: ResMut<IdeaWindowState>,
    idea_state: Res<IdeaState>,
    constructed: Res<ConstructedCity>,
    time: Res<Time>,
) {
    let learned = idea_state.learned.clone();
    let understood = compute_understood(&constructed.idea_deps, &learned);

    // First run (or an idea-list resize): adopt the state without announcing
    // it, so starting a session doesn't fire the window open.
    let first_look = snapshot.learned.len() != learned.len();
    let changed = snapshot.learned != learned || snapshot.understood != understood;

    if changed && !first_look {
        window.open = true;
        window.reveal = Some(Reveal {
            prev_learned: std::mem::take(&mut snapshot.learned),
            prev_understood: std::mem::take(&mut snapshot.understood),
            started: time.elapsed_secs(),
        });
    }
    if changed {
        snapshot.learned = learned;
        snapshot.understood = understood;
    }
}

pub fn idea_ui_system(
    mut contexts: EguiContexts,
    mut window: ResMut<IdeaWindowState>,
    idea_state: Res<IdeaState>,
    constructed: Res<ConstructedCity>,
    time: Res<Time>,
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
        &constructed.places,
    );

    // How far through the cross-fade we are, if one is running. Eased so the
    // new color arrives gently rather than snapping at the end.
    let fade = window.reveal.as_ref().map(|reveal| {
        let t = ((time.elapsed_secs() - reveal.started) / REVEAL_SECS).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    });
    if fade.is_some_and(|t| t < 1.0) {
        // Bevy usually redraws continuously, but don't rely on it for an
        // animation.
        ctx.request_repaint();
    }

    // `Window::open` needs its own bool: `window` is a `ResMut`, and handing
    // egui a `&mut` into it would mark the resource changed every frame.
    let mut open = true;
    let mut dismiss_focus = false;
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
                for (idx, row) in view.rows.iter().enumerate() {
                    let focus = window
                        .focus
                        .as_ref()
                        .filter(|highlight| highlight.idea == idx);
                    let geometry = idea_row(
                        ui,
                        idx,
                        row,
                        focus.map(|highlight| highlight.note.as_str()),
                        window.reveal.as_ref().zip(fade),
                    );
                    if focus.is_some() {
                        highlight_ring(ui, geometry.full);
                    }
                    for &prereq in &row.prereqs {
                        connector(ui, gutter_left, bars[prereq], geometry.bar, &mut lanes);
                    }
                    bars.push(geometry.bar);
                }
                ui.add_space(4.0);
                if window.focus.is_some() {
                    ui.separator();
                    dismiss_focus = ui.button("Clear highlight").clicked();
                }
            });
        });

    if !open {
        window.open = false;
    }
    if dismiss_focus {
        window.focus = None;
    }
    // Drop a finished reveal so it isn't re-evaluated every frame forever.
    if fade == Some(1.0) {
        window.reveal = None;
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

/// Where a drawn row ended up: `bar` for connectors to attach to, `full` for
/// the focus ring to enclose.
struct RowGeometry {
    bar: egui::Rect,
    full: egui::Rect,
}

/// Draws one idea's heading (with its confidence marker), summary, optional
/// focus note, and segment bar.
fn idea_row(
    ui: &mut egui::Ui,
    idx: usize,
    row: &IdeaRow,
    focus_note: Option<&str>,
    reveal: Option<(&Reveal, f32)>,
) -> RowGeometry {
    ui.add_space(4.0);
    let mut bar = egui::Rect::NOTHING;
    let full = ui
        .horizontal(|ui| {
            ui.add_space(LANE_GUTTER + row.depth as f32 * DEPTH_INDENT);
            ui.vertical(|ui| {
                heading(ui, row);
                note_label!(ui, row.summary());
                if let Some(note) = focus_note {
                    note_label!(ui, crate::col_format!(preview, "{}", note));
                }
                bar = segment_bar(ui, idx, row, reveal);
            });
        })
        .response
        .rect;
    RowGeometry { bar, full }
}

/// The idea's name plus a "?"/check marker. The marker is its own widget so it
/// can carry the hover text listing what this idea gates.
fn heading(ui: &mut egui::Ui, row: &IdeaRow) {
    ui.horizontal(|ui| {
        heading_label!(ui, row.name.clone());
        let color = if row.confident() {
            FontColors::understood()
        } else {
            FontColors::learned_only()
        };
        let marker = ui.label(
            egui::RichText::new(row.marker())
                .font(FontSizes::heading())
                .color(color),
        );
        marker.on_hover_ui(|ui| {
            if row.gated_places.is_empty() {
                ui.label(
                    egui::RichText::new("Nothing depends on this idea yet.")
                        .font(FontSizes::body()),
                );
                return;
            }
            ui.label(egui::RichText::new("Gates:").font(FontSizes::heading()));
            for place in &row.gated_places {
                ui.label(
                    egui::RichText::new(place.describe())
                        .font(FontSizes::body())
                        .color(match place.status {
                            crate::idea_view::GateStatus::Unlocked { .. } => {
                                FontColors::understood()
                            }
                            crate::idea_view::GateStatus::Locked { .. } => FontColors::unknown(),
                        }),
                );
            }
        });
    });
}

/// The rounded rectangle drawn around a focused idea. A stroke rather than a
/// fill, so it can be painted after the row without covering it.
fn highlight_ring(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().rect_stroke(
        rect.expand(3.0),
        6.0,
        egui::Stroke::new(1.5, FontColors::preview()),
        egui::StrokeKind::Outside,
    );
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

fn segment_color(state: SegmentState) -> egui::Color32 {
    match state {
        SegmentState::Understood => FontColors::understood(),
        SegmentState::LearnedOnly => FontColors::learned_only(),
        SegmentState::Unknown => FontColors::unknown(),
    }
}

fn lerp_color(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    egui::Color32::from_rgb(
        channel(from.r(), to.r()),
        channel(from.g(), to.g()),
        channel(from.b(), to.b()),
    )
}

/// Paints `row`'s segments across the available width and returns the rect they
/// occupy. During a reveal, each segment cross-fades from the color it had
/// before the gain to the one it has now — so what a book actually bought you
/// is the only thing moving on screen.
fn segment_bar(
    ui: &mut egui::Ui,
    idx: usize,
    row: &IdeaRow,
    reveal: Option<(&Reveal, f32)>,
) -> egui::Rect {
    let width = ui.available_width().max(row.segments.len() as f32);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, BAR_HEIGHT), egui::Sense::hover());

    let stride = rect.width() / row.segments.len() as f32;
    let segment_width = (stride - SEGMENT_GAP).max(1.0);
    let painter = ui.painter();
    for (i, &state) in row.segments.iter().enumerate() {
        let color = match reveal.and_then(|(reveal, t)| Some((reveal.prev_state(idx, i)?, t))) {
            Some((was, t)) if was != state => {
                lerp_color(segment_color(was), segment_color(state), t)
            }
            _ => segment_color(state),
        };
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + i as f32 * stride, rect.top()),
                egui::vec2(segment_width, rect.height()),
            ),
            0.0,
            color,
        );
    }

    let (understood, pending) = (row.understood, row.pending);
    let percent = (row.progress() * 100.0).round() as u32;
    response.on_hover_ui(|ui| {
        ui.label(
            egui::RichText::new(format!("{understood}/{SEGMENTS} understood ({percent}%)"))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal app with just the resources `announce_new_knowledge` needs.
    /// The run condition is deliberately omitted: it's an optimization, and
    /// leaving it off proves the system is idempotent when it *does* run.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(ConstructedCity::new(Vec::new()));
        app.init_resource::<IdeaState>();
        app.init_resource::<IdeaWindowState>();
        app.add_systems(Update, announce_new_knowledge);
        app
    }

    /// Opening a session shouldn't fire the window open at nothing -- the first
    /// look just adopts whatever's there.
    #[test]
    fn the_first_look_announces_nothing() {
        let mut app = app();
        app.update();
        assert!(!app.world().resource::<IdeaWindowState>().open);
        assert!(app.world().resource::<IdeaWindowState>().reveal.is_none());
    }

    /// Learning something pops the window and arms a fade carrying the masks as
    /// they stood *before* the gain -- that's what the cross-fade reads to know
    /// each segment's old color.
    #[test]
    fn learning_pops_the_window_and_arms_the_fade() {
        let mut app = app();
        app.update();

        app.world_mut().resource_mut::<IdeaState>().learn(0, 0b101);
        app.update();

        let window = app.world().resource::<IdeaWindowState>();
        assert!(window.open, "a gain should open the window");
        let reveal = window.reveal.as_ref().expect("fade armed");
        assert_eq!(reveal.prev_learned[0], 0, "masks are from before the gain");
        assert_eq!(reveal.prev_understood[0], 0);
    }

    /// Idle frames must not re-announce: otherwise a fade would restart forever
    /// and a window the player closed would spring back open.
    #[test]
    fn an_unchanged_frame_announces_nothing() {
        let mut app = app();
        app.update();
        app.world_mut().resource_mut::<IdeaState>().learn(0, 0b101);
        app.update();

        app.world_mut().resource_mut::<IdeaWindowState>().open = false;
        app.update();
        app.update();
        assert!(
            !app.world().resource::<IdeaWindowState>().open,
            "a closed window should stay closed while nothing is learned"
        );
    }

    /// A segment that was blocked and is now understood has to fade *from*
    /// amber, not from gray -- the old masks are what say so.
    #[test]
    fn prev_state_distinguishes_blocked_from_unread() {
        let reveal = Reveal {
            prev_learned: vec![0b110],
            prev_understood: vec![0b010],
            started: 0.0,
        };
        assert_eq!(reveal.prev_state(0, 1), Some(SegmentState::Understood));
        assert_eq!(reveal.prev_state(0, 2), Some(SegmentState::LearnedOnly));
        assert_eq!(reveal.prev_state(0, 0), Some(SegmentState::Unknown));
        assert_eq!(reveal.prev_state(9, 0), None, "unknown idea");
    }

    #[test]
    fn lerp_color_walks_from_one_end_to_the_other() {
        let (a, b) = (
            egui::Color32::from_rgb(0, 0, 0),
            egui::Color32::from_rgb(100, 200, 50),
        );
        assert_eq!(lerp_color(a, b, 0.0), a);
        assert_eq!(lerp_color(a, b, 1.0), b);
        assert_eq!(lerp_color(a, b, 0.5), egui::Color32::from_rgb(50, 100, 25));
    }
}
