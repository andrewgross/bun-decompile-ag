//! Public-API extraction over every committed fixture, snapshotted. Each
//! line pins a module's path, length, and a content fingerprint, so a read
//! from the wrong place with the right length (the pre-pointer Bun 1.4.1
//! failure) still shows up as a snapshot diff.

#![allow(clippy::unwrap_used)]

mod common;

use bun_decompile_ag::{
    ExtractError, ExtractOptions, extract_bundled_files, get_executable_version,
};

/// FNV-1a (64-bit): a dependency-free content fingerprint for snapshots.
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[test]
fn extract_bundled_files_snapshots_every_fixture() {
    for name in common::FIXTURES {
        let data = common::fixture(name);
        let files = extract_bundled_files(&data, ExtractOptions::default()).unwrap();

        let lines: Vec<String> = files
            .iter()
            .map(|file| {
                let sourcemap = match &file.sourcemap {
                    Some(map) => format!(
                        "sm{{mappings:{},sources:{},debugId:{}}}",
                        map.mappings.len(),
                        map.sources.len(),
                        map.debug_id.as_deref().unwrap_or("none"),
                    ),
                    None => "sm{none}".to_owned(),
                };
                format!(
                    "{} len={} fnv={:016x} {}",
                    file.path,
                    file.contents.len(),
                    fnv1a64(&file.contents),
                    sourcemap,
                )
            })
            .collect();

        insta::assert_snapshot!(name, lines.join("\n"));
    }
}

#[test]
fn get_executable_version_on_stripped_fixtures_finds_nothing() {
    // Fixtures are container-stripped payloads with no Bun runtime in them,
    // so there is no version string to find — and the search limit keeps the
    // module data itself out of the scan.
    for name in common::FIXTURES {
        let data = common::fixture(name);
        assert_eq!(
            get_executable_version(&data),
            Err(ExtractError::VersionNotFound),
            "{name}",
        );
    }
}

#[test]
fn extract_bundled_files_on_a_non_executable_errors_with_invalid_trailer() {
    let junk = [0xDE_u8, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD, 0xBE, 0xEF];
    let err = extract_bundled_files(&junk, ExtractOptions::default()).unwrap_err();
    assert!(matches!(err, ExtractError::InvalidTrailer(_)), "{err:?}");
}
