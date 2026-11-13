//! Embedding Momo Signature into baked PDFs as a Type0 font.
//!
//! A script font cannot use the standard-14/WinAnsi path the text boxes
//! use: the font file itself must travel with the document. Characters
//! are encoded as two-byte CIDs equal to their TrueType glyph ids
//! (`Identity-H`), widths come from the font's `hmtx` table, and a
//! A `ToUnicode` `CMap` keeps the baked signature copy/paste-able.

use crate::overlay::OverlayText;
use crate::signature::FONT_BYTES;
use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use std::collections::BTreeMap;
use std::fmt::Write as _;
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

/// Encode one signature line as `Identity-H` bytes (big-endian glyph ids).
#[must_use]
pub(super) fn encode_hex(face: &Face<'_>, line: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(line.chars().count() * 2);
    for c in line.chars() {
        bytes.extend_from_slice(&glyph_id(face, c).to_be_bytes());
    }
    bytes
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

/// Glyphs used by `boxes`: glyph id -> (source char, advance width in
/// 1000-unit em space, scaled from the face's actual units-per-em).
fn used_glyphs(face: &Face<'_>, boxes: &[&OverlayText]) -> BTreeMap<u16, (char, i64)> {
    let units_per_em = face.units_per_em();
    let mut used = BTreeMap::new();
    for item in boxes {
        for c in item.signature_line().chars() {
            let gid = glyph_id(face, c);
            let width = face
                .glyph_hor_advance(ttf_parser::GlyphId(gid))
                .map_or(500, |advance| width_1000(advance, units_per_em));
            used.entry(gid).or_insert((c, width));
        }
    }
    used
}

/// `ToUnicode` `CMap` text mapping glyph ids back to their source chars.
fn to_unicode_cmap(used: &BTreeMap<u16, (char, i64)>) -> Vec<u8> {
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
    let entries: Vec<(&u16, &(char, i64))> = used.iter().collect();
    for chunk in entries.chunks(100) {
        writeln!(out, "{} beginbfchar", chunk.len()).expect("writing to a String cannot fail");
        for (gid, (c, _)) in chunk {
            let mut utf16 = [0_u16; 2];
            let encoded = c.encode_utf16(&mut utf16);
            let mut hex = String::with_capacity(encoded.len() * 4);
            for unit in encoded {
                write!(hex, "{unit:04X}").expect("writing to a String cannot fail");
            }
            writeln!(out, "<{gid:04X}> <{hex}>").expect("writing to a String cannot fail");
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out.into_bytes()
}

/// Create the shared Type0 signature font (descriptor + `CIDFont` +
/// font stream) once per document and return its object id.
///
/// `boxes` must be every signature box in the document being baked,
/// not just one page's: the single font's `/W` widths and `ToUnicode`
/// `CMap` are built from this union, so glyphs used on any page are
/// covered. Callers insert the returned id into each signed page's
/// resources (see `super::insert_font_entry`); the 406 KB font
/// program is therefore embedded exactly once, however many pages
/// are signed.
pub(super) fn create_signature_font(
    doc: &mut Document,
    face: &Face<'_>,
    boxes: &[&OverlayText],
) -> ObjectId {
    let used = used_glyphs(face, boxes);
    let bbox = face.global_bounding_box();
    let font_stream = Stream::new(
        dictionary! { "Length1" => i64::try_from(FONT_BYTES.len()).unwrap_or(i64::MAX) },
        FONT_BYTES.to_vec(),
    );
    let font_file_id = doc.add_object(Object::Stream(font_stream));
    let descriptor_id = doc.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "MomoSignature",
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
    let mut widths: Vec<Object> = Vec::with_capacity(used.len() * 2);
    for (gid, (_, width)) in &used {
        widths.push(i64::from(*gid).into());
        widths.push(Object::Array(vec![(*width).into()]));
    }
    let cid_font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => "MomoSignature",
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
        to_unicode_cmap(&used),
    )));
    doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "MomoSignature",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(cid_font_id)],
        "ToUnicode" => Object::Reference(to_unicode_id),
    })
}
