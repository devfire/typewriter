//! Embedded-payload tests: saved boxes must reopen editable, and
//! editing them must replace the baked text, not duplicate it.

use super::PdfDoc;
use super::tests::fixture_pdf;
use crate::overlay::{BoxKind, OverlayState};
use crate::test_util::TempFile;
use lopdf::{Document, Object, ObjectId, Stream, dictionary};

use super::embedded::OVERLAY_MARKER;

/// Bake one text box (with a character `WinAnsi` cannot represent,
/// a custom size/color/width) and one signature box into `out`.
fn baked_fixture() -> TempFile {
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let text = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    let item = overlays.item_mut(text).unwrap();
    item.text = "H\u{e9}llo \u{2014} world".to_string();
    item.size_pt = 14.0;
    item.color = [10, 20, 30];
    item.width_pt = Some(120.0);
    let sig = overlays.add(0, 72.0, 200.0, BoxKind::Signature);
    overlays.item_mut(sig).unwrap().text = "Jane Doe".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    out
}

/// Text another viewer would extract from the file at `path`:
/// the baked rendering, independent of the embedded payload.
fn baked_text(path: &std::path::Path) -> String {
    let doc = Document::load(path).unwrap();
    doc.extract_text(&[1]).unwrap()
}

#[test]
fn saved_boxes_reopen_with_all_fields_and_no_baked_copy_underneath() {
    let out = baked_fixture();
    // The file on disk still renders for every other viewer.
    assert!(baked_text(out.path()).contains("Jane Doe"));

    let mut pdf = PdfDoc::open(out.path()).unwrap();
    let boxes = pdf.take_embedded_boxes();
    assert_eq!(boxes.len(), 2);
    let text = boxes.iter().find(|b| b.kind == BoxKind::Text).unwrap();
    assert_eq!(text.page, 0);
    assert!((text.x_pt - 72.0).abs() < 0.001);
    assert!((text.y_top_pt - 72.0).abs() < 0.001);
    // The em dash survives in the payload even though the baked
    // WinAnsi stream had to substitute it.
    assert_eq!(text.text, "H\u{e9}llo \u{2014} world");
    assert!((text.size_pt - 14.0).abs() < 0.001);
    assert_eq!(text.color, [10, 20, 30]);
    assert_eq!(text.width_pt, Some(120.0));
    let sig = boxes.iter().find(|b| b.kind == BoxKind::Signature).unwrap();
    assert_eq!(sig.text, "Jane Doe");

    // The baked streams are un-referenced in the opened document
    // and absent from the bytes the preview renders.
    assert!(pdf.doc.get_page_contents(pdf.page_ids[0]).is_empty());
    let rendered = Document::load_mem(&pdf.render_bytes).unwrap();
    assert!(!rendered.extract_text(&[1]).unwrap().contains("Jane Doe"));
}

#[test]
fn editing_a_restored_box_replaces_its_baked_text() {
    let out = baked_fixture();
    let mut pdf = PdfDoc::open(out.path()).unwrap();
    let mut overlays = OverlayState::default();
    overlays.restore_embedded(pdf.take_embedded_boxes());
    assert!(!overlays.is_dirty(), "restored boxes start clean");
    let text_id = overlays
        .items
        .iter()
        .find(|b| b.kind == BoxKind::Text)
        .unwrap()
        .id;
    overlays.item_mut(text_id).unwrap().text = "Goodbye world".to_string();

    let out2 = TempFile::new("embedded-edit-out");
    pdf.bake_and_save(&mut overlays, out2.path()).unwrap();
    let baked = baked_text(out2.path());
    assert!(baked.contains("Goodbye"), "new text missing: {baked:?}");
    assert_eq!(baked.matches("Goodbye").count(), 1, "duplicated: {baked:?}");
    assert!(!baked.contains("llo"), "old text still baked: {baked:?}");
    assert!(baked.contains("Jane Doe"), "signature lost: {baked:?}");

    // And the edited state is what the next open restores.
    let mut reopened = PdfDoc::open(out2.path()).unwrap();
    let boxes = reopened.take_embedded_boxes();
    assert_eq!(boxes.len(), 2);
    assert!(
        boxes.iter().any(|b| b.text == "Goodbye world"),
        "edit not persisted: {boxes:?}"
    );
}

