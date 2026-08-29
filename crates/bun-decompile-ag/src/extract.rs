//! Walk the module graph and decode every bundled file.
//!
//! Port of the second half of `src/lib/modules.ts`: the module walk itself,
//! entrypoint renaming, bunfs-root handling, and the decoding of Bun's
//! internal serialized sourcemap into a standard v3 sourcemap.
//!
//! V3/V4 records locate a module's contents and sourcemap through absolute
//! `{offset, length}` pointers; V1/V2 records carry none, so their modules
//! are walked sequentially. A serialized sourcemap opens with its sources
//! count and mappings length (u32 each), then two tables of per-source
//! pointers (names, then compressed contents), then the mappings text and
//! the source names.

use crate::bytes::read_u32_le;
use crate::error::{ExtractError, Result};
use crate::graph::ModuleGraph;
use crate::paths::{remove_bunfs_root_from_path, remove_leading_slash};

/// A file recovered from the executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledFile {
    /// Path relative to the bundle root (no leading slash).
    pub path: String,
    /// The file's bytes, exactly as bundled.
    pub contents: Vec<u8>,
    /// Present when the module carried an inline sourcemap.
    pub sourcemap: Option<SourceMap>,
}

/// A standard v3 sourcemap decoded from Bun's internal serialization.
///
/// With the `serde` feature, serializes to the same JSON the TS
/// implementation produced with `JSON.stringify`: declaration field order,
/// `debugId` camelCased, and omitted entirely when absent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SourceMap {
    /// Always 3.
    pub version: u8,
    /// The bundled file this map belongs to (same as `BundledFile::path`).
    pub file: String,
    /// The `//# debugId=` value from the tail of the file, when present.
    #[cfg_attr(
        feature = "serde",
        serde(rename = "debugId", skip_serializing_if = "Option::is_none")
    )]
    pub debug_id: Option<String>,
    /// VLQ mappings text.
    pub mappings: String,
    /// Original source names.
    pub sources: Vec<String>,
}

/// Options for the module walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractOptions {
    /// Rename the entrypoint's filename to `index.js` (default true, like
    /// the TS library).
    pub normalize_entrypoint_file_name: bool,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            normalize_entrypoint_file_name: true,
        }
    }
}

/// Read `len` bytes of the modules region starting at `start`.
fn slice(region: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    let truncated = ExtractError::Truncated { offset: start, len };
    let end = start.checked_add(len).ok_or_else(|| truncated.clone())?;
    region.get(start..end).ok_or(truncated)
}

fn checked_add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or(ExtractError::Truncated { offset: a, len: b })
}

/// Fail when a module's `field` extends past the modules region. The message
/// matches the TS `assertInModules` so CLI output stays identical.
fn check_in_modules(
    start: usize,
    len: usize,
    modules_len: usize,
    path: &str,
    field: &str,
) -> Result<()> {
    if start.checked_add(len).is_none_or(|end| end > modules_len) {
        return Err(ExtractError::InvalidExecutable(format!(
            "The {field} of {path} ({start}+{len}) extends past the modules region ({modules_len})"
        )));
    }
    Ok(())
}

/// Rename the last path segment to `index.js`. Spec: the TS
/// `path.replace(/\/[^\/\\]+$/, "/index.js")` — no rename when the path has
/// no `/`, ends in one, or the final segment contains a backslash.
fn normalize_entrypoint(path: &str) -> String {
    if let Some(idx) = path.rfind('/') {
        let segment = &path[idx + 1..];
        if !segment.is_empty() && !segment.contains('\\') {
            return format!("{}/index.js", &path[..idx]);
        }
    }
    path.to_owned()
}

/// Find a `//# debugId=<id>` line in the tail of the contents. Spec: the TS
/// `/^\/\/# debugId=([a-fA-F0-9-]{12,})$/m` over `contents.slice(len - 49)`
/// — including JS `slice` semantics where a negative index counts from the
/// end, so a sub-49-byte file yields a `2*len - 49` window start. Lines are
/// split on `\n` alone so that a `\r` invalidates the match, as it does for
/// the JS regex's `$`.
fn find_debug_id(contents: &[u8]) -> Option<String> {
    let start = if contents.len() >= 49 {
        contents.len() - 49
    } else {
        (contents.len() * 2).saturating_sub(49)
    };
    let tail = String::from_utf8_lossy(&contents[start..]);
    for line in tail.split('\n') {
        if let Some(id) = line.strip_prefix("//# debugId=") {
            if id.len() >= 12 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
                return Some(id.to_owned());
            }
        }
    }
    None
}

