import type { SectionLocation } from "./sections.js";

import { readCString } from "./sections.js";

/**
 * Find the `.bun` section in a PE32+ binary (Windows).
 * Bun embeds the compiled payload in a `.bun` section rather than appending it.
 * Returns the file offset and size, or null if not a PE or section not found.
 */
export function findPeBunSection(binary: DataView): SectionLocation | null {
  if (binary.byteLength < 64) return null;
  if (binary.getUint16(0, true) !== 0x5a4d) return null; // "MZ" DOS signature

  const peOffset = binary.getUint32(0x3c, true); // e_lfanew
  if (peOffset + 24 > binary.byteLength) return null;
  if (binary.getUint32(peOffset, true) !== 0x00004550) return null; // "PE\0\0"

  // COFF header (after the 4-byte signature): 2: NumberOfSections, 16: SizeOfOptionalHeader
  const numSections = binary.getUint16(peOffset + 6, true);
  const optHeaderSize = binary.getUint16(peOffset + 20, true);

  // Section table follows the optional header (signature 4 + COFF header 20 = 24).
  const sectionTable = peOffset + 24 + optHeaderSize;
  if (sectionTable + numSections * 40 > binary.byteLength) return null;

  for (let i = 0; i < numSections; i++) {
    const sh = sectionTable + i * 40; // section header is 40 bytes

    if (readCString(binary, sh, 8) === ".bun") {
      // 8: VirtualSize, 16: SizeOfRawData, 20: PointerToRawData.
      // VirtualSize is the exact payload length; SizeOfRawData is file-aligned
      // and may include trailing padding past the Bun trailer.
      const virtualSize = binary.getUint32(sh + 8, true);
      const rawSize = binary.getUint32(sh + 16, true);
      const rawPointer = binary.getUint32(sh + 20, true);

      let size = virtualSize > 0 && virtualSize <= rawSize ? virtualSize : rawSize;
      if (rawPointer + size > binary.byteLength) size = binary.byteLength - rawPointer;
      if (size <= 0) return null;

      return { offset: rawPointer, size };
    }
  }

  return null;
}
