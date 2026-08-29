# Rust migration plan

Port bun-decompile from TypeScript to Rust: a library crate embeddable by other
Rust projects, plus a thin CLI. Feature parity with the current extractor —
every Bun layout from v0.6.0 through 1.4.x (V1–V4 metadata, Mach-O/ELF/PE
sections, legacy appended) — verified against the TypeScript implementation
before it is retired.

## Goals

- Library-first: `bun-decompile-ag` as a normal Rust dependency with a small,
  documented API. No panics on untrusted input — every structural problem is a
  typed error.
- Standalone CLI with the same behavior as `dist/cli.js` (flags, output layout,
  exit codes, non-fatal version detection).
- Full multi-version support, proven by the existing fixtures and e2e matrix,
  plus a TS-vs-Rust parity harness during the migration.
- Reviewable as a stack: one component per commit, in dependency order, each
  green with its ported tests.

## Non-goals

- Web/browser support. The TS lib is runtime-agnostic so it runs in a browser;
  the Rust lib targets native only (no wasm target, no `no_std`).
- Input polymorphism. `BinaryInput` (DataView/ArrayBuffer/Uint8Array) collapses
  to `&[u8]`.
- Performance work. The parser touches each byte roughly once already; the goal
  is correctness and embeddability, not speed.
- Keeping the npm package alive from the Rust codebase (no napi-rs bridge).
  The TS implementation and npm packaging are deleted in the final commit once
  the parity gate passes; the last published npm version stays as-is.

## Licensing

The rewrite is MIT (decided). The TS on main stays UNLICENSED — it is a direct
derivative of upstream and nothing changes there before it is deleted.

Stance for the new code: the binary-format knowledge comes from Bun's own
MIT-licensed source (`src/standalone_graph/StandaloneModuleGraph.zig` is the
format reference, cited in the rustdoc), and the Rust is fresh expression
written against that format, with the TS implementation used as a behavioral
test oracle. Two practical rules follow: never carry upstream-authored comment
text or prose into the Rust verbatim, and keep crediting upstream's original
reverse-engineering in the README. Residual risk, acknowledged once: a close
port preserves structure, and structure can be argued as derivative; the
mitigations above are the chosen answer.

Mechanics: each crate ships its own `LICENSE` file and `license = "MIT"` in
its Cargo.toml from commit 1. No repo-root LICENSE until the TS-removal commit
— a root license would misleadingly appear to cover the UNLICENSED TS.

## Target layout

```
Cargo.toml                     # workspace root (edition 2024, workspace lints)
crates/
  bun-decompile-ag/            # library crate
    src/
      lib.rs                   # public API + pipeline (from lib/index.ts)
      error.rs                 # typed errors (from lib/errors.ts)
      bytes.rs                 # checked LE readers, read_cstring, find_bytes
      offsets.rs               # Offsets struct probe (from lib/offsets.ts)
      macho.rs                 # __BUN/__bun section finder (from lib/macho.ts)
      elf.rs                   # .bun section finder (from lib/elf.ts)
      pe.rs                    # .bun section finder (from lib/pe.ts)
      container.rs             # detect + dispatch + appended fallback (from lib/formats.ts)
      graph.rs                 # format resolution + invariants (from lib/modules.ts, first half)
      extract.rs               # module walk, sourcemaps, bunfs paths (from lib/modules.ts, second half)
      version.rs               # version markers + scan (from lib/index.ts)
    tests/
      fixtures.rs              # all committed .bin fixtures, insta snapshots
      containers.rs            # synthetic Mach-O/ELF/PE wrappers (port of wrapInElf/wrapInPe)
  bun-decompile-ag-cli/        # binary crate; [[bin]] name = "bun-decompile"
    src/
      main.rs                  # from cli.ts
      debug.rs                 # `bun-decompile debug <bin>` (from scripts/debug-binary.ts)
fuzz/                          # cargo-fuzz targets
  fuzz_targets/
    extract.rs                 # extract_bundled_files(arbitrary bytes)
    version.rs                 # get_executable_version(arbitrary bytes)
```

Two crates rather than one crate with a `cli` feature: library consumers never
see clap/anyhow/serde_json in their tree, and there is no feature-flag hygiene
to police. Precedent: `typst`/`typst-cli`.

Naming (decided): crate names carry the `-ag` suffix, matching the repo, so
the unscoped `bun-decompile` name on crates.io is left to the original author.
The installed binary stays `bun-decompile` — it replaces the npm CLI, which
installs that command today, and a PATH command is not a registry namespace.
Rust consumers import `bun_decompile_ag::…` (default lib name; not aliased, so
package and import names never diverge).

