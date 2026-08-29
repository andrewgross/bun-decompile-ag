//! Locate the `.bun` section in an ELF 64-bit executable (Linux, FreeBSD).
//!
//! Port of `src/lib/elf.ts`. Bun embeds its payload here since roughly
//! 1.3.10 (older Linux builds appended it instead). Layout facts are the
//! ELF64 spec: `e_shoff: u64` at +40, `e_shentsize: u16` at +58,
//! `e_shnum: u16` at +60, `e_shstrndx: u16` at +62; each section header has
//! `sh_name: u32` (an offset into `.shstrtab`) at +0, `sh_offset: u64` at
//! +24, and `sh_size: u64` at +32. Header fields are read in the byte order
//! `EI_DATA` declares, matching the TS implementation.

#![allow(dead_code)] // Consumers land over the next commits of the migration.

use crate::bytes::{read_cstring, read_u16_endian, read_u32_endian, read_u64_endian};
use crate::error::Result;
use crate::section::SectionLocation;

/// `\x7fELF`, as the u32 value read little-endian from offset 0.
pub(crate) const ELF_MAGIC_LE: u32 = 0x464C_457F;

/// Find the `.bun` section.
///
/// `Ok(None)` when this is not an ELF64 or no such section exists (Bun only
/// emits ELF64); `Err` only when a read runs past the end of the file.
pub(crate) fn find_elf_bun_section(binary: &[u8]) -> Result<Option<SectionLocation>> {
    if binary.len() < 64 {
        return Ok(None);
    }
    if read_u32_endian(binary, 0, true)? != ELF_MAGIC_LE {
        return Ok(None);
    }

    let is_elf64 = binary.get(4) == Some(&2); // EI_CLASS: 2 = 64-bit
    let le = binary.get(5) == Some(&1); // EI_DATA: 1 = little-endian
    if !is_elf64 {
        return Ok(None);
    }

    let len = binary.len() as u64;
    let shoff = read_u64_endian(binary, 40, le)?;
    let shentsize = u64::from(read_u16_endian(binary, 58, le)?);
    let shnum = u64::from(read_u16_endian(binary, 60, le)?);
    let shstrndx = u64::from(read_u16_endian(binary, 62, le)?);

    if shoff == 0 || shnum == 0 || shentsize < 64 {
        return Ok(None);
    }
    // shnum and shentsize are both u16-derived, so the product cannot
    // overflow; guard the addition anyway.
    let Some(table_end) = shoff.checked_add(shnum * shentsize) else {
        return Ok(None);
    };
    if table_end > len {
        return Ok(None);
    }
    if shstrndx >= shnum {
        return Ok(None);
    }

    // Section-header string table (.shstrtab), which section names index into.
    let str_hdr = to_index(shoff + shstrndx * shentsize);
    let str_tab_offset = read_u64_endian(binary, str_hdr + 24, le)?;
    let str_tab_size = read_u64_endian(binary, str_hdr + 32, le)?;

    for i in 0..shnum {
        let sh = to_index(shoff + i * shentsize);
        let name_offset = u64::from(read_u32_endian(binary, sh, le)?);
        if name_offset >= str_tab_size {
            continue;
        }

        let name_at = str_tab_offset.checked_add(name_offset);
        let Some(name_at) = name_at.and_then(|v| usize::try_from(v).ok()) else {
            continue;
        };
        if read_cstring(binary, name_at, 64) == ".bun" {
            let sect_offset = read_u64_endian(binary, sh + 24, le)?;
            let sect_size = read_u64_endian(binary, sh + 32, le)?;
            // Overrun aborts the whole search, matching the TS behavior.
            match sect_offset.checked_add(sect_size) {
                Some(end) if end <= len => {}
                _ => return Ok(None),
            }
            return Ok(Some(SectionLocation {
                offset: to_index(sect_offset),
                size: to_index(sect_size),
            }));
        }
    }

    Ok(None)
}

