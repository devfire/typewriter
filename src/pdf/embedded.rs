//! Editable-box state embedded in the saved PDF itself.
//!
//! Baking flattens each box into a page content stream, which alone
//! is not editable: the box's page, position, size, color, width,
//! and text (before `WinAnsi` lossy encoding) exist only in the
//! overlay. So every save also stores the baked boxes as plain PDF
//! objects under a `/Typewriter` key in the document catalog, and
//! every overlay content stream starts with [`OVERLAY_MARKER`].
//! Other viewers ignore the catalog key and render the baked
//! streams normally.
//!
//! When the app claims the payload on open (`take_embedded_boxes`),
//! the boxes become live overlays and the marked streams are
//! removed from each page's `/Contents` in the in-memory document,
//! so the preview shows the boxes once (not
//! the baked copy underneath them) and a later save re-bakes the
//! edited boxes instead of duplicating the old text. The removal is
//! only persisted if that later save happens; opening alone never
//! modifies the file. The orphaned stream objects stay in the file
//! history, unreferenced, like any incrementally-replaced object.
//!
//! The payload is untrusted input (any PDF can carry a forged
//! `/Typewriter` key), so decoding is all-or-nothing with bounded
//! counts and validated fields: a malformed, oversized, or
//! newer-version payload is ignored and the file opens as a plain
//! baked PDF rather than failing or restoring partial state.

use crate::overlay::{BoxId, BoxKind, OverlayText};
use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};

/// First bytes of every content stream baked from overlay boxes.
/// A PDF comment, so renderers ignore it; it lets the loader find
/// (and un-reference) exactly the streams a bake added, without
/// trusting object ids recorded in an earlier revision.
pub(super) const OVERLAY_MARKER: &[u8] = b"%TypewriterOverlay\n";

/// Catalog key holding the embedded box list.
const CATALOG_KEY: &[u8] = b"Typewriter";
/// Payload schema version written by [`store`]. Payloads with any
/// other version are ignored on load, so a newer app's files still
/// open (as uneditable baked PDFs) in an older build.
const VERSION: i64 = 1;
/// Bounds on payload decoding: a forged payload must not make the
/// loader allocate or restore unbounded state.
const MAX_BOXES: usize = 10_000;
const MAX_TEXT_CHARS: usize = 100_000;
/// Coordinates/sizes beyond any real page, rejected as corrupt.
const MAX_POINTS: f32 = 100_000.0;

/// Store `boxes` (exactly the boxes about to be baked) in `doc`'s
/// catalog, replacing any payload from an earlier save. An empty
/// list removes the key, so deleting every restored box and saving
/// leaves no stale editable copy behind.
pub(super) fn store(doc: &mut Document, boxes: &[OverlayText]) -> Result<()> {
    if boxes.is_empty() {
        if let Ok(catalog) = doc.catalog_mut() {
            catalog.remove(CATALOG_KEY);
        }
        return Ok(());
    }
    let mut dicts = Vec::with_capacity(boxes.len());
    for item in boxes {
        let mut dict = Dictionary::new();
        dict.set(
            "Kind",
            Object::Name(match item.kind {
                BoxKind::Text => b"Text".to_vec(),
                BoxKind::Signature => b"Signature".to_vec(),
            }),
        );
        // Stored 1-based, as humans and other tools count pages.
        dict.set("Page", Object::Integer(i64::try_from(item.page + 1)?));
        dict.set("X", Object::Real(item.x_pt));
        dict.set("Y", Object::Real(item.y_top_pt));
        dict.set("Text", encode_text(&item.text));
        dict.set("Size", Object::Real(item.size_pt));
        dict.set(
            "Color",
            Object::Array(item.color.iter().map(|&c| Object::Integer(i64::from(c))).collect()),
        );
        dict.set(
            "Width",
            item.width_pt.map_or(Object::Null, Object::Real),
        );
        dicts.push(Object::Dictionary(dict));
    }
    let mut payload = Dictionary::new();
    payload.set("Version", Object::Integer(VERSION));
    payload.set("Boxes", Object::Array(dicts));
    doc.catalog_mut()
        .context("failed to read PDF catalog for embedded boxes")?
        .set("Typewriter", Object::Dictionary(payload));
    Ok(())
}

/// Decode the embedded boxes in `doc`, if it carries a valid
/// payload for a document of `page_count` pages. `None` means "no
/// usable payload": no key, a foreign or newer-version value, or
/// any field failing validation (all-or-nothing, see module docs).
pub(super) fn extract(doc: &Document, page_count: usize) -> Option<Vec<OverlayText>> {
    let catalog = doc.catalog().ok()?;
    let payload = catalog.get(CATALOG_KEY).ok()?.as_dict().ok()?;
    if payload.get(b"Version").ok()?.as_i64().ok()? != VERSION {
        return None;
    }
    let boxes = payload.get(b"Boxes").ok()?.as_array().ok()?;
    if boxes.len() > MAX_BOXES {
        return None;
    }
    boxes
        .iter()
        .map(|obj| decode_box(obj.as_dict().ok()?, page_count))
        .collect()
}

