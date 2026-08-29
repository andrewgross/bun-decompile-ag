//! Bounds-checked little-endian reads over a byte slice.
//!
//! The TS implementation reads through a `DataView`, where an out-of-range
//! access throws `RangeError`; every read here returns
//! [`ExtractError::Truncated`] instead. This module is what makes the crate's
//! no-panic contract hold on malformed input.

use crate::error::{ExtractError, Result};

/// Read exactly `N` bytes starting at `offset`.
fn read_array<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N]> {
    let truncated = || ExtractError::Truncated { offset, len: N };
    let end = offset.checked_add(N).ok_or_else(truncated)?;
    let bytes = data.get(offset..end).ok_or_else(truncated)?;
    let mut out = [0_u8; N];
    out.copy_from_slice(bytes);
    Ok(out)
}

/// Read a little-endian `u16` at `offset`.
pub(crate) fn read_u16_le(data: &[u8], offset: usize) -> Result<u16> {
    read_u16_endian(data, offset, true)
}

/// Read a little-endian `u32` at `offset`.
pub(crate) fn read_u32_le(data: &[u8], offset: usize) -> Result<u32> {
    read_u32_endian(data, offset, true)
}

/// Read a little-endian `u64` at `offset`.
pub(crate) fn read_u64_le(data: &[u8], offset: usize) -> Result<u64> {
    read_u64_endian(data, offset, true)
}

/// Read a `u16` at `offset` in the given byte order (`little` mirrors the
/// boolean the TS code passes to `DataView` getters; the ELF parser derives
/// it from `EI_DATA`).
pub(crate) fn read_u16_endian(data: &[u8], offset: usize, little: bool) -> Result<u16> {
    let bytes = read_array(data, offset)?;
    Ok(if little {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

/// Read a `u32` at `offset` in the given byte order.
pub(crate) fn read_u32_endian(data: &[u8], offset: usize, little: bool) -> Result<u32> {
    let bytes = read_array(data, offset)?;
    Ok(if little {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

/// Read a `u64` at `offset` in the given byte order.
pub(crate) fn read_u64_endian(data: &[u8], offset: usize, little: bool) -> Result<u64> {
    let bytes = read_array(data, offset)?;
    Ok(if little {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    })
}

/// Read a NUL-terminated string of at most `max_len` bytes starting at
/// `offset`, as used for section and segment names in Mach-O, ELF, and PE
/// headers. Out-of-range offsets yield an empty string and invalid UTF-8 is
/// replaced, matching the TS `readCString` (whose `TextDecoder` never throws).
pub(crate) fn read_cstring(data: &[u8], offset: usize, max_len: usize) -> String {
    let Some(window) = data.get(offset..) else {
        return String::new();
    };
    let window = &window[..window.len().min(max_len)];
    let end = window.iter().position(|&b| b == 0).unwrap_or(window.len());
    String::from_utf8_lossy(&window[..end]).into_owned()
}

/// Find the first occurrence of `needle` whose start index lies in
/// `[from, limit)`. Only the start is bounded — a match may extend past
/// `limit` — mirroring the TS `indexOfBytes` used by version scanning.
#[allow(dead_code)] // Its consumer is version scanning, ported later in the migration.
pub(crate) fn find_bytes(data: &[u8], needle: &[u8], from: usize, limit: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let max = limit.min((data.len() + 1).saturating_sub(needle.len()));
    (from..max).find(|&i| data[i..].starts_with(needle))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn read_u32_le_at_valid_offset_returns_value() {
        let data = [0xEF, 0xBE, 0xAD, 0xDE, 0x99];
        assert_eq!(read_u32_le(&data, 0).unwrap(), 0xDEAD_BEEF);
        assert_eq!(read_u32_le(&data, 1).unwrap(), 0x99DE_ADBE);
    }

    #[test]
    fn read_u32_le_past_end_returns_truncated() {
        let data = [0_u8; 4];
        assert_eq!(
            read_u32_le(&data, 1),
            Err(ExtractError::Truncated { offset: 1, len: 4 }),
        );
    }

    #[test]
    fn read_u16_le_and_u64_le_round_trip() {
        let data = 0x0123_4567_89AB_CDEF_u64.to_le_bytes();
        assert_eq!(read_u16_le(&data, 0).unwrap(), 0xCDEF);
        assert_eq!(read_u64_le(&data, 0).unwrap(), 0x0123_4567_89AB_CDEF);
    }

    #[test]
    fn read_u64_le_with_offset_overflow_returns_truncated() {
        assert!(read_u64_le(&[], usize::MAX).is_err());
    }

    #[test]
    fn read_cstring_stops_at_nul() {
        assert_eq!(read_cstring(b"__BUN\0junk", 0, 16), "__BUN");
    }

    #[test]
    fn read_cstring_without_nul_reads_max_len() {
        assert_eq!(read_cstring(b"abcdef", 1, 3), "bcd");
    }

    #[test]
    fn read_cstring_past_end_returns_empty() {
        assert_eq!(read_cstring(b"ab", 5, 4), "");
        assert_eq!(read_cstring(b"ab", 2, 4), "");
    }

    #[test]
    fn read_cstring_with_invalid_utf8_replaces_bytes() {
        assert_eq!(read_cstring(&[0xFF, 0xFE], 0, 8), "\u{FFFD}\u{FFFD}");
    }

    #[test]
    fn find_bytes_locates_needle_after_from() {
        let data = b"bun bun";
        assert_eq!(find_bytes(data, b"bun", 0, data.len()), Some(0));
        assert_eq!(find_bytes(data, b"bun", 1, data.len()), Some(4));
    }

    #[test]
    fn find_bytes_limit_bounds_start_not_end() {
        // A match may straddle the limit; only its start must precede it.
        let data = b"aabbun";
        assert_eq!(find_bytes(data, b"bun", 0, 4), Some(3));
        assert_eq!(find_bytes(data, b"bun", 0, 3), None);
    }

    #[test]
    fn find_bytes_with_missing_needle_returns_none() {
        assert_eq!(find_bytes(b"aaaa", b"zz", 0, 4), None);
        assert_eq!(find_bytes(b"aa", b"aaaa", 0, 2), None);
    }
}
