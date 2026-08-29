//! Read the Bun version string embedded in a compiled executable.
//!
//! Port of the version half of `src/lib/index.ts`. Three embeddings are
//! known, tried newest-marker first; a marker hit that does not parse as a
//! version is skipped and the scan continues ("Bun v" also appears in help
//! text, and Bun 1.4.0+ leaves an unsubstituted placeholder after the ANSI
//! marker). The search is bounded to the bytes before the module data —
//! the section offset when the container has one, else a legacy heuristic —
//! so fake version strings inside bundled files can never match.
//!
//! Version detection is informational only: extraction never depends on it.

use crate::bytes::{find_bytes, read_u32_le};
use crate::container::find_bun_section_offset;
use crate::error::{ExtractError, Result};

/// The Bun version an executable was compiled with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BunVersion {
    /// Semantic version, e.g. `1.3.9` or `1.1.22-canary.96`.
    pub version: String,
    /// Build revision (a short commit hash), possibly empty.
    pub revision: String,
}

/// Bun 1.2–1.3.x: ANSI-styled "bun build v" followed by the literal version.
const MARKER_ANSI: &[u8] = b"\x1b[0m\x1b[1mbun build \x1b[0m\x1b[2mv";

/// Bun <= 1.1.25: a "bun meta" block naming the version.
const MARKER_OLD: &[u8] = b"----- bun meta -----\nBun v";

/// Bun 1.4.0+ substitutes the ANSI marker's literal at runtime, so it is
/// never in the file; the plain banner in the runtime's rodata still is.
const MARKER_BANNER: &[u8] = b"Bun v";

/// How long a window to read when nothing delimits the version text.
const BANNER_WINDOW: usize = 96;

const ESC: u8 = 0x1B;

/// One way a Bun version is embedded: the literal preceding it, what ends
/// the version text (`None` reads a fixed window), and how to parse it.
struct VersionMarker {
    marker: &'static [u8],
    terminator: Option<u8>,
    parse: fn(&str) -> Option<BunVersion>,
}

/// Every known embedding, oldest format last; the first that yields a
/// parseable version wins.
const VERSION_MARKERS: [VersionMarker; 3] = [
    VersionMarker {
        marker: MARKER_ANSI,
        terminator: Some(ESC),
        parse: parse_version_revision_anchored,
    },
    VersionMarker {
        marker: MARKER_OLD,
        terminator: Some(b':'),
        parse: parse_version_revision,
    },
    VersionMarker {
        marker: MARKER_BANNER,
        terminator: None,
        parse: parse_banner,
    },
];

/// Pull `<version> (<revision>)` spanning the whole text. Spec: the TS
/// `/^(.+) \((.+)\)$/` — both groups greedy, so the last ` (` wins.
fn parse_version_revision_anchored(text: &str) -> Option<BunVersion> {
    let text = text.strip_suffix(')')?;
    let (version, revision) = text.rsplit_once(" (")?;
    (!version.is_empty() && !revision.is_empty()).then(|| BunVersion {
        version: version.to_owned(),
        revision: revision.to_owned(),
    })
}

/// Pull `<version> (<revision>)` from the start of the text, trailing junk
/// allowed. Spec: the TS `/^(.+) \((.+)\)/` — greedy, so the revision runs
/// to the last `)` and the version to the last ` (` before it.
fn parse_version_revision(text: &str) -> Option<BunVersion> {
    let close = text.rfind(')')?;
    let mut search_end = close;
    loop {
        let open = text[..search_end].rfind(" (")?;
        if open > 0 && close > open + 2 {
            return Some(BunVersion {
                version: text[..open].to_owned(),
                revision: text[open + 2..close].to_owned(),
            });
        }
        if open == 0 {
            return None;
        }
        search_end = open;
    }
}

