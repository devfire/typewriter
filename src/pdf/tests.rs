use super::*;
use crate::overlay::baseline_y;

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
        let input = std::env::temp_dir().join(format!(
            "pdf-editor-test-{}-rotated-{rotate}.pdf",
            std::process::id()
        ));
        fixture.doc.save(&input).unwrap();
        // Go through `open` so the `/Rotate` inheritance is exercised.
        let mut pdf = PdfDoc::open(&input).unwrap();
        assert_eq!(pdf.sizes_pt, vec![size], "rotate {rotate}");
        let page_id = pdf.page_ids[0];
        let mut overlays = OverlayState::default();
        let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
        overlays.items[idx].text = "Hello".to_string();
        pdf.bake_and_save(&mut overlays, &out).unwrap();
        let content = pdf.doc.get_page_content(page_id).unwrap();
        let tm = tm_operands(&content);
        assert_eq!(tm.len(), expected.len(), "rotate {rotate}");
        for (actual, want) in tm.iter().zip(expected) {
            assert!((actual - want).abs() < 0.001, "rotate {rotate}: {tm:?}");
        }
        assert!(pdf.doc.extract_text(&[1]).unwrap().contains("Hello"));
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&input);
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
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let input = std::env::temp_dir().join(format!(
        "pdf-editor-test-{}-cropped-{stamp}.pdf",
        std::process::id()
    ));
    let out = std::env::temp_dir().join(format!(
        "pdf-editor-test-{}-cropped-out-{stamp}.pdf",
        std::process::id()
    ));
    doc.save(&input).unwrap();
    // Go through `open` so the effective box is derived from the file.
    let mut pdf = PdfDoc::open(&input).unwrap();
    // Status-bar size must agree with the renderer's (hayro) dimensions.
    assert_eq!(pdf.sizes_pt, vec![(540.0, 696.0)]);
    assert_eq!(hayro_page_sizes(&pdf.bytes), pdf.sizes_pt);
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.items[idx].text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, &out).unwrap();
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
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&input);
}

/// Build a minimal one-page `PdfDoc` in memory.
fn fixture_pdf() -> (PdfDoc, std::path::PathBuf) {
    fixture_pdf_rotated(0)
}

