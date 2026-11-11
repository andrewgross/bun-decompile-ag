//! Test-only helpers shared by the unit tests.

#![allow(clippy::unwrap_used)]

/// Every committed fixture: one per metadata format revision and target,
/// plus the Bun 1.4.0 (banner-only version) and 1.4.1 (path table) layouts.
pub(crate) const FIXTURES: [&str; 16] = [
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

/// Read a source file of the dummy program the fixtures were compiled from.
pub(crate) fn dummy_asset(name: &str) -> Vec<u8> {
    let path = format!("{}/../../fixtures/dummy/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).unwrap()
}

/// Read a committed fixture from `fixtures/`. Fixtures are container-stripped
/// payloads laid out as
/// `[modules][metadata][Offsets][trailer][total byte count: 8 bytes]`
/// (see `extract_payload`, which `scripts/build-fixtures.sh` uses).
pub(crate) fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/../../fixtures/{name}.bin", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).unwrap()
}

/// A fixture's payload: the bytes ending exactly at the Bun trailer, i.e.
/// what a `__BUN`/`.bun` section holds. This is the shape the container
/// tests wrap in synthetic executables.
pub(crate) fn fixture_payload(name: &str) -> Vec<u8> {
    let mut data = fixture(name);
    data.truncate(data.len() - 8);
    data
}

/// The mapping bytes of the entrypoint's sourcemap in a V4 fixture, and the
/// entrypoint's contents length. Fixtures hold the modules region at offset
/// 0, the entrypoint is module 0, and its record's sourcemap pointer sits at
/// +16/+20.
pub(crate) fn entry_mappings(name: &str) -> (Vec<u8>, usize) {
    let data = fixture(name);
    let u32_at = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
    let offsets = data.len() - 8 - 16 - 32;
    let record = u32_at(offsets + 8);
    let contents_len = u32_at(record + 12);
    let sourcemap = u32_at(record + 16);
    let sources = u32_at(sourcemap);
    let len = u32_at(sourcemap + 4);
    let start = sourcemap + 8 + 16 * sources;
    (data[start..start + len].to_vec(), contents_len)
}

/// Read any committed file under `fixtures/` by its full name.
pub(crate) fn fixture_file(name: &str) -> Vec<u8> {
    let path = format!("{}/../../fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).unwrap()
}
