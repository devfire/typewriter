//! Toolbar, status bar, and sidebar panels.
//!
//! Visual language: grouped, rounded cards on the toolbar, 38px-tall
//! buttons with 15px labels, and a card-based sidebar. Colors come from
//! `ui.visuals()` wherever possible so light and dark themes both work;
//! only the accent / danger fills are hard-coded brand colors.

use super::{ACCENT, DANGER, MAX_ZOOM, MIN_ZOOM, Mode, TypewriterApp};
use eframe::egui;

/// Height of every primary toolbar / sidebar button.
const BTN_H: f32 = 38.0;
/// Corner radius shared by buttons, pills, and cards.
const RADIUS: u8 = 10;

/// Large, easy-to-hit button with a 15px label.
fn big_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(label).size(15.0))
        .min_size(egui::vec2(0.0, BTN_H))
        .corner_radius(RADIUS)
}

/// Accent-filled call-to-action (Open, active mode, …).
fn primary_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(label)
            .size(15.0)
            .strong()
            .color(egui::Color32::WHITE),
    )
    .fill(ACCENT)
    .stroke(egui::Stroke::NONE)
    .min_size(egui::vec2(0.0, BTN_H))
    .corner_radius(RADIUS)
}

/// Square 38×38 icon button for paging / zooming.
fn icon_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(label).size(17.0))
        .min_size(egui::vec2(BTN_H, BTN_H))
        .corner_radius(RADIUS)
}

/// Rounded card grouping related controls.
fn group(ui: &mut egui::Ui) -> egui::Frame {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(12)
        .inner_margin(egui::Margin::same(5))
}

/// Read-only pill showing a value such as `Page 2 / 9` or `125%`.
fn pill(ui: &mut egui::Ui, text: String) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(14.0)
                    .monospace()
                    .color(ui.visuals().strong_text_color()),
            );
        });
}

/// Segmented Navigate/Type toggle: accent fill when active.
fn mode_button(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let text = egui::RichText::new(label).size(15.0);
    let button = if active {
        egui::Button::new(text.strong().color(egui::Color32::WHITE))
            .fill(ACCENT)
            .stroke(egui::Stroke::NONE)
    } else {
        egui::Button::new(text.color(ui.visuals().weak_text_color()))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
    };
    ui.add(
        button
            .min_size(egui::vec2(104.0, BTN_H))
            .corner_radius(RADIUS),
    )
}

/// Muted rounded hint card with a title and one-line explanation.
fn hint_card(ui: &mut egui::Ui, title: &str, body: &str) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(12)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(title).size(14.0).strong());
            ui.label(
                egui::RichText::new(body)
                    .size(13.0)
                    .color(ui.visuals().weak_text_color()),
            );
        });
}

