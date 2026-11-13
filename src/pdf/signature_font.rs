//! Embedding Momo Signature into baked PDFs as a Type0 font.
//!
//! A script font cannot use the standard-14/WinAnsi path the text boxes
//! use: the font file itself must travel with the document. Characters
//! are encoded as two-byte CIDs equal to their TrueType glyph ids
//! (`Identity-H`), widths come from the font's `hmtx` table, and a
//! `ToUnicode` `CMap` keeps the baked signature copy/paste-able.

use crate::overlay::OverlayText;
use crate::signature::FONT_BYTES;
use anyhow::{Context, Result, bail};
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

/// Numeric PDF object as an integer width (`Real` truncated, matching
/// how readers consume `/W` entries).
fn numeric(obj: &Object) -> Option<i64> {
    match obj {
        Object::Integer(value) => Some(*value),
        Object::Real(value) =>
        {
            #[allow(clippy::cast_possible_truncation)]
            Some(*value as i64)
        }
        _ => None,
    }
}

/// Parse a `CIDFont` `/W` array into glyph id -> width. Supports both
/// PDF forms: `c [w ...]` (consecutive glyphs) and `cfirst clast w`.
fn parse_widths(doc: &Document, array: &[Object]) -> Result<BTreeMap<u16, i64>> {
    let mut widths = BTreeMap::new();
    let mut idx = 0;
    while idx < array.len() {
        let first_obj = super::resolve(doc, &array[idx]).context("missing /W entry")?;
        let first = numeric(first_obj).context("invalid /W glyph id")?;
        let gid = u16::try_from(first).context("invalid /W glyph id")?;
        let next = array
            .get(idx + 1)
            .and_then(|obj| super::resolve(doc, obj))
            .context("missing /W value")?;
        if let Object::Array(values) = next {
            for (offset, value) in values.iter().enumerate() {
                let value = super::resolve(doc, value).context("missing /W width")?;
                let width = numeric(value).context("invalid /W width")?;
                let glyph =
                    u16::try_from(usize::from(gid) + offset).context("invalid /W glyph range")?;
                widths.insert(glyph, width);
            }
            idx += 2;
        } else {
            let last = numeric(next).context("invalid /W range")?;
            let last = u16::try_from(last).context("invalid /W range")?;
            let width_obj = array
                .get(idx + 2)
                .and_then(|obj| super::resolve(doc, obj))
                .context("missing /W range width")?;
            let width = numeric(width_obj).context("invalid /W range width")?;
            if last < gid {
                bail!("invalid /W range");
            }
            for glyph in gid..=last {
                widths.insert(glyph, width);
            }
            idx += 3;
        }
    }
    Ok(widths)
}

/// Hex strings (`<...>`) inside one `beginbfchar`/`endbfchar` section.
fn hex_tokens(section: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut rest = section;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else { break };
        tokens.push(&after[..close]);
        rest = &after[close + 1..];
    }
    tokens
}

/// Decode a `ToUnicode` destination: UTF-16BE hex to text.
fn decode_utf16_hex(hex: &str) -> Result<String> {
    let hex = hex.trim();
    if hex.is_empty() || !hex.len().is_multiple_of(4) {
        bail!("invalid ToUnicode destination");
    }
    let mut units = Vec::with_capacity(hex.len() / 4);
    for chunk in hex.as_bytes().chunks(4) {
        let chunk = std::str::from_utf8(chunk).context("invalid ToUnicode destination")?;
        units.push(u16::from_str_radix(chunk, 16).context("invalid ToUnicode destination")?);
    }
    String::from_utf16(&units).context("invalid ToUnicode destination")
}

/// Parse `beginbfchar` mappings from an existing `ToUnicode` `CMap`.
/// `bfrange` mappings are not produced by this crate; refusing them
/// keeps a foreign `CMap` from being silently truncated on rewrite.
fn parse_to_unicode(content: &[u8]) -> Result<BTreeMap<u16, String>> {
    let text = String::from_utf8_lossy(content);
    if text.contains("beginbfrange") {
        bail!("unsupported ToUnicode bfrange in existing signature font");
    }
    let mut map = BTreeMap::new();
    let mut rest = text.as_ref();
    while let Some(start) = rest.find("beginbfchar") {
        let after = &rest[start + "beginbfchar".len()..];
        let Some(end) = after.find("endbfchar") else {
            bail!("unterminated ToUnicode bfchar");
        };
        let tokens = hex_tokens(&after[..end]);
        if !tokens.len().is_multiple_of(2) {
            bail!("invalid ToUnicode bfchar entry count");
        }
        for pair in tokens.chunks_exact(2) {
            let gid =
                u16::from_str_radix(pair[0].trim(), 16).context("invalid ToUnicode glyph id")?;
            map.insert(gid, decode_utf16_hex(pair[1])?);
        }
        rest = &after[end + "endbfchar".len()..];
    }
    Ok(map)
}

