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

/// Creates a LayoutJob from multiple RichText segments and renders it as a label.
/// Usage: `label!(ui, "Currently foo, ", col_format!(preview, "will be bar"))`
#[macro_export]
macro_rules! label {
    ($ui:expr, $($segment:expr),+ $(,)?) => {
        {
            let mut lj = ::bevy_egui::egui::text::LayoutJob::default();
            $(
                let mut seg: $crate::ui_util::FormattedText = $segment.into();
                seg.fmt.font_id = $crate::ui_util::FontSizes::body();
                lj.append(&seg.txt, 0.0, seg.fmt);
            )+
            $ui.label(lj);
        }
    };
}

#[macro_export]
macro_rules! heading_label {
    ($ui:expr, $($segment:expr),+ $(,)?) => {
        {
            let mut lj = ::bevy_egui::egui::text::LayoutJob::default();
            $(
                let mut seg: $crate::ui_util::FormattedText = $segment.into();
                seg.fmt.font_id = $crate::ui_util::FontSizes::heading();
                lj.append(&seg.txt, 0.0, seg.fmt);
            )+
            $ui.label(lj);
        }
    };
}

#[macro_export]
macro_rules! note_label {
    ($ui:expr, $($segment:expr),+ $(,)?) => {
        {
            let mut lj = ::bevy_egui::egui::text::LayoutJob::default();
            $(
                let mut seg: $crate::ui_util::FormattedText = $segment.into();
                seg.fmt.font_id = $crate::ui_util::FontSizes::small();
                seg.fmt.italics = true;
                lj.append(&seg.txt, 0.0, seg.fmt);
            )+
            $ui.label(lj);
        }
    };
}
