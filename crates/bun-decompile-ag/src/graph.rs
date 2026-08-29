//! Resolve the on-disk module metadata format and build the module graph,
//! checking every structural invariant up front.
//!
//! Port of the first half of `src/lib/modules.ts`. The container alone does
//! not determine the metadata format: Linux and Windows targets kept the
//! appended container until Bun ~1.3.10, so a 1.2.4-era binary for those
//! targets is appended yet carries V3 metadata. Resolution order:
//!
//! 1. A 32-byte Offsets struct is V4 (Bun 1.3.0+, 52-byte chunks).
//! 2. Any named section is at least Bun 1.2.4, so V3 (36-byte chunks).
//! 3. An appended payload is probed for V3: V3/V4 chunks begin with an
//!    absolute pointer to the module's path, so each chunk can be validated
//!    independently — a wrong chunk size misaligns chunk 1 onward and the
//!    pointed-at bytes stop decoding to bunfs paths. Divisibility alone
//!    cannot discriminate (e.g. 252 divides by both 28 and 36).
//! 4. Otherwise V1/V2 (32/28-byte chunks, walked sequentially), told apart
//!    by whether the payload size lines up with the metadata pointer.

use crate::bytes::read_u32_le;
use crate::container::{BUN_TRAILER, BunSection, ContainerKind};
use crate::error::{ExtractError, Result};
use crate::offsets::ParsedOffsets;
use crate::paths::remove_bunfs_root_from_path;

/// Shape of the per-module metadata chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModuleFormat {
    /// Size of each per-module metadata chunk (28/32/36/52 bytes).
    pub chunk_size: usize,
    /// The new wire format inserts a NUL after each string
    /// (path/contents/sourcemap).
    pub new_format: bool,
    /// V3/V4 chunks carry an absolute start offset at +0; V1/V2 iterate
    /// sequentially.
    pub has_absolute_offsets: bool,
}

const FORMAT_V3: ModuleFormat = ModuleFormat {
    chunk_size: 36,
    new_format: true,
    has_absolute_offsets: true,
};

const FORMAT_V4: ModuleFormat = ModuleFormat {
    chunk_size: 52,
    new_format: true,
    has_absolute_offsets: true,
};

/// The fully-resolved module graph: everything needed to walk the modules,
/// with all invariants already checked by [`build_module_graph`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ModuleGraph<'a> {
    /// The payload bytes, ending exactly at the Bun trailer.
    pub data: &'a [u8],
    pub offsets: ParsedOffsets,
    pub format: ModuleFormat,
    /// Offset of the modules region within `data`.
    pub modules_start: usize,
    /// Offset of the metadata region within `data` (right after the modules
    /// region).
    pub metadata_start: usize,
    pub module_count: usize,
}

/// Does the metadata region parse cleanly as V3's 36-byte absolute-offset
/// chunks? Every chunk's path pointer must land inside the modules region and
/// decode to a path with a recognized bunfs root.
fn looks_like_v3(
    offsets: ParsedOffsets,
    data: &[u8],
    modules_start: usize,
    metadata_start: usize,
) -> bool {
    let chunk_size = FORMAT_V3.chunk_size;
    let metadata_length = offsets.modules_ptr_length as usize;

    if metadata_length == 0 || metadata_length % chunk_size != 0 {
        return false;
    }

    // The modules region runs from modules_start up to the metadata region.
    let modules_length = offsets.modules_ptr_offset as usize;

    for i in 0..metadata_length / chunk_size {
        let chunk = metadata_start + i * chunk_size;
        if chunk + chunk_size > data.len() {
            return false;
        }

        let Ok(path_start) = read_u32_le(data, chunk) else {
            return false;
        };
        let Ok(path_length) = read_u32_le(data, chunk + 4) else {
            return false;
        };
        let (path_start, path_length) = (path_start as usize, path_length as usize);

        if path_length == 0 || path_start + path_length > modules_length {
            return false;
        }

        let start = modules_start + path_start;
        let Some(path_bytes) = data.get(start..start + path_length) else {
            return false;
        };
        let path = String::from_utf8_lossy(path_bytes);
        if remove_bunfs_root_from_path(&path).is_err() {
            return false;
        }
    }

    true
}

