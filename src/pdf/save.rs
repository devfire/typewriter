//! Baking overlays into pages and writing the result.
//!
//! Saves are incremental: the backing bytes are emitted verbatim and
//! only the objects a bake added or changed are appended, followed
//! by a new xref section and a trailer whose `/Prev` chains to the
//! previous xref (`lopdf::IncrementalDocument`). The in-memory
//! document is mutated directly; on failure it is restored by
//! re-parsing the untouched backing bytes (see `bake_and_save`).

use super::{PdfDoc, append_overlay_stream, box_operations, signature_font};
use crate::overlay::{BoxKind, OverlayText};
use anyhow::{Context, Result};
use lopdf::{Document, IncrementalDocument, Object, ObjectId};
use std::collections::BTreeMap;
use std::path::Path;

impl PdfDoc {
    /// Bake `by_page` into `doc`: embed the shared signature font
    /// (once) and append one overlay content stream per page.
    /// Returns the streams added, for the embedded payload that
    /// records their ids and hashes (see `embedded::store`).
    pub(super) fn bake_pages(
        doc: &mut Document,
        page_ids: &[ObjectId],
        boxes_pt: &[(f32, f32, f32, f32)],
        rotations: &[i64],
        by_page: &BTreeMap<usize, Vec<&OverlayText>>,
        sig_font: Option<&signature_font::SignatureFont>,
        sig_face: Option<&ttf_parser::Face<'_>>,
    ) -> Result<Vec<super::embedded::BakedStream>> {
        let shared_sig_font = sig_font
            .zip(sig_face)
            .map(|(font, face)| font.embed(doc, face));
        let mut baked = Vec::with_capacity(by_page.len());
        for (page, boxes) in by_page {
            let page_id = page_ids[*page];
            let page_box = boxes_pt[*page];
            let rotate = rotations[*page];
            let mut operations = Vec::new();
            let text_boxes: Vec<&OverlayText> = boxes
                .iter()
                .copied()
                .filter(|item| item.kind == BoxKind::Text)
                .collect();
            if !text_boxes.is_empty() {
                let alias = Self::ensure_helvetica(doc, page_id)?;
                for item in text_boxes {
                    operations.extend(box_operations(item, &alias, page_box, rotate, None));
                }
            }
            let sig_boxes: Vec<&OverlayText> = boxes
                .iter()
                .copied()
                .filter(|item| item.kind == BoxKind::Signature)
                .collect();
            if let (false, Some(font), Some(type0_id)) =
                (sig_boxes.is_empty(), sig_font, shared_sig_font)
            {
                let alias = Self::ensure_signature(doc, page_id, type0_id)?;
                for item in sig_boxes {
                    operations.extend(box_operations(item, &alias, page_box, rotate, Some(font)));
                }
            }
            let id = append_overlay_stream(doc, page_id, operations)
                .context("failed to append overlay content stream")?;
            baked.push(super::embedded::BakedStream { page: *page, id });
        }
        Ok(baked)
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
        sig_face: Option<&ttf_parser::Face<'_>>,
        out: &Path,
    ) -> Result<Vec<u8>> {
        let baked_streams = Self::bake_pages(
            &mut self.doc,
            &self.page_ids,
            &self.boxes_pt,
            &self.rotations,
            by_page,
            sig_font,
            sig_face,
        )?;
        // Embed the just-baked boxes and their streams' ids and
        // hashes so the saved file reopens editable; part of the
        // same diff, so payload and baked streams always land in
        // the same incremental revision.
        super::embedded::store(&mut self.doc, baked_boxes, &baked_streams)?;
        // The previous revision, re-parsed from the untouched bytes:
        // the diff against it is exactly this update's object set.
        // Changed pre-existing objects are always dictionaries (page,
        // resources, font dicts); content streams are only ever added
        // under fresh ids, never mutated, so an unchanged stream id
        // needs no deep comparison of its bytes.
        let mut update = IncrementalDocument::create_from(
            (*self.bytes).clone(),
            Document::load_mem(&self.bytes).context("failed to re-parse PDF")?,
        );
        let changed: Vec<(ObjectId, Object)> = self
            .doc
            .objects
            .iter()
            .filter(|(id, obj)| {
                update
                    .get_prev_documents()
                    .objects
                    .get(id)
                    .is_none_or(|prev| {
                        !matches!((prev, *obj), (Object::Stream(_), Object::Stream(_)))
                            && prev != *obj
                    })
            })
            .map(|(id, obj)| (*id, obj.clone()))
            .collect();
        for (id, obj) in changed {
            update.new_document.set_object(id, obj);
        }
        update.new_document.max_id = update.new_document.max_id.max(self.doc.max_id);
        let mut saved = Vec::with_capacity(self.bytes.len());
        update
            .save_to(&mut saved)
            .with_context(|| format!("failed to save {}", out.display()))?;
        Self::atomic_write(&saved, out)?;
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
