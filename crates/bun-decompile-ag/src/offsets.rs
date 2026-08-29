//! Parser for the `Offsets` struct that sits immediately before the Bun
//! trailer.
//!
//! Port of `src/lib/offsets.ts`. Bun's `extern struct Offsets`
//! (little-endian), relative to the struct start:
//!
//! | bytes    | field                                                   |
//! |----------|---------------------------------------------------------|
//! | `0..8`   | `byte_count: u64` — total modules+metadata size         |
//! | `8..12`  | `modules_ptr.offset: u32`                               |
//! | `12..16` | `modules_ptr.length: u32`                               |
//! | `16..20` | `entry_point_id: u32`                                   |
//! | `20..28` | `compile_exec_argv_ptr` (V4 only)                       |
//! | `28..32` | `flags` (V4 only)                                       |
//!
//! V2/V3 pad out at 24 bytes; Bun 1.3.0+ (V4) is 32. The struct size is not
//! recorded anywhere in the file, so both sizes are probed with plausibility
//! checks — 32 first, because reading a real V4 struct 8 bytes late can still
//! pass the 24-byte probe's checks. Only the low u32 of `byte_count` is read,
//! matching the TS implementation (fine for payloads under 4 GiB).

#![allow(dead_code)] // Consumers land over the next commits of the migration.

use crate::bytes::read_u32_le;
use crate::error::{ExtractError, Result};

/// The Offsets struct parsed from just before the trailer, plus which of the
/// two struct sizes matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParsedOffsets {
    /// Total size of the modules + metadata payload (low u32 of `byte_count`).
    pub offset_byte_count: u32,
    /// Offset of the metadata region within the modules area.
    pub modules_ptr_offset: u32,
    /// Total length in bytes of the metadata entries.
    pub modules_ptr_length: u32,
    /// Index of the entry-point module.
    pub entry_point_id: u32,
    /// Size of the struct that matched: 24 (V2/V3) or 32 (V4).
    pub struct_size: usize,
}

/// Parse the Offsets struct ending at `trailer_offset`, probing the 32-byte
/// (V4) layout first, then the 24-byte (V2/V3) one.
pub(crate) fn parse_offsets(data: &[u8], trailer_offset: usize) -> Result<ParsedOffsets> {
    for struct_size in [32, 24] {
        if let Some(offsets) = try_parse_offsets_at(data, trailer_offset, struct_size)? {
            return Ok(offsets);
        }
    }
    Err(ExtractError::OffsetsNotFound)
}

