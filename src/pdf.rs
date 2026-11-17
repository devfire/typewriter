//! PDF document model and overlay writer.
//!
//! Original page content streams are left intact and bracketed in
//! `q`/`Q` streams so baked text and signatures (appended in a new
//! content stream per page) start in the default graphics state.
//! Saves are incremental updates: the bytes as opened (or last saved)
//! are emitted verbatim, followed only by the objects a bake added or
//! changed, a new xref section, and a trailer chaining to the previous
//! xref via `/Prev` (lopdf `IncrementalDocument`). Byte ranges covered
//! by existing digital signatures therefore stay intact. Existing
//! usable Helvetica fonts are reused; a new standard-14 Helvetica or
//! embedded signature font is added only when needed.

use crate::overlay::{BoxKind, LEADING, OverlayState, OverlayText, text_matrix, wrap_lines};
use anyhow::{Context, Result, bail};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat, dictionary};
use page_tree::{
    effective_font_dict, effective_fonts, inherited_int, insert_font_entry, page_box, resolve,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Loaded PDF. The raw file bytes live in one `Arc<Vec<u8>>` shared
/// with hayro's renderer (`PdfData: From<Arc<Vec<u8>>>`), so no
/// full-file clone is retained; lopdf separately holds its own
/// parsed representation in `doc`. `bytes` is the file as opened or
/// last successfully saved: the exact prefix the next incremental
/// save re-emits before appending its update.
pub struct PdfDoc {
    pub bytes: Arc<Vec<u8>>,
    /// Bytes hayro should render. Identical to `bytes` until the
    /// embedded boxes are claimed via `take_embedded_boxes`: then
    /// the baked overlay streams are stripped from `doc` (see
    /// `embedded`) and this is `bytes` plus an incremental update
    /// carrying only the changed page dictionaries, so the
    /// preview shows each restored box once instead of over its
    /// baked copy — and renders the actual file everywhere else.
    /// Never saved; saves stay incremental on `bytes`.
    pub render_bytes: Arc<Vec<u8>>,
    /// Decoded `/Typewriter` catalog payload, not yet verified
    /// against the document's streams. Claimed (and verified) by
    /// [`PdfDoc::take_embedded_boxes`]; `None` for plain PDFs and
    /// for payloads that fail field validation.
    embedded: Option<embedded::Embedded>,
    doc: Document,
    page_ids: Vec<ObjectId>,
    /// Display size per page in points (rotation-aware).
    pub sizes_pt: Vec<(f32, f32)>,
    /// `(x0, y0, x1, y1)` effective visible box (`CropBox` ∩ `MediaBox`) per page.
    boxes_pt: Vec<(f32, f32, f32, f32)>,
    /// Clockwise `/Rotate` per page, normalized to `0..360`.
    rotations: Vec<i64>,
}

impl PdfDoc {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = Arc::new(
            std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
        );
        let doc = Document::load_mem(&bytes).context("failed to parse PDF")?;
        // Encrypted PDFs are unsupported, including owner-password-only
        // files (empty user password): lopdf silently decrypts those
        // with the empty password and strips the trailer `/Encrypt`
        // entry, recording the state in `encryption_state` instead, so
        // both leftovers must be checked. Otherwise an owner-only file
        // would open and a save would re-emit it unencrypted.
        if doc.encryption_state.is_some() || doc.trailer.get(b"Encrypt").is_ok() {
            bail!("Encrypted/password-protected PDFs are not supported");
        }
        let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
        if page_ids.is_empty() {
            bail!("PDF has no pages");
        }
        let mut boxes_pt = Vec::with_capacity(page_ids.len());
        let mut sizes_pt = Vec::with_capacity(page_ids.len());
        let mut rotations = Vec::with_capacity(page_ids.len());
        for (idx, page_id) in page_ids.iter().enumerate() {
            // MediaBox is required (inheritable) per the PDF spec;
            // silently assuming US Letter would bake/display text at
            // wrong coordinates, so a page with neither MediaBox nor
            // CropBox anywhere in its parent chain is an error.
            let page_box = page_box(&doc, *page_id)
                .ok_or_else(|| anyhow::anyhow!("page {} has no MediaBox or CropBox", idx + 1))?;
            boxes_pt.push(page_box);
            let (w, h) = (page_box.2 - page_box.0, page_box.3 - page_box.1);
            let rotate = inherited_int(&doc, *page_id, b"Rotate").unwrap_or(0);
            let rotate = rotate.rem_euclid(360);
            rotations.push(rotate);
            let rotated = matches!(rotate, 90 | 270);
            sizes_pt.push(if rotated { (h, w) } else { (w, h) });
        }
        // A field-valid `/Typewriter` payload means the baked
        // overlay streams in this file may be this app's own boxes.
        // It is decoded now but only claimed (verified against the
        // streams, then stripped) by `take_embedded_boxes`, so a
        // caller that opens and re-saves without claiming it keeps
        // the old behaviour.
        let embedded = embedded::extract(&doc, page_ids.len());
        let render_bytes = Arc::clone(&bytes);
        Ok(Self {
            bytes,
            render_bytes,
            embedded,
            doc,
            page_ids,
            sizes_pt,
            boxes_pt,
            rotations,
        })
    }

    /// Claim the boxes restored from the embedded payload, leaving
    /// none behind. The payload is trusted only after
    /// `embedded::verify` confirms every recorded baked stream is
    /// still present under its recorded object id with its
    /// recorded hash, and no unrecorded marked stream exists; on
    /// any mismatch this returns no boxes and strips nothing, so
    /// the document stays exactly a plain baked PDF. On success
    /// the verified streams are un-referenced in `doc` and
    /// `render_bytes` becomes the backing bytes plus an
    /// incremental update with only the changed page dictionaries
    /// (see `render_bytes_with_page_updates`), so the boxes render
    /// once (as live overlays, owned by the caller's
    /// `OverlayState`) and a later save re-bakes them instead of
    /// duplicating the old text. The file on disk is untouched
    /// until that save, whose incremental diff then persists the
    /// removal. The app calls this once, right after opening.
    pub fn take_embedded_boxes(&mut self) -> Vec<OverlayText> {
        let Some(embedded) = self.embedded.take() else {
            return Vec::new();
        };
        if !embedded::verify(&self.doc, &self.page_ids, &embedded) {
            return Vec::new();
        }
        let changed_pages = embedded::strip_verified(&mut self.doc, &self.page_ids, &embedded);
        if let Some(rendered) = self.render_bytes_with_page_updates(&changed_pages) {
            self.render_bytes = Arc::new(rendered);
        }
        // If the update cannot be written `render_bytes` stays as
        // the baked bytes: the preview draws boxes over their
        // copies rather than failing to open.
        embedded.boxes
    }

    pub fn page_count(&self) -> usize {
        self.page_ids.len()
    }

    /// Bake every non-empty overlay box into its page as a new content
    /// stream and save to `out` as an incremental update. On success
    /// the baked boxes are drained from `overlays` (second save
    /// therefore adds zero new streams).
    ///
    /// The document parsed from `bytes` is never re-serialized: only
    /// objects the bake added or changed are appended after the
    /// untouched original bytes, with the new trailer's `/Prev`
    /// chaining to the previous xref. The bake mutates `self.doc`
    /// (the merged logical document) directly instead of a staged
    /// clone; on any failure `self.doc` is restored by re-parsing
    /// the untouched `bytes`, `overlays` is left intact, and a retry
    /// cannot double-bake earlier pages. The file write itself is
    /// atomic (temp file in the target directory + `rename`), so an
    /// in-place save never leaves a truncated original behind. A
    /// document built in memory (no backing bytes, e.g. in tests)
    /// has nothing to append to, so its first save is a full write
    /// staged on a clone; that establishes the bytes later saves
    /// append to.
    pub fn bake_and_save(&mut self, overlays: &mut OverlayState, out: &Path) -> Result<()> {
        let mut by_page: BTreeMap<usize, Vec<&OverlayText>> = BTreeMap::new();
        for item in overlays.items.iter().filter(|item| !item.is_empty()) {
            // An out-of-range page is a caller bug, not a box to
            // silently drop: bail before staging anything, leaving
            // `overlays` intact so nothing is lost on `clear_baked`.
            if item.page >= self.page_ids.len() {
                bail!(
                    "overlay box is on page {} but the document has {} page(s)",
                    item.page + 1,
                    self.page_ids.len()
                );
            }
            by_page.entry(item.page).or_default().push(item);
        }
        // Every signature box in the bake: the one shared embedded
        // font's widths/`ToUnicode` are built from this union.
        let all_sig_boxes: Vec<&OverlayText> = by_page
            .values()
            .flat_map(|boxes| boxes.iter().copied())
            .filter(|item| item.kind == BoxKind::Signature)
            .collect();
        let sig_face = if all_sig_boxes.is_empty() {
            None
        } else {
            Some(signature_font::face()?)
        };
        // Subset the signature font to this bake's glyphs once, so a
        // single tiny subset serves every signed page. A subset can
        // never gain glyphs, so a reloaded document's earlier subsets
        // are left untouched and this bake embeds its own.
        let sig_font = sig_face
            .as_ref()
            .map(|face| signature_font::SignatureFont::prepare(face, &all_sig_boxes))
            .transpose()?;
        // Exactly the boxes being baked, kept for the embedded
        // payload that makes the saved file re-editable.
        let baked_boxes: Vec<OverlayText> = by_page
            .values()
            .flat_map(|boxes| boxes.iter().map(|item| (*item).clone()))
            .collect();
        let saved = if self.bytes.is_empty() {
            let mut staged = self.doc.clone();
            let baked_streams = Self::bake_pages(
                &mut staged,
                &self.page_ids,
                &self.boxes_pt,
                &self.rotations,
                &by_page,
                sig_font.as_ref(),
                sig_face.as_ref(),
            )?;
            embedded::store(&mut staged, &baked_boxes, &baked_streams)?;
            let mut saved = Vec::new();
            staged
                .save_to(&mut saved)
                .with_context(|| format!("failed to save {}", out.display()))?;
            Self::atomic_write(&saved, out)?;
            self.doc = staged;
            saved
        } else {
            match self.bake_and_save_incremental(
                &by_page,
                &baked_boxes,
                sig_font.as_ref(),
                sig_face.as_ref(),
                out,
            ) {
                Ok(saved) => saved,
                Err(err) => {
                    // Restore the merged document from the untouched
                    // backing bytes; if even that fails the save's
                    // error is the one the caller needs.
                    if let Ok(restored) = Document::load_mem(&self.bytes) {
                        self.doc = restored;
                    }
                    return Err(err);
                }
            }
        };
        self.bytes = Arc::new(saved);
        // The just-saved bytes render correctly as-is (the boxes
        // are baked in and the overlays are drained below); the app
        // reloads right after saving anyway.
        self.render_bytes = Arc::clone(&self.bytes);
        overlays.clear_baked();
        Ok(())
    }

    /// Return the resource alias of a usable standard-14 `WinAnsi`
    /// Helvetica on `page_id`, adding a fresh one if the page has none.
    ///
    /// Only fonts in the page's effective resources (see
    /// `effective_fonts`) are considered: an ancestor Helvetica the
    /// page cannot see must not be reused.
    ///
    /// A pre-existing `/Helvetica` is reused only when it is a
    /// `/Subtype /Type1` font with direct `/Encoding /WinAnsiEncoding`,
    /// no `/Widths`, and no embedded `/FontFile*` in its descriptor;
    /// otherwise `WinAnsi` bytes or standard AFM widths would disagree
    /// with the page font.
    fn ensure_helvetica(doc: &mut Document, page_id: ObjectId) -> Result<String> {
        let fonts = effective_fonts(doc, page_id);
        for (name, font) in &fonts {
            if is_standard_winansi_helvetica(doc, font) {
                return Ok(String::from_utf8_lossy(name).into_owned());
            }
        }
        let alias = free_alias(&fonts, "F_TW");
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        insert_font_entry(doc, page_id, alias.as_bytes(), font_id)?;
        Ok(alias)
    }

    /// Return the resource alias on `page_id` for the shared embedded
    /// Momo Signature font `type0_id`, reusing an existing alias when
    /// `page_id` already references it and inserting a new `F_SIG`
    /// entry otherwise.
    fn ensure_signature(
        doc: &mut Document,
        page_id: ObjectId,
        type0_id: ObjectId,
    ) -> Result<String> {
        if let Some(alias) = signature_font::signature_alias(doc, page_id, type0_id) {
            return Ok(alias);
        }
        let fonts = effective_fonts(doc, page_id);
        let alias = free_alias(&fonts, "F_SIG");
        insert_font_entry(doc, page_id, alias.as_bytes(), type0_id)?;
        Ok(alias)
    }
}

