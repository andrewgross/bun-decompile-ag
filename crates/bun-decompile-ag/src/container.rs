//! Detect the host executable's container format and locate the Bun payload.
//!
//! Port of `src/lib/formats.ts`, the pipeline's first stage. Strategies in
//! order: a named section (`__BUN/__bun` on Mach-O, `.bun` on ELF and PE) for
//! modern binaries, then the legacy appended layout
//! `[native code][payload][Offsets][trailer][total byte count: 8 bytes]`.
//! Every container is normalized to a byte view that ends exactly at the Bun
//! trailer, so downstream parsing never cares which container it came from.

use crate::bytes::read_u32_le;
use crate::elf::{ELF_MAGIC_LE, find_elf_bun_section};
use crate::error::{ExtractError, Result};
use crate::macho::{MACHO_MAGIC_64_LE, find_macho_bun_section};
use crate::pe::{DOS_MAGIC_LE, find_pe_bun_section};
use crate::section::SectionLocation;

/// The 16-byte marker that terminates every Bun payload.
pub(crate) const BUN_TRAILER: &[u8] = b"\n---- Bun! ----\n";

/// Which container the Bun payload was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    /// Mach-O `__BUN/__bun` section (macOS, all Bun versions).
    MachO,
    /// ELF `.bun` section (Linux, Bun ~1.3.10+).
    Elf,
    /// PE `.bun` section (Windows, Bun ~1.3.9+).
    Pe,
    /// Appended to the end of the file (legacy, roughly Bun <= 1.2.x).
    Append,
}

/// The host executable format identified from magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutableFormat {
    MachO,
    Elf,
    Pe,
    Unknown,
}

impl ExecutableFormat {
    /// Lowercase name as the TS union used it (error message parity).
    fn name(self) -> &'static str {
        match self {
            Self::MachO => "macho",
            Self::Elf => "elf",
            Self::Pe => "pe",
            Self::Unknown => "unknown",
        }
    }

    /// The TS `format.toUpperCase()` (error message parity).
    fn upper_name(self) -> &'static str {
        match self {
            Self::MachO => "MACHO",
            Self::Elf => "ELF",
            Self::Pe => "PE",
            Self::Unknown => "UNKNOWN",
        }
    }

    fn container(self) -> Option<ContainerKind> {
        match self {
            Self::MachO => Some(ContainerKind::MachO),
            Self::Elf => Some(ContainerKind::Elf),
            Self::Pe => Some(ContainerKind::Pe),
            Self::Unknown => None,
        }
    }
}

/// The located payload: which container it was in, and its bytes normalized
/// to end exactly at the Bun trailer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BunSection<'a> {
    pub container: ContainerKind,
    pub data: &'a [u8],
}

/// Identify the host executable format from its magic bytes.
pub(crate) fn detect_executable_format(binary: &[u8]) -> ExecutableFormat {
    if binary.len() < 4 {
        return ExecutableFormat::Unknown;
    }
    let Ok(magic) = read_u32_le(binary, 0) else {
        return ExecutableFormat::Unknown;
    };
    if magic == MACHO_MAGIC_64_LE {
        return ExecutableFormat::MachO;
    }
    if magic == ELF_MAGIC_LE {
        return ExecutableFormat::Elf;
    }
    if magic & 0xFFFF == u32::from(DOS_MAGIC_LE) {
        return ExecutableFormat::Pe;
    }
    ExecutableFormat::Unknown
}

fn find_section(format: ExecutableFormat, binary: &[u8]) -> Result<Option<SectionLocation>> {
    match format {
        ExecutableFormat::MachO => find_macho_bun_section(binary),
        ExecutableFormat::Elf => find_elf_bun_section(binary),
        ExecutableFormat::Pe => find_pe_bun_section(binary),
        ExecutableFormat::Unknown => Ok(None),
    }
}