#[test]
fn deleting_all_restored_boxes_removes_text_and_payload() {
    let out = baked_fixture();
    let mut pdf = PdfDoc::open(out.path()).unwrap();
    let mut overlays = OverlayState::default();
    overlays.restore_embedded(pdf.take_embedded_boxes());
    let ids: Vec<_> = overlays.items.iter().map(|b| b.id).collect();
    for id in ids {
        overlays.remove(id);
    }
    let out2 = TempFile::new("embedded-delete-out");
    pdf.bake_and_save(&mut overlays, out2.path()).unwrap();
    let baked = baked_text(out2.path());
    assert!(
        !baked.contains("Jane Doe"),
        "deleted text remains: {baked:?}"
    );
    let mut reopened = PdfDoc::open(out2.path()).unwrap();
    assert!(reopened.take_embedded_boxes().is_empty());
}

#[test]
fn opening_without_saving_leaves_the_file_untouched() {
    let out = baked_fixture();
    let before = std::fs::read(out.path()).unwrap();
    let mut pdf = PdfDoc::open(out.path()).unwrap();
    assert_eq!(pdf.take_embedded_boxes().len(), 2);
    drop(pdf);
    assert_eq!(std::fs::read(out.path()).unwrap(), before);
}

/// Open a fixture whose catalog carries `payload`; returns the
/// boxes the loader restored (empty when the payload is rejected).
fn restored_for_payload(payload: Object) -> Vec<crate::overlay::OverlayText> {
    let (mut fixture, _unused) = fixture_pdf();
    fixture
        .doc
        .catalog_mut()
        .unwrap()
        .set("Typewriter", payload);
    let input = TempFile::new("embedded-forged");
    fixture.doc.save(input.path()).unwrap();
    let mut pdf = PdfDoc::open(input.path()).unwrap();
    pdf.take_embedded_boxes()
}

fn box_dict(page: i64, text: Object) -> Object {
    dictionary! {
        "Kind" => "Text",
        "Page" => Object::Integer(page),
        "X" => Object::Real(72.0),
        "Y" => Object::Real(72.0),
        "Text" => text,
        "Size" => Object::Real(12.0),
        "Color" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(0)],
        "Width" => Object::Null,
    }
    .into()
}

fn stream_dict(page: i64) -> Object {
    dictionary! {
        "Page" => Object::Integer(page),
        "Obj" => Object::Integer(1),
        "Gen" => Object::Integer(0),
        "Sha256" => Object::String(vec![b'0'; 64], lopdf::StringFormat::Literal),
    }
    .into()
}

#[test]
fn forged_or_newer_payloads_are_ignored_not_fatal() {
    // No BOM: the decoder's UTF-8 path for hand-written payloads.
    let valid_text = Object::String(b"Hello".to_vec(), lopdf::StringFormat::Literal);
    for payload in [
        // Newer schema version: open as a plain baked PDF.
        dictionary! { "Version" => 99_i64, "Boxes" => vec![box_dict(1, valid_text.clone())], "Streams" => vec![stream_dict(1)] }.into(),
        // Legacy version 1, which carried no stream hashes and so
        // cannot be verified: also ignored.
        dictionary! { "Version" => 1_i64, "Boxes" => vec![box_dict(1, valid_text.clone())] }.into(),
        // Box on a page the document does not have.
        dictionary! { "Version" => 2_i64, "Boxes" => vec![box_dict(99, valid_text.clone())], "Streams" => vec![stream_dict(99)] }.into(),
        // Stream recorded for a page with no boxes.
        dictionary! { "Version" => 2_i64, "Boxes" => vec![box_dict(1, valid_text.clone())], "Streams" => vec![stream_dict(2)] }.into(),
        // Truncated UTF-16BE text (odd byte count after the BOM).
        dictionary! {
            "Version" => 2_i64,
            "Boxes" => vec![box_dict(1, Object::String(vec![0xFE, 0xFF, 0x00], lopdf::StringFormat::Hexadecimal))],
            "Streams" => vec![stream_dict(1)],
        }
        .into(),
        // Not even a dictionary.
        Object::Integer(7),
    ] {
        assert!(
            restored_for_payload(payload).is_empty(),
            "payload must be ignored"
        );
    }
}

/// Bake the standard fixture, rewrite the saved file with
/// `mutate` applied (a full re-save, as another tool would do),
/// and return the rewritten file.
fn tampered(name: &str, mutate: impl FnOnce(&mut Document)) -> TempFile {
    let source = baked_fixture();
    let mut doc = Document::load(source.path()).unwrap();
    mutate(&mut doc);
    let out = TempFile::new(name);
    doc.save(out.path()).unwrap();
    out
}

