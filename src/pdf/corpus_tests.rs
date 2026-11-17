//! Real-producer corpus round-trip: open → bake → save → re-open.
//!
//! Every fixture in `tests/fixtures/` exercises a shape that in-memory
//! lopdf-built fixtures never produce (see issue #44): Ghostscript
//! object streams, xref streams with PNG predictors, hybrid references,
//! shadowed resources, inverted boxes, inherited rotation, odd Contents,
//! and files that already contain an incremental update. The test bakes
//! one text box plus one signature box, saves incrementally, then
//! re-opens the output with both lopdf and hayro.

use super::PdfDoc;
use crate::overlay::{BoxKind, OverlayState};
use crate::test_util::TempFile;
use std::path::{Path, PathBuf};

const FIXTURES: &[(&str, &str)] = &[
    (
        "gs-objstm-xrefstm.pdf",
        "Ghostscript object streams + xref stream",
    ),
    (
        "predictor12-xrefstm.pdf",
        "xref stream with PNG predictor /Predictor 12",
    ),
    (
        "hybrid-xrefstm.pdf",
        "hybrid reference (/XRefStm in a classic trailer)",
    ),
    (
        "shadowed-resources.pdf",
        "page /Resources shadowing ancestor /Resources (#37)",
    ),
    (
        "page-tree-oddities.pdf",
        "inverted MediaBox (#38), inherited Rotate, empty/indirect Contents",
    ),
    (
        "incremental-two-eof.pdf",
        "file already containing an incremental update",
    ),
];

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
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

fn qpdf_check(path: &Path) {
    let probe = std::process::Command::new("qpdf")
        .arg("--check")
        .arg(path)
        .output();
    let out = match probe {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => panic!("failed to run qpdf --check: {err:#}"),
        Ok(out) => out,
    };
    assert!(
        out.status.success(),
        "qpdf --check failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn corpus_round_trip_open_bake_save_reopen() {
    for (name, what) in FIXTURES {
        let path = fixture_path(name);
        assert!(
            path.is_file(),
            "missing fixture {name} ({what}): {}",
            path.display()
        );
        let original = std::fs::read(&path).unwrap();
        let mut pdf = PdfDoc::open(&path)
            .unwrap_or_else(|err| panic!("open failed for {name} ({what}): {err:#}"));
        let last = pdf.page_count() - 1;
        let mut overlays = OverlayState::default();
        let text_id = overlays.add(0, 72.0, 72.0, BoxKind::Text);
        overlays.item_mut(text_id).unwrap().text = "Corpus Hello".to_string();
        let sig_id = overlays.add(last, 72.0, 144.0, BoxKind::Signature);
        overlays.item_mut(sig_id).unwrap().text = "Jane Doe".to_string();
        let out = TempFile::new(&format!("corpus-{name}"));
        pdf.bake_and_save(&mut overlays, out.path())
            .unwrap_or_else(|err| panic!("bake failed for {name} ({what}): {err:#}"));
        let saved = std::fs::read(out.path()).unwrap();
        assert!(
            saved.starts_with(&original),
            "{name} ({what}): save re-serialized the original bytes"
        );
        assert!(
            saved.len() > original.len(),
            "{name} ({what}): no update appended"
        );
        let reopened = PdfDoc::open(out.path())
            .unwrap_or_else(|err| panic!("re-open failed for {name} ({what}): {err:#}"));
        assert_eq!(
            reopened.page_count(),
            pdf.page_count(),
            "{name} ({what}): page count changed"
        );
        let text = reopened
            .doc
            .extract_text(&[1])
            .unwrap_or_else(|err| panic!("extract failed for {name} ({what}): {err:#}"));
        assert!(
            text.contains("Corpus Hello"),
            "{name} ({what}): baked text missing: {text:?}"
        );
        let hayro_pdf = hayro::hayro_syntax::Pdf::new(saved.clone())
            .unwrap_or_else(|err| panic!("hayro parse failed for {name} ({what}): {err:?}"));
        assert_eq!(
            hayro_pdf.pages().len(),
            pdf.page_count(),
            "{name} ({what}): hayro page count changed"
        );
        assert!(
            !render_first_page_rgba(out.path()).is_empty(),
            "{name} ({what}): hayro render produced no pixels"
        );
        qpdf_check(out.path());
    }
}
