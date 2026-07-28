//! The Ideas window: the idea DAG drawn as a layered graph, one 50-segment bar
//! per idea.
//!
//! Layout comes straight from the depths [`crate::idea_view::idea_tree_view`]
//! computes: an idea's depth (its longest path from a root) is the screen row it
//! goes in, so every idea sits below everything it depends on, and ideas at the
//! same depth sit side by side. Dependency lines are painted *under* the cards
//! — reserved with `Painter::add` before the row is laid out and filled in once
//! every card's rect is known — so a line passing a layer disappears behind it
//! rather than scribbling through it. Edges may skip a layer; they're drawn
//! straight and left for the reader to follow, on the assumption that ideas
//! aren't usually more than a layer or two apart.
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
/// Gap between adjacent segments. Segments are drawn at least 1px wide, so a
/// narrow card degrades to a solid bar rather than to nothing.
const SEGMENT_GAP: f32 = 1.0;
/// Narrowest a card may get before a layer wraps instead of squeezing further.
/// Below about this, 50 segments stop being individually readable.
const CARD_MIN_WIDTH: f32 = 170.0;
/// Horizontal gap between cards sharing a layer.
const CARD_GAP: f32 = 8.0;
/// Vertical gap between layers — the room dependency lines have to be visible in.
const LAYER_GAP: f32 = 18.0;

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
        .default_width(560.0)
        .default_height(420.0)
        .resizable(true)
        .show(ctx, |ui| {
            legend(ui);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                // Reserve a slot for the dependency lines before any card is
                // drawn, so they end up beneath every card in paint order. The
                // shapes themselves can't be built until every card's rect is
                // known, which is why this is a placeholder rather than a draw.
                let painter = ui.painter().clone();
                let edges = painter.add(egui::Shape::Noop);

                let full_width = ui.available_width();
                let mut cards: Vec<egui::Rect> = vec![egui::Rect::NOTHING; view.rows.len()];

                for layer in view.layers() {
                    let count = layer.len().max(1) as f32;
                    let card_width =
                        ((full_width - (count - 1.0) * CARD_GAP) / count).max(CARD_MIN_WIDTH);
                    // Wrapping rather than clipping: a layer too crowded to fit
                    // spills onto another line, which muddles the depth reading
                    // but keeps every idea visible and readable. `Align::Min`
                    // rather than `horizontal_wrapped`'s centering, so cards in
                    // a layer line up along their tops however tall each gets.
                    let layout = egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true);
                    ui.with_layout(layout, |ui| {
                        ui.spacing_mut().item_spacing.x = CARD_GAP;
                        for &idx in &layer {
                            let focus = window
                                .focus
                                .as_ref()
                                .filter(|highlight| highlight.idea == idx);
                            let rect = idea_card(
                                ui,
                                idx,
                                &view.rows[idx],
                                card_width,
                                focus.map(|highlight| highlight.note.as_str()),
                                window.reveal.as_ref().zip(fade),
                            );
                            if focus.is_some() {
                                highlight_ring(ui, rect);
                            }
                            cards[idx] = rect;
                        }
                    });
                    ui.add_space(LAYER_GAP);
                }

                painter.set(edges, egui::Shape::Vec(dependency_lines(&view, &cards)));

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

/// One line per dependency, from the bottom of the prerequisite's card to the
/// top of its dependent's. Where a card has several prerequisites the arrival
/// points are fanned across its top edge, so two lines into the same idea stay
/// tellable apart instead of converging on one pixel.
fn dependency_lines(
    view: &crate::idea_view::IdeaTreeView,
    cards: &[egui::Rect],
) -> Vec<egui::Shape> {
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(105));
    let mut lines = Vec::new();
    for (idx, row) in view.rows.iter().enumerate() {
        let to = cards[idx];
        if !to.is_positive() {
            continue;
        }
        let fan = row.prereqs.len() as f32 + 1.0;
        for (i, &prereq) in row.prereqs.iter().enumerate() {
            let from = cards[prereq];
            if !from.is_positive() {
                continue;
            }
            let arrival = egui::pos2(to.left() + to.width() * (i as f32 + 1.0) / fan, to.top());
            lines.push(egui::Shape::line_segment(
                [from.center_bottom(), arrival],
                stroke,
            ));
        }
    }
    lines
}

/// Draws one idea as a card `width` wide: heading (with its confidence marker),
/// summary, optional focus note, and segment bar. Returns the card's rect, which
/// is what the dependency lines attach to.
fn idea_card(
    ui: &mut egui::Ui,
    idx: usize,
    row: &IdeaRow,
    width: f32,
    focus_note: Option<&str>,
    reveal: Option<(&Reveal, f32)>,
) -> egui::Rect {
    // The layout has to be stated: `allocate_ui` would inherit the enclosing
    // left-to-right one and lay the card's own contents out in a single line.
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            // An opaque fill is what makes "lines underneath" read as underneath:
            // a line passing this layer is hidden here and reappears in the gap.
            egui::Frame::new()
                .fill(egui::Color32::from_gray(38))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(58)))
                .inner_margin(egui::Margin::same(6))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, row);
                    note_label!(ui, row.summary());
                    if let Some(note) = focus_note {
                        note_label!(ui, crate::col_format!(preview, "{}", note));
                    }
                    segment_bar(ui, idx, row, reveal);
                });
        },
    )
    .response
    .rect
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
/// fill, so it can be painted after the card without covering it.
fn highlight_ring(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().rect_stroke(
        rect.expand(2.0),
        6.0,
        egui::Stroke::new(1.5, FontColors::preview()),
        egui::StrokeKind::Outside,
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

/// Paints `row`'s segments across the available width. During a reveal, each
/// segment cross-fades from the color it had before the gain to the one it has
/// now — so what a book actually bought you is the only thing moving on screen.
fn segment_bar(ui: &mut egui::Ui, idx: usize, row: &IdeaRow, reveal: Option<(&Reveal, f32)>) {
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