/// Remove baked overlay streams (identified by [`OVERLAY_MARKER`],
/// never by recorded object ids) from every page's `/Contents`.
/// The `q`/`Q` wrapper streams around pre-existing content are
/// unmarked and stay; they are a balanced no-op pair on their own.
pub(super) fn strip_baked_streams(doc: &mut Document, page_ids: &[ObjectId]) {
    for &page_id in page_ids {
        let contents = doc.get_page_contents(page_id);
        let kept: Vec<ObjectId> = contents
            .iter()
            .copied()
            .filter(|id| !is_overlay_stream(doc, *id))
            .collect();
        if kept.len() == contents.len() {
            continue;
        }
        let Ok(page) = doc.get_dictionary_mut(page_id) else {
            continue;
        };
        match kept.as_slice() {
            [] => {
                page.remove(b"Contents");
            }
            [only] => page.set("Contents", Object::Reference(*only)),
            _ => page.set(
                "Contents",
                Object::Array(kept.iter().map(|id| Object::Reference(*id)).collect()),
            ),
        }
    }
}

/// Whether `id` is a content stream starting with the overlay
/// marker. A stream that cannot be read or decoded is kept: dropping
/// unrecognized content would risk deleting original page art.
fn is_overlay_stream(doc: &Document, id: ObjectId) -> bool {
    let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
        return false;
    };
    if stream.content.starts_with(OVERLAY_MARKER) {
        return true;
    }
    // Another tool may have re-compressed the stream since the bake.
    stream
        .get_plain_content()
        .is_ok_and(|plain| plain.starts_with(OVERLAY_MARKER))
}

/// Validate and convert one payload dictionary. Every field is
/// required and range-checked; any failure rejects the whole
/// payload (the caller maps `None` to "no payload").
fn decode_box(dict: &Dictionary, page_count: usize) -> Option<OverlayText> {
    let kind = match dict.get(b"Kind").ok()?.as_name().ok()? {
        b"Text" => BoxKind::Text,
        b"Signature" => BoxKind::Signature,
        _ => return None,
    };
    let page = dict.get(b"Page").ok()?.as_i64().ok()?;
    let page = usize::try_from(page).ok()?.checked_sub(1)?;
    if page >= page_count {
        return None;
    }
    let x_pt = points(dict.get(b"X").ok()?)?;
    let y_top_pt = points(dict.get(b"Y").ok()?)?;
    let text = decode_text(dict.get(b"Text").ok()?)?;
    if text.chars().count() > MAX_TEXT_CHARS {
        return None;
    }
    let size_pt = dict.get(b"Size").ok()?.as_float().ok()?;
    if !(0.5..=1_000.0).contains(&size_pt) {
        return None;
    }
    let color_obj = dict.get(b"Color").ok()?.as_array().ok()?;
    let [r, g, b] = color_obj.as_slice() else {
        return None;
    };
    let mut color = [0_u8; 3];
    for (slot, obj) in color.iter_mut().zip([r, g, b]) {
        *slot = u8::try_from(obj.as_i64().ok()?).ok()?;
    }
    let width_pt = match dict.get(b"Width").ok()? {
        Object::Null => None,
        obj => {
            let width = points(obj)?;
            if width <= 0.0 {
                return None;
            }
            Some(width)
        }
    };
    // Ids are session-local and reassigned by
    // `OverlayState::restore_embedded`; the placeholder is replaced
    // before the box is ever displayed.
    let mut item = OverlayText::new(BoxId::default(), page, x_pt, y_top_pt, kind);
    item.text = text;
    item.size_pt = size_pt;
    item.color = color;
    item.width_pt = width_pt;
    Some(item)
}

/// A finite PDF number within any plausible page geometry.
fn points(obj: &Object) -> Option<f32> {
    let value = obj.as_float().ok()?;
    (value.is_finite() && value.abs() <= MAX_POINTS).then_some(value)
}

/// Encode `text` as a PDF text string: UTF-16BE with a byte-order
/// mark, hex-encoded so no byte needs literal-string escaping.
/// This preserves characters the baked `WinAnsi` stream cannot
/// represent, so a restored box keeps the user's original text.
fn encode_text(text: &str) -> Object {
    let mut bytes = vec![0xFE, 0xFF];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(bytes, StringFormat::Hexadecimal)
}

/// Inverse of [`encode_text`]. Strings without a BOM are read as
/// UTF-8 (what hand-written payloads most plausibly carry); invalid
/// UTF-16 (odd length, lone surrogates) rejects the box.
fn decode_text(obj: &Object) -> Option<String> {
    let bytes = obj.as_str().ok()?;
    if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        if !body.len().is_multiple_of(2) {
            return None;
        }
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        char::decode_utf16(units).collect::<Result<String, _>>().ok()
    } else {
        String::from_utf8(bytes.to_vec()).ok()
    }
}