/// Object ids of streams starting with the overlay marker.
fn marked_ids(doc: &Document) -> Vec<ObjectId> {
    doc.objects
        .iter()
        .filter_map(|(id, obj)| {
            obj.as_stream()
                .ok()
                .filter(|stream| stream.content.starts_with(OVERLAY_MARKER))
                .map(|_| *id)
        })
        .collect()
}

/// Assert `path` restores nothing and its baked stream survives:
/// the payload/stream disagreement must leave the file behaving
/// as a plain baked PDF, with the content other viewers see
/// still in place after the claim attempt.
fn assert_not_restored_and_not_stripped(path: &std::path::Path) {
    let mut pdf = PdfDoc::open(path).unwrap();
    assert!(pdf.take_embedded_boxes().is_empty(), "boxes restored");
    assert!(
        !pdf.doc.get_page_contents(pdf.page_ids[0]).is_empty(),
        "baked stream was stripped"
    );
}

#[test]
fn edited_baked_stream_is_not_restored_or_stripped() {
    // Another tool touched the baked text after the save: the
    // recorded hash no longer matches, so the (stale) payload
    // must not hide the now-current baked content.
    let out = tampered("embedded-edited-stream", |doc| {
        for id in marked_ids(doc) {
            let stream = doc.get_object_mut(id).unwrap().as_stream_mut().unwrap();
            let mut content = stream.content.clone();
            content.extend_from_slice(b"\n% edited elsewhere\n");
            stream.set_content(content);
        }
    });
    assert!(baked_text(out.path()).contains("Jane Doe"));
    assert_not_restored_and_not_stripped(out.path());
}

#[test]
fn marker_prefixed_real_content_is_not_hidden() {
    // Real page content that merely starts with the marker, under
    // the very object id the payload records: the hash mismatch
    // is the only thing standing between it and being stripped,
    // and it must hold.
    let out = tampered("embedded-forged-marker", |doc| {
        for id in marked_ids(doc) {
            let stream = doc.get_object_mut(id).unwrap().as_stream_mut().unwrap();
            let mut content = OVERLAY_MARKER.to_vec();
            content.extend_from_slice(b"BT (REAL) Tj ET\n");
            stream.set_content(content);
        }
    });
    assert_not_restored_and_not_stripped(out.path());
}

#[test]
fn merged_baked_stream_is_not_restored_or_duplicated() {
    // Another tool merged the page contents into a fresh stream:
    // the recorded object id is gone from `/Contents` and the
    // marked bytes live under a new id. Restoring the boxes here
    // would draw them over the surviving baked copy.
    let out = tampered("embedded-merged-stream", |doc| {
        let page_id = *doc.get_pages().values().next().unwrap();
        let marked = marked_ids(doc);
        assert_eq!(marked.len(), 1);
        let plain = doc
            .get_object(marked[0])
            .unwrap()
            .as_stream()
            .unwrap()
            .get_plain_content()
            .unwrap();
        let merged_id = doc.add_object(Stream::new(lopdf::Dictionary::new(), plain));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Contents", Object::Reference(merged_id));
    });
    assert_not_restored_and_not_stripped(out.path());
}

#[test]
fn tampered_payload_hash_or_object_id_is_not_trusted() {
    for (name, key, value) in [
        (
            "embedded-bad-hash",
            "Sha256",
            Object::String(vec![b'0'; 64], lopdf::StringFormat::Literal),
        ),
        ("embedded-bad-obj", "Obj", Object::Integer(999_999)),
    ] {
        let out = tampered(name, |doc| {
            let catalog = doc.catalog_mut().unwrap();
            let payload = catalog
                .get_mut(b"Typewriter")
                .unwrap()
                .as_dict_mut()
                .unwrap();
            let streams = payload.get_mut(b"Streams").unwrap().as_array_mut().unwrap();
            streams[0].as_dict_mut().unwrap().set(key, value);
        });
        assert_not_restored_and_not_stripped(out.path());
    }
}

/// Plain contents of every stream in the first page's `/Contents`
/// of the file at `path`, in order.
fn page_stream_bytes(path: &std::path::Path) -> Vec<Vec<u8>> {
    let doc = Document::load(path).unwrap();
    let page_id = *doc.get_pages().values().next().unwrap();
    doc.get_page_contents(page_id)
        .iter()
        .map(|id| {
            doc.get_object(*id)
                .unwrap()
                .as_stream()
                .unwrap()
                .get_plain_content()
                .unwrap()
        })
        .collect()
}

