//! Salsa incremental-computation layer (v2a). Inputs are file text; tracked
//! queries derive parse/line-index/item-tree/symbols/imports. The impure
//! outer loop (VFS reads, disk probes) lives in `AnalysisHost`, which sets
//! these inputs.

use std::sync::Arc;

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

/// Throwaway spike query — replaced by `parsed` in Task 3.
#[salsa::tracked(returns(clone))]
pub fn parsed_len(db: &dyn Db, src: SourceText) -> usize {
    src.text(db).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use salsa::Setter as _;

    #[test]
    fn db_boots_memoizes_and_recomputes() {
        let mut db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("hello"));
        assert_eq!(parsed_len(&db, src), 5);

        // Unchanged input → memoized (same revision, no panic, same value).
        assert_eq!(parsed_len(&db, src), 5);

        // Mutate the input → recompute reflects the new value.
        src.set_text(&mut db).to(Arc::from("hi"));
        assert_eq!(parsed_len(&db, src), 2);
    }
}
