//! `textDocument/codeAction`: the `disclose()` minimal-scope quick-fix
//! (v3c C3).
//!
//! SECURITY-SENSITIVE. The quick-fix wraps EXACTLY the confirmed leak's
//! `primary_span` — the sink's own minimal tainted expression — in
//! `disclose(...)`. It never expands to the enclosing statement or a
//! broader parent expression: a wider wrap would clear taint for MORE than
//! the flagged sink and could silently sanitize a second, unrelated leak
//! sharing the same statement. Differential-verified against `compactc`
//! 0.31.1 (see the task report): `c = disclose(getW());` compiles clean;
//! `disclose(c = getW());` (a wrap of the whole statement) is REJECTED —
//! the compiler re-locates the same undisclosed value at the inner RHS.

use analyzer_core::{Diagnostic, LineIndex, TextRange};
use lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, TextEdit, Url, WorkspaceEdit};
use std::collections::HashMap;

use crate::lsp_utils;

/// A confirmed disclosure leak (`E3100`+; never a `U`-family fail-closed
/// advisory — an advisory isn't a proven leak, so there's nothing to fix).
fn is_disclosure_leak(diag: &Diagnostic) -> bool {
    diag.code.prefix == "E" && diag.code.number >= 3100
}

/// Whether `diag` has a single, wrappable tainted expression at its
/// `primary_span`. The disclosure interpreter (`analyzer_core::disclosure`)
/// records two DISTINCT leak channels per sink (spec §3, K1-K7): a DATA leak
/// (the value written/returned — nature `"ledger operation"` or `"the value
/// returned from exported circuit …"`), whose `primary_span` is the minimal
/// tainted expression, and a CONTROL-flow leak for the SAME sink gated on a
/// witness (nature `"performing this ledger operation"` / `"returning this
/// value from exported circuit …"`), whose `primary_span` is the whole
/// statement — because the gating witness lives in a branch condition
/// elsewhere, not in any expression at that span. Wrapping a control leak's
/// span in `disclose()` would not satisfy the compiler's actual disclosure
/// requirement (the condition, not the sink, needs disclosing) and — being
/// a whole-statement span — is exactly the over-disclosure shape this
/// feature must never emit. There is no structured "kind" on `Diagnostic`
/// to switch on, so this matches the two control-leak natures embedded in
/// `leak_to_diagnostic`'s message text; every other `E3100`+ diagnostic is a
/// DATA leak and gets the quick-fix.
fn is_wrappable(diag: &Diagnostic) -> bool {
    is_disclosure_leak(diag)
        && !diag.message.contains("performing this ledger operation")
        && !diag
            .message
            .contains("returning this value from exported circuit")
}

/// True when byte ranges `a` and `b` share at least one position — used to
/// decide whether a leak is "at" the code-action request's range. Two
/// ranges that merely touch (`a.end() == b.start()`) do not count.
fn ranges_overlap(a: TextRange, b: TextRange) -> bool {
    a.start() < b.end() && b.start() < a.end()
}

/// Builds the "Reveal this value with disclose()" quick-fix for one
/// confirmed leak: two zero-width insertions — `disclose(` at
/// `diag.primary_span`'s start, `)` at its end — rather than a single
/// replace. Two insertions make the touched range provably exactly
/// `diag.primary_span`: neither edit's position is ever computed from
/// anything else (the enclosing statement, a parent expression), so the
/// span this quick-fix touches can never silently drift wider than the
/// sink it was built from.
fn disclose_quickfix(diag: &Diagnostic, li: &LineIndex, uri: &Url) -> CodeAction {
    let range = lsp_utils::range_to_lsp(li, diag.primary_span);
    let open_paren = TextEdit {
        range: lsp_types::Range::new(range.start, range.start),
        new_text: "disclose(".to_string(),
    };
    let close_paren = TextEdit {
        range: lsp_types::Range::new(range.end, range.end),
        new_text: ")".to_string(),
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![open_paren, close_paren]);
    CodeAction {
        title: "Reveal this value with disclose()".to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![lsp_utils::diagnostic_to_lsp(diag, li, uri)]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        is_preferred: Some(false),
        ..Default::default()
    }
}

