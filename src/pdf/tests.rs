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
    let mut item = OverlayText::new(
        crate::overlay::BoxId::default(),
        0,
        10.0,
        20.0,
        BoxKind::Text,
    );
    item.text = "a\nb".to_string();
    item.color = [0, 0, 255];
    let ops = box_operations(&item, "F_TW", (0.0, 0.0, 612.0, 800.0), 0, None);
    let operators: Vec<&str> = ops.iter().map(|op| op.operator.as_str()).collect();
    assert_eq!(
        operators,
        vec!["q", "BT", "rg", "Tf", "Tm", "Tj", "Td", "Tj", "ET", "Q"]
    );
    assert!(ops[6].operands[0].as_float().unwrap().abs() < 0.001);
    assert!((ops[6].operands[1].as_float().unwrap() + 14.4).abs() < 0.001);
}

fn tj_strings(ops: &[Operation]) -> Vec<Vec<u8>> {
    ops.iter()
        .filter(|op| op.operator == "Tj")
        .map(|op| op.operands[0].as_str().unwrap().to_vec())
        .collect()
}

fn fixed_width_item(text: &str, width_pt: f32) -> OverlayText {
    let mut item = OverlayText::new(
        crate::overlay::BoxId::default(),
        0,
        10.0,
        20.0,
        BoxKind::Text,
    );
    item.text = text.to_string();
    item.width_pt = Some(width_pt);
    item
}

#[test]
fn helvetica_width_uses_standard_metrics() {
    assert!((helvetica::helvetica_width_pt("WW", 12.0) - 22.656).abs() < 0.001);
    assert!((helvetica::helvetica_width_pt("ii", 12.0) - 5.328).abs() < 0.001);
}