/// The file offset of the embedded `__BUN`/`.bun` section, if any. Used by
/// version detection to bound its search.
pub(crate) fn find_bun_section_offset(binary: &[u8]) -> Result<Option<usize>> {
    let format = detect_executable_format(binary);
    Ok(find_section(format, binary)?.map(|location| location.offset))
}

/// Locate the embedded Bun payload, preferring a named section and falling
/// back to the legacy appended layout. Errors (rather than returning nothing)
/// so the specific failure is surfaced instead of parsing bad data.
pub(crate) fn locate_bun_section(binary: &[u8]) -> Result<BunSection<'_>> {
    let format = detect_executable_format(binary);

    // 1. Modern binaries embed the payload in a named section.
    if format != ExecutableFormat::Unknown {
        if let Some(location) = find_section(format, binary)? {
            let end = location.offset.checked_add(location.size);
            let data = end.and_then(|end| binary.get(location.offset..end)).ok_or(
                ExtractError::Truncated {
                    offset: location.offset,
                    len: location.size,
                },
            )?;
            assert_trailer_at_end(data, format)?;
            return Ok(BunSection {
                // format is not Unknown here, so a container kind exists.
                container: format.container().unwrap_or(ContainerKind::Append),
                data,
            });
        }
    }

    // 2. Legacy binaries (roughly Bun <= 1.2.x on any OS) append the payload.
    if let Some(section) = locate_appended(binary)? {
        return Ok(section);
    }

    // 3. Nothing matched.
    Err(ExtractError::InvalidTrailer(match format {
        ExecutableFormat::Unknown => {
            "not a Mach-O, ELF, or PE executable and no appended Bun payload".to_owned()
        }
        format => format!(
            "no __BUN/.bun section and no appended Bun payload in {} executable",
            format.upper_name(),
        ),
    }))
}

/// Legacy appended layout: the payload ends with the trailer followed by an
/// 8-byte total byte count that must equal the file size. Returns a view
/// trimmed to end at the trailer, like the section containers.
fn locate_appended(binary: &[u8]) -> Result<Option<BunSection<'_>>> {
    let Some(trailer_start) = binary.len().checked_sub(8 + BUN_TRAILER.len()) else {
        return Ok(None);
    };
    if !has_trailer_at(binary, trailer_start) {
        return Ok(None);
    }

    let total_byte_count = read_u32_le(binary, binary.len() - 8)?;
    if total_byte_count as usize != binary.len() {
        return Err(ExtractError::TotalByteCountMismatch);
    }

    Ok(Some(BunSection {
        container: ContainerKind::Append,
        data: &binary[..binary.len() - 8],
    }))
}

fn assert_trailer_at_end(data: &[u8], format: ExecutableFormat) -> Result<()> {
    let at_end = data
        .len()
        .checked_sub(BUN_TRAILER.len())
        .is_some_and(|start| has_trailer_at(data, start));
    if at_end {
        Ok(())
    } else {
        Err(ExtractError::InvalidTrailer(format!(
            "{} .bun section is missing the Bun trailer",
            format.name(),
        )))
    }
}

