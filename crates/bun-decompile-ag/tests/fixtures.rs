//! Public-API extraction over every committed fixture, snapshotted. Each
//! line pins a module's path, length, and a content fingerprint, so a read
//! from the wrong place with the right length (the pre-pointer Bun 1.4.1
//! failure) still shows up as a snapshot diff.

#![allow(clippy::unwrap_used)]

mod common;

use bun_decompile_ag::{
    ExtractError, ExtractOptions, extract_bundled_files, extract_payload, get_executable_version,
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
fn extract_bundled_files_on_sourcemap_fixtures_reproduces_the_bundlers_map() {
    for (name, map_name) in common::SOURCEMAP_FIXTURES {
        let files =
            extract_bundled_files(&common::fixture(name), ExtractOptions::default()).unwrap();
        let expected: serde_json::Value =
            serde_json::from_slice(&common::fixture_file(&format!("{map_name}.map"))).unwrap();

        assert_eq!(files[0].path, "index.js", "{name}");
        let sourcemap = files[0].sourcemap.as_ref().unwrap();
        // The binary form records nothing after the last mapping, so the
        // bundler's trailing `;` separators do not survive it (Bun's own
        // re-encoding drops them too).
        let mappings = expected["mappings"].as_str().unwrap();
        assert_eq!(sourcemap.mappings, mappings.trim_end_matches(';'), "{name}");
        let sources: Vec<&str> = expected["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|source| source.as_str().unwrap())
            .collect();
        assert_eq!(sourcemap.sources, sources, "{name}");
        if name == map_name {
            // Same build as the map, so the same debug id.
            assert_eq!(
                sourcemap.debug_id.as_deref(),
                expected["debugId"].as_str(),
                "{name}"
            );
        }
    }
}

#[test]
fn extract_bundled_files_on_every_fixture_yields_vlq_mappings() {
    // Mappings are base64 VLQ text whatever form Bun stored them in. Before
    // the binary form was decoded, Bun 1.4.x maps carried its raw bytes.
    let names = common::FIXTURES
        .into_iter()
        .chain(common::SOURCEMAP_FIXTURES.into_iter().map(|(name, _)| name));
    for name in names {
        let files =
            extract_bundled_files(&common::fixture(name), ExtractOptions::default()).unwrap();
        for file in &files {
            let Some(sourcemap) = &file.sourcemap else {
                continue;
            };
            let bad = sourcemap
                .mappings
                .bytes()
                .find(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b',' | b';')));
            assert_eq!(bad, None, "{name}: {} has non-VLQ mappings", file.path);
        }
    }
}

#[test]
fn extract_payload_on_every_fixture_returns_it_unchanged() {
    // A fixture is already a stripped payload in the appended layout, so
    // stripping it again is the identity.
    for name in common::FIXTURES {
        let data = common::fixture(name);
        assert_eq!(extract_payload(&data).unwrap(), data, "{name}");
    }
}

#[test]
fn extract_payload_on_a_non_executable_errors_with_invalid_trailer() {
    let err = extract_payload(&[0_u8; 64]).unwrap_err();
    assert!(matches!(err, ExtractError::InvalidTrailer(_)), "{err:?}");
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
