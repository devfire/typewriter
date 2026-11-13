//! Standard-14 Helvetica glyph widths, in 1000-unit em space.
//!
//! The baker writes text boxes in Helvetica (see `super::ensure_helvetica`),
//! whose widths are defined by the font's AFM data and need not be embedded
//! in the PDF. Knowing them lets the baker wrap fixed-width boxes exactly
//! as the saved file will be rendered. Bytes are `WinAnsi` codes, matching
//! `super::winansi_bytes`; entry 0 covers the space character.

/// Widths for bytes `0x20..=0x7E` (printable ASCII), 1000-unit em.
#[rustfmt::skip]
const ASCII_WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 222, 333, 333, 389, 584, 278, 333, 278, 278, // 0x20-0x2F
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0x30-0x39 digits
    278, 278, 584, 584, 584, 556, 1015, // 0x3A-0x40
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, // A-P
    778, 722, 667, 611, 722, 667, 944, 667, 667, 611, // Q-Z
    278, 333, 278, 469, 556, 222, // 0x5B-0x60
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, // a-p
    556, 333, 500, 278, 556, 500, 722, 500, 500, 500, // q-z
    334, 260, 334, 584, // 0x7B-0x7E
];

/// Widths for bytes `0x80..=0xFF` (`WinAnsi` punctuation and Latin-1).
/// Undefined slots use the `?` width (556), as does the encoder's fallback.
#[rustfmt::skip]
const HIGH_WIDTHS: [u16; 128] = [
    556, 556, 278, 556, 278, 1000, 556, 556, 333, 1000, 667, 278, 1000, 556, 611, 556, // 0x80-0x8F
    556, 222, 222, 400, 400, 500, 556, 1000, 333, 389, 556, 278, 944, 556, 500, 667, // 0x90-0x9F
    278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333, // 0xA0-0xAF
    400, 584, 333, 333, 333, 556, 556, 278, 333, 333, 365, 556, 834, 834, 834, 611, // 0xB0-0xBF
    667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278, // 0xC0-0xCF
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 667, // 0xD0-0xDF
    556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278, // 0xE0-0xEF
    556, 556, 556, 556, 556, 556, 556, 584, 556, 556, 556, 556, 556, 500, 556, 500, // 0xF0-0xFF
];

/// Width of one `WinAnsi` byte in 1000-unit em space.
fn width_1000(byte: u8) -> u16 {
    match byte {
        0x20..=0x7E => ASCII_WIDTHS[usize::from(byte - 0x20)],
        0x80..=0xFF => HIGH_WIDTHS[usize::from(byte - 0x80)],
        _ => 556,
    }
}

/// Rendered width of `text` in points at `size_pt`, as a PDF reader
/// using standard Helvetica will lay it out. Unmappable characters are
/// measured as the `?` the encoder substitutes for them.
#[must_use]
pub(super) fn helvetica_width_pt(text: &str, size_pt: f32) -> f32 {
    let units: u32 = super::winansi_bytes(text)
        .iter()
        .map(|byte| u32::from(width_1000(*byte)))
        .sum();
    #[allow(clippy::cast_precision_loss)]
    let units = units as f32;
    units * size_pt / 1000.0
}