/// Decode the serialized sourcemap that starts `start` bytes into `region`
/// (the modules region, found at `modules_start` in `data`). `None` when it
/// holds no mappings, as in the TS. Header fields are read from `data` like
/// the TS `DataView` reads; mappings and names are sliced from `region`.
fn decode_sourcemap(
    data: &[u8],
    modules_start: usize,
    region: &[u8],
    start: usize,
    path: &str,
    contents: &[u8],
) -> Result<Option<SourceMap>> {
    let header = checked_add(modules_start, start)?;
    let mappings_length = read_u32_le(data, checked_add(header, 4)?)? as usize;
    if mappings_length == 0 {
        return Ok(None);
    }
    let sources_count = read_u32_le(data, header)? as usize;

    // Mappings text follows the 8-byte header and the two per-source pointer
    // tables (16 bytes per source); the source names follow the mappings.
    let table = checked_add(
        8,
        sources_count
            .checked_mul(16)
            .ok_or(ExtractError::Truncated {
                offset: sources_count,
                len: 16,
            })?,
    )?;
    let mappings_start = checked_add(start, table)?;
    let mappings =
        String::from_utf8_lossy(slice(region, mappings_start, mappings_length)?).into_owned();

    let mut sources = Vec::with_capacity(sources_count.min(1024));
    let mut source_start = checked_add(mappings_start, mappings_length)?;
    for j in 0..sources_count {
        let length_at = checked_add(header, checked_add(12, j * 8)?)?;
        let source_length = read_u32_le(data, length_at)? as usize;
        sources.push(
            String::from_utf8_lossy(slice(region, source_start, source_length)?).into_owned(),
        );
        source_start = checked_add(source_start, source_length)?;
    }

    Ok(Some(SourceMap {
        version: 3,
        file: path.to_owned(),
        debug_id: find_debug_id(contents),
        mappings,
        sources,
    }))
}

