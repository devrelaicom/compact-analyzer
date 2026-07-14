//! Memoized recompute engine (spec Approach A).
//!
//! Parsing, the line index, and the item tree are produced by the salsa
//! `parsed` tracked query (`crate::db::parsed`), memoized on the per-file
//! `SourceText` salsa input. `AnalysisHost` provisions and updates that input
//! from the VFS (`source_for`), keyed on the content `Arc<str>` pointer
//! identity so unchanged text is never re-set into salsa, and any file edit
//! recomputes that file only. The syntax tree is stored as a `rowan::GreenNode`
//! because `SyntaxNode` is `!Send` — consumers rebuild a cursor with
//! `SyntaxNode::new_root(green)` (cheap).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use compactp_diagnostics::Diagnostic;
use rowan::GreenNode;

use crate::item_tree::ItemTree;
use crate::line_index::LineIndex;
use crate::vfs::{FileId, Vfs};

/// Everything M1 knows about one file. All fields are cheap to clone.
#[derive(Clone)]
pub struct FileAnalysis {
    pub green: GreenNode,
    pub diagnostics: Arc<Vec<Diagnostic>>,
    pub line_index: Arc<LineIndex>,
    pub item_tree: Arc<ItemTree>,
}

pub struct AnalysisHost {
    db: crate::db::CompactDatabase,
    /// One stable salsa input per file, plus the raw pointer of the `Arc<str>`
    /// last pushed into it — an unchanged pointer means unchanged content, so
    /// we skip `set_text` and avoid bumping the salsa revision. Same ABA-safe
    /// coupling as before: the `SourceText` input retains the `Arc<str>`.
    sources: HashMap<FileId, (usize, crate::db::SourceText)>,
    vfs: Vfs,
    stdlib: Option<FileId>,
    import_search_path: Vec<PathBuf>,
    workspace: crate::workspace::WorkspaceIndex,
    ledger_adts: crate::ledger_adts::LedgerAdtTable,
    /// Memoized `find_source_pathname` probes, keyed on `(from_dir, raw)`. The
    /// current `import_search_path` is an implicit part of the key, enforced by
    /// clearing on `set_import_search_path`. See [`cached_source_pathname`] for
    /// the full invalidation contract.
    ///
    /// [`cached_source_pathname`]: AnalysisHost::cached_source_pathname
    source_path_cache: HashMap<(PathBuf, String), Option<PathBuf>>,
    /// The `generation()` the `source_path_cache` was last valid for. A change
    /// means a VFS content mutation (create/change/overlay) has occurred, so the
    /// memo is stale and must be dropped.
    source_path_cache_generation: u64,
}

impl AnalysisHost {
    pub fn new() -> Self {
        Self {
            db: crate::db::CompactDatabase::default(),
            sources: HashMap::new(),
            vfs: Vfs::new(),
            stdlib: None,
            import_search_path: Vec::new(),
            workspace: crate::workspace::WorkspaceIndex::default(),
            ledger_adts: crate::ledger_adts::LedgerAdtTable::load(),
            source_path_cache: HashMap::new(),
            source_path_cache_generation: 0,
        }
    }

    /// Lazily provisions (or updates) the salsa `SourceText` input for `file`
    /// from current VFS content. `None` if unreadable.
    fn source_for(&mut self, file: FileId) -> Option<crate::db::SourceText> {
        use salsa::Setter as _;
        let text = self.vfs.read(file)?;
        let ptr = Arc::as_ptr(&text) as *const u8 as usize;
        match self.sources.get(&file).copied() {
            Some((cached_ptr, src)) if cached_ptr == ptr => Some(src),
            Some((_, src)) => {
                src.set_text(&mut self.db).to(text);
                self.sources.insert(file, (ptr, src));
                Some(src)
            }
            None => {
                let src = crate::db::SourceText::new(&self.db, text);
                self.sources.insert(file, (ptr, src));
                Some(src)
            }
        }
    }

