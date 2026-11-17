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
use std::sync::atomic::{AtomicU64, Ordering};

/// Counter for unique temp-file names used by atomic saves.
static SAVE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl PdfDoc {
    /// Bake `by_page` into `doc`: embed the shared signature font
    /// (once) and append one overlay content stream per page.
    pub(super) fn bake_pages(
        doc: &mut Document,
        page_ids: &[ObjectId],
        boxes_pt: &[(f32, f32, f32, f32)],
        rotations: &[i64],
        by_page: &BTreeMap<usize, Vec<&OverlayText>>,
        sig_font: Option<&signature_font::SignatureFont>,
        sig_face: Option<&ttf_parser::Face<'_>>,
    ) -> Result<()> {
        let shared_sig_font = sig_font
            .zip(sig_face)
            .map(|(font, face)| font.embed(doc, face));
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
            append_overlay_stream(doc, page_id, operations)
                .context("failed to append overlay content stream")?;
        }
        Ok(())
    }

    /// Bake into `self.doc` and serialize only the resulting update:
    /// the backing bytes verbatim, then the objects the bake added
    /// or changed, then a new xref/trailer. Returns the full saved
    /// bytes (which become the backing bytes for the next save).
    pub(super) fn bake_and_save_incremental(
        &mut self,
        by_page: &BTreeMap<usize, Vec<&OverlayText>>,
        sig_font: Option<&signature_font::SignatureFont>,
        sig_face: Option<&ttf_parser::Face<'_>>,
        out: &Path,
    ) -> Result<Vec<u8>> {
        Self::bake_pages(
            &mut self.doc,
            &self.page_ids,
            &self.boxes_pt,
            &self.rotations,
            by_page,
            sig_font,
            sig_face,
        )?;
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

    /// Write `bytes` to `out` without ever truncating `out` in place.
    ///
    /// The bytes are written to a unique temp file in the same
    /// directory (so the final `rename` stays on one filesystem and
    /// is atomic), `sync_all`-ed, then renamed over `out`. A failed
    /// write leaves the original intact and removes the temp file.
    /// When `out` already exists, its permissions are re-applied to
    /// the replacement.
    pub(super) fn atomic_write(bytes: &[u8], out: &Path) -> Result<()> {
        let dir = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| std::path::PathBuf::from("."), Path::to_path_buf);
        let stem = out.file_name().map_or_else(
            || std::ffi::OsString::from("out.pdf"),
            std::ffi::OsStr::to_os_string,
        );
        let seq = SAVE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut tmp_name = std::ffi::OsString::from(".");
        tmp_name.push(&stem);
        tmp_name.push(format!(".tmp-{}-{seq}", std::process::id()));
        let tmp = dir.join(tmp_name);
        let result = (|| {
            let permissions = std::fs::metadata(out).map(|m| m.permissions()).ok();
            std::fs::write(&tmp, bytes)
                .with_context(|| format!("failed to save {}", out.display()))?;
            if let Some(permissions) = permissions {
                std::fs::set_permissions(&tmp, permissions).with_context(|| {
                    format!("failed to preserve permissions for {}", out.display())
                })?;
            }
            std::fs::File::open(&tmp)
                .with_context(|| format!("failed to sync {}", out.display()))?
                .sync_all()
                .with_context(|| format!("failed to sync {}", out.display()))?;
            std::fs::rename(&tmp, out)
                .with_context(|| format!("failed to save {}", out.display()))?;
            if let Ok(dir_fd) = std::fs::File::open(&dir) {
                let _ = dir_fd.sync_all();
            }
            Ok::<(), anyhow::Error>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}
