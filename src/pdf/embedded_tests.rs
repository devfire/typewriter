//! Embedded-payload tests: saved boxes must reopen editable, and
//! editing them must replace the baked text, not duplicate it.

use super::tests::fixture_pdf;
use super::PdfDoc;
use crate::overlay::{BoxKind, OverlayState};
use crate::test_util::TempFile;
use lopdf::{Document, Object, dictionary};

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
    assert!(!overlays.dirty, "restored boxes start clean");
    let text_id = overlays
        .items
        .iter()
        .find(|b| b.kind == BoxKind::Text)
        .unwrap()
        .id;
    overlays.item_mut(text_id).unwrap().text = "Goodbye world".to_string();
    overlays.dirty = true;

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
    assert!(!baked.contains("Jane Doe"), "deleted text remains: {baked:?}");
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

#[test]
fn forged_or_newer_payloads_are_ignored_not_fatal() {
    // No BOM: the decoder's UTF-8 path for hand-written payloads.
    let valid_text = Object::String(b"Hello".to_vec(), lopdf::StringFormat::Literal);
    for payload in [
        // Newer schema version: open as a plain baked PDF.
        dictionary! { "Version" => 99_i64, "Boxes" => vec![box_dict(1, valid_text.clone())] }.into(),
        // Box on a page the document does not have.
        dictionary! { "Version" => 1_i64, "Boxes" => vec![box_dict(99, valid_text.clone())] }.into(),
        // Truncated UTF-16BE text (odd byte count after the BOM).
        dictionary! {
            "Version" => 1_i64,
            "Boxes" => vec![box_dict(1, Object::String(vec![0xFE, 0xFF, 0x00], lopdf::StringFormat::Hexadecimal))],
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
