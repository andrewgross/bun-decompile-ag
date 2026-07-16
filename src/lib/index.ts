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

/** How long a window to read when nothing delimits the version text. */
const BANNER_WINDOW = 96;

/** ASCII byte constants for the delimiters Bun happens to use. */
const ESC = 0x1b;
const COLON = 0x3a;

/**
 * One way a Bun version is embedded in an executable: a literal that precedes
 * it, how far the version text runs, and how to read a version back out.
 */
interface VersionMarker {
  /** Literal bytes immediately preceding the version text. */
  marker: string;
  /** Byte the version text ends at, or null to read a fixed-size window. */
  terminator: number | null;
  /** Parse the text after the marker; null when it isn't actually a version. */
  parse(text: string): BunVersion | null;
  /** Vestigial, kept for API compatibility — nothing reads it. */
  newFormat: boolean;
}

/** Pull "<version> (<revision>)" out of `text`. */
function matchVersionRevision(text: string, pattern: RegExp): BunVersion | null {
  const match = text.match(pattern);
  return match ? { version: match[1], revision: match[2] } : null;
}

/**
 * Bun 1.4.0+ banner: "1.4.0", "1.4.0+63bb0ca0d" or "1.1.22-canary.96", each
 * optionally followed by " (revision)". The revision is the "+<sha>" build
 * metadata when present, else the parenthetical.
 */
function parseBanner(text: string): BunVersion | null {
  const match = text.match(
    /^(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)(?:\+([0-9A-Za-z]+))?(?:\s*\(([^)]+)\))?/,
  );
  return match ? { version: match[1], revision: match[2] ?? match[3] ?? "" } : null;
}

/**
 * Every known embedding, oldest format last. Tried in order; the first that
 * yields a parseable version wins.
 */
const VERSION_MARKERS: VersionMarker[] = [
  {
    // Bun 1.2–1.3.x: ANSI-styled "bun build v" then the literal version, which
    // runs up to the next escape byte.
    marker: BUN_VERSION_MATCH,
    terminator: ESC,
    parse: (text) => matchVersionRevision(text, /^(.+) \((.+)\)$/),
    newFormat: true,
  },
  {
    // Bun ≤ 1.1.25: "----- bun meta -----\nBun v<version> (<rev>):"
    marker: BUN_VERSION_MATCH_OLD,
    terminator: COLON,
    parse: (text) => matchVersionRevision(text, /^(.+) \((.+)\)/),
    newFormat: false,
  },
  {
    // Bun 1.4.0+ substitutes the literal above at runtime, so it is never in the
    // file; the plain "Bun v<version>" banner in the runtime's rodata still is.
    marker: BUN_VERSION_MATCH_BANNER,
    terminator: null,
    parse: parseBanner,
    newFormat: true,
  },
];

/**
 * Find the first version `marker` in [0, searchLimit) whose text parses.
 *
 * A marker can legitimately match something that isn't a version — "Bun v" also
 * appears in help text, and 1.4.0+ leaves an unsubstituted placeholder after the
 * ANSI marker — so keep scanning past a hit that fails to parse.
 */
function scanForVersion(
  data: Bytes,
  searchLimit: number,
  marker: VersionMarker,
): BunVersion | null {
  const decoder = new TextDecoder();
  let from = 0;

  for (;;) {
    const index = indexOfBytes(data, marker.marker, from, searchLimit);
    if (index === -1) {
      return null;
    }

    const start = index + marker.marker.length;
    const end =
      marker.terminator === null
        ? Math.min(start + BANNER_WINDOW, data.length)
        : data.indexOf(marker.terminator, start);

    if (end > start) {
      const version = marker.parse(decoder.decode(data.subarray(start, end)));
      if (version) {
        return { ...version, newFormat: marker.newFormat };
      }
    }

    from = start;
  }
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

  for (const marker of VERSION_MARKERS) {
    const version = scanForVersion(bytes, searchLimit, marker);
    if (version) {
      return version;
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
