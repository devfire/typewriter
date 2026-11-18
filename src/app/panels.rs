//! Toolbar, status bar, and sidebar panels.
//!
//! Visual language: grouped, rounded cards on the toolbar, 38px-tall
//! buttons with 15px labels, and a card-based sidebar. Colors come from
//! `ui.visuals()` wherever possible so light and dark themes both work;
//! only the accent / danger fills are hard-coded brand colors.

use super::widgets::{
    big_button, box_width_control, group, hint_card, icon_button, mode_button, pill, primary_button,
};
use super::{
    ACCENT, BTN_H, DANGER, MAX_ZOOM, MIN_ZOOM, Mode, RADIUS, SaveMode, TypewriterApp, box_label,
};
use crate::overlay::{BoxId, BoxKind, OverlayText};
use eframe::egui;

/// Idle gap, in egui-time seconds, that ends an inspector edit
/// burst: typing paused this long starts a new undo step. Matches
/// the grouping `egui::util::undoer::Undoer` applies with its
/// default `stable_time`, without replacing the snapshot history
/// that canvas drags and discrete actions also use.
const INSPECTOR_GESTURE_IDLE_SECS: f64 = 1.0;

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
                    self.open_dialog(ui.ctx());
                }
                if ui
                    .add_enabled(self.overlays.is_dirty(), big_button("Save"))
                    .on_hover_text("Bake text into the PDF  (Ctrl+S)")
                    .clicked()
                {
                    self.save(SaveMode::InPlace);
                }
                if ui
                    .add_enabled(self.page_count() > 0, big_button("Save As…"))
                    .clicked()
                {
                    self.save_as_dialog(ui.ctx());
                }
            });
        });
    }

    /// Move to `page`, clearing the selection through
    /// [`crate::overlay::OverlayState::select`] (which also drops a selected
    /// empty box): a selection must never survive onto a page
    /// where its box is invisible, or Delete/Duplicate would act
    /// on a box the user cannot see. No-op on the current page.
    pub(super) fn change_page(&mut self, page: usize) {
        if page == self.page {
            return;
        }
        self.overlays.select(None);
        self.page = page;
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
                    self.change_page(self.page.saturating_sub(1));
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
                    self.change_page(self.page + 1);
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
                if mode_button(ui, "✍  Sign", self.mode == Mode::Sign)
                    .on_hover_text("Click the page to place a signature")
                    .clicked()
                {
                    self.mode = Mode::Sign;
                }
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
                        && let Some(geom) = doc.pages.get(self.page)
                    {
                        let (w, h) = geom.display_size();
                        pill(ui, format!("{w:.0} × {h:.0} pt"));
                    }
                    pill(ui, format!("{:.0}%", self.zoom * 100.0));
                    pill(ui, format!("{} box(es)", self.overlays.items.len()));
                    if self.overlays.is_dirty() {
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
        // Inspector (incl. signature Size slider) pinned to the bottom:
        // a long box list must not push it below the window.
        egui::TopBottomPanel::bottom("sidebar_inspector")
            .resizable(false)
            .frame(egui::Frame::new())
            .show_inside(ui, |ui| {
                ui.add_space(10.0);
                self.edit_actions(ui);
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Inspector").size(18.0).strong());
                ui.add_space(8.0);
                self.inspector(ui);
            });
        self.box_list(ui);
    }

    fn box_list(&mut self, ui: &mut egui::Ui) {
        let page_items = self.overlays.by_page(self.page);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Boxes").size(18.0).strong());
            pill(ui, format!("{}", page_items.len()));
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new("On this page · click one to edit it below")
                    .size(12.5)
                    .color(ui.visuals().weak_text_color()),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        if page_items.is_empty() {
            hint_card(
                ui,
                "Nothing here yet.",
                "Switch to Type or Sign mode and click anywhere on the page.",
            );
        }
        let mut select: Option<crate::overlay::BoxId> = None;
        let mut remove: Option<crate::overlay::BoxId> = None;
        // Only the entries scroll; header and pinned inspector stay on screen.
        let list_width = ui.available_width();
        egui::ScrollArea::vertical()
            .id_salt("box_list")
            .max_width(list_width)
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                for item in &page_items {
                    self.box_list_row(ui, item, &mut select, &mut remove);
                }
            });
        if let Some(idx) = select {
            self.overlays.select(Some(idx));
        }
        if let Some(idx) = remove {
            self.overlays.remove(idx);
        }
    }

    /// One box-list entry: selectable label plus delete button.
    /// Clicks are reported via `select`/`remove` so the caller can
    /// mutate `overlays` after the (borrowing) list pass.
    fn box_list_row(
        &self,
        ui: &mut egui::Ui,
        item: &OverlayText,
        select: &mut Option<BoxId>,
        remove: &mut Option<BoxId>,
    ) {
        let selected = self.overlays.selected == Some(item.id);
        let first_line: String = item
            .text
            .lines()
            .next()
            .unwrap_or("(empty)")
            .chars()
            .take(26)
            .collect();
        let row_inner_width = (ui.available_width() - 24.0).max(0.0);
        let stroke = if selected {
            egui::Stroke::new(1.5_f32, ACCENT)
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke
        };
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(stroke)
            .corner_radius(RADIUS)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(row_inner_width);
                ui.horizontal(|ui| {
                    let marker = match item.kind {
                        BoxKind::Signature => "✍ ",
                        BoxKind::Text => "",
                    };
                    let label = format!("{marker}{first_line}  ·  {:.0} pt", item.size_pt);
                    let spacing = ui.spacing().item_spacing.x;
                    let btn_w = (ui.available_width() - 34.0 - spacing).max(0.0);
                    let select_resp = ui.allocate_ui_with_layout(
                        egui::vec2(btn_w, 34.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_max_width(btn_w);
                            let btn = egui::Button::selectable(
                                selected,
                                egui::RichText::new(label).size(14.0),
                            )
                            .min_size(egui::vec2(btn_w, 34.0))
                            .truncate()
                            .corner_radius(RADIUS);
                            ui.add(btn)
                        },
                    );
                    if select_resp.inner.clicked() {
                        *select = Some(item.id);
                    }
                    let del_btn = egui::Button::new(egui::RichText::new("×").size(16.0))
                        .min_size(egui::vec2(34.0, 34.0))
                        .corner_radius(RADIUS);
                    if ui
                        .add(del_btn)
                        .on_hover_text(format!("Delete this {}", box_label(item.kind)))
                        .clicked()
                    {
                        *remove = Some(item.id);
                    }
                });
            });
        ui.add_space(6.0);
    }

    /// Copy / Cut / Paste / Duplicate for the selected box (text or
    /// signature). Always visible — unlike the inspector — so Paste
    /// stays reachable after Cut has cleared the selection.
    fn edit_actions(&mut self, ui: &mut egui::Ui) {
        let has_selection = self.overlays.selected.is_some();
        let can_paste = self.clipboard.is_some() && self.page_count() > 0;
        let half = (ui.available_width() - ui.spacing().item_spacing.x).max(0.0) / 2.0;
        let btn = |ui: &mut egui::Ui, enabled: bool, label: &str, tip: &str| {
            ui.add_enabled(enabled, big_button(label).min_size(egui::vec2(half, BTN_H)))
                .on_hover_text(tip)
                .clicked()
        };
        ui.horizontal(|ui| {
            if btn(ui, has_selection, "Copy", "Copy selected box  (Ctrl+C)") {
                self.copy_selected();
                self.seed_system_clipboard(ui.ctx());
            }
            if btn(ui, has_selection, "Cut", "Cut selected box  (Ctrl+X)") {
                self.cut_selected();
                self.seed_system_clipboard(ui.ctx());
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if btn(ui, can_paste, "Paste", "Paste box onto this page  (Ctrl+V)") {
                self.paste_clipboard();
            }
            if btn(
                ui,
                has_selection,
                "Duplicate",
                "Duplicate selected box  (Ctrl+D)",
            ) {
                self.duplicate_selected();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if btn(
                ui,
                self.overlays.can_undo(),
                "Undo",
                "Undo last edit  (Ctrl+Z)",
            ) {
                self.undo();
            }
            if btn(
                ui,
                self.overlays.can_redo(),
                "Redo",
                "Redo undone edit  (Ctrl+Shift+Z)",
            ) {
                self.redo();
            }
        });
    }

    /// Close any open inspector gesture, recording it as one undo
    /// step if it changed anything.
    fn end_inspector_gesture(&mut self) {
        self.overlays.end_gesture();
        self.inspector_gesture = None;
    }

    /// Fold one inspector frame into undo history. One gesture per
    /// continuous edit (typing burst, slider or color drag): the
    /// first changed frame opens it with the pre-edit snapshot.
    /// It closes only when a control loses focus, when the edited
    /// box changes, or after `INSPECTOR_GESTURE_IDLE_SECS` with no
    /// change — never on a merely unchanged frame, because
    /// egui-winit repaints on every key *release*, so each keystroke
    /// is followed by an unchanged frame that used to split the
    /// burst into one undo step per key. A held pointer suppresses
    /// the idle close: it may just be paused mid-drag.
    fn record_inspector_edit(
        &mut self,
        before: Vec<OverlayText>,
        selected: BoxId,
        changed: bool,
        lost_focus: bool,
        pointer_down: bool,
        now: f64,
    ) {
        if let Some((gesture_box, last_change)) = self.inspector_gesture
            && (gesture_box != selected
                || (!pointer_down && now - last_change >= INSPECTOR_GESTURE_IDLE_SECS))
        {
            self.end_inspector_gesture();
        }
        if changed {
            self.overlays.begin_gesture_with(before);
            self.inspector_gesture = Some((selected, now));
        }
        if lost_focus {
            self.end_inspector_gesture();
        }
    }

    /// Record one inspector frame (see `record_inspector_edit`) and,
    /// while a burst is open with the pointer up, wake once it has
    /// been idle long enough to close — even if no further input
    /// arrives in the meantime.
    fn finish_inspector_edit(
        &mut self,
        ui: &egui::Ui,
        before: Vec<OverlayText>,
        selected: BoxId,
        changed: bool,
        lost_focus: bool,
    ) {
        let pointer_down = ui.input(|i| i.pointer.any_down());
        let now = ui.input(|i| i.time);
        self.record_inspector_edit(before, selected, changed, lost_focus, pointer_down, now);
        if self.inspector_gesture.is_some() && !pointer_down {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    INSPECTOR_GESTURE_IDLE_SECS,
                ));
        }
    }

    /// "Delete box" button below the inspector controls.
    fn delete_selected_button(&mut self, ui: &mut egui::Ui, sel: BoxId) {
        ui.add_space(12.0);
        let delete_label = egui::RichText::new("Delete box")
            .size(15.0)
            .strong()
            .color(egui::Color32::WHITE);
        let delete_btn = egui::Button::new(delete_label)
            .min_size(egui::vec2(ui.available_width(), BTN_H))
            .fill(DANGER)
            .stroke(egui::Stroke::NONE)
            .corner_radius(RADIUS);
        if ui.add(delete_btn).clicked() {
            self.overlays.remove(sel);
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        // One-shot: placement sets this so the new box's editor grabs
        // keyboard focus on the next frame; never retain it past here.
        let focus_editor = std::mem::take(&mut self.focus_editor);
        let Some(sel) = self.overlays.selected else {
            // Selection gone: any inspector burst is complete.
            self.end_inspector_gesture();
            hint_card(
                ui,
                "Nothing selected.",
                "Click a box — or click the page in Type/Sign mode to add one.",
            );
            return;
        };
        // Pre-edit snapshot for undo: the controls below mutate the
        // box in place, so history needs the state from before them.
        // `items` is small; cloning it per inspector frame is cheap.
        let before = self.overlays.items.clone();
        let Some(item) = self.overlays.item_mut(sel) else {
            self.end_inspector_gesture();
            self.overlays.select(None);
            return;
        };
        let mut changed = false;
        let mut lost_focus = false;
        let inner_width = (ui.available_width() - 26.0).max(0.0);
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(RADIUS)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(inner_width);
                let is_signature = item.kind == BoxKind::Signature;
                ui.label(
                    egui::RichText::new(if is_signature { "Signature" } else { "Text" })
                        .size(13.0)
                        .strong(),
                );
                ui.add_space(4.0);
                // Box-scoped editor ID: egui persists `TextEditState`
                // (including its native undo history) under the widget
                // ID. Without a per-box salt, switching the selection
                // reuses the same auto-ID at this position, so Ctrl+Z
                // in box B can restore box A's text. `BoxId` is
                // session-unique and never reused, so it isolates both
                // the text and signature editors across boxes and
                // documents; app-level whole-box undo stays in
                // `OverlayState` history and is unaffected.
                let (editor, edit_h) = if is_signature {
                    (
                        egui::TextEdit::singleline(&mut item.text)
                            .hint_text("Type your name…")
                            .id_salt(("box-text", sel)),
                        44.0,
                    )
                } else {
                    (
                        egui::TextEdit::multiline(&mut item.text)
                            .hint_text("Type here… (? replaces non-WinAnsi glyphs)")
                            .desired_rows(5)
                            .id_salt(("box-text", sel)),
                        110.0,
                    )
                };
                let edit = ui.add_sized(
                    egui::vec2(ui.available_width(), edit_h),
                    editor.desired_width(f32::INFINITY),
                );
                if focus_editor {
                    edit.request_focus();
                }
                if is_signature {
                    ui.add_space(8.0);
                    let ink = egui::Color32::from_rgb(item.color[0], item.color[1], item.color[2]);
                    crate::signature::draw_preview(
                        ui,
                        &item.signature_line(),
                        ink,
                        item.text.trim().is_empty(),
                    );
                }
                changed |= edit.changed();
                lost_focus |= edit.lost_focus();
                ui.add_space(10.0);
                let size_range = if is_signature {
                    16.0..=64.0
                } else {
                    8.0..=36.0
                };
                ui.spacing_mut().slider_width = (ui.available_width() - 115.0).max(40.0);
                let size_resp = ui.add(
                    egui::Slider::new(&mut item.size_pt, size_range)
                        .suffix(" pt")
                        .text("Size"),
                );
                changed |= size_resp.changed();
                lost_focus |= size_resp.lost_focus();
                ui.add_space(6.0);
                let (width_changed, width_lost_focus) = box_width_control(ui, item);
                changed |= width_changed;
                lost_focus |= width_lost_focus;
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Color").size(13.5));
                    let color_resp =
                        egui::widgets::color_picker::color_edit_button_srgb(ui, &mut item.color);
                    changed |= color_resp.changed();
                    lost_focus |= color_resp.lost_focus();
                });
            });
        self.finish_inspector_edit(ui, before, sel, changed, lost_focus);
        self.delete_selected_button(ui, sel);
    }
}
