//! Editable-box state embedded in the saved PDF itself.
//!
//! Baking flattens each box into a page content stream, which alone
//! is not editable: the box's page, position, size, color, width,
//! and text (before `WinAnsi` lossy encoding) exist only in the
//! overlay. So every save also stores, as plain PDF objects under
//! a `/Typewriter` key in the document catalog:
//!
//! - the baked boxes, and
//! - for each baked overlay stream, its page, its object id, and
//!   the SHA-256 of its decompressed contents, recorded at bake
//!   time when the stream's bytes are known to be this app's own.
//!
//! Other viewers ignore the catalog key and render the baked
//! streams normally.
//!
//! The payload and the streams must agree before anything trusts
//! either. The `%TypewriterOverlay` marker alone proves nothing:
//! any file can start a stream with it, and another tool can edit,
//! merge, or rewrite the baked streams after the save, leaving the
//! payload describing content that is no longer what viewers see.
//! So claiming the payload (`take_embedded_boxes`) succeeds only
//! if *every* recorded stream is still present on its recorded
//! page, under its recorded object id, with its recorded hash
//! (which also proves it still starts with the marker, since the
//! hash covers the marker bytes), and no unrecorded marked stream
//! exists on any page. Only then are exactly those streams
//! un-referenced and the boxes restored as live overlays. Any
//! mismatch — forged marker, edited or merged stream, rewired
//! object id — and the whole payload is ignored: nothing is
//! stripped, nothing is restored, and the file behaves as a plain
//! baked PDF instead of hiding real content or resurrecting boxes
//! over a baked copy that is still there.
//!
//! The removal is only persisted if a later save happens; opening
//! alone never modifies the file. The orphaned stream objects stay
//! in the file history, unreferenced, like any
//! incrementally-replaced object.
//!
//! The payload is untrusted input (any PDF can carry a forged
//! `/Typewriter` key), so decoding is all-or-nothing with bounded
//! counts and validated fields: a malformed, oversized, or
//! newer-version payload is ignored rather than failing the open.
//! The hash binds payload to streams as *recorded at save time*;
//! it is a consistency check, not an authenticity proof — a file
//! authored from scratch with a self-consistent payload, streams,
//! and hashes is indistinguishable from a genuine bake, by
//! construction.

use crate::overlay::{BoxId, BoxKind, OverlayText};
use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// First bytes of every content stream baked from overlay boxes.
/// A PDF comment, so renderers ignore it. It identifies candidate
/// baked streams on sight, but is never trusted on its own: see
/// the hash verification in [`verify`].
pub(super) const OVERLAY_MARKER: &[u8] = b"%TypewriterOverlay\n";

/// Catalog key holding the embedded box list.
const CATALOG_KEY: &[u8] = b"Typewriter";
/// Payload schema version written by [`store`]. Version 1 carried
/// no stream hashes and is deliberately not accepted: unverifiable
/// payloads must not drive stripping. Any other version likewise
/// opens as an uneditable baked PDF.
const VERSION: i64 = 2;
/// Bounds on payload decoding: a forged payload must not make the
/// loader allocate or restore unbounded state.
const MAX_BOXES: usize = 10_000;
const MAX_TEXT_CHARS: usize = 100_000;
/// Cap on a candidate stream's decompressed size during
/// verification: real baked streams hold at most `MAX_BOXES`
/// boxes of text, and a forged stream must not expand without
/// bound before its hash can reject it.
const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
/// Coordinates/sizes beyond any real page, rejected as corrupt.
const MAX_POINTS: f32 = 100_000.0;

/// One overlay stream a bake just added: the page it was appended
/// to and its fresh object id in the document being saved.
#[derive(Debug, Clone, Copy)]
pub(super) struct BakedStream {
    pub page: usize,
    pub id: ObjectId,
}

/// A decoded, field-validated payload, not yet verified against
/// the document's actual streams (see [`verify`]).
#[derive(Debug)]
pub(super) struct Embedded {
    pub boxes: Vec<OverlayText>,
    streams: Vec<ExpectedStream>,
}

