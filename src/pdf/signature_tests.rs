//! Signature baking tests (split from `tests` for module size).

use super::tests::fixture_pdf;
use super::{PdfDoc, signature_font};
use crate::overlay::{BoxKind, OverlayState};
use crate::test_util::TempFile;
use lopdf::{Document, Object, ObjectId, dictionary};
use std::sync::Arc;

/// Whether `name` matches `^[A-Z]{6}\+MomoSignature$`: the subset
/// tag ISO 32000-1 §9.6.4 requires on subset font names.
fn is_subset_tagged_momo(name: &[u8]) -> bool {
    name.len() == 20
        && name[..6].iter().all(u8::is_ascii_uppercase)
        && name[6] == b'+'
        && &name[7..] == b"MomoSignature"
}

/// Every font name (`/BaseFont` on the `Type0` and `CIDFont` dicts,
/// `/FontName` on the descriptor) embedded for one signature text.
fn embedded_font_names(text: &str) -> Vec<Vec<u8>> {
    let face = signature_font::face().unwrap();
    let mut item = crate::overlay::OverlayText::new(
        crate::overlay::BoxId::default(),
        0,
        0.0,
        0.0,
        BoxKind::Signature,
    );
    item.text = text.to_string();
    let font = signature_font::SignatureFont::prepare(&face, &[&item]).unwrap();
    let mut doc = Document::new();
    font.embed(&mut doc, &face);
    let mut names = Vec::new();
    for dict in doc.objects.values().filter_map(|obj| obj.as_dict().ok()) {
        for key in [b"BaseFont".as_slice(), b"FontName".as_slice()] {
            if let Ok(name) = dict.get(key).and_then(Object::as_name) {
                names.push(name.to_vec());
            }
        }
    }
    names
}

#[test]
fn distinct_signature_subsets_get_distinct_tagged_font_names() {
    // Every embedded name for one bake is the same tagged name,
    // matching `^[A-Z]{6}\+MomoSignature$` (ISO 32000-1 §9.6.4).
    let jane = embedded_font_names("Jane Doe");
    assert_eq!(jane.len(), 3, "Type0 + CIDFont + descriptor: {jane:?}");
    assert!(
        jane.iter().all(|name| name == &jane[0]),
        "names within one bake disagree: {jane:?}"
    );
    assert!(
        is_subset_tagged_momo(&jane[0]),
        "name lacks subset tag: {:?}",
        String::from_utf8_lossy(&jane[0])
    );
    // A different glyph set embeds under a different tagged name, so
    // name-keyed font caches cannot substitute one subset for another.
    let zed = embedded_font_names("Zed Quinn");
    assert!(
        is_subset_tagged_momo(&zed[0]),
        "name lacks subset tag: {zed:?}"
    );
    assert_ne!(jane[0], zed[0], "distinct subsets share a font name");
    // The tag is deterministic: the same glyph set re-embeds under
    // the same name.
    assert_eq!(embedded_font_names("Jane Doe"), jane);
}

#[test]
fn signature_bake_embeds_momo_and_is_extractable() {
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 700.0, BoxKind::Signature);
    overlays.item_mut(idx).unwrap().text = "Jane Doe".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    assert_eq!(overlays.items, Vec::new());
    let bytes = std::fs::read(out.path()).unwrap();
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
}

