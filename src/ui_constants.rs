use bevy_egui::egui::FontId;

/// Font sizes used throughout the UI. Adjust these for consistent scaling.
pub struct FontSizes;

impl FontSizes {
    /// Large heading text (e.g., "Month", "Travelers")
    pub fn heading() -> FontId {
        FontId::proportional(13.0)
    }

    /// Secondary heading or label text
    pub fn label() -> FontId {
        FontId::proportional(11.0)
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
