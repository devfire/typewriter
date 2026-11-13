//! egui application shell: toolbar, sidebar inspector, page canvas.

use crate::overlay::OverlayState;
use crate::pdf::PdfDoc;
use crate::render::PageRenderer;
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
mod canvas;
mod close_prompt;
mod panels;
#[cfg(test)]
mod tests;
mod theme;
mod typewriter_icon;
mod widgets;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2F, 0x6F, 0xED);
/// Destructive-action fill (Delete box).
pub const DANGER: egui::Color32 = egui::Color32::from_rgb(0xD9, 0x34, 0x3B);
/// Height of every primary toolbar / sidebar / prompt button.
const BTN_H: f32 = 38.0;
/// Corner radius shared by buttons, pills, and cards.
const RADIUS: u8 = 10;
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;

/// Window-close lifecycle: `Prompting` while the unsaved-changes
/// modal is up, `Allowed` once the user saved or discarded so the
/// re-issued close request passes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloseState {
    Idle,
    Prompting,
    Allowed,
}

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
    close_state: CloseState,
    /// Per-box display layout cache, keyed by global box index and
    /// validated by `BoxLayoutKey`, so stale entries left behind by
    /// index shifts (removal) are recomputed rather than trusted.
    box_layouts: HashMap<usize, canvas::CachedBoxLayout>,
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
            close_state: CloseState::Idle,
            box_layouts: HashMap::new(),
        }
    }

    fn page_count(&self) -> usize {
        self.pdf_doc.as_ref().map_or(0, PdfDoc::page_count)
    }

    /// Load `path`, replacing the current document on success.
    /// Returns `false` (leaving the current document, page, and zoom
    /// untouched, with the reason in `status`) when the file cannot
    /// be opened or rendered.
    fn load(&mut self, path: PathBuf) -> bool {
        let doc = match PdfDoc::open(&path) {
            Ok(doc) => doc,
            Err(err) => {
                self.status = format!("Could not open {}: {err:#}", path.display());
                return false;
            }
        };
        // Shares the single `Arc<Vec<u8>>` allocation with `PdfDoc`.
        let hayro_pdf = match hayro::hayro_syntax::Pdf::new(std::sync::Arc::clone(&doc.bytes)) {
            Ok(pdf) => pdf,
            Err(hayro::hayro_syntax::LoadPdfError::Decryption(_)) => {
                self.status = "Encrypted/password-protected PDFs are not supported.".to_string();
                return false;
            }
            Err(hayro::hayro_syntax::LoadPdfError::Invalid) => {
                self.status = "Corrupt or unsupported PDF.".to_string();
                return false;
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
        true
    }

    /// Reload the just-saved `target`, restoring the page/zoom the
    /// user was viewing. A failed reload keeps `load`'s error in the
    /// status bar (prefixed with the successful save) instead of
    /// claiming a clean `Saved`.
    fn reload_after_save(&mut self, target: &Path) {
        let page = self.page;
        let zoom = self.zoom;
        if self.load(target.to_path_buf()) {
            self.page = page.min(self.page_count().saturating_sub(1));
            self.zoom = zoom;
            self.status = format!("Saved {}", target.display());
        } else {
            let reload_error = std::mem::take(&mut self.status);
            self.status = format!(
                "Saved {} but reload failed: {reload_error}",
                target.display()
            );
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .pick_file()
        {
            self.load(path);
        }
    }

    /// Bake overlays into the PDF. Returns `true` only when the file
    /// was actually written (dialog not cancelled, overwrite confirmed,
    /// write succeeded) — the close prompt relies on this to decide
    /// whether closing may proceed.
    fn save(&mut self, save_as: bool) -> bool {
        if self.pdf_doc.is_none() {
            return false;
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
                None => return false,
            }
        } else {
            let Some(source) = self.pdf_path.clone() else {
                return false;
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
                return false;
            }
            source
        };
        let result = match self.pdf_doc.as_mut() {
            Some(doc) => doc.bake_and_save(&mut self.overlays, &target),
            None => return false,
        };
        match result {
            Ok(()) => {
                self.reload_after_save(&target);
                true
            }
            Err(err) => {
                self.status = format!("Save failed: {err:#}");
                false
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // While the close prompt is up it owns the keyboard (Esc dismisses it).
        if self.close_state == CloseState::Prompting || ctx.wants_keyboard_input() {
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
        self.guard_close(ctx);
        self.handle_shortcuts(ctx);
        self.toolbar(ctx);
        self.status_bar(ctx);
        self.sidebar(ctx);
        self.canvas(ctx);
        self.close_prompt(ctx);
    }
}
