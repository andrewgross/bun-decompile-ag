//! Bun 1.4's binary sourcemap mappings, decoded back to VLQ text.
//!
//! Through 1.3.x a compiled binary stores each sourcemap's mappings as the
//! base64 VLQ text of a standard v3 sourcemap. From 1.4.0, Bun stores a
//! binary index of the same mappings instead, encoded from the bundler's VLQ at
//! compile time (Bun's `InternalSourceMap`; its
//! `src/sourcemap/InternalSourceMap.rs` is the format reference). Decoding
//! that index and re-encoding it as VLQ reproduces the bundler's mappings
//! exactly, except that separators for lines after the last mapping are gone
//! (the index cannot record them). Bun also drops VLQ name indexes and
//! one-field segments when encoding; its bundler emits neither.
//!
//! Layout, little-endian and unaligned:
//!
//! ```text
//! header        [0..8] total length   [8..16] mapping count     (u64)
//!               [16..24] original line count (u64, unused here)
//!               [24..28] window count  [28..32] stream offset     (u32)
//! window table  one 24-byte entry per window: the absolute state of its first
//!               mapping (generated line and column: i32), the window's offset
//!               into the stream (u32), then original line, original column,
//!               and source index (i32)
//! stream        the windows, then one byte of padding
//!
//! window        [0] mapping count, 1..=64   [1] flags
//!               [2..8] byte lengths of the column, original-line, and
//!                      original-column lanes (u16 each)
//!               [8..16]  bit k: the generated line advances
//!               [16..24] bit k: the original-line delta equals the
//!                        generated-line delta
//!               [24..32] bit k: the original-column delta equals the column
//!               then varint lanes (zig-zag LEB128), where delta k leads from
//!               mapping k to mapping k+1:
//!                 columns, one per delta: absolute when the line advanced,
//!                   otherwise a delta
//!                 original-line deltas, one per clear bit in [16..24]
//!                 original-column deltas, one per clear bit in [24..32]
//!                 flag bit 2: (k: u8, generated-line delta: varint) pairs for
//!                   lines that do not advance by exactly 0 or 1, 0xFF-terminated
//!                 flag bit 3: an 8-byte mask (bit k: the source is unchanged),
//!                   then a source-index delta per clear bit
//! ```
//!
//! The structure is validated throughout: a blob not laid out this way
//! fails with a reason instead of decoding to wrong mappings.

/// Why a blob could not be decoded.
pub(crate) type Reason = &'static str;

const HEADER_LEN: usize = 32;
const WINDOW_ENTRY_LEN: usize = 24;
const WINDOW_HEADER_LEN: usize = 32;
const MAX_WINDOW_MAPPINGS: u8 = 64;
const FLAG_LINE_EXCEPTIONS: u8 = 1 << 2;
const FLAG_SOURCE_DELTAS: u8 = 1 << 3;
const LINE_EXCEPTIONS_END: u8 = 0xFF;

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Whether mapping bytes are VLQ text (how Bun stored them through 1.3.x)
/// rather than the binary form, whose length fields always hold zero bytes.
pub(crate) fn is_vlq_text(raw: &[u8]) -> bool {
    raw.iter()
        .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b',' | b';'))
}