/// A baked fixture whose page already has one (unmarked) content
/// stream before the bake, so the bake must wrap it in `q`/`Q`.
/// Returns the saved file and the pre-existing stream's contents.
fn baked_fixture_with_content() -> (TempFile, Vec<u8>) {
    let (mut pdf, out) = fixture_pdf();
    let original: Vec<u8> = b"0 0 m\n".to_vec();
    let original_id = pdf
        .doc
        .add_object(Stream::new(lopdf::Dictionary::new(), original.clone()));
    pdf.doc
        .get_dictionary_mut(pdf.page_ids[0])
        .unwrap()
        .set("Contents", Object::Reference(original_id));
    let mut overlays = OverlayState::default();
    let text = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(text).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    (out, original)
}

#[test]
fn repeated_edit_save_cycles_do_not_pile_up_wrappers() {
    let (out, original) = baked_fixture_with_content();
    // One bake: [save wrapper, original, restore wrapper, overlay].
    let streams = page_stream_bytes(out.path());
    assert_eq!(streams.len(), 4, "first bake contents: {streams:?}");
    assert_eq!(streams[0], super::embedded::WRAP_SAVE);
    assert_eq!(streams[1], original);
    assert_eq!(streams[2], super::embedded::WRAP_RESTORE);
    assert!(streams[3].starts_with(OVERLAY_MARKER));

    for cycle in 0..3 {
        let mut pdf = PdfDoc::open(out.path()).unwrap();
        let mut overlays = OverlayState::default();
        overlays.restore_embedded(pdf.take_embedded_boxes());
        assert_eq!(overlays.items.len(), 1, "cycle {cycle}: boxes lost");
        // Claiming stripped the overlay *and* its wrapper pair:
        // only the pre-existing stream remains.
        let remaining = pdf.doc.get_page_contents(pdf.page_ids[0]);
        assert_eq!(remaining.len(), 1, "cycle {cycle}: wrappers left behind");
        pdf.bake_and_save(&mut overlays, out.path()).unwrap();
        let streams = page_stream_bytes(out.path());
        assert_eq!(
            streams.len(),
            4,
            "cycle {cycle}: wrappers piled up: {streams:?}"
        );
        assert_eq!(streams[0], super::embedded::WRAP_SAVE);
        assert_eq!(streams[1], original);
        assert_eq!(streams[2], super::embedded::WRAP_RESTORE);
    }
}

#[test]
fn claimed_render_bytes_are_backing_bytes_plus_small_page_update() {
    // A large pre-existing stream: a full re-serialization for
    // the preview would re-emit (and recompress) it on open; an
    // incremental render update must not.
    let (mut pdf, out) = fixture_pdf();
    let mut original = b"%".to_vec();
    original.extend(std::iter::repeat_n(b'P', 100_000));
    original.extend_from_slice(b"\n0 0 m\n");
    let original_id = pdf
        .doc
        .add_object(Stream::new(lopdf::Dictionary::new(), original));
    pdf.doc
        .get_dictionary_mut(pdf.page_ids[0])
        .unwrap()
        .set("Contents", Object::Reference(original_id));
    let mut overlays = OverlayState::default();
    let text = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(text).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();

    let backing = std::fs::read(out.path()).unwrap();
    let mut pdf = PdfDoc::open(out.path()).unwrap();
    assert_eq!(pdf.take_embedded_boxes().len(), 1);

    // The actual file, verbatim, then only the update: no second
    // copy of the large stream (or the baked overlay) in the tail.
    assert!(
        pdf.render_bytes.starts_with(&backing),
        "render bytes do not start with the file bytes"
    );
    let tail = &pdf.render_bytes[backing.len()..];
    assert!(
        tail.len() < 4_096,
        "render update too large: {}",
        tail.len()
    );
    assert!(
        !tail.windows(64).any(|w| w.iter().all(|&b| b == b'P')),
        "pre-existing stream re-emitted in render update"
    );
    assert!(
        !tail
            .windows(OVERLAY_MARKER.len())
            .any(|w| w == OVERLAY_MARKER),
        "baked overlay re-emitted in render update"
    );

    // The render bytes still parse as the stripped document: the
    // pre-existing stream survives, the baked copy is gone.
    let rendered = Document::load_mem(&pdf.render_bytes).unwrap();
    assert!(!rendered.extract_text(&[1]).unwrap().contains("Hello"));
    let page_id = *rendered.get_pages().values().next().unwrap();
    assert_eq!(rendered.get_page_contents(page_id).len(), 1);
}

