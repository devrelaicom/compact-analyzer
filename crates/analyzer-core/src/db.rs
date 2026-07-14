//! Salsa incremental-computation layer (v2a). Inputs are file text; tracked
//! queries derive parse/line-index/item-tree/symbols/imports. The impure
//! outer loop (VFS reads, disk probes) lives in `AnalysisHost`, which sets
//! these inputs.

use std::sync::Arc;

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
}
