//! Parsing of `compact compile --skip-zk --vscode` stderr into structured
//! diagnostics.
//!
//! Under `--vscode`, a compile error collapses onto a single line:
//! `Exception: <basename> line <N> char <C>: <message>`. The compiler is
//! fail-fast (it stops at the first error), but this parser is best-effort
//! across every line so callers never have to special-case multi-error
//! output if a future compiler version emits more than one.
//!
//! Lines that don't match the grammar (e.g. a `Usage:` block from a CLI
//! invocation error) are preserved verbatim in [`ParsedStderr::unparsed`]
//! rather than dropped, so a caller can still surface *something* to the
//! user when the shape drifts from what this parser expects.

/// One compiler diagnostic as emitted under `--vscode`, before position
/// mapping to a byte range.
///
/// `line` and `col` are 1-based, exactly as the compiler prints them; `col`
/// is a Unicode-scalar column, not a byte offset (mapping to a byte
/// `TextRange` is [`crate::locate`]'s job, not this module's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawCompilerDiagnostic {
    /// Basename of the source file the error was attributed to — the
    /// compiler prints only the basename, even for errors inside an
    /// `include`d/imported dependency, never the full path.
    pub file_basename: String,
    /// 1-based source line.
    pub line: u32,
    /// 1-based Unicode-scalar column.
    pub col: u32,
    /// The error text following `char <C>: `.
    pub message: String,
}

/// The result of parsing a full stderr capture: every line that matched the
/// `--vscode` grammar, plus anything left over.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedStderr {
    /// Diagnostics parsed from lines matching the `Exception: ... line N
    /// char C: message` grammar, in the order they appeared.
    pub diagnostics: Vec<RawCompilerDiagnostic>,
    /// Non-blank lines that did not match the grammar, joined with `\n` in
    /// their original order. Non-empty ⇒ the caller should surface a
    /// generic diagnostic (the compiler said *something*, but this parser
    /// couldn't structure it).
    pub unparsed: String,
}

/// Parses `compact compile --vscode` stderr, best-effort.
///
/// Every non-blank line is tried independently against the grammar: a line
/// that matches becomes a [`RawCompilerDiagnostic`]; a line that doesn't
/// (or that matches the shape but has an unparseable line/column number) is
/// appended to [`ParsedStderr::unparsed`] instead. Blank lines are dropped
/// silently. Empty input yields a default (empty) `ParsedStderr`. Never
/// panics.
pub fn parse_compiler_stderr(stderr: &str) -> ParsedStderr {
    let mut diagnostics = Vec::new();
    let mut unparsed_lines: Vec<&str> = Vec::new();

    for line in stderr.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line) {
            Some(diagnostic) => diagnostics.push(diagnostic),
            None => unparsed_lines.push(line),
        }
    }

    ParsedStderr {
        diagnostics,
        unparsed: unparsed_lines.join("\n"),
    }
}

