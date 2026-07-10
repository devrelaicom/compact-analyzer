//! Hover: render the resolved definition as Markdown.

use analyzer_core::{AnalysisHost, Definition, FilePosition, SyntaxKind, SyntaxNode, SyntaxToken};
use compactp_ast::AstNode;
use rowan::TokenAtOffset;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverResult {
    pub markdown: String,
}

/// Renders the definition at `pos` as Markdown: items get a fenced
/// ```compact signature block plus their doc paragraph; locals get their
/// detail in the fenced block. Falls back to the ledger-ADT method table
/// when `resolve` finds nothing (ledger-method calls have no `Definition`).
pub fn hover(host: &mut AnalysisHost, pos: FilePosition) -> Option<HoverResult> {
    if let Some(def) = host.resolve(pos) {
        let markdown = match &def {
            Definition::Item { file, index } => {
                let sym = host
                    .analyze(*file)?
                    .item_tree
                    .symbols
                    .get(*index as usize)?
                    .clone();
                match &sym.doc {
                    Some(doc) => format!("```compact\n{}\n```\n\n{}", sym.signature, doc),
                    None => format!("```compact\n{}\n```", sym.signature),
                }
            }
            Definition::Local { detail, .. } => format!("```compact\n{detail}\n```"),
        };
        return Some(HoverResult { markdown });
    }
    ledger_method_hover(host, pos)
}

/// Hover for `receiver.method` where `receiver` is a ledger field: render the
/// method's signature + doc from the ledger-ADT table.
fn ledger_method_hover(host: &mut AnalysisHost, pos: FilePosition) -> Option<HoverResult> {
    let root = {
        let analysis = host.analyze(pos.file)?;
        SyntaxNode::new_root(analysis.green.clone())
    };
    let token = ident_at(&root, pos.offset)?;
    let parent = token.parent()?;
    if !matches!(
        parent.kind(),
        SyntaxKind::MEMBER_EXPR | SyntaxKind::CALL_EXPR
    ) {
        return None;
    }
    // A DOT must precede the method IDENT (excludes a direct-call callee).
    let dot_before = parent
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .any(|t| t.kind() == SyntaxKind::DOT && t.text_range().end() <= token.text_range().start());
    if !dot_before {
        return None;
    }
    let receiver = parent
        .children()
        .find(|c| compactp_ast::Expr::can_cast(c.kind()))?;
    // One-level-deep receiver typing only (F7 / spec §4, same guard as
    // completion's `push_member` — Errata E4): the receiver must be a bare
    // identifier (`NAME_EXPR`). A chained/compound receiver (`CALL_EXPR`,
    // `INDEX_EXPR`, nested `MEMBER_EXPR`, e.g. `tbl.lookup(k).⎸`) would need
    // type inference (v2) to resolve correctly; without this guard,
    // `non_trivia_start` descends into the compound receiver's subtree and
    // resolves its innermost-leftmost identifier instead (`tbl`, a ledger
    // `Map` field), leaking that field's ADT method surface under the outer
    // method name whenever it happens to collide (e.g. `Map.member`).
    if receiver.kind() != SyntaxKind::NAME_EXPR {
        return None;
    }
    // Resolve the receiver at its first non-trivia token's start offset
    // (Errata E3): `receiver.text_range().start()` is NOT safe here — a
    // `NAME_EXPR`'s leading trivia (whitespace) is its own first child token,
    // so the node's raw range start can land on whitespace rather than the
    // identifier, and `host.resolve` (via `ident_at_offset`) requires the
    // offset to be at/adjacent to an IDENT token.
    let def = host.resolve(FilePosition {
        file: pos.file,
        offset: crate::completion::non_trivia_start(&receiver),
    })?;
    let adt = host.ledger_field_adt(&def)?;
    let name = token.text().to_string();
    let m = host
        .ledger_adt_methods(&adt)
        .iter()
        .find(|m| m.name == name)?;
    Some(HoverResult {
        markdown: format!("```compact\n{}\n```\n\n{}", m.sig, m.doc),
    })
}

/// The IDENT at/adjacent to `offset` (prefer the right token at a boundary).
fn ident_at(root: &SyntaxNode, offset: analyzer_core::TextSize) -> Option<SyntaxToken> {
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => (t.kind() == SyntaxKind::IDENT).then_some(t),
        TokenAtOffset::Between(l, r) => (r.kind() == SyntaxKind::IDENT)
            .then_some(r)
            .or_else(|| (l.kind() == SyntaxKind::IDENT).then_some(l)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, FilePosition, fixture};

    fn hover_md(source: &str) -> Option<String> {
        let (clean, offset) = fixture::extract(source);
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        Some(hover(&mut host, FilePosition { file, offset })?.markdown)
    }

    #[test]
    fn hover_on_circuit_shows_signature_and_doc() {
        let md = hover_md(
            "// Doubles a value.\ncircuit double(x: Field): Field { return x + x; }\n\
             circuit m(): Field { return dou$0ble(1); }",
        )
        .unwrap();
        assert_eq!(
            md,
            "```compact\ncircuit double(x: Field): Field\n```\n\nDoubles a value."
        );
    }

    #[test]
    fn hover_on_param_shows_detail() {
        let md = hover_md("circuit f(base: Field): Field { return ba$0se; }").unwrap();
        assert_eq!(md, "```compact\nbase: Field\n```");
    }

    #[test]
    fn hover_on_stdlib_shows_bundled_doc() {
        let md = hover_md(
            "import CompactStandardLibrary;\ncircuit m(x: Field): Bytes<32> { return persistentHa$0sh<Field>(x); }",
        )
        .unwrap();
        assert!(md.contains("persistentHash"), "{md}");
        assert!(md.contains("SHA-256"), "{md}");
    }

    #[test]
    fn hover_on_nothing_is_none() {
        assert!(hover_md("circuit f(): Field { return myst$0ery; }").is_none());
    }

    #[test]
    fn hover_on_ledger_method_shows_table_signature() {
        let md = hover_md("export ledger cnt: Counter;\ncircuit f(): [] { cnt.incre$0ment(1); }")
            .unwrap();
        assert!(md.contains("increment(amount: Uint<16>): []"), "{md}");
        assert!(md.contains("Increments the counter"), "{md}");
    }

    #[test]
    fn hover_on_plain_ledger_field_method_shows_cell_signature() {
        let md =
            hover_md("export ledger bal: Uint<64>;\ncircuit f(): [] { bal.wr$0ite(7); }").unwrap();
        assert!(md.contains("write(value: T): []"), "{md}");
    }

    #[test]
    fn member_on_chained_receiver_offers_nothing() {
        // F7 / spec §4, mirroring the identical Task-5 boundary regression:
        // a chained/CALL_EXPR receiver (`tbl.lookup(k).⎸`, where `tbl` is a
        // ledger `Map` field) is NOT a bare identifier. Hovering `member` here
        // must NOT resolve the inner `tbl` and leak `Map.member`'s signature —
        // that would require type inference (v2) to know `lookup(k)` returns
        // a `Field`, not a `Map`.
        let md = hover_md(
            "export ledger tbl: Map<Field, Field>;\n\
             circuit f(): [] { const k = 1; tbl.lookup(k).mem$0ber(k); }",
        );
        assert!(md.is_none(), "{md:?}");
    }
}
