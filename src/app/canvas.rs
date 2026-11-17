//! Page canvas: rendered page, overlay boxes, and box layout cache.

use super::{ACCENT, MAX_ZOOM, MIN_ZOOM, Mode, RADIUS, TypewriterApp, typewriter_icon};
use crate::overlay::{self, BoxId, BoxKind, OverlayText};
use eframe::egui;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Identity of the inputs that determine a box's laid-out lines and
/// measured auto width. The text itself is hashed so keying the cache
/// never clones the `String`.
#[derive(Debug, PartialEq, Eq)]
struct BoxLayoutKey {
    text_hash: u64,
    size_pt_bits: u32,
    kind: BoxKind,
    width_pt_bits: Option<u32>,
}

/// Laid-out display lines and measured auto width for one box,
/// reused across frames until its `BoxLayoutKey` changes.
pub(super) struct CachedBoxLayout {
    key: BoxLayoutKey,
    lines: Arc<[String]>,
    auto_width_pt: f32,
}

fn box_layout_key(item: &OverlayText) -> BoxLayoutKey {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    item.text.hash(&mut hasher);
    BoxLayoutKey {
        text_hash: hasher.finish(),
        size_pt_bits: item.size_pt.to_bits(),
        kind: item.kind,
        width_pt_bits: item.width_pt.map(f32::to_bits),
    }
}

impl TypewriterApp {
    pub(super) fn canvas(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::central_panel(ctx.style().as_ref())
            .fill(ctx.style().visuals.extreme_bg_color);
        egui::CentralPanel::default()
            .frame(frame)
            .show(ctx, |ui| self.canvas_content(ctx, ui));
    }

