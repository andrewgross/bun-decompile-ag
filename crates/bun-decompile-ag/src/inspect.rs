//! Structural inspection of a compiled executable, for diagnostics.
//!
//! Port of `src/scripts/debug-binary.ts`, reshaped into a library API: each
//! pipeline stage reports what it found or the error it stopped on, so a
//! binary from a future Bun release that breaks one stage still shows
//! everything up to that point. The CLI's `debug` subcommand formats this.

use crate::container::{
    BUN_TRAILER, BunSection, ContainerKind, ExecutableFormat, detect_executable_format,
    find_bun_section_offset, locate_bun_section,
};
use crate::error::ExtractError;
use crate::graph::build_module_graph;
use crate::offsets::{ParsedOffsets, parse_offsets};
use crate::version::{BunVersion, get_executable_version};

/// Everything [`inspect_executable`] could determine about a binary.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Inspection {
    /// Host executable format identified from magic bytes.
    pub format: ExecutableFormat,
    /// File offset of the `__BUN`/`.bun` section, when one exists.
    pub section_offset: Option<usize>,
    /// The embedded Bun version, or why it could not be read.
    pub version: Result<BunVersion, ExtractError>,
    /// The located payload and everything parsed from it, or the error the
    /// pipeline stopped on.
    pub payload: Result<PayloadInspection, ExtractError>,
}

/// The located payload and its parse progress.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PayloadInspection {
    /// Which container held the payload.
    pub container: ContainerKind,
    /// Payload size in bytes (ends exactly at the Bun trailer).
    pub payload_size: usize,
    /// The parsed Offsets struct, or why parsing failed.
    pub offsets: Result<OffsetsInspection, ExtractError>,
}

/// The parsed Offsets struct and the module graph built from it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct OffsetsInspection {
    /// Size of the struct that matched: 24 (V2/V3) or 32 (V4).
    pub struct_size: usize,
    /// Total modules+metadata size (low u32 of `byte_count`).
    pub offset_byte_count: u32,
    /// Offset of the metadata region within the modules area.
    pub modules_ptr_offset: u32,
    /// Total length of the metadata entries.
    pub modules_ptr_length: u32,
    /// Index of the entry-point module.
    pub entry_point_id: u32,
    /// The struct's raw bytes, for hexdump-level debugging.
    pub struct_bytes: Vec<u8>,
    /// The resolved module graph, or why building it failed.
    pub graph: Result<GraphInspection, ExtractError>,
}

/// The resolved module format and graph geometry.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct GraphInspection {
    /// Metadata chunk size: 32 (V1), 28 (V2), 36 (V3), or 52 (V4).
    pub chunk_size: usize,
    /// Whether strings are NUL-terminated on disk (V2+).
    pub new_format: bool,
    /// Whether chunks carry absolute offsets (V3/V4).
    pub has_absolute_offsets: bool,
    /// Number of bundled modules.
    pub module_count: usize,
    /// Offset of the modules region within the payload.
    pub modules_start: usize,
}

fn inspect_offsets(
    section: &BunSection<'_>,
    trailer_offset: usize,
) -> Result<OffsetsInspection, ExtractError> {
    let offsets: ParsedOffsets = parse_offsets(section.data, trailer_offset)?;

    let struct_start = trailer_offset.saturating_sub(offsets.struct_size);
    let struct_bytes = section
        .data
        .get(struct_start..trailer_offset)
        .unwrap_or_default()
        .to_vec();

    let graph = build_module_graph(section, offsets).map(|graph| GraphInspection {
        chunk_size: graph.format.chunk_size,
        new_format: graph.format.new_format,
        has_absolute_offsets: graph.format.has_absolute_offsets,
        module_count: graph.module_count,
        modules_start: graph.modules_start,
    });

    Ok(OffsetsInspection {
        struct_size: offsets.struct_size,
        offset_byte_count: offsets.offset_byte_count,
        modules_ptr_offset: offsets.modules_ptr_offset,
        modules_ptr_length: offsets.modules_ptr_length,
        entry_point_id: offsets.entry_point_id,
        struct_bytes,
        graph,
    })
}

/// Inspect a binary stage by stage without failing outright: container
/// detection, section offset, version, payload location, Offsets struct,
/// module graph. Never panics and never errors as a whole — each stage
/// records its own failure.
#[must_use]
pub fn inspect_executable(data: &[u8]) -> Inspection {
    let format = detect_executable_format(data);
    let section_offset = find_bun_section_offset(data).ok().flatten();
    let version = get_executable_version(data);

    let payload = locate_bun_section(data).map(|section| {
        let trailer_offset = section.data.len().saturating_sub(BUN_TRAILER.len());
        PayloadInspection {
            container: section.container,
            payload_size: section.data.len(),
            offsets: inspect_offsets(&section, trailer_offset),
        }
    });

    Inspection {
        format,
        section_offset,
        version,
        payload,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn inspect_executable_on_a_fixture_reports_the_full_pipeline() {
        let data = fixture("v1.3.9-darwin-arm64");
        let inspection = inspect_executable(&data);

        // Stripped fixtures have no host container and no runtime.
        assert_eq!(inspection.format, ExecutableFormat::Unknown);
        assert_eq!(inspection.section_offset, None);
        assert!(inspection.version.is_err());

        let payload = inspection.payload.unwrap();
        assert_eq!(payload.container, ContainerKind::Append);
        assert_eq!(payload.payload_size, data.len() - 8);

        let offsets = payload.offsets.unwrap();
        assert_eq!(offsets.struct_size, 32);
        assert_eq!(offsets.struct_bytes.len(), 32);

        let graph = offsets.graph.unwrap();
        assert_eq!(graph.chunk_size, 52);
        assert_eq!(graph.module_count, 4);
    }

    #[test]
    fn inspect_executable_on_garbage_reports_the_failure_stage() {
        let inspection = inspect_executable(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(inspection.format, ExecutableFormat::Unknown);
        assert!(inspection.version.is_err());
        assert!(matches!(
            inspection.payload,
            Err(ExtractError::InvalidTrailer(_)),
        ));
    }
}
