//! Fuzz `PdfDoc::open` + one bake: never panic, never hang.
//!
//! Guards lopdf/hayro upgrades (issue #44). Any panic on arbitrary
//! bytes is a bug; `Err` returns are fine.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Write as _;
use typewriter::overlay::{BoxKind, OverlayState};
use typewriter::pdf::PdfDoc;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() || data.len() > 256 * 1024 {
        return;
    }
    let mut input = tempfile::NamedTempFile::new().unwrap();
    if input.write_all(data).is_err() {
        return;
    }
    let Ok(mut pdf) = PdfDoc::open(input.path()) else {
        return;
    };
    // Exercise the embedded-payload verify/strip path on untrusted input.
    let _ = pdf.take_embedded_boxes();
    if pdf.page_count() == 0 {
        return;
    }
    let mut overlays = OverlayState::default();
    let id = overlays.add(0, 72.0, 72.0, BoxKind::Text);
    if let Some(item) = overlays.item_mut(id) {
        item.text = "Fuzz".to_string();
    }
    let out = tempfile::NamedTempFile::new().unwrap();
    let _ = pdf.bake_and_save(&mut overlays, out.path());
});
