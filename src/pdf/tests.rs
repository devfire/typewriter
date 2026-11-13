use super::*;
use crate::overlay::baseline_y;
use crate::test_util::TempFile;

#[test]
fn winansi_encodes_e_acute_as_single_byte() {
    assert_eq!(
        winansi_bytes("\u{e9}(ab)\\c"),
        vec![0xE9, b'(', b'a', b'b', b')', 0x5C, b'c']
    );
}

#[test]
fn winansi_unmappable_becomes_question_mark() {
    assert_eq!(winansi_bytes("\u{4e2d}"), vec![b'?']);
    assert_eq!(winansi_bytes("a\rb\u{1}c"), b"abc".to_vec());
}

#[test]
fn baseline_uses_ascent_not_full_size() {
    // 800pt page, box top at 20pt, 12pt text: 800-20-9.6 = 770.4.
    assert!((baseline_y(800.0, 20.0, 12.0) - 770.4).abs() < 0.001);
}

#[test]
fn two_lines_produce_two_tj_and_one_td() {
    let item = OverlayText {
        id: crate::overlay::BoxId::default(),
        kind: BoxKind::Text,
        page: 0,
        x_pt: 10.0,
        y_top_pt: 20.0,
        text: "a\nb".to_string(),
        size_pt: 12.0,
        color: [0, 0, 255],
        width_pt: None,
    };
    let ops = box_operations(&item, "F_TW", (0.0, 0.0, 612.0, 800.0), 0, None);
    let operators: Vec<&str> = ops.iter().map(|op| op.operator.as_str()).collect();
    assert_eq!(
        operators,
        vec!["q", "BT", "rg", "Tf", "Tm", "Tj", "Td", "Tj", "ET", "Q"]
    );
    let td = &ops[6];
    assert!(td.operands[0].as_float().unwrap().abs() < 0.001);
    assert!((td.operands[1].as_float().unwrap() + 14.4).abs() < 0.001);
}

/// Literal strings of every `Tj` in `ops`, in order.
fn tj_strings(ops: &[Operation]) -> Vec<Vec<u8>> {
    ops.iter()
        .filter(|op| op.operator == "Tj")
        .map(|op| op.operands[0].as_str().unwrap().to_vec())
        .collect()
}

fn fixed_width_item(text: &str, width_pt: f32) -> OverlayText {
    OverlayText {
        id: crate::overlay::BoxId::default(),
        kind: BoxKind::Text,
        page: 0,
        x_pt: 10.0,
        y_top_pt: 20.0,
        text: text.to_string(),
        size_pt: 12.0,
        color: [0, 0, 0],
        width_pt: Some(width_pt),
    }
}

#[test]
fn helvetica_width_uses_standard_metrics() {
    // W is 944/1000 em, i is 222/1000 em at 12pt.
    assert!((helvetica::helvetica_width_pt("WW", 12.0) - 22.656).abs() < 0.001);
    assert!((helvetica::helvetica_width_pt("ii", 12.0) - 5.328).abs() < 0.001);
}

#[test]
fn fixed_width_text_wraps_at_word_boundaries() {
    // Helvetica at 12pt: "aaaa" is 26.688pt (fits 30pt), but
    // "aaaa bbbb" is 56.7pt, so every word lands on its own line.
    let item = fixed_width_item("aaaa bbbb cccc", 30.0);
    let ops = box_operations(&item, "F_TW", (0.0, 0.0, 612.0, 800.0), 0, None);
    assert_eq!(
        tj_strings(&ops),
        vec![b"aaaa".to_vec(), b"bbbb".to_vec(), b"cccc".to_vec()]
    );
    for line in tj_strings(&ops) {
        let line = String::from_utf8(line).unwrap();
        assert!(helvetica::helvetica_width_pt(&line, 12.0) <= 30.0, "{line}");
    }
}

#[test]
fn fixed_width_long_word_is_hard_broken() {
    let item = fixed_width_item("aaaaaaaaaa", 30.0);
    let ops = box_operations(&item, "F_TW", (0.0, 0.0, 612.0, 800.0), 0, None);
    assert_eq!(
        tj_strings(&ops),
        vec![b"aaaa".to_vec(), b"aaaa".to_vec(), b"aa".to_vec()]
    );
}

