//! egui application shell: toolbar, sidebar inspector, page canvas.

use crate::overlay::{self, OverlayState};
use crate::pdf::PdfDoc;
use crate::render::PageRenderer;
use eframe::egui;
use std::path::PathBuf;
mod panels;
mod theme;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2F, 0x6F, 0xED);
/// Destructive-action fill (Delete box).
pub const DANGER: egui::Color32 = egui::Color32::from_rgb(0xD9, 0x34, 0x3B);
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Navigate,
    Type,
    Sign,
}

pub struct TypewriterApp {
    pdf_path: Option<PathBuf>,
    pdf_doc: Option<PdfDoc>,
    hayro_pdf: Option<hayro::hayro_syntax::Pdf>,
    renderer: PageRenderer,
    overlays: OverlayState,
    page: usize,
    zoom: f32,
    mode: Mode,
    status: String,
    focus_editor: bool,
    fit_pending: bool,
    /// Box currently being width-resized via its corner handle, if any.
    resize_box: Option<usize>,
}

impl TypewriterApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply_theme(&cc.egui_ctx);
        Self {
            pdf_path: None,
            pdf_doc: None,
            hayro_pdf: None,
            renderer: PageRenderer::default(),
            overlays: OverlayState::default(),
            page: 0,
            zoom: 1.0,
            mode: Mode::Navigate,
            status: "Open a PDF to begin.".to_string(),
            focus_editor: false,
            fit_pending: false,
            resize_box: None,
        }
    }

    fn page_count(&self) -> usize {
        self.pdf_doc.as_ref().map_or(0, PdfDoc::page_count)
    }

    fn load(&mut self, path: PathBuf) {
        let doc = match PdfDoc::open(&path) {
            Ok(doc) => doc,
            Err(err) => {
                self.status = format!("Could not open {}: {err:#}", path.display());
                return;
            }
        };
        let hayro_pdf = match hayro::hayro_syntax::Pdf::new(doc.bytes.clone()) {
            Ok(pdf) => pdf,
            Err(hayro::hayro_syntax::LoadPdfError::Decryption(_)) => {
                self.status = "Encrypted/password-protected PDFs are not supported.".to_string();
                return;
            }
            Err(hayro::hayro_syntax::LoadPdfError::Invalid) => {
                self.status = "Corrupt or unsupported PDF.".to_string();
                return;
            }
        };
        self.overlays.items.clear();
        self.overlays.selected = None;
        self.overlays.dirty = false;
        self.overlays.bump_generation();
        self.renderer.invalidate();
        self.page = 0;
        self.pdf_doc = Some(doc);
        self.hayro_pdf = Some(hayro_pdf);
        self.status = format!("Opened {}", path.display());
        self.pdf_path = Some(path);
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .pick_file()
        {
            self.load(path);
        }
    }

    fn save(&mut self, save_as: bool) {
        if self.pdf_doc.is_none() {
            return;
        }
        self.overlays.drop_empty_selected();
        let target = if save_as {
            let mut dialog = rfd::FileDialog::new().add_filter("PDF", &["pdf"]);
            if let Some(name) = self
                .pdf_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
            {
                dialog = dialog.set_file_name(name);
            }
            match dialog.save_file() {
                Some(path) => path,
                None => return,
            }
        } else {
            let Some(source) = self.pdf_path.clone() else {
                return;
            };
            let confirmed = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Overwrite PDF?")
                .set_description(format!(
                    "Save baked text into {}? This rewrites the file.",
                    source.display()
                ))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if confirmed != rfd::MessageDialogResult::Yes {
                return;
            }
            source
        };
        let result = match self.pdf_doc.as_mut() {
            Some(doc) => doc.bake_and_save(&mut self.overlays, &target),
            None => return,
        };
        match result {
            Ok(()) => {
                let message = format!("Saved {}", target.display());
                self.load(target);
                self.status = message;
            }
            Err(err) => {
                self.status = format!("Save failed: {err:#}");
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let (open, save, zoom_in, zoom_out, escape, delete) = ctx.input_mut(|i| {
            let ctrl = i.modifiers.ctrl || i.modifiers.command;
            (
                ctrl && i.consume_key(egui::Modifiers::CTRL, egui::Key::O),
                ctrl && i.consume_key(egui::Modifiers::CTRL, egui::Key::S),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Plus)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Equals),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Minus),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete),
            )
        });
        if open {
            self.open_dialog();
        }
        if save && self.overlays.dirty {
            self.save(false);
        }
        if zoom_in {
            self.zoom = (self.zoom * 1.25).clamp(MIN_ZOOM, MAX_ZOOM);
        }
        if zoom_out {
            self.zoom = (self.zoom / 1.25).clamp(MIN_ZOOM, MAX_ZOOM);
        }
        if escape {
            self.mode = Mode::Navigate;
            self.overlays.drop_empty_selected();
            self.overlays.selected = None;
        }
        if delete && let Some(sel) = self.overlays.selected {
            self.overlays.remove(sel);
        }
    }
}

