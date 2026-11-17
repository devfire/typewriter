//! Bounded PDF stream decoding for untrusted content.
//!
//! `lopdf`'s `Stream::get_plain_content` drives its Flate decoder with
//! `read_to_end` into a growing `Vec`, so the full expansion is allocated
//! before any caller can check its length. Verification must stop at the
//! configured limit *during* decoding, including intermediate results of
//! filter chains: a small highly-compressible stream must not exhaust
//! memory before its hash can reject it.
//!
//! This module reimplements lopdf 0.36.0's filter chain (`FlateDecode`,
//! `LZWDecode`, `ASCII85Decode`, PNG predictors) with a hard output cap:
//! each stage stops once it would exceed `limit`, and the whole decode
//! reports `None`. `None` also covers unreadable, undecodable, and
//! unknown-filter streams; callers fail closed (reject the payload or
//! abort the save), never trusting the content.

use lopdf::{Dictionary, Object, Stream};
use std::io::{Read, Write};

/// Decode `stream`'s content, enforcing `limit` on the compressed input,
/// every intermediate filter output, and the final result.
///
/// `None` means oversize, unreadable, undecodable, or unsupported: the
/// caller must fail closed (reject the payload / abort the save).
pub(super) fn decode_stream_content(stream: &Stream, limit: usize) -> Option<Vec<u8>> {
    if stream.content.len() > limit {
        return None;
    }
    let filters = match stream.filters() {
        Ok(filters) if !filters.is_empty() => filters,
        _ => return (stream.content.len() <= limit).then(|| stream.content.clone()),
    };
    let params = stream
        .dict
        .get(b"DecodeParms")
        .and_then(Object::as_dict)
        .ok();
    let mut current: Option<Vec<u8>> = None;
    for filter in filters {
        let input: &[u8] = current.as_deref().unwrap_or(&stream.content);
        let decoded = if filter == b"FlateDecode".as_slice() {
            apply_predictor(bounded_flate(input, limit)?, params, limit)?
        } else if filter == b"LZWDecode".as_slice() {
            apply_predictor(bounded_lzw(input, params, limit)?, params, limit)?
        } else if filter == b"ASCII85Decode".as_slice() {
            bounded_ascii85(input, limit)?
        } else {
            return None;
        };
        if decoded.len() > limit {
            return None;
        }
        current = Some(decoded);
    }
    current
}

/// Flate-decode at most `limit` bytes; `None` on oversize (or empty input
/// yielding empty output, matching lopdf). Decode errors are ignored like
/// lopdf (which warns and keeps the partial output): the partial bytes
/// still fail the caller's hash check, so this stays fail-closed.
fn bounded_flate(input: &[u8], limit: usize) -> Option<Vec<u8>> {
    if input.is_empty() {
        return Some(Vec::new());
    }
    let cap = limit.saturating_add(1);
    let decoder = flate2::read::ZlibDecoder::new(input);
    let mut out = Vec::new();
    out.try_reserve(cap.min(8192)).ok()?;
    let take = u64::try_from(cap).ok()?;
    let mut limited = decoder.take(take);
    let _ = limited.read_to_end(&mut out);
    (out.len() <= limit).then_some(out)
}

/// Writer that stops at `cap` bytes so the LZW decoder cannot grow the
/// output without bound. Overflow surfaces as an I/O error that aborts
/// decoding; the caller maps a full buffer to `None`.
struct CappedWriter {
    buf: Vec<u8>,
    cap: usize,
}

impl Write for CappedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let remaining = self.cap.saturating_sub(self.buf.len());
        if remaining == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "stream exceeds limit",
            ));
        }
        let take = remaining.min(data.len());
        self.buf.extend_from_slice(&data[..take]);
        if take < data.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "stream exceeds limit",
            ));
        }
        Ok(take)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// LZW-decode at most `limit` bytes, with lopdf's `EarlyChange` handling.
/// Like lopdf (and `bounded_flate`), decode errors yield the partial
/// output, which the caller still refuses to trust.
fn bounded_lzw(input: &[u8], params: Option<&Dictionary>, limit: usize) -> Option<Vec<u8>> {
    use weezl::{BitOrder, decode::Decoder};
    const MIN_BITS: u8 = 9;
    let early_change = params
        .and_then(|dict| dict.get(b"EarlyChange").ok())
        .and_then(|obj| Object::as_i64(obj).ok())
        .is_none_or(|value| value != 0);
    let mut decoder = if early_change {
        Decoder::with_tiff_size_switch(BitOrder::Msb, MIN_BITS - 1)
    } else {
        Decoder::new(BitOrder::Msb, MIN_BITS - 1)
    };
    let mut capped = CappedWriter {
        buf: Vec::new(),
        cap: limit.saturating_add(1),
    };
    capped.buf.try_reserve(8192.min(capped.cap)).ok()?;
    {
        let stream = decoder.into_stream(&mut capped);
        let _ = stream.decode_all(input);
    }
    (capped.buf.len() <= limit).then_some(capped.buf)
}