    /// Access to the curated ledger-ADT method table (owned by the
    /// `ledger_adts` module, which implements the public accessors on top of
    /// this private getter).
    pub(crate) fn ledger_adts_table(&self) -> &crate::ledger_adts::LedgerAdtTable {
        &self.ledger_adts
    }

    pub fn vfs(&self) -> &Vfs {
        &self.vfs
    }

    pub fn vfs_mut(&mut self) -> &mut Vfs {
        &mut self.vfs
    }

    /// Registers the materialized stdlib stub so the resolver can target it.
    pub fn register_stdlib(&mut self, path: &std::path::Path) -> FileId {
        let file = self.vfs.file_id(path);
        self.stdlib = Some(file);
        file
    }

    pub fn stdlib_file(&self) -> Option<FileId> {
        self.stdlib
    }

    /// Directories consulted (after the importing file's own directory) when
    /// resolving non-absolute imports/includes. Mirrors `--compact-path`.
    pub fn set_import_search_path(&mut self, path: Vec<PathBuf>) {
        self.import_search_path = path;
        // The search path is an implicit part of every memoized probe key and a
        // change here does NOT bump the VFS generation, so the generation hook
        // in `cached_source_pathname` would miss it: drop the memo explicitly so
        // probes re-run against the new path. (Invalidation hook 3 of 3.)
        self.source_path_cache.clear();
    }

    pub fn import_search_path(&self) -> Vec<PathBuf> {
        self.import_search_path.clone()
    }

    /// Memoizing wrapper over [`crate::find_source_pathname`]. Within one
    /// resolution epoch each `(from_dir, raw)` pair is probed (i.e. its
    /// `Path::exists` syscalls run) at most once. `find_source_pathname` itself
    /// stays pure and unchanged; this only caches its results.
    ///
    /// # Invalidation contract (why a memoized value is always correct)
    /// The memo MUST return exactly what a fresh `find_source_pathname` against
    /// the current on-disk state (as known to the host) and the current
    /// `import_search_path` would return. For a fixed `(from_dir, raw)` key the
    /// result can only change via one of three host events, and each clears the
    /// memo:
    /// 1. any VFS content mutation — a watched-file CREATE/CHANGE
    ///    (`Vfs::invalidate_disk`) or an overlay op
    ///    (`Vfs::set_overlay`/`remove_overlay`) — bumps `generation()`, which is
    ///    detected and cleared at the top of this method;
    /// 2. a watched-file DELETE, routed through `forget_file`, which does NOT
    ///    bump the generation and so is cleared inside `forget_file`;
    /// 3. an `import_search_path` change, cleared inside `set_import_search_path`.
    ///
    /// Interning a path (`Vfs::file_id`) is the only other host mutation and it
    /// cannot change a probe result (it touches neither disk existence,
    /// `from_dir`, `raw`, nor `import_search_path`), so it needs no hook. These
    /// three hooks therefore form a complete cover.
    pub(crate) fn cached_source_pathname(
        &mut self,
        from_dir: &std::path::Path,
        raw: &str,
    ) -> Option<PathBuf> {
        // Hook 1 of 3: any content mutation bumps the generation → the whole
        // memo is stale. Clear and re-key to the current generation.
        let generation = self.generation();
        if generation != self.source_path_cache_generation {
            self.source_path_cache.clear();
            self.source_path_cache_generation = generation;
        }
        let key = (from_dir.to_path_buf(), raw.to_string());
        if let Some(cached) = self.source_path_cache.get(&key) {
            return cached.clone();
        }
        // Compute BEFORE touching the cache: the immutable borrow of
        // `import_search_path` must not overlap the mutable borrow in `insert`.
        let result = crate::find_source_pathname(from_dir, &self.import_search_path, raw);
        self.source_path_cache.insert(key, result.clone());
        result
    }

    /// Current workspace revision (see `Vfs::generation`).
    pub fn generation(&self) -> u64 {
        self.vfs.generation()
    }

    /// Parses `file`, memoized by salsa on the `SourceText` input (which
    /// changes exactly when the content is replaced). `None` if unreadable.
    pub fn analyze(&mut self, file: FileId) -> Option<FileAnalysis> {
        let src = self.source_for(file)?;
        Some(crate::db::parsed(&self.db, src))
    }

