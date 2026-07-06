//! Conversions between analyzer-core's byte-offset world and LSP's
//! UTF-16 protocol types. This module is the ONLY place lsp_types and
//! analyzer_core types meet.

// Consumed by the server module in the next task.
#![allow(dead_code)]

use analyzer_core::{Diagnostic, LineIndex, Severity, TextRange};
use lsp_types::{
    DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Position, Range,
    Url,
};

/// Extracts an absolute filesystem path from a `file://` URI.
/// Non-file schemes (untitled:, vscode-notebook-cell:, …) return `None`;
/// the server ignores those documents.
pub(crate) fn abs_path_from_uri(uri: &Url) -> Option<std::path::PathBuf> {
    if uri.scheme() != "file" {
        return None;
    }
    uri.to_file_path().ok()
}

pub(crate) fn range_to_lsp(li: &LineIndex, range: TextRange) -> Range {
    let start = li.line_col(range.start());
    let end = li.line_col(range.end());
    Range::new(
        Position::new(start.line, start.col),
        Position::new(end.line, end.col),
    )
}

pub(crate) fn diagnostic_to_lsp(
    d: &Diagnostic,
    li: &LineIndex,
    uri: &Url,
) -> lsp_types::Diagnostic {
    let severity = match d.severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Note => DiagnosticSeverity::INFORMATION,
    };

    let mut message = d.message.clone();
    for note in &d.notes {
        message.push_str("\nnote: ");
        message.push_str(note);
    }

    let related_information = if d.secondary_spans.is_empty() {
        None
    } else {
        Some(
            d.secondary_spans
                .iter()
                .map(|s| DiagnosticRelatedInformation {
                    location: Location {
                        uri: uri.clone(),
                        range: range_to_lsp(li, s.span),
                    },
                    message: s
                        .label
                        .clone()
                        .unwrap_or_else(|| "related location".to_string()),
                })
                .collect(),
        )
    };

    lsp_types::Diagnostic {
        range: range_to_lsp(li, d.primary_span),
        severity: Some(severity),
        code: Some(NumberOrString::String(d.code.to_string())),
        source: Some("compact-analyzer".to_string()),
        message,
        related_information,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{Diagnostic, DiagnosticCode, LineIndex, TextRange, TextSize};
    use lsp_types::{DiagnosticSeverity, Position};
    use std::sync::Arc;

    #[test]
    fn rejects_non_file_uris() {
        let uri = lsp_types::Url::parse("untitled:Untitled-1").unwrap();
        assert!(abs_path_from_uri(&uri).is_none());
    }

    #[test]
    fn accepts_file_uris() {
        let uri = lsp_types::Url::parse("file:///tmp/x.compact").unwrap();
        assert_eq!(
            abs_path_from_uri(&uri),
            Some(std::path::PathBuf::from("/tmp/x.compact"))
        );
    }

    #[test]
    fn range_maps_to_utf16_columns() {
        // Verified fixture: byte offset 23 in this source is UTF-16 col 21
        let li = LineIndex::new(Arc::from("/* \u{1F600} */ ledger count Field;"));
        let range = range_to_lsp(&li, TextRange::new(TextSize::new(23), TextSize::new(23)));
        assert_eq!(range.start, Position::new(0, 21));
        assert_eq!(range.end, Position::new(0, 21));
    }

    #[test]
    fn diagnostic_carries_source_code_and_severity() {
        let li = LineIndex::new(Arc::from("ledger count Field;"));
        let uri = lsp_types::Url::parse("file:///tmp/x.compact").unwrap();
        let diag = Diagnostic::error(
            DiagnosticCode::new("E", 1),
            "expected COLON".to_string(),
            TextRange::new(TextSize::new(12), TextSize::new(12)),
        )
        .with_note("ledger declarations need a type".to_string());

        let lsp = diagnostic_to_lsp(&diag, &li, &uri);
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(lsp.source.as_deref(), Some("compact-analyzer"));
        assert_eq!(
            lsp.code,
            Some(lsp_types::NumberOrString::String("E0001".to_string()))
        );
        assert_eq!(lsp.range.start, Position::new(0, 12));
        assert_eq!(
            lsp.message,
            "expected COLON\nnote: ledger declarations need a type"
        );
    }

    #[test]
    fn secondary_spans_become_related_information() {
        let li = LineIndex::new(Arc::from("ledger a: Field;\nledger a: Field;"));
        let uri = lsp_types::Url::parse("file:///tmp/x.compact").unwrap();
        let diag = Diagnostic::error(
            DiagnosticCode::new("E", 2),
            "duplicate name".to_string(),
            TextRange::new(TextSize::new(24), TextSize::new(25)),
        )
        .with_secondary(
            TextRange::new(TextSize::new(7), TextSize::new(8)),
            Some("first defined here".to_string()),
        );

        let lsp = diagnostic_to_lsp(&diag, &li, &uri);
        let related = lsp.related_information.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].message, "first defined here");
        assert_eq!(related[0].location.range.start, Position::new(0, 7));
    }
}