#[test]
fn fixed_width_text_wraps_at_word_boundaries() {
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
fn fixed_width_and_auto_width_preserve_spaces_and_expand_tabs_consistently() {
    let mut item = fixed_width_item("  a\tb   c", 200.0);
    let fixed_ops = box_operations(&item, "F_TW", (0.0, 0.0, 612.0, 800.0), 0, None);
    item.width_pt = None;
    let auto_ops = box_operations(&item, "F_TW", (0.0, 0.0, 612.0, 800.0), 0, None);
    assert_eq!(tj_strings(&fixed_ops), vec![b"  a    b   c".to_vec()]);
    assert_eq!(tj_strings(&fixed_ops), tj_strings(&auto_ops));
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
    // MediaBox 612x792 at the origin; CropBox inset by (36, 48) -> 540x696 at (36, 48).
    let (mut fixture, out) = fixture_pdf();
    let page_id = fixture.page_ids[0];
    fixture.doc.get_dictionary_mut(page_id).unwrap().set(
        "CropBox",
        vec![36.into(), 48.into(), 576.into(), 744.into()],
    );
    let input = TempFile::new("cropped");
    fixture.doc.save(input.path()).unwrap();
    let mut pdf = PdfDoc::open(input.path()).unwrap();
    assert_eq!(pdf.sizes_pt, vec![(540.0, 696.0)]);
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let content = pdf.doc.get_page_content(page_id).unwrap();
    let tm = tm_operands(&content);
    let expected = [1.0, 0.0, 0.0, 1.0, 108.0, 662.4];
    for (actual, want) in tm.iter().zip(expected) {
        assert!((actual - want).abs() < 0.001, "cropped Tm: {tm:?}");
    }
    assert!(pdf.doc.extract_text(&[1]).unwrap().contains("Hello"));
}

#[test]
fn inverted_media_box_opens_with_positive_size() {
    // ISO 32000-1 §7.9.5: corners may be listed in any order.
    let (mut fixture, _out) = fixture_pdf();
    let page_id = fixture.page_ids[0];
    fixture.doc.get_dictionary_mut(page_id).unwrap().set(
        "MediaBox",
        vec![612.into(), 792.into(), 0.into(), 0.into()],
    );
    let input = TempFile::new("inverted-mediabox");
    fixture.doc.save(input.path()).unwrap();
    let pdf = PdfDoc::open(input.path()).unwrap();
    assert_eq!(pdf.sizes_pt, vec![(612.0, 792.0)]);
}

#[test]
fn inverted_crop_box_intersects_like_normalized_crop_box() {
    let (mut fixture, _out) = fixture_pdf();
    let page_id = fixture.page_ids[0];
    fixture.doc.get_dictionary_mut(page_id).unwrap().set(
        "CropBox",
        vec![576.into(), 744.into(), 36.into(), 48.into()],
    );
    let input = TempFile::new("inverted-cropbox");
    fixture.doc.save(input.path()).unwrap();
    let pdf = PdfDoc::open(input.path()).unwrap();
    assert_eq!(pdf.sizes_pt, vec![(540.0, 696.0)]);
}

fn render_first_page_rgba(path: &Path) -> Vec<u8> {
    let pdf = hayro::hayro_syntax::Pdf::new(std::fs::read(path).unwrap()).unwrap();
    let pix = hayro::render(
        &pdf.pages()[0],
        &hayro::RenderCache::new(),
        &hayro::hayro_interpret::InterpreterSettings::default(),
        &hayro::RenderSettings::default(),
        &hayro::PixmapSettings {
            x_scale: 1.0,
            y_scale: 1.0,
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        },
    );
    pix.data_as_u8_slice().to_vec()
}

#[test]
fn bake_wraps_existing_content_in_q_q_to_isolate_graphics_and_text_state() {
    let (mut clean_pdf, clean_out) = fixture_pdf();
    let mut clean_overlays = OverlayState::default();
    let clean_idx = clean_overlays.add(0, 72.0, 72.0, BoxKind::Text);
    clean_overlays.item_mut(clean_idx).unwrap().text = "Hello".to_string();
    clean_pdf
        .bake_and_save(&mut clean_overlays, clean_out.path())
        .unwrap();
    let clean_rgba = render_first_page_rgba(clean_out.path());
    assert!(clean_rgba.iter().any(|&b| b != 255));

    let (mut dirty_fixture, _unused) = fixture_pdf();
    let page_id = dirty_fixture.page_ids[0];
    let dirty_ops =
        b"0.75 0 0 -0.75 0 841.92 cm\n0 0 1 1 re W n\nBT 3 Tr 50 Tc 50 Tw 10 Tz 40 Ts ET";
    let orig_id = dirty_fixture
        .doc
        .add_object(Stream::new(Dictionary::new(), dirty_ops.to_vec()));
    dirty_fixture
        .doc
        .get_dictionary_mut(page_id)
        .unwrap()
        .set("Contents", Object::Reference(orig_id));
    let input = TempFile::new("dirty-state");
    let out = TempFile::new("dirty-state-out");
    dirty_fixture.doc.save(input.path()).unwrap();
    let mut pdf = PdfDoc::open(input.path()).unwrap();

    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();

    let content_ids = pdf.doc.get_page_contents(page_id);
    assert_eq!(content_ids.len(), 4);
    assert_eq!(content_ids[1], orig_id);
    let stream_ops = |id| {
        let s = pdf.doc.get_object(id).and_then(Object::as_stream).unwrap();
        Content::decode(&s.content).unwrap().operations
    };
    assert_eq!(stream_ops(content_ids[0])[0].operator, "q");
    assert_eq!(stream_ops(content_ids[2])[0].operator, "Q");
    assert!(pdf.doc.extract_text(&[1]).unwrap().contains("Hello"));
    assert_eq!(render_first_page_rgba(out.path()), clean_rgba);
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
            "Type" => "Pages", "Kids" => vec![Object::Reference(page_id)], "Count" => 1,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog", "Pages" => Object::Reference(page_tree_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));
    let out = TempFile::new(&format!("fixture-{rotate}"));
    let rotate = rotate.rem_euclid(360);
    let size = if matches!(rotate, 90 | 270) {
        (792.0, 612.0)
    } else {
        (612.0, 792.0)
    };
    let pdf = PdfDoc {
        bytes: Arc::new(Vec::new()),
        render_bytes: Arc::new(Vec::new()),
        embedded: None,
        doc,
        page_ids: vec![page_id],
        sizes_pt: vec![size],
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
    assert_eq!(overlays.items, Vec::new());
    assert!(!overlays.is_dirty());
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 1);
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
    let missing_dir = TempFile::new("missing");
    let bad_out = missing_dir.path().join("out.pdf");
    assert!(pdf.bake_and_save(&mut overlays, &bad_out).is_err());
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 0);
    assert_eq!(overlays.items.len(), 1);
    assert!(overlays.is_dirty());
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    assert_eq!(overlays.items, Vec::new());
    assert_eq!(pdf.doc.get_page_contents(page_id).len(), 1);
    let text = pdf.doc.extract_text(&[1]).unwrap();
    assert_eq!(text.matches("Hello").count(), 1, "baked >1x: {text:?}");
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
    assert!(overlays.is_dirty());
    assert_eq!(pdf.doc.get_page_contents(pdf.page_ids[0]).len(), 0);
}

