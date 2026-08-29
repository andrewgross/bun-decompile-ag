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
    let path = format!(
        "{}/../../src/lib/tests/dummy/{name}",
        env!("CARGO_MANIFEST_DIR"),
    );
    std::fs::read(path).unwrap()
}

/// Read a committed fixture from `src/lib/tests/fixtures/`. Fixtures are
/// container-stripped payloads laid out as
/// `[modules][metadata][Offsets][trailer][total byte count: 8 bytes]`
/// (see `src/scripts/extract-section.ts`).
pub(crate) fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../src/lib/tests/fixtures/{name}.bin",
        env!("CARGO_MANIFEST_DIR"),
    );
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
