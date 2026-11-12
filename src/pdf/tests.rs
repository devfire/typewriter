use super::*;

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
        kind: BoxKind::Text,
        page: 0,
        x_pt: 10.0,
        y_top_pt: 20.0,
        text: "a\nb".to_string(),
        size_pt: 12.0,
        color: [0, 0, 255],
        width_pt: None,
    };
    let ops = box_operations(&item, "F_TW", (0.0, 0.0, 612.0, 800.0), None);
    let operators: Vec<&str> = ops.iter().map(|op| op.operator.as_str()).collect();
    assert_eq!(
        operators,
        vec!["q", "BT", "rg", "Tf", "Tm", "Tj", "Td", "Tj", "ET", "Q"]
    );
    let td = &ops[6];
    assert!(td.operands[0].as_float().unwrap().abs() < 0.001);
    assert!((td.operands[1].as_float().unwrap() + 14.4).abs() < 0.001);
}

/// Build a minimal one-page `PdfDoc` in memory.
fn fixture_pdf() -> (PdfDoc, std::path::PathBuf) {
    let mut doc = Document::new();
    let page_tree_id = doc.new_object_id();
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(page_tree_id),
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
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
    let out = std::env::temp_dir().join(format!(
        "pdf-editor-test-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let pdf = PdfDoc {
        bytes: Vec::new(),
        doc,
        page_ids: vec![page_id],
        sizes_pt: vec![(612.0, 792.0)],
        boxes_pt: vec![(0.0, 0.0, 612.0, 792.0)],
    };
    (pdf, out)
}

#[test]
fn bake_drains_boxes_and_second_bake_adds_no_streams() {
    let (mut pdf, out) = fixture_pdf();
    let page_id = pdf.page_ids[0];
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.items[idx].text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, &out).unwrap();
    assert!(overlays.items.is_empty());
    assert!(!overlays.dirty);
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 1);
    // Second save: drained, so zero new content streams.
    pdf.bake_and_save(&mut overlays, &out).unwrap();
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 1);
    let text = pdf.doc.extract_text(&[1]).unwrap();
    assert!(text.contains("Hello"), "baked text missing: {text:?}");
    let _ = std::fs::remove_file(&out);
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
    let alias = pdf.ensure_helvetica(page_id).unwrap();
    assert_eq!(alias, "F_TW1");
    let fonts = pdf.doc.get_page_fonts(page_id).unwrap();
    assert!(fonts.contains_key(b"F_TW1".as_slice()));
    // Existing Helvetica is reused on the second call.
    assert_eq!(pdf.ensure_helvetica(page_id).unwrap(), "F_TW1");
}

#[test]
fn signature_bake_embeds_momo_and_is_extractable() {
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 700.0, BoxKind::Signature);
    overlays.items[idx].text = "Jane Doe".to_string();
    pdf.bake_and_save(&mut overlays, &out).unwrap();
    assert!(overlays.items.is_empty());
    let bytes = std::fs::read(&out).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("MomoSignature"), "Type0 font not embedded");
    assert!(text.contains("Identity-H"));
    assert!(text.contains("FontFile2"), "font program not embedded");
    // ToUnicode CMap makes the baked signature extractable as text.
    let extracted = pdf.doc.extract_text(&[1]).unwrap();
    assert!(
        extracted.contains("Jane Doe"),
        "signature not extractable: {extracted:?}"
    );
    let _ = std::fs::remove_file(&out);
}

#[test]
fn signature_encode_uses_two_byte_glyph_ids() {
    let face = signature_font::face().unwrap();
    let encoded = signature_font::encode_hex(&face, "Ab");
    assert_eq!(encoded.len(), 4);
    assert_ne!(&encoded[0..2], &[0, 0], "capital A mapped to .notdef");
}
