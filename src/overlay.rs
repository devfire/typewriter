//! Overlay text boxes and screen<->PDF coordinate math.
//!
//! Single source of truth for `ASCENT`/`LEADING`: the egui preview in
//! `app.rs` and the content-stream baker in `pdf.rs` both import these,
//! so preview and baked output cannot drift.

use eframe::egui::{Pos2, Rect};

/// Fraction of the font size from the top of the em box to the baseline.
pub const ASCENT: f32 = 0.8;
/// Line spacing as a multiple of the font size.
pub const LEADING: f32 = 1.2;

/// What a box contains: ordinary typed text, or a typed signature
/// rendered (preview and baked PDF) in the Momo Signature script font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    Text,
    Signature,
}

/// One typewriter text box. `x_pt`/`y_top_pt` are PDF points, with
/// `y_top_pt` measured from the TOP of the page to the top of the em box.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayText {
    pub kind: BoxKind,
    pub page: usize,
    pub x_pt: f32,
    pub y_top_pt: f32,
    pub text: String,
    pub size_pt: f32,
    pub color: [u8; 3],
    /// Manual box width in PDF points, set by dragging the resize
    /// handle. `None` = auto-fit to the measured rendered text.
    pub width_pt: Option<f32>,
}

impl OverlayText {
    pub fn new(page: usize, x_pt: f32, y_top_pt: f32, kind: BoxKind) -> Self {
        let (size_pt, color) = match kind {
            BoxKind::Text => (12.0, [0, 0, 0]),
            // Larger by default, in dark-blue pen ink.
            BoxKind::Signature => (32.0, [0x20, 0x2F, 0x6B]),
        };
        Self {
            kind,
            page,
            x_pt,
            y_top_pt,
            text: String::new(),
            size_pt,
            color,
            width_pt: None,
        }
    }

    /// The signature as one line: signatures never wrap, so embedded
    /// newlines (pasted text) collapse to spaces.
    #[must_use]
    pub fn signature_line(&self) -> String {
        self.text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// All overlay state for the open document.
#[derive(Debug, Default)]
pub struct OverlayState {
    pub items: Vec<OverlayText>,
    pub selected: Option<usize>,
    pub dirty: bool,
    /// Bumped whenever the underlying document bytes change (open,
    /// post-save reload) so the texture cache can invalidate.
    pub doc_generation: u64,
}

impl OverlayState {
    /// Insert a new empty box and select it. Returns its index.
    pub fn add(&mut self, page: usize, x_pt: f32, y_top_pt: f32, kind: BoxKind) -> usize {
        self.items
            .push(OverlayText::new(page, x_pt, y_top_pt, kind));
        let idx = self.items.len() - 1;
        self.selected = Some(idx);
        self.dirty = true;
        idx
    }

    pub fn remove(&mut self, idx: usize) {
        if idx >= self.items.len() {
            return;
        }
        self.items.remove(idx);
        self.selected = match self.selected {
            Some(sel) if sel == idx => None,
            Some(sel) if sel > idx => Some(sel - 1),
            other => other,
        };
        self.dirty = true;
    }

    /// Boxes on `page`, paired with their global indices.
    pub fn by_page(&self, page: usize) -> Vec<(usize, &OverlayText)> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.page == page)
            .collect()
    }

    /// Drop the selected box if it is empty (click placed it but the
    /// user never typed). Called before selection changes / save.
    pub fn drop_empty_selected(&mut self) {
        if let Some(sel) = self.selected
            && self.items.get(sel).is_some_and(OverlayText::is_empty)
        {
            self.items.remove(sel);
            self.selected = None;
        }
    }

    /// Clear everything after a successful bake.
    pub fn clear_baked(&mut self) {
        self.items.clear();
        self.selected = None;
        self.dirty = false;
        self.doc_generation = self.doc_generation.wrapping_add(1);
    }

    pub fn bump_generation(&mut self) {
        self.doc_generation = self.doc_generation.wrapping_add(1);
    }
}