/// One baked stream the payload expects to find, exactly as
/// recorded at save time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExpectedStream {
    page: usize,
    id: ObjectId,
    sha256: [u8; 32],
}

/// Store `boxes` (exactly the boxes just baked, which produced
/// `baked`) in `doc`'s catalog, replacing any payload from an
/// earlier save. Each baked stream's hash is computed from `doc`
/// now, while its bytes are known-good; a stream that cannot be
/// read back aborts the save rather than writing a payload the
/// next open could not verify. An empty box list removes the key,
/// so deleting every restored box and saving leaves no stale
/// editable copy behind.
pub(super) fn store(
    doc: &mut Document,
    boxes: &[OverlayText],
    baked: &[BakedStream],
) -> Result<()> {
    if boxes.is_empty() {
        if let Ok(catalog) = doc.catalog_mut() {
            catalog.remove(CATALOG_KEY);
        }
        return Ok(());
    }
    let mut stream_dicts = Vec::with_capacity(baked.len());
    for stream in baked {
        let plain = plain_stream(doc, stream.id)
            .context("failed to read back baked stream for embedded payload")?;
        let mut dict = Dictionary::new();
        dict.set("Page", Object::Integer(i64::try_from(stream.page + 1)?));
        dict.set("Obj", Object::Integer(i64::from(stream.id.0)));
        dict.set("Gen", Object::Integer(i64::from(stream.id.1)));
        dict.set(
            "Sha256",
            Object::String(to_hex(&Sha256::digest(&plain)), StringFormat::Literal),
        );
        stream_dicts.push(Object::Dictionary(dict));
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
    payload.set("Streams", Object::Array(stream_dicts));
    doc.catalog_mut()
        .context("failed to read PDF catalog for embedded boxes")?
        .set("Typewriter", Object::Dictionary(payload));
    Ok(())
}

/// Decode and field-validate the payload in `doc`, for a document
/// of `page_count` pages. `None` means "no usable payload": no
/// key, a foreign or other-version value, or any field failing
/// validation (all-or-nothing, see module docs). Stream existence
/// and hashes are *not* checked here; that is [`verify`], run at
/// claim time against the live document.
pub(super) fn extract(doc: &Document, page_count: usize) -> Option<Embedded> {
    let catalog = doc.catalog().ok()?;
    let payload = catalog.get(CATALOG_KEY).ok()?.as_dict().ok()?;
    if payload.get(b"Version").ok()?.as_i64().ok()? != VERSION {
        return None;
    }
    let boxes = payload.get(b"Boxes").ok()?.as_array().ok()?;
    if boxes.is_empty() || boxes.len() > MAX_BOXES {
        return None;
    }
    let boxes: Vec<OverlayText> = boxes
        .iter()
        .map(|obj| decode_box(obj.as_dict().ok()?, page_count))
        .collect::<Option<_>>()?;
    let streams = payload.get(b"Streams").ok()?.as_array().ok()?;
    // One baked stream per page with boxes, never more.
    if streams.is_empty() || streams.len() > page_count {
        return None;
    }
    let streams: Vec<ExpectedStream> = streams
        .iter()
        .map(|obj| decode_stream(obj.as_dict().ok()?, page_count))
        .collect::<Option<_>>()?;
    // The payload must describe exactly one stream per page that
    // has boxes, and no stream for a page without any: a payload
    // describing a different bake than its box list is rejected
    // before hashes are even consulted.
    let box_pages: BTreeSet<usize> = boxes.iter().map(|item| item.page).collect();
    let stream_pages: BTreeSet<usize> = streams.iter().map(|stream| stream.page).collect();
    if box_pages != stream_pages {
        return None;
    }
    let unique_ids: HashSet<ObjectId> = streams.iter().map(|stream| stream.id).collect();
    if unique_ids.len() != streams.len() {
        return None;
    }
    Some(Embedded { boxes, streams })
}

/// Whether every stream `embedded` records is still exactly where
/// and what the payload says, and no other marked stream exists.
/// This is the gate between "payload present" and "payload
/// trusted": stripping or restoring on anything less would let a
/// forged or stale payload hide real page content (marked stream
/// that was never a bake) or duplicate it (bake rewritten so its
/// stream no longer matches, leaving the copy in place while the
/// boxes come back as overlays).
pub(super) fn verify(doc: &Document, page_ids: &[ObjectId], embedded: &Embedded) -> bool {
    let expected: BTreeMap<usize, &ExpectedStream> =
        embedded.streams.iter().map(|s| (s.page, s)).collect();
    for (page, &page_id) in page_ids.iter().enumerate() {
        let contents = doc.get_page_contents(page_id);
        let expected_here = expected.get(&page);
        let mut found = false;
        for id in &contents {
            // Non-stream entries in `/Contents` cannot occur from
            // lopdf's accessor, but an unreadable object is simply
            // not a baked stream; it is never stripped either way.
            let Some(plain) = plain_stream(doc, *id) else {
                continue;
            };
            if !plain.starts_with(OVERLAY_MARKER) {
                continue;
            }
            // A marked stream: it must be *the* recorded stream
            // for this page, byte for byte. Anything else — wrong
            // id, edited bytes, an extra marked stream, a marked
            // stream on a page the payload does not claim — is a
            // divergence the payload cannot account for.
            match expected_here {
                Some(stream) if stream.id == *id && !found => {
                    if Sha256::digest(&plain)[..] != stream.sha256 {
                        return false;
                    }
                    found = true;
                }
                _ => return false,
            }
        }
        if expected_here.is_some() && !found {
            return false;
        }
    }
    true
}

/// Remove exactly the verified baked streams from every page's
/// `/Contents`. Must only be called after [`verify`] returned
/// true for the same document; it trusts `embedded` blindly. The
/// `q`/`Q` wrapper streams around pre-existing content are not
/// recorded and stay; they are a balanced no-op pair on their own.
pub(super) fn strip_verified(doc: &mut Document, page_ids: &[ObjectId], embedded: &Embedded) {
    let doomed: HashSet<ObjectId> = embedded.streams.iter().map(|s| s.id).collect();
    for &page_id in page_ids {
        let contents = doc.get_page_contents(page_id);
        let kept: Vec<ObjectId> = contents
            .iter()
            .copied()
            .filter(|id| !doomed.contains(id))
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

/// Decompressed contents of stream `id`, bounded by
/// [`MAX_STREAM_BYTES`]. `None` for non-streams, unreadable or
/// undecodable streams, and oversize ones — all of which simply
/// fail verification or (at bake time) abort the save.
fn plain_stream(doc: &Document, id: ObjectId) -> Option<Vec<u8>> {
    let stream = doc.get_object(id).ok()?.as_stream().ok()?;
    if stream.content.len() > MAX_STREAM_BYTES {
        return None;
    }
    let plain = stream.get_plain_content().ok()?;
    (plain.len() <= MAX_STREAM_BYTES).then_some(plain)
}

/// Validate and convert one stream dictionary.
fn decode_stream(dict: &Dictionary, page_count: usize) -> Option<ExpectedStream> {
    let page = dict.get(b"Page").ok()?.as_i64().ok()?;
    let page = usize::try_from(page).ok()?.checked_sub(1)?;
    if page >= page_count {
        return None;
    }
    let obj = u32::try_from(dict.get(b"Obj").ok()?.as_i64().ok()?).ok()?;
    let generation = u16::try_from(dict.get(b"Gen").ok()?.as_i64().ok()?).ok()?;
    let hex = dict.get(b"Sha256").ok()?.as_str().ok()?;
    Some(ExpectedStream {
        page,
        id: (obj, generation),
        sha256: parse_hex_32(hex)?,
    })
}

/// Lowercase hex of `bytes`, as stored in the payload.
fn to_hex(bytes: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[usize::from(byte >> 4)]);
        out.push(HEX[usize::from(byte & 0x0F)]);
    }
    out
}

/// Parse exactly 64 hex characters into a 32-byte hash. Mixed case
/// is accepted; anything else rejects the payload.
fn parse_hex_32(hex: &[u8]) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let nibble = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = [0_u8; 32];
    for (slot, pair) in out.iter_mut().zip(hex.chunks_exact(2)) {
        *slot = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(out)
}

/// Validate and convert one box dictionary. Every field is
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
