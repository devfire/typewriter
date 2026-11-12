//! Momo Signature: the embedded script font used for signature boxes.
//!
//! One embedded copy serves both renderers: egui registers it for the
//! on-page preview, and `pdf::signature_font` embeds the same bytes as a
//! Type0 font when baking, so preview and saved PDF cannot drift.
//! Licensed under the SIL Open Font License 1.1 (see `assets/OFL.txt`).

use eframe::egui;

/// Raw TrueType bytes (also embedded verbatim into baked PDFs).
pub const FONT_BYTES: &[u8] = include_bytes!("../assets/MomoSignature-Regular.ttf");

/// Key into egui's `font_data` map.
pub const FONT_KEY: &str = "momo-signature";

/// egui font-family name for signature text.
pub const FONT_FAMILY: &str = "Momo Signature";

/// Live signature preview on a ruled line, as shown in the inspector.
pub fn draw_preview(ui: &mut egui::Ui, line: &str, ink: egui::Color32, is_placeholder: bool) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 62.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, ui.visuals().extreme_bg_color);
    painter.text(
        rect.center() - egui::vec2(0.0, 4.0),
        egui::Align2::CENTER_CENTER,
        if is_placeholder { "Your name" } else { line },
        font_id(34.0),
        if is_placeholder {
            ui.visuals().weak_text_color()
        } else {
            ink
        },
    );
    painter.line_segment(
        [
            egui::pos2(rect.min.x + 14.0, rect.max.y - 10.0),
            egui::pos2(rect.max.x - 14.0, rect.max.y - 10.0),
        ],
        egui::Stroke::new(1.0_f32, ui.visuals().weak_text_color()),
    );
}

/// Font id for drawing a signature at `size_pt` (scaled by zoom by callers).
#[must_use]
pub fn font_id(size_pt: f32) -> egui::FontId {
    egui::FontId::new(size_pt, egui::FontFamily::Name(FONT_FAMILY.into()))
}
