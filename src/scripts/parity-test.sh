#!/usr/bin/env bash
set -eo pipefail

# Parity harness: the migration acceptance gate. Runs the TypeScript CLI and
# the Rust CLI over the same inputs and asserts their output is identical —
# extracted files and .map files byte-for-byte, stdout after normalizing the
# version line, and exit codes (including the failure cases).
#
# Inputs, cheapest first:
#   1. Every committed fixture (fast, no Bun downloads).
#   2. A freshly compiled dummy binary (needs Bun on PATH).
#   3. The cross-target matrix (slow; only with --full, needs Bun downloads).
#   4. Error cases: a non-executable and a corrupt trailer.
#
# Usage:
#   src/scripts/parity-test.sh [--full]

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
FIXTURES_DIR="$PROJECT_DIR/src/lib/tests/fixtures"
DUMMY_SRC="$PROJECT_DIR/src/lib/tests/dummy/index.ts"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

TS_CLI="bun $PROJECT_DIR/src/cli.ts"
RUST_CLI="${RUST_CLI:-$PROJECT_DIR/target/release/bun-decompile}"

if [ ! -x "$RUST_CLI" ]; then
  echo "Building the Rust CLI (set RUST_CLI to skip)..."
  (cd "$PROJECT_DIR" && cargo build --release -p bun-decompile-ag-cli)
fi

FAILURES=0

# Normalize two expected, benign differences before diffing stdout:
#   1. The version line reports "Bun v<ver> (<rev>)"; collapse the revision so
#      wording never fails parity while a wrong version still would.
#   2. The final "Extracted N file(s) to <dir>" names the output directory,
#      which is deliberately different for the two CLIs (ts/ vs rust/); drop
#      the path so only the file count is compared. The extracted trees
#      themselves are still diffed byte-for-byte separately.
normalize_stdout() {
  sed -E -e 's/^Bun v([^ ]+).*/Bun v\1/' \
         -e 's/^(Extracted [0-9]+ file\(s\) to ).*/\1<dir>/'
}

# Extract with both CLIs and diff everything: exit codes, normalized stdout,
# and the full output tree (files and .map). $1 is a label, $2 the input.
compare_extract() {
  local label="$1" input="$2" extra="${3:-}"
  local ts_out="$WORK/ts" rust_out="$WORK/rust"
  rm -rf "$ts_out" "$rust_out"

  set +e
  $TS_CLI "$input" -o "$ts_out" $extra >"$WORK/ts.stdout" 2>"$WORK/ts.stderr"
  local ts_code=$?
  "$RUST_CLI" "$input" -o "$rust_out" $extra >"$WORK/rust.stdout" 2>"$WORK/rust.stderr"
  local rust_code=$?
  set -e

  local ok=1
  if [ "$ts_code" != "$rust_code" ]; then
    echo "  MISMATCH [$label]: exit codes differ (ts=$ts_code rust=$rust_code)"
    ok=0
  fi
  if ! diff <(normalize_stdout <"$WORK/ts.stdout") \
            <(normalize_stdout <"$WORK/rust.stdout") >/dev/null; then
    echo "  MISMATCH [$label]: stdout differs"
    diff <(normalize_stdout <"$WORK/ts.stdout") <(normalize_stdout <"$WORK/rust.stdout") || true
    ok=0
  fi
  if [ -d "$ts_out" ] || [ -d "$rust_out" ]; then
    if ! diff -r "$ts_out" "$rust_out" >/dev/null 2>&1; then
      echo "  MISMATCH [$label]: output trees differ"
      diff -r "$ts_out" "$rust_out" || true
      ok=0
    fi
  fi

  if [ "$ok" = 1 ]; then
    echo "  ok   $label"
  else
    FAILURES=$((FAILURES + 1))
  fi
}

