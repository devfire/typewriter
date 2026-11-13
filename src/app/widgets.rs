//! Shared toolbar / sidebar widgets built on the shared constants.

use super::{ACCENT, BTN_H, RADIUS};
use crate::overlay::BoxKind;
use eframe::egui;

/// Large, easy-to-hit button with a 15px label.
pub(super) fn big_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(label).size(15.0))
        .min_size(egui::vec2(0.0, BTN_H))
        .corner_radius(RADIUS)
}

/// Accent-filled call-to-action (Open, active mode, …).
pub(super) fn primary_button(label: &str) -> egui::Button<'_> {
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
pub(super) fn icon_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(label).size(17.0))
        .min_size(egui::vec2(BTN_H, BTN_H))
        .corner_radius(RADIUS)
}

/// Rounded card grouping related controls.
pub(super) fn group(ui: &mut egui::Ui) -> egui::Frame {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::same(5))
}

/// Read-only pill showing a value such as `Page 2 / 9` or `125%`.
pub(super) fn pill(ui: &mut egui::Ui, text: String) {
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
pub(super) fn mode_button(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
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

/// "Box width" row: fixed-width drag value with Auto reset, or the
/// auto-fit state with a hint about the on-page resize handle.
/// Returns whether the width changed.
pub(super) fn box_width_control(ui: &mut egui::Ui, item: &mut crate::overlay::OverlayText) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(match item.kind {
                // Signatures never wrap, so width only sizes their frame.
                BoxKind::Signature => "Box width (frame only)",
                BoxKind::Text => "Box width",
            })
            .size(13.5),
        );
        let mut reset_width = false;
        match &mut item.width_pt {
            Some(width_pt) => {
                let width_resp = ui.add(
                    egui::DragValue::new(width_pt)
                        .speed(1.0)
                        .range(24.0..=1200.0)
                        .suffix(" pt"),
                );
                changed |= width_resp.changed();
                if ui.small_button("Auto").clicked() {
                    reset_width = true;
                }
            }
            None => {
                ui.label(egui::RichText::new("Auto (fits text)").size(13.5).weak())
                    .on_hover_text(
                        "Drag the blue handle at the box's bottom-right \
                         corner on the page to set a fixed width",
                    );
            }
        }
        if reset_width {
            item.width_pt = None;
            changed = true;
        }
    });
    changed
}

/// Muted rounded hint card with a title and one-line explanation.
pub(super) fn hint_card(ui: &mut egui::Ui, title: &str, body: &str) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(RADIUS)
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
