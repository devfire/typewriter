//! Signature baking tests (split from `tests` for module size).

use super::tests::fixture_pdf;
use super::{PdfDoc, signature_font};
use crate::overlay::{BoxKind, OverlayState};
use crate::test_util::TempFile;
use lopdf::{Document, Object, ObjectId, dictionary};
use std::sync::Arc;

#[test]
fn signature_bake_embeds_momo_and_is_extractable() {
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 700.0, BoxKind::Signature);
    overlays.item_mut(idx).unwrap().text = "Jane Doe".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    assert!(overlays.items.is_empty());
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
}

#[test]
fn reloaded_signature_bake_extends_embedded_font_with_new_glyphs() {
    // First session signs with a subset of glyphs.
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let first = overlays.add(0, 72.0, 700.0, BoxKind::Signature);
    overlays.item_mut(first).unwrap().text = "Jane Doe".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    drop(pdf);
    // Second session: reload from disk and sign with characters the
    // first signature never used (`Z`, `Q`, `u`, `i`). The reused
    // embedded font must gain widths and ToUnicode entries for them.
    let mut pdf = PdfDoc::open(out.path()).unwrap();
    let mut overlays = OverlayState::default();
    let second = overlays.add(0, 72.0, 600.0, BoxKind::Signature);
    overlays.item_mut(second).unwrap().text = "Zed Quinn".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let extracted = pdf.doc.extract_text(&[1]).unwrap();
    assert!(extracted.contains("Jane Doe"), "first: {extracted:?}");
    assert!(extracted.contains("Zed Quinn"), "second: {extracted:?}");
    // Explicit /W entry for a glyph only the second signature uses.
    let face = signature_font::face().unwrap();
    let z_gid = u16::from_be_bytes(signature_font::encode_hex(&face, "Z").try_into().unwrap());
    let cid_font = pdf
        .doc
        .objects
        .values()
        .find_map(|obj| {
            let dict = obj.as_dict().ok()?;
            let is_cid = dict
                .get(b"Subtype")
                .is_ok_and(|subtype| subtype.as_name().is_ok_and(|name| name == b"CIDFontType2"));
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