/// ASCII85-decode at most `limit` bytes. Logic mirrors lopdf 0.36.0
/// (`z` groups, whitespace skipped, stop at the first out-of-alphabet
/// byte, `~>` end marker), with a cap checked after every group.
fn bounded_ascii85(input: &[u8], limit: usize) -> Option<Vec<u8>> {
    let cap = limit.saturating_add(1);
    let body = if input.len() >= 2 && input[input.len() - 2..] == *b"~>" {
        &input[..input.len() - 2]
    } else {
        input
    };
    let mut output = Vec::new();
    output.try_reserve(body.len().min(cap).min(8192)).ok()?;
    let mut buffer: u32 = 0;
    let mut count: usize = 0;
    for &byte in body {
        if byte == b'z' {
            if count != 0 {
                return None;
            }
            output.extend_from_slice(&[0, 0, 0, 0]);
            if output.len() > limit {
                return None;
            }
            continue;
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            break;
        }
        buffer = buffer.checked_mul(85)?;
        buffer += u32::from(byte - b'!');
        count += 1;
        if count == 5 {
            output.extend_from_slice(&buffer.to_be_bytes());
            if output.len() > limit {
                return None;
            }
            buffer = 0;
            count = 0;
        }
    }
    if count > 0 {
        for _ in count..5 {
            buffer = buffer.checked_mul(85)?;
            buffer += 84;
        }
        let bytes = buffer.to_be_bytes();
        let tail = bytes.get(..count - 1)?;
        output.extend_from_slice(tail);
        if output.len() > limit {
            return None;
        }
    }
    Some(output)
}

/// PNG-predictor stage for Flate/LZW output, mirroring lopdf 0.36.0:
/// predictor `1` (or absent params) is a no-op; `10..=15` run the PNG
/// row filters. Attacker-controlled `Columns`/`Colors`/`BitsPerComponent`
/// size the row buffer, so they are checked against `limit` before any
/// allocation, and the total output is capped too.
fn apply_predictor(data: Vec<u8>, params: Option<&Dictionary>, limit: usize) -> Option<Vec<u8>> {
    let Some(params) = params else {
        return Some(data);
    };
    let predictor = params
        .get(b"Predictor")
        .and_then(Object::as_i64)
        .ok()
        .unwrap_or(1);
    if !(10..=15).contains(&predictor) {
        return Some(data);
    }
    let columns = params
        .get(b"Columns")
        .and_then(Object::as_i64)
        .ok()
        .unwrap_or(1)
        .max(1);
    let colors = params
        .get(b"Colors")
        .and_then(Object::as_i64)
        .ok()
        .unwrap_or(1)
        .max(1);
    let bits = params
        .get(b"BitsPerComponent")
        .and_then(Object::as_i64)
        .ok()
        .unwrap_or(8)
        .max(8);
    let columns = usize::try_from(columns).ok()?;
    let colors = usize::try_from(colors).ok()?;
    let bits = usize::try_from(bits).ok()?;
    let bytes_per_pixel = colors.checked_mul(bits)?.checked_div(8)?;
    let bytes_per_row = bytes_per_pixel.checked_mul(columns)?;
    if bytes_per_row == 0 || bytes_per_row > limit {
        return None;
    }
    decode_frame_bounded(&data, bytes_per_pixel, bytes_per_row, limit)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterType {
    None,
    Sub,
    Up,
    Avg,
    Paeth,
}

impl TryFrom<u8> for FilterType {
    type Error = ();
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Sub),
            2 => Ok(Self::Up),
            3 => Ok(Self::Avg),
            4 => Ok(Self::Paeth),
            _ => Err(()),
        }
    }
}

fn paeth_predict(left: u8, above: u8, upper_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(above) - i16::from(upper_left);
    let dist = |value: u8| (estimate - i16::from(value)).abs();
    if dist(left) <= dist(above) && dist(left) <= dist(upper_left) {
        left
    } else if dist(above) <= dist(upper_left) {
        above
    } else {
        upper_left
    }
}