    /// Welcome screen shown before any PDF is loaded.
    fn empty_state(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.22).max(24.0));
            typewriter_icon::typewriter_icon(ui);
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Typewriter PDF").size(30.0).strong());
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Open a PDF, switch to Type or Sign mode, and click the page to write or sign.",
                )
                .size(15.0)
                .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(20.0);
            let open_btn = egui::Button::new(
                egui::RichText::new("Open a PDF…")
                    .size(16.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(ACCENT)
            .stroke(egui::Stroke::NONE)
            .min_size(egui::vec2(200.0, 48.0))
            .corner_radius(RADIUS);
            if ui
                .add(open_btn)
                .on_hover_text("Open a PDF  (Ctrl+O)")
                .clicked()
            {
                self.open_dialog();
            }
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new("Ctrl+O to open  ·  Ctrl+S to save  ·  Esc to navigate")
                    .size(12.5)
                    .color(ui.visuals().weak_text_color()),
            );
        });
    }

    fn canvas_content(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let (Some(pdf), Some(doc)) = (self.hayro_pdf.as_ref(), self.pdf_doc.as_ref()) else {
            self.empty_state(ui);
            return;
        };
        let page = self.page.min(doc.page_count().saturating_sub(1));
        let (dim_w, dim_h) = doc.sizes_pt.get(page).copied().unwrap_or((612.0, 792.0));
        if self.fit_pending {
            let avail = ui.available_size();
            if dim_w > 0.0 && dim_h > 0.0 && avail.x > 0.0 && avail.y > 0.0 {
                self.zoom = (avail.x / dim_w)
                    .min(avail.y / dim_h)
                    .clamp(MIN_ZOOM, MAX_ZOOM);
            }
            self.fit_pending = false;
        }
        let zoom = self.zoom;
        let texture = match self.renderer.render_page(ctx, pdf, page, zoom) {
            Ok(texture) => Some(texture),
            Err(err) => {
                self.status = format!("Render failed: {err:#}");
                None
            }
        };
        let mode = self.mode;
        let page_size = egui::vec2(dim_w * zoom, dim_h * zoom);
        let viewport = ui.available_size();
        let origin = ui.cursor().min;
        let scroll_offset = self.canvas_scroll_offset(origin, viewport, page_size);
        let mut scroll = egui::ScrollArea::both()
            .auto_shrink([false, false])
            .scroll_source(egui::containers::scroll_area::ScrollSource {
                drag: mode == Mode::Navigate,
                ..Default::default()
            });
        if let Some(offset) = scroll_offset {
            scroll = scroll.scroll_offset(offset);
        }
        let output = scroll.show(ui, |ui| {
            // Content is at least the viewport so the page centers when it
            // fits; larger content lets the page pan when it overflows.
            let content_size = page_size.max(ui.available_size());
            let (content_rect, _) = ui.allocate_exact_size(content_size, egui::Sense::hover());
            let page_min = content_rect.left_top() + (content_size - page_size) * 0.5;
            let page_rect = egui::Rect::from_min_size(page_min, page_size);
            let page_resp = ui.allocate_rect(page_rect, egui::Sense::click_and_drag());
            let painter = ui.painter_at(page_rect.expand(16.0));
            // Layered soft shadow so the page floats over the canvas.
            painter.rect_filled(
                page_rect.translate(egui::vec2(0.0, 10.0)).expand(2.0),
                8.0,
                egui::Color32::from_black_alpha(28),
            );
            painter.rect_filled(
                page_rect.translate(egui::vec2(0.0, 4.0)),
                6.0,
                egui::Color32::from_black_alpha(36),
            );
            painter.rect_filled(page_rect, 4.0, egui::Color32::WHITE);
            painter.rect_stroke(
                page_rect,
                4.0,
                egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(36)),
                egui::StrokeKind::Outside,
            );
            if let Some(texture) = &texture {
                painter.image(
                    texture.id(),
                    page_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            self.draw_boxes(ui, page_rect, page, zoom, &page_resp);
        });
        self.canvas_view = Some(super::CanvasView {
            page: self.page,
            zoom_bits: self.zoom.to_bits(),
            origin,
            scroll_offset: output.state.offset,
        });
    }

    /// Center a new page/zoom. When only the canvas origin moves, adjust the
    /// previous user scroll offset by the same delta so the page stays put.
    pub(super) fn canvas_scroll_offset(
        &self,
        origin: egui::Pos2,
        viewport: egui::Vec2,
        page_size: egui::Vec2,
    ) -> Option<egui::Vec2> {
        let centered = (page_size - viewport).max(egui::Vec2::ZERO) * 0.5;
        match self.canvas_view {
            None => Some(centered),
            Some(previous)
                if previous.page != self.page || previous.zoom_bits != self.zoom.to_bits() =>
            {
                Some(centered)
            }
            Some(previous) if previous.origin != origin => {
                Some(previous.scroll_offset + (origin - previous.origin))
            }
            Some(_) => None,
        }
    }

    /// Display lines and measured auto width for box `idx`, cached by
    /// text, size, kind, and fixed width. Layout is in PDF points and
    /// uses the baked Helvetica metrics for text, so it does not
    /// depend on zoom.
    fn box_layout(&mut self, ui: &egui::Ui, idx: BoxId, zoom: f32) -> Option<(Arc<[String]>, f32)> {
        let item = self.overlays.item(idx)?;
        let key = box_layout_key(item);
        if let Some(cached) = self.box_layouts.get(&idx)
            && cached.key == key
        {
            return Some((Arc::clone(&cached.lines), cached.auto_width_pt));
        }
        let font = box_font(item, zoom);
        let lines = box_display_lines(item);
        let auto_width_pt = measured_width_pt(ui, &lines, &font, zoom, item);
        let lines: Arc<[String]> = Arc::from(lines);
        self.box_layouts.insert(
            idx,
            CachedBoxLayout {
                key,
                lines: Arc::clone(&lines),
                auto_width_pt,
            },
        );
        Some((lines, auto_width_pt))
    }

    #[allow(clippy::cast_precision_loss)]
    pub(super) fn draw_boxes(
        &mut self,
        ui: &mut egui::Ui,
        page_rect: egui::Rect,
        page: usize,
        zoom: f32,
        page_resp: &egui::Response,
    ) {
        self.box_layouts
            .retain(|id, _| self.overlays.item(*id).is_some());
        let box_indices: Vec<BoxId> = self
            .overlays
            .by_page(page)
            .into_iter()
            .map(|item| item.id)
            .collect();
        let mut box_rects: Vec<egui::Rect> = Vec::with_capacity(box_indices.len());
        for idx in box_indices {
            let Some((display_lines, auto_width_pt)) = self.box_layout(ui, idx, zoom) else {
                continue;
            };
            let Some(item) = self.overlays.item(idx) else {
                continue;
            };
            let font = box_font(item, zoom);
            let top_left = overlay::pdf_top_to_screen(item.x_pt, item.y_top_pt, page_rect, zoom);
            let line_count = display_lines.len().max(1) as f32;
            let width_pt = item.width_pt.map_or(auto_width_pt, |w| w.max(24.0));
            let height_pt = line_count * overlay::LEADING * item.size_pt;
            // Pad visual box for script font ascenders/descenders outside the em box.
            let (pad_top_pt, pad_bottom_pt) = box_vertical_pad_pt(item);
            let rect = egui::Rect::from_min_max(
                egui::pos2(top_left.x, top_left.y - pad_top_pt * zoom),
                egui::pos2(
                    top_left.x + width_pt * zoom,
                    top_left.y + (height_pt + pad_bottom_pt) * zoom,
                ),
            );
            box_rects.push(rect);
            let selected = self.overlays.selected == Some(idx);
            let resp = ui.interact(
                rect,
                ui.id().with(("tw-box", idx)),
                egui::Sense::click_and_drag(),
            );
            let handle_rect =
                egui::Rect::from_center_size(rect.right_bottom(), egui::vec2(16.0, 16.0));
            if selected {
                self.resize_handle(ui, idx, handle_rect, width_pt, zoom);
            }
            if resp.clicked() {
                self.overlays.selected = Some(idx);
            }
            if resp.drag_started() && self.resize_box != Some(idx) {
                self.overlays.begin_gesture();
            }
            if resp.dragged() && self.resize_box != Some(idx) {
                self.overlays.selected = Some(idx);
                let delta = resp.drag_delta();
                let max_left_pt = (page_rect.width() / zoom - width_pt).max(0.0);
                let max_top_pt = (page_rect.height() / zoom - height_pt).max(0.0);
                if let Some(current) = self.overlays.item_mut(idx) {
                    current.x_pt = (current.x_pt + delta.x / zoom).clamp(0.0, max_left_pt);
                    current.y_top_pt = (current.y_top_pt + delta.y / zoom).clamp(0.0, max_top_pt);
                }
                self.overlays.dirty = true;
            }
            if resp.drag_stopped() {
                self.overlays.end_gesture();
            }
            let Some(item) = self.overlays.item(idx) else {
                continue;
            };
            let painter = ui.painter_at(rect.expand(4.0));
            if selected {
                painter.rect_filled(rect, 2.0, egui::Color32::from_white_alpha(170));
                painter.rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.5_f32, ACCENT),
                    egui::StrokeKind::Outside,
                );
                paint_box_text(&painter, item, &display_lines, &font, rect, zoom, true);
                painter.rect_filled(handle_rect, 3.0, ACCENT);
                painter.rect_stroke(
                    handle_rect,
                    3.0,
                    egui::Stroke::new(1.5_f32, egui::Color32::WHITE),
                    egui::StrokeKind::Outside,
                );
            } else {
                paint_box_text(&painter, item, &display_lines, &font, rect, zoom, false);
            }
        }
        self.place_box_on_click(page, page_rect, zoom, page_resp, &box_rects);
    }

    /// Click on empty page area creates a box in Type / Sign mode.
    fn place_box_on_click(
        &mut self,
        page: usize,
        page_rect: egui::Rect,
        zoom: f32,
        page_resp: &egui::Response,
        box_rects: &[egui::Rect],
    ) {
        let new_kind = match self.mode {
            Mode::Type => Some(BoxKind::Text),
            Mode::Sign => Some(BoxKind::Signature),
            Mode::Navigate => None,
        };
        if let Some(kind) = new_kind
            && page_resp.clicked()
            && let Some(pos) = page_resp.interact_pointer_pos()
            && !box_rects.iter().any(|r| r.contains(pos))
        {
            let (x_pt, y_top_pt) = overlay::screen_to_pdf_top(pos, page_rect, zoom);
            self.overlays.add(page, x_pt, y_top_pt, kind);
            self.focus_editor = true;
        }
    }

    /// Bottom-right corner handle of a selected box: drag to resize width,
    /// double-click to return to auto-fit.
    fn resize_handle(
        &mut self,
        ui: &mut egui::Ui,
        idx: BoxId,
        handle_rect: egui::Rect,
        width_pt: f32,
        zoom: f32,
    ) {
        let id = ui.id().with(("tw-resize", idx));
        let resize_resp = ui
            .interact(handle_rect, id, egui::Sense::click_and_drag())
            .on_hover_text("Drag to widen/narrow the box \u{00b7} double-click for auto-fit");
        if resize_resp.double_clicked() {
            self.overlays.checkpoint();
            if let Some(current) = self.overlays.item_mut(idx) {
                current.width_pt = None;
            }
            self.overlays.dirty = true;
        }
        if resize_resp.drag_started() {
            self.overlays.begin_gesture();
            self.resize_box = Some(idx);
        }
        if resize_resp.hovered() || resize_resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if self.resize_box == Some(idx) && resize_resp.dragged() {
            let delta_pt = resize_resp.drag_delta().x / zoom;
            if let Some(current) = self.overlays.item_mut(idx) {
                let base = current.width_pt.unwrap_or(width_pt);
                current.width_pt = Some((base + delta_pt).max(24.0));
            }
            self.overlays.dirty = true;
        }
        if resize_resp.drag_stopped() {
            self.overlays.end_gesture();
            self.resize_box = None;
        }
    }
}

