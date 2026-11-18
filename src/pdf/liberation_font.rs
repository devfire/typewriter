//! Embedding Liberation Sans into baked PDFs for non-`WinAnsi` text.
//!
//! Ordinary text bakes as standard-14 Helvetica, whose `WinAnsi`
//! encoding cannot represent e.g. Cyrillic: the encoder substituted
//! `?`, so the saved file silently differed from the preview. A text
//! box containing any character `WinAnsi` cannot represent instead
//! bakes in a subset of the bundled Liberation Sans — the same face
//! the preview draws — as a Type0 font (`Identity-H`, subset glyph
//! ids as CIDs, `ToUnicode` for extraction), mirroring
//! `signature_font`. Characters Liberation Sans itself lacks are a
//! save error, never a `?`: the preview would show a fallback glyph
//! the baked file could not reproduce.

use super::signature_font::{to_unicode_cmap, width_1000};
use crate::overlay::OverlayText;

/// Raw Liberation Sans TrueType bytes (also drawn by the preview via
/// `crate::text_font`, which re-exports this constant).
pub(crate) const FONT_BYTES: &[u8] = include_bytes!("../../assets/LiberationSans-Regular.ttf");
use anyhow::{Context, Result, bail};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use subsetter::GlyphRemapper;
use ttf_parser::Face;

fn face() -> Result<Face<'static>> {
    Face::parse(FONT_BYTES, 0).context("failed to parse bundled Liberation Sans font")
}

/// Parsed Liberation face, cached for preview width measurement.
// Only reachable via `liberation_width_pt` (binary preview path).
#[allow(dead_code)]
fn cached_face() -> &'static Face<'static> {
    static FACE: OnceLock<Face<'static>> = OnceLock::new();
    FACE.get_or_init(|| face().expect("bundled Liberation Sans font must parse"))
}

/// Rendered width of `text` in points at `size_pt` in Liberation Sans.
/// Characters without a glyph measure as `?`, matching `encode_hex`'s
/// caller contract (such text is rejected at save, so this only
/// affects the transient preview).
// Used by the binary's preview; the library target has no caller.
#[allow(dead_code)]
#[must_use]
pub(crate) fn liberation_width_pt(text: &str, size_pt: f32) -> f32 {
    let face = cached_face();
    let units_per_em = f32::from(face.units_per_em());
    let units: f32 = text
        .chars()
        .map(|c| {
            let gid = face
                .glyph_index(c)
                .or_else(|| face.glyph_index('?'))
                .unwrap_or_default();
            f32::from(face.glyph_hor_advance(gid).unwrap_or(500))
        })
        .sum();
    if units_per_em == 0.0 {
        return 0.0;
    }
    units * size_pt / units_per_em
}

/// Whether `item` must bake in Liberation Sans rather than Helvetica.
#[must_use]
pub(super) fn needs_liberation(item: &OverlayText) -> bool {
    !super::winansi::is_winansi_encodable(&item.text)
}

/// Font name for one bake's subset: `ABCDEF+LiberationSans` (see
/// `signature_font::subset_name` for the tagging rationale).
fn subset_name(old_ids: &[u16]) -> String {
    let mut hasher = Sha256::new();
    for id in old_ids {
        hasher.update(id.to_be_bytes());
    }
    let digest = hasher.finalize();
    let mut name = String::with_capacity(6 + 1 + "LiberationSans".len());
    for byte in &digest[..6] {
        name.push(char::from(b'A' + byte % 26));
    }
    name.push('+');
    name.push_str("LiberationSans");
    name
}

/// One bake's subsetted Liberation Sans font, serving every text box
/// in the bake that `WinAnsi` cannot represent.
pub(super) struct LiberationFont {
    face: Face<'static>,
    name: String,
    /// Source char -> glyph id in the subset.
    encoding: BTreeMap<char, u16>,
    /// Subset glyph id -> (source char, width in 1000-unit em space).
    used: BTreeMap<u16, (char, i64)>,
    subset: Vec<u8>,
}