/// Whether `font` is an unmodified standard-14 Type1 Helvetica using
/// `WinAnsiEncoding` with no custom `/Widths` or embedded font program.
fn is_standard_winansi_helvetica(doc: &Document, font: &Dictionary) -> bool {
    let is_name = |key: &[u8], expected: &[u8]| {
        font.get(key)
            .ok()
            .and_then(|obj| obj.as_name().ok())
            .is_some_and(|n| n == expected)
    };
    if !is_name(b"BaseFont", b"Helvetica")
        || !is_name(b"Subtype", b"Type1")
        || !is_name(b"Encoding", b"WinAnsiEncoding")
        || font.has(b"Widths")
        || font.has(b"FontFile")
        || font.has(b"FontFile2")
        || font.has(b"FontFile3")
    {
        return false;
    }
    if let Ok(desc_obj) = font.get(b"FontDescriptor") {
        let Some(Object::Dictionary(desc)) = resolve(doc, desc_obj) else {
            return false;
        };
        if desc.has(b"FontFile") || desc.has(b"FontFile2") || desc.has(b"FontFile3") {
            return false;
        }
    }
    true
}

/// `base` if free on the page, else `base1`, `base2`, ...
fn free_alias(fonts: &std::collections::BTreeMap<Vec<u8>, &Dictionary>, base: &str) -> String {
    if !fonts.contains_key(base.as_bytes()) {
        return base.to_string();
    }
    let mut n = 1_u32;
    loop {
        let candidate = format!("{base}{n}");
        if !fonts.contains_key(candidate.as_bytes()) {
            return candidate;
        }
        n += 1;
    }
}

