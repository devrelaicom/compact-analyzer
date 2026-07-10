//! CST-derived folding ranges (byte ranges; the binary maps to lines).

use analyzer_core::{AnalysisHost, FileId, SyntaxKind, SyntaxNode, TextRange, TextSize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Region,
    Imports,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub range: TextRange,
    pub kind: FoldKind,
}

pub fn folding_ranges(host: &mut AnalysisHost, file: FileId) -> Vec<FoldRange> {
    let root = match host.analyze(file) {
        Some(a) => SyntaxNode::new_root(a.green.clone()),
        None => return Vec::new(),
    };
    let mut out = Vec::new();

    // 1. Brace-delimited bodies: from the opening `{` to the closing `}`.
    for node in root.descendants() {
        if matches!(
            node.kind(),
            SyntaxKind::CIRCUIT_DEF
                | SyntaxKind::CONSTRUCTOR_DEF
                | SyntaxKind::CONTRACT_DECL
                | SyntaxKind::MODULE_DEF
                | SyntaxKind::STRUCT_DEF
                | SyntaxKind::ENUM_DEF
        ) && let Some(range) = brace_span(&node)
        {
            out.push(FoldRange {
                range,
                kind: FoldKind::Region,
            });
        }
    }

    // 2. Leading consecutive import/include run (the root is always
    // SOURCE_FILE; children are the top-level items in document order).
    let import_run: Vec<SyntaxNode> = root
        .children()
        .take_while(|n| {
            matches!(n.kind(), SyntaxKind::IMPORT | SyntaxKind::INCLUDE)
                || n.kind() == SyntaxKind::PRAGMA
        })
        .filter(|n| matches!(n.kind(), SyntaxKind::IMPORT | SyntaxKind::INCLUDE))
        .collect();
    if let (Some(first), Some(last)) = (import_run.first(), import_run.last())
        && import_run.len() >= 2
    {
        out.push(FoldRange {
            range: TextRange::new(first.text_range().start(), last.text_range().end()),
            kind: FoldKind::Imports,
        });
    }

    // 3. Multi-line block comments.
    for tok in root
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
    {
        if tok.kind() == SyntaxKind::BLOCK_COMMENT && tok.text().contains('\n') {
            out.push(FoldRange {
                range: tok.text_range(),
                kind: FoldKind::Comment,
            });
        }
    }

    out
}

/// Byte range from a node's first `{` to its last `}` (start of `{` to start
/// of `}`), or `None` if either brace is missing.
fn brace_span(node: &SyntaxNode) -> Option<TextRange> {
    let mut open: Option<TextSize> = None;
    let mut close: Option<TextSize> = None;
    for t in node
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
    {
        match t.kind() {
            SyntaxKind::L_BRACE if open.is_none() => open = Some(t.text_range().start()),
            SyntaxKind::R_BRACE => close = Some(t.text_range().start()),
            _ => {}
        }
    }
    // Circuit bodies are a BLOCK child, so also look one level in.
    if (open.is_none() || close.is_none())
        && let Some(block) = node.children().find(|c| c.kind() == SyntaxKind::BLOCK)
    {
        for t in block
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
        {
            match t.kind() {
                SyntaxKind::L_BRACE if open.is_none() => open = Some(t.text_range().start()),
                SyntaxKind::R_BRACE => close = Some(t.text_range().start()),
                _ => {}
            }
        }
    }
    Some(TextRange::new(open?, close?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::AnalysisHost;

    fn folds(source: &str) -> Vec<(String, FoldKind)> {
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/m.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let text = source.to_string();
        folding_ranges(&mut host, file)
            .into_iter()
            .map(|f| (text[f.range].to_string(), f.kind))
            .collect()
    }

    #[test]
    fn folds_circuit_body_and_struct() {
        let v = folds("circuit f(): Field {\n  return 0;\n}\nstruct P {\n  x: Field;\n}");
        assert!(
            v.iter()
                .any(|(t, k)| t.starts_with('{') && *k == FoldKind::Region)
        );
        assert_eq!(v.iter().filter(|(_, k)| *k == FoldKind::Region).count(), 2);
    }

    #[test]
    fn folds_import_group_and_block_comment() {
        let v = folds(
            "import CompactStandardLibrary;\nimport Foo;\n/* a\n   b */\ncircuit f(): [] { }",
        );
        assert!(v.iter().any(|(_, k)| *k == FoldKind::Imports));
        assert!(v.iter().any(|(_, k)| *k == FoldKind::Comment));
    }

    #[test]
    fn single_line_body_is_not_folded_here() {
        // Byte range still spans one line; the binary drops it, but the ide
        // layer emits a range only when start != end line is possible — so we
        // still emit; assert the range is present but the binary will filter.
        let v = folds("circuit f(): [] { }");
        // Region range for `{ }` is emitted (single-line filtering is the
        // binary's job); nothing else.
        //
        // Deliberate strengthening over the brief: `Iterator::all` is
        // vacuously true on an empty Vec, so the brief's assertion alone
        // would also pass if folding produced nothing. Assert the fold is
        // actually present — exactly one Region fold for the `{ }` body —
        // so this test genuinely guards the single-line-body-still-emits
        // behavior instead of passing vacuously.
        assert_eq!(v.len(), 1);
        assert!(v.iter().all(|(_, k)| *k == FoldKind::Region));
    }
}
