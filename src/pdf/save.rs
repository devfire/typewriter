//! Baking overlays into pages and writing the result.
//!
//! Saves are incremental: the backing bytes are emitted verbatim and
//! only the objects a bake added or changed are appended, followed
//! by a new xref section and a trailer whose `/Prev` chains to the
//! previous xref (`lopdf::IncrementalDocument`). The in-memory
//! document is mutated directly; on failure the pre-save logical
//! document is restored (see `bake_and_save`).

use super::{PdfDoc, append_overlay_stream, box_operations, signature_font};
use crate::geometry::PageGeometry;
use crate::overlay::{BoxKind, OverlayText};
use anyhow::{Context, Result, bail};
use lopdf::{Document, IncrementalDocument, ObjectId};
use std::collections::BTreeMap;
use std::path::Path;

impl PdfDoc {
    /// Bake `by_page` into `doc`: embed the shared signature font
    /// (once) and append one overlay content stream per page.
    /// Returns the streams added, for the embedded payload that
    /// records their ids and hashes (see `embedded::store`).
    pub(super) fn bake_pages(
        doc: &mut Document,
        pages: &[PageGeometry],
        by_page: &BTreeMap<usize, Vec<&OverlayText>>,
        sig_font: Option<&signature_font::SignatureFont>,
    ) -> Result<(
        Vec<super::embedded::BakedStream>,
        std::collections::BTreeSet<ObjectId>,
    )> {
        let shared_sig_font = sig_font.map(|font| font.embed(doc));
        let mut touched = std::collections::BTreeSet::new();
        let mut baked = Vec::with_capacity(by_page.len());
        for (page, boxes) in by_page {
            // `bake_and_save` rejects out-of-range pages before baking,
            // so this is a caller bug, not a box to silently drop.
            let Some(geom) = pages.get(*page) else {
                bail!(
                    "overlay box is on page {} but the document has {} page(s)",
                    page + 1,
                    pages.len()
                );
            };
            let page_id = geom.id;
            let needs_text = boxes.iter().any(|item| item.kind == BoxKind::Text);
            let needs_sig = boxes.iter().any(|item| item.kind == BoxKind::Signature);
            let text_alias = needs_text
                .then(|| Self::ensure_helvetica(doc, page_id, &mut touched))
                .transpose()?;
            let sig_alias = if needs_sig && let Some(type0_id) = shared_sig_font {
                Some(Self::ensure_signature(
                    doc,
                    page_id,
                    type0_id,
                    &mut touched,
                )?)
            } else {
                None
            };
            let mut operations = Vec::new();
            for &item in boxes {
                match item.kind {
                    BoxKind::Text => {
                        if let Some(alias) = text_alias.as_ref() {
                            operations.extend(box_operations(item, alias, geom, None));
                        }
                    }
                    BoxKind::Signature => {
                        if let (Some(alias), Some(font)) = (sig_alias.as_ref(), sig_font) {
                            operations.extend(box_operations(item, alias, geom, Some(font)));
                        }
                    }
                }
            }
            let id = append_overlay_stream(doc, page_id, operations, &mut touched)
                .context("failed to append overlay content stream")?;
            baked.push(super::embedded::BakedStream { page: *page, id });
        }
        Ok((baked, touched))
    }

