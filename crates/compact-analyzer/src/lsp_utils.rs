//! Conversions between analyzer-core's byte-offset world and LSP's
//! UTF-16 protocol types. This module is the ONLY place lsp_types and
//! analyzer_core types meet.

use analyzer_core::{Diagnostic, LineIndex, Severity, TextRange, TextSize};
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

/// The range spanning an entire document — used for whole-document-replace
/// edits (`textDocument/formatting`, Task 7). `range_to_lsp`/`LineIndex::line_col`
/// already clamp an out-of-bounds end offset down to the last line/UTF-16
/// column, so `TextSize::of(text)` (one-past-the-end) needs no bespoke
/// last-line arithmetic here.
pub(crate) fn whole_document_range(li: &LineIndex, text: &str) -> Range {
    range_to_lsp(li, TextRange::new(TextSize::new(0), TextSize::of(text)))
}

pub(crate) fn offset_from_position(
    li: &analyzer_core::LineIndex,
    pos: Position,
) -> Option<analyzer_core::TextSize> {
    li.offset(analyzer_core::LineCol {
        line: pos.line,
        col: pos.character,
    })
}

pub(crate) fn nav_target_to_location(
    host: &mut analyzer_core::AnalysisHost,
    target: &analyzer_ide::NavTarget,
) -> Option<Location> {
    let uri = Url::from_file_path(host.vfs().path(target.file)).ok()?;
    let analysis = host.analyze(target.file)?;
    Some(Location {
        uri,
        range: range_to_lsp(&analysis.line_index, target.name_range),
    })
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

/// Builds an LSP diagnostic for a `compact` compiler error, tagged
/// `source = "compactc"` (distinct from native `"compact-analyzer"`
/// diagnostics) and always `ERROR` severity — the `compact` CLI is fail-fast
/// and only reports genuine compile errors.
///
/// Unlike [`diagnostic_to_lsp`], there is no `code` and no related
/// information: the `--vscode` diagnostic grammar carries only a position and
/// a message ([`analyzer_toolchain::RawCompilerDiagnostic`]). `range` is the
/// byte [`TextRange`] already resolved by `analyzer_toolchain::locate`.
pub(crate) fn compiler_diagnostic_to_lsp(
    range: TextRange,
    message: String,
    li: &LineIndex,
) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range: range_to_lsp(li, range),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("compactc".to_string()),
        message,
        ..Default::default()
    }
}

pub(crate) fn completion_kind_to_lsp(
    kind: analyzer_ide::CompletionKind,
) -> lsp_types::CompletionItemKind {
    use analyzer_ide::CompletionKind as K;
    use lsp_types::CompletionItemKind as L;
    match kind {
        K::Keyword => L::KEYWORD,
        K::Circuit | K::Witness | K::StdlibItem => L::FUNCTION,
        K::Struct => L::STRUCT,
        K::StructField => L::FIELD,
        K::Enum => L::ENUM,
        K::EnumVariant => L::ENUM_MEMBER,
        K::Module => L::MODULE,
        K::TypeAlias => L::INTERFACE,
        K::LedgerField => L::PROPERTY,
        K::LedgerMethod => L::METHOD,
        K::Param | K::Local => L::VARIABLE,
        K::Generic => L::TYPE_PARAMETER,
        K::BuiltinType => L::KEYWORD,
    }
}

pub(crate) fn completion_item_to_lsp(c: analyzer_ide::CompletionItem) -> lsp_types::CompletionItem {
    lsp_types::CompletionItem {
        label: c.label,
        kind: Some(completion_kind_to_lsp(c.kind)),
        detail: c.detail,
        documentation: c.documentation.map(|value| {
            lsp_types::Documentation::MarkupContent(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value,
            })
        }),
        ..Default::default()
    }
}