    /// (Re)builds the workspace-index entry for one file: its symbol table and
    /// its outgoing import/include dependency edges. Reads/parses `file`
    /// itself via the VFS; for each import/include target it only interns the
    /// path (records the target's `FileId`) without reading or parsing it. An
    /// unreadable file is evicted from the index.
    pub fn index_file(&mut self, file: FileId) {
        let Some(src) = self.source_for(file) else {
            self.workspace.remove(file);
            return;
        };
        let symbols: Vec<(String, u32)> = crate::db::file_symbols(&self.db, src).to_vec();
        let raw_deps = crate::db::raw_imports(&self.db, src);

        let from_dir = self.vfs().path(file).parent().map(|d| d.to_path_buf());
        let search = self.import_search_path();
        let mut deps = std::collections::BTreeSet::new();
        if let Some(from_dir) = from_dir {
            for dep in raw_deps.iter() {
                if let Some(path) = crate::find_source_pathname(&from_dir, &search, &dep.raw) {
                    deps.insert(self.vfs_mut().file_id(&path));
                }
            }
        }
        self.workspace.set_file(file, symbols, deps);
    }

    /// Evicts a file (deleted on disk) from the workspace index.
    pub fn remove_workspace_file(&mut self, file: FileId) {
        self.workspace.remove(file);
        self.forget_file(file);
    }

    /// Drops the salsa `SourceText` input for `file` (e.g. deleted on disk).
    /// Parsing is memoized by salsa on the input itself, not by this host, so
    /// this only forgets the host's `FileId -> SourceText` binding; the next
    /// `source_for` call re-provisions a fresh input from the VFS.
    pub fn forget_file(&mut self, file: FileId) {
        self.sources.remove(&file);
        // A watched-file DELETE reaches us here (via `remove_workspace_file`)
        // and does NOT bump the VFS generation, so the generation hook in
        // `cached_source_pathname` would miss it: a stale `Some(deleted_path)`
        // must not survive the deletion. (Invalidation hook 2 of 3.)
        self.source_path_cache.clear();
    }

    /// All files currently in the workspace index.
    pub fn workspace_files(&self) -> Vec<FileId> {
        self.workspace.files()
    }

    /// Files that import/include `file` (direct dependents).
    pub fn dependents_of(&self, file: FileId) -> Vec<FileId> {
        self.workspace.dependents(file)
    }

    /// Symbols whose name subsequence-matches `query` (case-insensitive).
    /// Returns (file, symbol index into that file's ItemTree).
    pub fn workspace_symbols(&mut self, query: &str) -> Vec<(FileId, u32)> {
        self.workspace.symbols_matching(query)
    }

    /// Discovers and indexes every `.compact` under `roots`. Polls
    /// `should_continue` before each file so a superseded crawl abandons early.
    pub fn discover_and_index(
        &mut self,
        roots: &[std::path::PathBuf],
        should_continue: &dyn Fn() -> bool,
    ) {
        for path in crate::workspace::discover_compact_files(roots) {
            if !should_continue() {
                return;
            }
            let file = self.vfs_mut().file_id(&path);
            self.index_file(file);
        }
    }
}

impl Default for AnalysisHost {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Opens `text` as an overlay in a fresh host and returns (host, file).
    fn host_with(text: &str) -> (AnalysisHost, crate::FileId) {
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(Path::new("/test/doc.compact"));
        host.vfs_mut().set_overlay(file, text.to_string(), 1);
        (host, file)
    }