    /// Bake into `self.doc` and serialize only the resulting update:
    /// the backing bytes verbatim, then the objects the bake added
    /// or changed, then a new xref/trailer. Returns the full saved
    /// bytes (which become the backing bytes for the next save).
    pub(super) fn bake_and_save_incremental(
        &mut self,
        by_page: &BTreeMap<usize, Vec<&OverlayText>>,
        baked_boxes: &[OverlayText],
        sig_font: Option<&signature_font::SignatureFont>,
        out: &Path,
    ) -> Result<Vec<u8>> {
        let max_id_before = self.doc.max_id;
        let (baked_streams, mut touched) =
            Self::bake_pages(&mut self.doc, &self.pages, by_page, sig_font)?;
        // Embed the just-baked boxes and their streams' ids and
        // hashes so the saved file reopens editable; part of the
        // same update, so payload and baked streams always land in
        // the same incremental revision.
        super::embedded::store(&mut self.doc, baked_boxes, &baked_streams, &mut touched)?;
        // The update is exactly the bake's new objects plus the
        // pre-existing dictionaries changed since the backing bytes
        // were written: the bake's own mutations (page `Contents`,
        // resources/font dicts, catalog), recorded in `touched` at
        // each mutation site, and earlier in-memory changes (pages
        // stripped by `take_embedded_boxes`), carried in `dirty`.
        // Fresh `add_object` ids always exceed the pre-bake
        // `max_id`, so they need no recording; content streams are
        // only ever added under fresh ids, never mutated. No
        // re-parse of the backing bytes and no whole-document deep
        // comparison.
        // `IncrementalDocument`'s writer emits the previous bytes
        // verbatim and reads only trailer, xref position/type, and
        // max id from the previous document (see
        // `render_bytes_with_page_updates`), so a metadata-only
        // stand-in suffices — no second parse of the backing bytes
        // and no object graph cloned. `xref_start` is tracked on
        // the document after every save (see `parse_startxref`): a
        // stale offset would chain `/Prev` to the wrong revision
        // and the file would no longer parse.
        let mut prev = Document::new();
        prev.version.clone_from(&self.doc.version);
        prev.binary_mark.clone_from(&self.doc.binary_mark);
        prev.trailer.clone_from(&self.doc.trailer);
        prev.reference_table =
            lopdf::xref::Xref::new(0, self.doc.reference_table.cross_reference_type);
        prev.max_id = self.doc.max_id;
        prev.max_bookmark_id = self.doc.max_bookmark_id;
        prev.xref_start = self.doc.xref_start;
        touched.extend(self.dirty.iter().copied());
        let mut update = IncrementalDocument::create_from((*self.bytes).clone(), prev);
        for (id, obj) in &self.doc.objects {
            if id.0 > max_id_before || touched.contains(id) {
                update.new_document.set_object(*id, obj.clone());
            }
        }
        update.new_document.max_id = update.new_document.max_id.max(self.doc.max_id);
        let mut saved = Vec::with_capacity(self.bytes.len());
        update
            .save_to(&mut saved)
            .with_context(|| format!("failed to save {}", out.display()))?;
        let xref_start = parse_startxref(&saved).context("failed to locate xref in saved PDF")?;
        Self::atomic_write(&saved, out)?;
        self.doc.xref_start = xref_start;
        self.dirty.clear();
        Ok(saved)
    }

    /// Render bytes after baked streams were stripped from `doc`:
    /// the backing bytes verbatim plus an incremental update
    /// carrying only the changed page dictionaries, exactly like a
    /// save but held in memory for the preview. Re-serializing the
    /// whole document instead (the old `doc.save_to`) would copy
    /// and recompress every stream of a large scanned file just to
    /// open it, and the preview would render lopdf's rewrite of
    /// the file rather than the file itself. `None` when there is
    /// nothing to append or the update cannot be written; the
    /// caller then keeps rendering the backing bytes.
    pub(super) fn render_bytes_with_page_updates(
        &self,
        changed_pages: &[ObjectId],
    ) -> Option<Vec<u8>> {
        if changed_pages.is_empty() || self.bytes.is_empty() {
            return None;
        }
        // `IncrementalDocument`'s writer emits the previous bytes
        // verbatim and reads only trailer, xref position/type,
        // and max id from the previous document (`new_from_prev`
        // discards its objects), so a metadata-only stand-in for
        // the pre-strip document suffices — no second parse of
        // the backing bytes, and no object graph cloned.
        let mut prev = Document::new();
        prev.version.clone_from(&self.doc.version);
        prev.binary_mark.clone_from(&self.doc.binary_mark);
        prev.trailer.clone_from(&self.doc.trailer);
        prev.reference_table =
            lopdf::xref::Xref::new(0, self.doc.reference_table.cross_reference_type);
        prev.max_id = self.doc.max_id;
        prev.max_bookmark_id = self.doc.max_bookmark_id;
        prev.xref_start = self.doc.xref_start;
        let mut update = IncrementalDocument::create_from((*self.bytes).clone(), prev);
        for &page_id in changed_pages {
            let page = self.doc.get_object(page_id).ok()?.clone();
            update.new_document.set_object(page_id, page);
        }
        update.new_document.max_id = update.new_document.max_id.max(self.doc.max_id);
        let mut rendered = Vec::with_capacity(self.bytes.len());
        update.save_to(&mut rendered).ok()?;
        (!rendered.is_empty()).then_some(rendered)
    }