fn has_trailer_at(data: &[u8], offset: usize) -> bool {
    data.get(offset..offset + BUN_TRAILER.len()) == Some(BUN_TRAILER)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::elf::build_elf;
    use crate::macho::build_macho;
    use crate::pe::build_pe;
    use crate::testutil::fixture_payload;

    #[test]
    fn detect_executable_format_identifies_magic_bytes() {
        let detect = |bytes: &[u8]| {
            let mut b = vec![0_u8; 64];
            b[..bytes.len()].copy_from_slice(bytes);
            detect_executable_format(&b)
        };
        assert_eq!(detect(&[0xCF, 0xFA, 0xED, 0xFE]), ExecutableFormat::MachO);
        assert_eq!(detect(&[0x7F, b'E', b'L', b'F']), ExecutableFormat::Elf);
        assert_eq!(detect(b"MZ"), ExecutableFormat::Pe);
        assert_eq!(detect(&[0xDE, 0xAD, 0xBE, 0xEF]), ExecutableFormat::Unknown);
        assert_eq!(detect_executable_format(&[0x7F]), ExecutableFormat::Unknown);
    }

    #[test]
    fn locate_bun_section_on_appended_fixture_ends_at_trailer() {
        let payload = fixture_payload("v1.1.0-native");
        // Reconstruct the on-disk appended form: payload + total byte count.
        let mut binary = payload.clone();
        binary.extend_from_slice(&u64::try_from(payload.len() + 8).unwrap().to_le_bytes());

        let section = locate_bun_section(&binary).unwrap();
        assert_eq!(section.container, ContainerKind::Append);
        assert_eq!(section.data.len(), payload.len());
        assert!(section.data.ends_with(BUN_TRAILER));
    }

    #[test]
    fn locate_bun_section_with_wrong_total_byte_count_errors() {
        let payload = fixture_payload("v1.1.0-native");
        let mut binary = payload.clone();
        binary.extend_from_slice(&u64::try_from(payload.len() + 9).unwrap().to_le_bytes());

        assert_eq!(
            locate_bun_section(&binary),
            Err(ExtractError::TotalByteCountMismatch),
        );
    }

    #[test]
    fn locate_bun_section_through_synthetic_macho_finds_payload() {
        let payload = fixture_payload("v1.3.9-darwin-arm64");
        let binary = build_macho("__BUN", "__bun", None, &payload);
        let section = locate_bun_section(&binary).unwrap();
        assert_eq!(section.container, ContainerKind::MachO);
        assert_eq!(section.data, payload.as_slice());
    }

    #[test]
    fn locate_bun_section_through_synthetic_elf_finds_payload() {
        let payload = fixture_payload("v1.3.9-linux-x64");
        let binary = build_elf(&payload, true);
        let section = locate_bun_section(&binary).unwrap();
        assert_eq!(section.container, ContainerKind::Elf);
        assert_eq!(section.data, payload.as_slice());
    }

    #[test]
    fn locate_bun_section_through_synthetic_pe_finds_payload() {
        let payload = fixture_payload("v1.3.9-windows-x64");
        let binary = build_pe(&payload, 0);
        let section = locate_bun_section(&binary).unwrap();
        assert_eq!(section.container, ContainerKind::Pe);
        assert_eq!(section.data, payload.as_slice());
    }

    #[test]
    fn locate_bun_section_with_section_missing_trailer_errors() {
        let binary = build_pe(b"NOT A BUN PAYLOAD", 0);
        assert_eq!(
            locate_bun_section(&binary),
            Err(ExtractError::InvalidTrailer(
                "pe .bun section is missing the Bun trailer".to_owned(),
            )),
        );
    }

    #[test]
    fn locate_bun_section_on_non_executable_errors_with_unknown_message() {
        let binary = [0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD, 0xBE, 0xEF];
        assert_eq!(
            locate_bun_section(&binary),
            Err(ExtractError::InvalidTrailer(
                "not a Mach-O, ELF, or PE executable and no appended Bun payload".to_owned(),
            )),
        );
    }

    #[test]
    fn locate_bun_section_on_sectionless_macho_names_the_format() {
        let binary = build_macho("__TEXT", "__text", None, b"nothing");
        assert_eq!(
            locate_bun_section(&binary),
            Err(ExtractError::InvalidTrailer(
                "no __BUN/.bun section and no appended Bun payload in MACHO executable".to_owned(),
            )),
        );
    }

    #[test]
    fn find_bun_section_offset_returns_section_start() {
        let payload = fixture_payload("v1.3.9-darwin-arm64");
        let binary = build_macho("__BUN", "__bun", None, &payload);
        let offset = find_bun_section_offset(&binary).unwrap().unwrap();
        assert_eq!(&binary[offset..offset + 4], &payload[..4]);
        assert_eq!(find_bun_section_offset(&[0_u8; 64]).unwrap(), None,);
    }
}
