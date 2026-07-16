# bun-decompile-ag

Extracts the original transpiled sources from an executable produced by
`bun build --compile`.

This is a maintained fork of [lafkpages/bun-decompile](https://github.com/lafkpages/bun-decompile),
which did the original reverse-engineering of Bun's standalone module graph. See
[Credits](#credits).

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
on the version string.

## Installation

```sh
npm install -g bun-decompile-ag
```

Or with Bun:

```sh
bun add -g bun-decompile-ag
```

Or run it without installing:

```sh
bunx bun-decompile-ag <binary>
npx bun-decompile-ag <binary>
```

The CLI runs on Node 18+ and on Bun. You do **not** need Bun installed to
extract a Bun binary.

## Usage

```sh
bun-decompile-ag <input-binary> [options]
```

Installs also provide a shorter `bun-decompile` alias.

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
bun-decompile-ag ./my-app -o ./extracted
```

Keep the entrypoint's original bundled filename instead of renaming it to
`index.js`:

```sh
bun-decompile-ag ./my-app --no-normalize
```

Inline sourcemaps, when present, are written alongside each file as `<file>.map`.

## Library usage

```ts
import { readFile } from "node:fs/promises";

import { extractBundledFiles, getExecutableVersion } from "bun-decompile-ag";

const binary = await readFile("./my-app");

const { version, revision } = getExecutableVersion(binary);
const files = extractBundledFiles(binary);

for (const file of files) {
  console.log(file.path, file.contents.byteLength);
}
```

Both entry points accept a `Buffer`, `Uint8Array`, `ArrayBuffer`, or `DataView`
(`BinaryInput`). Views are read using their own `byteOffset`/`byteLength`, so a
pooled or sliced `Buffer` works correctly.

The library itself is runtime-agnostic — it touches no Node or Bun APIs and
operates purely on `DataView`, so it runs unchanged in the browser.

### API

| Export                 | Description                                                        |
| ---------------------- | ------------------------------------------------------------------ |
| `extractBundledFiles`  | Returns `BundledFile[]` — `{ path, contents, sourcemap? }`         |
| `getExecutableVersion` | Returns `{ version, revision }` read from the embedded Bun runtime |
| `removeLeadingSlash`   | Path helper                                                        |

Structural problems raise a subclass of `InvalidExecutableError`
(`InvalidTrailerError`, `TotalByteCountMismatchError`, `VersionNotFoundError`)
rather than returning partial or corrupt data.

### A note on Bun 1.4.0

Bun 1.4.0 stopped embedding a literal version string after its ANSI
`bun build v` marker — it became a runtime-substituted placeholder — which
breaks naive version detection. This fork falls back to scanning the runtime's
`Bun v<version>` banner.

Version detection is informational only. `extractBundledFiles` is driven
entirely by the binary's struct layout, so a future Bun release that changes the
banner again will still extract; the CLI degrades to a warning rather than
failing.

## Development

```sh
bun install
```

| Script                   | Description                                   |
| ------------------------ | --------------------------------------------- |
| `bun run build`          | Compile to `dist/` (JS + type declarations)   |
| `bun test`               | Unit tests                                    |
| `bun run test:e2e`       | End-to-end tests against real Bun releases    |
| `bun run typecheck`      | Typecheck without emitting                    |
| `bun run lint`           | Prettier check                                |
| `bun run build-fixtures` | Regenerate unit-test fixtures                 |
| `bun run debug-binary`   | Inspect a binary's container, format, offsets |

### Unit tests

Build the dummy test binary first, then run the suite:

```sh
bun build --compile --sourcemap=inline src/lib/tests/dummy/index.ts --outfile src/lib/tests/dummy/dummy
DUMMY_VERSION=$(bun --version) bun test
```

### E2E tests

Compiles a dummy binary with Bun v1.1.0, v1.1.26, v1.2.4, and v1.3.9 — one per
metadata format revision — and extracts each with the CLI. Then uses v1.3.14 to
cross-compile `bun-linux-x64` and `bun-windows-x64` targets, exercising ELF and
PE section extraction.

```sh
bun run test:e2e
```

Downloaded Bun versions are cached under `~/.cache/bun-decompile/`.

## Credits

Originally created by [LuisAFK (@lafkpages)](https://github.com/lafkpages) as
[bun-decompile](https://github.com/lafkpages/bun-decompile). The original
project worked out how Bun's standalone module graph is laid out and how to walk
it — the foundation everything here is built on.

This fork adds:

- Linux (ELF) and Windows (PE) `.bun` section support
- V3 and V4 metadata formats (Bun 1.2.4+ and 1.3.0+)
- Bun 1.4.0 version-banner detection, and non-fatal version detection
- A refactor into a container-detection pipeline, plus a Node-compatible CLI

## License

Not currently licensed for redistribution. The upstream project publishes no
license, so its copyright is reserved by default and this derivative work cannot
grant terms over it. ("UNLICENSED" here is npm's marker for _no license
granted_ — it is not [The Unlicense](https://unlicense.org).)

If you want to use this, open an issue — resolving this upstream is in progress.
