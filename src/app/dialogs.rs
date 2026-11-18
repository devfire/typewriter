//! Native file dialogs on a worker thread.
//!
//! `rfd::FileDialog` blocks until the user picks a file, so running
//! it on the UI thread freezes the whole frame for the duration of
//! the dialog. Each dialog here runs on a one-shot thread and sends
//! its picked path back over a channel; `poll_dialog_outcomes` (run
//! every frame) applies the result, and the thread calls
//! `request_repaint` on completion so the pick lands immediately
//! instead of waiting for the next input event. Only one dialog is
//! ever outstanding: a second request while one is open is dropped.

use super::TypewriterApp;
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// A finished file dialog's picked path (`None` = cancelled).
pub(super) enum DialogOutcome {
    Open(Option<PathBuf>),
    SaveAs(Option<PathBuf>),
}

impl TypewriterApp {
    /// Show the Open dialog off the UI thread; the picked path goes
    /// through `begin_open` (and its unsaved-changes guard) when it
    /// arrives.
    pub(super) fn open_dialog(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.spawn_dialog(ctx, DialogKind::Open) else {
            return;
        };
        self.dialog_rx = Some(rx);
    }

    /// Show the Save As dialog off the UI thread; the picked path is
    /// baked to when it arrives, and picking it already consents to
    /// overwriting that file, so later in-place saves don't re-ask.
    pub(super) fn save_as_dialog(&mut self, ctx: &egui::Context) {
        if self.pdf_doc.is_none() {
            return;
        }
        self.overlays.drop_empty_selected();
        let Some(rx) = self.spawn_dialog(ctx, DialogKind::SaveAs) else {
            return;
        };
        self.dialog_rx = Some(rx);
    }

    /// Apply a finished dialog, if any. Called once per frame.
    pub(super) fn poll_dialog_outcomes(&mut self) {
        let Some(rx) = &self.dialog_rx else {
            return;
        };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.dialog_rx = None;
                return;
            }
        };
        self.dialog_rx = None;
        match outcome {
            DialogOutcome::Open(Some(path)) => self.begin_open(path),
            DialogOutcome::SaveAs(Some(path)) => {
                self.overwrite_confirmed_for = Some(path.clone());
                self.bake_to(&path);
            }
            DialogOutcome::Open(None) | DialogOutcome::SaveAs(None) => {}
        }
    }

    /// Spawn the one-shot dialog thread, or `None` when a dialog is
    /// already outstanding (or, for Save As, nothing is loaded).
    fn spawn_dialog(
        &mut self,
        ctx: &egui::Context,
        kind: DialogKind,
    ) -> Option<Receiver<DialogOutcome>> {
        if self.dialog_rx.is_some() {
            return None;
        }
        let (tx, rx) = channel::<DialogOutcome>();
        let ctx = ctx.clone();
        let file_name = match kind {
            DialogKind::Open => None,
            DialogKind::SaveAs => self
                .pdf_path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned()),
        };
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().add_filter("PDF", &["pdf"]);
            if let Some(name) = file_name {
                dialog = dialog.set_file_name(name);
            }
            let outcome = match kind {
                DialogKind::Open => DialogOutcome::Open(dialog.pick_file()),
                DialogKind::SaveAs => DialogOutcome::SaveAs(dialog.save_file()),
            };
            // The receiver is gone if the app closed mid-dialog.
            let _ = tx.send(outcome);
            ctx.request_repaint();
        });
        Some(rx)
    }
}

/// Which native dialog a worker thread should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogKind {
    Open,
    SaveAs,
}
