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
/// `no_eq`: one of `FileAnalysis`'s fields
/// (`diagnostics: Arc<Vec<compactp_diagnostics::Diagnostic>>`) can't derive
/// `PartialEq` — `Diagnostic` is defined in an external crate and doesn't
/// implement it. Without `no_eq`, salsa requires the return type to be
/// comparable so it can "backdate" unchanged results to downstream queries;
/// `no_eq` opts this query out of that comparison, so `parsed`'s direct
/// tracked dependents (`file_symbols`, `raw_imports`, both below) always
/// re-execute whenever a file's text changes — `parsed`'s result is treated
/// as changed every time. Those dependents immediately project out
/// `PartialEq` types (`Arc<[(String, u32)]>`, `Arc<[RawDep]>`), though, so
/// *their* outputs backdate normally; that projection layer, not `parsed`
/// itself, is the real backdating firewall insulating further consumers.
/// `no_eq` costs nothing in principle here regardless: `FileAnalysis`
/// embeds the full `green` tree, which reflects every source byte including
/// trivia, so `parsed` could never backdate even if it were `PartialEq`.
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

/// A cross-file resolved edge: one import/include target after host-side
/// disk resolution. `target: None` means the raw string did not resolve to a
/// file.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ResolvedDep {
    pub raw: String,
    pub is_include: bool,
    pub target: Option<(crate::FileId, SourceText)>,
}

/// Per-file cross-file environment, set by the host from `index_file`'s
/// resolution pass. One handle per `FileId`, stored host-side like `sources`.
#[salsa::input]
pub struct FileDeps {
    #[returns(clone)]
    pub deps: Arc<[ResolvedDep]>,
}

/// Workspace singleton: the stdlib file's `(FileId, SourceText)`, or `None`.
#[salsa::input]
pub struct Workspace {
    #[returns(clone)]
    pub stdlib: Option<(crate::FileId, SourceText)>,
}

/// Item tree for `src`, memoized on the `SourceText` input. Spike-minimal:
/// just projects `parsed`'s item tree out. `ItemTree` doesn't derive
/// `PartialEq` (Task 2 gives this query its own backdating story), so `no_eq`
/// opts out of salsa's default backdating comparison, same as `parsed` above.
#[salsa::tracked(returns(clone), no_eq)]
pub fn item_tree(db: &dyn Db, src: SourceText) -> Arc<ItemTree> {
    crate::db::parsed(db, src).item_tree
}

/// Throwaway spike: proves a tracked query can read a *second* file's
/// `item_tree` (reached via a `FileDeps` entry) purely, with no I/O. Deleted
/// in Task 5 once the real cross-file resolution queries land.
#[salsa::tracked(returns(clone))]
pub fn spike_cross_file(db: &dyn Db, fd: FileDeps, raw: String) -> Option<String> {
    let dep = fd.deps(db).iter().find(|d| d.raw == raw)?.clone();
    let (_, target_src) = dep.target?;
    let tree = crate::db::item_tree(db, target_src);
    tree.top_level().next().map(|(_, s)| s.name.clone())
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
    fn spike_reads_another_files_item_tree_purely() {
        let db = CompactDatabase::default();
        let target_src = SourceText::new(&db, Arc::from("export circuit tgt(): [] {}"));
        let deps: Arc<[ResolvedDep]> = Arc::from(vec![ResolvedDep {
            raw: "T".to_string(),
            is_include: false,
            target: Some((crate::FileId::from_raw_for_test(1), target_src)),
        }]);
        let fd = FileDeps::new(&db, deps);
        // The spike resolves raw "T" to its target's first top-level symbol name.
        assert_eq!(spike_cross_file(&db, fd, "T".to_string()).as_deref(), Some("tgt"));
        assert_eq!(spike_cross_file(&db, fd, "Nope".to_string()), None);
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