/// Build the `q BT ... ET Q` operations for one box.
fn box_operations(
    item: &OverlayText,
    alias: &str,
    page_box: (f32, f32, f32, f32),
    rotate: i64,
    sig_font: Option<&signature_font::SignatureFont>,
) -> Vec<Operation> {
    let size = item.size_pt;
    let leading = LEADING * size;
    let tm: Vec<Object> = text_matrix(item, page_box, rotate)
        .into_iter()
        .map(Object::Real)
        .collect();
    let mut ops = vec![
        Operation::new("q", vec![]),
        Operation::new("BT", vec![]),
        Operation::new(
            "rg",
            vec![
                Object::Real(f32::from(item.color[0]) / 255.0),
                Object::Real(f32::from(item.color[1]) / 255.0),
                Object::Real(f32::from(item.color[2]) / 255.0),
            ],
        ),
        Operation::new("Tf", vec![alias.into(), Object::Real(size)]),
        Operation::new("Tm", tm),
    ];
    let lines: Vec<Object> = match (item.kind, sig_font) {
        (BoxKind::Signature, Some(font)) => vec![Object::String(
            font.encode_hex(&item.signature_line()),
            StringFormat::Hexadecimal,
        )],
        (BoxKind::Text, _) if item.width_pt.is_some() => {
            // Fixed width: wrap with real Helvetica metrics so the
            // baked lines fit the box the user sized. The clamp
            // mirrors the preview's minimum box width.
            let width = item.width_pt.unwrap_or_default().max(24.0);
            let logical = crate::overlay::logical_lines(&item.text);
            wrap_lines(&logical, width, &|line: &str| {
                helvetica::helvetica_width_pt(line, size)
            })
            .iter()
            .map(|line| Object::String(winansi_bytes(line), StringFormat::Literal))
            .collect()
        }
        _ => winansi_lines(&item.text)
            .into_iter()
            .map(|line| Object::String(line, StringFormat::Literal))
            .collect(),
    };
    for (idx, line) in lines.into_iter().enumerate() {
        if idx > 0 {
            ops.push(Operation::new(
                "Td",
                vec![Object::Real(0.0), Object::Real(-leading)],
            ));
        }
        ops.push(Operation::new("Tj", vec![line]));
    }
    ops.push(Operation::new("ET", vec![]));
    ops.push(Operation::new("Q", vec![]));
    ops
}

