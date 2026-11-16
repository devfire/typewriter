//! Liberation Sans: Helvetica-metric-compatible face for text boxes.
//!
//! Text boxes bake into the PDF as standard-14 Helvetica, so the
//! on-page preview draws them in Liberation Sans (metrically
//! compatible with Arial/Helvetica) instead of egui's default
//! proportional font: glyph advances then match
//! `pdf::helvetica_width_pt`, and the lines wrapped by that measure
//! also *look* wrapped at the same place. Preview only — the baked
//! PDF still uses Helvetica itself and embeds nothing for text.
//! Licensed under the SIL Open Font License 1.1 (see
//! `assets/OFL-LiberationSans.txt`).

use eframe::egui;

/// Raw TrueType bytes, registered with egui by `app::theme`.
pub const FONT_BYTES: &[u8] = include_bytes!("../assets/LiberationSans-Regular.ttf");

/// Key into egui's `font_data` map.
pub const FONT_KEY: &str = "liberation-sans";

/// egui font-family name for text-box preview text.
pub const FONT_FAMILY: &str = "Liberation Sans";

/// Font id for drawing a text box at `size_pt` (scaled by zoom by callers).
#[must_use]
pub fn font_id(size_pt: f32) -> egui::FontId {
    egui::FontId::new(size_pt, egui::FontFamily::Name(FONT_FAMILY.into()))
}