/// Build a minimal one-page `PdfDoc` whose page has `/Rotate rotate`.
fn fixture_pdf_rotated(rotate: i64) -> (PdfDoc, std::path::PathBuf) {
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
    let out = std::env::temp_dir().join(format!(
        "pdf-editor-test-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let rotate = rotate.rem_euclid(360);
    let swapped = matches!(rotate, 90 | 270);
    let pdf = PdfDoc {
        bytes: Vec::new(),
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
fn failed_bake_leaves_state_unchanged_and_retry_bakes_once() {
    let (mut pdf, out) = fixture_pdf();
    let page_id = pdf.page_ids[0];
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.items[idx].text = "Hello".to_string();
    // Saving into a missing directory fails after the bake has staged
    // its content stream and font objects.
    let bad_out = std::env::temp_dir()
        .join(format!(
            "pdf-editor-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .join("out.pdf");
    assert!(pdf.bake_and_save(&mut overlays, &bad_out).is_err());
    // Neither the document nor the overlays were mutated by the failure.
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 0);
    assert_eq!(overlays.items.len(), 1);
    assert!(overlays.dirty);
    // Retry against a writable path bakes the text exactly once.
    pdf.bake_and_save(&mut overlays, &out).unwrap();
    assert!(overlays.items.is_empty());
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 1);
    let text = pdf.doc.extract_text(&[1]).unwrap();
    assert_eq!(
        text.matches("Hello").count(),
        1,
        "text baked more than once: {text:?}"
    );
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
fn multi_page_signature_bake_embeds_font_program_once() {
    // Two pages signed in one bake share a single embedded font: the
    // font program stream (`Length1` = the TrueType file) and its
    // descriptor must each appear exactly once, however many pages
    // reference the font.
    let mut doc = Document::new();
    let page_tree_id = doc.new_object_id();
    let page_ids: Vec<ObjectId> = (0..2)
        .map(|_| {
            doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => Object::Reference(page_tree_id),
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            })
        })
        .collect();
    doc.set_object(
        page_tree_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids
                .iter()
                .map(|id| Object::Reference(*id))
                .collect::<Vec<_>>(),
            "Count" => 2,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(page_tree_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));
    let out = std::env::temp_dir().join(format!(
        "pdf-editor-test-{}-multipage-sig-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut pdf = PdfDoc {
        bytes: Vec::new(),
        doc,
        page_ids,
        sizes_pt: vec![(612.0, 792.0); 2],
        boxes_pt: vec![(0.0, 0.0, 612.0, 792.0); 2],
        rotations: vec![0; 2],
    };
    let mut overlays = OverlayState::default();
    // Distinct glyphs per page: the shared font's widths/`ToUnicode`
    // must cover the union, or the second page loses extraction.
    let first = overlays.add(0, 72.0, 700.0, BoxKind::Signature);
    overlays.items[first].text = "Jane Doe".to_string();
    let second = overlays.add(1, 72.0, 700.0, BoxKind::Signature);
    overlays.items[second].text = "Zed Quinn".to_string();
    pdf.bake_and_save(&mut overlays, &out).unwrap();
    let font_programs = pdf
        .doc
        .objects
        .values()
        .filter(|obj| matches!(obj, Object::Stream(stream) if stream.dict.has(b"Length1")))
        .count();
    assert_eq!(font_programs, 1, "font program embedded per page");
    let descriptors = pdf
        .doc
        .objects
        .values()
        .filter(|obj| {
            matches!(obj, Object::Dictionary(dict) if dict.has(b"FontFile2"))
        })
        .count();
    assert_eq!(descriptors, 1, "font descriptor embedded per page");
    // Both pages reference a Momo font and extract their signature.
    for page_id in &pdf.page_ids {
        let fonts = pdf.doc.get_page_fonts(*page_id).unwrap();
        assert!(
            fonts.values().any(|font| font
                .get(b"BaseFont")
                .is_ok_and(|base| base.as_name().is_ok_and(|n| n == b"MomoSignature"))),
            "signed page does not reference MomoSignature"
        );
    }
    let extracted = pdf.doc.extract_text(&[1, 2]).unwrap();
    assert!(extracted.contains("Jane Doe"), "page 1: {extracted:?}");
    assert!(extracted.contains("Zed Quinn"), "page 2: {extracted:?}");
    let _ = std::fs::remove_file(&out);
}

#[test]
fn reloaded_signature_bake_extends_embedded_font_with_new_glyphs() {
    // First session signs with a subset of glyphs.
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let first = overlays.add(0, 72.0, 700.0, BoxKind::Signature);
    overlays.items[first].text = "Jane Doe".to_string();
    pdf.bake_and_save(&mut overlays, &out).unwrap();
    drop(pdf);
    // Second session: reload from disk and sign with characters the
    // first signature never used (`Z`, `Q`, `u`, `i`). The reused
    // embedded font must gain widths and ToUnicode entries for them.
    let mut pdf = PdfDoc::open(&out).unwrap();
    let mut overlays = OverlayState::default();
    let second = overlays.add(0, 72.0, 600.0, BoxKind::Signature);
    overlays.items[second].text = "Zed Quinn".to_string();
    pdf.bake_and_save(&mut overlays, &out).unwrap();
    let extracted = pdf.doc.extract_text(&[1]).unwrap();
    assert!(extracted.contains("Jane Doe"), "first: {extracted:?}");
    assert!(extracted.contains("Zed Quinn"), "second: {extracted:?}");
    // Explicit /W entry for a glyph only the second signature uses.
    let face = signature_font::face().unwrap();
    let z_gid = u16::from_be_bytes(
        signature_font::encode_hex(&face, "Z")
            .try_into()
            .unwrap(),
    );
    let cid_font = pdf
        .doc
        .objects
        .values()
        .find_map(|obj| {
            let dict = obj.as_dict().ok()?;
            let is_cid = dict.get(b"Subtype").is_ok_and(|subtype| {
                subtype.as_name().is_ok_and(|name| name == b"CIDFontType2")
            });
            is_cid.then_some(dict)
        })
        .expect("signed PDF has no CIDFont");
    let widths = cid_font.get(b"W").unwrap().as_array().unwrap();
    let mut explicit = Vec::new();
    let mut idx = 0;
    while idx + 1 < widths.len() {
        if let (Ok(gid), Ok(run)) = (widths[idx].as_i64(), widths[idx + 1].as_array())
            && gid == i64::from(z_gid)
        {
            explicit = run.clone();
            break;
        }
        idx += 2;
    }
    assert_eq!(explicit.len(), 1, "no explicit /W width for glyph Z");
    assert_ne!(explicit[0].as_i64().unwrap(), 500, "Z uses default width");
    // Extending must not re-embed the font program.
    let font_programs = pdf
        .doc
        .objects
        .values()
        .filter(|obj| matches!(obj, Object::Stream(stream) if stream.dict.has(b"Length1")))
        .count();
    assert_eq!(font_programs, 1, "font program re-embedded on reload");
    let _ = std::fs::remove_file(&out);
}

#[test]
fn signature_encode_uses_two_byte_glyph_ids() {
    let face = signature_font::face().unwrap();
    let encoded = signature_font::encode_hex(&face, "Ab");
    assert_eq!(encoded.len(), 4);
    assert_ne!(&encoded[0..2], &[0, 0], "capital A mapped to .notdef");
}

#[test]
fn signature_width_scales_from_actual_units_per_em() {
    // PDF widths are in 1000-unit em space; a 2048-upem font (the common
    // TrueType value) must scale down, not pass advances through raw.
    assert_eq!(signature_font::width_1000(2_048, 2_048), 1_000);
    assert_eq!(signature_font::width_1000(1_024, 2_048), 500);
    assert_eq!(signature_font::width_1000(500, 1_000), 500);
    // Rounds to nearest instead of truncating: 1/16 em = 62.5 -> 63.
    assert_eq!(signature_font::width_1000(1, 16), 63);
    // Invalid zero units-per-em must not panic (division by zero).
    assert_eq!(signature_font::width_1000(700, 0), 700);
}
