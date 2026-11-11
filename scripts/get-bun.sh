#!/usr/bin/env bash
# Helper to download and cache specific Bun versions.
# Source this file and call get_bun_path to get a path to a specific Bun binary.
#
# Usage:
#   source scripts/get-bun.sh
#   BUN=$(get_bun_path "1.3.9")
#   "$BUN" --version  # prints 1.3.9

BUN_CACHE_DIR="${BUN_CACHE_DIR:-$HOME/.cache/bun-decompile}"

get_bun_path() {
  local version="$1"
  if [ -z "$version" ]; then
    echo "Usage: get_bun_path <version>" >&2
    return 1
  fi

  local cache_dir="$BUN_CACHE_DIR/$version"
  local bun_bin="$cache_dir/bun"

  # Return cached binary if it exists
  if [ -x "$bun_bin" ]; then
    echo "$bun_bin"
    return 0
  fi

  # Detect platform
  local os arch
  case "$(uname -s)" in
    Darwin) os="darwin" ;;
    Linux)  os="linux" ;;
    *)
      echo "Unsupported OS: $(uname -s)" >&2
      return 1
      ;;
  esac

  case "$(uname -m)" in
    arm64|aarch64) arch="aarch64" ;;
    x86_64)        arch="x64" ;;
    *)
      echo "Unsupported architecture: $(uname -m)" >&2
      return 1
      ;;
  esac

  local url="https://github.com/oven-sh/bun/releases/download/bun-v${version}/bun-${os}-${arch}.zip"
  local tmpzip
  tmpzip="$(mktemp /tmp/bun-download-XXXXXX.zip)"

  echo "Downloading Bun v${version} from ${url}..." >&2

  if ! curl -fSL --progress-bar -o "$tmpzip" "$url"; then
    echo "Failed to download Bun v${version}" >&2
    rm -f "$tmpzip"
    return 1
  fi

  mkdir -p "$cache_dir"

  # Extract — zip contains bun-{os}-{arch}/bun
  if ! unzip -qo "$tmpzip" -d "$cache_dir"; then
    echo "Failed to extract Bun v${version}" >&2
    rm -f "$tmpzip"
    return 1
  fi

  # Move binary from nested directory to cache root
  local nested="$cache_dir/bun-${os}-${arch}/bun"
  if [ -f "$nested" ]; then
    mv "$nested" "$bun_bin"
    rm -rf "$cache_dir/bun-${os}-${arch}"
  fi

  rm -f "$tmpzip"

  if [ ! -x "$bun_bin" ]; then
    chmod +x "$bun_bin"
  fi

  echo "Cached Bun v${version} at ${bun_bin}" >&2
  echo "$bun_bin"
}
