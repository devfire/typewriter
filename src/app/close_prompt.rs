//! Unsaved-changes guard for window close and opening another PDF.
//!
//! egui signals a window-close request for exactly one frame; if the
//! document is dirty we cancel that close and show a modal offering
//! Save / Discard / Cancel. The same modal guards `begin_open`: the
//! picked path is parked in the prompt and only loaded after Discard
//! or a successful Save, so Cancel / failed Save / failed load keep
//! the current document and its overlays. Save only proceeds when
//! `save` reports the file was actually written, so a failed write
//! keeps the prompt open. The prompt's Save choice is itself the
//! overwrite consent, so `save` is not asked to confirm it again.

use super::{
    ACCENT, BTN_H, CloseState, DANGER, PendingAction, PromptChoice, RADIUS, SaveMode, TypewriterApp,
};
use eframe::egui;
use std::path::PathBuf;

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
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        match self.close_state {
            CloseState::Allowed => {}
            CloseState::Idle => {
                if self.overlays.is_dirty() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.close_state = CloseState::Prompting(PendingAction::Close);
                }
            }
            CloseState::Prompting(_) => {
                // A prompt is already up: never let the window close
                // out from under it. The explicit close request
                // supersedes any parked action, so re-aim the prompt
                // at closing (a parked `Open` path is dropped).
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_state = CloseState::Prompting(PendingAction::Close);
            }
        }
    }

    /// Open `path`, or — with unsaved boxes — park it behind the
    /// unsaved-changes prompt instead of discarding them.
    pub(super) fn begin_open(&mut self, path: PathBuf) {
        if matches!(self.close_state, CloseState::Prompting(_)) {
            return;
        }
        if self.overlays.is_dirty() {
            self.close_state = CloseState::Prompting(PendingAction::Open(path));
        } else {
            self.load(path);
        }
    }

    /// Apply a prompt button. Failed Save (failed write) leaves
    /// the prompt — and the overlays — in place.
    pub(super) fn resolve_prompt(&mut self, ctx: &egui::Context, choice: PromptChoice) {
        match choice {
            PromptChoice::Save => {
                if self.save(SaveMode::InPlaceConfirmed) {
                    self.complete_prompt(ctx);
                }
            }
            PromptChoice::Discard => self.complete_prompt(ctx),
            PromptChoice::Cancel => self.close_state = CloseState::Idle,
        }
    }

    /// Carry out the action parked behind the prompt. For `Open` this
    /// is the first point overlays can be lost, and only via a
    /// successful `load` (which clears them); a failed load keeps them.
    fn complete_prompt(&mut self, ctx: &egui::Context) {
        let CloseState::Prompting(action) =
            std::mem::replace(&mut self.close_state, CloseState::Idle)
        else {
            return;
        };
        match action {
            PendingAction::Close => {
                self.close_state = CloseState::Allowed;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            PendingAction::Open(path) => {
                self.load(path);
            }
        }
    }

    /// Modal shown while a [`PendingAction`] waits on unsaved changes.
    pub(super) fn close_prompt(&mut self, ctx: &egui::Context) {
        let CloseState::Prompting(action) = &self.close_state else {
            return;
        };
        let verb = match action {
            PendingAction::Close => "before closing",
            PendingAction::Open(_) => "before opening another PDF",
        };
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
                egui::RichText::new(format!("{file} has unsaved changes. Save them {verb}?"))
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
            self.resolve_prompt(ctx, PromptChoice::Save);
        } else if discard {
            self.resolve_prompt(ctx, PromptChoice::Discard);
        } else if cancel || modal.should_close() {
            self.resolve_prompt(ctx, PromptChoice::Cancel);
        }
    }
}