/// Determine the on-disk module format — see the module docs for the
/// resolution order and why the container alone is not enough.
pub(crate) fn resolve_module_format(
    container: ContainerKind,
    offsets: ParsedOffsets,
    data: &[u8],
    trailer_offset: usize,
    modules_start: usize,
    metadata_start: usize,
) -> Result<ModuleFormat> {
    if offsets.struct_size == 32 {
        return Ok(FORMAT_V4);
    }

    // A __BUN/.bun section is only ever emitted by Bun >= 1.2.4, i.e. V3.
    if container != ContainerKind::Append {
        return Ok(FORMAT_V3);
    }

    if looks_like_v3(offsets, data, modules_start, metadata_start) {
        return Ok(FORMAT_V3);
    }

    // V1/V2 appended: tell the 28-byte (new) and 32-byte (old) chunk layouts
    // apart by whether the payload size lines up with the metadata pointer.
    let truncated = ExtractError::Truncated { offset: 0, len: 4 };
    let struct_start = trailer_offset
        .checked_sub(offsets.struct_size)
        .ok_or(truncated.clone())?;
    let probe_at = struct_start.checked_sub(20).ok_or(truncated)?;
    let payload_size =
        u64::from(read_u32_le(data, probe_at)?) + u64::from(read_u32_le(data, probe_at + 4)?);
    let new_format = payload_size + 1 == u64::from(offsets.modules_ptr_offset);

    Ok(ModuleFormat {
        chunk_size: if new_format { 28 } else { 32 },
        new_format,
        has_absolute_offsets: false,
    })
}

