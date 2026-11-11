//! Locate the `__BUN/__bun` section in a Mach-O 64-bit executable (macOS).
//!
//! Port of `src/lib/macho.ts`. Bun embeds its payload in this section on
//! macOS in every release that targets it. Layout facts are the Mach-O ABI:
//! a 32-byte `mach_header_64` (`ncmds` at +16, `sizeofcmds` at +20), then the
//! load commands; an `LC_SEGMENT_64` command has its 16-byte segment name at
//! +8, `nsects` at +64, and its 80-byte `section_64` headers inline from +72,
//! each with the name at +0, `size: u64` at +40, and the file offset at +48.

use crate::bytes::{read_cstring, read_u32_le, read_u64_le};
use crate::error::Result;
use crate::section::SectionLocation;

/// `MH_MAGIC_64`, as the u32 value read little-endian from offset 0 (the
/// on-disk byte sequence is `CF FA ED FE`).
pub(crate) const MACHO_MAGIC_64_LE: u32 = 0xFEED_FACF;

const LC_SEGMENT_64: u32 = 0x19;

/// Find the `__BUN/__bun` section.
///
/// `Ok(None)` when this is not a Mach-O 64 binary or no such section exists;
/// `Err` only when a header length field points outside the file (where the
/// TS implementation let a `DataView` `RangeError` escape).
pub(crate) fn find_macho_bun_section(binary: &[u8]) -> Result<Option<SectionLocation>> {
    if binary.len() < 32 {
        return Ok(None);
    }
    if read_u32_le(binary, 0)? != MACHO_MAGIC_64_LE {
        return Ok(None);
    }

    let ncmds = read_u32_le(binary, 16)?;
    let sizeofcmds = read_u32_le(binary, 20)? as usize;
    let header_size = 32;

    if binary.len() < header_size + sizeofcmds {
        return Ok(None);
    }

    let mut offset = header_size;

    for _ in 0..ncmds {
        if offset + 8 > binary.len() {
            break;
        }

        let cmd = read_u32_le(binary, offset)?;
        let cmdsize = read_u32_le(binary, offset + 4)? as usize;

        if cmdsize < 8 || offset + cmdsize > binary.len() {
            break;
        }

        if cmd == LC_SEGMENT_64 && read_cstring(binary, offset + 8, 16) == "__BUN" {
            let nsects = read_u32_le(binary, offset + 64)?;
            let mut sect_offset = offset + 72;

            for _ in 0..nsects {
                if sect_offset + 80 > binary.len() {
                    break;
                }

                if read_cstring(binary, sect_offset, 16) == "__bun" {
                    let size = read_u64_le(binary, sect_offset + 40)?;
                    let file_offset = read_u32_le(binary, sect_offset + 48)?;
                    // A size that cannot index memory (only conceivable for a
                    // >4 GiB claim on a 32-bit host) is treated as no section.
                    let Ok(size) = usize::try_from(size) else {
                        return Ok(None);
                    };
                    return Ok(Some(SectionLocation {
                        offset: file_offset as usize,
                        size,
                    }));
                }

                sect_offset += 80;
            }
        }

        offset += cmdsize;
    }

    Ok(None)
}

