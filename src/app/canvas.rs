//! Page canvas: rendered page, overlay boxes, and box layout cache.

use super::{ACCENT, MAX_ZOOM, MIN_ZOOM, Mode, RADIUS, TypewriterApp, typewriter_icon};
use crate::overlay;
use crate::render::PageRenderer;
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
    kind: crate::overlay::BoxKind,
    width_pt_bits: Option<u32>,
    zoom_bits: u32,
}

/// Laid-out display lines and measured auto width for one box,
/// reused across frames until its `BoxLayoutKey` changes. Sharing
/// the lines behind an `Arc` makes a cache hit allocation-free
/// apart from the `Arc` bump.
pub(super) struct CachedBoxLayout {
    key: BoxLayoutKey,
    lines: Arc<[String]>,
    auto_width_pt: f32,
}

fn box_layout_key(item: &crate::overlay::OverlayText, zoom: f32) -> BoxLayoutKey {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    item.text.hash(&mut hasher);
    BoxLayoutKey {
        text_hash: hasher.finish(),
        size_pt_bits: item.size_pt.to_bits(),
        kind: item.kind,
        width_pt_bits: item.width_pt.map(f32::to_bits),
        zoom_bits: zoom.to_bits(),
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
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("Open a PDF…")
                            .size(16.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(ACCENT)
                    .stroke(egui::Stroke::NONE)
                    .min_size(egui::vec2(200.0, 48.0))
                    .corner_radius(RADIUS),
                )
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
        let (dim_w, dim_h) = PageRenderer::page_points(pdf, page)
            .or_else(|| doc.sizes_pt.get(page).copied())
            .unwrap_or((612.0, 792.0));
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
        let generation = self.overlays.doc_generation;
        let texture = match self.renderer.render_page(ctx, pdf, page, zoom, generation) {
            Ok(texture) => Some(texture),
            Err(err) => {
                self.status = format!("Render failed: {err:#}");
                None
            }
        };
        let mode = self.mode;
        egui::ScrollArea::both()
            .scroll_source(egui::containers::scroll_area::ScrollSource {
                drag: mode == Mode::Navigate,
                ..Default::default()
            })
            .show(ui, |ui| {
                let page_size = egui::vec2(dim_w * zoom, dim_h * zoom);
                let (page_rect, page_resp) =
                    ui.allocate_exact_size(page_size, egui::Sense::click_and_drag());
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
    }

    /// Display lines and measured auto width for box `idx`, served
    /// from `box_layouts` when text, size, kind, fixed width, and
    /// zoom are unchanged, so the steady-state frame path performs
    /// no glyph layout and no `String`/`Vec<String>` cloning. The
    /// expensive layout runs only on a cache miss.
    fn box_layout(&mut self, ui: &egui::Ui, idx: usize, zoom: f32) -> Option<(Arc<[String]>, f32)> {
        let item = self.overlays.items.get(idx)?;
        let key = box_layout_key(item, zoom);
        if let Some(cached) = self.box_layouts.get(&idx)
            && cached.key == key
        {
            return Some((Arc::clone(&cached.lines), cached.auto_width_pt));
        }
        let font = box_font(item, zoom);
        let lines = box_display_lines(ui, item, &font, zoom);
        let auto_width_pt = measured_width_pt(ui, &lines, &font, zoom, item.kind);
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
    fn draw_boxes(
        &mut self,
        ui: &mut egui::Ui,
        page_rect: egui::Rect,
        page: usize,
        zoom: f32,
        page_resp: &egui::Response,
    ) {
        self.box_layouts
            .retain(|idx, _| *idx < self.overlays.items.len());
        // Indices only: boxes are never cloned. Each box's scalar
        // fields are copied out below before any `&mut self` call,
        // and its lines/width come from the layout cache.
        let box_indices: Vec<usize> = self
            .overlays
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.page == page)
            .map(|(idx, _)| idx)
            .collect();
        let mut box_rects: Vec<egui::Rect> = Vec::with_capacity(box_indices.len());
        for idx in box_indices {
            let Some((display_lines, auto_width_pt)) = self.box_layout(ui, idx, zoom) else {
                continue;
            };
            let Some(item) = self.overlays.items.get(idx) else {
                continue;
            };
            let font = box_font(item, zoom);
            let top_left = overlay::pdf_top_to_screen(item.x_pt, item.y_top_pt, page_rect, zoom);
            let line_count = display_lines.len().max(1) as f32;
            let width_pt = item.width_pt.map_or(auto_width_pt, |w| w.max(24.0));
            let height_pt = line_count * overlay::LEADING * item.size_pt;
            // The script font's glyphs rise ~1.35 em above and swash
            // ~0.9 em below the baseline, far outside the em box
            // (0.8/0.4 em). Pad the visual box so they are not clipped;
            // the baseline (and thus the baked PDF) is unaffected.
            let (pad_top_pt, pad_bottom_pt) = box_vertical_pad_pt(item);
            let size_pt = item.size_pt;
            let kind = item.kind;
            let text_is_empty = item.text.is_empty();
            let color = egui::Color32::from_rgb(item.color[0], item.color[1], item.color[2]);
            let rect = egui::Rect::from_min_max(
                egui::pos2(top_left.x, top_left.y - pad_top_pt * zoom),
                egui::pos2(
                    top_left.x + width_pt * zoom,
                    top_left.y + (height_pt + pad_bottom_pt) * zoom,
                ),
            );
            box_rects.push(rect);
            let selected = self.overlays.selected == Some(idx);
            let handle_rect =
                egui::Rect::from_center_size(rect.right_bottom(), egui::vec2(16.0, 16.0));
            if selected {
                self.resize_handle(ui, idx, handle_rect, width_pt, zoom);
            }
            let resp = ui.interact(
                rect,
                ui.id().with(("tw-box", idx)),
                egui::Sense::click_and_drag(),
            );
            if resp.clicked() {
                self.overlays.selected = Some(idx);
            }
            if resp.dragged() && self.resize_box != Some(idx) {
                self.overlays.selected = Some(idx);
                let delta = resp.drag_delta();
                if let Some(current) = self.overlays.items.get_mut(idx) {
                    current.x_pt = (current.x_pt + delta.x / zoom).max(0.0);
                    current.y_top_pt = (current.y_top_pt + delta.y / zoom).max(0.0);
                }
                self.overlays.dirty = true;
            }
            let painter = ui.painter_at(rect.expand(4.0));
            if selected {
                painter.rect_filled(rect, 2.0, egui::Color32::from_white_alpha(170));
                painter.rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.5_f32, ACCENT),
                    egui::StrokeKind::Outside,
                );
            }
            paint_box_text(
                &painter,
                &display_lines,
                &font,
                color,
                rect,
                zoom,
                selected,
                pad_top_pt,
                size_pt,
                kind,
                text_is_empty,
            );
            if selected {
                painter.rect_filled(handle_rect, 3.0, ACCENT);
                painter.rect_stroke(
                    handle_rect,
                    3.0,
                    egui::Stroke::new(1.5_f32, egui::Color32::WHITE),
                    egui::StrokeKind::Outside,
                );
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
            Mode::Type => Some(crate::overlay::BoxKind::Text),
            Mode::Sign => Some(crate::overlay::BoxKind::Signature),
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

    /// Bottom-right corner handle of a selected box: drag to set a
    /// fixed box width, double-click to return to auto-fit.
    fn resize_handle(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        handle_rect: egui::Rect,
        width_pt: f32,
        zoom: f32,
    ) {
        let resize_resp = ui
            .interact(
                handle_rect,
                ui.id().with(("tw-resize", idx)),
                egui::Sense::drag(),
            )
            .on_hover_text("Drag to widen/narrow the box \u{00b7} double-click for auto-fit");
        if resize_resp.double_clicked() {
            if let Some(current) = self.overlays.items.get_mut(idx) {
                current.width_pt = None;
            }
            self.overlays.dirty = true;
        }
        if resize_resp.drag_started() {
            self.resize_box = Some(idx);
        }
        if resize_resp.hovered() || resize_resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if self.resize_box == Some(idx) && resize_resp.dragged() {
            let delta_pt = resize_resp.drag_delta().x / zoom;
            if let Some(current) = self.overlays.items.get_mut(idx) {
                let base = current.width_pt.unwrap_or(width_pt);
                current.width_pt = Some((base + delta_pt).max(24.0));
            }
            self.overlays.dirty = true;
        }
        if resize_resp.drag_stopped() {
            self.resize_box = None;
        }
    }
}

/// Draw a box's lines (plus the empty-box placeholder when selected)
/// inside `rect`.
#[allow(clippy::too_many_arguments, clippy::cast_precision_loss)]
fn paint_box_text(
    painter: &egui::Painter,
    display_lines: &[String],
    font: &egui::FontId,
    color: egui::Color32,
    rect: egui::Rect,
    zoom: f32,
    selected: bool,
    pad_top_pt: f32,
    size_pt: f32,
    kind: crate::overlay::BoxKind,
    text_is_empty: bool,
) {
    for (line_idx, line) in display_lines.iter().enumerate() {
        let y = rect.min.y
            + (pad_top_pt + overlay::ASCENT * size_pt) * zoom
            + (line_idx as f32) * overlay::LEADING * size_pt * zoom;
        painter.text(
            egui::pos2(rect.min.x, y),
            egui::Align2::LEFT_BOTTOM,
            line,
            font.clone(),
            color,
        );
    }
    if text_is_empty && selected {
        painter.text(
            egui::pos2(
                rect.min.x,
                rect.min.y + (pad_top_pt + overlay::ASCENT * size_pt) * zoom,
            ),
            egui::Align2::LEFT_BOTTOM,
            match kind {
                crate::overlay::BoxKind::Signature => "Sign here\u{2026}",
                crate::overlay::BoxKind::Text => "Type here\u{2026}",
            },
            font.clone(),
            egui::Color32::GRAY,
        );
    }
}

/// On-page box width in points: widest line as actually rendered by
/// egui (real font metrics, not a per-character guess), plus padding.
fn measured_width_pt(
    ui: &egui::Ui,
    lines: &[String],
    font: &egui::FontId,
    zoom: f32,
    kind: crate::overlay::BoxKind,
) -> f32 {
    let min_pt = match kind {
        crate::overlay::BoxKind::Signature => 96.0,
        crate::overlay::BoxKind::Text => 48.0,
    };
    let widest_pt = lines
        .iter()
        .map(|line| line_width_pt(ui, line, font, zoom))
        .fold(0.0_f32, f32::max);
    (widest_pt + 8.0).max(min_pt)
}

/// Extra visual box height (above, below) in points, beyond the em
/// box. Signatures need it for the script font's tall ascenders and
/// swash descenders; ordinary text fits inside its em box.
fn box_vertical_pad_pt(item: &crate::overlay::OverlayText) -> (f32, f32) {
    match item.kind {
        crate::overlay::BoxKind::Signature => (0.70 * item.size_pt, 0.50 * item.size_pt),
        crate::overlay::BoxKind::Text => (0.0, 0.0),
    }
}

/// Preview font for a box: Momo Signature for signatures, proportional else.
fn box_font(item: &crate::overlay::OverlayText, zoom: f32) -> egui::FontId {
    match item.kind {
        crate::overlay::BoxKind::Signature => crate::signature::font_id(item.size_pt * zoom),
        crate::overlay::BoxKind::Text => egui::FontId::proportional(item.size_pt * zoom),
    }
}

/// Rendered width of one line in points, via egui's real font metrics.
fn line_width_pt(ui: &egui::Ui, line: &str, font: &egui::FontId, zoom: f32) -> f32 {
    ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(line.to_string(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    }) / zoom
}

/// Lines as drawn: signatures are always a single flattened line;
/// fixed-width text boxes wrap at their width, as the baker wraps
/// them (each side with its own font's metrics).
fn box_display_lines(
    ui: &egui::Ui,
    item: &crate::overlay::OverlayText,
    font: &egui::FontId,
    zoom: f32,
) -> Vec<String> {
    match item.kind {
        crate::overlay::BoxKind::Signature => vec![item.signature_line()],
        crate::overlay::BoxKind::Text => {
            let lines: Vec<String> = item.text.lines().map(ToString::to_string).collect();
            match item.width_pt {
                Some(width) => {
                    crate::overlay::wrap_lines(&lines, width.max(24.0), &|line: &str| {
                        line_width_pt(ui, line, font, zoom)
                    })
                }
                None => lines,
            }
        }
    }
}
