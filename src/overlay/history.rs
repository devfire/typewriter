//! Undo/redo snapshot history for [`OverlayState`](super::OverlayState).
//!
//! A snapshot is a clone of the overlay `items` vec (small by
//! design). Discrete actions checkpoint the pre-action items;
//! continuous edits (drags, typing, slider drags) bracket
//! themselves as a *gesture* so the whole edit is one undo step.

use std::collections::VecDeque;

use super::OverlayText;

/// Maximum snapshots kept per stack; each is a clone of the (small)
/// `items` vec, so the cap bounds history memory.
const MAX_HISTORY: usize = 100;

#[derive(Debug, Default)]
pub(super) struct History {
    undo_stack: VecDeque<Vec<OverlayText>>,
    redo_stack: VecDeque<Vec<OverlayText>>,
    /// Pre-edit snapshot of an in-progress continuous edit, taken by
    /// `begin_gesture*` and pushed by `end_gesture` if it changed.
    gesture: Option<Vec<OverlayText>>,
}

impl History {
    /// Push `snapshot` (pre-action `items`) as one undo step and
    /// clear the redo stack: a new action invalidates redone futures.
    /// A snapshot identical to the newest step is not duplicated.
    fn push(&mut self, snapshot: Vec<OverlayText>) {
        if self.undo_stack.back() != Some(&snapshot) {
            self.undo_stack.push_back(snapshot);
            if self.undo_stack.len() > MAX_HISTORY {
                self.undo_stack.pop_front();
            }
        }
        self.redo_stack.clear();
    }

    /// Record `items` as one undo step, for a discrete action about
    /// to change them. Any in-progress gesture is first closed out
    /// as its own step, so checkpointing mid-gesture never merges
    /// two actions into one snapshot.
    pub(super) fn checkpoint(&mut self, items: &[OverlayText]) {
        self.end_gesture(items);
        self.push(items.to_vec());
    }

    /// Start a continuous edit: remember `items` so `end_gesture`
    /// can record the whole edit as a single undo step. No-op if a
    /// gesture already runs, so nested starts keep the oldest
    /// (pre-edit) snapshot.
    pub(super) fn begin_gesture(&mut self, items: &[OverlayText]) {
        if self.gesture.is_none() {
            self.gesture = Some(items.to_vec());
        }
    }

    /// Like `begin_gesture`, but with a snapshot taken before the
    /// widget that starts the edit already mutated the items (the
    /// inspector clones `items` before handing a box to its controls).
    pub(super) fn begin_gesture_with(&mut self, snapshot: Vec<OverlayText>) {
        if self.gesture.is_none() {
            self.gesture = Some(snapshot);
        }
    }

    /// Finish a continuous edit, recording its pre-edit snapshot as
    /// one undo step if the edit actually changed anything.
    pub(super) fn end_gesture(&mut self, items: &[OverlayText]) {
        if let Some(snapshot) = self.gesture.take()
            && snapshot != items
        {
            self.push(snapshot);
        }
    }

    pub(super) fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub(super) fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Swap `items` with the previous snapshot, moving the current
    /// one onto the redo stack. `false` when nothing to undo.
    pub(super) fn undo(&mut self, items: &mut Vec<OverlayText>) -> bool {
        self.end_gesture(items);
        let Some(previous) = self.undo_stack.pop_back() else {
            return false;
        };
        let current = std::mem::replace(items, previous);
        self.redo_stack.push_back(current);
        true
    }

    /// Inverse of `undo`. `false` when nothing to redo.
    pub(super) fn redo(&mut self, items: &mut Vec<OverlayText>) -> bool {
        self.end_gesture(items);
        let Some(next) = self.redo_stack.pop_back() else {
            return false;
        };
        let current = std::mem::replace(items, next);
        self.undo_stack.push_back(current);
        if self.undo_stack.len() > MAX_HISTORY {
            self.undo_stack.pop_front();
        }
        true
    }

    /// Drop all history: baked (or newly loaded) boxes must not be
    /// resurrected by undo.
    pub(super) fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.gesture = None;
    }
}
