import type { SectionLocation } from "./sections.js";

import { LC_SEGMENT_64, MACHO_MAGIC_64_LE } from "./constants.js";
import { readCString } from "./sections.js";

/**
 * Find the `__BUN/__bun` section in a Mach-O 64-bit binary (macOS).
 * Returns the file offset and size, or null if not a Mach-O or section not found.
 */
export function findMachoBunSection(binary: DataView): SectionLocation | null {
  if (binary.byteLength < 32) return null;

  if (binary.getUint32(0, true) !== MACHO_MAGIC_64_LE) return null;

  // Mach-O 64-bit header:
  //   0: magic (4)      4: cputype (4)    8: cpusubtype (4)  12: filetype (4)
  //  16: ncmds (4)     20: sizeofcmds (4) 24: flags (4)      28: reserved (4)
  const ncmds = binary.getUint32(16, true);
  const sizeofcmds = binary.getUint32(20, true);
  const headerSize = 32;

  if (binary.byteLength < headerSize + sizeofcmds) return null;

  let offset = headerSize;

  for (let i = 0; i < ncmds; i++) {
    if (offset + 8 > binary.byteLength) break;

    const cmd = binary.getUint32(offset, true);
    const cmdsize = binary.getUint32(offset + 4, true);

    if (cmdsize < 8 || offset + cmdsize > binary.byteLength) break;

    if (cmd === LC_SEGMENT_64) {
      // LC_SEGMENT_64: 8: segname (16) ... 64: nsects (4). Section headers follow at +72.
      const segname = readCString(binary, offset + 8, 16);

      if (segname === "__BUN") {
        const nsects = binary.getUint32(offset + 64, true);
        let sectOffset = offset + 72;

        for (let j = 0; j < nsects; j++) {
          // section_64: 0: sectname (16) ... 40: size (8, u64), 48: offset (4, u32). Total 80.
          if (sectOffset + 80 > binary.byteLength) break;

          const sectname = readCString(binary, sectOffset, 16);

          if (sectname === "__bun") {
            const size = Number(binary.getBigUint64(sectOffset + 40, true));
            const fileOffset = binary.getUint32(sectOffset + 48, true);
            return { offset: fileOffset, size };
          }

          sectOffset += 80;
        }
      }
    }

    offset += cmdsize;
  }

  return null;
}
