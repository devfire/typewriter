//! Unsaved-changes guard for window close.
//!
//! egui signals a window-close request for exactly one frame; if the
//! document is dirty we cancel that close and show a modal offering
//! Save / Discard / Cancel. Save only closes when `save` reports the
//! file was actually written, so cancelling the overwrite dialog or a
//! failed write keeps the app (and the prompt) open.

use super::{ACCENT, BTN_H, CloseState, DANGER, RADIUS, SaveMode, TypewriterApp};
use eframe::egui;

/// Visual style of a prompt button: filled (Save/Discard, caller
/// supplies the fill color) vs. plain (Cancel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonStyle {
    Filled,
    Plain,
}

fn prompt_button(label: &str, style: ButtonStyle) -> egui::Button<'_> {
    let text = egui::RichText::new(label).size(15.0);
    let text = if style == ButtonStyle::Filled {
        text.strong().color(egui::Color32::WHITE)
    } else {
        text
    };
    egui::Button::new(text)
        .min_size(egui::vec2(104.0, BTN_H))
        .corner_radius(RADIUS)
}

impl TypewriterApp {
    /// Intercept a close request while there are unsaved changes.
    pub(super) fn guard_close(&mut self, ctx: &egui::Context) {
        if self.close_state != CloseState::Idle || !self.overlays.dirty {
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_state = CloseState::Prompting;
        }
    }

    /// Modal shown after [`Self::guard_close`] cancelled a close.
    pub(super) fn close_prompt(&mut self, ctx: &egui::Context) {
        if self.close_state != CloseState::Prompting {
            return;
        }
        let mut save = false;
        let mut discard = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("unsaved_changes")).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.label(egui::RichText::new("Unsaved changes").size(18.0).strong());
            ui.add_space(6.0);
            let file = self
                .pdf_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map_or_else(
                    || "This document".to_string(),
                    |name| format!("\u{201c}{}\u{201d}", name.to_string_lossy()),
                );
            ui.label(
                egui::RichText::new(format!(
                    "{file} has unsaved changes. Save them before closing?"
                ))
                .size(14.0),
            );
            ui.add_space(16.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        prompt_button("Save", ButtonStyle::Filled)
                            .fill(ACCENT)
                            .stroke(egui::Stroke::NONE),
                    )
                    .clicked()
                {
                    save = true;
                }
                if ui
                    .add(
                        prompt_button("Discard", ButtonStyle::Filled)
                            .fill(DANGER)
                            .stroke(egui::Stroke::NONE),
                    )
                    .clicked()
                {
                    discard = true;
                }
                if ui
                    .add(prompt_button("Cancel", ButtonStyle::Plain))
                    .clicked()
                {
                    cancel = true;
                }
            });
        });
        if save {
            // A cancelled overwrite dialog or failed write leaves
            // `dirty` set; keep the prompt open instead of closing.
            if self.save(SaveMode::InPlace) {
                self.close_state = CloseState::Allowed;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        } else if discard {
            self.close_state = CloseState::Allowed;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if cancel || modal.should_close() {
            self.close_state = CloseState::Idle;
        }
    }
}
