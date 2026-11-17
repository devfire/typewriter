//! Embedding Momo Signature into baked PDFs as a Type0 font.
//!
//! A script font cannot use the standard-14/WinAnsi path the text boxes
//! use: the font file itself must travel with the document. Only the
//! glyphs a bake actually uses are embedded: `subsetter` produces a
//! tiny TrueType subset of the 406 KB Momo program, and characters
//! are encoded as two-byte CIDs equal to their glyph ids *in the
//! subset* (`Identity-H`, with the subsetter's identity GID->CID
//! mapping). Widths come from the original face's `hmtx` table, and a
//! `ToUnicode` `CMap` keeps the baked signature copy/paste-able.
//!
//! Because a subset only contains its own bake's glyphs, it can
//! never gain glyphs later. A bake on a reloaded document therefore
//! embeds a fresh subset font for its boxes instead of patching the
//! previous bake's font; each subset is a few KB, so fonts from
//! earlier sessions are simply left untouched.

use crate::overlay::OverlayText;
use crate::signature::FONT_BYTES;
use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use subsetter::GlyphRemapper;
use ttf_parser::Face;

/// Parsed Momo face over the embedded static bytes.
pub(super) fn face() -> Result<Face<'static>> {
    Face::parse(FONT_BYTES, 0).context("failed to parse embedded Momo Signature font")
}

/// Glyph id for `c`, falling back to `?` and finally to `.notdef` (0).
fn glyph_id(face: &Face<'_>, c: char) -> u16 {
    face.glyph_index(c)
        .or_else(|| face.glyph_index('?'))
        .map_or(0, |g| g.0)
}

/// Convert a horizontal advance in font units to PDF's 1000-unit em
/// space (`/W` and `/DW` are always in 1000-unit space, regardless of
/// the font's actual units-per-em). Rounds to nearest. A zero
/// units-per-em (invalid font) leaves the advance unscaled rather
/// than dividing by zero.
#[must_use]
pub(super) fn width_1000(advance: u16, units_per_em: u16) -> i64 {
    if units_per_em == 0 {
        return i64::from(advance);
    }
    let advance = i64::from(advance);
    let units_per_em = i64::from(units_per_em);
    (advance * 1_000 + units_per_em / 2) / units_per_em
}

/// Glyphs used by `boxes`, keyed by glyph id in the *original* face:
/// glyph id -> (char for `ToUnicode`, advance width in 1000-unit em
/// space, scaled from the face's actual units-per-em). The char is
/// the source char only when the face actually maps it; unmappable
/// chars render as the `?` fallback glyph, so that glyph records
/// `?` — never whichever unmappable char reached it first.
fn used_glyphs(face: &Face<'_>, boxes: &[&OverlayText]) -> BTreeMap<u16, (char, i64)> {
    let units_per_em = face.units_per_em();
    let mut used = BTreeMap::new();
    for item in boxes {
        for c in item.signature_line().chars() {
            let (gid, extracted) = match face.glyph_index(c) {
                Some(gid) => (gid.0, c),
                None => (glyph_id(face, c), '?'),
            };
            let width = face
                .glyph_hor_advance(ttf_parser::GlyphId(gid))
                .map_or(500, |advance| width_1000(advance, units_per_em));
            used.entry(gid).or_insert((extracted, width));
        }
    }
    used
}

