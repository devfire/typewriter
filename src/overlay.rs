//! Overlay text boxes and screen<->PDF coordinate math.
//!
//! Single source of truth for `ASCENT`/`LEADING`: the egui preview in
//! `app.rs` and the content-stream baker in `pdf.rs` both import these,
//! and both wrap text with `pdf::helvetica_width_pt`, so preview and
//! baked output cannot drift.

use eframe::egui::{Pos2, Rect};

mod history;
use history::History;

/// Fraction of the font size from the top of the em box to the baseline.
pub const ASCENT: f32 = 0.8;
/// Line spacing as a multiple of the font size.
pub const LEADING: f32 = 1.2;

/// Offset applied to a pasted/duplicated box, in PDF points, so it
/// does not land exactly on top of its source. Repeated pastes
/// cascade by this amount (see [`OverlayState::paste`]).
pub const PASTE_OFFSET_PT: f32 = 12.0;

/// What a box contains: ordinary typed text, or a typed signature
/// rendered (preview and baked PDF) in the Momo Signature script font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    Text,
    Signature,
}

/// Stable identity of an overlay box, assigned once by
/// [`OverlayState::add`] and never reused. Selection/resize state
/// holds a `BoxId`, not a positional index, so removing an earlier
/// box cannot silently retarget it at a different box.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BoxId(u64);

/// One typewriter text box. `x_pt`/`y_top_pt` are PDF points, with
/// `y_top_pt` measured from the TOP of the page to the top of the em box.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayText {
    pub id: BoxId,
    pub kind: BoxKind,
    pub page: usize,
    pub x_pt: f32,
    pub y_top_pt: f32,
    pub text: String,
    pub size_pt: f32,
    pub color: [u8; 3],
    /// Manual box width in PDF points, set by dragging the resize
    /// handle. `None` = auto-fit to the measured rendered text.
    /// Text boxes wrap at this width in both the preview and the
    /// baked PDF; signatures never wrap (see `signature_line`), so
    /// for them it only sizes the on-screen frame.
    pub width_pt: Option<f32>,
}

impl OverlayText {
    pub fn new(id: BoxId, page: usize, x_pt: f32, y_top_pt: f32, kind: BoxKind) -> Self {
        let (size_pt, color) = match kind {
            BoxKind::Text => (12.0, [0, 0, 0]),
            // Larger by default, in dark-blue pen ink.
            BoxKind::Signature => (32.0, [0x20, 0x2F, 0x6B]),
        };
        Self {
            id,
            kind,
            page,
            x_pt,
            y_top_pt,
            text: String::new(),
            size_pt,
            color,
            width_pt: None,
        }
    }

