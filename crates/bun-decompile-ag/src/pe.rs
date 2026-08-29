//! Locate the `.bun` section in a PE32+ executable (Windows).
//!
//! Port of `src/lib/pe.ts`. Layout facts are the PE/COFF spec: the DOS
//! header's `e_lfanew` at 0x3C points at the `PE\0\0` signature; the COFF
//! header that follows has `NumberOfSections` at +6 and
//! `SizeOfOptionalHeader` at +20; the 40-byte section headers start after
//! the optional header, each with the name at +0, `VirtualSize` at +8,
//! `SizeOfRawData` at +16, and `PointerToRawData` at +20. `VirtualSize` is
//! the exact payload length while `SizeOfRawData` is file-aligned and may
//! include padding past the Bun trailer, so the smaller one wins.

#![allow(dead_code)] // Consumers land over the next commits of the migration.

use crate::bytes::{read_cstring, read_u16_le, read_u32_le};
use crate::error::Result;
use crate::section::SectionLocation;

/// `MZ`, the DOS signature as a little-endian u16 at offset 0.
pub(crate) const DOS_MAGIC_LE: u16 = 0x5A4D;

/// `PE\0\0`, the PE signature as a little-endian u32.
const PE_SIGNATURE_LE: u32 = 0x0000_4550;