# Compare exit code and normalized stderr for an expected-failure input.
compare_failure() {
  local label="$1" input="$2"
  set +e
  $TS_CLI "$input" -o "$WORK/ts" >/dev/null 2>"$WORK/ts.stderr"
  local ts_code=$?
  "$RUST_CLI" "$input" -o "$WORK/rust" >/dev/null 2>"$WORK/rust.stderr"
  local rust_code=$?
  set -e

  if [ "$ts_code" = "$rust_code" ] && diff "$WORK/ts.stderr" "$WORK/rust.stderr" >/dev/null; then
    echo "  ok   $label (both exit $ts_code)"
  else
    echo "  MISMATCH [$label]: ts(exit $ts_code) rust(exit $rust_code)"
    diff "$WORK/ts.stderr" "$WORK/rust.stderr" || true
    FAILURES=$((FAILURES + 1))
  fi
}

echo "=== Fixtures ==="
for fixture in "$FIXTURES_DIR"/v*.bin; do
  compare_extract "$(basename "$fixture")" "$fixture"
done

echo "=== Fixture (no --normalize) ==="
compare_extract "v1.3.9-darwin-arm64 --no-normalize" \
  "$FIXTURES_DIR/v1.3.9-darwin-arm64.bin" "--no-normalize"

echo "=== Fresh dummy binary ==="
if command -v bun >/dev/null 2>&1; then
  DUMMY="$WORK/dummy"
  bun build --compile --sourcemap=inline "$DUMMY_SRC" --outfile "$DUMMY" >/dev/null 2>&1
  compare_extract "fresh-dummy" "$DUMMY"
else
  echo "  skip: bun not on PATH"
fi

echo "=== Error cases ==="
head -c 8 /dev/zero >"$WORK/not-exec.bin"
compare_failure "non-executable" "$WORK/not-exec.bin"
head -c 64 "$FIXTURES_DIR/v1.1.0-native.bin" >"$WORK/truncated.bin"
compare_failure "corrupt-trailer" "$WORK/truncated.bin"
# Point the entrypoint's contents (V4 record +8) far past the modules region.
# Fixture layout: [modules][metadata][Offsets: 32B][trailer: 16B][u64 total],
# with the modules region at offset 0 and the metadata offset at Offsets+8.
cp "$FIXTURES_DIR/v1.4.1-darwin-arm64.bin" "$WORK/bad-pointer.bin"
offsets_at=$(($(wc -c <"$WORK/bad-pointer.bin") - 8 - 16 - 32))
metadata_at=$(od -An -t u4 -j $((offsets_at + 8)) -N 4 "$WORK/bad-pointer.bin" | tr -d ' ')
printf '\377\377\377\177' |
  dd of="$WORK/bad-pointer.bin" bs=1 seek=$((metadata_at + 8)) conv=notrunc 2>/dev/null
compare_failure "contents-pointer-overrun" "$WORK/bad-pointer.bin"

if [ "${1:-}" = "--full" ]; then
  echo "=== Cross-target matrix ==="
  source "$SCRIPT_DIR/get-bun.sh"
  for version in "1.1.26" "1.2.4" "1.3.9" "1.3.14" "1.4.0" "1.4.1"; do
    XBUN=$(get_bun_path "$version")
    for tgt in "bun-darwin-arm64" "bun-linux-x64" "bun-windows-x64"; do
      label="${tgt#bun-}"
      outfile="$WORK/xplat-${version}-${label}"
      "$XBUN" build --compile --target="$tgt" "$DUMMY_SRC" --outfile "$outfile" >/dev/null 2>&1 || true
      [ -f "${outfile}.exe" ] && outfile="${outfile}.exe"
      [ -s "$outfile" ] && compare_extract "v${version} ${label}" "$outfile"
    done
  done
fi

echo ""
if [ "$FAILURES" -eq 0 ]; then
  echo "Parity holds: TS and Rust CLIs agree on every input."
else
  echo "Parity FAILED: $FAILURES mismatch(es)."
  exit 1
fi
