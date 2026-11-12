//! PDF document model and append-only overlay writer.
//!
//! Original page content is never decoded or rewritten: baked text is
//! appended as a new content stream per page via lopdf's
//! `add_to_page_content`. Existing Helvetica fonts are reused; a new
//! standard-14 Helvetica is added only when the page has none.

use crate::overlay::{ASCENT, BoxKind, LEADING, OverlayState, OverlayText};
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
    /// `(x0, y0, x1, y1)` `MediaBox` (fallback `CropBox`) per page.
    boxes_pt: Vec<(f32, f32, f32, f32)>,
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
            let rotated = matches!(((rotate % 360) + 360) % 360, 90 | 270);
            sizes_pt.push(if rotated { (h, w) } else { (w, h) });
        }
        Ok(Self {
            bytes,
            doc,
            page_ids,
            sizes_pt,
            boxes_pt,
        })
    }

    pub fn page_count(&self) -> usize {
        self.page_ids.len()
    }

    /// Bake every non-empty overlay box into its page as a new content
    /// stream and save to `out`. On success the baked boxes are drained
    /// from `overlays` (second save therefore adds zero new streams).
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
        for (page, boxes) in by_page {
            let page_id = self.page_ids[page];
            let page_box = self.boxes_pt[page];
            let mut operations = Vec::new();
            let text_boxes: Vec<&OverlayText> = boxes
                .iter()
                .copied()
                .filter(|item| item.kind == BoxKind::Text)
                .collect();
            if !text_boxes.is_empty() {
                let alias = self.ensure_helvetica(page_id)?;
                for item in text_boxes {
                    operations.extend(box_operations(item, &alias, page_box, None));
                }
            }
            let sig_boxes: Vec<&OverlayText> = boxes
                .iter()
                .copied()
                .filter(|item| item.kind == BoxKind::Signature)
                .collect();
            if let (false, Some(face)) = (sig_boxes.is_empty(), sig_face.as_ref()) {
                let alias = self.ensure_signature(page_id, face, &sig_boxes)?;
                for item in sig_boxes {
                    operations.extend(box_operations(item, &alias, page_box, Some(face)));
                }
            }
            self.doc
                .add_to_page_content(page_id, Content { operations })
                .context("failed to append overlay content stream")?;
        }
        self.doc
            .save(out)
            .with_context(|| format!("failed to save {}", out.display()))?;
        overlays.clear_baked();
        Ok(())
    }

    /// Return the resource alias of a usable Helvetica on `page_id`,
    /// adding a standard-14 Helvetica if the page has none.
    fn ensure_helvetica(&mut self, page_id: ObjectId) -> Result<String> {
        let fonts = self
            .doc
            .get_page_fonts(page_id)
            .context("failed to read page fonts")?;
        for (name, font) in &fonts {
            if let Ok(base) = font.get(b"BaseFont")
                && base.as_name().is_ok_and(|n| n == b"Helvetica")
            {
                return Ok(String::from_utf8_lossy(name).into_owned());
            }
        }
        let alias = if fonts.contains_key(b"F_TW".as_slice()) {
            let mut n = 1_u32;
            loop {
                let candidate = format!("F_TW{n}");
                if !fonts.contains_key(candidate.as_bytes()) {
                    break candidate;
                }
                n += 1;
            }
        } else {
            "F_TW".to_string()
        };
        let font_id = self.doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        insert_font_entry(&mut self.doc, page_id, alias.as_bytes(), font_id)?;
        Ok(alias)
    }

    /// Return the resource alias of the embedded Momo Signature font on
    /// `page_id`, embedding it (once per page) if not already present.
    fn ensure_signature(
        &mut self,
        page_id: ObjectId,
        face: &ttf_parser::Face<'_>,
        boxes: &[&OverlayText],
    ) -> Result<String> {
        let fonts = self
            .doc
            .get_page_fonts(page_id)
            .context("failed to read page fonts")?;
        for (name, font) in &fonts {
            if let Ok(base) = font.get(b"BaseFont")
                && base.as_name().is_ok_and(|n| n == b"MomoSignature")
            {
                return Ok(String::from_utf8_lossy(name).into_owned());
            }
        }
        let alias = free_alias(&fonts, "F_SIG");
        signature_font::ensure_signature_font(&mut self.doc, page_id, face, boxes, alias)
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

/// Top-left-origin baseline: PDF y of the first line's baseline.
#[must_use]
pub fn baseline_y(y1: f32, y_top_pt: f32, size_pt: f32) -> f32 {
    y1 - y_top_pt - ASCENT * size_pt
}

/// Build the `q BT ... ET Q` operations for one box.
fn box_operations(
    item: &OverlayText,
    alias: &str,
    page_box: (f32, f32, f32, f32),
    sig_face: Option<&ttf_parser::Face<'_>>,
) -> Vec<Operation> {
    let (x0, _y0, _x1, y1) = page_box;
    let size = item.size_pt;
    let leading = LEADING * size;
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
        Operation::new(
            "Tm",
            vec![
                Object::Real(1.0),
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(1.0),
                Object::Real(x0 + item.x_pt),
                Object::Real(baseline_y(y1, item.y_top_pt, size)),
            ],
        ),
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

/// Walk the `Parent` chain looking for an inherited array entry.
fn inherited_array(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<[f32; 4]> {
    let mut current = Some(page_id);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            return None;
        }
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(obj) = dict.get(key)
            && let Some(Object::Array(items)) = resolve(doc, obj)
            && items.len() >= 4
        {
            let mut out = [0.0_f32; 4];
            for (slot, item) in out.iter_mut().zip(items.iter()) {
                *slot = resolve(doc, item)?.as_float().ok()?;
            }
            return Some(out);
        }
        current = dict
            .get(b"Parent")
            .ok()
            .and_then(|obj| obj.as_reference().ok());
    }
    None
}

fn inherited_int(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<i64> {
    let mut current = Some(page_id);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            return None;
        }
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(obj) = dict.get(key)
            && let Some(value) = resolve(doc, obj)
            && let Ok(v) = value.as_i64()
        {
            return Some(v);
        }
        current = dict
            .get(b"Parent")
            .ok()
            .and_then(|obj| obj.as_reference().ok());
    }
    None
}

fn page_box(doc: &Document, page_id: ObjectId) -> Option<(f32, f32, f32, f32)> {
    let [x0, y0, x1, y1] = inherited_array(doc, page_id, b"MediaBox")
        .or_else(|| inherited_array(doc, page_id, b"CropBox"))?;
    Some((x0, y0, x1, y1))
}

#[derive(Clone, Copy)]
enum ResourcesTarget {
    /// Resources is a referenced object.
    Reference(ObjectId),
    /// Resources is a direct dictionary on this tree node.
    Direct(ObjectId),
}

fn find_resources_target(doc: &Document, page_id: ObjectId) -> Option<ResourcesTarget> {
    let mut current = Some(page_id);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            return None;
        }
        let dict = doc.get_dictionary(id).ok()?;
        match dict.get(b"Resources") {
            Ok(Object::Reference(res_id)) => return Some(ResourcesTarget::Reference(*res_id)),
            Ok(Object::Dictionary(_)) => return Some(ResourcesTarget::Direct(id)),
            _ => {}
        }
        current = dict
            .get(b"Parent")
            .ok()
            .and_then(|obj| obj.as_reference().ok());
    }
    None
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
