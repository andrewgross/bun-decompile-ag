import type { SectionLocation } from "./sections.js";

import { BUN_TRAILER, ELF_MAGIC_LE, MACHO_MAGIC_64_LE } from "./constants.js";
import { findElfBunSection } from "./elf.js";
import { InvalidTrailerError, TotalByteCountMismatchError } from "./errors.js";
import { findMachoBunSection } from "./macho.js";
import { findPeBunSection } from "./pe.js";

export type ContainerKind = "macho" | "elf" | "pe" | "append";
export type ExecutableFormat = "macho" | "elf" | "pe" | "unknown";

export interface BunSection {
  container: ContainerKind;
  /**
   * The payload region, normalized to end exactly at the Bun trailer:
   * `[header?][modules][metadata][Offsets][trailer]`. Downstream parsing is
   * container-agnostic because every container is presented this way.
   */
  view: DataView;
}

const trailerBytes = new TextEncoder().encode(BUN_TRAILER);

const SECTION_FINDERS: Record<
  Exclude<ExecutableFormat, "unknown">,
  (binary: DataView) => SectionLocation | null
> = {
  macho: findMachoBunSection,
  elf: findElfBunSection,
  pe: findPeBunSection,
};

/** Identify the host executable format from its magic bytes. */
export function detectExecutableFormat(binary: DataView): ExecutableFormat {
  if (binary.byteLength < 4) return "unknown";
  const magic = binary.getUint32(0, true);
  if (magic === MACHO_MAGIC_64_LE) return "macho";
  if (magic === ELF_MAGIC_LE) return "elf";
  if ((magic & 0xffff) === 0x5a4d) return "pe"; // "MZ" DOS signature
  return "unknown";
}

/** The file offset of the embedded `__BUN`/`.bun` section, or null if none. */
export function findBunSectionOffset(binary: DataView): number | null {
  const format = detectExecutableFormat(binary);
  if (format === "unknown") return null;
  return SECTION_FINDERS[format](binary)?.offset ?? null;
}

/**
 * Locate the embedded Bun payload, trying strategies in order:
 *   1. A named section (`__BUN/__bun`, `.bun`) — modern macOS/Linux/Windows.
 *   2. Appended to the end of the file with a trailing byte count — legacy.
 *
 * Throws (rather than returning null) so the specific failure is surfaced
 * instead of silently proceeding with bad data.
 */
export function locateBunSection(binary: DataView): BunSection {
  const format = detectExecutableFormat(binary);

  // 1. Modern binaries embed the payload in a named section.
  if (format !== "unknown") {
    const location = SECTION_FINDERS[format](binary);
    if (location) {
      const view = new DataView(binary.buffer, binary.byteOffset + location.offset, location.size);
      assertTrailerAtEnd(view, `${format} .bun section`);
      return { container: format, view };
    }
  }

  // 2. Legacy binaries (old macOS/Linux/Windows, ≲ Bun 1.2.x) append the payload.
  const appended = locateAppended(binary);
  if (appended) return appended;

  // 3. Nothing matched.
  throw new InvalidTrailerError(
    format === "unknown"
      ? "not a Mach-O, ELF, or PE executable and no appended Bun payload"
      : `no __BUN/.bun section and no appended Bun payload in ${format.toUpperCase()} executable`,
  );
}

/**
 * Legacy appended layout: `[native code][payload][Offsets][trailer 16B][totalByteCount 8B]`.
 * Returns a view trimmed to end at the trailer (matching the section containers),
 * or null if there is no appended trailer.
 */
function locateAppended(binary: DataView): BunSection | null {
  const trailerStart = binary.byteLength - 8 - trailerBytes.length;
  if (trailerStart < 0 || !hasTrailerAt(binary, trailerStart)) return null;

  const totalByteCount = binary.getUint32(binary.byteLength - 8, true);
  if (totalByteCount !== binary.byteLength) throw new TotalByteCountMismatchError();

  const view = new DataView(binary.buffer, binary.byteOffset, binary.byteLength - 8);
  return { container: "append", view };
}

function assertTrailerAtEnd(view: DataView, what: string): void {
  if (!hasTrailerAt(view, view.byteLength - trailerBytes.length)) {
    throw new InvalidTrailerError(`${what} is missing the Bun trailer`);
  }
}

function hasTrailerAt(view: DataView, offset: number): boolean {
  if (offset < 0 || offset + trailerBytes.length > view.byteLength) return false;
  for (let i = 0; i < trailerBytes.length; i++) {
    if (view.getUint8(offset + i) !== trailerBytes[i]) return false;
  }
  return true;
}