#[test]
fn failed_in_place_save_leaves_original_intact() {
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let bytes_before = std::fs::read(out.path()).unwrap();
    // An in-place save that fails before staging must not touch the file.
    let mut bad = OverlayState::default();
    let idx = bad.add(99, 72.0, 72.0, BoxKind::Text);
    bad.item_mut(idx).unwrap().text = "Hello".to_string();
    assert!(pdf.bake_and_save(&mut bad, out.path()).is_err());
    assert_eq!(std::fs::read(out.path()).unwrap(), bytes_before);
    assert!(PdfDoc::open(out.path()).is_ok());
}

#[test]
fn in_place_save_replaces_file_without_temp_litter() {
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(out.path()).unwrap().permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(out.path(), permissions).unwrap();
    }
    // Second save rewrites the same path through the atomic path.
    pdf.bake_and_save(&mut OverlayState::default(), out.path())
        .unwrap();
    let file_name = out.path().file_name().unwrap().to_string_lossy();
    let prefix = format!(".{file_name}.tmp-");
    let litter: Vec<_> = std::fs::read_dir(out.path().parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with(&prefix))
        .collect();
    assert!(litter.is_empty(), "temp files left behind: {litter:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(out.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let text = PdfDoc::open(out.path())
        .unwrap()
        .doc
        .extract_text(&[1])
        .unwrap();
    assert!(text.contains("Hello"), "baked text missing: {text:?}");
}

#[cfg(unix)]
#[test]
fn in_place_save_through_symlink_updates_target_and_keeps_link() {
    let (mut pdf, out) = fixture_pdf();
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "Hello".to_string();
    pdf.bake_and_save(&mut overlays, out.path()).unwrap();
    let link = TempFile::new("symlink-link");
    std::os::unix::fs::symlink(out.path(), link.path()).unwrap();
    let mut overlays = OverlayState::default();
    let idx = overlays.add(0, 72.0, 144.0, BoxKind::Text);
    overlays.item_mut(idx).unwrap().text = "World".to_string();
    pdf.bake_and_save(&mut overlays, link.path()).unwrap();
    assert!(
        std::fs::symlink_metadata(link.path())
            .unwrap()
            .file_type()
            .is_symlink(),
        "save through a symlink replaced the link with a regular file"
    );
    assert_eq!(
        std::fs::read(link.path()).unwrap(),
        std::fs::read(out.path()).unwrap()
    );
    let text = PdfDoc::open(out.path())
        .unwrap()
        .doc
        .extract_text(&[1])
        .unwrap();
    assert!(text.contains("World"), "target not updated: {text:?}");
}