/// `analyzer_ide::InlayHint` -> LSP `InlayHint`: `position` is the byte
/// offset converted through the line index (mirrors `range_to_lsp`'s
/// offset->`Position` conversion). `kind: TYPE` and zero padding on both
/// sides match the brief's rendering (`": Field"` immediately after the
/// binding name, no extra whitespace inserted by the client).
pub(crate) fn inlay_hint_to_lsp(
    hint: &analyzer_ide::InlayHint,
    li: &LineIndex,
) -> lsp_types::InlayHint {
    let pos = li.line_col(TextSize::new(hint.offset));
    lsp_types::InlayHint {
        position: Position::new(pos.line, pos.col),
        label: lsp_types::InlayHintLabel::String(hint.label.clone()),
        kind: Some(lsp_types::InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(false),
        data: None,
    }
}

/// One `SignatureInformation` (v2c never proposes overload alternatives —
/// `signature_help` already resolved to exactly one circuit/witness item),
/// `active_signature: Some(0)`. Each `ParamLabel`'s byte offsets are rendered
/// as `ParameterLabel::Simple(substring)` rather than
/// `ParameterLabel::LabelOffsets` — LSP label offsets are UTF-16 into the
/// label string, and `Simple` sidesteps that conversion entirely since the
/// substring IS the label text the client displays.
pub(crate) fn signature_help_to_lsp(
    d: analyzer_ide::SignatureHelpData,
) -> lsp_types::SignatureHelp {
    let label = d.label;
    let parameters = d
        .params
        .iter()
        .map(|p| lsp_types::ParameterInformation {
            label: lsp_types::ParameterLabel::Simple(
                label[p.start as usize..p.end as usize].to_string(),
            ),
            documentation: None,
        })
        .collect();
    lsp_types::SignatureHelp {
        signatures: vec![lsp_types::SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: None,
        }],
        active_signature: Some(0),
        active_parameter: d.active_param,
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

    #[cfg(unix)]
    #[test]
    fn accepts_file_uris() {
        let uri = lsp_types::Url::parse("file:///tmp/x.compact").unwrap();
        assert_eq!(
            abs_path_from_uri(&uri),
            Some(std::path::PathBuf::from("/tmp/x.compact"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn accepts_file_uris() {
        let uri = lsp_types::Url::parse("file:///C:/tmp/x.compact").unwrap();
        assert_eq!(
            abs_path_from_uri(&uri),
            Some(std::path::PathBuf::from(r"C:\tmp\x.compact"))
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
    fn whole_document_range_spans_first_to_last_line_and_column() {
        let text = "circuit f(): Field {\n  return 1;\n}\n";
        let li = LineIndex::new(Arc::from(text));
        let range = whole_document_range(&li, text);
        assert_eq!(range.start, Position::new(0, 0));
        // Trailing "\n" means the last line (index 3) is empty, at column 0.
        assert_eq!(range.end, Position::new(3, 0));
    }

    #[test]
    fn whole_document_range_ends_mid_last_line_without_trailing_newline() {
        let text = "export circuit foo(): Field {\n  return 1;\n}";
        let li = LineIndex::new(Arc::from(text));
        let range = whole_document_range(&li, text);
        assert_eq!(range.start, Position::new(0, 0));
        assert_eq!(range.end, Position::new(2, 1)); // "}" is 1 UTF-16 unit wide
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

    #[test]
    fn compiler_diagnostic_is_sourced_compactc_and_error() {
        let li = LineIndex::new(Arc::from("  round.incremen(1);"));
        let diag = compiler_diagnostic_to_lsp(
            TextRange::new(TextSize::new(7), TextSize::new(8)),
            "operation incremen undefined for ledger field type Counter".to_string(),
            &li,
        );
        assert_eq!(diag.source.as_deref(), Some("compactc"));
        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diag.range.start, Position::new(0, 7));
        assert_eq!(diag.range.end, Position::new(0, 8));
        assert!(diag.message.contains("incremen"));
        // No code / related info: the `--vscode` grammar carries neither.
        assert!(diag.code.is_none());
        assert!(diag.related_information.is_none());
    }

    #[test]
    fn warning_and_note_severities_map_correctly() {
        let li = LineIndex::new(Arc::from("x"));
        let uri = lsp_types::Url::parse("file:///tmp/x.compact").unwrap();
        let warn = Diagnostic::warning(
            DiagnosticCode::new("W", 1),
            "w".to_string(),
            TextRange::new(TextSize::new(0), TextSize::new(1)),
        );
        assert_eq!(
            diagnostic_to_lsp(&warn, &li, &uri).severity,
            Some(DiagnosticSeverity::WARNING)
        );
        let note = Diagnostic::note(
            DiagnosticCode::new("N", 1),
            "n".to_string(),
            TextRange::new(TextSize::new(0), TextSize::new(1)),
        );
        assert_eq!(
            diagnostic_to_lsp(&note, &li, &uri).severity,
            Some(DiagnosticSeverity::INFORMATION)
        );
    }
}
