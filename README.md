# bun-decompile

Extracts the original transpiled sources from an executable file generated via `bun build --compile`.

Supports all Bun binary formats from v0.6.0 through v1.3.x+.

## Installation

```sh
bun add -g bun-decompile
```

Or run directly with `bunx`:

```sh
bunx bun-decompile <binary>
```

## Usage

```sh
bun-decompile <input-binary> [options]
```

### Options

| Flag | Description |
|---|---|
| `-o, --output <dir>` | Output directory (default: `./decompiled`) |
| `--no-normalize` | Don't normalize entrypoint filename to `index.js` |
| `-v, --version` | Print version and exit |
| `-h, --help` | Print help and exit |

### Examples

Extract files from a compiled binary:

```sh
bun-decompile ./my-app -o ./extracted
```

Extract without normalizing the entrypoint filename:

```sh
bun-decompile ./my-app --no-normalize
```

## Library Usage

You can also import the core functions directly:

```ts
import { extractBundledFiles, getExecutableVersion } from "bun-decompile/src/lib";

const data = await Bun.file("./my-app").arrayBuffer();
const version = getExecutableVersion(data);
const files = extractBundledFiles(data);
```

## Development

```sh
bun install
```

### Unit tests

Build the dummy test binary, then run tests:

```sh
bun build --compile --sourcemap=inline src/lib/tests/dummy/index.ts --outfile src/lib/tests/dummy/dummy
DUMMY_VERSION=$(bun --version) bun test
```

### E2E tests

Downloads Bun v1.1.0, v1.1.26, v1.2.4, and v1.3.9, compiles dummy binaries with each, and extracts them with the CLI:

```sh
bun run test:e2e
```

Downloaded versions are cached in `~/.cache/bun-decompile/`.

### Build fixtures

Generates small extracted data sections for each format version, used in unit tests:

```sh
bun run build-fixtures
```
