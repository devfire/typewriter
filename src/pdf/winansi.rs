//! `WinAnsi` encoding for baked Helvetica text.

/// Split text into WinAnsi-encoded lines; a single trailing empty line
/// (from a final newline) is dropped so it emits no `Tj`.
#[must_use]
pub fn winansi_lines(text: &str) -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = text.split('\n').map(winansi_bytes).collect();
    if lines.len() > 1 && lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    lines
}

/// Encode a string as `WinAnsi` bytes: one byte per character, `?` for
/// unmappable characters. `\r` is stripped, other control characters
/// below 0x20 (except `\n`) are dropped. lopdf escapes `\ ( )` itself
/// when writing literal strings, so no pre-escaping happens here.
#[must_use]
pub fn winansi_bytes(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\r' => {}
            '\n' => out.push(b'\n'),
            c if (c as u32) < 0x20 => {}
            c => out.push(winansi_char(c)),
        }
    }
    out
}

fn winansi_char(c: char) -> u8 {
    let code = c as u32;
    if (0x20..=0x7E).contains(&code) || (0xA0..=0xFF).contains(&code) {
        #[allow(clippy::cast_possible_truncation)]
        return code as u8;
    }
    // WinAnsi 0x80..=0x9F punctuation block.
    match c {
        '\u{20AC}' => 0x80,
        '\u{201A}' => 0x82,
        '\u{0192}' => 0x83,
        '\u{201E}' => 0x84,
        '\u{2026}' => 0x85,
        '\u{2020}' => 0x86,
        '\u{2021}' => 0x87,
        '\u{02C6}' => 0x88,
        '\u{2030}' => 0x89,
        '\u{0160}' => 0x8A,
        '\u{2039}' => 0x8B,
        '\u{0152}' => 0x8C,
        '\u{017D}' => 0x8E,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201C}' => 0x93,
        '\u{201D}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{02DC}' => 0x98,
        '\u{2122}' => 0x99,
        '\u{0161}' => 0x9A,
        '\u{203A}' => 0x9B,
        '\u{0153}' => 0x9C,
        '\u{017E}' => 0x9E,
        '\u{0178}' => 0x9F,
        _ => b'?',
    }
}
