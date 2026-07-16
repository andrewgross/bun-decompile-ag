import type { BunFile } from "bun";

import { readdirSync } from "node:fs";
import { basename, join } from "node:path";

import { beforeAll, beforeEach, describe, expect, test } from "bun:test";

import {
  extractBundledFiles,
  getExecutableVersion,
  InvalidExecutableError,
  InvalidTrailerError,
  TotalByteCountMismatchError,
} from "..";
import { detectExecutableFormat, locateBunSection } from "../formats";
import { removeBunfsRootFromPath } from "../modules";

let dummy: BunFile;
let dummyData: ArrayBuffer;

let notAnExecutable: ArrayBuffer;

const expectedVersion = process.env.DUMMY_VERSION;

if (!expectedVersion) {
  throw new Error("$DUMMY_VERSION is not set");
}

beforeAll(async () => {
  // Get a reference to the dummy executable file
  dummy = Bun.file("src/lib/tests/dummy/dummy");

  // Generate some binary data which is not a Bun-compiled executable
  notAnExecutable = new ArrayBuffer(8);
  const view = new DataView(notAnExecutable);
  view.setUint32(0, 0xdeadbeef, true);
  view.setUint32(4, 0xdeadbeef, true);
});

beforeEach(async () => {
  // Re-read the executable data from the file before each test
  dummyData = await dummy.arrayBuffer();
});

describe("extractBundledFiles", () => {
  test("with dummy executable", () => {
    const bundledFiles = extractBundledFiles(dummyData);

    // There should be exactly four bundled files
    expect(bundledFiles).toHaveLength(4);

    // All file paths should not have slashes
    for (const bundledFile of bundledFiles) {
      expect(bundledFile.path).not.toInclude("/");
    }

    // The first file should be the entrypoint
    expect(bundledFiles[0].path).toBe("index.js");

    // After that, the rest of the files will be in an unknown order
    // so we'll sort them by path to make the test deterministic
    const restSorted = bundledFiles.slice(1).sort((a, b) => a.path.localeCompare(b.path));
    expect(restSorted[0].path).toMatch(/^fakeversion.*\.bin$/);
    expect(restSorted[1].path).toMatch(/^favicon.*\.png$/);
    expect(restSorted[2].path).toMatch(/^password2.*\.bin$/);
  });

  test("with a non-executable", () => {
    expect(() => extractBundledFiles(notAnExecutable)).toThrowError(InvalidTrailerError);
  });

  test("with a corrupt executable", () => {
    // Corrupt the binary so neither Mach-O nor appended format can find valid data
    const corrupted = dummyData.slice(0);
    const view = new DataView(corrupted);

    // Corrupt the Mach-O magic (first 4 bytes) to prevent Mach-O section detection
    view.setUint32(0, 0xdeadbeef, true);

    // The appended format check will also fail since the file end doesn't have a valid trailer

    expect(() => extractBundledFiles(corrupted)).toThrowError(InvalidTrailerError);
  });
});

