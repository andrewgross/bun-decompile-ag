//! Extract the bundled source files from an executable built with
//! `bun build --compile`.
//!
//! Bun embeds a standalone module graph — every bundled file, plus offsets
//! and a trailer — inside the compiled executable. The on-disk layout is
//! defined by Bun's own MIT-licensed implementation in
//! `src/standalone_graph/StandaloneModuleGraph.zig`, which is the format
//! reference for this crate. Every layout Bun has shipped from v0.6.0
//! through v1.4.x is supported:
//!
//! - metadata formats V1 through V4 (32/28/36/52-byte module chunks with a
//!   24- or 32-byte `Offsets` struct), and
//! - every container: a Mach-O `__BUN/__bun` section (macOS), an ELF or PE
//!   `.bun` section (Linux ~1.3.10+, Windows ~1.3.9+), or the legacy
//!   payload appended to the end of the file (roughly Bun <= 1.2.x).
//!
//! The two entry points are [`extract_bundled_files`] and
//! [`get_executable_version`]:
//!
//! ```no_run
//! use bun_decompile_ag::{ExtractOptions, extract_bundled_files, get_executable_version};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let data = std::fs::read("./my-app")?;
//!
//! if let Ok(version) = get_executable_version(&data) {
//!     println!("Bun v{}", version.version);
//! }
//!
//! for file in extract_bundled_files(&data, ExtractOptions::default())? {
//!     println!("{} ({} bytes)", file.path, file.contents.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Malformed input never panics: every structural problem surfaces as an
//! [`ExtractError`]. Version detection is informational only — extraction is
//! driven entirely by the binary's struct layout, so it keeps working when a
//! future Bun changes its version banner again.
//!
//! With the `serde` feature enabled, [`SourceMap`] implements
//! `serde::Serialize` and serializes as a standard v3 sourcemap JSON object.

mod bytes;
mod container;
mod elf;
mod error;
mod extract;
mod graph;
mod inspect;
mod macho;
mod offsets;
mod paths;
mod pe;
mod section;
#[cfg(test)]
mod testutil;
mod version;

pub use container::{ContainerKind, ExecutableFormat};
pub use error::{ExtractError, Result};
pub use extract::{BundledFile, ExtractOptions, SourceMap};
pub use inspect::{
    GraphInspection, Inspection, OffsetsInspection, PayloadInspection, inspect_executable,
};
pub use paths::remove_leading_slash;
pub use version::BunVersion;

use container::{BUN_TRAILER, locate_bun_section};
use extract::extract_modules;
use graph::build_module_graph;
use offsets::parse_offsets;

/// Extract the bundled source files from a `bun build --compile` executable.
///
/// The pipeline: detect the container and locate the payload, parse the
/// `Offsets` struct just before the trailer, resolve the metadata format and
/// check every structural invariant, then walk the module graph. The entry
/// point is always first in the returned list.
///
/// # Errors
///
/// Any structural problem — no Bun payload, a corrupt trailer, region sizes
/// that do not line up — returns an [`ExtractError`] rather than partial or
/// corrupt data.
pub fn extract_bundled_files(data: &[u8], options: ExtractOptions) -> Result<Vec<BundledFile>> {
    let section = locate_bun_section(data)?;
    // locate_bun_section guarantees the payload ends with the 16-byte
    // trailer; saturate anyway so no input can panic.
    let trailer_offset = section.data.len().saturating_sub(BUN_TRAILER.len());
    let offsets = parse_offsets(section.data, trailer_offset)?;
    let graph = build_module_graph(&section, offsets)?;
    extract_modules(&graph, options)
}

/// Copy out the Bun payload as a self-contained blob in the legacy appended
/// layout, `[modules][metadata][Offsets][trailer][total length: u64 LE]`,
/// with the host executable and any section header stripped.
///
/// [`extract_bundled_files`] reads the blob back to the same files, which
/// makes it a compact test fixture: the committed fixtures are built this
/// way. Even a V3 payload taken out of a section round-trips, because Bun
/// 1.2.x appended that same layout for its Linux and Windows targets.
///
/// # Errors
///
/// The same structural errors as [`extract_bundled_files`]: the payload is
/// cut only once its module graph validates.
pub fn extract_payload(data: &[u8]) -> Result<Vec<u8>> {
    let section = locate_bun_section(data)?;
    let trailer_offset = section.data.len().saturating_sub(BUN_TRAILER.len());
    let offsets = parse_offsets(section.data, trailer_offset)?;
    let graph = build_module_graph(&section, offsets)?;

    let payload = section
        .data
        .get(graph.modules_start..)
        .ok_or(ExtractError::Truncated {
            offset: graph.modules_start,
            len: 0,
        })?;
    let total = payload.len() + 8;
    let mut blob = Vec::with_capacity(total);
    blob.extend_from_slice(payload);
    blob.extend_from_slice(&(total as u64).to_le_bytes());
    Ok(blob)
}

/// Read the Bun version this executable was compiled with.
///
/// Informational only: [`extract_bundled_files`] never depends on it, and
/// the CLI treats a failure here as a warning, not an error.
///
/// # Errors
///
/// [`ExtractError::VersionNotFound`] when no known version embedding parses;
/// other variants only when a container header is structurally broken.
pub fn get_executable_version(data: &[u8]) -> Result<BunVersion> {
    version::get_executable_version(data)
}