/// Resolve the module format, compute the region offsets, and assert every
/// invariant — extraction can then run straight through and trust its inputs.
pub(crate) fn build_module_graph<'a>(
    section: &BunSection<'a>,
    offsets: ParsedOffsets,
) -> Result<ModuleGraph<'a>> {
    let data = section.data;
    let trailer_offset = data.len().saturating_sub(BUN_TRAILER.len());

    // Fail loudly rather than extracting garbage. This invariant comes first:
    // resolving the format reads the metadata region, so the regions must be
    // sane before anything else.
    let payload_span = offsets.offset_byte_count as usize + offsets.struct_size + BUN_TRAILER.len();
    let Some(modules_start) = data.len().checked_sub(payload_span) else {
        return Err(ExtractError::InvalidExecutable(format!(
            "Declared payload size ({}) exceeds the section ({})",
            offsets.offset_byte_count,
            data.len(),
        )));
    };

    let metadata_start = modules_start + offsets.modules_ptr_offset as usize;
    let format = resolve_module_format(
        section.container,
        offsets,
        data,
        trailer_offset,
        modules_start,
        metadata_start,
    )?;

    let metadata_length = offsets.modules_ptr_length as usize;
    if metadata_length % format.chunk_size != 0 {
        return Err(ExtractError::InvalidExecutable(format!(
            "Metadata length {metadata_length} is not a multiple of chunk size {}",
            format.chunk_size,
        )));
    }
    let module_count = metadata_length / format.chunk_size;
    if module_count == 0 {
        return Err(ExtractError::InvalidExecutable(
            "Executable contains no bundled modules".to_owned(),
        ));
    }
    if offsets.entry_point_id as usize >= module_count {
        return Err(ExtractError::InvalidExecutable(format!(
            "Entry point id {} is out of range for {module_count} modules",
            offsets.entry_point_id,
        )));
    }
    if metadata_start + metadata_length > trailer_offset {
        return Err(ExtractError::InvalidExecutable(
            "Module metadata extends past the Offsets struct".to_owned(),
        ));
    }

    Ok(ModuleGraph {
        data,
        offsets,
        format,
        modules_start,
        metadata_start,
        module_count,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::container::locate_bun_section;
    use crate::offsets::parse_offsets;
    use crate::testutil::fixture;

    /// Locate + parse offsets + build the graph for a committed fixture
    /// (fixture files are valid appended-format binaries as-is).
    fn graph_for(data: &[u8]) -> (ContainerKind, ModuleFormat, usize) {
        let section = locate_bun_section(data).unwrap();
        let offsets = parse_offsets(section.data, section.data.len() - BUN_TRAILER.len()).unwrap();
        let graph = build_module_graph(&section, offsets).unwrap();
        (section.container, graph.format, graph.module_count)
    }

    #[test]
    fn build_module_graph_on_v1_fixture_resolves_32_byte_old_format() {
        let data = fixture("v1.1.0-native");
        let (container, format, count) = graph_for(&data);
        assert_eq!(container, ContainerKind::Append);
        assert_eq!(
            format,
            ModuleFormat {
                chunk_size: 32,
                new_format: false,
                has_absolute_offsets: false,
            },
        );
        assert_eq!(count, 4);
    }

    #[test]
    fn build_module_graph_on_v2_fixture_resolves_28_byte_new_format() {
        let data = fixture("v1.1.26-darwin-arm64");
        let (_, format, count) = graph_for(&data);
        assert_eq!(
            format,
            ModuleFormat {
                chunk_size: 28,
                new_format: true,
                has_absolute_offsets: false,
            },
        );
        assert_eq!(count, 4);
    }

    #[test]
    fn build_module_graph_on_appended_v3_fixture_probes_v3() {
        // The documented tricky case: appended container carrying V3
        // metadata (1.2.4-era Linux/Windows targets, and stripped fixtures).
        for name in [
            "v1.2.4-darwin-arm64",
            "v1.2.4-linux-x64",
            "v1.2.4-windows-x64",
        ] {
            let data = fixture(name);
            let (container, format, count) = graph_for(&data);
            assert_eq!(container, ContainerKind::Append, "{name}");
            assert_eq!(format, FORMAT_V3, "{name}");
            assert_eq!(count, 4, "{name}");
        }
    }

    #[test]
    fn build_module_graph_on_v4_fixture_resolves_52_byte_chunks() {
        for name in [
            "v1.3.9-darwin-arm64",
            "v1.3.9-linux-x64",
            "v1.3.9-windows-x64",
        ] {
            let data = fixture(name);
            let (_, format, count) = graph_for(&data);
            assert_eq!(format, FORMAT_V4, "{name}");
            assert_eq!(count, 4, "{name}");
        }
    }

    #[test]
    fn resolve_module_format_with_section_container_is_v3() {
        // Any named section implies Bun >= 1.2.4 regardless of the bytes.
        let offsets = ParsedOffsets {
            offset_byte_count: 0,
            modules_ptr_offset: 0,
            modules_ptr_length: 36,
            entry_point_id: 0,
            struct_size: 24,
        };
        let format =
            resolve_module_format(ContainerKind::MachO, offsets, &[0; 64], 48, 0, 0).unwrap();
        assert_eq!(format, FORMAT_V3);
    }

    /// A synthetic section: `payload_len` zero bytes then the trailer, with
    /// directly-constructed offsets (bypassing the probe's checks).
    fn synthetic_section(payload_len: usize) -> Vec<u8> {
        let mut data = vec![0_u8; payload_len];
        data.extend_from_slice(BUN_TRAILER);
        data
    }

    fn offsets(byte_count: u32, ptr_offset: u32, ptr_length: u32, entry: u32) -> ParsedOffsets {
        ParsedOffsets {
            offset_byte_count: byte_count,
            modules_ptr_offset: ptr_offset,
            modules_ptr_length: ptr_length,
            entry_point_id: entry,
            struct_size: 24,
        }
    }

    fn build_err(data: &[u8], offsets: ParsedOffsets) -> ExtractError {
        let section = BunSection {
            container: ContainerKind::Append,
            data,
        };
        build_module_graph(&section, offsets).unwrap_err()
    }

    #[test]
    fn build_module_graph_with_oversized_payload_claim_errors() {
        let data = synthetic_section(64);
        let err = build_err(&data, offsets(100, 10, 32, 0));
        assert!(
            err.to_string()
                .starts_with("Declared payload size (100) exceeds the section (80)"),
            "{err}",
        );
    }

    #[test]
    fn build_module_graph_with_misaligned_metadata_length_errors() {
        let data = synthetic_section(64);
        let err = build_err(&data, offsets(40, 10, 13, 0));
        assert!(
            err.to_string()
                .starts_with("Metadata length 13 is not a multiple of chunk size 32"),
            "{err}",
        );
    }

    #[test]
    fn build_module_graph_with_zero_modules_errors() {
        let data = synthetic_section(64);
        let err = build_err(&data, offsets(40, 10, 0, 0));
        assert!(
            err.to_string()
                .starts_with("Executable contains no bundled modules"),
            "{err}",
        );
    }

    #[test]
    fn build_module_graph_with_entry_point_out_of_range_errors() {
        let data = synthetic_section(64);
        let err = build_err(&data, offsets(40, 10, 32, 5));
        assert!(
            err.to_string()
                .starts_with("Entry point id 5 is out of range for 1 modules"),
            "{err}",
        );
    }

    #[test]
    fn build_module_graph_with_metadata_past_trailer_errors() {
        let data = synthetic_section(64);
        let err = build_err(&data, offsets(40, 40, 32, 0));
        assert!(
            err.to_string()
                .starts_with("Module metadata extends past the Offsets struct"),
            "{err}",
        );
    }
}