describe("getExecutableVersion", () => {
  test("with dummy executable", () => {
    const version = getExecutableVersion(dummyData);

    // The version of Bun in the executable should be the same as the current runtime,
    // as we call Bun build earlier with this same instance of Bun (supposedly)

    // Use a RegEx to allow for canary versions (eg. 1.1.22-canary.96)
    expect(version.version).toMatch(new RegExp(`^${expectedVersion}(-.+)?$`));
  });

  test("with current runtime", async () => {
    expect(process.execPath).toBeString();

    const runtimeExecutable = Bun.file(process.execPath);
    const runtimeExecutableData = await runtimeExecutable.arrayBuffer();

    const version = getExecutableVersion(runtimeExecutableData);

    // The versions should be equal as we are comparing to the current runtime
    expect(version.version).toMatch(new RegExp(`^${Bun.version}(-.+)?$`));
  });

  test("with a non-executable", () => {
    expect(() => getExecutableVersion(notAnExecutable)).toThrowError(InvalidExecutableError);
  });

  test("with Bun 1.4.0+ banner-only format", () => {
    // Bun 1.4.0+ compiled binaries no longer embed a literal version after the
    // ANSI "bun build v" marker (it's a runtime-substituted placeholder); only
    // the "Bun v<version> (<sha>)" banner remains. Synthesize that layout: a
    // 512-byte buffer whose trailing offsetByteCount is 0 (so the legacy search
    // limit stays wide) with the banner near the start.
    const buf = new Uint8Array(512);
    buf.set(new TextEncoder().encode("Bun v1.4.0 (63bb0ca0d)"), 64);

    const version = getExecutableVersion(buf.buffer);
    expect(version.version).toBe("1.4.0");
    expect(version.revision).toBe("63bb0ca0d");
  });

  test("with the pre-1.1.26 bun meta format", () => {
    // Bun <= 1.1.25 wrote "----- bun meta -----\nBun v<version> (<rev>):".
    // Covered end-to-end by the e2e suite (which compiles real 1.1.x binaries);
    // pinned here so the marker is exercised without downloading a Bun release.
    const buf = new Uint8Array(512);
    buf.set(new TextEncoder().encode("----- bun meta -----\nBun v1.1.0 (5903a614):"), 64);

    const version = getExecutableVersion(buf.buffer);
    expect(version.version).toBe("1.1.0");
    expect(version.revision).toBe("5903a614");
  });

  test("skips a marker hit that isn't a version", () => {
    // "Bun v" also shows up in help text, and 1.4.0+ leaves an unsubstituted
    // placeholder after the ANSI marker, so the first hit is not always real.
    const buf = new Uint8Array(512);
    const enc = new TextEncoder();
    buf.set(enc.encode("Bun v<version> is not a version"), 32);
    buf.set(enc.encode("Bun v1.4.0 (63bb0ca0d)"), 128);

    const version = getExecutableVersion(buf.buffer);
    expect(version.version).toBe("1.4.0");
  });
});

// Wrap a raw Bun payload (the bytes of a __BUN/.bun section) in a minimal
// synthetic ELF64 / PE, so the ELF and PE section finders can be exercised
// without committing hundred-MB cross-compiled binaries.
function wrapInElf(payload: Uint8Array): ArrayBuffer {
  const SHDR = 64;
  const NSEC = 3; // null, .shstrtab, .bun
  const bunOffset = 64;
  const strtabOffset = bunOffset + payload.length;
  const strtab = new TextEncoder().encode("\0.shstrtab\0.bun\0"); // ".shstrtab"@1, ".bun"@11
  const shoff = strtabOffset + strtab.length;
  const buf = new Uint8Array(shoff + NSEC * SHDR);
  const dv = new DataView(buf.buffer);

  buf.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1], 0); // magic, EI_CLASS=64, EI_DATA=LE, version
  dv.setBigUint64(40, BigInt(shoff), true); // e_shoff
  dv.setUint16(58, SHDR, true); // e_shentsize
  dv.setUint16(60, NSEC, true); // e_shnum
  dv.setUint16(62, 1, true); // e_shstrndx -> .shstrtab

  buf.set(payload, bunOffset);
  buf.set(strtab, strtabOffset);

  const shstrtab = shoff + 1 * SHDR;
  dv.setUint32(shstrtab, 1, true); // sh_name -> ".shstrtab"
  dv.setBigUint64(shstrtab + 24, BigInt(strtabOffset), true);
  dv.setBigUint64(shstrtab + 32, BigInt(strtab.length), true);

  const bun = shoff + 2 * SHDR;
  dv.setUint32(bun, 11, true); // sh_name -> ".bun"
  dv.setBigUint64(bun + 24, BigInt(bunOffset), true);
  dv.setBigUint64(bun + 32, BigInt(payload.length), true);

  return buf.buffer;
}

