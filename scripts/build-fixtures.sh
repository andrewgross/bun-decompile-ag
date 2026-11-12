#!/usr/bin/env bash
set -eo pipefail

# Build test fixtures for milestone Bun versions and compile targets.
#
# Each fixture is a small extracted payload (~few KB) that IS checked into git,
# so `cargo test` covers every metadata format (V1-V4) and both bunfs path
# roots on any OS, with no Bun downloads and no platform-specific setup.
#
# Targets are cross-compiled, so a single host produces Linux and Windows
# fixtures too — no Windows machine required. Bun < 1.1.26 predates `--target`
# and is built natively instead.
#
# Note the fixtures are container-stripped (`bun-decompile debug --payload`),
# so they exercise format resolution and module walking, not section finding.
# Container parsing is covered by the synthetic ELF/PE/Mach-O tests and by the
# e2e suite, which builds real binaries.
#
# Two sets:
#   dummy      v<version>-<target>.bin from fixtures/dummy/, one per metadata
#              format revision
#   sourcemap  sourcemap-v<version>-<target>.bin (and .map) from
#              fixtures/sourcemap/, for Bun 1.4's binary sourcemap mappings
#
# Usage: scripts/build-fixtures.sh [dummy|sourcemap|all]   (default: all)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXTURES_DIR="${FIXTURES_DIR:-$PROJECT_DIR/fixtures}"
DUMMY_DIR="$PROJECT_DIR/fixtures/dummy"
DUMMY_SRC="$DUMMY_DIR/index.ts"
SETS="${1:-all}"
case "$SETS" in
  dummy|sourcemap|all) ;;
  *) echo "Usage: $0 [dummy|sourcemap|all]" >&2; exit 1 ;;
esac

source "$SCRIPT_DIR/get-bun.sh"

# The Rust CLI strips each compiled binary down to its payload.
RUST_CLI="${RUST_CLI:-$PROJECT_DIR/target/release/bun-decompile}"
if [ ! -x "$RUST_CLI" ]; then
  echo "Building the Rust CLI (set RUST_CLI to skip)..."
  (cd "$PROJECT_DIR" && cargo build --release -p bun-decompile-ag-cli)
fi

# Milestone versions, one per metadata format revision:
#   1.1.0  → V1 (32-byte chunks)      1.2.4  → V3 (36-byte chunks)
#   1.1.26 → V2 (28-byte chunks)      1.3.9  → V4 (52-byte chunks)
#   1.4.0  → V4, banner-only version  1.4.1  → V4, paths in a separate string
#                                              table (contents only reachable
#                                              through the record's pointer)
VERSIONS=("1.1.0" "1.1.26" "1.2.4" "1.3.9" "1.4.0" "1.4.1")

# Cross-compile targets. windows-x64 matters beyond the container: its bundled
# paths use the `B:/~BUN/root` bunfs root rather than `/$bunfs/root`.
TARGETS=("bun-darwin-arm64" "bun-linux-x64" "bun-windows-x64")

mkdir -p "$FIXTURES_DIR"

build_fixture() {
  local version="$1" target="$2" label="$3"
  local outfile="/tmp/dummy-fixture-${version}-${label}"
  local bun_bin
  bun_bin=$(get_bun_path "$version")

  if [ -n "$target" ]; then
    "$bun_bin" build --compile --sourcemap=inline --target="$target" \
      "$DUMMY_SRC" --outfile "$outfile" >/dev/null 2>&1 || true
  else
    "$bun_bin" build --compile --sourcemap=inline \
      "$DUMMY_SRC" --outfile "$outfile" >/dev/null 2>&1 || true
  fi

  # Windows targets get an .exe suffix; older Bun writes next to the source
  # instead of honouring --outfile.
  [ -f "${outfile}.exe" ] && outfile="${outfile}.exe"
  local stray="$DUMMY_DIR/$(basename "$outfile")"
  [ ! -f "$outfile" ] && [ -f "$stray" ] && mv "$stray" "$outfile"

  if [ ! -s "$outfile" ]; then
    echo "  SKIP ${label}: compile failed (target unsupported on v${version}?)"
    return 0
  fi

  "$RUST_CLI" debug "$outfile" \
    --payload "$FIXTURES_DIR/v${version}-${label}.bin" >/dev/null
  echo "  ok   ${label} -> v${version}-${label}.bin"
  rm -f "$outfile"
}

if [ "$SETS" != "sourcemap" ]; then
  for version in "${VERSIONS[@]}"; do
    echo "=== Bun v${version} ==="

    # Bun 1.1.0 has no --compile --target support; build for the host instead.
    if [ "$version" = "1.1.0" ]; then
      build_fixture "$version" "" "native"
      echo ""
      continue
    fi

    for target in "${TARGETS[@]}"; do
      build_fixture "$version" "$target" "${target#bun-}"
    done
    echo ""
  done
fi

# Sourcemap fixtures: a multi-module program whose sourcemap exercises every
# part of Bun 1.4's binary mappings encoding (several windows, source switches,
# multi-line gaps, multi-byte varints). Built with --sourcemap=external, for
# which Bun 1.4.1 also writes the bundler's own map next to the binary: that
# .map is what decoding the embedded binary mappings must reproduce. Bun 1.4.0
# writes no .map but bundles this program identically, so its fixtures are
# checked against the 1.4.1 maps. Built from the repo root so the map's source
# paths are stable.
SOURCEMAP_VERSIONS=("1.4.0" "1.4.1")

build_sourcemap_fixture() {
  local version="$1" target="$2" label="${2#bun-}"
  local outfile="/tmp/sourcemap-fixture-${version}-${label}"
  local name="sourcemap-v${version}-${label}"
  local bun_bin
  bun_bin=$(get_bun_path "$version")
  rm -f "$outfile" "${outfile}.exe" "${outfile}.map"

  (cd "$PROJECT_DIR" && "$bun_bin" build --compile --sourcemap=external \
    --target="$target" fixtures/sourcemap/index.ts --outfile "$outfile" >/dev/null 2>&1) || true

  local binary="$outfile"
  [ -f "${outfile}.exe" ] && binary="${outfile}.exe"
  if [ ! -s "$binary" ]; then
    echo "  SKIP ${label}: compile failed"
    return 0
  fi

  "$RUST_CLI" debug "$binary" --payload "$FIXTURES_DIR/${name}.bin" >/dev/null
  if [ -f "${outfile}.map" ]; then
    cp "${outfile}.map" "$FIXTURES_DIR/${name}.map"
    echo "  ok   ${label} -> ${name}.bin, ${name}.map"
  else
    echo "  ok   ${label} -> ${name}.bin"
  fi
  rm -f "$binary" "${outfile}.map"
}

if [ "$SETS" != "dummy" ]; then
  for version in "${SOURCEMAP_VERSIONS[@]}"; do
    echo "=== Bun v${version} (sourcemap) ==="
    for target in "${TARGETS[@]}"; do
      build_sourcemap_fixture "$version" "$target"
    done
    echo ""
  done
fi

echo "Fixtures in $FIXTURES_DIR:"
ls -la "$FIXTURES_DIR"
