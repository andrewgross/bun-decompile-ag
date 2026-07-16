import type { BundledFile } from "./modules.js";

import {
  BUN_TRAILER,
  BUN_VERSION_MATCH,
  BUN_VERSION_MATCH_BANNER,
  BUN_VERSION_MATCH_OLD,
} from "./constants.js";
import { VersionNotFoundError } from "./errors.js";
import { findBunSectionOffset, locateBunSection } from "./formats.js";
import { buildModuleGraph, extractModules } from "./modules.js";
import { parseOffsets } from "./offsets.js";

// Public API re-exports.
export {
  InvalidExecutableError,
  InvalidTrailerError,
  TotalByteCountMismatchError,
  VersionNotFoundError,
} from "./errors.js";
export { removeLeadingSlash, type BundledFile } from "./modules.js";
export type { ContainerKind } from "./formats.js";

export interface ExtractBundledFilesOptions {
  normaliseEntrypointFileName?: boolean;
}

/** Any binary input the public API accepts. */
export type BinaryInput = DataView | ArrayBuffer | Uint8Array<ArrayBufferLike>;

/** A byte view over any backing buffer — notably including a Node Buffer. */
type Bytes = Uint8Array<ArrayBufferLike>;

/**
 * Normalise binary input to a DataView windowed over exactly the caller's bytes.
 *
 * Views are re-wrapped using their own byteOffset/byteLength rather than the
 * whole backing buffer: Node pools Buffer allocations, so `buf.buffer` is
 * routinely larger than the file and reading it whole would parse adjacent junk.
 */
function toDataView(input: BinaryInput): DataView {
  if (input instanceof DataView) return input;
  if (input instanceof ArrayBuffer) return new DataView(input);
  return new DataView(input.buffer, input.byteOffset, input.byteLength);
}

/**
 * Extract the bundled source files from a `bun build --compile` executable.
 *
 * Pipeline:
 *   1. Detect the container (Mach-O / ELF / PE section, or legacy appended) and
 *      locate the payload — see `locateBunSection`.
 *   2. Parse the Offsets footer just before the trailer.
 *   3. Resolve the module format and check all invariants up front.
 *   4. Walk the module graph and return every file.
 *
 * Any structural problem raises (a subclass of) `InvalidExecutableError` rather
 * than returning partial or corrupt data.
 *
 * Reverse-engineered from bun `src/standalone_graph/StandaloneModuleGraph.zig`.
 */
export function extractBundledFiles(
  compiledBinaryData: BinaryInput,
  options: ExtractBundledFilesOptions = {},
): BundledFile[] {
  const binary = toDataView(compiledBinaryData);

  const section = locateBunSection(binary);
  const offsets = parseOffsets(section.view, section.view.byteLength - BUN_TRAILER.length);
  const graph = buildModuleGraph(section, offsets);

  return extractModules(graph, {
    normaliseEntrypointFileName: options.normaliseEntrypointFileName ?? true,
  });
}

export interface BunVersion {
  version: string;
  revision: string;
  newFormat?: boolean;
}

function getExecutableVersionNew(data: Bytes, searchLimit: number): BunVersion {
  const versionIndex = data.findIndex((_, index) => {
    if (index >= searchLimit) {
      return false;
    }

    for (let i = 0; i < BUN_VERSION_MATCH.length; i++) {
      if (data[index + i] !== BUN_VERSION_MATCH.charCodeAt(i)) {
        return false;
      }
    }

    return true;
  });

  if (versionIndex === -1) {
    throw new VersionNotFoundError();
  }

  const versionEndIndex = data.indexOf(27, versionIndex + BUN_VERSION_MATCH.length);

  if (versionEndIndex <= 0) {
    throw new VersionNotFoundError();
  }

  const decoder = new TextDecoder();
  const versionString = decoder.decode(
    data.slice(versionIndex + BUN_VERSION_MATCH.length, versionEndIndex),
  );

  const [, version, revision] = versionString.match(/^(.+) \((.+)\)$/) ?? [];

  if (!version) {
    throw new VersionNotFoundError();
  }

  return { version, revision };
}