function wrapInPe(payload: Uint8Array): ArrayBuffer {
  const peOffset = 64;
  const sectionTable = peOffset + 24; // 4 (signature) + 20 (COFF), no optional header
  const payloadOffset = sectionTable + 40; // one section header
  const buf = new Uint8Array(payloadOffset + payload.length);
  const dv = new DataView(buf.buffer);

  buf.set([0x4d, 0x5a], 0); // "MZ"
  dv.setUint32(0x3c, peOffset, true); // e_lfanew
  dv.setUint32(peOffset, 0x00004550, true); // "PE\0\0"
  dv.setUint16(peOffset + 6, 1, true); // NumberOfSections
  dv.setUint16(peOffset + 20, 0, true); // SizeOfOptionalHeader

  buf.set(new TextEncoder().encode(".bun"), sectionTable); // section name (8 bytes)
  dv.setUint32(sectionTable + 8, payload.length, true); // VirtualSize
  dv.setUint32(sectionTable + 16, payload.length, true); // SizeOfRawData
  dv.setUint32(sectionTable + 20, payloadOffset, true); // PointerToRawData

  buf.set(payload, payloadOffset);
  return buf.buffer;
}

describe("container formats", () => {
  test("detectExecutableFormat identifies magic bytes", () => {
    const detect = (bytes: number[]) => {
      const b = new Uint8Array(64);
      b.set(bytes, 0);
      return detectExecutableFormat(new DataView(b.buffer));
    };
    expect(detect([0xcf, 0xfa, 0xed, 0xfe])).toBe("macho"); // 0xFEEDFACF little-endian
    expect(detect([0x7f, 0x45, 0x4c, 0x46])).toBe("elf");
    expect(detect([0x4d, 0x5a])).toBe("pe");
    expect(detect([0xde, 0xad, 0xbe, 0xef])).toBe("unknown");
  });

  // The dummy binary is compiled by the host's Bun, so its container varies by
  // OS (Mach-O on macOS, ELF on Linux). Pull the payload out container-agnostically
  // rather than assuming Mach-O, then re-wrap it to exercise each container's
  // section finder on every OS.
  function dummyPayload(): Uint8Array {
    const { view } = locateBunSection(new DataView(dummyData));
    return new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
  }

  test("extracts through a synthetic ELF .bun section", () => {
    const files = extractBundledFiles(wrapInElf(dummyPayload()));
    expect(files[0].path).toBe("index.js");
    expect(files).toHaveLength(4);
  });

  test("extracts through a synthetic PE .bun section", () => {
    const files = extractBundledFiles(wrapInPe(dummyPayload()));
    expect(files[0].path).toBe("index.js");
    expect(files).toHaveLength(4);
  });
});

describe("removeBunfsRootFromPath", () => {
  test("strips unix, legacy, and Windows roots", () => {
    expect(removeBunfsRootFromPath("/$bunfs/root/index.js")).toBe("/index.js");
    expect(removeBunfsRootFromPath("compiled://root/a.js")).toBe("/a.js");
    expect(removeBunfsRootFromPath("B:/~BUN/root/index.js")).toBe("/index.js");
    expect(removeBunfsRootFromPath("Z:\\~BUN\\root\\x.js")).toBe("\\x.js");
  });

  test("throws on an unrecognised root", () => {
    expect(() => removeBunfsRootFromPath("/nope/index.js")).toThrow();
  });
});

describe("multi-version fixtures", () => {
  const fixturesDir = join(import.meta.dir, "fixtures");
  let fixtures: string[] = [];

  try {
    fixtures = readdirSync(fixturesDir)
      .filter((f) => f.startsWith("v") && f.endsWith(".bin"))
      .map((f) => join(fixturesDir, f));
  } catch {
    // fixtures directory may not exist yet
  }

  if (fixtures.length === 0) {
    test.skip("no fixtures found (run `bun run build-fixtures` first)", () => {});
  } else {
    for (const fixturePath of fixtures) {
      test(`parses ${basename(fixturePath)}`, async () => {
        const data = await Bun.file(fixturePath).arrayBuffer();
        const files = extractBundledFiles(data);
        expect(files.length).toBeGreaterThan(0);
        expect(files[0].path).toBe("index.js");
      });
    }
  }
});
