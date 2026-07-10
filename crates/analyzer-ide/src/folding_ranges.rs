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
    //
    // A leading `pragma` (real files typically start with
    // `pragma language_version >= 0.x;`) is skipped first, THEN the run is
    // taken as consecutive imports/includes only. A bare
    // `take_while(IMPORT | INCLUDE | PRAGMA)` would treat PRAGMA as a
    // permanent pass-through: an *interior* pragma between two imports would
    // keep the run alive across it (swallowing the pragma line into a
    // `kind=imports` fold), while starting the `take_while` at child 0 (the
    // leading pragma itself) would stop the run immediately in the common
    // case, producing no imports fold at all.
    let import_run: Vec<SyntaxNode> = root
        .children()
        .skip_while(|n| n.kind() == SyntaxKind::PRAGMA)
        .take_while(|n| matches!(n.kind(), SyntaxKind::IMPORT | SyntaxKind::INCLUDE))
        .collect();
    if let (Some(first), Some(last)) = (import_run.first(), import_run.last())
        && import_run.len() >= 2
    {
        // An import NODE's `text_range().start()` includes its leading
        // trivia (whitespace/comments, and — after a leading pragma — the
        // newline separating it from the pragma line). Anchor on the first
        // non-trivia token instead so the fold starts at `import`, not at
        // the preceding trivia. Verified against compactp's CST: for
        // `pragma p;\nimport A;`, the `IMPORT` node's first child is
        // `WHITESPACE "\n"`, i.e. `text_range().start()` points one byte
        // before the `import` keyword.
        let start = crate::completion::non_trivia_start(first);
        out.push(FoldRange {
            range: TextRange::new(start, last.text_range().end()),
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
    // Guard against a malformed/error-recovered node presenting `}` before
    // `{` (unreachable for well-formed source today, but `folding_ranges` is
    // an entry point fed by whatever the parser produces, and
    // `TextRange::new` asserts start <= end and would panic otherwise).
    let open = open?;
    let close = close?;
    (open <= close).then(|| TextRange::new(open, close))
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
    fn imports_fold_skips_leading_pragma_and_stops_at_trailing_pragma() {
        // F1 + F2: a leading `pragma` must not be swallowed into the fold's
        // start (F2's trivia-anchoring), and a trailing pragma must not
        // extend the run past the last import (F1's run detector).
        let v = folds("pragma p;\nimport A;\nimport B;\npragma q;\ncircuit f(): [] { }");
        let imports: Vec<&(String, FoldKind)> =
            v.iter().filter(|(_, k)| *k == FoldKind::Imports).collect();
        assert_eq!(imports.len(), 1, "expected exactly one Imports fold: {v:?}");
        assert_eq!(imports[0].0, "import A;\nimport B;");
    }

    #[test]
    fn interior_pragma_splits_the_import_run() {
        // F1: an interior pragma between two imports must not keep the
        // leading-run detector alive across it. The old
        // `take_while(IMPORT|INCLUDE|PRAGMA)` treated PRAGMA as a permanent
        // pass-through, so this fixture used to fold `import A;` through
        // `import B;`, swallowing `pragma foo;` into a `kind=imports` fold.
        // After the fix the run stops at the interior pragma, leaving a
        // single-import run that's below the `>= 2` threshold, so no
        // Imports fold is emitted at all.
        let v = folds("import A;\npragma foo;\nimport B;\ncircuit f(): [] { }");
        assert!(
            !v.iter().any(|(_, k)| *k == FoldKind::Imports),
            "a single leading import (run broken by the interior pragma) must not fold: {v:?}"
        );
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