/// Decode the binary form into VLQ mappings text. `max_generated_line` bounds
/// the generated line of every mapping: a mapping cannot point past the end of
/// the module it maps, and the bound also caps the `;` run between mappings.
pub(crate) fn decode_binary(raw: &[u8], max_generated_line: usize) -> Result<String, Reason> {
    let total = read_u64(raw, 0).ok_or("it is shorter than its header")?;
    if usize::try_from(total).ok() != Some(raw.len()) {
        return Err("its length field disagrees with its size");
    }
    let mapping_count = read_u64(raw, 8).ok_or("it is shorter than its header")?;
    let window_count = read_u32(raw, 24).ok_or("it is shorter than its header")?;
    let stream_offset = read_u32(raw, 28).ok_or("it is shorter than its header")?;

    let table_end = window_count
        .checked_mul(WINDOW_ENTRY_LEN)
        .and_then(|len| len.checked_add(HEADER_LEN))
        .ok_or("its window table overflows")?;
    if stream_offset < table_end {
        return Err("its window table overlaps the stream");
    }
    // The stream ends with one byte of padding, so it is never empty.
    let stream = raw
        .get(stream_offset..)
        .filter(|stream| !stream.is_empty())
        .ok_or("its stream starts past the end")?;
    let table = raw
        .get(HEADER_LEN..table_end)
        .ok_or("its window table runs past the end")?;

    let mut out = VlqWriter::new(i64::try_from(max_generated_line).unwrap_or(i64::MAX));
    for entry in table.chunks_exact(WINDOW_ENTRY_LEN) {
        let field = |at: usize| [entry[at], entry[at + 1], entry[at + 2], entry[at + 3]];
        let position = |at: usize| i64::from(i32::from_le_bytes(field(at)));
        let first = State {
            generated_line: position(0),
            generated_column: position(4),
            original_line: position(12),
            original_column: position(16),
            source_index: position(20),
        };
        let offset = usize::try_from(u32::from_le_bytes(field(8)))
            .map_err(|_| "a window starts past the stream")?;
        let window = stream
            .get(offset..)
            .ok_or("a window starts past the stream")?;
        decode_window(window, first, &mut out)?;
    }

    if u64::try_from(out.count).ok() != Some(mapping_count) {
        return Err("its mapping count disagrees with its windows");
    }
    Ok(out.text)
}

/// Absolute state of one mapping. Kept in i64 so no sum of i32 deltas can
/// overflow before it is range-checked.
#[derive(Debug, Clone, Copy, Default)]
struct State {
    generated_line: i64,
    generated_column: i64,
    source_index: i64,
    original_line: i64,
    original_column: i64,
}

/// Decode one window: emit its first mapping from `first`, then step through
/// its deltas.
fn decode_window(window: &[u8], first: State, out: &mut VlqWriter) -> Result<(), Reason> {
    let (header, body) = window
        .split_at_checked(WINDOW_HEADER_LEN)
        .ok_or("a window header runs past the stream")?;
    let count = header[0];
    if count == 0 {
        return Err("a window holds no mappings");
    }
    if count > MAX_WINDOW_MAPPINGS {
        return Err("a window holds more than 64 mappings");
    }
    let flags = header[1];
    if flags & !(FLAG_LINE_EXCEPTIONS | FLAG_SOURCE_DELTAS) != 0 {
        return Err("a window sets unknown flags");
    }
    let line_advances = mask(header, 8);
    let original_line_follows = mask(header, 16);
    let original_column_follows = mask(header, 24);

    let mut lanes = Lanes::new(header, body)?;
    let deltas = usize::from(count) - 1;
    let line_exceptions = if flags & FLAG_LINE_EXCEPTIONS != 0 {
        lanes.line_exceptions(deltas)?
    } else {
        Vec::new()
    };
    let source_unchanged = if flags & FLAG_SOURCE_DELTAS != 0 {
        Some(lanes.source_mask()?)
    } else {
        None
    };

    let mut state = first;
    out.emit(&state)?;
    let mut exceptions = line_exceptions.into_iter().peekable();
    for k in 0..deltas {
        let mut line_delta = i64::from(bit(line_advances, k));
        let column = i64::from(lanes.columns.next()?);
        let original_line_eq = bit(original_line_follows, k);
        let mut original_line_delta = if original_line_eq {
            line_delta
        } else {
            i64::from(lanes.original_lines.next()?)
        };
        let original_column_delta = if bit(original_column_follows, k) {
            column
        } else {
            i64::from(lanes.original_columns.next()?)
        };
        if let Some((_, delta)) = exceptions.next_if(|&(at, _)| at == k) {
            line_delta = i64::from(delta);
            if original_line_eq {
                original_line_delta = line_delta;
            }
        }
        let source_delta = match source_unchanged {
            Some(unchanged) if !bit(unchanged, k) => i64::from(lanes.sources.next()?),
            _ => 0,
        };

        if line_delta == 0 {
            state.generated_column += column;
        } else {
            state.generated_line += line_delta;
            state.generated_column = column;
        }
        state.original_line += original_line_delta;
        state.original_column += original_column_delta;
        state.source_index += source_delta;
        out.emit(&state)?;
    }

    if !(lanes.columns.done() && lanes.original_lines.done() && lanes.original_columns.done()) {
        return Err("a window lane holds more values than its mappings use");
    }
    Ok(())
}