/// Bun 1.4.0+ banner: `1.4.0`, `1.4.0+63bb0ca0d`, or `1.1.22-canary.96`,
/// each optionally followed by ` (revision)`. The revision is the `+<sha>`
/// build metadata when present, else the parenthetical. Spec: the TS
/// `/^(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)(?:\+([0-9A-Za-z]+))?(?:\s*\(([^)]+)\))?/`.
fn parse_banner(text: &str) -> Option<BunVersion> {
    fn digits(bytes: &[u8], pos: &mut usize) -> bool {
        let start = *pos;
        while bytes.get(*pos).is_some_and(u8::is_ascii_digit) {
            *pos += 1;
        }
        *pos > start
    }

    let bytes = text.as_bytes();
    let mut pos = 0;

    if !digits(bytes, &mut pos) {
        return None;
    }
    for _ in 0..2 {
        if bytes.get(pos) != Some(&b'.') {
            return None;
        }
        pos += 1;
        if !digits(bytes, &mut pos) {
            return None;
        }
    }

    // Optional prerelease: -[0-9A-Za-z.]+ (an unmatched bare '-' stays
    // unconsumed, like the regex's optional group).
    if bytes.get(pos) == Some(&b'-') {
        let mut p = pos + 1;
        while bytes
            .get(p)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'.')
        {
            p += 1;
        }
        if p > pos + 1 {
            pos = p;
        }
    }
    let version = text[..pos].to_owned();

    // Optional build metadata: +[0-9A-Za-z]+
    let mut build = None;
    if bytes.get(pos) == Some(&b'+') {
        let start = pos + 1;
        let mut p = start;
        while bytes.get(p).is_some_and(u8::is_ascii_alphanumeric) {
            p += 1;
        }
        if p > start {
            build = Some(text[start..p].to_owned());
            pos = p;
        }
    }

    // Optional parenthetical: \s*\(([^)]+)\)
    let mut paren = None;
    let mut p = pos;
    while bytes
        .get(p)
        .is_some_and(|c| c.is_ascii_whitespace() || *c == 0x0B)
    {
        p += 1;
    }
    if bytes.get(p) == Some(&b'(') {
        let start = p + 1;
        let mut q = start;
        while bytes.get(q).is_some_and(|c| *c != b')') {
            q += 1;
        }
        if bytes.get(q) == Some(&b')') && q > start {
            paren = Some(text[start..q].to_owned());
        }
    }

    Some(BunVersion {
        version,
        revision: build.or(paren).unwrap_or_default(),
    })
}

/// Find the first `marker` starting in `[0, search_limit)` whose following
/// text parses as a version. A hit that does not parse is skipped and the
/// scan continues from just past the marker.
fn scan_for_version(
    data: &[u8],
    search_limit: usize,
    marker: &VersionMarker,
) -> Option<BunVersion> {
    let mut from = 0;

    loop {
        let index = find_bytes(data, marker.marker, from, search_limit)?;
        let start = index + marker.marker.len();

        // The terminator search is not bounded by the limit, matching TS.
        let end = match marker.terminator {
            None => Some((start + BANNER_WINDOW).min(data.len())),
            Some(terminator) => data[start..]
                .iter()
                .position(|&b| b == terminator)
                .map(|p| start + p),
        };

        if let Some(end) = end {
            if end > start {
                let text = String::from_utf8_lossy(&data[start..end]);
                if let Some(version) = (marker.parse)(&text) {
                    return Some(version);
                }
            }
        }

        from = start;
    }
}

/// Search bound for appended-format binaries: the start of the module data,
/// computed from the trailing Offsets byte count. Keeps embedded module
/// bytes out of the version scan.
fn modules_start_legacy(data: &[u8]) -> usize {
    if data.len() <= 48 {
        return data.len();
    }
    let Ok(count) = read_u32_le(data, data.len() - 48) else {
        return data.len();
    };
    data.len().saturating_sub(count as usize + 48)
}