/// Find the `.bun` section.
///
/// `Ok(None)` when this is not a PE or no such section exists; `Err` only
/// when a read runs past the end of the file.
pub(crate) fn find_pe_bun_section(binary: &[u8]) -> Result<Option<SectionLocation>> {
    if binary.len() < 64 {
        return Ok(None);
    }
    if read_u16_le(binary, 0)? != DOS_MAGIC_LE {
        return Ok(None);
    }

    let pe_offset = read_u32_le(binary, 0x3C)? as usize; // e_lfanew
    if pe_offset + 24 > binary.len() {
        return Ok(None);
    }
    if read_u32_le(binary, pe_offset)? != PE_SIGNATURE_LE {
        return Ok(None);
    }

    let num_sections = read_u16_le(binary, pe_offset + 6)? as usize;
    let opt_header_size = read_u16_le(binary, pe_offset + 20)? as usize;

    // Section table follows the optional header (signature 4 + COFF 20 = 24).
    let section_table = pe_offset + 24 + opt_header_size;
    if section_table + num_sections * 40 > binary.len() {
        return Ok(None);
    }

    for i in 0..num_sections {
        let sh = section_table + i * 40;

        if read_cstring(binary, sh, 8) == ".bun" {
            let virtual_size = read_u32_le(binary, sh + 8)? as usize;
            let raw_size = read_u32_le(binary, sh + 16)? as usize;
            let raw_pointer = read_u32_le(binary, sh + 20)? as usize;

            let mut size = if virtual_size > 0 && virtual_size <= raw_size {
                virtual_size
            } else {
                raw_size
            };
            // Clamp a section claiming more bytes than the file holds.
            match raw_pointer.checked_add(size) {
                Some(end) if end <= binary.len() => {}
                _ => size = binary.len().saturating_sub(raw_pointer),
            }
            if size == 0 {
                return Ok(None);
            }

            return Ok(Some(SectionLocation {
                offset: raw_pointer,
                size,
            }));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const PE_OFFSET: usize = 64;
    const SECTION_TABLE: usize = PE_OFFSET + 24; // no optional header
    const PAYLOAD_OFFSET: usize = SECTION_TABLE + 40; // one section header

    fn put_u32(b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// Minimal synthetic PE wrapping `payload` in a `.bun` section — the Rust
    /// port of the TS test suite's `wrapInPe`. `raw_padding` bytes of
    /// file-alignment padding follow the payload and are counted in
    /// `SizeOfRawData` but not `VirtualSize`.
    fn build_pe(payload: &[u8], raw_padding: usize) -> Vec<u8> {
        let mut b = vec![0_u8; PAYLOAD_OFFSET + payload.len() + raw_padding];

        b[0] = b'M';
        b[1] = b'Z';
        put_u32(&mut b, 0x3C, u32::try_from(PE_OFFSET).unwrap()); // e_lfanew
        put_u32(&mut b, PE_OFFSET, 0x0000_4550); // "PE\0\0"
        b[PE_OFFSET + 6] = 1; // NumberOfSections
        // SizeOfOptionalHeader stays 0.

        b[SECTION_TABLE..SECTION_TABLE + 4].copy_from_slice(b".bun");
        let virtual_size = u32::try_from(payload.len()).unwrap();
        let raw_size = u32::try_from(payload.len() + raw_padding).unwrap();
        put_u32(&mut b, SECTION_TABLE + 8, virtual_size); // VirtualSize
        put_u32(&mut b, SECTION_TABLE + 16, raw_size); // SizeOfRawData
        put_u32(
            &mut b,
            SECTION_TABLE + 20,
            u32::try_from(PAYLOAD_OFFSET).unwrap(),
        );

        b[PAYLOAD_OFFSET..PAYLOAD_OFFSET + payload.len()].copy_from_slice(payload);
        b
    }

    #[test]
    fn find_pe_bun_section_locates_payload() {
        let binary = build_pe(b"PAYLOAD", 0);
        let loc = find_pe_bun_section(&binary).unwrap().unwrap();
        assert_eq!(
            loc,
            SectionLocation {
                offset: PAYLOAD_OFFSET,
                size: 7,
            },
        );
    }

    #[test]
    fn find_pe_bun_section_prefers_virtual_size_over_aligned_raw_size() {
        // File alignment pads SizeOfRawData past the Bun trailer;
        // VirtualSize is the exact payload length and must win.
        let binary = build_pe(b"PAYLOAD", 12);
        let loc = find_pe_bun_section(&binary).unwrap().unwrap();
        assert_eq!(loc.size, 7);
    }

    #[test]
    fn find_pe_bun_section_with_zero_virtual_size_uses_raw_size() {
        let mut binary = build_pe(b"PAYLOAD", 0);
        put_u32(&mut binary, SECTION_TABLE + 8, 0); // VirtualSize
        let loc = find_pe_bun_section(&binary).unwrap().unwrap();
        assert_eq!(loc.size, 7);
    }

    #[test]
    fn find_pe_bun_section_clamps_size_to_file_end() {
        let mut binary = build_pe(b"PAYLOAD", 0);
        put_u32(&mut binary, SECTION_TABLE + 8, 9999); // VirtualSize
        put_u32(&mut binary, SECTION_TABLE + 16, 9999); // SizeOfRawData
        let loc = find_pe_bun_section(&binary).unwrap().unwrap();
        assert_eq!(loc.size, binary.len() - PAYLOAD_OFFSET);
    }

    #[test]
    fn find_pe_bun_section_with_pointer_past_file_returns_none() {
        let mut binary = build_pe(b"PAYLOAD", 0);
        put_u32(&mut binary, SECTION_TABLE + 20, 0x00FF_FFFF); // PointerToRawData
        assert_eq!(find_pe_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_pe_bun_section_with_wrong_dos_magic_returns_none() {
        let mut binary = build_pe(b"PAYLOAD", 0);
        binary[0] = 0xDE;
        assert_eq!(find_pe_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_pe_bun_section_with_missing_pe_signature_returns_none() {
        let mut binary = build_pe(b"PAYLOAD", 0);
        put_u32(&mut binary, PE_OFFSET, 0xDEAD_BEEF);
        assert_eq!(find_pe_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_pe_bun_section_with_other_section_name_returns_none() {
        let mut binary = build_pe(b"PAYLOAD", 0);
        binary[SECTION_TABLE..SECTION_TABLE + 5].copy_from_slice(b".text");
        assert_eq!(find_pe_bun_section(&binary).unwrap(), None);
    }
}