/// The varint lanes of one window.
struct Lanes<'a> {
    columns: Lane<'a>,
    original_lines: Lane<'a>,
    original_columns: Lane<'a>,
    sources: Lane<'a>,
    /// Bytes after the three length-prefixed lanes: the optional sections.
    rest: &'a [u8],
}

impl<'a> Lanes<'a> {
    /// Split the window body after its 32-byte header into lanes, using the
    /// header's three lane lengths.
    fn new(header: &[u8], body: &'a [u8]) -> Result<Self, Reason> {
        let mut rest = body;
        let mut take = |at: usize| -> Result<Lane<'a>, Reason> {
            let len = usize::from(u16::from_le_bytes([header[at], header[at + 1]]));
            let (lane, tail) = rest
                .split_at_checked(len)
                .ok_or("a window lane runs past the stream")?;
            rest = tail;
            Ok(Lane {
                bytes: lane,
                pos: 0,
            })
        };
        let columns = take(2)?;
        let original_lines = take(4)?;
        let original_columns = take(6)?;
        Ok(Self {
            columns,
            original_lines,
            original_columns,
            sources: Lane { bytes: &[], pos: 0 },
            rest,
        })
    }

    /// Parse the (k, generated-line delta) pairs. Bun writes them in
    /// increasing k, one per delta at most.
    fn line_exceptions(&mut self, deltas: usize) -> Result<Vec<(usize, i32)>, Reason> {
        let mut lane = Lane {
            bytes: self.rest,
            pos: 0,
        };
        let mut exceptions: Vec<(usize, i32)> = Vec::new();
        loop {
            let at = *lane
                .bytes
                .get(lane.pos)
                .ok_or("a window's line exceptions run past the stream")?;
            lane.pos += 1;
            if at == LINE_EXCEPTIONS_END {
                break;
            }
            let at = usize::from(at);
            if at >= deltas || exceptions.last().is_some_and(|&(last, _)| at <= last) {
                return Err("a window's line exceptions are out of order");
            }
            exceptions.push((at, lane.next()?));
        }
        self.rest = lane.bytes.get(lane.pos..).unwrap_or_default();
        Ok(exceptions)
    }

    /// The 8-byte source mask; the source-delta lane is everything after it.
    fn source_mask(&mut self) -> Result<&'a [u8], Reason> {
        let (unchanged, deltas) = self
            .rest
            .split_at_checked(8)
            .ok_or("a window's source mask runs past the stream")?;
        self.sources = Lane {
            bytes: deltas,
            pos: 0,
        };
        self.rest = &[];
        Ok(unchanged)
    }
}

/// A run of zig-zag LEB128 varints.
struct Lane<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Lane<'_> {
    /// Read the next varint: at most 5 bytes, holding at most 32 bits.
    fn next(&mut self) -> Result<i32, Reason> {
        let mut value: u64 = 0;
        for i in 0..5 {
            let byte = *self
                .bytes
                .get(self.pos)
                .ok_or("a varint runs past its lane")?;
            self.pos += 1;
            value |= u64::from(byte & 0x7F) << (7 * i);
            if byte & 0x80 == 0 {
                let value = u32::try_from(value).map_err(|_| "a varint overflows 32 bits")?;
                // Zig-zag: the low bit carries the sign. value >> 1 < 2^31.
                let magnitude = i32::try_from(value >> 1).unwrap_or(i32::MAX);
                return Ok(if value & 1 == 1 {
                    -magnitude - 1
                } else {
                    magnitude
                });
            }
        }
        Err("a varint is longer than 5 bytes")
    }

    fn done(&self) -> bool {
        self.pos == self.bytes.len()
    }
}

/// Accumulates mappings as VLQ text: `;` per generated line, `,` between
/// segments on a line, four fields per segment, each a delta from the
/// previous mapping (the column resets on every new line).
struct VlqWriter {
    text: String,
    count: usize,
    line: i64,
    max_line: i64,
    previous: State,
}

impl VlqWriter {
    fn new(max_line: i64) -> Self {
        Self {
            text: String::new(),
            count: 0,
            line: 0,
            max_line,
            previous: State::default(),
        }
    }