/// Tries to parse one line as `[Exception: ]<basename> line <N> char <C>:
/// <message>`.
///
/// Splits on the literal markers `" line "` and `" char "` rather than on
/// the first whitespace, so a basename containing spaces (not seen in the
/// wild, but not contractually ruled out either) wouldn't be truncated.
/// Returns `None` — never panics — for anything that doesn't fit: a missing
/// marker, an empty basename, or a line/column that doesn't parse as a
/// `u32` (negative, overflowing, or non-numeric).
fn parse_line(line: &str) -> Option<RawCompilerDiagnostic> {
    let rest = line.strip_prefix("Exception: ").unwrap_or(line);

    let (file_basename, after_basename) = rest.split_once(" line ")?;
    if file_basename.is_empty() {
        return None;
    }

    let (line_str, after_line) = after_basename.split_once(" char ")?;
    let line_no: u32 = line_str.parse().ok()?;

    // `char <C>` is immediately followed by `: <message>`; since <C> is
    // pure digits, the *first* `": "` in `after_line` can only be the
    // boundary right after it — no scan-ahead needed to avoid a false
    // match inside the message body.
    let colon_at = after_line.find(": ")?;
    let (col_str, after_col) = after_line.split_at(colon_at);
    let col_no: u32 = col_str.parse().ok()?;
    let message = &after_col[": ".len()..];

    Some(RawCompilerDiagnostic {
        file_basename: file_basename.to_string(),
        line: line_no,
        col: col_no,
        message: message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Real fixtures, captured 2026-07-10 against `compact 0.5.1`
    // (compiler 0.31.1, language 0.23.0) by compiling deliberately broken
    // `.compact` files with `compact compile --skip-zk --vscode <src>
    // <scratch>` and pasting the true stderr. See task-2-report.md for the
    // full capture transcript.

    #[test]
    fn parses_semantic_error_bad_ledger_adt_method() {
        // Real capture: `count.incremen(1)` on a `Counter` ledger field.
        let stderr = "Exception: bad.compact line 8 char 8: operation incremen undefined for ledger field type Counter\n";

        let parsed = parse_compiler_stderr(stderr);

        assert_eq!(
            parsed.diagnostics,
            vec![RawCompilerDiagnostic {
                file_basename: "bad.compact".to_string(),
                line: 8,
                col: 8,
                message: "operation incremen undefined for ledger field type Counter".to_string(),
            }]
        );
        assert_eq!(parsed.unparsed, "");
    }

    #[test]
    fn parses_parse_error_garbage_token() {
        // Real capture: a bare `zzz` token where a program element is expected.
        let stderr = "Exception: parse.compact line 3 char 1: parse error: found \"zzz\" looking for a program element or end of file\n";

        let parsed = parse_compiler_stderr(stderr);

        assert_eq!(parsed.diagnostics.len(), 1);
        let diag = &parsed.diagnostics[0];
        assert_eq!(diag.file_basename, "parse.compact");
        assert_eq!(diag.line, 3);
        assert_eq!(diag.col, 1);
        assert!(
            diag.message.starts_with("parse error:"),
            "message was: {:?}",
            diag.message
        );
        assert_eq!(parsed.unparsed, "");
    }

    #[test]
    fn parses_unbound_identifier_error() {
        // Real capture: reference to an identifier that doesn't exist.
        let stderr =
            "Exception: unbound.compact line 4 char 13: unbound identifier undefined_in_util\n";

        let parsed = parse_compiler_stderr(stderr);

        assert_eq!(
            parsed.diagnostics,
            vec![RawCompilerDiagnostic {
                file_basename: "unbound.compact".to_string(),
                line: 4,
                col: 13,
                message: "unbound identifier undefined_in_util".to_string(),
            }]
        );
    }

    #[test]
    fn parses_dependency_attributed_error_with_included_file_basename() {
        // Real capture: `main_with_include.compact` does `include "util";`
        // and calls `helper()`, which references an unbound identifier
        // inside `util.compact`. The compiler attributes the error to the
        // basename of the file that actually contains the bug, not the
        // file passed on the command line.
        let stderr =
            "Exception: util.compact line 4 char 13: unbound identifier undefined_in_util\n";

        let parsed = parse_compiler_stderr(stderr);

        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].file_basename, "util.compact");
    }

    #[test]
    fn non_matching_usage_block_yields_no_diagnostics_and_nonempty_unparsed() {
        // Real capture: `compact compile` invoked with no source/target
        // arguments (a CLI invocation error, exit code 1 — not a compile
        // error, but stderr shape this parser must not choke on).
        let stderr = "Usage: compactc.bin <flag> ... <source-pathname> <target-directory-pathname>\n       --help displays detailed usage information\n";

        let parsed = parse_compiler_stderr(stderr);

        assert!(parsed.diagnostics.is_empty());
        assert!(!parsed.unparsed.is_empty());
        assert!(parsed.unparsed.contains("Usage:"));
    }

    #[test]
    fn empty_stderr_yields_default_parsed_stderr() {
        let parsed = parse_compiler_stderr("");

        assert_eq!(parsed, ParsedStderr::default());
        assert!(parsed.diagnostics.is_empty());
        assert_eq!(parsed.unparsed, "");
    }

    #[test]
    fn blank_only_stderr_yields_default_parsed_stderr() {
        let parsed = parse_compiler_stderr("\n\n   \n");

        assert_eq!(parsed, ParsedStderr::default());
    }

    #[test]
    fn missing_exception_prefix_still_parses() {
        // The `Exception: ` prefix is stripped when present but is not
        // required for a line to match — defensive against a future
        // compiler version dropping it.
        let stderr =
            "bad.compact line 8 char 8: operation incremen undefined for ledger field type Counter";

        let parsed = parse_compiler_stderr(stderr);

        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].file_basename, "bad.compact");
    }

    #[test]
    fn non_numeric_line_number_is_unparsed_not_panicking() {
        let stderr = "Exception: bad.compact line four char 8: message\n";

        let parsed = parse_compiler_stderr(stderr);

        assert!(parsed.diagnostics.is_empty());
        assert!(parsed.unparsed.contains("line four char 8"));
    }

    #[test]
    fn non_numeric_col_number_is_unparsed_not_panicking() {
        let stderr = "Exception: bad.compact line 8 char eight: message\n";

        let parsed = parse_compiler_stderr(stderr);

        assert!(parsed.diagnostics.is_empty());
        assert!(parsed.unparsed.contains("char eight"));
    }

    #[test]
    fn negative_column_number_is_unparsed_not_panicking() {
        // u32 rejects a leading `-`; must not panic on the parse.
        let stderr = "Exception: bad.compact line 8 char -1: message\n";

        let parsed = parse_compiler_stderr(stderr);

        assert!(parsed.diagnostics.is_empty());
        assert!(!parsed.unparsed.is_empty());
    }

    #[test]
    fn overflowing_line_number_is_unparsed_not_panicking() {
        let stderr = "Exception: bad.compact line 99999999999999999999 char 8: message\n";

        let parsed = parse_compiler_stderr(stderr);

        assert!(parsed.diagnostics.is_empty());
        assert!(!parsed.unparsed.is_empty());
    }

    #[test]
    fn missing_char_marker_is_unparsed_not_panicking() {
        let stderr = "Exception: bad.compact line 8: message with no char marker\n";

        let parsed = parse_compiler_stderr(stderr);

        assert!(parsed.diagnostics.is_empty());
        assert!(!parsed.unparsed.is_empty());
    }

    #[test]
    fn missing_line_marker_is_unparsed_not_panicking() {
        let stderr = "Exception: this is not the expected grammar at all\n";

        let parsed = parse_compiler_stderr(stderr);

        assert!(parsed.diagnostics.is_empty());
        assert!(!parsed.unparsed.is_empty());
    }

    #[test]
    fn best_effort_parses_every_matching_line_even_though_compiler_is_fail_fast() {
        // The real compiler only ever emits one `Exception:` line, but the
        // parser is defensively best-effort across all lines in case a
        // future version batches multiple errors.
        let stderr = "Exception: a.compact line 1 char 1: first error\nException: b.compact line 2 char 3: second error\n";

        let parsed = parse_compiler_stderr(stderr);

        assert_eq!(parsed.diagnostics.len(), 2);
        assert_eq!(parsed.diagnostics[0].file_basename, "a.compact");
        assert_eq!(parsed.diagnostics[1].file_basename, "b.compact");
        assert_eq!(parsed.unparsed, "");
    }

    #[test]
    fn mixed_matching_and_junk_lines_split_correctly() {
        let stderr = "some preamble noise\nException: real.compact line 5 char 2: the actual error\ntrailing noise\n";

        let parsed = parse_compiler_stderr(stderr);

        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].file_basename, "real.compact");
        assert_eq!(parsed.unparsed, "some preamble noise\ntrailing noise");
    }

    #[test]
    fn message_containing_colon_space_is_not_truncated() {
        // Regression guard for the "first `: ` after `char <C>`" rule: the
        // message itself contains `: ` (as in the real parse-error
        // fixture), and it must not be cut short.
        let stderr = "Exception: x.compact line 1 char 1: note: something: nested colons here\n";

        let parsed = parse_compiler_stderr(stderr);

        assert_eq!(
            parsed.diagnostics[0].message,
            "note: something: nested colons here"
        );
    }

    #[test]
    fn empty_basename_is_unparsed_not_panicking() {
        // Exercises the `if file_basename.is_empty() { return None; }` guard
        // directly: a leading space before ` line ` makes the basename an
        // empty string once split on the marker. Without the guard this
        // would produce a nonsensical diagnostic with an empty basename;
        // with it, the whole line falls through to `unparsed`. No panic.
        let stderr = " line 1 char 1: message";

        let parsed = parse_compiler_stderr(stderr);

        assert!(parsed.diagnostics.is_empty());
        assert_eq!(parsed.unparsed, " line 1 char 1: message");
    }

    #[test]
    fn basename_containing_spaces_is_not_truncated() {
        // The brief's VERIFY item: prove the ` line `/` char ` marker split
        // (not a first-whitespace split) so a basename with an interior
        // space survives intact. This is a REAL capture: copying a broken
        // source to `my file.compact` and compiling it with `compact 0.5.1`
        // yields exactly this stderr verbatim (see task-2-report.md) — the
        // compiler does emit spaced basenames.
        let stderr =
            "Exception: my file.compact line 4 char 13: unbound identifier undefined_in_util\n";

        let parsed = parse_compiler_stderr(stderr);

        assert_eq!(
            parsed.diagnostics,
            vec![RawCompilerDiagnostic {
                file_basename: "my file.compact".to_string(),
                line: 4,
                col: 13,
                message: "unbound identifier undefined_in_util".to_string(),
            }]
        );
        assert_eq!(parsed.unparsed, "");
    }
}