fn try_parse_offsets_at(
    data: &[u8],
    trailer_offset: usize,
    struct_size: usize,
) -> Result<Option<ParsedOffsets>> {
    let Some(struct_start) = trailer_offset.checked_sub(struct_size) else {
        return Ok(None);
    };

    let offset_byte_count = read_u32_le(data, struct_start)?;
    let modules_ptr_offset = read_u32_le(data, struct_start + 8)?;
    let modules_ptr_length = read_u32_le(data, struct_start + 12)?;
    let entry_point_id = read_u32_le(data, struct_start + 16)?;

    // Plausibility checks that tell a real struct apart from payload bytes.
    let valid = u64::from(offset_byte_count) <= struct_start as u64
        && modules_ptr_offset <= offset_byte_count
        && modules_ptr_length != 0
        && modules_ptr_length <= offset_byte_count
        && entry_point_id <= 10_000;

    Ok(valid.then_some(ParsedOffsets {
        offset_byte_count,
        modules_ptr_offset,
        modules_ptr_length,
        entry_point_id,
        struct_size,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const TRAILER_LEN: usize = 16; // b"\n---- Bun! ----\n"

    /// Build `[payload fill][struct bytes][16-byte trailer space]` and return
    /// the data plus its `trailer_offset`. The trailer bytes themselves are
    /// irrelevant to this parser.
    fn buffer_with_struct(payload_len: usize, fill: u8, struct_bytes: &[u8]) -> (Vec<u8>, usize) {
        let mut data = vec![fill; payload_len];
        data.extend_from_slice(struct_bytes);
        let trailer_offset = data.len();
        data.extend_from_slice(&[0; TRAILER_LEN]);
        (data, trailer_offset)
    }

    fn v4_struct(
        byte_count: u32,
        ptr_offset: u32,
        ptr_length: u32,
        entry_point_id: u32,
        argv_offset: u32,
        argv_length: u32,
    ) -> Vec<u8> {
        let mut s = Vec::new();
        s.extend_from_slice(&u64::from(byte_count).to_le_bytes());
        s.extend_from_slice(&ptr_offset.to_le_bytes());
        s.extend_from_slice(&ptr_length.to_le_bytes());
        s.extend_from_slice(&entry_point_id.to_le_bytes());
        s.extend_from_slice(&argv_offset.to_le_bytes());
        s.extend_from_slice(&argv_length.to_le_bytes());
        s.extend_from_slice(&0_u32.to_le_bytes()); // flags
        s
    }

    fn v2_struct(
        byte_count: u32,
        ptr_offset: u32,
        ptr_length: u32,
        entry_point_id: u32,
    ) -> Vec<u8> {
        let mut s = Vec::new();
        s.extend_from_slice(&u64::from(byte_count).to_le_bytes());
        s.extend_from_slice(&ptr_offset.to_le_bytes());
        s.extend_from_slice(&ptr_length.to_le_bytes());
        s.extend_from_slice(&entry_point_id.to_le_bytes());
        s.extend_from_slice(&[0; 4]); // padding to 24
        s
    }

    #[test]
    fn parse_offsets_with_24_byte_struct_returns_v2_layout() {
        // 0xFF payload fill makes the misaligned 32-byte probe implausible.
        let (data, trailer) = buffer_with_struct(64, 0xFF, &v2_struct(64, 40, 24, 0));
        let offsets = parse_offsets(&data, trailer).unwrap();
        assert_eq!(
            offsets,
            ParsedOffsets {
                offset_byte_count: 64,
                modules_ptr_offset: 40,
                modules_ptr_length: 24,
                entry_point_id: 0,
                struct_size: 24,
            },
        );
    }

    #[test]
    fn parse_offsets_prefers_32_byte_struct_when_both_probes_pass() {
        // Constructed so the 24-byte probe (which lands 8 bytes into the V4
        // struct) would also pass its checks; the 32-byte probe must win.
        let (data, trailer) = buffer_with_struct(64, 0x00, &v4_struct(64, 40, 24, 0, 8, 0));
        let offsets = parse_offsets(&data, trailer).unwrap();
        assert_eq!(offsets.struct_size, 32);
        assert_eq!(offsets.offset_byte_count, 64);
        assert_eq!(offsets.modules_ptr_offset, 40);
        assert_eq!(offsets.modules_ptr_length, 24);
        assert_eq!(offsets.entry_point_id, 0);
    }

    #[test]
    fn parse_offsets_with_implausible_entry_point_returns_offsets_not_found() {
        let (data, trailer) = buffer_with_struct(64, 0xFF, &v2_struct(64, 40, 24, 10_001));
        assert_eq!(
            parse_offsets(&data, trailer),
            Err(ExtractError::OffsetsNotFound)
        );
    }

    #[test]
    fn parse_offsets_with_zero_metadata_length_returns_offsets_not_found() {
        let (data, trailer) = buffer_with_struct(64, 0xFF, &v2_struct(64, 40, 0, 0));
        assert_eq!(
            parse_offsets(&data, trailer),
            Err(ExtractError::OffsetsNotFound)
        );
    }

    #[test]
    fn parse_offsets_with_byte_count_past_struct_returns_offsets_not_found() {
        // byte_count claims more payload than exists before the struct.
        let (data, trailer) = buffer_with_struct(64, 0xFF, &v2_struct(65, 40, 24, 0));
        assert_eq!(
            parse_offsets(&data, trailer),
            Err(ExtractError::OffsetsNotFound)
        );
    }

    #[test]
    fn parse_offsets_with_room_for_neither_struct_returns_offsets_not_found() {
        assert_eq!(
            parse_offsets(&[0; 16], 8),
            Err(ExtractError::OffsetsNotFound)
        );
    }

    // Values pinned from the TS implementation over the committed fixtures
    // (scripts in docs/rust-migration.md; fixture layout is
    // [payload ending at trailer][8-byte total count]).

    use crate::testutil::fixture;

    #[test]
    fn parse_offsets_on_v1_1_26_fixture_matches_ts_values() {
        let data = fixture("v1.1.26-darwin-arm64");
        let payload = &data[..data.len() - 8];
        let offsets = parse_offsets(payload, payload.len() - TRAILER_LEN).unwrap();
        assert_eq!(
            offsets,
            ParsedOffsets {
                offset_byte_count: 14362,
                modules_ptr_offset: 14250,
                modules_ptr_length: 112, // 4 modules x 28-byte V2 chunks
                entry_point_id: 0,
                struct_size: 24,
            },
        );
    }

    #[test]
    fn parse_offsets_on_v1_3_9_fixture_matches_ts_values() {
        let data = fixture("v1.3.9-darwin-arm64");
        let payload = &data[..data.len() - 8];
        let offsets = parse_offsets(payload, payload.len() - TRAILER_LEN).unwrap();
        assert_eq!(
            offsets,
            ParsedOffsets {
                offset_byte_count: 14478,
                modules_ptr_offset: 14269,
                modules_ptr_length: 208, // 4 modules x 52-byte V4 chunks
                entry_point_id: 0,
                struct_size: 32,
            },
        );
    }
}
