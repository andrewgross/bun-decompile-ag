# bun-decompile

Extracts the original transpiled sources from an executable produced by
`bun build --compile`.

This is a maintained fork of [lafkpages/bun-decompile](https://github.com/lafkpages/bun-decompile),
which did the original reverse-engineering of Bun's standalone module graph. See
[Credits](#credits). It is written in Rust: a library crate, `bun-decompile-ag`,
and the `bun-decompile` command-line tool. The TypeScript implementation it
replaced is preserved at the [`ts-final`](https://github.com/andrewgross/bun-decompile-ag/tree/ts-final)
tag.

## What it supports

Every Bun binary layout from v0.6.0 through v1.4.x, across all three desktop
platforms. Bun embeds the compiled payload differently depending on version and
target; the extractor detects the container by sniffing magic bytes and
dispatches accordingly, so you don't need to tell it which one you have.

| Platform | Container                    | Since        |
| -------- | ---------------------------- | ------------ |
| macOS    | Mach-O `__BUN/__bun` section | all versions |
| Linux    | ELF `.bun` section           | ~1.3.10+     |
| Windows  | PE `.bun` section            | ~1.3.9+      |
| any      | appended-to-EOF (legacy)     | ≲ 1.2.x      |

Four on-disk metadata formats are handled (V1 through V4), spanning the
0.6.0-era layout through the 32-byte offset struct introduced in 1.3.0. Bun
1.4.0 is supported, including its
[reworked version banner](#a-note-on-bun-140) — extraction itself never depends
on the version string — and so is Bun 1.4.1's
[separate path table](#a-note-on-bun-141). Sourcemaps from 1.4.0+ binaries have
a [known limitation](#known-limitation-sourcemaps-from-bun-140).

## Installation

```sh
cargo install --locked --git https://github.com/andrewgross/bun-decompile-ag bun-decompile-ag-cli
```

This installs the `bun-decompile` command. It needs Rust 1.85 or newer to
build. You do **not** need Bun installed to extract a Bun binary.

The TypeScript release on npm, `@andrewgross/bun-decompile`, no longer receives
updates. Its last version, 0.1.1, predates the Bun 1.4.1 fix.

## Usage

```sh
bun-decompile <input-binary> [options]
bun-decompile debug <input-binary> [--payload <file>]
```

```console
$ bun-decompile ./my-app -o ./extracted
Bun v1.3.9 (cf6cdbbb)
Extracted 4 file(s) to ./extracted

$ ls ./extracted
index.js  index.js.map  logo-47vhjydn.png  data-twdsv7gz.bin
```

The first line reports the Bun version the binary was built with. It is
informational: if the version can't be read, the CLI warns and still extracts
(see [A note on Bun 1.4.0](#a-note-on-bun-140)). The entrypoint is written as
`index.js` unless `--no-normalize` is passed, and other bundled assets keep the
hashed names Bun gave them. Any error is fatal and exits non-zero rather than
writing partial output.

### Options

| Flag                 | Description                                       |
| -------------------- | ------------------------------------------------- |
| `-o, --output <dir>` | Output directory (default: `./decompiled`)        |
| `--no-normalize`     | Don't normalize entrypoint filename to `index.js` |
| `-v, --version`      | Print version and exit                            |
| `-h, --help`         | Print help and exit                               |

### Examples

Extract into a directory of your choosing:

```sh
bun-decompile ./my-app -o ./extracted
```

Keep the entrypoint's original bundled filename instead of renaming it to
`index.js`:

```sh
bun-decompile ./my-app --no-normalize
```

Inline sourcemaps, when present, are written alongside each file as `<file>.map`.

### Inspecting a binary

```sh
bun-decompile debug ./my-app
```

Prints what each stage of the extractor found: the executable format, the
section offset, the Bun version, the `Offsets` struct (with a hexdump), and the
resolved module graph. Each stage reports its own failure, so a binary from a
future Bun release that breaks one stage still shows everything up to it.
`--payload <file>` also writes the container-stripped payload, the format of the
test fixtures.

## Library usage

```toml
[dependencies]
bun-decompile-ag = { git = "https://github.com/andrewgross/bun-decompile-ag" }
```

```rust
use bun_decompile_ag::{ExtractOptions, extract_bundled_files, get_executable_version};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = std::fs::read("./my-app")?;

    if let Ok(version) = get_executable_version(&data) {
        println!("Bun v{} ({})", version.version, version.revision);
    }

    for file in extract_bundled_files(&data, ExtractOptions::default())? {
        println!("{} ({} bytes)", file.path, file.contents.len());
    }
    Ok(())
}
```

### API

| Item                     | Description                                                              |
| ------------------------ | ------------------------------------------------------------------------ |
| `extract_bundled_files`  | `Vec<BundledFile>` (`path`, `contents`, `sourcemap`), entry point first |
| `get_executable_version` | `BunVersion` (`version`, `revision`) read from the embedded Bun runtime  |
| `inspect_executable`     | Stage-by-stage structural report (what `bun-decompile debug` prints)     |
| `extract_payload`        | The container-stripped payload, as a self-contained appended blob        |
| `remove_leading_slash`   | Path helper                                                              |

Malformed input never panics: every structural problem is an `ExtractError`
rather than partial or corrupt data, and the entry points are fuzzed to hold
that. The `serde` feature makes `SourceMap` serializable as a standard v3
sourcemap JSON object. The library has no dependencies beyond `thiserror` (and
`serde` behind that feature).

### A note on Bun 1.4.0

Bun 1.4.0 stopped embedding a literal version string after its ANSI
`bun build v` marker — it became a runtime-substituted placeholder — which
breaks naive version detection. This fork falls back to scanning the runtime's
`Bun v<version>` banner.

Version detection is informational only. `extract_bundled_files` is driven
entirely by the binary's struct layout, so a future Bun release that changes the
banner again will still extract; the CLI degrades to a warning rather than
failing.

### A note on Bun 1.4.1

Bun 1.4.1 moved every module's path into one string table near the end of the
payload. Up to 1.4.0 each module happened to be laid out as path, NUL,
contents, NUL, sourcemap, so reading the contents "right after the path"
worked; on 1.4.1 that read returns a slice of the path table, with the right
length and no error. V3/V4 records carry an absolute `{offset, length}`
pointer for every field (path, contents, sourcemap, bytecode, module info,
bytecode origin path), so contents and sourcemaps are read through those
pointers, and a pointer outside the modules region fails loudly. V1/V2 records
have no pointers and are still walked sequentially. On 1.4.0 and earlier the
pointer read returns byte-identical output.

### Known limitation: sourcemaps from Bun 1.4.0+

Through 1.3.x, Bun stores a module's sourcemap mappings as VLQ text, which is
what a `.map` file's `mappings` field holds. Bun 1.4.0 switched to a binary
encoding that the extractor does not decode yet, so the `.map` files written for
1.4.0+ binaries carry those raw bytes in `mappings` and are not usable by
sourcemap tools. Their `sources` and `debugId` are correct, and the extracted
files themselves are unaffected.

## Development

`make check` runs everything CI does: `cargo fmt --check`, clippy with
warnings denied, the test suite, doctests, and `cargo deny`.

| Command          | Description                                                     |
| ---------------- | --------------------------------------------------------------- |
| `make test`      | Unit and integration tests (nextest when installed), doctests   |
| `make lint`      | Clippy, warnings denied                                         |
| `make fmt`       | Format the workspace                                            |
| `make e2e`       | End-to-end tests against real Bun releases                      |
| `make dummy`     | Compile the test dummy with the Bun on `PATH`                   |
| `make fixtures`  | Regenerate the committed fixtures from the milestone Bun releases |

### Tests

The fixtures in `fixtures/` are container-stripped payloads, one per metadata
format revision and target (plus the 1.4.0 and 1.4.1 layouts), compiled from
the dummy program in `fixtures/dummy/`. They are committed, so `cargo test`
covers every format on any OS with no Bun install; snapshot tests pin each
module's path, length, and content fingerprint. A few tests also run against a
real compiled dummy and skip quietly without one; build it with `make dummy`
(needs Bun on `PATH`).

### E2E tests

Compiles the dummy with Bun v1.1.0, v1.1.26, v1.2.4, v1.3.9, v1.4.0 and v1.4.1
(one per metadata format revision, plus the 1.4.x layout changes) and extracts
each with the CLI. Then cross-compiles `bun-darwin-arm64`, `bun-linux-x64`, and
`bun-windows-x64` targets with v1.1.26, v1.2.4, v1.3.9 and v1.3.14, exercising
every container a given version and target produce.

```sh
make e2e
```

Downloaded Bun versions are cached under `~/.cache/bun-decompile/`.

### Fuzzing

Both entry points have `cargo-fuzz` targets enforcing the no-panic contract; see
[`fuzz/README.md`](fuzz/README.md). CI runs a short smoke pass of each.

## Credits

Originally created by [LuisAFK (@lafkpages)](https://github.com/lafkpages) as
[bun-decompile](https://github.com/lafkpages/bun-decompile). The original
project worked out how Bun's standalone module graph is laid out and how to walk
it — the foundation everything here is built on.

This fork adds:

- Linux (ELF) and Windows (PE) `.bun` section support
- V3 and V4 metadata formats (Bun 1.2.4+ and 1.3.0+)
- Bun 1.4.0 version-banner detection, and non-fatal version detection
- Bun 1.4.1's separate path table (contents and sourcemaps read through the
  record pointers)
- A container-detection pipeline, rewritten in Rust as an embeddable library
  and CLI, fuzzed so that no input can make it panic

## License

[MIT](LICENSE).

The Rust implementation is new code written against the binary format as Bun's
own MIT-licensed source defines it (`src/standalone_graph/StandaloneModuleGraph.zig`),
with the earlier TypeScript implementation used as a behavioral test oracle.
That TypeScript implementation, a derivative of
[lafkpages/bun-decompile](https://github.com/lafkpages/bun-decompile), which
publishes no license, was never licensed: it remains **UNLICENSED** in this
repository's history up to the `ts-final` tag and in the npm releases of
`@andrewgross/bun-decompile`.