/// Convert a u64 that has already been checked against the file length into
/// a usize index. The conversion therefore cannot fail (a slice length is a
/// usize); saturate defensively rather than panic.
fn to_index(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const SHDR: usize = 64;
    const NSEC: usize = 3; // null, .shstrtab, .bun

    fn put_u16(b: &mut [u8], at: usize, v: u16, little: bool) {
        b[at..at + 2].copy_from_slice(&if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        });
    }

    fn put_u32(b: &mut [u8], at: usize, v: u32, little: bool) {
        b[at..at + 4].copy_from_slice(&if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        });
    }

    fn put_u64(b: &mut [u8], at: usize, v: u64, little: bool) {
        b[at..at + 8].copy_from_slice(&if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        });
    }

    /// Minimal synthetic ELF64 wrapping `payload` in a `.bun` section — the
    /// Rust port of the TS test suite's `wrapInElf`, parameterized on byte
    /// order.
    fn build_elf(payload: &[u8], little: bool) -> Vec<u8> {
        let bun_offset = 64;
        let strtab_offset = bun_offset + payload.len();
        let strtab = b"\0.shstrtab\0.bun\0"; // ".shstrtab" @ 1, ".bun" @ 11
        let shoff = strtab_offset + strtab.len();
        let mut b = vec![0_u8; shoff + NSEC * SHDR];

        b[0..7].copy_from_slice(&[0x7F, b'E', b'L', b'F', 2, if little { 1 } else { 2 }, 1]);
        put_u64(&mut b, 40, shoff as u64, little); // e_shoff
        put_u16(&mut b, 58, u16::try_from(SHDR).unwrap(), little); // e_shentsize
        put_u16(&mut b, 60, u16::try_from(NSEC).unwrap(), little); // e_shnum
        put_u16(&mut b, 62, 1, little); // e_shstrndx -> .shstrtab

        b[bun_offset..bun_offset + payload.len()].copy_from_slice(payload);
        b[strtab_offset..strtab_offset + strtab.len()].copy_from_slice(strtab);

        let shstrtab = shoff + SHDR;
        put_u32(&mut b, shstrtab, 1, little); // sh_name -> ".shstrtab"
        put_u64(&mut b, shstrtab + 24, strtab_offset as u64, little);
        put_u64(&mut b, shstrtab + 32, strtab.len() as u64, little);

        let bun = shoff + 2 * SHDR;
        put_u32(&mut b, bun, 11, little); // sh_name -> ".bun"
        put_u64(&mut b, bun + 24, bun_offset as u64, little);
        put_u64(&mut b, bun + 32, payload.len() as u64, little);

        b
    }

    #[test]
    fn find_elf_bun_section_locates_payload() {
        let binary = build_elf(b"PAYLOAD", true);
        let loc = find_elf_bun_section(&binary).unwrap().unwrap();
        assert_eq!(
            loc,
            SectionLocation {
                offset: 64,
                size: 7
            }
        );
    }

    #[test]
    fn find_elf_bun_section_honors_big_endian_headers() {
        // EI_DATA = 2: every header field is big-endian, and the TS
        // implementation reads them that way; so must the port.
        let binary = build_elf(b"PAYLOAD", false);
        let loc = find_elf_bun_section(&binary).unwrap().unwrap();
        assert_eq!(
            loc,
            SectionLocation {
                offset: 64,
                size: 7
            }
        );
    }

    #[test]
    fn find_elf_bun_section_with_wrong_magic_returns_none() {
        let mut binary = build_elf(b"PAYLOAD", true);
        binary[0] = 0xDE;
        assert_eq!(find_elf_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_elf_bun_section_with_elf32_returns_none() {
        let mut binary = build_elf(b"PAYLOAD", true);
        binary[4] = 1; // EI_CLASS: 32-bit
        assert_eq!(find_elf_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_elf_bun_section_with_section_overrunning_file_returns_none() {
        let mut binary = build_elf(b"PAYLOAD", true);
        let shoff = 64 + 7 + 16;
        let bun = shoff + 2 * SHDR;
        put_u64(&mut binary, bun + 32, u64::MAX, true); // sh_size
        assert_eq!(find_elf_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_elf_bun_section_with_name_outside_strtab_returns_none() {
        let mut binary = build_elf(b"PAYLOAD", true);
        let shoff = 64 + 7 + 16;
        let bun = shoff + 2 * SHDR;
        put_u32(&mut binary, bun, 9999, true); // sh_name beyond .shstrtab
        assert_eq!(find_elf_bun_section(&binary).unwrap(), None);
    }

    #[test]
    fn find_elf_bun_section_with_short_input_returns_none() {
        assert_eq!(
            find_elf_bun_section(&[0x7F, b'E', b'L', b'F']).unwrap(),
            None
        );
    }
}
