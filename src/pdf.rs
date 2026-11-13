//! PDF document model and append-only overlay writer.
//!
//! Original page content is never decoded or rewritten: baked text is
//! appended as a new content stream per page via lopdf's
//! `add_to_page_content`. Existing Helvetica fonts are reused; a new
//! standard-14 Helvetica is added only when the page has none.

use crate::overlay::{BoxKind, LEADING, OverlayState, OverlayText, text_matrix};
use anyhow::{Context, Result, bail};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat, dictionary};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// Loaded PDF: sole owner of the file bytes in the application.
pub struct PdfDoc {
    pub bytes: Vec<u8>,
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
        let bytes =
            std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        let doc = Document::load_mem(&bytes).context("failed to parse PDF")?;
        let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
        if page_ids.is_empty() {
            bail!("PDF has no pages");
        }
        let mut boxes_pt = Vec::with_capacity(page_ids.len());
        let mut sizes_pt = Vec::with_capacity(page_ids.len());
        let mut rotations = Vec::with_capacity(page_ids.len());
        let mut hayro_sizes: Option<Vec<(f32, f32)>> = None;
        for (idx, page_id) in page_ids.iter().enumerate() {
            let page_box = page_box(&doc, *page_id).unwrap_or_else(|| {
                if hayro_sizes.is_none() {
                    hayro_sizes = Some(hayro_page_sizes(&bytes));
                }
                hayro_sizes
                    .as_ref()
                    .and_then(|sizes| sizes.get(idx).copied())
                    .map_or((0.0, 0.0, 612.0, 792.0), |(w, h)| (0.0, 0.0, w, h))
            });
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
    /// retry cannot double-bake earlier pages.
    pub fn bake_and_save(&mut self, overlays: &mut OverlayState, out: &Path) -> Result<()> {
        let mut by_page: BTreeMap<usize, Vec<&OverlayText>> = BTreeMap::new();
        for item in overlays.items.iter().filter(|item| !item.is_empty()) {
            if item.page < self.page_ids.len() {
                by_page.entry(item.page).or_default().push(item);
            }
        }
        let sig_face = if overlays
            .items
            .iter()
            .any(|item| item.kind == BoxKind::Signature && !item.is_empty())
        {
            Some(signature_font::face()?)
        } else {
            None
        };
        // Every signature box in the bake: the one shared embedded
        // font's widths/`ToUnicode` are built from this union.
        let all_sig_boxes: Vec<&OverlayText> = by_page
            .values()
            .flat_map(|boxes| boxes.iter().copied())
            .filter(|item| item.kind == BoxKind::Signature)
            .collect();
        let mut staged = self.doc.clone();
        // Object id of the shared Type0 signature font, created on
        // first use in this bake and reused on every signed page so
        // the font program is embedded once per document.
        let mut shared_sig_font: Option<ObjectId> = None;
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
            if let (false, Some(face)) = (sig_boxes.is_empty(), sig_face.as_ref()) {
                let alias = Self::ensure_signature(
                    &mut staged,
                    page_id,
                    face,
                    &all_sig_boxes,
                    &mut shared_sig_font,
                )?;
                for item in sig_boxes {
                    operations.extend(box_operations(item, &alias, page_box, rotate, Some(face)));
                }
            }
            staged
                .add_to_page_content(page_id, Content { operations })
                .context("failed to append overlay content stream")?;
        }
        staged
            .save(out)
            .with_context(|| format!("failed to save {}", out.display()))?;
        self.doc = staged;
        overlays.clear_baked();
        Ok(())
    }