The TS code stays in place, untouched, for the whole migration — it is the
parity oracle. Removing it is the final commit, after the acceptance gate.

## Public API sketch

```rust
pub fn extract_bundled_files(data: &[u8], options: &ExtractOptions)
    -> Result<Vec<BundledFile>, ExtractError>;

pub fn get_executable_version(data: &[u8]) -> Result<BunVersion, ExtractError>;

#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    /// Rename the entrypoint to index.js (default true, like the TS lib).
    pub normalize_entrypoint_file_name: bool,  // Default impl sets true
}

pub struct BundledFile {
    pub path: String,
    pub contents: Vec<u8>,
    pub sourcemap: Option<SourceMap>,
}

/// Decoded from Bun's internal serialized sourcemap; serializes to a
/// standard v3 sourcemap JSON. Field order matches the TS JSON.stringify
/// output so `.map` files are byte-identical in the parity harness.
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SourceMap {
    pub version: u8,           // always 3
    pub file: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub debug_id: Option<String>,   // serialized as "debugId"
    pub mappings: String,
    pub sources: Vec<String>,
}

pub struct BunVersion { pub version: String, pub revision: String }
// The vestigial `newFormat` flag is not ported.

#[non_exhaustive]
pub enum ExtractError {
    InvalidExecutable(String),
    InvalidTrailer(String),
    TotalByteCountMismatch,
    OffsetsNotFound,
    VersionNotFound,
    // Out-of-bounds reads become a structured variant instead of the TS
    // behavior of an uncaught DataView RangeError.
    Truncated { offset: usize, len: usize },
}
```

Design notes:

- Owned `Vec<u8>` contents, not zero-copy `&'a [u8]`. The TS code copies via
  `.slice()` anyway, payloads are small relative to the host binary, and a
  lifetime-free API is far easier for downstream crates. Revisit only if a real
  consumer needs zero-copy.
- The TS error hierarchy (subclasses of `InvalidExecutableError`) flattens to
  one `#[non_exhaustive]` enum; `matches!` replaces `instanceof`.
- All multi-byte reads go through `bytes.rs` checked readers returning
  `Result`, so malformed input can never panic. This is the one deliberate
  behavior change from TS (where an OOB read throws RangeError with an
  unhelpful message); the fuzzer enforces it.
- No `regex` dependency in the library. The four patterns (banner version,
  `<version> (<rev>)`, Windows bunfs root, leading-slash strip) are small
  hand-rolled parsers with the TS regexes quoted in comments as the spec.

## Dependencies

Library: `thiserror`; `serde` (optional feature, for `SourceMap`). Nothing
else.

CLI: `clap` (derive), `anyhow`, `serde_json`, and the library with `serde` on.

Dev/test: `insta` (snapshots), `libfuzzer-sys` via `cargo-fuzz`.

Deliberately rejected: `object`/`goblin` for container parsing. Our three
finders are 46–63 lines each, already debugged against real binaries, and a
1:1 port keeps each diff reviewable against the TS original. A general Mach-O/
ELF/PE library replaces known-good code with different edge-case behavior and
a large dependency. Optionally, `object` can appear later as a dev-dependency
oracle (assert both agree on section offset/size over the fixtures) — not in
the critical path.

## Toolchain and quality gates

- **Toolchain**: latest stable, edition 2024, `rust-version` (MSRV) pinned at
  1.85 and enforced by a CI job on that exact toolchain.
- **Lints**: `[workspace.lints]` with `rust.unsafe_code = "forbid"` (a parser
  of untrusted input has no business with unsafe), `warn(missing_docs)` in the
  library, and clippy pedantic cherry-picks. Library code additionally warns on
  `clippy::unwrap_used`/`expect_used` — the no-panic contract, backed by fuzz.
