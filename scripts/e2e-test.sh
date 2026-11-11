#!/usr/bin/env bash
set -eo pipefail

# End-to-end test: compile real binaries with multiple Bun versions,
# then extract them with our CLI and verify the results.
#
# This is slow (downloads Bun versions, compiles ~60MB binaries) and is meant
# to be run separately from `cargo test` (`make e2e`).

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DUMMY_DIR="$PROJECT_DIR/fixtures/dummy"
DUMMY_SRC="$DUMMY_DIR/index.ts"

# The CLI under test: the release build by default (built on demand).
# Override to test another build, e.g.
#   DECOMPILE_CMD="./target/debug/bun-decompile" bash scripts/e2e-test.sh
if [ -z "${DECOMPILE_CMD:-}" ]; then
  DECOMPILE_CMD="$PROJECT_DIR/target/release/bun-decompile"
  if [ ! -x "$DECOMPILE_CMD" ]; then
    echo "Building the Rust CLI (set DECOMPILE_CMD to skip)..."
    (cd "$PROJECT_DIR" && cargo build --release -p bun-decompile-ag-cli)
  fi
fi

source "$SCRIPT_DIR/get-bun.sh"

# Milestone versions representing each format revision
VERSIONS=("1.1.0" "1.1.26" "1.2.4" "1.3.9" "1.4.0" "1.4.1")

for version in "${VERSIONS[@]}"; do
  echo "=== Testing Bun v${version} ==="

  BUN_BIN=$(get_bun_path "$version")
  outname="dummy-e2e-${version}"
  outfile="/tmp/${outname}"
  outdir="/tmp/decompiled-e2e-${version}"

  # 1. Compile the dummy binary using that Bun version
  #    Older Bun versions ignore the --outfile directory and write next to the
  #    source file, so we check both locations.
  "$BUN_BIN" build --compile --sourcemap=inline \
    "$DUMMY_SRC" --outfile "$outfile" || true

  # Handle older Bun writing output next to source instead of --outfile path
  if [ ! -f "$outfile" ] && [ -f "$DUMMY_DIR/$outname" ]; then
    mv "$DUMMY_DIR/$outname" "$outfile"
  fi

  if [ ! -f "$outfile" ]; then
    echo "  FAIL: compilation failed for Bun v${version}"
    exit 1
  fi

  # 2. Extract using the CLI under test
  $DECOMPILE_CMD "$outfile" -o "$outdir"

  # 3. Verify expected files exist
  if [ ! -f "$outdir/index.js" ]; then
    echo "  FAIL: index.js not found"
    exit 1
  fi

  file_count=$(ls "$outdir/" | wc -l | tr -d ' ')
  echo "  Extracted ${file_count} files"

  # 4. Cleanup
  rm -rf "$outfile" "$outdir"

  echo ""
done

# Cross-target matrix. The container a binary uses depends on BOTH the Bun
# version and the target, and the two vary independently:
#
#   1.2.4  darwin → macho section   linux/windows → appended (still V3 metadata)
#   1.3.9  darwin → macho section   linux → appended        windows → PE section
#   1.3.14 darwin → macho section   linux → ELF section     windows → PE section
#
# so a version-only or target-only sweep misses combinations. Bun 1.1.0 predates
# `--compile --target` and is covered by the native loop above.
echo "=== Cross-target matrix ==="
for version in "1.1.26" "1.2.4" "1.3.9" "1.3.14"; do
  XBUN=$(get_bun_path "$version")

  for tgt in "bun-darwin-arm64" "bun-linux-x64" "bun-windows-x64"; do
    label="${tgt#bun-}"
    outfile="/tmp/dummy-xplat-${version}-${label}"
    outdir="/tmp/decompiled-xplat-${version}-${label}"

    "$XBUN" build --compile --target="$tgt" "$DUMMY_SRC" --outfile "$outfile" >/dev/null 2>&1 || true
    # Windows targets get an .exe suffix.
    [ -f "${outfile}.exe" ] && outfile="${outfile}.exe"

    if [ ! -s "$outfile" ]; then
      echo "  FAIL: cross-compile $tgt failed for v${version}"
      exit 1
    fi

    $DECOMPILE_CMD "$outfile" -o "$outdir" >/dev/null
    if [ ! -f "$outdir/index.js" ]; then
      echo "  FAIL: index.js not found for v${version} $tgt"
      exit 1
    fi
    echo "  v${version} ${label}: extracted $(ls "$outdir" | wc -l | tr -d ' ') files"

    rm -rf "$outfile" "$outdir"
  done
done

echo ""
echo "All versions passed!"