    fn emit(&mut self, state: &State) -> Result<(), Reason> {
        let fields = [
            state.generated_line,
            state.generated_column,
            state.source_index,
            state.original_line,
            state.original_column,
        ];
        if fields
            .iter()
            .any(|&field| field < 0 || field > i64::from(i32::MAX))
        {
            return Err("a mapping position is out of range");
        }
        if state.generated_line < self.line {
            return Err("a mapping goes back to an earlier generated line");
        }
        if state.generated_line > self.max_line {
            return Err("a mapping points past the end of the module");
        }

        if state.generated_line > self.line {
            for _ in self.line..state.generated_line {
                self.text.push(';');
            }
            self.line = state.generated_line;
            self.previous.generated_column = 0;
        } else if !self.text.is_empty() && !self.text.ends_with(';') {
            self.text.push(',');
        }
        push_vlq(
            &mut self.text,
            state.generated_column - self.previous.generated_column,
        );
        push_vlq(
            &mut self.text,
            state.source_index - self.previous.source_index,
        );
        push_vlq(
            &mut self.text,
            state.original_line - self.previous.original_line,
        );
        push_vlq(
            &mut self.text,
            state.original_column - self.previous.original_column,
        );
        self.previous = *state;
        self.count += 1;
        Ok(())
    }
}

/// Append one base64 VLQ value: sign in the low bit, then 5-bit groups, low
/// group first, with bit 5 marking a continuation.
fn push_vlq(out: &mut String, value: i64) {
    let mut rest = (value.unsigned_abs() << 1) | u64::from(value < 0);
    loop {
        let mut digit = u8::try_from(rest & 0x1F).unwrap_or(0);
        rest >>= 5;
        if rest != 0 {
            digit |= 0x20;
        }
        out.push(char::from(BASE64[usize::from(digit)]));
        if rest == 0 {
            break;
        }
    }
}

/// An 8-byte bitmask at `at` in a window header.
fn mask(header: &[u8], at: usize) -> &[u8] {
    &header[at..at + 8]
}

/// Bit `k` of an 8-byte mask; `k` is below 63 for every window delta.
fn bit(mask: &[u8], k: usize) -> bool {
    mask.get(k >> 3)
        .is_some_and(|byte| (byte >> (k & 7)) & 1 == 1)
}