    /// Return the resource alias of a usable Helvetica on `page_id`,
    /// adding a standard-14 Helvetica if the page has none.
    fn ensure_helvetica(doc: &mut Document, page_id: ObjectId) -> Result<String> {
        let fonts = doc
            .get_page_fonts(page_id)
            .context("failed to read page fonts")?;
        for (name, font) in &fonts {
            if let Ok(base) = font.get(b"BaseFont")
                && base.as_name().is_ok_and(|n| n == b"Helvetica")
            {
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

    /// Return the resource alias of the embedded Momo Signature font on
    /// `page_id`, extending it with glyphs from `all_boxes` when it
    /// already exists (on this page or elsewhere in the document, e.g.
    /// after a save/reload): reusing a subset font as-is would bake
    /// new characters at the default width with no `ToUnicode`
    /// mapping. A font present only on another page is referenced
    /// from this page instead of embedding a second copy. Otherwise
    /// the shared Type0 font in `shared` is referenced from this
    /// page, creating it (from `all_boxes`, the union of every
    /// signature box in the bake) on first use: the font program is
    /// embedded once per document, not once per page.
    fn ensure_signature(
        doc: &mut Document,
        page_id: ObjectId,
        face: &ttf_parser::Face<'_>,
        all_boxes: &[&OverlayText],
        shared: &mut Option<ObjectId>,
    ) -> Result<String> {
        if let Some((alias, type0_id)) = signature_font::signature_font_on_page(doc, page_id) {
            signature_font::extend_signature_font(doc, type0_id, face, all_boxes)?;
            *shared = Some(type0_id);
            return Ok(alias);
        }
        if let Some(type0_id) = shared.or_else(|| signature_font::find_signature_font(doc)) {
            signature_font::extend_signature_font(doc, type0_id, face, all_boxes)?;
            *shared = Some(type0_id);
            let fonts = doc
                .get_page_fonts(page_id)
                .context("failed to read page fonts")?;
            let alias = free_alias(&fonts, "F_SIG");
            insert_font_entry(doc, page_id, alias.as_bytes(), type0_id)?;
            return Ok(alias);
        }
        let fonts = doc
            .get_page_fonts(page_id)
            .context("failed to read page fonts")?;
        let alias = free_alias(&fonts, "F_SIG");
        let type0_id = *shared
            .get_or_insert_with(|| signature_font::create_signature_font(doc, face, all_boxes));
        insert_font_entry(doc, page_id, alias.as_bytes(), type0_id)?;
        Ok(alias)
    }
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

/// Split text into WinAnsi-encoded lines; a single trailing empty line
/// (from a final newline) is dropped so it emits no `Tj`.
#[must_use]
pub fn winansi_lines(text: &str) -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = text.split('\n').map(winansi_bytes).collect();
    if lines.len() > 1 && lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    lines
}

/// Encode a string as `WinAnsi` bytes: one byte per character, `?` for
/// unmappable characters. `\r` is stripped, other control characters
/// below 0x20 (except `\n`) are dropped. lopdf escapes `\ ( )` itself
/// when writing literal strings, so no pre-escaping happens here.
#[must_use]
pub fn winansi_bytes(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\r' => {}
            '\n' => out.push(b'\n'),
            c if (c as u32) < 0x20 => {}
            c => out.push(winansi_char(c)),
        }
    }
    out
}

fn winansi_char(c: char) -> u8 {
    let code = c as u32;
    if (0x20..=0x7E).contains(&code) || (0xA0..=0xFF).contains(&code) {
        #[allow(clippy::cast_possible_truncation)]
        return code as u8;
    }
    // WinAnsi 0x80..=0x9F punctuation block.
    match c {
        '\u{20AC}' => 0x80,
        '\u{201A}' => 0x82,
        '\u{0192}' => 0x83,
        '\u{201E}' => 0x84,
        '\u{2026}' => 0x85,
        '\u{2020}' => 0x86,
        '\u{2021}' => 0x87,
        '\u{02C6}' => 0x88,
        '\u{2030}' => 0x89,
        '\u{0160}' => 0x8A,
        '\u{2039}' => 0x8B,
        '\u{0152}' => 0x8C,
        '\u{017D}' => 0x8E,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201C}' => 0x93,
        '\u{201D}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{02DC}' => 0x98,
        '\u{2122}' => 0x99,
        '\u{0161}' => 0x9A,
        '\u{203A}' => 0x9B,
        '\u{0153}' => 0x9C,
        '\u{017E}' => 0x9E,
        '\u{0178}' => 0x9F,
        _ => b'?',
    }
}

fn hayro_page_sizes(bytes: &[u8]) -> Vec<(f32, f32)> {
    let Ok(pdf) = hayro::hayro_syntax::Pdf::new(bytes.to_vec()) else {
        return Vec::new();
    };
    pdf.pages()
        .iter()
        .map(hayro::hayro_syntax::page::Page::render_dimensions)
        .collect()
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

mod signature_font;

#[cfg(test)]
mod tests;
