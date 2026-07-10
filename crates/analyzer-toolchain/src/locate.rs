//! Maps a compiler-reported `(line, col)` position to a byte [`TextRange`].
//!
//! `RawCompilerDiagnostic::col` (see [`crate::parse`]) is a 1-based
//! **Unicode-scalar** column, empirically re-confirmed against real `compact
//! 0.5.1` output (2026-07-10): a diagnostic whose
//! offending token is preceded by a multibyte char (`é`, 2 bytes / 1 scalar)
//! or an astral char (`😀`, 4 bytes / 2 UTF-16 units / 1 scalar) reports the
//! *scalar* column in both cases, never the byte column and never the
//! UTF-16 column. `analyzer_core::LineIndex::offset` must **not** be reused
//! here — it converts a UTF-16 column (the LSP convention), which diverges
//! from the compiler's scalar column for any astral character on the line.
//!
//! Converting scalar column to a byte offset therefore has to walk `char`s
//! (Rust `char` = Unicode scalar) from the line's start, not `u16` units.
//! `LineIndex` also has no public line-start/byte accessor (`line_starts` is
//! private), so the line start is found by a local scan here instead.
//!
//! Once the byte offset is found, the end of the range is snapped to
//! whichever CST token starts at (or contains) that offset, so callers get
//! a proper squiggle under the offending token rather than a zero-width
//! point — the source is parsed once per call for this purpose.

use analyzer_core::SyntaxNode;
use text_size::{TextRange, TextSize};

/// Maps a 1-based `line` + 1-based Unicode-scalar `col` (the `compact`
/// CLI's `--vscode` diagnostic convention) to a byte [`TextRange`] in
/// `source`, snapped to the CST token starting at that position.
///
/// Returns `None` when `line` or `col` is `0` (the compiler is 1-based; a
/// `0` can only arrive from a corrupted/adversarial caller) or when `line`
/// doesn't exist in `source`. A `col` past the end of its line clamps to
/// the byte offset at the line's content end (before its line terminator,
/// if any) rather than returning `None` — mirroring the clamp convention
/// `analyzer_core::LineIndex::offset` already uses for UTF-16 columns.
///
/// Never panics on any input.
pub fn locate(source: &str, line: u32, col: u32) -> Option<TextRange> {
    if line == 0 || col == 0 {
        return None;
    }

    let line_start = line_start_byte(source, line)?;
    let start = TextSize::new(start_byte_for_col(source, line_start, col) as u32);

    let parsed = compactp_parser::parse(source);
    let root = SyntaxNode::new_root(parsed.green);

    // M3 invariant (E7/B1): rowan's `token_at_offset` asserts `offset <=
    // root.text_range().end()` and PANICS otherwise. `start` is always
    // within bounds given how it's computed above (it never advances past
    // `source`'s own length), but clamp anyway — exactly as the merged
    // `anchor_token`/`ident_at_offset`/`selection_ranges` guards do — so
    // this function can never panic regardless of future changes to the
    // byte-offset computation above.
    let offset = start.min(root.text_range().end());

    // `token_at_offset` returns `TokenAtOffset::Between(l, r)` exactly when
    // `offset` sits on the shared boundary of two adjacent tokens, and in
    // that case `r` always starts at `offset` by construction — which is
    // exactly the token the compiler means to point at (it reports token
    // *starts*, per the `--vscode` fixtures). `right_biased()` picks that
    // `r` in the `Between` case and the sole token in the `Single` case, so
    // this is the same preference `anchor_token` applies for the same
    // reason. `None` (only possible for an empty CST) falls back to a
    // zero-width range at the clamped offset instead of dropping the
    // diagnostic.
    match root.token_at_offset(offset).right_biased() {
        Some(token) => Some(token.text_range()),
        None => Some(TextRange::empty(offset)),
    }
}

/// Byte offset of the start of `line` (1-based), or `None` if `source`
/// doesn't have that many lines.
///
/// Scans for the `(line - 1)`-th `\n` byte rather than reusing
/// `analyzer_core::LineIndex` (its `line_starts` table is private, and it's
/// keyed to a UTF-16 column convention this module must not use — see the
/// module doc comment).
fn line_start_byte(source: &str, line: u32) -> Option<usize> {
    if line == 1 {
        return Some(0);
    }
    let mut newlines_seen = 0u32;
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            newlines_seen += 1;
            if newlines_seen == line - 1 {
                return Some(i + 1);
            }
        }
    }
    None
}

