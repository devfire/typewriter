//! Liberation Sans: Helvetica-metric-compatible face for text boxes.
//!
//! Text boxes bake into the PDF as standard-14 Helvetica, so the
//! on-page preview draws them in Liberation Sans (metrically
//! compatible with Arial/Helvetica) instead of egui's default
//! proportional font: glyph advances then match
//! the baked font's width measure, and the lines wrapped by that
//! measure also *look* wrapped at the same place. The baked PDF
//! uses Helvetica for `WinAnsi` text and embeds this face (subset)
//! for text `WinAnsi` cannot represent (see `pdf::liberation_font`).
//! Licensed under the SIL Open Font License 1.1 (see
//! `assets/OFL-LiberationSans.txt`).

use eframe::egui;

/// Raw TrueType bytes, registered with egui by `app::theme`.
pub const FONT_BYTES: &[u8] = crate::pdf::FONT_BYTES;

/// Key into egui's `font_data` map.
pub const FONT_KEY: &str = "liberation-sans";

/// egui font-family name for text-box preview text.
pub const FONT_FAMILY: &str = "Liberation Sans";

/// Font id for drawing a text box at `size_pt` (scaled by zoom by callers).
#[must_use]
pub fn font_id(size_pt: f32) -> egui::FontId {
    egui::FontId::new(size_pt, egui::FontFamily::Name(FONT_FAMILY.into()))
}
