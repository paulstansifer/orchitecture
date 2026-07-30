use std::collections::HashMap;
use std::hash::Hash;

use bevy_egui::egui::{self, Response, Ui};
use bevy_egui::egui::{Color32, FontId};

/// Font sizes used throughout the UI. Adjust these for consistent scaling.
pub struct FontSizes;

impl FontSizes {
    /// Large heading text (e.g., "Month", "Travelers")
    pub fn heading() -> FontId {
        FontId::proportional(13.0)
    }

    /// Regular body text and most UI elements
    pub fn body() -> FontId {
        FontId::proportional(11.0)
    }

    /// Small text (e.g., details, small numbers)
    pub fn small() -> FontId {
        FontId::proportional(10.0)
    }
}

/// Font colors used throughout the UI.
pub struct FontColors;

impl FontColors {
    /// Light blue for previewing the future
    pub fn preview() -> Color32 {
        Color32::from_rgb(140, 170, 220)
    }

    /// Red for problems
    pub fn problem() -> Color32 {
        Color32::from_rgb(240, 120, 100)
    }

    /// White for default text
    pub fn default() -> Color32 {
        Color32::WHITE
    }

    /// Green for an idea segment that's understood — learned, with every
    /// prerequisite's matching segment understood too.
    pub fn understood() -> Color32 {
        Color32::from_rgb(120, 200, 130)
    }

    /// Amber for an idea segment that's been read about but can't be used yet,
    /// because some prerequisite is missing that same segment.
    pub fn learned_only() -> Color32 {
        Color32::from_rgb(210, 170, 70)
    }

    /// Dark gray for an idea segment nobody has read about.
    pub fn unknown() -> Color32 {
        Color32::from_gray(55)
    }
}

/// A lot like egui's `RichText`.
pub struct FormattedText {
    pub txt: String,
    pub fmt: bevy_egui::egui::TextFormat,
}

pub fn default_text_format() -> bevy_egui::egui::TextFormat {
    let mut fmt = bevy_egui::egui::TextFormat::default();
    fmt.color = FontColors::default();
    fmt
}

impl From<&str> for FormattedText {
    fn from(txt: &str) -> Self {
        FormattedText {
            txt: txt.to_string(),
            fmt: default_text_format(),
        }
    }
}

impl From<String> for FormattedText {
    fn from(txt: String) -> Self {
        FormattedText {
            txt,
            fmt: default_text_format(),
        }
    }
}

impl From<Option<String>> for FormattedText {
    fn from(txt: Option<String>) -> Self {
        FormattedText {
            txt: txt.unwrap_or("".to_string()),
            fmt: default_text_format(),
        }
    }
}

impl From<Option<FormattedText>> for FormattedText {
    fn from(fmt_txt: Option<FormattedText>) -> Self {
        fmt_txt.unwrap_or(FormattedText {
            txt: "".to_string(),
            fmt: default_text_format(),
        })
    }
}

/// Create colored text for use in `label!` or `note_label!` macros.
/// Usage: `col_format!(preview, "number={}", 5)`
#[macro_export]
macro_rules! col_format {
    ($color:ident, $text:expr $(, $arg:expr)*) => {{
        let mut fmt = $crate::ui_util::default_text_format();
        fmt.color = $crate::ui_util::FontColors::$color();
        $crate::ui_util::FormattedText {
            txt: format!($text, $($arg),*).to_string(),
            fmt,
        }
    }};
}

/// Builds a `LayoutJob` from `FormattedText` segments — each stamped with the
/// given font and italics setting — and renders it as a label. Backs the
/// `label!` / `heading_label!` / `note_label!` macros; not meant to be called
/// directly.
#[macro_export]
macro_rules! styled_label {
    ($ui:expr, $font:expr, $italics:expr, $($segment:expr),+ $(,)?) => {
        {
            let mut lj = ::bevy_egui::egui::text::LayoutJob::default();
            $(
                let mut seg: $crate::ui_util::FormattedText = $segment.into();
                seg.fmt.font_id = $font;
                seg.fmt.italics = $italics;
                lj.append(&seg.txt, 0.0, seg.fmt);
            )+
            $ui.label(lj)
        }
    };
}

/// Creates a LayoutJob from multiple RichText segments and renders it as a label.
/// Usage: `label!(ui, "Currently foo, ", col_format!(preview, "will be bar"))`
#[macro_export]
macro_rules! label {
    ($ui:expr, $($segment:expr),+ $(,)?) => {
        $crate::styled_label!($ui, $crate::ui_util::FontSizes::body(), false, $($segment),+)
    };
}

#[macro_export]
macro_rules! heading_label {
    ($ui:expr, $($segment:expr),+ $(,)?) => {
        $crate::styled_label!($ui, $crate::ui_util::FontSizes::heading(), false, $($segment),+)
    };
}

#[macro_export]
macro_rules! note_label {
    ($ui:expr, $($segment:expr),+ $(,)?) => {
        $crate::styled_label!($ui, $crate::ui_util::FontSizes::small(), true, $($segment),+)
    };
}

/// Renders a single fixed-size icon image. Thin wrapper over the
/// `egui::Image`/`SizedTexture` boilerplate that recurs at every icon cell.
pub fn icon(ui: &mut Ui, tex: egui::TextureId, size: [f32; 2]) -> Response {
    ui.add(egui::Image::new(egui::load::SizedTexture::new(tex, size)))
}

/// Renders the icon for `key` if `textures` has one registered, or falls back
/// to a plain text label (e.g. `""` for a blank cell). Used for the resource
/// grid's icon column, where not every key is guaranteed a texture.
pub fn resource_icon_cell<K: Eq + Hash>(
    ui: &mut Ui,
    textures: &HashMap<K, egui::TextureId>,
    key: &K,
    size: [f32; 2],
    fallback: &str,
) -> Response {
    match textures.get(key) {
        Some(&tex) => icon(ui, tex, size),
        None => ui.label(fallback),
    }
}

/// Colored text for a signed quantity this month projects: preview-blue for a
/// gain, problem-red for a loss, `None` at zero (callers typically fall back
/// to an empty segment or omit the piece entirely). `plus_prefix`/`minus_prefix`
/// let call sites vary the wording, e.g. `" +"`/" –" or "+"/"-".
pub fn signed_delta(delta: i64, plus_prefix: &str, minus_prefix: &str) -> Option<FormattedText> {
    if delta > 0 {
        Some(col_format_helper(
            FontColors::preview(),
            format!("{plus_prefix}{delta}"),
        ))
    } else if delta < 0 {
        Some(col_format_helper(
            FontColors::problem(),
            format!("{minus_prefix}{}", -delta),
        ))
    } else {
        None
    }
}

fn col_format_helper(color: Color32, txt: String) -> FormattedText {
    let mut fmt = default_text_format();
    fmt.color = color;
    FormattedText { txt, fmt }
}