/// Append the baked overlay `operations` to `page_id`, wrapping any
/// pre-existing page content streams in a `q` / `Q` pair first.
///
/// Content streams in a page's `/Contents` array are concatenated as a
/// single token stream, so without isolation the overlay would inherit
/// whatever graphics or text state the page left active (a residual
/// `cm` transform, `3 Tr` invisible text from OCR, `Tc`/`Tw`/`Tz`/`Ts`
/// spacing, a `W n` clipping path, or `ExtGState` alpha). Wrapping the
/// existing streams between a prepended `q` stream and an appended `Q`
/// stream restores the initial page graphics state before the overlay
/// runs, without decoding or rewriting the original streams.
fn append_overlay_stream(
    doc: &mut Document,
    page_id: ObjectId,
    operations: Vec<Operation>,
) -> Result<ObjectId> {
    let existing = doc.get_page_contents(page_id);
    // Every baked stream carries the marker the embedded payload's
    // hash verification looks for when the boxes are claimed.
    let mut marked_bytes = embedded::OVERLAY_MARKER.to_vec();
    marked_bytes.extend_from_slice(&Content::encode(&Content { operations })?);
    if existing.is_empty() {
        let overlay_id = doc.add_object(Stream::new(Dictionary::new(), marked_bytes));
        doc.get_dictionary_mut(page_id)?
            .set("Contents", Object::Reference(overlay_id));
        return Ok(overlay_id);
    }
    let overlay_bytes = marked_bytes;
    // Marked like the overlay, so a later open strips the pair
    // with it instead of nesting a new pair around the old one
    // on every save (see `embedded::strip_verified`).
    let save_id = doc.add_object(Stream::new(Dictionary::new(), embedded::WRAP_SAVE.to_vec()));
    let restore_id = doc.add_object(Stream::new(
        Dictionary::new(),
        embedded::WRAP_RESTORE.to_vec(),
    ));
    let overlay_id = doc.add_object(Stream::new(Dictionary::new(), overlay_bytes));
    let mut contents = Vec::with_capacity(existing.len() + 3);
    contents.push(Object::Reference(save_id));
    contents.extend(existing.into_iter().map(Object::Reference));
    contents.push(Object::Reference(restore_id));
    contents.push(Object::Reference(overlay_id));
    doc.get_dictionary_mut(page_id)?
        .set("Contents", Object::Array(contents));
    Ok(overlay_id)
}

mod bounded;
mod embedded;
mod helvetica;
mod page_tree;
mod save;
mod signature_font;
mod winansi;

pub(crate) use helvetica::helvetica_width_pt;
pub use winansi::{winansi_bytes, winansi_lines};

#[cfg(test)]
mod embedded_tests;
#[cfg(test)]
mod encrypted_tests;
#[cfg(test)]
mod incremental_tests;
#[cfg(test)]
mod signature_tests;
#[cfg(test)]
mod tests;