/// Minimal synthetic Mach-O 64 for tests: header, optional filler command,
/// one segment with one section pointing at `payload` appended after the
/// headers.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) fn build_macho(
    segname: &str,
    sectname: &str,
    leading_cmd: Option<(u32, usize)>,
    payload: &[u8],
) -> Vec<u8> {
    const SEGMENT_CMD_LEN: usize = 72;
    const SECTION_LEN: usize = 80;
    let (lead_cmd, lead_len) = leading_cmd.unwrap_or((0, 0));
    let seg_cmdsize = SEGMENT_CMD_LEN + SECTION_LEN;
    let sizeofcmds = lead_len + seg_cmdsize;
    let payload_offset = 32 + sizeofcmds;
    let mut b = vec![0_u8; payload_offset + payload.len()];

    b[0..4].copy_from_slice(&MACHO_MAGIC_64_LE.to_le_bytes());
    let ncmds: u32 = if lead_len > 0 { 2 } else { 1 };
    b[16..20].copy_from_slice(&ncmds.to_le_bytes());
    b[20..24].copy_from_slice(&u32::try_from(sizeofcmds).unwrap().to_le_bytes());

    if lead_len > 0 {
        b[32..36].copy_from_slice(&lead_cmd.to_le_bytes());
        b[36..40].copy_from_slice(&u32::try_from(lead_len).unwrap().to_le_bytes());
    }

    let seg = 32 + lead_len;
    b[seg..seg + 4].copy_from_slice(&LC_SEGMENT_64.to_le_bytes());
    b[seg + 4..seg + 8].copy_from_slice(&u32::try_from(seg_cmdsize).unwrap().to_le_bytes());
    b[seg + 8..seg + 8 + segname.len()].copy_from_slice(segname.as_bytes());
    b[seg + 64..seg + 68].copy_from_slice(&1_u32.to_le_bytes()); // nsects

    let sect = seg + SEGMENT_CMD_LEN;
    b[sect..sect + sectname.len()].copy_from_slice(sectname.as_bytes());
    b[sect + 40..sect + 48].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    b[sect + 48..sect + 52].copy_from_slice(&u32::try_from(payload_offset).unwrap().to_le_bytes());

    b[payload_offset..].copy_from_slice(payload);
    b
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn find_macho_bun_section_locates_payload() {
        let binary = build_macho("__BUN", "__bun", None, b"PAYLOAD");
        let loc = find_macho_bun_section(&binary).unwrap().unwrap();
        assert_eq!(
            loc,
            SectionLocation {
                offset: 32 + 72 + 80,
                size: 7,
            },
        );
    }

    #[test]
    fn find_macho_bun_section_walks_past_other_commands() {
        // A 24-byte filler command (cmd 0x2, LC_SYMTAB-sized) precedes the
        // segment, exercising the cmdsize walk.
        let binary = build_macho("__BUN", "__bun", Some((0x2, 24)), b"PAYLOAD");
        let loc = find_macho_bun_section(&binary).unwrap().unwrap();
        assert_eq!(loc.offset, 32 + 24 + 72 + 80);
        assert_eq!(loc.size, 7);
    }

    #[test]
    fn find_macho_bun_section_with_wrong_magic_returns_none() {
        let mut binary = build_macho("__BUN", "__bun", None, b"PAYLOAD");
        binary[0] = 0xDE;
        assert_eq!(find_macho_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_macho_bun_section_with_other_segment_name_returns_none() {
        let binary = build_macho("__TEXT", "__bun", None, b"PAYLOAD");
        assert_eq!(find_macho_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_macho_bun_section_with_other_section_name_returns_none() {
        let binary = build_macho("__BUN", "__text", None, b"PAYLOAD");
        assert_eq!(find_macho_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_macho_bun_section_with_short_input_returns_none() {
        assert_eq!(
            find_macho_bun_section(&[0xCF, 0xFA, 0xED, 0xFE]).unwrap(),
            None
        );
    }

    // The dummy binary is a real `bun build --compile` output. Built on macOS
    // it is a Mach-O; skip quietly when it is absent (not built) or not a
    // Mach-O (built on another OS). The e2e suite covers real containers on
    // every platform.
    #[test]
    fn find_macho_bun_section_on_real_dummy_binary_ends_at_trailer() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/dummy/dummy");
        let Ok(binary) = std::fs::read(path) else {
            eprintln!("skipping: dummy binary not built");
            return;
        };
        let Some(loc) = find_macho_bun_section(&binary).unwrap() else {
            eprintln!("skipping: dummy binary is not a Mach-O");
            return;
        };
        assert!(loc.offset + loc.size <= binary.len());
        assert_eq!(
            &binary[loc.offset + loc.size - 16..loc.offset + loc.size],
            b"\n---- Bun! ----\n",
        );
    }
}