/// Filters `leaks` (the file's full `disclosure_diagnostics` set, U-family
/// advisories included) to the confirmed, wrappable leaks overlapping
/// `requested_range`, and builds one `disclose()` quick-fix per match. Not
/// fix-all: each sink gets its own explicit, independent action.
pub(crate) fn disclosure_quickfixes(
    leaks: &[Diagnostic],
    requested_range: TextRange,
    li: &LineIndex,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    leaks
        .iter()
        .filter(|d| is_wrappable(d))
        .filter(|d| ranges_overlap(d.primary_span, requested_range))
        .map(|d| CodeActionOrCommand::CodeAction(disclose_quickfix(d, li, uri)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{DiagnosticCode, TextSize};
    use std::sync::Arc;

    fn leak_at(start: u32, end: u32) -> Diagnostic {
        Diagnostic::error(
            DiagnosticCode::new("E", 3100),
            "potential witness-value disclosure: ledger operation".to_string(),
            TextRange::new(TextSize::new(start), TextSize::new(end)),
        )
    }

    fn control_leak_at(start: u32, end: u32) -> Diagnostic {
        Diagnostic::error(
            DiagnosticCode::new("E", 3100),
            "potential witness-value disclosure: performing this ledger operation".to_string(),
            TextRange::new(TextSize::new(start), TextSize::new(end)),
        )
    }

    fn advisory_at(start: u32, end: u32) -> Diagnostic {
        Diagnostic::warning(
            DiagnosticCode::new("U", 3100),
            "unverified: Unknown declared type".to_string(),
            TextRange::new(TextSize::new(start), TextSize::new(end)),
        )
    }

    #[test]
    fn quickfix_wraps_exactly_the_minimal_span() {
        // `c = getW();` — byte 4..10 is the minimal RHS expression `getW()`.
        let li = LineIndex::new(Arc::from("c = getW();"));
        let uri = Url::parse("file:///tmp/x.compact").unwrap();
        let leak = leak_at(4, 10);

        let fixes = disclosure_quickfixes(
            std::slice::from_ref(&leak),
            TextRange::new(TextSize::new(5), TextSize::new(5)),
            &li,
            &uri,
        );
        assert_eq!(fixes.len(), 1, "expected exactly one quickfix");
        let CodeActionOrCommand::CodeAction(action) = &fixes[0] else {
            panic!("expected a CodeAction, not a Command");
        };
        assert_eq!(action.title, "Reveal this value with disclose()");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert_eq!(action.is_preferred, Some(false));

        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
        assert_eq!(edits.len(), 2, "an insert-open + insert-close pair");
        // `disclose(` is inserted exactly at the span start (byte 4) ...
        assert_eq!(edits[0].new_text, "disclose(");
        assert_eq!(
            edits[0].range.start,
            lsp_utils::range_to_lsp(&li, leak.primary_span).start
        );
        assert_eq!(
            edits[0].range.start, edits[0].range.end,
            "a zero-width insertion"
        );
        // ... and `)` exactly at the span end (byte 10) — the COMBINED
        // touched range therefore equals `diag.primary_span` exactly, never
        // the whole `c = getW();` statement.
        assert_eq!(edits[1].new_text, ")");
        assert_eq!(
            edits[1].range.start,
            lsp_utils::range_to_lsp(&li, leak.primary_span).end
        );
        assert_eq!(
            edits[1].range.start, edits[1].range.end,
            "a zero-width insertion"
        );

        let touched_start = edits.iter().map(|e| e.range.start).min().unwrap();
        let touched_end = edits.iter().map(|e| e.range.end).max().unwrap();
        let diag_range = lsp_utils::range_to_lsp(&li, leak.primary_span);
        assert_eq!(
            (touched_start, touched_end),
            (diag_range.start, diag_range.end),
            "the edit's touched range must equal the diagnostic's span, not the whole statement"
        );
    }

    #[test]
    fn no_quickfix_outside_the_requested_range() {
        let li = LineIndex::new(Arc::from("c = getW();"));
        let uri = Url::parse("file:///tmp/x.compact").unwrap();
        let leak = leak_at(4, 10);

        // Cursor at byte 0 (`c`), far from the leak's span (4..10).
        let fixes = disclosure_quickfixes(
            std::slice::from_ref(&leak),
            TextRange::new(TextSize::new(0), TextSize::new(0)),
            &li,
            &uri,
        );
        assert!(fixes.is_empty());
    }

    #[test]
    fn no_quickfix_for_a_control_flow_leak() {
        // A control-flow (K2/K6) leak's span is the whole gated statement —
        // there is no single expression `disclose()` around it would fix,
        // so it must never be offered a wrap quick-fix.
        let li = LineIndex::new(Arc::from("c = getW();"));
        let uri = Url::parse("file:///tmp/x.compact").unwrap();
        let leak = control_leak_at(0, 11);

        let fixes = disclosure_quickfixes(
            std::slice::from_ref(&leak),
            TextRange::new(TextSize::new(5), TextSize::new(5)),
            &li,
            &uri,
        );
        assert!(fixes.is_empty());
    }

    #[test]
    fn no_quickfix_for_a_u_family_advisory() {
        // An advisory is unverified, not a confirmed leak — never actionable.
        let li = LineIndex::new(Arc::from("c = getW();"));
        let uri = Url::parse("file:///tmp/x.compact").unwrap();
        let advisory = advisory_at(4, 10);

        let fixes = disclosure_quickfixes(
            std::slice::from_ref(&advisory),
            TextRange::new(TextSize::new(5), TextSize::new(5)),
            &li,
            &uri,
        );
        assert!(fixes.is_empty());
    }
}
