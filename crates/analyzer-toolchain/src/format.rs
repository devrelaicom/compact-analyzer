//! Formats a Compact source buffer via `compact format`, round-tripped
//! through a private temp file so no real project file is ever touched.
//!
//! `compact format`'s flag surface (`compact help format`, real `compact
//! 0.5.1`, 2026-07-10) is `--check`, `--directory`, `--verbose`, `--version`,
//! `--language-version`, `--help` — there is **no `--stdout` flag**, so a
//! temp-file round-trip is the only way to format an in-memory buffer.
//! `--check` was ruled out too: it only *reports* whether a file is
//! formatted (exit `1` if not) without ever writing, so it can't produce the
//! reformatted text this function needs.
//!
//! Empirically verified against real `compact 0.5.1` (2026-07-10): given a
//! syntactically valid but unformatted file, `compact format <path>` exits
//! `0`, is silent on both stdout and stderr, and rewrites `<path>` in place
//! (confirmed idempotent: formatting the already-formatted output again
//! leaves it byte-for-byte unchanged). Given a syntactically broken file, it
//! exits `1`, leaves the file untouched, and writes `"<path>: failed\nError:
//! formatting failed"` to stderr (stdout stays empty). This matches the
//! plan exactly — no plan correction was needed.

use std::process::Command;
use std::time::Duration;

use crate::discovery::Toolchain;
use crate::process::{ProcessResult, run_with_timeout};

/// Formats `text` by writing it to a private temp file, running `<tc.compact_bin>
/// format <temp>`, and reading the temp file back on success.
///
/// Returns `None` — never panics, never surfaces an error — when the
/// toolchain rejects the input (a syntax error, reported as a non-zero
/// exit), the invocation times out, or any of the temp-file/process
/// plumbing itself fails (temp dir/file creation, write, or read-back).
/// Formatting is a nice-to-have editor feature, not a correctness-critical
/// path, so every failure mode silently degrades to "no formatted text
/// available" rather than erroring.
///
/// A fresh, uniquely-named temp *directory* (via [`tempfile::Builder`]) is
/// created for every call and auto-cleaned on drop; the caller's own files
/// are never touched, and `text` is never written anywhere else.
pub fn format_source(tc: &Toolchain, text: &str, timeout: Duration) -> Option<String> {
    let dir = tempfile::Builder::new()
        .prefix("compact-analyzer-fmt-")
        .tempdir()
        .ok()?;
    let path = dir.path().join("buffer.compact");
    std::fs::write(&path, text).ok()?;

    let mut command = Command::new(&tc.compact_bin);
    command.arg("format").arg(&path);

    match run_with_timeout(command, timeout) {
        ProcessResult::Exited { status, .. } if status.success() => {
            std::fs::read_to_string(&path).ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNFORMATTED: &str = "pragma language_version >= 0.16;\n\n\
        export circuit foo(   ): Field {\n  return    1;\n}\n";

    const BROKEN: &str = "pragma language_version >= 0.16;\n\n\
        export circuit foo(: Field {\n  return 1;\n}\n";

    #[test]
    fn formats_valid_unformatted_source_and_is_idempotent() {
        let Some(tc) = Toolchain::discover(None) else {
            eprintln!("compact not present; skipping");
            return;
        };

        let formatted = format_source(&tc, UNFORMATTED, Duration::from_secs(30))
            .expect("expected Some(formatted) for valid-but-unformatted source");

        assert_ne!(
            formatted, UNFORMATTED,
            "formatting an unformatted-but-valid file should change it"
        );
        assert!(
            formatted.contains("export circuit foo(): Field {"),
            "formatted output was: {formatted:?}"
        );

        let reformatted = format_source(&tc, &formatted, Duration::from_secs(30))
            .expect("expected Some(formatted) when reformatting already-formatted source");
        assert_eq!(
            reformatted, formatted,
            "formatting already-formatted source should be idempotent"
        );
    }

    #[test]
    fn returns_none_for_syntactically_broken_source() {
        let Some(tc) = Toolchain::discover(None) else {
            eprintln!("compact not present; skipping");
            return;
        };

        assert_eq!(format_source(&tc, BROKEN, Duration::from_secs(30)), None);
    }
}