/// Byte offset of the 1-based Unicode-scalar column `col`, within the line
/// starting at byte `line_start`.
///
/// Advances `col - 1` `char`s (Rust `char` = Unicode scalar, matching the
/// compiler's column convention) from `line_start`, stopping early — and
/// clamping to the current byte offset — at a `\n` or at the end of
/// `source`, whichever comes first. This never crosses into a following
/// line: a `col` past the line's actual content clamps to that line's
/// content end.
///
/// `col` must be `>= 1`; callers are expected to have already rejected
/// `col == 0` (see [`locate`]).
fn start_byte_for_col(source: &str, line_start: usize, col: u32) -> usize {
    let mut offset = line_start;
    let mut remaining = col - 1;
    for ch in source[line_start..].chars() {
        if remaining == 0 || ch == '\n' {
            break;
        }
        offset += ch.len_utf8();
        remaining -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::locate;
    use text_size::TextRange;

    #[test]
    fn ascii_baseline_lands_on_undefined_name_token() {
        let source = "circuit foo(): [] {\n  const x = undefined_name;\n}\n";
        let range = locate(source, 2, 13).expect("expected a range");
        let expected_start = source.find("undefined_name").unwrap() as u32;
        let expected_end = expected_start + "undefined_name".len() as u32;
        assert_eq!(
            range,
            TextRange::new(expected_start.into(), expected_end.into())
        );
    }

    #[test]
    fn multibyte_char_before_token_maps_to_byte_not_scalar_column() {
        // Real capture: `compact 0.5.1` reports `line 4 char 21` for this
        // exact source (verified 2026-07-10) — scalar column 21, even though
        // the preceding `é` makes the byte column 22.
        let source = "pragma language_version >= 0.16;\n\nexport circuit foo(): [] {\n  /* é */ const x = undefined_name;\n}\n";
        let range = locate(source, 4, 21).expect("expected a range");
        let expected_start = source.find("undefined_name").unwrap() as u32;
        let expected_end = expected_start + "undefined_name".len() as u32;
        assert_eq!(
            range,
            TextRange::new(expected_start.into(), expected_end.into())
        );
    }

    #[test]
    fn astral_char_before_token_maps_to_scalar_not_utf16_or_byte_column() {
        // Real capture: `compact 0.5.1` reports `line 4 char 21` for this
        // exact source (verified 2026-07-10) — scalar column 21. A
        // UTF-16-based mapping would compute column 22 (😀 is 2 UTF-16
        // units); a byte-based mapping would compute column 24 (😀 is 4
        // bytes). This is the test that would fail if `locate` wrongly used
        // either of those conventions instead of Unicode scalars.
        let source = "pragma language_version >= 0.16;\n\nexport circuit foo(): [] {\n  /* 😀 */ const x = undefined_name;\n}\n";
        let range = locate(source, 4, 21).expect("expected a range");
        let expected_start = source.find("undefined_name").unwrap() as u32;
        let expected_end = expected_start + "undefined_name".len() as u32;
        assert_eq!(
            range,
            TextRange::new(expected_start.into(), expected_end.into())
        );
    }

    #[test]
    fn out_of_bounds_line_is_none() {
        let source = "circuit foo(): [] {\n  const x = undefined_name;\n}\n";
        assert_eq!(locate(source, 999, 1), None);
    }

    #[test]
    fn col_past_end_of_line_clamps_to_line_content_end() {
        // Design choice: col past the line's content clamps to the byte
        // offset right at the line's content end (before its line
        // terminator), rather than returning `None`.
        let source = "circuit foo(): [] {\n  return 1;\n}\n";
        let line2 = source.lines().nth(1).unwrap();
        let line2_start = source.find(line2).unwrap();
        let expected_clamp = (line2_start + line2.len()) as u32;

        let range = locate(source, 2, 9999).expect("clamp choice returns Some");
        assert_eq!(range.start(), expected_clamp.into());
    }

    #[test]
    fn zero_line_or_zero_col_is_none_not_panic() {
        let source = "circuit foo(): [] {\n  const x = undefined_name;\n}\n";
        assert_eq!(locate(source, 0, 5), None);
        assert_eq!(locate(source, 5, 0), None);
        assert_eq!(locate(source, 0, 0), None);
    }

    #[test]
    fn empty_source_falls_back_to_zero_width_range_without_panic() {
        assert_eq!(locate("", 1, 1), Some(TextRange::empty(0.into())));
    }
}