/// Draw a box's lines (plus the empty-box placeholder when selected) inside `rect`.
fn paint_box_text(
    painter: &egui::Painter,
    item: &OverlayText,
    display_lines: &[String],
    font: &egui::FontId,
    rect: egui::Rect,
    zoom: f32,
    selected: bool,
) {
    let (pad_top_pt, _) = box_vertical_pad_pt(item);
    let color = egui::Color32::from_rgb(item.color[0], item.color[1], item.color[2]);
    let first_baseline_y = rect.min.y + (pad_top_pt + overlay::ASCENT * item.size_pt) * zoom;
    for (line_idx, line) in display_lines.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let baseline_y =
            first_baseline_y + (line_idx as f32) * overlay::LEADING * item.size_pt * zoom;
        let pos = egui::pos2(rect.min.x, baseline_y);
        paint_line_at_baseline(painter, pos, line, font, color);
    }
    if item.text.is_empty() && selected {
        let placeholder = match item.kind {
            BoxKind::Signature => "Sign here\u{2026}",
            BoxKind::Text => "Type here\u{2026}",
        };
        let pos = egui::pos2(rect.min.x, first_baseline_y);
        paint_line_at_baseline(painter, pos, placeholder, font, egui::Color32::GRAY);
    }
}

/// Lay out `text` as a single unwrapped row and place its glyph baseline
/// at `baseline_left.y` so the on-page preview shares the baked PDF's baseline.
fn paint_line_at_baseline(
    painter: &egui::Painter,
    baseline_left: egui::Pos2,
    text: &str,
    font: &egui::FontId,
    color: egui::Color32,
) {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), color);
    let baseline_offset = galley
        .rows
        .first()
        .and_then(|row| row.glyphs.first())
        .map_or_else(|| galley.size().y, |glyph| glyph.pos.y);
    painter.galley(
        egui::pos2(baseline_left.x, baseline_left.y - baseline_offset),
        galley,
        color,
    );
}