#[test]
fn fixed_width_bake_wraps_text_in_saved_content() {
    let (mut pdf, out) = fixture_pdf();
    let page_id = pdf.page_ids[0];
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "aaaa bbbb cccc".to_string();
    overlays.item_mut(idx).unwrap().width_pt = Some(30.0);
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let content = pdf.doc.get_page_content(page_id).unwrap();
    let text = String::from_utf8(content).unwrap();
    assert_eq!(text.split_whitespace().filter(|t| *t == "Tj").count(), 3);
    for word in ["aaaa", "bbbb", "cccc"] {
        assert!(text.contains(word), "wrapped word {word} missing: {text}");
    }
}

/// Operands of the first `Tm` operator in a decoded content stream.
fn tm_operands(content: &[u8]) -> Vec<f32> {
    let text = String::from_utf8(content.to_vec()).unwrap();
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let pos = tokens.iter().position(|token| *token == "Tm").unwrap();
    tokens[pos - 6..pos]
        .iter()
        .map(|token| token.parse().unwrap())
        .collect()
}

#[test]
fn rotated_pages_bake_text_at_previewed_position() {
    // 612x792 page, box at display (72, 72), 12pt text: the baseline
    // sits 72 + 0.8 * 12 = 81.6pt below the displayed top edge.
    for (rotate, size, expected) in [
        (90, (792.0, 612.0), [0.0, 1.0, -1.0, 0.0, 81.6, 72.0]),
        (180, (612.0, 792.0), [-1.0, 0.0, 0.0, -1.0, 540.0, 81.6]),
        (270, (792.0, 612.0), [0.0, -1.0, 1.0, 0.0, 530.4, 720.0]),
    ] {
        let (mut fixture, out) = fixture_pdf_rotated(rotate);
        let input = TempFile::new(&format!("rotated-{rotate}"));
        fixture.doc.save(input.path()).unwrap();
        // Go through `open` so the `/Rotate` inheritance is exercised.
        let mut pdf = PdfDoc::open(input.path()).unwrap();
        assert_eq!(pdf.sizes_pt, vec![size], "rotate {rotate}");
        let page_id = pdf.page_ids[0];
        let mut overlays = OverlayState::default();
        let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
        overlays.item_mut(idx).unwrap().text = "Hello".to_string();
        pdf.bake_and_save(&mut overlays, out.path()).unwrap();
        let content = pdf.doc.get_page_content(page_id).unwrap();
        let tm = tm_operands(&content);
        assert_eq!(tm.len(), expected.len(), "rotate {rotate}");
        for (actual, want) in tm.iter().zip(expected) {
            assert!((actual - want).abs() < 0.001, "rotate {rotate}: {tm:?}");
        }
        assert!(pdf.doc.extract_text(&[1]).unwrap().contains("Hello"));
    }
}

#[test]
fn cropped_pages_bake_text_at_previewed_position() {
    // MediaBox 612x792 at the origin; CropBox inset by (36, 48) with a
    // non-zero origin, so the visible page is 540x696 starting at
    // (36, 48). hayro renders the crop box, so the baker must too.
    let mut doc = Document::new();
    let page_tree_id = doc.new_object_id();
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(page_tree_id),
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "CropBox" => vec![36.into(), 48.into(), 576.into(), 744.into()],
    });
    doc.set_object(
        page_tree_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(page_tree_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));
    let input = TempFile::new("cropped");
    let out = TempFile::new("cropped-out");
    doc.save(input.path()).unwrap();
    // Go through `open` so the effective box is derived from the file.
    let mut pdf = PdfDoc::open(input.path()).unwrap();
    // Status-bar size must agree with the renderer's (hayro) dimensions.
    assert_eq!(pdf.sizes_pt, vec![(540.0, 696.0)]);
    assert_eq!(hayro_page_sizes(&pdf.bytes), pdf.sizes_pt);
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let content = pdf.doc.get_page_content(page_id).unwrap();
    let tm = tm_operands(&content);
    // Baseline: x = 36 + 72, y = 744 - 72 - 0.8 * 12 = 662.4. The old
    // MediaBox-derived box would give (72, 710.4) instead.
    let expected = [1.0, 0.0, 0.0, 1.0, 108.0, 662.4];
    assert_eq!(tm.len(), expected.len());
    for (actual, want) in tm.iter().zip(expected) {
        assert!((actual - want).abs() < 0.001, "cropped Tm: {tm:?}");
    }
    assert!(pdf.doc.extract_text(&[1]).unwrap().contains("Hello"));
}

/// Build a minimal one-page `PdfDoc` in memory.
pub(super) fn fixture_pdf() -> (PdfDoc, TempFile) {
    fixture_pdf_rotated(0)
}