/// Convert a screen position inside `page_rect` to PDF points measured
/// from the page's top-left corner.
#[must_use]
pub fn screen_to_pdf_top(screen: Pos2, page_rect: Rect, zoom: f32) -> (f32, f32) {
    let zoom = if zoom > 0.0 { zoom } else { 1.0 };
    (
        ((screen.x - page_rect.min.x) / zoom).max(0.0),
        ((screen.y - page_rect.min.y) / zoom).max(0.0),
    )
}

/// Inverse of [`screen_to_pdf_top`].
#[must_use]
pub fn pdf_top_to_screen(x_pt: f32, y_top_pt: f32, page_rect: Rect, zoom: f32) -> Pos2 {
    Pos2::new(
        page_rect.min.x + x_pt * zoom,
        page_rect.min.y + y_top_pt * zoom,
    )
}

/// Top-left-origin baseline: PDF y of the first line's baseline.
#[must_use]
pub fn baseline_y(y1: f32, y_top_pt: f32, size_pt: f32) -> f32 {
    y1 - y_top_pt - ASCENT * size_pt
}

/// `Tm` operands `[a b c d e f]` for a box on a page with `/Rotate`
/// `rotate` (clockwise display rotation, in degrees).
///
/// Overlay coordinates are display space: `x_pt` measured from the
/// displayed page's left edge, `y_top_pt` from its top edge. The
/// linear part counter-rotates the glyphs in unrotated user space so
/// they read upright once the renderer applies `/Rotate`, and
/// `(e, f)` maps the first line's baseline origin back into user
/// space. `/Rotate` values that are not a multiple of 90 are invalid
/// per the PDF spec and are treated as 0, matching how `sizes_pt`
/// only swaps the display size for 90/270.
#[must_use]
pub fn text_matrix(item: &OverlayText, page_box: (f32, f32, f32, f32), rotate: i64) -> [f32; 6] {
    let (x0, y0, x1, y1) = page_box;
    // Distance from the displayed page's top edge to the baseline.
    let baseline_top = item.y_top_pt + ASCENT * item.size_pt;
    match rotate.rem_euclid(360) {
        90 => [0.0, 1.0, -1.0, 0.0, x0 + baseline_top, y0 + item.x_pt],
        180 => [-1.0, 0.0, 0.0, -1.0, x1 - item.x_pt, y0 + baseline_top],
        270 => [0.0, -1.0, 1.0, 0.0, x1 - baseline_top, y1 - item.x_pt],
        _ => [
            1.0,
            0.0,
            0.0,
            1.0,
            x0 + item.x_pt,
            baseline_y(y1, item.y_top_pt, item.size_pt),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::new(10.0, 20.0), eframe::egui::vec2(612.0, 792.0))
    }

    #[test]
    fn screen_pdf_roundtrip_at_zoom_1() {
        let r = rect();
        let (x, y) = screen_to_pdf_top(Pos2::new(110.0, 220.0), r, 1.0);
        assert!((x - 100.0).abs() < 0.001);
        assert!((y - 200.0).abs() < 0.001);
        let back = pdf_top_to_screen(x, y, r, 1.0);
        assert!((back.x - 110.0).abs() < 0.001);
        assert!((back.y - 220.0).abs() < 0.001);
    }

    #[test]
    fn screen_pdf_roundtrip_at_zoom_1_5() {
        let r = rect();
        let (x, y) = screen_to_pdf_top(Pos2::new(160.0, 320.0), r, 1.5);
        assert!((x - 100.0).abs() < 0.001);
        assert!((y - 200.0).abs() < 0.001);
        let back = pdf_top_to_screen(x, y, r, 1.5);
        assert!((back.x - 160.0).abs() < 0.001);
        assert!((back.y - 320.0).abs() < 0.001);
    }

    #[test]
    fn add_selects_and_remove_clears_selection() {
        let mut state = OverlayState::default();
        let idx = state.add(0, 10.0, 20.0, BoxKind::Text);
        assert_eq!(state.selected, Some(idx));
        assert!(state.dirty);
        state.remove(idx);
        assert_eq!(state.selected, None);
        assert!(state.items.is_empty());
    }
}
