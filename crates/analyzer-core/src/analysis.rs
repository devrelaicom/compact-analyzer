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
    /// One stable `FileDeps` salsa input per file, set by `index_file`'s
    /// resolution pass. Mirrors `sources`' create-or-update pattern.
    dep_inputs: HashMap<FileId, crate::db::FileDeps>,
    /// The `Workspace` singleton input (currently just the stdlib handle),
    /// provisioned lazily on first use (`workspace()`) or by `register_stdlib`.
    workspace_input: Option<crate::db::Workspace>,
    vfs: Vfs,
    stdlib: Option<FileId>,
    import_search_path: Vec<PathBuf>,
    workspace: crate::workspace::WorkspaceIndex,
    ledger_adts: crate::ledger_adts::LedgerAdtTable,
}

impl AnalysisHost {
    pub fn new() -> Self {
        Self {
            db: crate::db::CompactDatabase::default(),
            sources: HashMap::new(),
            dep_inputs: HashMap::new(),
            workspace_input: None,
            vfs: Vfs::new(),
            stdlib: None,
            import_search_path: Vec::new(),
            workspace: crate::workspace::WorkspaceIndex::default(),
            ledger_adts: crate::ledger_adts::LedgerAdtTable::load(),
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
                // Keyset grew: republish the workspace-wide file_srcs map.
                self.republish_file_srcs();
                Some(src)
            }
        }
    }

    /// Bridge accessor for the resolution dispatcher: exposes the private
    /// `source_for` provisioner to `resolve.rs`'s host bridge under the name the
    /// task brief uses.
    pub(crate) fn src_of(&mut self, file: FileId) -> Option<crate::db::SourceText> {
        self.source_for(file)
    }

    /// Borrow the salsa database, for the host bridge to hand to tracked queries
    /// (`resolve_query`) defined in `db.rs`.
    pub(crate) fn db_ref(&self) -> &crate::db::CompactDatabase {
        &self.db
    }

    /// Ensures `file` and every file transitively reachable from it by `include`
    /// are indexed, so the tracked resolution queries can read each hop's
    /// `FileDeps` from the workspace map. The host `resolve()` bridge calls this
    /// before dispatching: unlike the old imperative resolver (which walked the
    /// include graph lazily off the VFS), the salsa path resolves includes
    /// purely from pre-published `FileDeps`, so the graph must be materialized
    /// first. Already-indexed files are left untouched (their inputs stay
    /// current via `did_change`/watcher re-indexing); the explicit `seen` set
    /// makes an include cycle terminate. Import (non-include) targets need no
    /// indexing — their `SourceText` rides along in the `ResolvedDep`.
    pub(crate) fn ensure_indexed(&mut self, file: FileId) {
        let mut stack = vec![file];
        let mut seen = std::collections::HashSet::new();
        while let Some(f) = stack.pop() {
            if !seen.insert(f) {
                continue;
            }
            if !self.dep_inputs.contains_key(&f) {
                self.index_file(f);
            }
            let targets: Vec<FileId> = match self.dep_inputs.get(&f).copied() {
                Some(fd) => fd
                    .deps(&self.db)
                    .iter()
                    .filter(|d| d.is_include)
                    .filter_map(|d| d.target.map(|(t, _)| t))
                    .collect(),
                None => Vec::new(),
            };
            stack.extend(targets);
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

    /// Registers the materialized stdlib stub so the resolver can target it,
    /// and publishes it as the `Workspace` singleton's `stdlib` field so
    /// tracked resolution queries can read it purely.
    pub fn register_stdlib(&mut self, path: &std::path::Path) -> FileId {
        use salsa::Setter as _;
        let file = self.vfs.file_id(path);
        self.stdlib = Some(file);
        let src = self.source_for(file);
        let stdlib = src.map(|s| (file, s));
        match self.workspace_input {
            Some(ws) => {
                ws.set_stdlib(&mut self.db).to(stdlib);
            }
            None => {
                let map = self.current_file_deps_map();
                let srcs = self.current_file_srcs_map();
                self.workspace_input = Some(crate::db::Workspace::new(&self.db, stdlib, map, srcs));
            }
        }
        file
    }

    pub fn stdlib_file(&self) -> Option<FileId> {
        self.stdlib
    }

    /// The `Workspace` singleton, provisioned empty if the stdlib was never
    /// registered. Read by the host `resolve()` bridge and the tracked
    /// resolution queries.
    pub(crate) fn workspace(&mut self) -> crate::db::Workspace {
        if let Some(ws) = self.workspace_input {
            return ws;
        }
        let map = self.current_file_deps_map();
        let srcs = self.current_file_srcs_map();
        let ws = crate::db::Workspace::new(&self.db, None, map, srcs);
        self.workspace_input = Some(ws);
        ws
    }

    /// Snapshot of the `FileId -> FileDeps` map published to `Workspace`, built
    /// from the host's per-file `dep_inputs`. Cross-file resolution queries
    /// (e.g. the transitive include walk) look a *target*'s `FileDeps` up here.
    fn current_file_deps_map(
        &self,
    ) -> Arc<std::collections::BTreeMap<FileId, crate::db::FileDeps>> {
        Arc::new(self.dep_inputs.iter().map(|(&f, &fd)| (f, fd)).collect())
    }

    /// Snapshot of the `FileId -> SourceText` map published to `Workspace`,
    /// built from the host's `sources`. Every resolution target's text lives
    /// here: `index_file` provisions each dep target via `source_for` before
    /// publishing, so `sources` is the complete set of reachable files.
    fn current_file_srcs_map(
        &self,
    ) -> Arc<std::collections::BTreeMap<FileId, crate::db::SourceText>> {
        Arc::new(self.sources.iter().map(|(&f, &(_, s))| (f, s)).collect())
    }

    /// Republishes `Workspace.file_deps` from `dep_inputs`. Called ONLY when the
    /// keyset changes (a `FileDeps` input was newly created) — republishing on
    /// every content update would bump the salsa revision and over-invalidate
    /// every resolution query. Per-file dep *content* changes flow through the
    /// individual `FileDeps` inputs, not this map.
    fn republish_file_deps_map(&mut self) {
        use salsa::Setter as _;
        let map = self.current_file_deps_map();
        match self.workspace_input {
            Some(ws) => {
                ws.set_file_deps(&mut self.db).to(map);
            }
            None => {
                let srcs = self.current_file_srcs_map();
                self.workspace_input = Some(crate::db::Workspace::new(&self.db, None, map, srcs));
            }
        }
    }

    /// Republishes `Workspace.file_srcs` from `sources`. Called only when the
    /// `sources` keyset changes (a file enters/leaves the workspace) — a
    /// structural event, exactly as `file_deps` is republished only on its own
    /// keyset change. In-body edits reuse a file's existing `SourceText` input
    /// (see `source_for`), so they never reach here.
    fn republish_file_srcs(&mut self) {
        use salsa::Setter as _;
        let srcs = self.current_file_srcs_map();
        match self.workspace_input {
            Some(ws) => {
                ws.set_file_srcs(&mut self.db).to(srcs);
            }
            None => {
                let deps = self.current_file_deps_map();
                self.workspace_input = Some(crate::db::Workspace::new(&self.db, None, deps, srcs));
            }
        }
    }

    /// Directories consulted (after the importing file's own directory) when
    /// resolving non-absolute imports/includes. Mirrors `--compact-path`.
    pub fn set_import_search_path(&mut self, path: Vec<PathBuf>) {
        // Just store the new path; re-indexing republishes each file's
        // `FileDeps` under it. There is no separate disk-probe memo to clear.
        self.import_search_path = path;
    }

    pub fn import_search_path(&self) -> Vec<PathBuf> {
        self.import_search_path.clone()
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
        let mut edges = std::collections::BTreeSet::new();
        let mut resolved: Vec<crate::db::ResolvedDep> = Vec::new();
        if let Some(from_dir) = from_dir {
            for dep in raw_deps.iter() {
                // A resolved-but-unreadable target becomes `None` (no edge, no
                // panic): honours the resolver's "unreadable → skip, never die"
                // ethos.
                let target =
                    crate::find_source_pathname(&from_dir, &search, &dep.raw).and_then(|path| {
                        let fid = self.vfs_mut().file_id(&path);
                        self.source_for(fid).map(|s| (fid, s))
                    });
                if let Some((fid, _)) = target {
                    edges.insert(fid);
                }
                resolved.push(crate::db::ResolvedDep {
                    raw: dep.raw.clone(),
                    is_include: dep.is_include,
                    target,
                });
            }
        }
        self.publish_file_deps(file, resolved);
        self.workspace.set_file(file, symbols, edges);
    }

    /// Create-or-update the `FileDeps` salsa input for `file` from `index_file`'s
    /// resolution pass. Mirrors `source_for`'s create-or-update pattern.
    fn publish_file_deps(&mut self, file: FileId, resolved: Vec<crate::db::ResolvedDep>) {
        use salsa::Setter as _;
        let arc: Arc<[crate::db::ResolvedDep]> = Arc::from(resolved);
        match self.dep_inputs.get(&file).copied() {
            Some(fd) => {
                fd.set_deps(&mut self.db).to(arc);
            }
            None => {
                let fd = crate::db::FileDeps::new(&self.db, arc);
                self.dep_inputs.insert(file, fd);
                // Keyset grew: republish the workspace-wide map.
                self.republish_file_deps_map();
            }
        }
    }

    /// The `FileDeps` salsa input for `file`, provisioned empty if `file` was
    /// never indexed (e.g. it was only resolved as someone else's dep target).
    /// Read by the host `resolve()` bridge and the tracked resolution queries.
    pub(crate) fn file_deps(&mut self, file: FileId) -> crate::db::FileDeps {
        if let Some(fd) = self.dep_inputs.get(&file).copied() {
            return fd;
        }
        let fd = crate::db::FileDeps::new(&self.db, Arc::from(Vec::new()));
        self.dep_inputs.insert(file, fd);
        // Keyset grew: republish the workspace-wide map.
        self.republish_file_deps_map();
        fd
    }

    /// Evicts a file (deleted on disk) from the workspace index.
    pub fn remove_workspace_file(&mut self, file: FileId) {
        self.workspace.remove(file);
        self.forget_file(file);
    }

    /// Drops the salsa `SourceText` input for `file` (e.g. deleted on disk).
    /// Parsing is memoized by salsa on the input itself, not by this host, so
    /// this only forgets the host's `FileId -> SourceText` binding; the next
    /// `source_for` call re-provisions a fresh input from the VFS. The old
    /// `SourceText` input and its memoized `parsed`/`file_symbols`/
    /// `raw_imports` results are NOT reclaimed from the salsa database —
    /// salsa has no input GC — so they're simply orphaned, unreachable but
    /// still resident. This is intentional for v2a and weaker reclamation
    /// than the pre-v2a parse cache; bounding this growth (explicit input
    /// removal, an LRU, ...) is a v2b concern.
    pub fn forget_file(&mut self, file: FileId) {
        self.sources.remove(&file);
        // Keyset shrank: republish the workspace-wide file_srcs map so the
        // removed file's key disappears from `Workspace.file_srcs`.
        self.republish_file_srcs();
        // Also drop the file's `FileDeps` input and republish the workspace-wide
        // map so the removed file's key disappears from `Workspace.file_deps`
        // (the keyset shrank — `republish_file_deps_map` recomputes from the
        // current `dep_inputs` keyset, which is correct for the shrink case).
        self.dep_inputs.remove(&file);
        self.republish_file_deps_map();
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

    /// Disclosure (WPP) diagnostics for `file`. v3a stub: empty until the
    /// interpreter lands (Task A1 replaces the body). Never panics.
    pub fn disclosure_diagnostics(&mut self, _file: FileId) -> Vec<Diagnostic> {
        Vec::new()
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

    #[test]
    fn index_file_publishes_resolved_dep_edges() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Foo.compact"), "module Foo {}").unwrap();
        let main = dir.path().join("main.compact");
        std::fs::write(&main, "import \"Foo\";\ninclude \"missing\";").unwrap();

        let mut host = AnalysisHost::new();
        let foo = host.vfs_mut().file_id(&dir.path().join("Foo.compact"));
        let main_id = host.vfs_mut().file_id(&main);
        host.index_file(foo);
        host.index_file(main_id);

        let fd = host.file_deps(main_id);
        let deps = crate::db::FileDeps::deps(fd, &host.db);
        // The string-path import resolves to Foo; the include does not resolve.
        let foo_dep = deps.iter().find(|d| d.raw == "Foo").unwrap();
        assert!(matches!(foo_dep.target, Some((f, _)) if f == foo));
        let miss = deps.iter().find(|d| d.raw == "missing").unwrap();
        assert!(miss.is_include && miss.target.is_none());
    }

    #[test]
    fn register_stdlib_publishes_workspace_singleton() {
        let dir = tempfile::tempdir().unwrap();
        let stdlib_path = dir.path().join("std.compact");
        std::fs::write(&stdlib_path, "module Std {}").unwrap();

        let mut host = AnalysisHost::new();
        // Before registration, the singleton is provisioned empty (no stdlib).
        let ws0 = host.workspace();
        assert!(crate::db::Workspace::stdlib(ws0, &host.db).is_none());

        let stdlib_id = host.register_stdlib(&stdlib_path);
        let ws1 = host.workspace();
        // Same handle across calls, now carrying the registered stdlib file.
        assert!(ws0 == ws1);
        let stdlib = crate::db::Workspace::stdlib(ws1, &host.db);
        assert!(matches!(stdlib, Some((f, _)) if f == stdlib_id));
    }

    /// Cross-file resolution memo reuse (Task 11, step 1): editing the
    /// *importing* file's circuit body must not touch the *imported* file's
    /// `item_tree` memo at all. This is cross-FILE isolation — a completely
    /// different mechanism from same-file backdating (`item_tree` firewall):
    /// editing main.compact never changes lib.compact's `SourceText` salsa
    /// input, so lib's `parsed`/`item_tree` queries never even re-run; the
    /// memoized `Arc` from the first computation survives untouched.
    #[test]
    fn editing_importer_body_does_not_reresolve_imported_file() {
        let dir = tempfile::tempdir().unwrap();
        // The identifier import `import L;` resolves via `find_source_pathname`,
        // which requires the disk filename to match the imported identifier
        // exactly (`L.compact`) — same rule `identifier_import_resolves_across_files`
        // in resolve.rs exercises with `Foo`/`Foo.compact`.
        let lib_path = dir.path().join("L.compact");
        std::fs::write(&lib_path, "module L { export circuit ex(): [] {} }").unwrap();

        let main_path = dir.path().join("main.compact");
        let main_v1 = "import L;\ncircuit main(): [] { ex(); }";
        std::fs::write(&main_path, main_v1).unwrap();

        let mut host = AnalysisHost::new();
        let lib_file = host.vfs_mut().file_id(&lib_path);
        let main_file = host.vfs_mut().file_id(&main_path);
        host.vfs_mut()
            .set_overlay(main_file, main_v1.to_string(), 1);

        // Resolve `ex` from main into lib.
        let call_offset = main_v1.find("ex();").unwrap() as u32;
        let pos1 = crate::resolve::FilePosition {
            file: main_file,
            offset: crate::TextSize::new(call_offset),
        };
        let def1 = host.resolve(pos1).expect("ex resolves across files");
        match def1 {
            crate::Definition::Item { file, .. } => assert_eq!(file, lib_file),
            crate::Definition::Local { .. } => panic!("expected a cross-file item"),
        }

        // Capture lib's item_tree Arc before the edit.
        let lib_src = host
            .source_for(lib_file)
            .expect("lib was provisioned as main's import dep target");
        let lib_tree_before = crate::db::item_tree(host.db_ref(), lib_src);

        // Edit main's circuit BODY only — its import of L is untouched.
        let main_v2 = "import L;\ncircuit main(): [] { ex(); ex(); }";
        host.vfs_mut()
            .set_overlay(main_file, main_v2.to_string(), 2);

        // Re-resolve `ex` from the edited main.
        let call_offset2 = main_v2.find("ex();").unwrap() as u32;
        let pos2 = crate::resolve::FilePosition {
            file: main_file,
            offset: crate::TextSize::new(call_offset2),
        };
        let def2 = host
            .resolve(pos2)
            .expect("ex still resolves across files after the body edit");
        assert_eq!(def1, def2);

        // lib's item_tree Arc must be pointer-identical: lib's SourceText
        // input never changed, so its item_tree memo was never re-run.
        let lib_src_after = host.source_for(lib_file).unwrap();
        let lib_tree_after = crate::db::item_tree(host.db_ref(), lib_src_after);
        assert!(
            std::sync::Arc::ptr_eq(&lib_tree_before, &lib_tree_after),
            "editing main's body must not re-run lib's item_tree (cross-file isolation)"
        );
    }

    /// Trivia-only edit does not re-resolve, downstream firewall (Task 11,
    /// step 2). A comment-only edit to a file must leave `resolve()`'s
    /// answer equal, and must be observable as a *downstream* backdate: NOT
    /// on `item_tree` itself (it is a direct dependent of the `no_eq`
    /// `parsed` query, so it always re-executes and reallocates its `Arc` —
    /// see `trivia_only_edit_backdates_item_tree` in `db.rs`), but on
    /// `file_symbols`, which depends on `item_tree`'s *value* and therefore
    /// skips re-execution (same `Arc`) once `item_tree`'s change is
    /// backdated.
    #[test]
    fn trivia_only_edit_does_not_reresolve_downstream() {
        let text1 = "export circuit c(): [] {}\ncircuit main(): [] { c(); }";
        let (mut host, file) = host_with(text1);

        let call_offset = text1.find("c();").unwrap() as u32;
        let pos1 = crate::resolve::FilePosition {
            file,
            offset: crate::TextSize::new(call_offset),
        };
        let def1 = host.resolve(pos1).expect("c resolves");

        let src_before = host.source_for(file).unwrap();
        let symbols_before = crate::db::file_symbols(host.db_ref(), src_before);

        // Comment-only edit: items are byte-identical, only trivia changed.
        let text2 = "export circuit c(): [] {}\ncircuit main(): [] { c(); } // note";
        host.vfs_mut().set_overlay(file, text2.to_string(), 2);

        let call_offset2 = text2.find("c();").unwrap() as u32;
        let pos2 = crate::resolve::FilePosition {
            file,
            offset: crate::TextSize::new(call_offset2),
        };
        let def2 = host
            .resolve(pos2)
            .expect("c still resolves after the trivia-only edit");
        assert_eq!(def1, def2, "resolve() must return an equal Definition");

        let src_after = host.source_for(file).unwrap();
        let symbols_after = crate::db::file_symbols(host.db_ref(), src_after);
        assert!(
            std::sync::Arc::ptr_eq(&symbols_before, &symbols_after),
            "file_symbols must backdate (same Arc) across a trivia-only edit \
             — the firewall is observable downstream of item_tree, not on it"
        );
    }
}