/// Walk the module graph and return every bundled file, entry point first.
pub(crate) fn extract_modules(
    graph: &ModuleGraph<'_>,
    options: ExtractOptions,
) -> Result<Vec<BundledFile>> {
    let ModuleGraph {
        data,
        offsets,
        format,
        modules_start,
        metadata_start,
        module_count,
    } = *graph;

    let region = data
        .get(modules_start..metadata_start)
        .ok_or(ExtractError::Truncated {
            offset: modules_start,
            len: metadata_start.saturating_sub(modules_start),
        })?;

    let mut files: Vec<BundledFile> = Vec::with_capacity(module_count.min(1024));
    let mut current_offset: usize = 0;

    for i in 0..module_count {
        let is_entrypoint = i == offsets.entry_point_id as usize;
        let meta = checked_add(metadata_start, i * format.chunk_size)?;

        if format.has_absolute_offsets {
            current_offset = read_u32_le(data, meta)? as usize;
        }
        let path_length = read_u32_le(data, checked_add(meta, 4)?)? as usize;
        let contents_length = read_u32_le(data, checked_add(meta, 12)?)? as usize;
        let sourcemap_length = read_u32_le(data, checked_add(meta, 20)?)? as usize;

        let mut path =
            String::from_utf8_lossy(slice(region, current_offset, path_length)?).into_owned();
        if options.normalize_entrypoint_file_name && is_entrypoint {
            path = normalize_entrypoint(&path);
        }
        let stripped = remove_bunfs_root_from_path(&path)?;
        if !stripped.starts_with('/') {
            return Err(ExtractError::InvalidExecutable(
                "Invalid path in bundled file in executable".to_owned(),
            ));
        }
        let path = remove_leading_slash(stripped).to_owned();

        // The new wire format NUL-terminates each string.
        let string_pad = usize::from(format.new_format);

        // Only the record's pointers say where V3/V4 contents and sourcemaps
        // live. Through Bun 1.4.0 they happened to follow the path (path,
        // NUL, contents, NUL, sourcemap), so walking sequentially found the
        // same bytes; Bun 1.4.1 gathers every path into one string table, and
        // "right after the path" lands inside that table instead.
        let contents_start = if format.has_absolute_offsets {
            read_u32_le(data, checked_add(meta, 8)?)? as usize
        } else {
            checked_add(checked_add(current_offset, path_length)?, string_pad)?
        };
        check_in_modules(
            contents_start,
            contents_length,
            region.len(),
            &path,
            "contents",
        )?;
        let contents_end = checked_add(contents_start, contents_length)?;
        let contents = slice(region, contents_start, contents_length)?.to_vec();

        let mut sourcemap = None;
        if sourcemap_length > 0 {
            let sourcemap_start = if format.has_absolute_offsets {
                read_u32_le(data, checked_add(meta, 16)?)? as usize
            } else {
                checked_add(contents_end, 1)?
            };
            check_in_modules(
                sourcemap_start,
                sourcemap_length,
                region.len(),
                &path,
                "sourcemap",
            )?;
            sourcemap = decode_sourcemap(
                data,
                modules_start,
                region,
                sourcemap_start,
                &path,
                &contents,
            )?;
        }

        let file = BundledFile {
            path,
            contents,
            sourcemap,
        };
        if is_entrypoint {
            files.insert(0, file);
        } else {
            files.push(file);
        }

        let advance = path_length + contents_length + sourcemap_length + 2 * string_pad;
        current_offset = checked_add(current_offset, advance)?;
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::container::{BUN_TRAILER, locate_bun_section};
    use crate::graph::build_module_graph;
    use crate::offsets::parse_offsets;
    use crate::testutil::{FIXTURES, dummy_asset, fixture};

    /// The modules region of a fixture: where it starts in the data, and its
    /// length.
    fn modules_region(data: &[u8]) -> (usize, usize) {
        let section = locate_bun_section(data).unwrap();
        let offsets = parse_offsets(section.data, section.data.len() - BUN_TRAILER.len()).unwrap();
        let graph = build_module_graph(&section, offsets).unwrap();
        (
            graph.modules_start,
            graph.metadata_start - graph.modules_start,
        )
    }

    fn try_extract(data: &[u8], options: ExtractOptions) -> Result<Vec<BundledFile>> {
        let section = locate_bun_section(data)?;
        let offsets = parse_offsets(section.data, section.data.len() - BUN_TRAILER.len())?;
        let graph = build_module_graph(&section, offsets)?;
        extract_modules(&graph, options)
    }

    fn extract(data: &[u8], options: ExtractOptions) -> Vec<BundledFile> {
        try_extract(data, options).unwrap()
    }

    /// Overwrite the u32 at `at`: a corrupted record field.
    fn put_u32(data: &mut [u8], at: usize, value: usize) {
        data[at..at + 4].copy_from_slice(&u32::try_from(value).unwrap().to_le_bytes());
    }

    #[test]
    fn extract_modules_on_every_fixture_recovers_the_dummy_bundle() {
        for name in FIXTURES {
            let data = fixture(name);
            let files = extract(&data, ExtractOptions::default());

            assert_eq!(files[0].path, "index.js", "{name}");
            for file in &files {
                assert!(!file.path.contains('/'), "{name}: {}", file.path);
            }

            // The entrypoint holds the dummy's code, not some other stretch
            // of the payload: on Bun 1.4.1 a read "right after the path"
            // returns the path table instead. Fixtures are built with
            // --sourcemap=inline (Bun 1.1.0 predates the serialized map).
            let entry = String::from_utf8_lossy(&files[0].contents);
            assert!(
                entry.contains("Access granted"),
                "{name}: entry is not the dummy"
            );
            if let Some(sourcemap) = &files[0].sourcemap {
                assert!(sourcemap.sources[0].ends_with("index.ts"), "{name}");
            }
            for file in &files {
                for root in [b"/$bunfs/".as_slice(), b"B:/~BUN/"] {
                    assert!(
                        !file.contents.starts_with(root),
                        "{name}: {} holds path bytes",
                        file.path,
                    );
                }
            }

            // Embedded assets must be byte-identical to the dummy sources.
            for (prefix, source) in [
                ("favicon", "favicon.png"),
                ("password2", "password2.bin"),
                ("fakeversion", "fakeversion.bin"),
            ] {
                let file = files
                    .iter()
                    .find(|f| f.path.starts_with(prefix))
                    .unwrap_or_else(|| panic!("{name}: no {prefix}* file"));
                assert_eq!(file.contents, dummy_asset(source), "{name}: {prefix}");
                assert_eq!(file.sourcemap, None, "{name}: {prefix}");
            }

            // Bun 1.4.1 also embeds the text-loaded password1 as its own
            // module; earlier versions only inline it into the entrypoint.
            let rest: Vec<&str> = files[1..]
                .iter()
                .map(|f| f.path.as_str())
                .filter(|p| {
                    !["favicon", "password2", "fakeversion"]
                        .iter()
                        .any(|a| p.starts_with(a))
                })
                .collect();
            if name.starts_with("v1.4.1") {
                assert_eq!(rest.len(), 1, "{name}: {rest:?}");
                assert!(rest[0].starts_with("password1"), "{name}: {rest:?}");
                let password1 = files.iter().find(|f| f.path == rest[0]).unwrap();
                assert_eq!(password1.contents, dummy_asset("password1.txt"), "{name}");
            } else {
                assert!(rest.is_empty(), "{name}: unexpected {rest:?}");
            }
        }
    }

    #[test]
    fn extract_modules_on_v1_4_1_fixture_reads_through_the_record_pointers() {
        // Bun 1.4.1 puts every path in one table after the contents, so the
        // pre-1.4.1 sequential read lands inside it. Pinned from the TS.
        let data = fixture("v1.4.1-darwin-arm64");
        let files = extract(&data, ExtractOptions::default());

        let summary: Vec<(&str, usize)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.contents.len()))
            .collect();
        assert_eq!(
            summary,
            [
                ("index.js", 2356),
                ("favicon-47vhjydn.png", 11253),
                ("password1-ntkv8amb.txt", 5),
                ("password2-twdsv7gz.bin", 6),
                ("fakeversion-a1s96rrt.bin", 52),
            ],
        );
        assert!(files[0].contents.starts_with(b"// @bun\n"));

        let sourcemap = files[0].sourcemap.as_ref().unwrap();
        assert_eq!(sourcemap.sources, ["src/lib/tests/dummy/index.ts"]);
        assert_eq!(
            sourcemap.debug_id.as_deref(),
            Some("46AEE77ECCECCE7B64756E2164756E21"),
        );
    }

    #[test]
    fn extract_modules_when_contents_pointer_overruns_modules_errors() {
        // Module 0's record is the first metadata chunk, which starts right
        // after the modules region; its contents pointer sits at +8.
        let mut data = fixture("v1.4.1-darwin-arm64");
        let (modules_start, modules_len) = modules_region(&data);
        put_u32(
            &mut data,
            modules_start + modules_len + 8,
            modules_len - 100,
        );

        let err = try_extract(&data, ExtractOptions::default()).unwrap_err();
        assert_eq!(
            err,
            ExtractError::InvalidExecutable(format!(
                "The contents of index.js ({}+2356) extends past the modules region ({modules_len})",
                modules_len - 100,
            )),
        );
    }

    #[test]
    fn extract_modules_when_sourcemap_pointer_overruns_modules_errors() {
        // The sourcemap pointer sits at +16 in the record.
        let mut data = fixture("v1.4.1-darwin-arm64");
        let (modules_start, modules_len) = modules_region(&data);
        put_u32(&mut data, modules_start + modules_len + 16, modules_len);

        let err = try_extract(&data, ExtractOptions::default()).unwrap_err();
        assert_eq!(
            err,
            ExtractError::InvalidExecutable(format!(
                "The sourcemap of index.js ({modules_len}+882) extends past the modules region ({modules_len})",
            )),
        );
    }

    #[test]
    fn extract_modules_when_v2_contents_length_overruns_modules_errors() {
        // V1/V2 records have no pointers; an inflated contents length
        // overruns the sequential walk and reports through the same check.
        // The contents length sits at +12 in the record.
        let mut data = fixture("v1.1.26-darwin-arm64");
        let (modules_start, modules_len) = modules_region(&data);
        put_u32(&mut data, modules_start + modules_len + 12, modules_len);

        let err = try_extract(&data, ExtractOptions::default()).unwrap_err();
        let ExtractError::InvalidExecutable(message) = err else {
            panic!("expected InvalidExecutable, got {err:?}");
        };
        assert!(
            message.starts_with("The contents of index.js (")
                && message.ends_with(&format!(
                    "+{modules_len}) extends past the modules region ({modules_len})"
                )),
            "{message}",
        );
    }

    // Exact values pinned from the TS implementation (see the oracle dump
    // scripts referenced in docs/rust-migration.md).

    #[test]
    fn extract_modules_on_v1_3_9_fixture_matches_ts_oracle() {
        let data = fixture("v1.3.9-darwin-arm64");
        let files = extract(&data, ExtractOptions::default());

        let summary: Vec<(&str, usize)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.contents.len()))
            .collect();
        assert_eq!(
            summary,
            [
                ("index.js", 1753),
                ("favicon-47vhjydn.png", 11253),
                ("password2-twdsv7gz.bin", 6),
                ("fakeversion-a1s96rrt.bin", 52),
            ],
        );

        let sourcemap = files[0].sourcemap.as_ref().unwrap();
        assert_eq!(sourcemap.version, 3);
        assert_eq!(sourcemap.file, "index.js");
        assert_eq!(sourcemap.mappings.len(), 345);
        assert_eq!(sourcemap.sources.len(), 1);
        assert_eq!(
            sourcemap.debug_id.as_deref(),
            Some("AD9B1BC5A0F03AAA64756E2164756E21"),
        );
        // v3 sourcemap `sources` entries are names, not content; this is the
        // dummy program's path, pinned from the TS oracle.
        assert_eq!(sourcemap.sources[0], "src/lib/tests/dummy/index.ts");
    }

    #[test]
    fn extract_modules_on_v1_fixture_matches_ts_oracle() {
        // V1 (Bun 1.1.0): 16-hex asset name suffixes and no sourcemaps.
        let data = fixture("v1.1.0-native");
        let files = extract(&data, ExtractOptions::default());

        let summary: Vec<(&str, usize)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.contents.len()))
            .collect();
        assert_eq!(
            summary,
            [
                ("index.js", 33554),
                ("favicon-550dfe5251fb8704.png", 11253),
                ("password2-ff9067db19addcda.bin", 6),
                ("fakeversion-da3838862959814a.bin", 52),
            ],
        );
        assert!(files.iter().all(|f| f.sourcemap.is_none()));
    }

    #[test]
    fn extract_modules_on_windows_fixture_strips_the_drive_root() {
        // Windows bundles use the B:/~BUN/root bunfs root.
        let data = fixture("v1.3.9-windows-x64");
        let files = extract(&data, ExtractOptions::default());
        assert_eq!(files[0].path, "index.js");
        assert_eq!(
            files[0].sourcemap.as_ref().unwrap().debug_id.as_deref(),
            Some("E65309CBA1E3DDB464756E2164756E21"),
        );
    }

    #[test]
    fn normalize_entrypoint_replaces_only_a_clean_final_segment() {
        assert_eq!(
            normalize_entrypoint("/$bunfs/root/cli.js"),
            "/$bunfs/root/index.js",
        );
        assert_eq!(
            normalize_entrypoint("B:/~BUN/root/app.js"),
            "B:/~BUN/root/index.js"
        );
        // No slash at all: unchanged.
        assert_eq!(normalize_entrypoint("cli.js"), "cli.js");
        // Backslash in the final segment: unchanged (TS [^\/\\]+).
        assert_eq!(normalize_entrypoint("/a\\b.js"), "/a\\b.js");
        // Trailing slash leaves an empty segment: unchanged.
        assert_eq!(normalize_entrypoint("/a/"), "/a/");
    }

    #[test]
    fn find_debug_id_accepts_only_a_full_hex_line() {
        // Pad so the file is longer than the 49-byte search window and the
        // debugId line sits fully inside it.
        let with = |line: &str| find_debug_id(format!("{}\n{line}", "x".repeat(48)).as_bytes());
        assert_eq!(
            with("//# debugId=AD9B1BC5A0F03AAA64756E2164756E21"),
            Some("AD9B1BC5A0F03AAA64756E2164756E21".to_owned()),
        );
        assert_eq!(
            with("//# debugId=aaaa-bbbb-cccc"),
            Some("aaaa-bbbb-cccc".to_owned())
        );
        assert_eq!(with("//# debugId=tooshort"), None);
        assert_eq!(with("//# debugId=NOT_HEX_AT_ALL!"), None);
        // A trailing \r blocks the JS regex's $, and must block ours.
        assert_eq!(with("//# debugId=AD9B1BC5A0F03AAA64756E2164756E21\r"), None);
    }

    #[test]
    fn find_debug_id_on_short_contents_keeps_js_window_semantics() {
        // JS contents.slice(len - 49) with len < 49 starts at 2*len - 49,
        // which can cut into the debugId line; the TS then finds nothing,
        // and neither must we.
        let short = b"code;\n//# debugId=aaaa-bbbb-cccc"; // 32 bytes
        assert_eq!(find_debug_id(short), None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn sourcemap_serializes_like_the_ts_json_stringify() {
        let sourcemap = SourceMap {
            version: 3,
            file: "index.js".to_owned(),
            debug_id: Some("AD9B1BC5A0F03AAA64756E2164756E21".to_owned()),
            mappings: "AAAA;BBBB".to_owned(),
            sources: vec!["src/index.ts".to_owned()],
        };
        assert_eq!(
            serde_json::to_string(&sourcemap).unwrap(),
            r#"{"version":3,"file":"index.js","debugId":"AD9B1BC5A0F03AAA64756E2164756E21","mappings":"AAAA;BBBB","sources":["src/index.ts"]}"#,
        );

        // An absent debugId is omitted, like JSON.stringify on undefined.
        let without = SourceMap {
            debug_id: None,
            ..sourcemap
        };
        assert_eq!(
            serde_json::to_string(&without).unwrap(),
            r#"{"version":3,"file":"index.js","mappings":"AAAA;BBBB","sources":["src/index.ts"]}"#,
        );
    }
}