fn decode_row(filter: FilterType, bpp: usize, previous: &[u8], current: &mut [u8]) {
    use FilterType::{Avg, None, Paeth, Sub, Up};
    let len = current.len();
    let bpp = bpp.min(len);
    match filter {
        None => (),
        Sub => {
            for i in bpp..len {
                // Lengths are checked by the caller, so these index
                // into rows of equal size.
                current[i] = current[i].wrapping_add(current[i - bpp]);
            }
        }
        Up => {
            for i in 0..len {
                current[i] = current[i].wrapping_add(previous[i]);
            }
        }
        Avg => {
            for i in 0..bpp {
                current[i] = current[i].wrapping_add(previous[i] / 2);
            }
            for i in bpp..len {
                let add = current[i - bpp].wrapping_add(previous[i] / 2);
                current[i] = current[i].wrapping_add(add);
            }
        }
        Paeth => {
            for i in 0..bpp {
                current[i] = current[i].wrapping_add(paeth_predict(0, previous[i], 0));
            }
            for i in bpp..len {
                let predict = paeth_predict(current[i - bpp], previous[i], previous[i - bpp]);
                current[i] = current[i].wrapping_add(predict);
            }
        }
    }
}

/// PNG frame decode with the total output capped at `limit`. Returns
/// `None` for invalid filter bytes, truncated rows, or oversize output
/// (all fail closed upstream).
fn decode_frame_bounded(
    content: &[u8],
    bytes_per_pixel: usize,
    bytes_per_row: usize,
    limit: usize,
) -> Option<Vec<u8>> {
    let mut previous = Vec::new();
    previous.try_reserve(bytes_per_row).ok()?;
    previous.resize(bytes_per_row, 0);
    let mut current = Vec::new();
    current.try_reserve(bytes_per_row).ok()?;
    current.resize(bytes_per_row, 0);
    let mut decoded = Vec::new();
    decoded.try_reserve(content.len().min(limit)).ok()?;
    let mut pos: usize = 0;
    while pos < content.len() {
        let filter = FilterType::try_from(*content.get(pos)?).ok()?;
        pos += 1;
        let end = pos.checked_add(bytes_per_row)?;
        let row = content.get(pos..end)?;
        pos = end;
        current.copy_from_slice(row);
        decode_row(filter, bytes_per_pixel, &previous, &mut current);
        if decoded.len().checked_add(bytes_per_row)? > limit {
            return None;
        }
        decoded.extend_from_slice(&current);
        std::mem::swap(&mut previous, &mut current);
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::decode_stream_content;
    use flate2::{Compression, write::ZlibEncoder};
    use lopdf::{Dictionary, Stream, dictionary};
    use std::io::Write as _;

    fn flate_stream(payload: &[u8], dict_extra: Dictionary) -> Stream {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(payload).unwrap();
        let content = encoder.finish().unwrap();
        let mut dict = dictionary! {"Filter" => "FlateDecode"};
        for (key, value) in dict_extra {
            dict.set(key, value);
        }
        Stream::new(dict, content)
    }

    #[test]
    fn uncompressed_passthrough_respects_limit() {
        let stream = Stream::new(Dictionary::new(), vec![1, 2, 3]);
        assert_eq!(
            decode_stream_content(&stream, 8).as_deref(),
            Some([1, 2, 3].as_slice())
        );
        assert_eq!(decode_stream_content(&stream, 2), None);
    }

    #[test]
    fn flate_bomb_stops_at_limit() {
        let big = vec![0x20; 256 * 1024];
        let stream = flate_stream(&big, Dictionary::new());
        assert!(stream.content.len() < 2048);
        assert_eq!(decode_stream_content(&stream, 4096), None);
        assert_eq!(
            decode_stream_content(&stream, big.len()).as_deref(),
            Some(big.as_slice())
        );
    }

    #[test]
    fn intermediate_filter_output_is_capped() {
        // Double Flate: inner expansion already exceeds the limit even
        // though the final output would be tiny.
        let big = vec![b'A'; 128 * 1024];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&big).unwrap();
        let once = encoder.finish().unwrap();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&once).unwrap();
        let twice = encoder.finish().unwrap();
        let dict = dictionary! {"Filter" => vec!["FlateDecode".into(), "FlateDecode".into()]};
        let stream = Stream::new(dict, twice);
        assert_eq!(decode_stream_content(&stream, 4096), None);
    }

    #[test]
    fn unknown_filter_fails_closed() {
        let dict = dictionary! {"Filter" => "DCTDecode"};
        let stream = Stream::new(dict, vec![0xFF, 0xD8]);
        assert_eq!(decode_stream_content(&stream, 1024), None);
    }
}
