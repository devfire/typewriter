//! Vector typewriter mark for the empty state.
//!
//! Drawn with the egui painter instead of a font glyph: the bundled fonts
//! have no `\u{270E}`/typewriter glyph, so a text mark renders as a tofu
//! box. Vector shapes stay sharp at any DPI and need no image asset.

use super::ACCENT;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind};

const PAPER: Color32 = Color32::from_rgb(0xF8, 0xFA, 0xFC);
const PAPER_EDGE: Color32 = Color32::from_rgb(0x94, 0xA3, 0xB8);
const INK: Color32 = Color32::from_rgb(0x64, 0x74, 0x8B);
const BODY: Color32 = Color32::from_rgb(0x1E, 0x29, 0x3B);
const BODY_EDGE: Color32 = Color32::from_rgb(0x47, 0x55, 0x69);
const METAL: Color32 = Color32::from_rgb(0x33, 0x41, 0x55);
const KEY: Color32 = Color32::from_rgb(0xE2, 0xE8, 0xF0);

/// Draws a front-facing typewriter with a typed sheet in its carriage.
pub(super) fn typewriter_icon(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(148.0, 112.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let at = |x: f32, y: f32| Pos2::new(rect.min.x + x, rect.min.y + y);
    let box_at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(at(x, y), egui::vec2(w, h));

    // Typed sheet, tucked behind the carriage roller.
    painter.rect_filled(box_at(44.0, 4.0, 60.0, 56.0), 2.0, PAPER);
    painter.rect_stroke(
        box_at(44.0, 4.0, 60.0, 56.0),
        2.0,
        Stroke::new(1.2_f32, PAPER_EDGE),
        StrokeKind::Inside,
    );
    for (i, (x, w)) in [(52.0, 44.0), (52.0, 38.0), (52.0, 44.0), (52.0, 26.0)]
        .into_iter()
        .enumerate()
    {
        #[allow(clippy::cast_precision_loss)]
        let y = 16.0 + i as f32 * 9.0;
        let color = if i == 3 { ACCENT } else { INK };
        painter.line_segment([at(x, y), at(x + w, y)], Stroke::new(2.4_f32, color));
    }

    // Carriage return lever and roller knobs.
    painter.line_segment([at(28.0, 52.0), at(16.0, 42.0)], Stroke::new(3.0_f32, METAL));
    painter.circle_filled(at(14.0, 40.0), 4.0, ACCENT);
    painter.rect_filled(box_at(30.0, 50.0, 88.0, 11.0), 5.0, METAL);
    painter.rect_stroke(
        box_at(30.0, 50.0, 88.0, 11.0),
        5.0,
        Stroke::new(1.0_f32, BODY_EDGE),
        StrokeKind::Inside,
    );
    painter.circle_filled(at(28.0, 55.5), 6.5, BODY);
    painter.circle_stroke(at(28.0, 55.5), 6.5, Stroke::new(1.2_f32, BODY_EDGE));
    painter.circle_filled(at(120.0, 55.5), 6.5, BODY);
    painter.circle_stroke(at(120.0, 55.5), 6.5, Stroke::new(1.2_f32, BODY_EDGE));

    // Typebars fanning up to the print point.
    for dx in [-16.0, -8.0, 0.0, 8.0, 16.0] {
        painter.line_segment(
            [at(74.0 + dx, 68.0), at(74.0 + dx * 0.25, 59.0)],
            Stroke::new(1.6_f32, PAPER_EDGE),
        );
    }
    painter.circle_filled(at(74.0, 58.0), 3.0, ACCENT);

    // Body: trapezoid shell wider at the base.
    painter.add(Shape::convex_polygon(
        vec![at(36.0, 64.0), at(112.0, 64.0), at(124.0, 106.0), at(24.0, 106.0)],
        BODY,
        Stroke::new(1.2_f32, BODY_EDGE),
    ));
    // Accent nameplate strip across the top deck.
    painter.rect_filled(box_at(52.0, 67.0, 44.0, 5.0), 2.5, ACCENT);

    // Three staggered rows of round keys.
    for (row, (count, y)) in [(7, 78.0), (6, 85.0), (5, 92.0)].into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let offset = row as f32 * 5.0;
        for k in 0..count {
            #[allow(clippy::cast_precision_loss)]
            let x = 48.0 + offset + k as f32 * 9.5;
            painter.circle_filled(at(x, y), 3.6, KEY);
            painter.circle_stroke(at(x, y), 3.6, Stroke::new(0.8_f32, PAPER_EDGE));
        }
    }
    // Space bar.
    painter.rect_filled(box_at(58.0, 98.0, 32.0, 5.0), 2.5, KEY);

    // Feet.
    painter.rect_filled(box_at(32.0, 106.0, 12.0, 4.0), 1.5, METAL);
    painter.rect_filled(box_at(104.0, 106.0, 12.0, 4.0), 1.5, METAL);
}