    /// Write `bytes` to `out` without ever truncating `out` in place.
    ///
    /// A symlink destination is resolved first, so the rename
    /// replaces the link target (in the target's own directory)
    /// and the link itself survives. The bytes go to an
    /// exclusively created temp file in that directory (0600 on
    /// Unix, so there is no world-readable window), the existing
    /// file's permissions are applied to the handle before any
    /// content is written, the file is `sync_all`-ed, then it is
    /// persisted (renamed) over the destination. A failed write
    /// leaves the original intact and removes the temp file.
    pub(super) fn atomic_write(bytes: &[u8], out: &Path) -> Result<()> {
        use std::io::Write as _;
        // Resolve a symlink (or any existing destination) to its
        // target; a not-yet-existing path keeps its given form.
        let resolved = std::fs::canonicalize(out).unwrap_or_else(|_| out.to_path_buf());
        let dir = resolved
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| std::path::PathBuf::from("."), Path::to_path_buf);
        let permissions = std::fs::metadata(&resolved).map(|m| m.permissions()).ok();
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)
            .with_context(|| format!("failed to save {}", out.display()))?;
        if let Some(permissions) = permissions {
            tmp.as_file()
                .set_permissions(permissions)
                .with_context(|| format!("failed to preserve permissions for {}", out.display()))?;
        }
        tmp.write_all(bytes)
            .with_context(|| format!("failed to save {}", out.display()))?;
        tmp.as_file()
            .sync_all()
            .with_context(|| format!("failed to sync {}", out.display()))?;
        tmp.persist(&resolved)
            .map_err(|err| err.error)
            .with_context(|| format!("failed to save {}", out.display()))?;
        // Best-effort directory sync for rename durability: some
        // filesystems cannot sync directories, and failure here
        // cannot corrupt the already-synced file, so it is ignored.
        if let Ok(dir_fd) = std::fs::File::open(&dir) {
            let _ = dir_fd.sync_all();
        }
        Ok(())
    }
}

/// Offset of the latest xref section in freshly written `saved`:
/// the integer after the final `startxref`, which both the full
/// and incremental writers emit as `"\nstartxref\n{offset}\n%%EOF"`.
/// Tracked on the document after every save so the next
/// incremental update's `/Prev` chains to the revision just
/// written without re-parsing the backing bytes. Anchored at the
/// end (digits between the last `startxref` and the trailing
/// `%%EOF`), so binary stream content cannot spoof it.
#[must_use]
fn startxref_tail(saved: &[u8]) -> Option<usize> {
    let tail = saved.strip_suffix(b"\n%%EOF")?;
    let digits = tail
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    let value_start = tail.len() - digits;
    let marker = b"\nstartxref\n";
    let marker_start = value_start.checked_sub(marker.len())?;
    if &tail[marker_start..value_start] != marker {
        return None;
    }
    std::str::from_utf8(&tail[value_start..]).ok()?.parse().ok()
}

pub(super) fn parse_startxref(saved: &[u8]) -> Result<usize> {
    startxref_tail(saved).context("saved PDF does not end with startxref offset and %%EOF")
}