#[cfg(unix)]
#[test]
fn atomic_write_to_new_file_is_not_world_readable() {
    use std::os::unix::fs::PermissionsExt as _;
    let out = TempFile::new("atomic-new");
    PdfDoc::atomic_write(b"%PDF-fake", out.path()).unwrap();
    assert_eq!(std::fs::read(out.path()).unwrap(), b"%PDF-fake");
    assert_eq!(
        std::fs::metadata(out.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn open_without_media_or_crop_box_is_an_error() {
    let (mut fixture, _out) = fixture_pdf();
    let page_id = fixture.page_ids[0];
    fixture
        .doc
        .get_dictionary_mut(page_id)
        .unwrap()
        .remove(b"MediaBox");
    let input = TempFile::new("no-mediabox");
    fixture.doc.save(input.path()).unwrap();
    let Err(err) = PdfDoc::open(input.path()) else {
        panic!("page without MediaBox/CropBox must not open");
    };
    assert!(
        err.to_string().contains("no MediaBox or CropBox"),
        "unexpected: {err:#}"
    );
}

#[test]
fn short_media_box_aborts_instead_of_inheriting() {
    // A present-but-malformed (fewer than 4 numbers) MediaBox must
    // abort the parent walk, like a non-numeric element does, rather
    // than silently inheriting the ancestor's box.
    let (mut fixture, _out) = fixture_pdf();
    let page_id = fixture.page_ids[0];
    let parent_id = fixture
        .doc
        .get_dictionary(page_id)
        .unwrap()
        .get(b"Parent")
        .unwrap()
        .as_reference()
        .unwrap();
    fixture
        .doc
        .get_dictionary_mut(parent_id)
        .unwrap()
        .set("MediaBox", vec![0.into(), 0.into(), 100.into(), 100.into()]);
    fixture
        .doc
        .get_dictionary_mut(page_id)
        .unwrap()
        .set("MediaBox", vec![0.into(), 0.into(), 612.into()]);
    let input = TempFile::new("short-mediabox");
    fixture.doc.save(input.path()).unwrap();
    let Err(err) = PdfDoc::open(input.path()) else {
        panic!("page with short MediaBox must not open via inherited box");
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
    let times_id = pdf.doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman",
    });
    pdf.doc.get_dictionary_mut(page_id).unwrap().set(
        "Resources",
        dictionary! { "Font" => dictionary! { "F_TW" => Object::Reference(times_id) } },
    );
    assert_eq!(
        PdfDoc::ensure_helvetica(&mut pdf.doc, page_id).unwrap(),
        "F_TW1"
    );
    let fonts = pdf.doc.get_page_fonts(page_id).unwrap();
    assert!(fonts.contains_key(b"F_TW1".as_slice()));
    assert_eq!(
        PdfDoc::ensure_helvetica(&mut pdf.doc, page_id).unwrap(),
        "F_TW1"
    );
}

#[test]
fn non_standard_helvetica_on_page_is_not_reused() {
    let (mut pdf, _out) = fixture_pdf();
    let page_id = pdf.page_ids[0];
    let embedded_desc = pdf.doc.add_object(dictionary! {
        "Type" => "FontDescriptor", "FontFile2" => Object::Reference((999, 0)),
    });
    let mut font = Dictionary::new();
    for (name, dict) in [
        (
            "F1",
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        ),
        (
            "F2",
            dictionary! {
                "Type" => "Font", "Subtype" => "TrueType",
                "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
            },
        ),
        (
            "F3",
            dictionary! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
                "Encoding" => "WinAnsiEncoding", "Widths" => vec![Object::Integer(500)],
            },
        ),
        (
            "F4",
            dictionary! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
                "Encoding" => "WinAnsiEncoding", "FontDescriptor" => Object::Reference(embedded_desc),
            },
        ),
    ] {
        font.set(name, Object::Reference(pdf.doc.add_object(dict)));
    }
    pdf.doc
        .get_dictionary_mut(page_id)
        .unwrap()
        .set("Resources", dictionary! { "Font" => font });

    assert_eq!(
        PdfDoc::ensure_helvetica(&mut pdf.doc, page_id).unwrap(),
        "F_TW"
    );
    assert_eq!(
        PdfDoc::ensure_helvetica(&mut pdf.doc, page_id).unwrap(),
        "F_TW"
    );
}

#[test]
fn helvetica_from_shadowed_inherited_resources_is_not_reused() {
    // Page-local /Resources replaces the inherited one (ISO 32000-1
    // section 7.7.3.4): the ancestor's /FH Helvetica is invisible to a page
    // whose own resources only define /F1, so baking must insert a
    // fresh Helvetica into the page's effective resources instead of
    // emitting `/FH ... Tf` against a dict that has no /FH.
    let (mut pdf, _out) = fixture_pdf();
    let page_id = pdf.page_ids[0];
    let parent_id = pdf
        .doc
        .get_dictionary(page_id)
        .unwrap()
        .get(b"Parent")
        .unwrap()
        .as_reference()
        .unwrap();
    let helvetica_id = pdf.doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let times_id = pdf.doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman",
    });
    let ancestor_resources_id = pdf.doc.add_object(
        dictionary! { "Font" => dictionary! { "FH" => Object::Reference(helvetica_id) } },
    );
    let page_resources_id = pdf
        .doc
        .add_object(dictionary! { "Font" => dictionary! { "F1" => Object::Reference(times_id) } });
    pdf.doc
        .get_dictionary_mut(parent_id)
        .unwrap()
        .set("Resources", Object::Reference(ancestor_resources_id));
    pdf.doc
        .get_dictionary_mut(page_id)
        .unwrap()
        .set("Resources", Object::Reference(page_resources_id));

    let alias = PdfDoc::ensure_helvetica(&mut pdf.doc, page_id).unwrap();
    assert_eq!(alias, "F_TW");
    let fonts = effective_fonts(&pdf.doc, page_id);
    let inserted = fonts
        .get(alias.as_bytes())
        .expect("alias used for Tf must exist in effective /Font dict");
    assert!(
        is_standard_winansi_helvetica(&pdf.doc, inserted),
        "inserted font must be standard WinAnsi Helvetica"
    );
    assert!(!fonts.contains_key(b"FH".as_slice()));
    // Second call reuses the now-visible inserted Helvetica.
    assert_eq!(
        PdfDoc::ensure_helvetica(&mut pdf.doc, page_id).unwrap(),
        alias
    );
}