#[test]
fn signature_fallback_glyph_extracts_as_question_mark_regardless_of_order() {
    // Momo has no Cyrillic zhe: it renders as the `?` glyph, so it must
    // also extract as `?` — whether the unmappable char or a real `?`
    // reaches the shared fallback glyph first.
    let face = signature_font::face().unwrap();
    assert!(face.glyph_index('\u{436}').is_none(), "test premise: zhe must be unmappable");
    assert!(face.glyph_index('?').is_some(), "test premise: `?` must be mappable");
    for input in ["\u{436}A?", "?A\u{436}"] {
        let (mut pdf, out) = fixture_pdf();
        let mut overlays = OverlayState::default();
        let idx = overlays.add(0, 72.0, 700.0, BoxKind::Signature);
        overlays.item_mut(idx).unwrap().text = input.to_string();
        pdf.bake_and_save(&mut overlays, out.path()).unwrap();
        let extracted = pdf.doc.extract_text(&[1]).unwrap();
        assert!(
            extracted.contains("?A?"),
            "input {input:?} must extract as `?A?`: {extracted:?}"
        );
    }
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
    let out = TempFile::new("multipage-sig");
    let mut pdf = PdfDoc {
        bytes: Arc::new(Vec::new()),
        render_bytes: Arc::new(Vec::new()),
        embedded: None,
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
    overlays.item_mut(first).unwrap().text = "Jane Doe".to_string();
    let second = overlays.add(1, 72.0, 700.0, BoxKind::Signature);
    overlays.item_mut(second).unwrap().text = "Zed Quinn".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
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
        .filter(|obj| matches!(obj, Object::Dictionary(dict) if dict.has(b"FontFile2")))
        .count();
    assert_eq!(descriptors, 1, "font descriptor embedded per page");
    // Both pages reference a subset-tagged Momo font and extract
    // their signature.
    for page_id in &pdf.page_ids {
        let fonts = pdf.doc.get_page_fonts(*page_id).unwrap();
        assert!(
            fonts.values().any(|font| font
                .get(b"BaseFont")
                .is_ok_and(|base| base.as_name().is_ok_and(is_subset_tagged_momo))),
            "signed page does not reference a subset-tagged MomoSignature"
        );
    }
    let extracted = pdf.doc.extract_text(&[1, 2]).unwrap();
    assert!(extracted.contains("Jane Doe"), "page 1: {extracted:?}");
    assert!(extracted.contains("Zed Quinn"), "page 2: {extracted:?}");
}

#[test]
fn reloaded_signature_bake_adds_fresh_subset_font() {
    // First session signs with a subset of glyphs.
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let first = overlays.add(0, 72.0, 700.0, BoxKind::Signature);
    overlays.item_mut(first).unwrap().text = "Jane Doe".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    drop(pdf);
    // Second session: reload from disk and sign with characters the
    // first signature never used (`Z`, `Q`, `u`, `i`). The first
    // subset cannot gain those glyphs, so this bake embeds a fresh
    // subset font alongside it instead of patching the old one.
    let mut pdf = PdfDoc::open(out.path()).unwrap();
    let mut overlays = OverlayState::default();
    let second = overlays.add(0, 72.0, 600.0, BoxKind::Signature);
    overlays.item_mut(second).unwrap().text = "Zed Quinn".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let extracted = pdf.doc.extract_text(&[1]).unwrap();
    assert!(extracted.contains("Jane Doe"), "first: {extracted:?}");
    assert!(extracted.contains("Zed Quinn"), "second: {extracted:?}");
    // Two subset font programs, each a tiny fraction of the 406 KB
    // full font (asserted against the full size, not a magic cap).
    let font_lens: Vec<i64> = pdf
        .doc
        .objects
        .values()
        .filter_map(|obj| match obj {
            Object::Stream(stream) => stream.dict.get(b"Length1").ok()?.as_i64().ok(),
            _ => None,
        })
        .collect();
    assert_eq!(font_lens.len(), 2, "expected two subset font programs");
    for len in &font_lens {
        assert!(
            usize::try_from(*len).is_ok_and(|len| len > 0 && len < crate::signature::FONT_BYTES.len() / 10),
            "font program is not a small subset: {len}"
        );
    }
    // A /W width matching Z's real advance exists in some CIDFont
    // (subset glyph ids differ per bake, so match by width value).
    let face = signature_font::face().unwrap();
    let z_gid = face.glyph_index('Z').unwrap();
    let z_width = signature_font::width_1000(
        face.glyph_hor_advance(z_gid).unwrap(),
        face.units_per_em(),
    );
    assert_ne!(z_width, 500, "test premise: Z must not use default width");
    let has_z_width = pdf.doc.objects.values().any(|obj| {
        let Ok(dict) = obj.as_dict() else { return false };
        let is_cid = dict
            .get(b"Subtype")
            .is_ok_and(|s| s.as_name().is_ok_and(|n| n == b"CIDFontType2"));
        if !is_cid {
            return false;
        }
        let Ok(widths) = dict.get(b"W").and_then(|w| w.as_array()) else {
            return false;
        };
        widths.iter().any(|w| {
            w.as_array()
                .is_ok_and(|run| run.iter().any(|v| v.as_i64().is_ok_and(|x| x == z_width)))
        })
    });
    assert!(has_z_width, "no CIDFont carries Z's width {z_width}");
}

#[test]
fn signature_encode_uses_two_byte_subset_glyph_ids() {
    let face = signature_font::face().unwrap();
    let mut item = crate::overlay::OverlayText::new(
        crate::overlay::BoxId::default(),
        0,
        0.0,
        0.0,
        BoxKind::Signature,
    );
    item.text = "Ab".to_string();
    let font = signature_font::SignatureFont::prepare(&face, &[&item]).unwrap();
    assert!(
        font.subset_len() < crate::signature::FONT_BYTES.len() / 10,
        "subset is not small: {}",
        font.subset_len()
    );
    let encoded = font.encode_hex("Ab");
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