fn read_u64(data: &[u8], at: usize) -> Option<u64> {
    let bytes = data.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<usize> {
    let bytes = data.get(at..at.checked_add(4)?)?;
    usize::try_from(u32::from_le_bytes(bytes.try_into().ok()?)).ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testutil::{entry_mappings, fixture_file};

    const FIXTURE: &str = "sourcemap-v1.4.1-darwin-arm64";

    fn u32_at(blob: &[u8], at: usize) -> usize {
        u32::from_le_bytes(blob[at..at + 4].try_into().unwrap()) as usize
    }

    fn put_u32(blob: &mut [u8], at: usize, value: u32) {
        blob[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Byte offset of window `n` within the blob.
    fn window_at(blob: &[u8], n: usize) -> usize {
        u32_at(blob, 28) + u32_at(blob, HEADER_LEN + n * WINDOW_ENTRY_LEN + 8)
    }

    /// Decode the fixture's blob after `corrupt` has edited it.
    fn decode_corrupted(corrupt: impl FnOnce(&mut Vec<u8>)) -> Result<String, Reason> {
        let (mut blob, contents_len) = entry_mappings(FIXTURE);
        corrupt(&mut blob);
        decode_binary(&blob, contents_len)
    }

    fn vlq(value: i64) -> String {
        let mut out = String::new();
        push_vlq(&mut out, value);
        out
    }

    fn varint(bytes: &[u8]) -> Result<i32, Reason> {
        Lane { bytes, pos: 0 }.next()
    }

    #[test]
    fn decode_binary_on_fixture_reproduces_the_bundlers_mappings() {
        let (blob, contents_len) = entry_mappings(FIXTURE);
        let map: serde_json::Value =
            serde_json::from_slice(&fixture_file(&format!("{FIXTURE}.map"))).unwrap();
        let expected = map["mappings"].as_str().unwrap().trim_end_matches(';');
        assert_eq!(decode_binary(&blob, contents_len).unwrap(), expected);
    }

    #[test]
    fn decode_binary_fixture_exercises_every_part_of_the_encoding() {
        // Guards the fixture itself: regenerating it from a smaller program
        // must not silently drop coverage.
        let (blob, _) = entry_mappings(FIXTURE);
        let windows = u32_at(&blob, 24);
        let flags: Vec<u8> = (0..windows)
            .map(|n| blob[window_at(&blob, n) + 1])
            .collect();
        assert!(windows >= 2, "one window only");
        assert!(
            flags.iter().any(|f| f & FLAG_LINE_EXCEPTIONS != 0),
            "{flags:?}"
        );
        assert!(
            flags.iter().any(|f| f & FLAG_SOURCE_DELTAS != 0),
            "{flags:?}"
        );
    }

    #[test]
    fn is_vlq_text_accepts_vlq_and_rejects_the_binary_form() {
        assert!(is_vlq_text(b";;AAaO,SAAS+/;AAC"));
        assert!(!is_vlq_text(&entry_mappings(FIXTURE).0));
        assert!(!is_vlq_text(b"AAAA BBBB"));
    }

    #[test]
    fn decode_binary_when_length_field_disagrees_errors() {
        assert_eq!(
            decode_corrupted(|b| b[0] ^= 1),
            Err("its length field disagrees with its size"),
        );
        // A truncated blob trips the same check.
        assert_eq!(
            decode_corrupted(|b| b.truncate(b.len() - 1)),
            Err("its length field disagrees with its size"),
        );
    }

    #[test]
    fn decode_binary_when_mapping_count_disagrees_errors() {
        assert_eq!(
            decode_corrupted(|b| b[8] ^= 1),
            Err("its mapping count disagrees with its windows"),
        );
    }

    #[test]
    fn decode_binary_when_stream_offset_is_out_of_place_errors() {
        assert_eq!(
            decode_corrupted(|b| {
                let len = u32::try_from(b.len()).unwrap();
                put_u32(b, 28, len);
            }),
            Err("its stream starts past the end"),
        );
        assert_eq!(
            decode_corrupted(|b| put_u32(b, 28, 33)),
            Err("its window table overlaps the stream"),
        );
    }

    #[test]
    fn decode_binary_when_window_starts_past_the_stream_errors() {
        assert_eq!(
            decode_corrupted(|b| put_u32(b, HEADER_LEN + 8, u32::MAX)),
            Err("a window starts past the stream"),
        );
    }

    #[test]
    fn decode_binary_when_window_count_is_out_of_range_errors() {
        assert_eq!(
            decode_corrupted(|b| {
                let at = window_at(b, 0);
                b[at] = 0;
            }),
            Err("a window holds no mappings"),
        );
        assert_eq!(
            decode_corrupted(|b| {
                let at = window_at(b, 0);
                b[at] = 65;
            }),
            Err("a window holds more than 64 mappings"),
        );
    }

    #[test]
    fn decode_binary_when_window_sets_unknown_flags_errors() {
        assert_eq!(
            decode_corrupted(|b| {
                let at = window_at(b, 0) + 1;
                b[at] |= 1 << 4;
            }),
            Err("a window sets unknown flags"),
        );
    }

    #[test]
    fn decode_binary_when_lane_runs_past_the_stream_errors() {
        assert_eq!(
            decode_corrupted(|b| {
                let at = window_at(b, 0) + 2;
                b[at..at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
            }),
            Err("a window lane runs past the stream"),
        );
    }

    #[test]
    fn decode_binary_when_lane_holds_unused_values_errors() {
        // One mapping fewer leaves a value unread in every lane.
        assert_eq!(
            decode_corrupted(|b| {
                let at = window_at(b, 0);
                b[at] -= 1;
            }),
            Err("a window lane holds more values than its mappings use"),
        );
    }

    #[test]
    fn decode_binary_when_mapping_points_past_the_module_errors() {
        let (blob, _) = entry_mappings(FIXTURE);
        assert_eq!(
            decode_binary(&blob, 3),
            Err("a mapping points past the end of the module"),
        );
    }

    #[test]
    fn decode_binary_when_window_seed_is_negative_errors() {
        assert_eq!(
            decode_corrupted(|b| put_u32(b, HEADER_LEN + 4, u32::MAX)),
            Err("a mapping position is out of range"),
        );
    }

    /// Assemble a blob holding one window, seeded at generated line
    /// `seed_line` with every other position 0.
    fn one_window_blob(seed_line: u32, mapping_count: u64, window: &[u8]) -> Vec<u8> {
        let stream_offset = HEADER_LEN + WINDOW_ENTRY_LEN;
        let total = stream_offset + window.len() + 1; // one byte of padding
        let mut blob = Vec::new();
        blob.extend_from_slice(&(total as u64).to_le_bytes());
        blob.extend_from_slice(&mapping_count.to_le_bytes());
        blob.extend_from_slice(&10_u64.to_le_bytes()); // original line count, unused
        blob.extend_from_slice(&1_u32.to_le_bytes());
        blob.extend_from_slice(&u32::try_from(stream_offset).unwrap().to_le_bytes());
        blob.extend_from_slice(&seed_line.to_le_bytes());
        blob.extend_from_slice(&[0; WINDOW_ENTRY_LEN - 4]); // window at stream offset 0
        blob.extend_from_slice(window);
        blob.push(0);
        blob
    }

    /// Two mappings whose delta jumps generated lines through a line
    /// exception (zig-zag `jump`), lands on column 2, and has the original
    /// column follow the column. The original line either follows the jump
    /// or moves 5 on its own.
    fn jump_window(original_line_follows: bool, jump: u8) -> Vec<u8> {
        let mut window = vec![0; WINDOW_HEADER_LEN];
        window[0] = 2;
        window[1] = FLAG_LINE_EXCEPTIONS;
        window[2] = 1; // column lane length
        window[4] = u8::from(!original_line_follows); // original-line lane length
        window[8] = 1; // the line advances...
        window[16] = u8::from(original_line_follows);
        window[24] = 1;
        window.push(0x04); // column 2
        if !original_line_follows {
            window.push(0x0A); // original line +5
        }
        window.extend_from_slice(&[0, jump, LINE_EXCEPTIONS_END]); // ...by `jump`
        window
    }

    #[test]
    fn decode_binary_line_exception_carries_a_following_original_line() {
        let blob = one_window_blob(0, 2, &jump_window(true, 0x06)); // jump 3
        assert_eq!(decode_binary(&blob, 100), Ok("AAAA;;;EAGE".to_owned()));
    }

    #[test]
    fn decode_binary_line_exception_leaves_an_independent_original_line() {
        let blob = one_window_blob(0, 2, &jump_window(false, 0x06)); // jump 3
        assert_eq!(decode_binary(&blob, 100), Ok("AAAA;;;EAKE".to_owned()));
    }

    #[test]
    fn decode_binary_when_a_line_exception_goes_backwards_errors() {
        let blob = one_window_blob(5, 2, &jump_window(false, 0x05)); // jump -3
        assert_eq!(
            decode_binary(&blob, 100),
            Err("a mapping goes back to an earlier generated line"),
        );
    }

    #[test]
    fn lane_next_decodes_zigzag_varints() {
        assert_eq!(varint(&[0x00]), Ok(0));
        assert_eq!(varint(&[0x01]), Ok(-1));
        assert_eq!(varint(&[0x02]), Ok(1));
        assert_eq!(varint(&[0x7F]), Ok(-64));
        assert_eq!(varint(&[0x80, 0x01]), Ok(64));
        assert_eq!(varint(&[0xFE, 0xFF, 0xFF, 0xFF, 0x0F]), Ok(i32::MAX));
        assert_eq!(varint(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]), Ok(i32::MIN));
    }

    #[test]
    fn lane_next_with_malformed_varint_errors() {
        assert_eq!(varint(&[]), Err("a varint runs past its lane"));
        assert_eq!(varint(&[0x80]), Err("a varint runs past its lane"));
        assert_eq!(
            varint(&[0xFF, 0xFF, 0xFF, 0xFF, 0x7F]),
            Err("a varint overflows 32 bits"),
        );
        assert_eq!(
            varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err("a varint is longer than 5 bytes"),
        );
    }

    #[test]
    fn push_vlq_encodes_standard_base64_vlq() {
        assert_eq!(vlq(0), "A");
        assert_eq!(vlq(1), "C");
        assert_eq!(vlq(-1), "D");
        assert_eq!(vlq(15), "e");
        assert_eq!(vlq(16), "gB");
        assert_eq!(vlq(-16), "hB");
        assert_eq!(vlq(1000), "w+B");
        assert_eq!(vlq(i64::from(i32::MAX)), "+/////D");
        assert_eq!(vlq(-i64::from(i32::MAX)), "//////D");
    }
}