function getExecutableVersionOld(data: Bytes, searchLimit: number): BunVersion {
  const versionIndex = data.findIndex((_, index) => {
    if (index >= searchLimit) {
      return false;
    }

    for (let i = 0; i < BUN_VERSION_MATCH_OLD.length; i++) {
      if (data[index + i] !== BUN_VERSION_MATCH_OLD.charCodeAt(i)) {
        return false;
      }
    }

    return true;
  });

  if (versionIndex === -1) {
    throw new VersionNotFoundError();
  }

  const versionEndIndex = data.indexOf(58, versionIndex + BUN_VERSION_MATCH_OLD.length);

  if (versionEndIndex <= 0) {
    throw new VersionNotFoundError();
  }

  const decoder = new TextDecoder();
  const versionString = decoder.decode(
    data.slice(versionIndex + BUN_VERSION_MATCH_OLD.length, versionEndIndex),
  );

  const [, version, revision] = versionString.match(/^(.+) \((.+)\)/) ?? [];

  if (!version) {
    throw new VersionNotFoundError();
  }

  return { version, revision };
}

/**
 * Newest version-string format (Bun 1.4.0+).
 *
 * Bun 1.4.0 stopped embedding a literal version after the ANSI
 * BUN_VERSION_MATCH marker — that became a runtime-substituted placeholder
 * (e.g. `...\x1b[2mv\xc0\x04...`) — but the plain "Bun v<version> (<revision>)"
 * banner is still present in the runtime's read-only data. Scan for it and pull
 * out the semantic version (and revision, if present).
 */
function getExecutableVersionBanner(data: Bytes, searchLimit: number): BunVersion {
  const decoder = new TextDecoder();
  const needle = BUN_VERSION_MATCH_BANNER;

  // A "Bun v" occurrence may be help text rather than the version banner, so
  // keep scanning until one actually parses as a version.
  let from = 0;
  for (;;) {
    const index = indexOfBytes(data, needle, from, searchLimit);
    if (index === -1) break;

    const start = index + needle.length;
    const window = decoder.decode(data.subarray(start, Math.min(start + 96, data.length)));

    // Accepts "1.4.0", "1.4.0+63bb0ca0d", "1.1.22-canary.96", each optionally
    // followed by " (revision)". Revision is the "+<sha>" build metadata when
    // present, else the parenthetical (a git sha or a platform string).
    const match = window.match(
      /^(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)(?:\+([0-9A-Za-z]+))?(?:\s*\(([^)]+)\))?/,
    );
    if (match) {
      return { version: match[1], revision: match[2] ?? match[3] ?? "" };
    }

    from = start;
  }

  throw new VersionNotFoundError();
}

/**
 * Find the first occurrence of an ASCII needle in `data` within [from, limit).
 * Returns the byte offset, or -1 if not found.
 */
function indexOfBytes(data: Bytes, needle: string, from: number, limit: number): number {
  const max = Math.min(limit, data.length - needle.length + 1);
  for (let i = from; i < max; i++) {
    let matched = true;
    for (let j = 0; j < needle.length; j++) {
      if (data[i + j] !== needle.charCodeAt(j)) {
        matched = false;
        break;
      }
    }
    if (matched) return i;
  }
  return -1;
}

export function getExecutableVersion(data: BinaryInput): BunVersion {
  const binary = toDataView(data);
  const bytes = new Uint8Array(binary.buffer, binary.byteOffset, binary.byteLength);

  // Determine search limit to exclude embedded module data (prevents matching
  // fake version strings inside bundled files). Works for any container that
  // uses a __BUN/.bun section; otherwise fall back to the legacy heuristic.
  const sectionOffset = findBunSectionOffset(binary);
  const searchLimit = sectionOffset ?? getModulesStartLegacy(binary);

  // Try each known version-string format in order, newest binaries last. Bun
  // 1.4.0+ fails the first two matchers (the literal after BUN_VERSION_MATCH is
  // gone) and falls back to the "Bun v" banner.
  const matchers: Array<() => BunVersion> = [
    () => ({ ...getExecutableVersionNew(bytes, searchLimit), newFormat: true }),
    () => ({ ...getExecutableVersionOld(bytes, searchLimit), newFormat: false }),
    () => ({ ...getExecutableVersionBanner(bytes, searchLimit), newFormat: true }),
  ];

  for (const matcher of matchers) {
    try {
      return matcher();
    } catch (e) {
      if (e instanceof VersionNotFoundError) continue;
      throw e;
    }
  }

  throw new VersionNotFoundError();
}

/**
 * Legacy modulesStart calculation for appended-format binaries.
 * Used only by getExecutableVersion to determine version-search bounds.
 */
function getModulesStartLegacy(compiledBinaryData: DataView): number {
  if (compiledBinaryData.byteLength <= 48) {
    return compiledBinaryData.byteLength;
  }

  const offsetByteCount = compiledBinaryData.getUint32(compiledBinaryData.byteLength - 48, true);

  return compiledBinaryData.byteLength - (offsetByteCount + 48);
}