- **Format**: `rustfmt` default config, `cargo fmt --check` in CI.
- **Clippy**: `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **Tests**: `cargo-nextest` as the runner, plus `cargo test --doc` (nextest
  does not run doctests; the API examples in rustdoc should compile and pass).
- **Supply chain**: `cargo-deny` (advisories, licenses, duplicate/yanked
  crates). With three runtime deps this is nearly free to keep green.
- **Fuzzing**: `cargo-fuzz` targets for both public entry points, corpus
  seeded from the committed fixtures. Invariant: any byte input returns
  `Ok`/`Err`, never panics. Short smoke run in CI (~60s per target), longer
  runs locally.
- **Coverage** (optional): `cargo-llvm-cov`, informational job.
- **CI** (`.github/workflows/rust.yml`, added in the scaffold commit; the
  existing TS `ci.yml` keeps running unchanged during migration):
  - fmt + clippy + nextest + doctests on ubuntu/macos/windows
  - MSRV job (ubuntu only)
  - cargo-deny
  - fixture integration tests run everywhere (fixtures are committed bytes —
    no Bun needed, which is why they exist)
  - e2e + parity as a separate slow job (needs Bun downloads; Linux only, like
    today's `e2e` job)
- **Makefile** mirroring current muscle memory:
  `make fmt` / `make lint` / `make test` / `make e2e` / `make parity` /
  `make fuzz` / `make check` (everything CI runs).

## Testing strategy

Four layers, strongest guarantee last:

1. **Ported unit tests** (per component, in the same commit as the component):
   offsets probe on synthetic 24/32-byte structs, the synthetic ELF/PE
   wrappers (`wrapInElf`/`wrapInPe` ported to Rust builders), a new synthetic
   Mach-O wrapper (the TS suite only covers Mach-O via the dummy binary — the
   Rust suite should not need a live Bun to cover its host platform), version
   marker tests including the 1.4.0 banner-only and skip-non-version cases,
   bunfs root stripping including both Windows slash styles.

2. **Fixture integration tests**: the ten committed
   `src/lib/tests/fixtures/*.bin` payloads (V1–V4 × darwin/linux/windows)
   extracted end-to-end; `insta` snapshots of per-fixture module lists (path,
   contents length, sourcemap presence, content hash) so a parser regression
   shows up as a readable snapshot diff. Fixtures stay where they are; Rust
   tests reach them via a path relative to `CARGO_MANIFEST_DIR`. They move to
   a top-level `fixtures/` only in the final TS-removal commit.
   Optional addition: have `build-fixtures.sh` also emit small windows of the
   runtime around each version marker (`version-v1.3.9.bin`, …) so version
   detection gets real-binary coverage under plain `cargo test`; today the
   stripped payload fixtures contain no runtime and hence no version string.

3. **TS↔Rust parity harness** (`src/scripts/parity-test.sh`, the migration
   acceptance gate): run `bun src/cli.ts` and the Rust CLI over the same
   inputs — every fixture, a freshly compiled dummy, and the e2e cross-target
   matrix — into two output trees and `diff -r` them. Extracted files must be
   byte-identical, `.map` files byte-identical (serde field order and compact
   encoding are chosen to match `JSON.stringify`; if that ever proves
   fragile, the harness falls back to JSON-normalized comparison for `.map`
   only), stdout compared after normalizing the version-line prefix, exit
   codes equal — including the failure cases (non-executable input, corrupt
   trailer).

4. **E2E against real Bun releases**: `e2e-test.sh` and `get-bun.sh` already
   download/cache milestone versions (1.1.0, 1.1.26, 1.2.4, 1.3.9, plus the
   1.3.14 cross-target matrix) and only shell out to a CLI. One small prep
   change parameterizes the CLI invocation (`DECOMPILE_CMD`, defaulting to the
   current `bun src/cli.ts`) so the identical script drives both
   implementations.

Behavioral deltas accepted up front (everything else is bug-for-bug, including
oddities like the `entryPointId > 10000` plausibility check and entrypoint
renaming happening before root-stripping):

- Library error _types_ are idiomatic Rust; CLI-visible error _text_ keeps the
  current wording (including the "Either the binary is corrupted…" suffix).
- Truncated/corrupt inputs that today surface as DataView RangeErrors become
  typed `ExtractError`s.
- `BunVersion.newFormat` (documented as vestigial) is dropped.

## Commit stack

One component per commit on branch `rust`, dependency order, each commit
compiles, passes clippy, and carries its tests. Later commits never rewrite
earlier ones except via explicit follow-ups.

| #   | Commit                    | Contents                                                                                                                                                                                                                   | Ported from              |
| --- | ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------ |
| 1   | workspace scaffolding     | Cargo workspace, both crates (lib empty, CLI prints version), per-crate MIT LICENSE + `license = "MIT"` metadata, workspace lints, Makefile, `rust.yml` CI, cargo-deny config                                              | —                        |
| 2   | errors + byte readers     | `error.rs`, `bytes.rs` (checked LE reads, `read_cstring`, `find_bytes`) + unit tests                                                                                                                                       | errors.ts, sections.ts   |
| 3   | Offsets struct probe      | `offsets.rs` (32-then-24 probe, validity checks) + synthetic-struct tests                                                                                                                                                  | offsets.ts               |
| 4   | Mach-O section finder     | `macho.rs` + new synthetic Mach-O builder tests                                                                                                                                                                            | macho.ts                 |
| 5   | ELF section finder        | `elf.rs` + ported `wrapInElf` tests                                                                                                                                                                                        | elf.ts                   |
| 6   | PE section finder         | `pe.rs` + ported `wrapInPe` tests                                                                                                                                                                                          | pe.ts                    |
| 7   | container detect/dispatch | `container.rs`: magic sniff, section dispatch, appended fallback, trailer assert, byte-count check + tests                                                                                                                 | formats.ts               |
| 8   | module format + graph     | `graph.rs`: `looksLikeV3` probe, V1/V2 heuristic, `build_module_graph` invariants + tests                                                                                                                                  | modules.ts (1st half)    |
| 9   | module extraction         | `extract.rs`: walk, sourcemap decode, bunfs roots, entrypoint rename; full fixture integration tests + insta snapshots                                                                                                     | modules.ts (2nd half)    |
| 10  | version detection         | `version.rs`: three markers, parse fallbacks, search limit, legacy limit heuristic + ported tests                                                                                                                          | index.ts (version half)  |
| 11  | public API + docs         | `lib.rs` pipeline, rustdoc with doctests, `serde` feature                                                                                                                                                                  | index.ts (pipeline half) |
| 12  | CLI                       | clap interface, output writing, `.map` serialization, non-fatal version warning, exit codes                                                                                                                                | cli.ts                   |
| 13  | debug subcommand          | `bun-decompile debug <bin>`: container/format/offsets dump                                                                                                                                                                 | scripts/debug-binary.ts  |
| 14  | parity + e2e wiring       | `DECOMPILE_CMD` in e2e-test.sh, parity-test.sh, slow CI job running both                                                                                                                                                   | e2e-test.sh              |
| 15  | fuzz targets              | cargo-fuzz setup, fixture-seeded corpus, CI smoke                                                                                                                                                                          | —                        |
| 16  | retire TS (gated)         | remove src/ TS + dist + npm packaging, promote MIT LICENSE to repo root, move fixtures to `fixtures/`, rewrite README (credits kept), port `extract-section` into the debug subcommand (`build-fixtures.sh` keeps working) | —                        |

Review mechanics: every commit is kept clean by rebasing fixes into the commit
they belong to (no "address review" commits), so the branch reads as a stack.
Two ways to review it, pick per taste:

- One PR `rust → main`, reviewed commit-by-commit (GitHub's per-commit view).
  Lowest ceremony; works fine solo.
- Chained PRs (each commit's branch PR'd against the previous one), if CI
  gating per component matters. Graphite or `spr` automate this, but at 16
  commits by one person, plain branches are manageable.

Commit 16 does not land until the acceptance gate passes: parity harness green
over fixtures + dummy + full cross-target matrix, e2e green with
`DECOMPILE_CMD` pointing at the Rust CLI, fuzzers run clean for a meaningful
local budget (say 30+ minutes each with no findings).

## Settled decisions

1. **TS is deleted in commit 16** once the parity gate passes. No frozen
   branch needed — git history keeps it, and the pre-removal tip gets a tag
   (`ts-final`). The npm package stops receiving updates; its last published
   version keeps working.
2. **MIT from day one** (see Licensing), which unblocks crates.io. Once the
   API settles after commit 16: publish `bun-decompile-ag` and
   `bun-decompile-ag-cli` at 0.1.0, add `cargo-semver-checks` to the release
   path, and ship prebuilt CLI binaries from a GitHub release workflow
   (cargo-dist optional). Until first publish, Rust consumers can already use
   a git dependency:
   `bun-decompile-ag = { git = "https://github.com/andrewgross/bun-decompile-ag" }`.
3. **Names**: crates `bun-decompile-ag` / `bun-decompile-ag-cli`; installed
   binary `bun-decompile`.

## Known friction

- Windows CI for the TS suite never existed; the Rust matrix adds it. Expect
  to shake out path-separator assumptions in the CLI output-writing code on
  the first run (the extractor itself is platform-independent).
