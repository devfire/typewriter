//! PDF document model and append-only overlay writer.
//!
//! Original page content is never decoded or rewritten: baked text is
//! appended as a new content stream per page via lopdf's
//! `add_to_page_content`. Existing Helvetica fonts are reused; a new
//! standard-14 Helvetica is added only when the page has none.

use crate::overlay::{BoxKind, LEADING, OverlayState, OverlayText, text_matrix, wrap_lines};
use anyhow::{Context, Result, bail};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat, dictionary};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Counter for unique temp-file names used by atomic saves.
static SAVE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Loaded PDF. The raw file bytes live in one `Arc<Vec<u8>>` shared
/// with hayro's renderer (`PdfData: From<Arc<Vec<u8>>>`), so no
/// full-file clone is retained; lopdf separately holds its own
/// parsed representation in `doc`.
pub struct PdfDoc {
    pub bytes: Arc<Vec<u8>>,
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
        Ok(Self {
            bytes,
            doc,
            page_ids,
            sizes_pt,
            boxes_pt,
            rotations,
        })
    }

    pub fn page_count(&self) -> usize {
        self.page_ids.len()
    }

    /// Bake every non-empty overlay box into its page as a new content
    /// stream and save to `out`. On success the baked boxes are drained
    /// from `overlays` (second save therefore adds zero new streams).
    ///
    /// The bake is staged on a clone of the document and swapped into
    /// `self` only after the save succeeds, so a failure on any page or
    /// during the write leaves `self.doc` and `overlays` unchanged and a
    /// retry cannot double-bake earlier pages. The file write itself is
    /// atomic (temp file in the target directory + `rename`), so an
    /// in-place save never leaves a truncated original behind.
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
        let mut staged = self.doc.clone();
        // Prepare the single shared Type0 signature font once per bake
        // (creating it or extending an already-embedded one with the
        // union of `all_sig_boxes`) so the font program is embedded at
        // most once per document and `/W` + `ToUnicode` are not
        // re-parsed on every signed page.
        let shared_sig_font = if let Some(face) = sig_face.as_ref() {
            let type0_id = if let Some(existing) = signature_font::find_signature_font(&staged) {
                signature_font::extend_signature_font(&mut staged, existing, face, &all_sig_boxes)?;
                existing
            } else {
                signature_font::create_signature_font(&mut staged, face, &all_sig_boxes)
            };
            Some(type0_id)
        } else {
            None
        };
        for (page, boxes) in by_page {
            let page_id = self.page_ids[page];
            let page_box = self.boxes_pt[page];
            let rotate = self.rotations[page];
            let mut operations = Vec::new();
            let text_boxes: Vec<&OverlayText> = boxes
                .iter()
                .copied()
                .filter(|item| item.kind == BoxKind::Text)
                .collect();
            if !text_boxes.is_empty() {
                let alias = Self::ensure_helvetica(&mut staged, page_id)?;
                for item in text_boxes {
                    operations.extend(box_operations(item, &alias, page_box, rotate, None));
                }
            }
            let sig_boxes: Vec<&OverlayText> = boxes
                .iter()
                .copied()
                .filter(|item| item.kind == BoxKind::Signature)
                .collect();
            if let (false, Some(face), Some(type0_id)) =
                (sig_boxes.is_empty(), sig_face.as_ref(), shared_sig_font)
            {
                let alias = Self::ensure_signature(&mut staged, page_id, type0_id)?;
                for item in sig_boxes {
                    operations.extend(box_operations(item, &alias, page_box, rotate, Some(face)));
                }
            }
            append_overlay_stream(&mut staged, page_id, operations)
                .context("failed to append overlay content stream")?;
        }
        Self::atomic_save(&mut staged, out)?;
        self.doc = staged;
        overlays.clear_baked();
        Ok(())
    }

    /// Write `staged` to `out` without ever truncating `out` in place.
    ///
    /// The document is saved to a unique temp file in the same directory
    /// (so the final `rename` stays on one filesystem and is atomic),
    /// `sync_all`-ed, then renamed over `out`. A failed write leaves the
    /// original intact and removes the temp file. When `out` already
    /// exists, its permissions are re-applied to the replacement.
    fn atomic_save(staged: &mut Document, out: &Path) -> Result<()> {
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
            staged
                .save(&tmp)
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

    /// Return the resource alias of a usable standard-14 `WinAnsi`
    /// Helvetica on `page_id`, adding a fresh one if the page has none.
    ///
    /// A pre-existing `/Helvetica` is reused only when it is a
    /// `/Subtype /Type1` font with direct `/Encoding /WinAnsiEncoding`,
    /// no `/Widths`, and no embedded `/FontFile*` in its descriptor;
    /// otherwise `WinAnsi` bytes or standard AFM widths would disagree
    /// with the page font.
    fn ensure_helvetica(doc: &mut Document, page_id: ObjectId) -> Result<String> {
        let fonts = doc
            .get_page_fonts(page_id)
            .context("failed to read page fonts")?;
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
        if let Some((alias, _)) = signature_font::signature_font_on_page(doc, page_id) {
            return Ok(alias);
        }
        let fonts = doc
            .get_page_fonts(page_id)
            .context("failed to read page fonts")?;
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
    sig_face: Option<&ttf_parser::Face<'_>>,
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
    let lines: Vec<Object> = match (item.kind, sig_face) {
        (BoxKind::Signature, Some(face)) => vec![Object::String(
            signature_font::encode_hex(face, &item.signature_line()),
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
) -> Result<()> {
    let existing = doc.get_page_contents(page_id);
    if existing.is_empty() {
        doc.add_to_page_content(page_id, Content { operations })?;
        return Ok(());
    }
    let overlay_bytes = Content::encode(&Content { operations })?;
    let save_id = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    // Leading newline guards against an existing stream that does not
    // end with whitespace when viewers or `get_page_content` concatenate
    // stream bytes directly.
    let restore_id = doc.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));
    let overlay_id = doc.add_object(Stream::new(Dictionary::new(), overlay_bytes));
    let mut contents = Vec::with_capacity(existing.len() + 3);
    contents.push(Object::Reference(save_id));
    contents.extend(existing.into_iter().map(Object::Reference));
    contents.push(Object::Reference(restore_id));
    contents.push(Object::Reference(overlay_id));
    doc.get_dictionary_mut(page_id)?
        .set("Contents", Object::Array(contents));
    Ok(())
}

fn resolve<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
    match obj {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

/// Outcome of visiting one node while walking a `Parent` chain.
enum ParentVisit<T> {
    /// Value found; stop walking and return it.
    Found(T),
    /// No usable value on this node; walk to its parent.
    Continue,
    /// Malformed value on this node; abort instead of inheriting.
    Stop,
}

/// Walk the `Parent` chain from `page_id`, visiting each node's
/// dictionary until `visit` finds a value or aborts. Cycles and
/// non-dictionary nodes end the walk with `None`.
fn parent_chain_find<T>(
    doc: &Document,
    page_id: ObjectId,
    mut visit: impl FnMut(ObjectId, &Dictionary) -> ParentVisit<T>,
) -> Option<T> {
    let mut current = Some(page_id);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            return None;
        }
        let dict = doc.get_dictionary(id).ok()?;
        match visit(id, dict) {
            ParentVisit::Found(value) => return Some(value),
            ParentVisit::Continue => {}
            ParentVisit::Stop => return None,
        }
        current = dict
            .get(b"Parent")
            .ok()
            .and_then(|obj| obj.as_reference().ok());
    }
    None
}

/// Walk the `Parent` chain looking for an inherited array entry.
/// A present-but-malformed array aborts the walk rather than
/// silently inheriting an ancestor's box.
fn inherited_array(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<[f32; 4]> {
    parent_chain_find(doc, page_id, |_id, dict| {
        let Ok(obj) = dict.get(key) else {
            return ParentVisit::Continue;
        };
        let Some(Object::Array(items)) = resolve(doc, obj) else {
            return ParentVisit::Continue;
        };
        if items.len() < 4 {
            return ParentVisit::Continue;
        }
        let mut out = [0.0_f32; 4];
        for (slot, item) in out.iter_mut().zip(items.iter()) {
            let Some(value) = resolve(doc, item).and_then(|obj| obj.as_float().ok()) else {
                return ParentVisit::Stop;
            };
            *slot = value;
        }
        ParentVisit::Found(out)
    })
}

fn inherited_int(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<i64> {
    parent_chain_find(doc, page_id, |_id, dict| {
        dict.get(key)
            .ok()
            .and_then(|obj| resolve(doc, obj))
            .and_then(|value| value.as_i64().ok())
            .map_or(ParentVisit::Continue, ParentVisit::Found)
    })
}

/// Effective visible page box: the `CropBox` intersected with the
/// `MediaBox`, matching what hayro renders. `CropBox` defaults to
/// `MediaBox` when absent (and vice versa). Disjoint boxes clamp to a
/// zero-area box at the max origin, mirroring hayro's intersection.
fn page_box(doc: &Document, page_id: ObjectId) -> Option<(f32, f32, f32, f32)> {
    let media = inherited_array(doc, page_id, b"MediaBox");
    let crop = inherited_array(doc, page_id, b"CropBox");
    match (media, crop) {
        (Some([mx0, my0, mx1, my1]), Some([cx0, cy0, cx1, cy1])) => {
            let x0 = mx0.max(cx0);
            let y0 = my0.max(cy0);
            Some((x0, y0, mx1.min(cx1).max(x0), my1.min(cy1).max(y0)))
        }
        (Some([x0, y0, x1, y1]), None) | (None, Some([x0, y0, x1, y1])) => Some((x0, y0, x1, y1)),
        (None, None) => None,
    }
}

#[derive(Clone, Copy)]
enum ResourcesTarget {
    /// Resources is a referenced object.
    Reference(ObjectId),
    /// Resources is a direct dictionary on this tree node.
    Direct(ObjectId),
}

fn find_resources_target(doc: &Document, page_id: ObjectId) -> Option<ResourcesTarget> {
    parent_chain_find(doc, page_id, |id, dict| match dict.get(b"Resources") {
        Ok(Object::Reference(res_id)) => ParentVisit::Found(ResourcesTarget::Reference(*res_id)),
        Ok(Object::Dictionary(_)) => ParentVisit::Found(ResourcesTarget::Direct(id)),
        _ => ParentVisit::Continue,
    })
}

fn resources_dict_mut(doc: &mut Document, target: ResourcesTarget) -> Result<&mut Dictionary> {
    match target {
        ResourcesTarget::Reference(id) => Ok(doc.get_dictionary_mut(id)?),
        ResourcesTarget::Direct(node_id) => {
            let node = doc.get_dictionary_mut(node_id)?;
            let resources = node.get_mut(b"Resources")?;
            Ok(resources.as_dict_mut()?)
        }
    }
}

/// Insert `alias -> font_id` into the page's effective resources
/// without shadowing inherited entries: an inherited resources
/// dictionary is mutated in place, and a page-local one is created
/// only when no ancestor defines resources at all.
fn insert_font_entry(
    doc: &mut Document,
    page_id: ObjectId,
    alias: &[u8],
    font_id: ObjectId,
) -> Result<()> {
    let Some(target) = find_resources_target(doc, page_id) else {
        let resources = doc.get_or_create_resources(page_id)?;
        let resources = resources.as_dict_mut()?;
        let mut font = Dictionary::new();
        font.set(alias.to_vec(), Object::Reference(font_id));
        resources.set("Font", font);
        return Ok(());
    };
    // Font sub-dictionary may itself be a reference; resolve first so
    // the borrow of the resources dictionary is not held across it.
    let font_ref = {
        let resources = resources_dict_mut(doc, target)?;
        match resources.get(b"Font") {
            Ok(Object::Reference(id)) => Some(*id),
            _ => None,
        }
    };
    if let Some(font_dict_id) = font_ref {
        doc.get_dictionary_mut(font_dict_id)?
            .set(alias.to_vec(), Object::Reference(font_id));
        return Ok(());
    }
    let resources = resources_dict_mut(doc, target)?;
    if let Ok(font_obj) = resources.get_mut(b"Font") {
        font_obj
            .as_dict_mut()?
            .set(alias.to_vec(), Object::Reference(font_id));
    } else {
        let mut font = Dictionary::new();
        font.set(alias.to_vec(), Object::Reference(font_id));
        resources.set("Font", font);
    }
    Ok(())
}

mod helvetica;
mod signature_font;
mod winansi;

pub use winansi::{winansi_bytes, winansi_lines};

#[cfg(test)]
mod signature_tests;
#[cfg(test)]
mod tests;
