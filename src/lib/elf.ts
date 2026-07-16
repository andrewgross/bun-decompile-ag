import type { SectionLocation } from "./sections.js";

import { ELF_MAGIC_LE } from "./constants.js";
import { readCString } from "./sections.js";

/**
 * Find the `.bun` section in an ELF 64-bit binary (Linux / FreeBSD).
 * Bun ≥ ~1.3.10 embeds the compiled payload here (older versions appended it).
 * Returns the file offset and size, or null if not an ELF64 or section not found.
 */
export function findElfBunSection(binary: DataView): SectionLocation | null {
  if (binary.byteLength < 64) return null;
  if (binary.getUint32(0, true) !== ELF_MAGIC_LE) return null;

  const isElf64 = binary.getUint8(4) === 2; // EI_CLASS: 2 = 64-bit
  const le = binary.getUint8(5) === 1; // EI_DATA: 1 = little-endian
  if (!isElf64) return null; // Bun only emits ELF64

  // ELF64 header: 40: e_shoff (8), 58: e_shentsize (2), 60: e_shnum (2), 62: e_shstrndx (2)
  const shoff = Number(binary.getBigUint64(40, le));
  const shentsize = binary.getUint16(58, le);
  const shnum = binary.getUint16(60, le);
  const shstrndx = binary.getUint16(62, le);

  if (shoff === 0 || shnum === 0 || shentsize < 64) return null;
  if (shoff + shnum * shentsize > binary.byteLength) return null;
  if (shstrndx >= shnum) return null;

  // Section-header string table (.shstrtab): section_header: 24: sh_offset (8), 32: sh_size (8)
  const strHdr = shoff + shstrndx * shentsize;
  const strTabOffset = Number(binary.getBigUint64(strHdr + 24, le));
  const strTabSize = Number(binary.getBigUint64(strHdr + 32, le));

  for (let i = 0; i < shnum; i++) {
    const sh = shoff + i * shentsize;
    const nameOffset = binary.getUint32(sh, le); // sh_name: index into .shstrtab
    if (nameOffset >= strTabSize) continue;

    if (readCString(binary, strTabOffset + nameOffset, 64) === ".bun") {
      const sectOffset = Number(binary.getBigUint64(sh + 24, le));
      const sectSize = Number(binary.getBigUint64(sh + 32, le));
      if (sectOffset + sectSize > binary.byteLength) return null;
      return { offset: sectOffset, size: sectSize };
    }
  }

  return null;
}