/// `ToUnicode` `CMap` text mapping glyph ids back to their source text.
fn to_unicode_cmap(used: &BTreeMap<u16, String>) -> Vec<u8> {
    let mut out = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    // bfchar blocks accept at most 100 entries.
    let entries: Vec<(&u16, &String)> = used.iter().collect();
    for chunk in entries.chunks(100) {
        writeln!(out, "{} beginbfchar", chunk.len()).expect("writing to a String cannot fail");
        for (gid, text) in chunk {
            let mut hex = String::new();
            for unit in text.encode_utf16() {
                write!(hex, "{unit:04X}").expect("writing to a String cannot fail");
            }
            writeln!(out, "<{gid:04X}> <{hex}>").expect("writing to a String cannot fail");
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out.into_bytes()
}

/// Font name for one bake's subset: `ABCDEF+MomoSignature`.
///
/// ISO 32000-1 §9.6.4 requires a subset font's name to start with
/// a tag of six uppercase letters followed by `+`. Distinct bakes
/// embed distinct subsets under otherwise identical names, so
/// name-keyed font caches could substitute one subset for another.
/// The tag is deterministic: six letters derived from `Sha256` of
/// the sorted original glyph ids (`old_ids` comes from a
/// `BTreeMap`, so it is already sorted), hence the same glyph set
/// always yields the same name and a different set yields a
/// different one (up to hash collision).
fn subset_name(old_ids: &[u16]) -> String {
    let mut hasher = Sha256::new();
    for id in old_ids {
        hasher.update(id.to_be_bytes());
    }
    let digest = hasher.finalize();
    let mut name = String::with_capacity(6 + 1 + "MomoSignature".len());
    for byte in &digest[..6] {
        name.push(char::from(b'A' + byte % 26));
    }
    name.push('+');
    name.push_str("MomoSignature");
    name
}

/// One bake's subsetted signature font: the subset font program plus
/// everything needed to encode text for it and describe it in a PDF.
///
/// Glyph ids in the subset differ from the original face (the
/// subsetter compacts them into a contiguous range starting at
/// `.notdef`), so encoding, `/W` widths, and `ToUnicode` are all
/// keyed by subset glyph id, never by original glyph id.
pub(super) struct SignatureFont {
    /// Tagged font name (`ABCDEF+MomoSignature`; see `subset_name`).
    name: String,
    /// Source char -> glyph id in the subset.
    encoding: BTreeMap<char, u16>,
    /// Subset glyph id -> (source char, width in 1000-unit em space).
    used: BTreeMap<u16, (char, i64)>,
    /// The subset TrueType program (`FontFile2` content).
    subset: Vec<u8>,
}

impl SignatureFont {
    /// Subset the embedded Momo font to the glyphs used by `boxes`
    /// (which must be every signature box in the bake, so one subset
    /// serves all signed pages).
    pub(super) fn prepare(face: &Face<'_>, boxes: &[&OverlayText]) -> Result<Self> {
        let used_old = used_glyphs(face, boxes);
        let old_ids: Vec<u16> = used_old.keys().copied().collect();
        // Sorted so subset glyph ids are deterministic for a bake.
        let remapper = GlyphRemapper::new_from_glyphs_sorted(&old_ids);
        let subset = subsetter::subset(FONT_BYTES, 0, &remapper)
            .context("failed to subset embedded Momo Signature font")?;
        let mut encoding = BTreeMap::new();
        for item in boxes {
            for c in item.signature_line().chars() {
                // Every glyph in `used_old` was remapped above; a
                // glyph missing from the remapper can only be
                // `.notdef`, which the remapper always keeps at 0.
                let new = remapper.get(glyph_id(face, c)).unwrap_or(0);
                encoding.entry(c).or_insert(new);
            }
        }
        let used = used_old
            .iter()
            .map(|(old, (c, width))| (remapper.get(*old).unwrap_or(0), (*c, *width)))
            .collect();
        Ok(Self {
            name: subset_name(&old_ids),
            encoding,
            used,
            subset,
        })
    }

    /// Encode one signature line as `Identity-H` bytes (big-endian
    /// subset glyph ids). Characters outside this font's bake fall
    /// back to `?`'s glyph and finally to `.notdef` (0).
    #[must_use]
    pub(super) fn encode_hex(&self, line: &str) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(line.chars().count() * 2);
        for c in line.chars() {
            let gid = self
                .encoding
                .get(&c)
                .or_else(|| self.encoding.get(&'?'))
                .copied()
                .unwrap_or(0);
            bytes.extend_from_slice(&gid.to_be_bytes());
        }
        bytes
    }

    /// Length of the subset font program in bytes.
    #[cfg(test)]
    #[must_use]
    pub(super) fn subset_len(&self) -> usize {
        self.subset.len()
    }

    /// Embed this font in `doc` as a Type0 font (descriptor +
    /// `CIDFont` + subset font stream) and return its object id.
    /// Callers insert the id into each signed page's resources (see
    /// `super::insert_font_entry`); the subset is embedded exactly
    /// once per bake, however many pages are signed.
    pub(super) fn embed(&self, doc: &mut Document, face: &Face<'_>) -> ObjectId {
        let bbox = face.global_bounding_box();
        let font_stream = Stream::new(
            dictionary! { "Length1" => i64::try_from(self.subset.len()).unwrap_or(i64::MAX) },
            self.subset.clone(),
        );
        let font_file_id = doc.add_object(Object::Stream(font_stream));
        let descriptor_id = doc.add_object(dictionary! {
            "Type" => "FontDescriptor",
            "FontName" => self.name.as_str(),
            "Flags" => 40,
            "FontBBox" => vec![
                i64::from(bbox.x_min).into(),
                i64::from(bbox.y_min).into(),
                i64::from(bbox.x_max).into(),
                i64::from(bbox.y_max).into(),
            ],
            "ItalicAngle" => 0,
            "Ascent" => i64::from(face.ascender()),
            "Descent" => i64::from(face.descender()),
            "CapHeight" => i64::from(face.capital_height().unwrap_or(face.ascender())),
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

/// Resource alias on `page_id` for the exact signature font
/// `type0_id`, if the page already references it. Matching the
/// object id (not just the `MomoSignature` name) matters on
/// reloaded documents, where a page may already reference an
/// earlier bake's subset whose glyph ids differ from this bake's.
pub(super) fn signature_alias(
    doc: &Document,
    page_id: ObjectId,
    type0_id: ObjectId,
) -> Option<Vec<u8>> {
    let fonts = super::effective_font_dict(doc, page_id)?;
    for (alias, value) in fonts {
        if value.as_reference().is_ok_and(|id| id == type0_id) {
            return Some(alias.clone());
        }
    }
    None
}