#[test]
fn legacy_unmarked_wrapper_pair_is_stripped_with_its_overlay() {
    // Rewrite a marked bake's wrappers to the pre-marking bytes,
    // as in a version 2 file: the pair in the written positions
    // is still that bake's, so claiming strips it too.
    let (source, original) = baked_fixture_with_content();
    let mut doc = Document::load(source.path()).unwrap();
    let page_id = *doc.get_pages().values().next().unwrap();
    for id in doc.get_page_contents(page_id) {
        let stream = doc.get_object_mut(id).unwrap().as_stream_mut().unwrap();
        if stream.content == super::embedded::WRAP_SAVE {
            stream.set_content(b"q\n".to_vec());
        } else if stream.content == super::embedded::WRAP_RESTORE {
            stream.set_content(b"\nQ\n".to_vec());
        }
    }
    let out = TempFile::new("embedded-legacy-wrappers");
    doc.save(out.path()).unwrap();

    let mut pdf = PdfDoc::open(out.path()).unwrap();
    assert_eq!(pdf.take_embedded_boxes().len(), 1);
    let remaining = pdf.doc.get_page_contents(pdf.page_ids[0]);
    assert_eq!(remaining.len(), 1, "legacy wrappers left behind");
    let plain = pdf
        .doc
        .get_object(remaining[0])
        .unwrap()
        .as_stream()
        .unwrap()
        .get_plain_content()
        .unwrap();
    assert_eq!(plain, original);
}

#[test]
fn tampered_wrapper_is_not_restored_or_stripped() {
    // One half of the marked pair edited after the save: the pair
    // can no longer be accounted for, so the claim must fail as
    // a whole rather than strip the overlay and leave a dangling
    // `q` (or restore the boxes over a still-baked copy).
    let (source, _original) = baked_fixture_with_content();
    let mut doc = Document::load(source.path()).unwrap();
    for obj in doc.objects.values_mut() {
        if let Ok(stream) = obj.as_stream_mut()
            && stream.content == super::embedded::WRAP_RESTORE
        {
            stream.set_content(b"\nQ Q %TypewriterWrap\n".to_vec());
        }
    }
    let out = TempFile::new("embedded-tampered-wrapper");
    doc.save(out.path()).unwrap();
    assert_not_restored_and_not_stripped(out.path());
}

#[test]
fn wrap_marker_in_box_text_still_restores() {
    // User data containing `%TypewriterWrap` inside a baked overlay must
    // not be mistaken for a wrapper stream: the overlay is recognized by
    // prefix + hash first, wrappers only by exact bytes at their positions.
    let (mut pdf, out) = super::tests::fixture_pdf();
    let mut overlays = OverlayState::default();
    let id = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(id).unwrap().text = "a %TypewriterWrap b".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let mut reopened = PdfDoc::open(out.path()).unwrap();
    let boxes = reopened.take_embedded_boxes();
    assert_eq!(boxes.len(), 1, "marker text must not void the payload");
    assert_eq!(boxes[0].text, "a %TypewriterWrap b");
}

#[test]
fn wrap_marker_in_preexisting_content_does_not_void_payload() {
    // Pre-existing page text carrying the same bytes must not invalidate
    // an otherwise valid bake: wrappers are positional exact matches.
    let (mut pdf, out) = super::tests::fixture_pdf();
    let original: Vec<u8> = b"BT (%TypewriterWrap) Tj ET\n".to_vec();
    let original_id = pdf
        .doc
        .add_object(Stream::new(lopdf::Dictionary::new(), original.clone()));
    pdf.doc
        .get_dictionary_mut(pdf.page_ids[0])
        .unwrap()
        .set("Contents", Object::Reference(original_id));
    let mut overlays = OverlayState::default();
    let id = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(id).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let mut reopened = PdfDoc::open(out.path()).unwrap();
    let boxes = reopened.take_embedded_boxes();
    assert_eq!(boxes.len(), 1, "pre-existing marker must not void payload");
    assert_eq!(boxes[0].text, "Hello");
}