impl TypewriterApp {
    pub(super) fn toolbar(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::new()
            .fill(ctx.style().visuals.panel_fill)
            .inner_margin(egui::Margin::symmetric(12, 10));
        egui::TopBottomPanel::top("toolbar")
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
                    ui.label(
                        egui::RichText::new("✎ Typewriter")
                            .size(19.0)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.label(
                        egui::RichText::new("PDF")
                            .size(19.0)
                            .strong()
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(2.0);
                    self.file_group(ui);
                    self.page_group(ui);
                    self.zoom_group(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.mode_group(ui);
                    });
                });
            });
    }

    fn file_group(&mut self, ui: &mut egui::Ui) {
        group(ui).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(primary_button("Open…"))
                    .on_hover_text("Open a PDF  (Ctrl+O)")
                    .clicked()
                {
                    self.open_dialog();
                }
                if ui
                    .add_enabled(self.overlays.dirty, big_button("Save"))
                    .on_hover_text("Bake text into the PDF  (Ctrl+S)")
                    .clicked()
                {
                    self.save(false);
                }
                if ui
                    .add_enabled(self.page_count() > 0, big_button("Save As…"))
                    .clicked()
                {
                    self.save(true);
                }
            });
        });
    }

    fn page_group(&mut self, ui: &mut egui::Ui) {
        let count = self.page_count();
        group(ui).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.page > 0, icon_button("◀"))
                    .on_hover_text("Previous page")
                    .clicked()
                {
                    self.page = self.page.saturating_sub(1);
                }
                pill(
                    ui,
                    if count == 0 {
                        "– / –".to_string()
                    } else {
                        format!("{} / {}", self.page + 1, count)
                    },
                );
                if ui
                    .add_enabled(self.page + 1 < count, icon_button("▶"))
                    .on_hover_text("Next page")
                    .clicked()
                {
                    self.page += 1;
                }
            });
        });
    }

    fn zoom_group(&mut self, ui: &mut egui::Ui) {
        group(ui).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(icon_button("−"))
                    .on_hover_text("Zoom out  (−)")
                    .clicked()
                {
                    self.zoom = (self.zoom / 1.25).clamp(MIN_ZOOM, MAX_ZOOM);
                }
                pill(ui, format!("{:.0}%", self.zoom * 100.0));
                if ui
                    .add(icon_button("+"))
                    .on_hover_text("Zoom in  (+)")
                    .clicked()
                {
                    self.zoom = (self.zoom * 1.25).clamp(MIN_ZOOM, MAX_ZOOM);
                }
                if ui
                    .add_enabled(self.page_count() > 0, big_button("Fit"))
                    .on_hover_text("Fit page to window")
                    .clicked()
                {
                    self.fit_pending = true;
                }
            });
        });
    }

    fn mode_group(&mut self, ui: &mut egui::Ui) {
        group(ui).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if mode_button(ui, "✎  Type", self.mode == Mode::Type)
                    .on_hover_text("Click the page to add text")
                    .clicked()
                {
                    self.mode = Mode::Type;
                }
                if mode_button(ui, "✦  Navigate", self.mode == Mode::Navigate)
                    .on_hover_text("Drag to pan, click a box to select it")
                    .clicked()
                {
                    self.mode = Mode::Navigate;
                }
            });
        });
    }

    pub(super) fn status_bar(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::new()
            .fill(ctx.style().visuals.panel_fill)
            .inner_margin(egui::Margin::symmetric(12, 7));
        egui::TopBottomPanel::bottom("status")
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(10.0, 6.0);
                    if let Some(path) = &self.pdf_path
                        && let Some(name) = path.file_name()
                    {
                        ui.label(
                            egui::RichText::new(name.to_string_lossy().into_owned())
                                .size(13.5)
                                .strong(),
                        );
                    }
                    if let Some(doc) = &self.pdf_doc
                        && let Some((w, h)) = doc.sizes_pt.get(self.page)
                    {
                        pill(ui, format!("{w:.0} × {h:.0} pt"));
                    }
                    pill(ui, format!("{:.0}%", self.zoom * 100.0));
                    pill(ui, format!("{} box(es)", self.overlays.items.len()));
                    if self.overlays.dirty {
                        ui.label(
                            egui::RichText::new("● Unsaved changes")
                                .size(13.5)
                                .strong()
                                .color(ui.visuals().warn_fg_color),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(&self.status)
                                .size(13.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                });
            });
    }

    pub(super) fn sidebar(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::new()
            .fill(ctx.style().visuals.panel_fill)
            .inner_margin(egui::Margin::same(14));
        egui::SidePanel::left("sidebar")
            .default_width(300.0)
            .min_width(250.0)
            .max_width(420.0)
            .resizable(true)
            .frame(frame)
            .show(ctx, |ui| self.sidebar_content(ui));
    }

    fn sidebar_content(&mut self, ui: &mut egui::Ui) {
        self.box_list(ui);
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Inspector").size(18.0).strong());
        ui.add_space(8.0);
        self.inspector(ui);
    }

    fn box_list(&mut self, ui: &mut egui::Ui) {
        let page_items = self.overlays.by_page(self.page);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Text boxes").size(18.0).strong());
            pill(ui, format!("{}", page_items.len()));
        });
        ui.label(
            egui::RichText::new("On this page · click one to edit it below")
                .size(12.5)
                .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(8.0);
        if page_items.is_empty() {
            hint_card(
                ui,
                "No text here yet.",
                "Switch to Type mode and click anywhere on the page.",
            );
        }
        let mut select: Option<usize> = None;
        let mut remove: Option<usize> = None;
        for (idx, item) in &page_items {
            let selected = self.overlays.selected == Some(*idx);
            let first_line: String = item
                .text
                .lines()
                .next()
                .unwrap_or("(empty)")
                .chars()
                .take(26)
                .collect();
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .stroke(egui::Stroke::new(
                    if selected { 1.5_f32 } else { 1.0_f32 },
                    if selected {
                        ACCENT
                    } else {
                        ui.visuals().widgets.noninteractive.bg_stroke.color
                    },
                ))
                .corner_radius(RADIUS)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let label = format!("{first_line}  ·  {:.0} pt", item.size_pt);
                        if ui
                            .add_sized(
                                egui::vec2((ui.available_width() - 44.0).max(80.0), 34.0),
                                egui::Button::selectable(
                                    selected,
                                    egui::RichText::new(label).size(14.0),
                                )
                                .corner_radius(8),
                            )
                            .clicked()
                        {
                            select = Some(*idx);
                        }
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("×").size(16.0))
                                    .min_size(egui::vec2(34.0, 34.0))
                                    .corner_radius(8),
                            )
                            .on_hover_text("Delete this text box")
                            .clicked()
                        {
                            remove = Some(*idx);
                        }
                    });
                });
            ui.add_space(6.0);
        }
        if let Some(idx) = select {
            self.overlays.selected = Some(idx);
        }
        if let Some(idx) = remove {
            self.overlays.remove(idx);
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = self.overlays.selected else {
            hint_card(
                ui,
                "Nothing selected.",
                "Click a text box — or click the page in Type mode to add one.",
            );
            return;
        };
        let Some(item) = self.overlays.items.get_mut(sel) else {
            self.overlays.selected = None;
            return;
        };
        let mut changed = false;
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(12)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Text").size(13.0).strong());
                ui.add_space(4.0);
                let edit = ui.add_sized(
                    egui::vec2(ui.available_width(), 110.0),
                    egui::TextEdit::multiline(&mut item.text)
                        .hint_text("Type here… (? replaces non-WinAnsi glyphs)")
                        .desired_rows(5),
                );
                if self.focus_editor {
                    edit.request_focus();
                    self.focus_editor = false;
                }
                changed |= edit.changed();
                ui.add_space(10.0);
                let size_resp = ui.add(
                    egui::Slider::new(&mut item.size_pt, 8.0..=36.0)
                        .suffix(" pt")
                        .text("Size"),
                );
                changed |= size_resp.changed();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Color").size(13.5));
                    let color_resp =
                        egui::widgets::color_picker::color_edit_button_srgb(ui, &mut item.color);
                    changed |= color_resp.changed();
                });
            });
        if changed {
            self.overlays.dirty = true;
        }
        ui.add_space(12.0);
        if ui
            .add_sized(
                egui::vec2(ui.available_width(), BTN_H),
                egui::Button::new(
                    egui::RichText::new("Delete box")
                        .size(15.0)
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(DANGER)
                .stroke(egui::Stroke::NONE)
                .corner_radius(RADIUS),
            )
            .clicked()
        {
            self.overlays.remove(sel);
        }
    }
}
