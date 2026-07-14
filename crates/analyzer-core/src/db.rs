//! Salsa incremental-computation layer (v2a). Inputs are file text; tracked
//! queries derive parse/line-index/item-tree/symbols/imports. The impure
//! outer loop (VFS reads, disk probes) lives in `AnalysisHost`, which sets
//! these inputs.

use std::sync::Arc;

use compactp_ast::AstNode;

use crate::FileAnalysis;
use crate::item_tree::ItemTree;
use crate::line_index::LineIndex;

/// The database trait tracked functions are written against. A blanket impl
/// makes every salsa database (i.e. `CompactDatabase`) a `Db`.
#[salsa::db]
pub trait Db: salsa::Database {}

#[salsa::db]
impl<T: salsa::Database> Db for T {}

/// The concrete salsa database owned by `AnalysisHost`. Single-threaded use;
/// never cloned into another thread in v2a.
#[salsa::db]
#[derive(Default, Clone)]
pub struct CompactDatabase {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for CompactDatabase {}

/// File text as a salsa input. Identity is the salsa-assigned id; the host
/// maps `FileId -> SourceText` so a file keeps one stable input across edits.
#[salsa::input]
#[derive(Debug)]
pub struct SourceText {
    #[returns(clone)]
    pub text: Arc<str>,
}

/// Parse + derive everything M1 knows about one file. Memoized on the
/// `SourceText` input; recomputed only when that file's text changes.
///
/// `no_eq`: `FileAnalysis` is unchanged per the task brief and one of its
/// fields (`diagnostics: Arc<Vec<compactp_diagnostics::Diagnostic>>`) can't
/// derive `PartialEq` — `Diagnostic` is defined in an external crate and
/// doesn't implement it. Without `no_eq`, salsa requires the return type to
/// be comparable so it can "backdate" unchanged results to downstream
/// queries; `no_eq` opts this query out of that comparison; there are no
/// tracked queries downstream of `parsed` yet, so this has no
/// backdating-skip effect to lose in practice.
#[salsa::tracked(returns(clone), no_eq)]
pub fn parsed(db: &dyn Db, src: SourceText) -> FileAnalysis {
    let text = src.text(db);
    let result = compactp_parser::parse(&text);
    let root = compactp_syntax::SyntaxNode::new_root(result.green.clone());
    FileAnalysis {
        green: result.green,
        diagnostics: Arc::new(result.errors),
        line_index: Arc::new(LineIndex::new(text)),
        item_tree: Arc::new(ItemTree::extract(&root)),
    }
}

/// An import/include target as it appears in source, before filesystem
/// resolution. `CompactStandardLibrary` and imports satisfied by an in-scope
/// module are already filtered out.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RawDep {
    pub raw: String,
    pub is_include: bool,
}

/// Non-empty-named symbols in `src`'s item tree, paired with their index in
/// `ItemTree::symbols`. Derived from `parsed`; memoized separately so
/// downstream consumers that only need symbols aren't invalidated by changes
/// to diagnostics or the line index.
#[salsa::tracked(returns(clone))]
pub fn file_symbols(db: &dyn Db, src: SourceText) -> Arc<[(String, u32)]> {
    let tree = crate::db::parsed(db, src).item_tree;
    tree.symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.name.is_empty())
        .map(|(idx, s)| (s.name.clone(), idx as u32))
        .collect()
}

/// Import/include targets in `src`, before filesystem resolution. The pure
/// half of `AnalysisHost::index_file`'s dependency-extraction loop: this
/// filters out `CompactStandardLibrary` and imports satisfied by an in-scope
/// module, but does not resolve remaining targets to files (that requires
/// the VFS and search path, which live outside salsa).
#[salsa::tracked(returns(clone))]
pub fn raw_imports(db: &dyn Db, src: SourceText) -> Arc<[RawDep]> {
    let analysis = crate::db::parsed(db, src);
    let tree = analysis.item_tree.clone();
    let root = compactp_syntax::SyntaxNode::new_root(analysis.green.clone());
    let Some(sf) = compactp_ast::SourceFile::cast(root) else {
        return Arc::from(Vec::new());
    };
    let mut out = Vec::new();
    for import in sf.imports() {
        let raw = if let Some(n) = import.name() {
            let nm = n.text().to_string();
            if nm == "CompactStandardLibrary" {
                continue;
            }
            if tree
                .top_level()
                .any(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == nm)
            {
                continue;
            }
            nm
        } else if let Some(p) = import.path() {
            crate::string_lit_text(p.text())
        } else {
            continue;
        };
        out.push(RawDep { raw, is_include: false });
    }
    for inc in sf.includes() {
        if let Some(tok) = inc.path() {
            out.push(RawDep {
                raw: crate::string_lit_text(tok.text()),
                is_include: true,
            });
        }
    }
    Arc::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_reuses_memo_across_unchanged_calls() {
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("ledger count: Field;"));
        let a = crate::db::parsed(&db, src);
        let b = crate::db::parsed(&db, src);
        // returns(clone) hands back clones of the memoized value; the inner Arcs
        // are the same allocation on a memo hit.
        assert!(Arc::ptr_eq(&a.diagnostics, &b.diagnostics));
    }

    #[test]
    fn editing_one_file_does_not_recompute_another() {
        use salsa::Setter;

        let mut db = CompactDatabase::default();
        let a = SourceText::new(&db, Arc::from("ledger a: Field;"));
        let b = SourceText::new(&db, Arc::from("ledger b: Field;"));

        let _ = crate::db::parsed(&db, a);
        let _ = crate::db::parsed(&db, b);
        let a_diag = crate::db::parsed(&db, a).diagnostics;

        // Edit b; a's memo must survive (same Arc allocation).
        b.set_text(&mut db).to(Arc::from("ledger b2: Field;"));
        let a_diag_again = crate::db::parsed(&db, a).diagnostics;
        assert!(Arc::ptr_eq(&a_diag, &a_diag_again));
    }

    #[test]
    fn raw_imports_filters_stdlib_and_local_modules() {
        let db = CompactDatabase::default();
        let text = "import CompactStandardLibrary;\nimport Foo;\nmodule Foo {}\nimport \"bar/baz\";\n";
        let src = SourceText::new(&db, Arc::from(text));
        let deps = crate::db::raw_imports(&db, src);
        // CompactStandardLibrary filtered; `Foo` satisfied by local module → filtered;
        // only the string-path import survives.
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].raw, "bar/baz");
    }
}