/// Read the Bun version the executable was compiled with.
///
/// Informational only — extraction never depends on it. Errors with
/// [`ExtractError::VersionNotFound`] when no marker parses.
pub(crate) fn get_executable_version(data: &[u8]) -> Result<BunVersion> {
    let search_limit = match find_bun_section_offset(data)? {
        Some(offset) => offset,
        None => modules_start_legacy(data),
    };

    for marker in &VERSION_MARKERS {
        if let Some(version) = scan_for_version(data, search_limit, marker) {
            return Ok(version);
        }
    }

    Err(ExtractError::VersionNotFound)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn buffer_with(parts: &[(usize, &[u8])]) -> Vec<u8> {
        let mut buf = vec![0_u8; 512];
        for (at, bytes) in parts {
            buf[*at..*at + bytes.len()].copy_from_slice(bytes);
        }
        buf
    }

    fn version_of(buf: &[u8]) -> BunVersion {
        get_executable_version(buf).unwrap()
    }

    #[test]
    fn get_executable_version_reads_the_ansi_build_marker() {
        // Bun 1.2-1.3.x: literal version after the styled "bun build v",
        // terminated by the next escape byte.
        let buf = buffer_with(&[(
            64,
            b"\x1b[0m\x1b[1mbun build \x1b[0m\x1b[2mv1.2.4 (bd9a68e6)\x1b[0m".as_slice(),
        )]);
        let version = version_of(&buf);
        assert_eq!(version.version, "1.2.4");
        assert_eq!(version.revision, "bd9a68e6");
    }

    #[test]
    fn get_executable_version_reads_the_old_bun_meta_block() {
        let buf = buffer_with(&[(
            64,
            b"----- bun meta -----\nBun v1.1.0 (5903a614):".as_slice(),
        )]);
        let version = version_of(&buf);
        assert_eq!(version.version, "1.1.0");
        assert_eq!(version.revision, "5903a614");
    }

    #[test]
    fn get_executable_version_reads_the_banner_only_format() {
        // Bun 1.4.0+: only the plain banner remains in the file.
        let buf = buffer_with(&[(64, b"Bun v1.4.0 (63bb0ca0d)".as_slice())]);
        let version = version_of(&buf);
        assert_eq!(version.version, "1.4.0");
        assert_eq!(version.revision, "63bb0ca0d");
    }

    #[test]
    fn get_executable_version_skips_a_marker_hit_that_is_not_a_version() {
        let buf = buffer_with(&[
            (32, b"Bun v<version> is not a version".as_slice()),
            (128, b"Bun v1.4.0 (63bb0ca0d)".as_slice()),
        ]);
        assert_eq!(version_of(&buf).version, "1.4.0");
    }

    #[test]
    fn get_executable_version_ignores_matches_past_the_search_limit() {
        // Trailing offsetByteCount 400 puts the legacy modules start at
        // 512 - 448 = 64; a banner inside the "module data" must not match.
        let mut buf = buffer_with(&[(100, b"Bun v9.9.9 (deadbeef)".as_slice())]);
        buf[512 - 48..512 - 44].copy_from_slice(&400_u32.to_le_bytes());
        assert_eq!(
            get_executable_version(&buf),
            Err(ExtractError::VersionNotFound),
        );

        // The same banner before the limit is found.
        let mut buf = buffer_with(&[(20, b"Bun v9.9.9 (deadbeef)".as_slice())]);
        buf[512 - 48..512 - 44].copy_from_slice(&400_u32.to_le_bytes());
        assert_eq!(version_of(&buf).version, "9.9.9");
    }

    #[test]
    fn get_executable_version_on_a_non_executable_errors() {
        assert_eq!(
            get_executable_version(&[0xDE, 0xAD, 0xBE, 0xEF]),
            Err(ExtractError::VersionNotFound),
        );
    }

    #[test]
    fn parse_banner_handles_plain_build_and_prerelease_forms() {
        let v = |t: &str| parse_banner(t).unwrap();
        assert_eq!(v("1.4.0").version, "1.4.0");
        assert_eq!(v("1.4.0").revision, "");
        assert_eq!(v("1.4.0+63bb0ca0d").revision, "63bb0ca0d");
        assert_eq!(v("1.1.22-canary.96").version, "1.1.22-canary.96");
        // Build metadata wins over the parenthetical.
        let both = v("1.4.0+aaa (bbb)");
        assert_eq!(both.revision, "aaa");
        // A bare '-' is not a prerelease; the version stops before it.
        assert_eq!(v("1.4.0- (rev)").version, "1.4.0");
        assert_eq!(parse_banner("x1.4.0"), None);
        assert_eq!(parse_banner("1.4"), None);
    }

    #[test]
    fn parse_version_revision_is_greedy_like_the_ts_regex() {
        // Unanchored form: revision runs to the last ')', version to the
        // last ' (' before it, trailing junk ignored.
        let v = parse_version_revision("a (b) c (d) e").unwrap();
        assert_eq!((v.version.as_str(), v.revision.as_str()), ("a (b) c", "d"));

        // Anchored form (ANSI marker): the whole text must be consumed.
        let v = parse_version_revision_anchored("1.2.4 (abc)").unwrap();
        assert_eq!((v.version.as_str(), v.revision.as_str()), ("1.2.4", "abc"));
        assert_eq!(parse_version_revision_anchored("1.2.4 (abc) x"), None);
    }

    // The compiled dummy binary embeds a fake "----- bun meta -----" block
    // inside a bundled module specifically to fool version detection; the
    // section-offset search limit is what keeps it from matching. Skip when
    // the dummy is not built.
    #[test]
    fn get_executable_version_on_real_dummy_binary_is_not_fooled() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../src/lib/tests/dummy/dummy"
        );
        let Ok(binary) = std::fs::read(path) else {
            eprintln!("skipping: dummy binary not built");
            return;
        };
        let version = get_executable_version(&binary).unwrap();
        assert_ne!(version.version, "fakeversion1");
        assert!(
            version.version.as_bytes()[0].is_ascii_digit(),
            "unexpected version: {version:?}",
        );
    }
}
