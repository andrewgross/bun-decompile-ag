//! Shared type for the location of the embedded Bun payload inside a host
//! executable. Port of `src/lib/sections.ts` (whose `readCString` half lives
//! in `bytes.rs`).

#![allow(dead_code)] // Consumers land over the next commits of the migration.

/// Absolute file offset and exact byte length of the `__BUN`/`.bun` section
/// inside a host executable. The section data ends exactly at the Bun
/// trailer; the container dispatcher asserts that when it takes a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SectionLocation {
    /// File offset of the section's first byte.
    pub offset: usize,
    /// Exact size in bytes of the section contents.
    pub size: usize,
}