    #[test]
    fn valid_source_has_no_diagnostics() {
        let (mut host, file) = host_with("ledger count: Field;");
        let analysis = host.analyze(file).unwrap();
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn broken_source_reports_expected_colon() {
        // Verified fixture: zero-width span at offset 12
        let (mut host, file) = host_with("ledger count Field;");
        let analysis = host.analyze(file).unwrap();
        assert_eq!(analysis.diagnostics.len(), 1);
        let diag = &analysis.diagnostics[0];
        assert_eq!(diag.message, "expected COLON");
        assert_eq!(diag.primary_span.start(), crate::TextSize::new(12));
        assert_eq!(diag.primary_span.end(), crate::TextSize::new(12));
    }

    #[test]
    fn diagnostics_spans_stay_in_bounds() {
        let source = "@@@";
        let (mut host, file) = host_with(source);
        let analysis = host.analyze(file).unwrap();
        assert!(!analysis.diagnostics.is_empty());
        for diag in analysis.diagnostics.iter() {
            assert!(u32::from(diag.primary_span.end()) <= source.len() as u32);
        }
    }

    #[test]
    fn unchanged_content_hits_the_cache() {
        let (mut host, file) = host_with("ledger count: Field;");
        let first = host.analyze(file).unwrap();
        let second = host.analyze(file).unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &first.diagnostics,
            &second.diagnostics
        ));
    }

    #[test]
    fn edit_invalidates_the_cache() {
        let (mut host, file) = host_with("ledger count: Field;");
        let before = host.analyze(file).unwrap();
        assert!(before.diagnostics.is_empty());

        host.vfs_mut()
            .set_overlay(file, "ledger count Field;".to_string(), 2);
        let after = host.analyze(file).unwrap();
        assert_eq!(after.diagnostics.len(), 1);
        assert!(!std::sync::Arc::ptr_eq(
            &before.diagnostics,
            &after.diagnostics
        ));
    }

    #[test]
    fn forget_file_is_transparent_to_parse() {
        let (mut host, file) = host_with("ledger count: Field;");
        let first = host.analyze(file).unwrap();
        host.forget_file(file);
        let second = host.analyze(file).unwrap();
        // The host's FileId -> SourceText binding was dropped, but salsa
        // memoizes parsing on the input's value, not on host-side identity,
        // so the recompute is transparent: same diagnostics value (an eviction
        // that actually drops a stale disk file is Task 5's concern).
        // `compactp_diagnostics::Diagnostic` (external crate) derives `Debug`
        // but not `PartialEq`, so value equality is asserted via its `Debug`
        // representation rather than `assert_eq!` directly on the `Vec`.
        assert_eq!(
            format!("{:?}", first.diagnostics),
            format!("{:?}", second.diagnostics)
        );
    }

    #[test]
    fn tree_is_lossless() {
        let source = "/* comment */ ledger count: Field; // trailing";
        let (mut host, file) = host_with(source);
        let analysis = host.analyze(file).unwrap();
        let root = compactp_syntax::SyntaxNode::new_root(analysis.green);
        assert_eq!(root.text().to_string(), source);
    }

    #[test]
    fn unreadable_file_returns_none() {
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(Path::new("/nonexistent/nope.compact"));
        assert!(host.analyze(file).is_none());
    }

    #[test]
    fn source_for_updates_input_on_edit_only() {
        let (mut host, file) = host_with("abcd");
        let src1 = host.source_for(file).unwrap();
        assert_eq!(src1.text(&host.db).len(), 4);

        // Same content re-provisioned → same input handle, no revision churn.
        let src2 = host.source_for(file).unwrap();
        assert_eq!(src1, src2);

        // Edit → same handle, new text visible to queries.
        host.vfs_mut().set_overlay(file, "xyz".to_string(), 2);
        let src3 = host.source_for(file).unwrap();
        assert_eq!(src1, src3);
        assert_eq!(src3.text(&host.db).len(), 3);
    }

    #[test]
    fn index_file_records_resolved_cross_file_dep() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Foo.compact"), "module Foo {}").unwrap();
        let main = dir.path().join("main.compact");
        std::fs::write(&main, "import \"Foo\";").unwrap();

        let mut host = AnalysisHost::new();
        let foo = host.vfs_mut().file_id(&dir.path().join("Foo.compact"));
        let main_id = host.vfs_mut().file_id(&main);
        host.index_file(foo);
        host.index_file(main_id);

        assert_eq!(host.dependents_of(foo), vec![main_id]);
    }
}
