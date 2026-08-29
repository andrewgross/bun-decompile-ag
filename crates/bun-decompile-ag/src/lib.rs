//! Extract the bundled source files from an executable built with
//! `bun build --compile`.
//!
//! Bun embeds a standalone module graph — every bundled file, plus offsets and
//! a trailer — inside the compiled executable. The on-disk layout is defined by
//! Bun's own MIT-licensed implementation in
//! `src/standalone_graph/StandaloneModuleGraph.zig`, which is the format
//! reference for this crate. Every layout Bun has shipped from v0.6.0 through
//! v1.4.x is supported: V1–V4 metadata formats, embedded in a Mach-O
//! `__BUN/__bun` section, an ELF or PE `.bun` section, or appended to the end
//! of the file (legacy).
//!
//! This crate is being ported component-by-component from the TypeScript
//! implementation in this repository; the public API lands with the final
//! components. See `docs/rust-migration.md` for the plan and commit stack.

mod bytes;
mod container;
mod elf;
mod error;
mod graph;
mod macho;
mod offsets;
mod paths;
mod pe;
mod section;
#[cfg(test)]
mod testutil;

pub use error::{ExtractError, Result};
