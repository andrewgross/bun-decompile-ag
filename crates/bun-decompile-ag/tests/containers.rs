//! Public-API extraction through each synthetic container, and against the
//! real compiled dummy binary when present — the integration port of the TS
//! suite's "container formats" tests.

#![allow(clippy::unwrap_used)]

mod common;

use bun_decompile_ag::{ExtractOptions, extract_bundled_files, get_executable_version};

fn assert_dummy_bundle(binary: &[u8], what: &str) {
    let files = extract_bundled_files(binary, ExtractOptions::default()).unwrap();
    assert_eq!(files[0].path, "index.js", "{what}");
    // Bun 1.4.1+ also embeds the text-loaded password1 as its own module,
    // so the real dummy holds 4 or 5 files depending on the Bun that built it.
    for asset in ["favicon", "password2", "fakeversion"] {
        assert!(
            files.iter().any(|f| f.path.starts_with(asset)),
            "{what}: no {asset}* file",
        );
    }
    assert!((4..=5).contains(&files.len()), "{what}: {}", files.len());
}

#[test]
fn extracts_through_a_synthetic_macho_bun_section() {
    let binary = common::wrap_in_macho(&common::fixture_payload("v1.3.9-darwin-arm64"));
    assert_dummy_bundle(&binary, "macho");
}

#[test]
fn extracts_through_a_synthetic_elf_bun_section() {
    let binary = common::wrap_in_elf(&common::fixture_payload("v1.3.9-linux-x64"));
    assert_dummy_bundle(&binary, "elf");
}

#[test]
fn extracts_through_a_synthetic_pe_bun_section() {
    let binary = common::wrap_in_pe(&common::fixture_payload("v1.3.9-windows-x64"));
    assert_dummy_bundle(&binary, "pe");
}

#[test]
fn a_v2_payload_in_a_named_section_errors_like_the_ts() {
    // The container-to-format shortcut only goes one way: named sections
    // exist since Bun 1.2.4, so the resolver assumes V3 for them. A V2
    // payload wrapped in a section (which no real Bun ever produced) fails
    // the chunk-size invariant — same as the TS implementation.
    let binary = common::wrap_in_elf(&common::fixture_payload("v1.1.26-linux-x64"));
    let err = extract_bundled_files(&binary, ExtractOptions::default()).unwrap_err();
    assert!(
        err.to_string()
            .starts_with("Metadata length 112 is not a multiple of chunk size 36"),
        "{err}",
    );
}

// The real `bun build --compile` output, when built. Extraction and version
// detection through the public API; the embedded fakeversion bait must not
// win over the real version.
#[test]
fn extracts_the_real_dummy_binary_when_present() {
    let Some(binary) = common::dummy_binary() else {
        eprintln!("skipping: dummy binary not built");
        return;
    };
    assert_dummy_bundle(&binary, "dummy");

    let version = get_executable_version(&binary).unwrap();
    assert_ne!(version.version, "fakeversion1");
    assert!(version.version.as_bytes()[0].is_ascii_digit());
}