/// Build a minimal one-page `PdfDoc` whose page has `/Rotate rotate`.
fn fixture_pdf_rotated(rotate: i64) -> (PdfDoc, TempFile) {
    let mut doc = Document::new();
    let page_tree_id = doc.new_object_id();
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(page_tree_id),
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Rotate" => Object::Integer(rotate),
    });
    doc.set_object(
        page_tree_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(page_tree_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));
    let out = TempFile::new(&format!("fixture-{rotate}"));
    let rotate = rotate.rem_euclid(360);
    let swapped = matches!(rotate, 90 | 270);
    let pdf = PdfDoc {
        bytes: Arc::new(Vec::new()),
        doc,
        page_ids: vec![page_id],
        sizes_pt: vec![if swapped {
            (792.0, 612.0)
        } else {
            (612.0, 792.0)
        }],
        boxes_pt: vec![(0.0, 0.0, 612.0, 792.0)],
        rotations: vec![rotate],
    };
    (pdf, out)
}

#[test]
fn bake_drains_boxes_and_second_bake_adds_no_streams() {
    let (mut pdf, out) = fixture_pdf();
    let page_id = pdf.page_ids[0];
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    assert!(overlays.items.is_empty());
    assert!(!overlays.dirty);
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 1);
    // Second save: drained, so zero new content streams.
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 1);
    let text = pdf.doc.extract_text(&[1]).unwrap();
    assert!(text.contains("Hello"), "baked text missing: {text:?}");
}

#[test]
fn failed_bake_leaves_state_unchanged_and_retry_bakes_once() {
    let (mut pdf, out) = fixture_pdf();
    let page_id = pdf.page_ids[0];
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    // Saving into a missing directory fails after the bake has staged
    // its content stream and font objects.
    // A path under a temp-file path (not a directory): saving
    // fails after the bake has staged its content stream/fonts.
    let missing_dir = TempFile::new("missing");
    let bad_out = missing_dir.path().join("out.pdf");
    assert!(pdf.bake_and_save(&mut overlays, &bad_out).is_err());
    // Neither the document nor the overlays were mutated by the failure.
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 0);
    assert_eq!(overlays.items.len(), 1);
    assert!(overlays.dirty);
    // Retry against a writable path bakes the text exactly once.
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    assert!(overlays.items.is_empty());
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 1);
    let text = pdf.doc.extract_text(&[1]).unwrap();
    assert_eq!(
        text.matches("Hello").count(),
        1,
        "text baked more than once: {text:?}"
    );
}

#[test]
fn bake_with_out_of_range_page_bails_without_dropping_boxes() {
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let idx = overlays.add(99, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    let err = pdf.bake_and_save(&mut overlays, out.path()).unwrap_err();
    assert!(err.to_string().contains("page 100"), "unexpected: {err:#}");
    // Nothing baked, nothing cleared: the box survives for a retry.
    assert_eq!(overlays.items.len(), 1);
    assert!(overlays.dirty);
    assert_eq!(pdf.doc.get_page_contents(pdf.page_ids[0]).len(), 0);
}

#[test]
fn open_without_media_or_crop_box_is_an_error() {
    let mut doc = Document::new();
    let page_tree_id = doc.new_object_id();
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(page_tree_id),
    });
    doc.set_object(
        page_tree_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(page_tree_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));
    let input = TempFile::new("no-mediabox");
    doc.save(input.path()).unwrap();
    let Err(err) = PdfDoc::open(input.path()) else {
        panic!("page without MediaBox/CropBox must not open");
    };
    assert!(
        err.to_string().contains("no MediaBox or CropBox"),
        "unexpected: {err:#}"
    );
}

#[test]
fn existing_ftw_alias_is_not_overwritten() {
    let (mut pdf, _out) = fixture_pdf();
    let page_id = pdf.page_ids[0];
    // Pre-existing F_TW pointing at a non-Helvetica font.
    let times_id = pdf.doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman",
    });
    let mut font = Dictionary::new();
    font.set("F_TW", Object::Reference(times_id));
    let mut resources = Dictionary::new();
    resources.set("Font", font);
    pdf.doc
        .get_dictionary_mut(page_id)
        .unwrap()
        .set("Resources", resources);
    let alias = PdfDoc::ensure_helvetica(&mut pdf.doc, page_id).unwrap();
    assert_eq!(alias, "F_TW1");
    let fonts = pdf.doc.get_page_fonts(page_id).unwrap();
    assert!(fonts.contains_key(b"F_TW1".as_slice()));
    // Existing Helvetica is reused on the second call.
    assert_eq!(
        PdfDoc::ensure_helvetica(&mut pdf.doc, page_id).unwrap(),
        "F_TW1"
    );
}