/// On-page box width in points: widest line plus padding. Text uses
/// the baked Helvetica metrics so the auto-fit frame matches the
/// saved PDF; signatures use egui's script-font metrics, as only the
/// preview renders that face.
fn measured_width_pt(
    ui: &egui::Ui,
    lines: &[String],
    font: &egui::FontId,
    zoom: f32,
    item: &OverlayText,
) -> f32 {
    let min_pt = match item.kind {
        BoxKind::Signature => 96.0,
        BoxKind::Text => 48.0,
    };
    let widest_pt = lines
        .iter()
        .map(|line| match item.kind {
            BoxKind::Text => crate::pdf::helvetica_width_pt(line, item.size_pt),
            BoxKind::Signature => line_width_pt(ui, line, font, zoom),
        })
        .fold(0.0_f32, f32::max);
    (widest_pt + 8.0).max(min_pt)
}

/// Extra visual box height (above, below) in points for script ascenders/descenders.
fn box_vertical_pad_pt(item: &OverlayText) -> (f32, f32) {
    match item.kind {
        BoxKind::Signature => (0.70 * item.size_pt, 0.50 * item.size_pt),
        BoxKind::Text => (0.0, 0.0),
    }
}

/// Preview font for a box: Momo Signature for signatures,
/// Helvetica-metric Liberation Sans for text (matching the baked
/// Helvetica's glyph widths, not just the wrap measure).
fn box_font(item: &OverlayText, zoom: f32) -> egui::FontId {
    match item.kind {
        BoxKind::Signature => crate::signature::font_id(item.size_pt * zoom),
        BoxKind::Text => crate::text_font::font_id(item.size_pt * zoom),
    }
}

/// Rendered width of one signature line in points, via egui's real
/// font metrics (text boxes use Helvetica metrics instead).
fn line_width_pt(ui: &egui::Ui, line: &str, font: &egui::FontId, zoom: f32) -> f32 {
    ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(line.to_string(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    }) / zoom
}

/// Lines as drawn: single flattened line for signatures, wrapped for
/// fixed-width text. Text wraps with the same Helvetica metrics as
/// the PDF baker (`pdf::box_operations`), so preview line breaks match the
/// saved file exactly and never depend on zoom or the UI font.
pub(super) fn box_display_lines(item: &OverlayText) -> Vec<String> {
    match item.kind {
        BoxKind::Signature => vec![item.signature_line()],
        BoxKind::Text => {
            let lines = overlay::logical_lines(&item.text);
            match item.width_pt {
                Some(width) => overlay::wrap_lines(&lines, width.max(24.0), &|line: &str| {
                    crate::pdf::helvetica_width_pt(line, item.size_pt)
                }),
                None => lines,
            }
        }
    }
}