/// Extend an already-embedded signature font (found after a reload)
/// with the glyphs used by `boxes`: merge their widths into the
/// descendant `CIDFont`'s `/W` and their mappings into the `Type0`
/// font's `ToUnicode` `CMap`. Existing entries are preserved, so a
/// second session can sign with characters the first session never
/// used without re-embedding the 406 KB font program.
pub(super) fn extend_signature_font(
    doc: &mut Document,
    type0_id: ObjectId,
    face: &Face<'_>,
    boxes: &[&OverlayText],
) -> Result<()> {
    let used = used_glyphs(face, boxes);
    if used.is_empty() {
        return Ok(());
    }
    let (cid_id, to_unicode_id) = {
        let type0 = doc
            .get_dictionary(type0_id)
            .context("existing signature font is not a dictionary")?;
        let descendants = type0
            .get(b"DescendantFonts")
            .context("existing signature font has no descendant fonts")?;
        let descendants = super::resolve(doc, descendants)
            .and_then(|obj| obj.as_array().ok())
            .context("existing signature font has invalid descendant fonts")?;
        let descendant = descendants
            .first()
            .context("existing signature font has no descendant fonts")?;
        let cid_id = descendant
            .as_reference()
            .context("existing signature CIDFont is not a reference")?;
        let to_unicode_id = match type0.get(b"ToUnicode") {
            Ok(obj) => Some(
                obj.as_reference()
                    .context("existing ToUnicode is not a reference")?,
            ),
            Err(_) => None,
        };
        (cid_id, to_unicode_id)
    };
    let mut widths = {
        let cid = doc
            .get_dictionary(cid_id)
            .context("existing signature CIDFont is not a dictionary")?;
        match cid.get(b"W") {
            Ok(obj) => {
                let array = super::resolve(doc, obj)
                    .and_then(|resolved| resolved.as_array().ok())
                    .context("existing signature font has invalid /W")?;
                parse_widths(doc, array)?
            }
            Err(_) => BTreeMap::new(),
        }
    };
    for (gid, (_, width)) in &used {
        widths.insert(*gid, *width);
    }
    let mut unicode = match to_unicode_id {
        Some(id) => {
            let stream = doc
                .get_object(id)
                .and_then(|obj| obj.as_stream())
                .context("existing ToUnicode is not a stream")?;
            let content = stream
                .get_plain_content()
                .context("failed to read existing ToUnicode")?;
            parse_to_unicode(&content)?
        }
        None => BTreeMap::new(),
    };
    for (gid, (c, _)) in &used {
        unicode.insert(*gid, c.to_string());
    }
    let widths_array: Vec<Object> = widths
        .iter()
        .flat_map(|(gid, width)| [i64::from(*gid).into(), Object::Array(vec![(*width).into()])])
        .collect();
    doc.get_dictionary_mut(cid_id)
        .context("existing signature CIDFont is not a dictionary")?
        .set("W", Object::Array(widths_array));
    let cmap = to_unicode_cmap(&unicode);
    if let Some(id) = to_unicode_id {
        doc.get_object_mut(id)
            .and_then(|obj| obj.as_stream_mut())
            .context("existing ToUnicode is not a stream")?
            .set_plain_content(cmap);
    } else {
        let new_id = doc.add_object(Object::Stream(Stream::new(Dictionary::new(), cmap)));
        doc.get_dictionary_mut(type0_id)
            .context("existing signature font is not a dictionary")?
            .set("ToUnicode", Object::Reference(new_id));
    }
    Ok(())
}

/// `(alias, Type0 object id)` of a `MomoSignature` font referenced
/// from `page_id`, if any. Only referenced fonts are returned:
/// embedded fonts created by this crate are always references.
pub(super) fn signature_font_on_page(
    doc: &Document,
    page_id: ObjectId,
) -> Option<(String, ObjectId)> {
    let target = super::find_resources_target(doc, page_id)?;
    let resources = match target {
        super::ResourcesTarget::Reference(id) => doc.get_dictionary(id).ok()?,
        super::ResourcesTarget::Direct(node_id) => doc
            .get_dictionary(node_id)
            .ok()?
            .get(b"Resources")
            .ok()?
            .as_dict()
            .ok()?,
    };
    let fonts = super::resolve(doc, resources.get(b"Font").ok()?)?;
    let fonts = fonts.as_dict().ok()?;
    for (alias, value) in fonts {
        let Ok(type0_id) = value.as_reference() else {
            continue;
        };
        let Ok(font) = doc.get_dictionary(type0_id) else {
            continue;
        };
        if font
            .get(b"BaseFont")
            .is_ok_and(|base| base.as_name().is_ok_and(|name| name == b"MomoSignature"))
        {
            return Some((String::from_utf8_lossy(alias).into_owned(), type0_id));
        }
    }
    None
}

/// Object id of any `Type0` `MomoSignature` font already embedded in
/// the document, regardless of which page references it.
pub(super) fn find_signature_font(doc: &Document) -> Option<ObjectId> {
    doc.objects.iter().find_map(|(id, obj)| {
        let dict = obj.as_dict().ok()?;
        let is_type0 = dict
            .get(b"Subtype")
            .is_ok_and(|subtype| subtype.as_name().is_ok_and(|name| name == b"Type0"));
        let is_momo = dict
            .get(b"BaseFont")
            .is_ok_and(|base| base.as_name().is_ok_and(|name| name == b"MomoSignature"));
        (is_type0 && is_momo).then_some(*id)
    })
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
        to_unicode_cmap(
            &used
                .iter()
                .map(|(gid, (c, _))| (*gid, c.to_string()))
                .collect(),
        ),
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
