#![no_main]

//! The no-panic contract for [`extract_bundled_files`] and
//! [`extract_payload`]: any byte input must return `Ok` or `Err`, never panic
//! or abort. Seed the corpus from the committed fixtures (see
//! `fuzz/README.md`).

use bun_decompile_ag::{ExtractOptions, extract_bundled_files, extract_payload};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Both option settings, since normalization changes the path handling.
    let _ = extract_bundled_files(data, ExtractOptions::default());
    let _ = extract_bundled_files(
        data,
        ExtractOptions {
            normalize_entrypoint_file_name: false,
        },
    );
    let _ = extract_payload(data);
});