impl eframe::App for TypewriterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);
        self.toolbar(ctx);
        self.status_bar(ctx);
        self.sidebar(ctx);
        self.canvas(ctx);
    }
}

impl TypewriterApp {
    fn canvas(&mut self, ctx: &egui::Context) {
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
            ui.label(egui::RichText::new("✎").size(56.0).color(ACCENT));
            ui.add_space(6.0);
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
                    .corner_radius(12),
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

    #[allow(clippy::cast_precision_loss)]
    fn draw_boxes(
        &mut self,
        ui: &mut egui::Ui,
        page_rect: egui::Rect,
        page: usize,
        zoom: f32,
        page_resp: &egui::Response,
    ) {
        let boxes: Vec<(usize, crate::overlay::OverlayText)> = self
            .overlays
            .by_page(page)
            .into_iter()
            .map(|(idx, item)| (idx, item.clone()))
            .collect();
        let mut box_rects: Vec<egui::Rect> = Vec::with_capacity(boxes.len());
        for (idx, item) in &boxes {
            let top_left = overlay::pdf_top_to_screen(item.x_pt, item.y_top_pt, page_rect, zoom);
            let font = box_font(item, zoom);
            let display_lines = box_display_lines(item);
            let line_count = item.text.lines().count().max(1) as f32;
            let auto_width_pt = measured_width_pt(ui, &display_lines, &font, zoom, item.kind);
            let width_pt = item.width_pt.map_or(auto_width_pt, |w| w.max(24.0));
            let height_pt = line_count * overlay::LEADING * item.size_pt;
            // The script font's glyphs rise ~1.35 em above and swash
            // ~0.9 em below the baseline, far outside the em box
            // (0.8/0.4 em). Pad the visual box so they are not clipped;
            // the baseline (and thus the baked PDF) is unaffected.
            let (pad_top_pt, pad_bottom_pt) = box_vertical_pad_pt(item);
            let rect = egui::Rect::from_min_max(
                egui::pos2(top_left.x, top_left.y - pad_top_pt * zoom),
                egui::pos2(
                    top_left.x + width_pt * zoom,
                    top_left.y + (height_pt + pad_bottom_pt) * zoom,
                ),
            );
            box_rects.push(rect);
            let selected = self.overlays.selected == Some(*idx);
            let handle_rect =
                egui::Rect::from_center_size(rect.right_bottom(), egui::vec2(16.0, 16.0));
            if selected {
                self.resize_handle(ui, *idx, handle_rect, width_pt, zoom);
            }
            let resp = ui.interact(
                rect,
                ui.id().with(("tw-box", idx)),
                egui::Sense::click_and_drag(),
            );
            if resp.clicked() {
                self.overlays.selected = Some(*idx);
            }
            if resp.dragged() && self.resize_box != Some(*idx) {
                self.overlays.selected = Some(*idx);
                let delta = resp.drag_delta();
                if let Some(current) = self.overlays.items.get_mut(*idx) {
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
            let color = egui::Color32::from_rgb(item.color[0], item.color[1], item.color[2]);
            paint_box_text(
                &painter,
                item,
                &display_lines,
                &font,
                color,
                rect,
                zoom,
                selected,
                pad_top_pt,
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
        // Click on empty page area creates a box in Type / Sign mode.
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
    item: &crate::overlay::OverlayText,
    display_lines: &[String],
    font: &egui::FontId,
    color: egui::Color32,
    rect: egui::Rect,
    zoom: f32,
    selected: bool,
    pad_top_pt: f32,
) {
    for (line_idx, line) in display_lines.iter().enumerate() {
        let y = rect.min.y
            + (pad_top_pt + overlay::ASCENT * item.size_pt) * zoom
            + (line_idx as f32) * overlay::LEADING * item.size_pt * zoom;
        painter.text(
            egui::pos2(rect.min.x, y),
            egui::Align2::LEFT_BOTTOM,
            line,
            font.clone(),
            color,
        );
    }
    if item.text.is_empty() && selected {
        painter.text(
            egui::pos2(
                rect.min.x,
                rect.min.y + (pad_top_pt + overlay::ASCENT * item.size_pt) * zoom,
            ),
            egui::Align2::LEFT_BOTTOM,
            match item.kind {
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
    let widest_px = lines
        .iter()
        .map(|line| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(line.clone(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0_f32, f32::max);
    (widest_px / zoom + 8.0).max(min_pt)
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

/// Lines as drawn: signatures are always a single flattened line.
fn box_display_lines(item: &crate::overlay::OverlayText) -> Vec<String> {
    match item.kind {
        crate::overlay::BoxKind::Signature => vec![item.signature_line()],
        crate::overlay::BoxKind::Text => item.text.lines().map(ToString::to_string).collect(),
    }
}
