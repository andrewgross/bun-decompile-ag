//! Shared helpers for the integration tests: fixture loading and the
//! synthetic container builders (the integration-level counterparts of the
//! TS suite's `wrapInElf`/`wrapInPe`; the unit tests have their own inside
//! the crate).

#![allow(dead_code)] // Each integration test binary uses a subset.
#![allow(clippy::unwrap_used)]

/// Every committed fixture: one per metadata format revision and target,
/// plus the Bun 1.4.0 (banner-only version) and 1.4.1 (path table) layouts.
pub const FIXTURES: [&str; 16] = [
    "v1.1.0-native",
    "v1.1.26-darwin-arm64",
    "v1.1.26-linux-x64",
    "v1.1.26-windows-x64",
    "v1.2.4-darwin-arm64",
    "v1.2.4-linux-x64",
    "v1.2.4-windows-x64",
    "v1.3.9-darwin-arm64",
    "v1.3.9-linux-x64",
    "v1.3.9-windows-x64",
    "v1.4.0-darwin-arm64",
    "v1.4.0-linux-x64",
    "v1.4.0-windows-x64",
    "v1.4.1-darwin-arm64",
    "v1.4.1-linux-x64",
    "v1.4.1-windows-x64",
];

/// Read a committed fixture: an appended-format blob
/// `[payload ending at trailer][total byte count: 8 bytes]`.
pub fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/../../fixtures/{name}.bin", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).unwrap()
}

/// A fixture's payload: the bytes ending exactly at the Bun trailer — what
/// a `__BUN`/`.bun` section holds.
pub fn fixture_payload(name: &str) -> Vec<u8> {
    let mut data = fixture(name);
    data.truncate(data.len() - 8);
    data
}

/// Path of the compiled dummy binary (present only after
/// `bun build --compile` has run; tests soft-skip without it).
pub fn dummy_binary() -> Option<Vec<u8>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/dummy/dummy");
    std::fs::read(path).ok()
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Minimal Mach-O 64 wrapping `payload` in a `__BUN/__bun` section.
pub fn wrap_in_macho(payload: &[u8]) -> Vec<u8> {
    let cmdsize = 72 + 80;
    let payload_offset = 32 + cmdsize;
    let mut b = vec![0_u8; payload_offset + payload.len()];

    put_u32(&mut b, 0, 0xFEED_FACF); // MH_MAGIC_64
    put_u32(&mut b, 16, 1); // ncmds
    put_u32(&mut b, 20, u32::try_from(cmdsize).unwrap()); // sizeofcmds

    put_u32(&mut b, 32, 0x19); // LC_SEGMENT_64
    put_u32(&mut b, 36, u32::try_from(cmdsize).unwrap());
    b[40..45].copy_from_slice(b"__BUN"); // segname
    put_u32(&mut b, 32 + 64, 1); // nsects

    let sect = 32 + 72;
    b[sect..sect + 5].copy_from_slice(b"__bun"); // sectname
    put_u64(&mut b, sect + 40, payload.len() as u64); // size
    put_u32(&mut b, sect + 48, u32::try_from(payload_offset).unwrap()); // offset

    b[payload_offset..].copy_from_slice(payload);
    b
}

/// Minimal ELF64 wrapping `payload` in a `.bun` section.
pub fn wrap_in_elf(payload: &[u8]) -> Vec<u8> {
    const SHDR: usize = 64;
    const NSEC: usize = 3; // null, .shstrtab, .bun
    let bun_offset = 64;
    let strtab_offset = bun_offset + payload.len();
    let strtab = b"\0.shstrtab\0.bun\0"; // ".shstrtab" @ 1, ".bun" @ 11
    let shoff = strtab_offset + strtab.len();
    let mut b = vec![0_u8; shoff + NSEC * SHDR];

    b[0..7].copy_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1]);
    put_u64(&mut b, 40, shoff as u64); // e_shoff
    b[58..60].copy_from_slice(&u16::try_from(SHDR).unwrap().to_le_bytes()); // e_shentsize
    b[60..62].copy_from_slice(&u16::try_from(NSEC).unwrap().to_le_bytes()); // e_shnum
    b[62..64].copy_from_slice(&1_u16.to_le_bytes()); // e_shstrndx

    b[bun_offset..bun_offset + payload.len()].copy_from_slice(payload);
    b[strtab_offset..strtab_offset + strtab.len()].copy_from_slice(strtab);

    let shstrtab = shoff + SHDR;
    put_u32(&mut b, shstrtab, 1);
    put_u64(&mut b, shstrtab + 24, strtab_offset as u64);
    put_u64(&mut b, shstrtab + 32, strtab.len() as u64);

    let bun = shoff + 2 * SHDR;
    put_u32(&mut b, bun, 11);
    put_u64(&mut b, bun + 24, bun_offset as u64);
    put_u64(&mut b, bun + 32, payload.len() as u64);

    b
}

/// Minimal PE wrapping `payload` in a `.bun` section.
pub fn wrap_in_pe(payload: &[u8]) -> Vec<u8> {
    let pe_offset = 64;
    let section_table = pe_offset + 24; // signature 4 + COFF 20, no optional header
    let payload_offset = section_table + 40; // one section header
    let mut b = vec![0_u8; payload_offset + payload.len()];

    b[0] = b'M';
    b[1] = b'Z';
    put_u32(&mut b, 0x3C, u32::try_from(pe_offset).unwrap()); // e_lfanew
    put_u32(&mut b, pe_offset, 0x0000_4550); // "PE\0\0"
    b[pe_offset + 6] = 1; // NumberOfSections

    b[section_table..section_table + 4].copy_from_slice(b".bun");
    put_u32(
        &mut b,
        section_table + 8,
        u32::try_from(payload.len()).unwrap(),
    ); // VirtualSize
    put_u32(
        &mut b,
        section_table + 16,
        u32::try_from(payload.len()).unwrap(),
    ); // SizeOfRawData
    put_u32(
        &mut b,
        section_table + 20,
        u32::try_from(payload_offset).unwrap(),
    );

    b[payload_offset..].copy_from_slice(payload);
    b
}