impl LiberationFont {
    /// Subset Liberation Sans to the glyphs used by `boxes`.
    ///
    /// # Errors
    /// Returns an error naming the first character the face has no
    /// glyph for, so the save fails visibly instead of baking `?`.
    pub(super) fn prepare(boxes: &[&OverlayText]) -> Result<Self> {
        let face = face()?;
        let units_per_em = face.units_per_em();
        let mut used_old: BTreeMap<u16, (char, i64)> = BTreeMap::new();
        for item in boxes {
            for line in crate::overlay::logical_lines(&item.text) {
                for c in line.chars() {
                    let Some(gid) = face.glyph_index(c) else {
                        bail!(
                            "text contains '{}' (U+{:04X}), which cannot be saved: \
                             it is outside WinAnsi and the embedded Liberation Sans \
                             font has no glyph for it; remove or replace it before saving",
                            c,
                            u32::from(c)
                        );
                    };
                    let width = face
                        .glyph_hor_advance(gid)
                        .map_or(500, |advance| width_1000(advance, units_per_em));
                    used_old.entry(gid.0).or_insert((c, width));
                }
            }
        }
        let old_ids: Vec<u16> = used_old.keys().copied().collect();
        let remapper = GlyphRemapper::new_from_glyphs_sorted(&old_ids);
        let subset = subsetter::subset(FONT_BYTES, 0, &remapper)
            .context("failed to subset bundled Liberation Sans font")?;
        let mut encoding = BTreeMap::new();
        for item in boxes {
            for line in crate::overlay::logical_lines(&item.text) {
                for c in line.chars() {
                    if let Some(gid) = face.glyph_index(c) {
                        encoding
                            .entry(c)
                            .or_insert_with(|| remapper.get(gid.0).unwrap_or(0));
                    }
                }
            }
        }
        let used = used_old
            .iter()
            .map(|(old, (c, width))| (remapper.get(*old).unwrap_or(0), (*c, *width)))
            .collect();
        Ok(Self {
            face,
            name: subset_name(&old_ids),
            encoding,
            used,
            subset,
        })
    }

    /// Encode one logical line as `Identity-H` bytes (big-endian
    /// subset glyph ids). Every char was validated by `prepare`.
    #[must_use]
    pub(super) fn encode_hex(&self, line: &str) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(line.chars().count() * 2);
        for c in line.chars() {
            let gid = self.encoding.get(&c).copied().unwrap_or(0);
            bytes.extend_from_slice(&gid.to_be_bytes());
        }
        bytes
    }

    /// Rendered width of `line` in points at `size_pt` in this font.
    #[must_use]
    pub(super) fn width_pt(&self, line: &str, size_pt: f32) -> f32 {
        let mut units = 0_i64;
        for c in line.chars() {
            if let Some(gid) = self.encoding.get(&c)
                && let Some((_, width)) = self.used.get(gid)
            {
                units += *width;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let units = units as f32;
        units * size_pt / 1000.0
    }

    /// Embed this font in `doc` as a Type0 font and return its id.
    pub(super) fn embed(&self, doc: &mut Document) -> ObjectId {
        let bbox = self.face.global_bounding_box();
        let font_stream = Stream::new(
            dictionary! { "Length1" => i64::try_from(self.subset.len()).unwrap_or(i64::MAX) },
            self.subset.clone(),
        );
        let font_file_id = doc.add_object(Object::Stream(font_stream));
        let descriptor_id = doc.add_object(dictionary! {
            "Type" => "FontDescriptor",
            "FontName" => self.name.as_str(),
            "Flags" => 32,
            "FontBBox" => vec![
                i64::from(bbox.x_min).into(),
                i64::from(bbox.y_min).into(),
                i64::from(bbox.x_max).into(),
                i64::from(bbox.y_max).into(),
            ],
            "ItalicAngle" => 0,
            "Ascent" => i64::from(self.face.ascender()),
            "Descent" => i64::from(self.face.descender()),
            "CapHeight" => i64::from(self.face.capital_height().unwrap_or(self.face.ascender())),
            "StemV" => 80,
            "FontFile2" => Object::Reference(font_file_id),
        });
        let mut widths: Vec<Object> = Vec::with_capacity(self.used.len() * 2);
        for (gid, (_, width)) in &self.used {
            widths.push(i64::from(*gid).into());
            widths.push(Object::Array(vec![(*width).into()]));
        }
        let cid_font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "CIDFontType2",
            "BaseFont" => self.name.as_str(),
            "CIDSystemInfo" => dictionary! {
                "Registry" => Object::String(b"Adobe".to_vec(), lopdf::StringFormat::Literal),
                "Ordering" => Object::String(b"Identity".to_vec(), lopdf::StringFormat::Literal),
                "Supplement" => 0,
            },
            "FontDescriptor" => Object::Reference(descriptor_id),
            "DW" => 500,
            "W" => Object::Array(widths),
        });
        let to_unicode_id = doc.add_object(Object::Stream(Stream::new(
            Dictionary::new(),
            to_unicode_cmap(
                &self
                    .used
                    .iter()
                    .map(|(gid, (c, _))| (*gid, c.to_string()))
                    .collect(),
            ),
        )));
        doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => self.name.as_str(),
            "Encoding" => "Identity-H",
            "DescendantFonts" => vec![Object::Reference(cid_font_id)],
            "ToUnicode" => Object::Reference(to_unicode_id),
        })
    }
}
