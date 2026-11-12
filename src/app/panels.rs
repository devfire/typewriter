//! Toolbar, status bar, and sidebar panels.

use super::{ACCENT, MAX_ZOOM, MIN_ZOOM, Mode, TypewriterApp};
use eframe::egui;

impl TypewriterApp {
    pub(super) fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open\u{2026}").clicked() {
                    self.open_dialog();
                }
                ui.separator();
                let count = self.page_count();
                if ui
                    .add_enabled(self.page > 0, egui::Button::new("<"))
                    .clicked()
                {
                    self.page = self.page.saturating_sub(1);
                }
                ui.label(if count == 0 {
                    "\u{2013} / \u{2013}".to_string()
                } else {
                    format!("{} / {}", self.page + 1, count)
                });
                if ui
                    .add_enabled(self.page + 1 < count, egui::Button::new(">"))
                    .clicked()
                {
                    self.page += 1;
                }
                ui.separator();
                if ui.button("\u{2212}").clicked() {
                    self.zoom = (self.zoom / 1.25).clamp(MIN_ZOOM, MAX_ZOOM);
                }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                if ui.button("+").clicked() {
                    self.zoom = (self.zoom * 1.25).clamp(MIN_ZOOM, MAX_ZOOM);
                }
                if ui
                    .add_enabled(count > 0, egui::Button::new("Fit"))
                    .clicked()
                {
                    self.fit_pending = true;
                }
                ui.separator();
                let type_btn = egui::Button::new("Type")
                    .fill(if self.mode == Mode::Type {
                        ACCENT
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .stroke(egui::Stroke::new(1.0_f32, ACCENT));
                if ui.add_enabled(count > 0, type_btn).clicked() {
                    self.mode = if self.mode == Mode::Type {
                        Mode::Navigate
                    } else {
                        Mode::Type
                    };
                }
                ui.separator();
                if ui
                    .add_enabled(self.overlays.dirty, egui::Button::new("Save"))
                    .clicked()
                {
                    self.save(false);
                }
                if ui
                    .add_enabled(count > 0, egui::Button::new("Save As\u{2026}"))
                    .clicked()
                {
                    self.save(true);
                }
            });
        });
    }

    pub(super) fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(path) = &self.pdf_path
                    && let Some(name) = path.file_name()
                {
                    ui.label(name.to_string_lossy().into_owned());
                    ui.separator();
                }
                if let Some(doc) = &self.pdf_doc
                    && let Some((w, h)) = doc.sizes_pt.get(self.page)
                {
                    ui.label(format!("{w:.0} \u{d7} {h:.0} pt"));
                    ui.separator();
                }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                ui.separator();
                ui.label(format!("{} box(es)", self.overlays.items.len()));
                if self.overlays.dirty {
                    ui.colored_label(ACCENT, "\u{25cf} unsaved");
                }
                ui.separator();
                ui.label(&self.status);
            });
        });
    }

    pub(super) fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .default_width(260.0)
            .show(ctx, |ui| self.sidebar_content(ui));
    }

    fn sidebar_content(&mut self, ui: &mut egui::Ui) {
        ui.heading("Text boxes (this page)");
        let mut select: Option<usize> = None;
        let mut remove: Option<usize> = None;
        for (idx, item) in self.overlays.by_page(self.page) {
            let first_line = item.text.lines().next().unwrap_or("(empty)");
            let label = format!("{first_line}  ({:.0} pt)", item.size_pt);
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.overlays.selected == Some(idx), label)
                    .clicked()
                {
                    select = Some(idx);
                }
                if ui.small_button("\u{d7}").clicked() {
                    remove = Some(idx);
                }
            });
        }
        if let Some(idx) = select {
            self.overlays.selected = Some(idx);
        }
        if let Some(idx) = remove {
            self.overlays.remove(idx);
        }
        ui.separator();
        ui.heading("Inspector");
        let Some(sel) = self.overlays.selected else {
            ui.label("Click the page in Type mode to add text.");
            return;
        };
        let Some(item) = self.overlays.items.get_mut(sel) else {
            self.overlays.selected = None;
            return;
        };
        let edit = ui.add(
            egui::TextEdit::multiline(&mut item.text)
                .hint_text("Type here\u{2026} (? replaces non-WinAnsi glyphs)")
                .desired_rows(4),
        );
        if self.focus_editor {
            edit.request_focus();
            self.focus_editor = false;
        }
        if edit.changed() {
            self.overlays.dirty = true;
        }
        let size_resp = ui.add(
            egui::Slider::new(&mut item.size_pt, 8.0..=36.0)
                .suffix(" pt")
                .text("Size"),
        );
        if size_resp.changed() {
            self.overlays.dirty = true;
        }
        ui.horizontal(|ui| {
            ui.label("Color");
            let color_resp =
                egui::widgets::color_picker::color_edit_button_srgb(ui, &mut item.color);
            if color_resp.changed() {
                self.overlays.dirty = true;
            }
        });
        if ui.button("Delete box").clicked() {
            self.overlays.remove(sel);
        }
    }
}
