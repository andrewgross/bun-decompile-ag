//! Bunfs path handling: every bundled file path starts with the virtual
//! filesystem root Bun mounted it under, which varies by version and OS.
//!
//! Port of the path helpers in `src/lib/modules.ts`.

#![allow(dead_code)] // Consumers land over the next commits of the migration.

use crate::error::{ExtractError, Result};

/// Modern bunfs root (macOS/Linux).
pub(crate) const BUNFS_ROOT: &str = "/$bunfs/root";

/// Legacy bunfs root (old Bun versions).
pub(crate) const BUNFS_ROOT_OLD: &str = "compiled://root";

/// Strip the bunfs root from a bundled path, whichever form it uses.
/// Errors when the path starts with none of them (same message as the TS
/// bare `Error`).
pub(crate) fn remove_bunfs_root_from_path(path: &str) -> Result<&str> {
    if let Some(rest) = path.strip_prefix(BUNFS_ROOT) {
        return Ok(rest);
    }
    if let Some(rest) = path.strip_prefix(BUNFS_ROOT_OLD) {
        return Ok(rest);
    }
    if let Some(len) = windows_root_len(path) {
        return Ok(&path[len..]);
    }
    Err(ExtractError::InvalidBunfsPath(path.to_owned()))
}

/// Windows mounts the virtual FS on a drive, e.g. `B:/~BUN/root` (the drive
/// letter varies), with forward or back slashes. Returns the prefix length
/// when the path starts that way. Spec: the TS regex
/// `^[A-Za-z]:[/\\]~BUN[/\\]root`.
fn windows_root_len(path: &str) -> Option<usize> {
    let b = path.as_bytes();
    if b.len() < 12 {
        return None;
    }
    let sep = |c: u8| c == b'/' || c == b'\\';
    (b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && sep(b[2])
        && &b[3..7] == b"~BUN"
        && sep(b[7])
        && &b[8..12] == b"root")
        .then_some(12)
}

/// Strip one leading `/` and then one leading `./`, if present. Spec: the TS
/// regex `^\/?(?:\.\/)?`. Exported by the TS package as a public helper.
pub(crate) fn remove_leading_slash(path: &str) -> &str {
    let path = path.strip_prefix('/').unwrap_or(path);
    path.strip_prefix("./").unwrap_or(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn remove_bunfs_root_from_path_strips_unix_legacy_and_windows_roots() {
        assert_eq!(
            remove_bunfs_root_from_path("/$bunfs/root/index.js").unwrap(),
            "/index.js",
        );
        assert_eq!(
            remove_bunfs_root_from_path("compiled://root/a.js").unwrap(),
            "/a.js",
        );
        assert_eq!(
            remove_bunfs_root_from_path("B:/~BUN/root/index.js").unwrap(),
            "/index.js",
        );
        assert_eq!(
            remove_bunfs_root_from_path("Z:\\~BUN\\root\\x.js").unwrap(),
            "\\x.js",
        );
    }

    #[test]
    fn remove_bunfs_root_from_path_with_unknown_root_errors() {
        assert_eq!(
            remove_bunfs_root_from_path("/nope/index.js"),
            Err(ExtractError::InvalidBunfsPath("/nope/index.js".to_owned())),
        );
    }

    #[test]
    fn remove_leading_slash_strips_slash_then_dot_slash() {
        assert_eq!(remove_leading_slash("/index.js"), "index.js");
        assert_eq!(remove_leading_slash("/./index.js"), "index.js");
        assert_eq!(remove_leading_slash("./index.js"), "index.js");
        assert_eq!(remove_leading_slash("index.js"), "index.js");
        assert_eq!(remove_leading_slash(""), "");
    }
}
