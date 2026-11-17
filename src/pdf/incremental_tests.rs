//! Incremental-save tests (split from `tests` for module size).

use super::PdfDoc;
use super::tests::fixture_pdf;
use crate::overlay::{BoxKind, OverlayState};
use crate::test_util::TempFile;

#[test]
fn incremental_save_keeps_original_bytes_as_prefix() {
    // A signed PDF stays verifiable only if the signed byte range is
    // untouched: the saved file must start with the exact original
    // bytes, with the update (and its `/Prev` xref chain) appended.
    let (mut fixture, _unused) = fixture_pdf();
    let input = TempFile::new("incremental-input");
    fixture.doc.save(input.path()).unwrap();
    let original = std::fs::read(input.path()).unwrap();
    let mut pdf = PdfDoc::open(input.path()).unwrap();
    let out = TempFile::new("incremental-out");
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let saved = std::fs::read(out.path()).unwrap();
    assert!(
        saved.starts_with(&original),
        "save re-serialized the original bytes"
    );
    assert!(saved.len() > original.len(), "no update appended");
    assert!(
        saved[original.len()..].windows(5).any(|w| w == b"/Prev"),
        "appended trailer does not chain to the previous xref"
    );
    let text = PdfDoc::open(out.path())
        .unwrap()
        .doc
        .extract_text(&[1])
        .unwrap();
    assert!(text.contains("Hello"), "baked text missing: {text:?}");
}

#[test]
fn incremental_save_appends_to_xref_table_pdf() {
    // Most producer PDFs use a classic xref table, not the xref
    // stream lopdf creates by default: the appended section must
    // use the same form and still chain via `/Prev`.
    let (mut fixture, _unused) = fixture_pdf();
    fixture.doc.reference_table =
        lopdf::xref::Xref::new(0, lopdf::xref::XrefType::CrossReferenceTable);
    let input = TempFile::new("incremental-table-input");
    fixture.doc.save(input.path()).unwrap();
    let original = std::fs::read(input.path()).unwrap();
    assert!(
        original.windows(5).any(|w| w == b"\nxref"),
        "test premise: fixture is not an xref-table PDF"
    );
    let mut pdf = PdfDoc::open(input.path()).unwrap();
    let out = TempFile::new("incremental-table-out");
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let saved = std::fs::read(out.path()).unwrap();
    assert!(saved.starts_with(&original));
    assert!(
        saved[original.len()..].windows(5).any(|w| w == b"/Prev"),
        "appended trailer does not chain to the previous xref"
    );
    let text = PdfDoc::open(out.path())
        .unwrap()
        .doc
        .extract_text(&[1])
        .unwrap();
    assert!(text.contains("Hello"), "baked text missing: {text:?}");
}

#[test]
fn second_save_without_reopen_appends_to_first_save() {
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let first = std::fs::read(out.path()).unwrap();
    // No reload: the next save must build on the first save's bytes,
    // not on the (now stale) bytes the document was opened from.
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 144.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "World".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let second = std::fs::read(out.path()).unwrap();
    assert!(
        second.starts_with(&first),
        "second save did not append to the first"
    );
    let text = PdfDoc::open(out.path())
        .unwrap()
        .doc
        .extract_text(&[1])
        .unwrap();
    assert!(text.contains("Hello"), "first bake lost: {text:?}");
    assert!(text.contains("World"), "second bake missing: {text:?}");
}

#[test]
fn failed_incremental_save_restores_doc_and_retry_bakes_once() {
    // Same guarantee as the staged-clone path, but for a document
    // with backing bytes: a failed write must roll `doc` back to
    // the parsed original, not leave the bake half-applied.
    let (mut fixture, _unused) = fixture_pdf();
    let input = TempFile::new("incremental-failure-input");
    fixture.doc.save(input.path()).unwrap();
    let mut pdf = PdfDoc::open(input.path()).unwrap();
    let page_id = pdf.page_ids[0];
    let out = TempFile::new("incremental-failure-out");
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    let missing_dir = TempFile::new("incremental-missing");
    let bad_out = missing_dir.path().join("out.pdf");
    assert!(pdf.bake_and_save(&mut overlays, &bad_out).is_err());
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 0);
    assert_eq!(overlays.items.len(), 1);
    assert!(overlays.dirty);
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 1);
    let text = pdf.doc.extract_text(&[1]).unwrap();
    assert_eq!(text.matches("Hello").count(), 1, "baked >1x: {text:?}");
}