    /// The signature as one line: signatures never wrap, so embedded
    /// newlines (pasted text) collapse to spaces.
    #[must_use]
    pub fn signature_line(&self) -> String {
        self.text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// All overlay state for the open document.
///
/// Undo/redo is a snapshot stack of `items` (see [`Self::undo`]):
/// discrete mutators ([`Self::add`], [`Self::remove`],
/// [`Self::paste`], …) checkpoint automatically, while continuous
/// edits (canvas drags, inspector typing/slider drags) mutate fields
/// directly and bracket themselves with
/// [`Self::begin_gesture`]/[`Self::end_gesture`] so a whole drag or
/// edit burst is one undo step. Selection is not snapshotted; it is
/// only revalidated against the restored items. `next_id` never
/// rewinds, so a box created after undoing an `add` never reuses
/// the undone box's id.
#[derive(Debug, Default)]
pub struct OverlayState {
    pub items: Vec<OverlayText>,
    pub selected: Option<BoxId>,
    next_id: u64,
    pub dirty: bool,
    history: History,
}

impl OverlayState {
    /// Record the current `items` as one undo step. Discrete
    /// mutators call this before changing anything; see
    /// `history::History` for the snapshot/gesture mechanics.
    pub fn checkpoint(&mut self) {
        self.history.checkpoint(&self.items);
    }

    /// Start a continuous edit (drag, typing, slider) so it becomes
    /// a single undo step when [`Self::end_gesture`] is called.
    pub fn begin_gesture(&mut self) {
        self.history.begin_gesture(&self.items);
    }

    /// Like [`Self::begin_gesture`], but with a snapshot taken before
    /// the widget starting the edit already mutated `items`.
    pub fn begin_gesture_with(&mut self, snapshot: Vec<OverlayText>) {
        self.history.begin_gesture_with(snapshot);
    }

    /// Finish a continuous edit, recording it as one undo step if
    /// it changed anything.
    pub fn end_gesture(&mut self) {
        self.history.end_gesture(&self.items);
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Restore the previous `items` snapshot. Returns `false` when
    /// there is nothing to undo. Dirty is recomputed from the
    /// restored items: undoing back to no boxes means nothing is
    /// left to bake.
    pub fn undo(&mut self) -> bool {
        if !self.history.undo(&mut self.items) {
            return false;
        }
        self.after_restore();
        true
    }

    /// Inverse of [`Self::undo`]. Returns `false` when there is
    /// nothing to redo.
    pub fn redo(&mut self) -> bool {
        if !self.history.redo(&mut self.items) {
            return false;
        }
        self.after_restore();
        true
    }

    /// Common post-restore fixups: a selection pointing at a box the
    /// restored snapshot does not contain is dropped, and dirty
    /// reflects whether anything remains to bake.
    fn after_restore(&mut self) {
        if self.selected.is_some_and(|sel| self.item(sel).is_none()) {
            self.selected = None;
        }
        self.dirty = !self.items.is_empty();
    }

    fn fresh_id(&mut self) -> BoxId {
        let id = BoxId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Insert a new empty box and select it. Returns its stable id.
    pub fn add(&mut self, page: usize, x_pt: f32, y_top_pt: f32, kind: BoxKind) -> BoxId {
        self.checkpoint();
        let id = self.fresh_id();
        self.items
            .push(OverlayText::new(id, page, x_pt, y_top_pt, kind));
        self.selected = Some(id);
        self.dirty = true;
        id
    }

    /// Box with stable identity `id`, wherever it currently sits.
    #[must_use]
    pub fn item(&self, id: BoxId) -> Option<&OverlayText> {
        self.items.iter().find(|item| item.id == id)
    }

    /// Mutable box with stable identity `id`.
    pub fn item_mut(&mut self, id: BoxId) -> Option<&mut OverlayText> {
        self.items.iter_mut().find(|item| item.id == id)
    }

    pub fn remove(&mut self, id: BoxId) {
        let Some(pos) = self.items.iter().position(|item| item.id == id) else {
            return;
        };
        self.checkpoint();
        self.items.remove(pos);
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.dirty = true;
    }

    /// Snapshot of the selected box (text or signature, with its
    /// size, color, and width) for the clipboard. The snapshot's
    /// `id` is stale by construction: [`Self::paste`] always
    /// assigns a fresh one, never reusing the source's.
    #[must_use]
    pub fn copy_selected(&self) -> Option<OverlayText> {
        self.selected.and_then(|id| self.item(id)).cloned()
    }

    /// Like [`Self::copy_selected`], but also removes the selected box.
    pub fn cut_selected(&mut self) -> Option<OverlayText> {
        let copied = self.copy_selected()?;
        if let Some(sel) = self.selected {
            self.remove(sel);
        }
        Some(copied)
    }

    /// Insert a copy of `source` on `page` (which may differ from
    /// `source.page`, for copy-on-one-page / paste-on-another),
    /// offset by [`PASTE_OFFSET_PT`] from `source`'s position,
    /// and select it. `source` is advanced to the pasted position,
    /// so pasting repeatedly from the same clipboard snapshot
    /// cascades instead of stacking copies in one spot. Returns
    /// the new box's fresh id.
    pub fn paste(&mut self, source: &mut OverlayText, page: usize) -> BoxId {
        self.checkpoint();
        source.x_pt = (source.x_pt + PASTE_OFFSET_PT).max(0.0);
        source.y_top_pt = (source.y_top_pt + PASTE_OFFSET_PT).max(0.0);
        source.page = page;
        let id = self.fresh_id();
        let mut item = source.clone();
        item.id = id;
        self.items.push(item);
        self.selected = Some(id);
        self.dirty = true;
        id
    }

    /// Duplicate the selected box in place (same page, offset by
    /// [`PASTE_OFFSET_PT`]), without touching the clipboard.
    /// Returns the new box's id, or `None` when nothing is selected.
    pub fn duplicate_selected(&mut self) -> Option<BoxId> {
        let mut source = self.copy_selected()?;
        let page = source.page;
        Some(self.paste(&mut source, page))
    }

    /// Boxes on `page`.
    #[must_use]
    pub fn by_page(&self, page: usize) -> Vec<&OverlayText> {
        self.items.iter().filter(|item| item.page == page).collect()
    }

    /// Drop the selected box if it is empty (click placed it but the
    /// user never typed). Called before selection changes / save.
    pub fn drop_empty_selected(&mut self) {
        if let Some(sel) = self.selected
            && self.item(sel).is_some_and(OverlayText::is_empty)
        {
            self.checkpoint();
            self.items.retain(|item| item.id != sel);
            self.selected = None;
        }
    }

    /// Adopt boxes restored from a PDF's embedded payload (see
    /// `pdf::embedded`) as the live, clean state: fresh session ids
    /// (payload ids are placeholders), no selection, not dirty —
    /// the boxes are already baked into the file as opened, so
    /// closing without edits has nothing to save. History is
    /// cleared: undo must not resurrect pre-load state.
    pub fn restore_embedded(&mut self, mut items: Vec<OverlayText>) {
        for item in &mut items {
            item.id = self.fresh_id();
        }
        self.items = items;
        self.selected = None;
        self.dirty = false;
        self.history.clear();
    }

    /// Clear everything after a successful bake.
    pub fn clear_baked(&mut self) {
        self.items.clear();
        self.selected = None;
        self.dirty = false;
        // Baked (or newly loaded) boxes must not be resurrected by
        // undo: they are already in the PDF / no longer on screen.
        self.history.clear();
    }
}

/// Tab stop width in spaces, shared by the preview and PDF baker.
pub const TAB_SPACES: &str = "    ";

/// Split `text` into logical lines (`\r\n` or `\n`, dropping a single
/// trailing empty line from a final newline) with `\t` expanded to
/// [`TAB_SPACES`]. Shared by the egui preview and the PDF baker so
/// auto-width and fixed-width boxes see the same lines and tab widths.
#[must_use]
pub fn logical_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            line.strip_suffix('\r')
                .unwrap_or(line)
                .replace('\t', TAB_SPACES)
        })
        .collect();
    if lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn hard_break(
    word: &str,
    max_width_pt: f32,
    measure: &impl Fn(&str) -> f32,
    out: &mut Vec<String>,
) -> String {
    let mut chunk = String::new();
    for ch in word.chars() {
        chunk.push(ch);
        if measure(&chunk) > max_width_pt && chunk.chars().count() > 1 {
            chunk.truncate(chunk.len() - ch.len_utf8());
            out.push(std::mem::take(&mut chunk));
            chunk.push(ch);
        }
    }
    chunk
}

/// Greedily wrap one logical line to at most `max_width_pt`, breaking
/// at whitespace boundaries without normalizing spaces and hard-breaking
/// a single word wider than the box. `measure` returns a candidate
/// string's rendered width in points; both the preview and the baker
/// pass Helvetica metrics so their line breaks match exactly, while
/// sharing the breaking rules. A non-positive width disables wrapping.
#[must_use]
pub fn wrap_line(line: &str, max_width_pt: f32, measure: &impl Fn(&str) -> f32) -> Vec<String> {
    if max_width_pt <= 0.0 || line.is_empty() || measure(line) <= max_width_pt {
        return vec![line.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut rest = line;
    while !rest.is_empty() {
        let ws_len = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        let (ws, after_ws) = rest.split_at(ws_len);
        if after_ws.is_empty() {
            if current.is_empty() {
                current = hard_break(ws, max_width_pt, measure, &mut out);
            } else {
                let candidate = format!("{current}{ws}");
                if measure(&candidate) <= max_width_pt {
                    current = candidate;
                }
            }
            break;
        }
        let word_len = after_ws.find(char::is_whitespace).unwrap_or(after_ws.len());
        let (word, next_rest) = after_ws.split_at(word_len);
        rest = next_rest;
        let candidate = format!("{current}{ws}{word}");
        if measure(&candidate) <= max_width_pt {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current = if measure(word) <= max_width_pt {
            word.to_string()
        } else {
            hard_break(word, max_width_pt, measure, &mut out)
        };
    }
    if !current.is_empty() || out.is_empty() {
        out.push(current);
    }
    out
}

/// Wrap every line in `lines` with [`wrap_line`], flattening the result.
#[must_use]
pub fn wrap_lines(
    lines: &[String],
    max_width_pt: f32,
    measure: &impl Fn(&str) -> f32,
) -> Vec<String> {
    lines
        .iter()
        .flat_map(|line| wrap_line(line, max_width_pt, measure))
        .collect()
}

/// Convert a screen position inside `page_rect` to PDF points measured
/// from the page's top-left corner.
#[must_use]
pub fn screen_to_pdf_top(screen: Pos2, page_rect: Rect, zoom: f32) -> (f32, f32) {
    let zoom = if zoom > 0.0 { zoom } else { 1.0 };
    (
        ((screen.x - page_rect.min.x) / zoom).max(0.0),
        ((screen.y - page_rect.min.y) / zoom).max(0.0),
    )
}

/// Inverse of [`screen_to_pdf_top`].
#[must_use]
pub fn pdf_top_to_screen(x_pt: f32, y_top_pt: f32, page_rect: Rect, zoom: f32) -> Pos2 {
    Pos2::new(
        page_rect.min.x + x_pt * zoom,
        page_rect.min.y + y_top_pt * zoom,
    )
}

/// Top-left-origin baseline: PDF y of the first line's baseline.
#[must_use]
pub fn baseline_y(y1: f32, y_top_pt: f32, size_pt: f32) -> f32 {
    y1 - y_top_pt - ASCENT * size_pt
}

/// `Tm` operands `[a b c d e f]` for a box on a page with `/Rotate`
/// `rotate` (clockwise display rotation, in degrees).
///
/// Overlay coordinates are display space: `x_pt` measured from the
/// displayed page's left edge, `y_top_pt` from its top edge. The
/// linear part counter-rotates the glyphs in unrotated user space so
/// they read upright once the renderer applies `/Rotate`, and
/// `(e, f)` maps the first line's baseline origin back into user
/// space. `/Rotate` values that are not a multiple of 90 are invalid
/// per the PDF spec and are treated as 0, matching how `sizes_pt`
/// only swaps the display size for 90/270.
#[must_use]
pub fn text_matrix(item: &OverlayText, page_box: (f32, f32, f32, f32), rotate: i64) -> [f32; 6] {
    let (x0, y0, x1, y1) = page_box;
    // Distance from the displayed page's top edge to the baseline.
    let baseline_top = item.y_top_pt + ASCENT * item.size_pt;
    match rotate.rem_euclid(360) {
        90 => [0.0, 1.0, -1.0, 0.0, x0 + baseline_top, y0 + item.x_pt],
        180 => [-1.0, 0.0, 0.0, -1.0, x1 - item.x_pt, y0 + baseline_top],
        270 => [0.0, -1.0, 1.0, 0.0, x1 - baseline_top, y1 - item.x_pt],
        _ => [
            1.0,
            0.0,
            0.0,
            1.0,
            x0 + item.x_pt,
            baseline_y(y1, item.y_top_pt, item.size_pt),
        ],
    }
}

#[cfg(test)]
mod tests;
