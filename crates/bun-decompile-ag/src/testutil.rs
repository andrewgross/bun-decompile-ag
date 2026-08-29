//! Test-only helpers shared by the unit tests.

#![allow(clippy::unwrap_used)]

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
