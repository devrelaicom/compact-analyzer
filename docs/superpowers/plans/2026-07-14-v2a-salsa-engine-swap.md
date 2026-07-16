# v2a — Salsa Engine Swap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `analyzer-core`'s three hand-rolled memo layers (the `Arc`-pointer-keyed parse cache, the generation-keyed `source_path_cache`, and the dirty-flagged workspace aggregate) with a salsa 0.28 database, **preserving every existing user-facing behavior** — no new features, same public `AnalysisHost` API, same test outcomes.

**Architecture:** `AnalysisHost` keeps owning the `Vfs` (path interning + overlay/disk resolution — the impure "outer loop" that defines inputs) and gains a `salsa::Database`. File text becomes a salsa **input** (`SourceText`); parsing, line-indexing, item-tree extraction, per-file symbol/import extraction become salsa **tracked queries**; disk-dependent import resolution stays in the mutable host layer and feeds a per-file **resolved-deps input** so queries remain pure functions of inputs. The host's public query methods (`analyze`, `index_file`, `workspace_symbols`, `dependents_of`, …) keep their names and return types and delegate to salsa.

**Tech Stack:** Rust (edition 2024, rustc 1.90+), `salsa = "0.28"`, `rowan` CST via `compactp_*` crates, `text-size` offsets.

## Global Constraints

- **Behavior-preserving:** v2a introduces **zero** new user-facing behavior. The existing test suite + corpus smoke are the oracle; a task is done only when they stay green.
- **No LSP types in `analyzer-core`:** byte offsets (`TextSize`/`TextRange`) only; protocol conversion stays in the `compact-analyzer` binary (`crates/analyzer-core/src/lib.rs:4-5`).
- **Analysis is single-threaded:** the host has no concurrent consumer (`crates/compact-analyzer/src/server.rs:265-266`). Do **not** adopt salsa's cross-thread `Cancelled`/snapshot mechanism in v2a — preserve the cooperative `should_continue` polling exactly.
- **Salsa version pinned at `0.28`** (workspace dependency). API: `#[salsa::input]`, `#[salsa::tracked]`, `#[returns(clone)]`/`#[returns(deref)]`, `use salsa::Setter`, `#[salsa::db]`.
- **Never commit a `[patch.crates-io]` section** (repo standing rule; local compactp iteration only).
- Rust: `edition = "2024"`, `rust-version = "1.90"`, `license = "MIT"` (from `Cargo.toml [workspace.package]`).

---

## File Structure

- **Create** `crates/analyzer-core/src/db.rs` — the salsa layer: `Db` trait, `CompactDatabase`, the `SourceText` and `ResolvedDeps` inputs, and every tracked query (`parsed`, `file_symbols`, `raw_imports`).
- **Modify** `crates/analyzer-core/src/analysis.rs` — `AnalysisHost` loses its `cache`/`source_path_cache`/`source_path_cache_generation` fields, gains `db: CompactDatabase` + input-handle maps; its public methods delegate to `db.rs` queries. `FileAnalysis` type is unchanged.
- **Modify** `crates/analyzer-core/src/lib.rs` — add `mod db;` and re-export what the binary/ide need (nothing new expected; `AnalysisHost`/`FileAnalysis` stay the public surface).
- **Modify** `crates/analyzer-core/Cargo.toml` and root `Cargo.toml` — add `salsa = "0.28"`.
- **Modify** `crates/analyzer-core/src/workspace.rs` — `WorkspaceIndex` stays the aggregate holder, but its per-file inputs are now sourced from salsa query outputs (Task 6). No signature changes to `dependents`/`symbols_matching`/`files`.

Task boundaries below are drawn so a reviewer could reject one task while approving its neighbor. Each ends with `cargo test -p analyzer-core` (plus, at the end, the whole workspace) green.

---

## Task 1: Salsa dependency + database skeleton (spike)

De-risks the salsa 0.28 API against a throwaway query before any real porting.

**Files:**
- Modify: `Cargo.toml` (root, `[workspace.dependencies]`)
- Modify: `crates/analyzer-core/Cargo.toml`
- Create: `crates/analyzer-core/src/db.rs`
- Modify: `crates/analyzer-core/src/lib.rs:7-16` (add `mod db;`)

**Interfaces:**
- Produces: `crate::db::{Db, CompactDatabase}`; a throwaway `SourceText` input + `parsed_len` tracked fn used only by this task's test (removed/repurposed in Task 3).

- [ ] **Step 1: Add the dependency**

In root `Cargo.toml` under `[workspace.dependencies]`, add:

```toml
salsa = "0.28"
```

In `crates/analyzer-core/Cargo.toml` under `[dependencies]`, add:

```toml
salsa = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/analyzer-core/src/db.rs`:

```rust
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
pub struct SourceText {
    #[returns(clone)]
    pub text: Arc<str>,
}

/// Throwaway spike query — replaced by `parsed` in Task 3.
#[salsa::tracked(returns(clone))]
pub fn parsed_len<'db>(db: &'db dyn Db, src: SourceText) -> usize {
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
```

Add `mod db;` to `crates/analyzer-core/src/lib.rs` alongside the other `mod` lines (near line 7).

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p analyzer-core db::tests::db_boots_memoizes_and_recomputes`
Expected: FAIL to **compile** first (until the dep resolves), then PASS once salsa is wired — if it fails to compile on a salsa macro, fix the macro usage against the pinned 0.28 API before proceeding. This is the spike's whole purpose.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p analyzer-core db::tests::db_boots_memoizes_and_recomputes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/analyzer-core/Cargo.toml crates/analyzer-core/src/db.rs crates/analyzer-core/src/lib.rs
git commit -m "feat(v2a): add salsa 0.28 and boot the database skeleton"
```

---

## Task 2: `SourceText` input provisioning from the VFS

Give the host a way to lazily create/update a file's salsa input from VFS content, keyed by `FileId`, with an unchanged-content fast path (mirrors today's `Arc::as_ptr` cache key).

**Files:**
- Modify: `crates/analyzer-core/src/analysis.rs:30-71` (fields + `new`)
- Test: `crates/analyzer-core/src/analysis.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `db::{CompactDatabase, SourceText}` (Task 1); `Vfs::read` returning `Arc<str>` (`vfs.rs:94`).
- Produces: `AnalysisHost::source_for(&mut self, file: FileId) -> Option<SourceText>` — provisions/updates the input, `None` if the file is unreadable.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `analysis.rs`:

```rust
#[test]
fn source_for_updates_input_on_edit_only() {
    use crate::db::parsed_len;
    let (mut host, file) = host_with("abcd");
    let src1 = host.source_for(file).unwrap();
    assert_eq!(parsed_len(&host.db, src1), 4);

    // Same content re-provisioned → same input handle, no revision churn.
    let src2 = host.source_for(file).unwrap();
    assert_eq!(src1, src2);

    // Edit → same handle, new text visible to queries.
    host.vfs_mut().set_overlay(file, "xyz".to_string(), 2);
    let src3 = host.source_for(file).unwrap();
    assert_eq!(src1, src3);
    assert_eq!(parsed_len(&host.db, src3), 3);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core source_for_updates_input_on_edit_only`
Expected: FAIL — `no method named source_for` / `no field db`.

- [ ] **Step 3: Write minimal implementation**

In `analysis.rs`, change the struct + `new` (drop the old parse `cache`; keep the rest for now — other caches are removed in later tasks):

```rust
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
    source_path_cache: HashMap<(PathBuf, String), Option<PathBuf>>,
    source_path_cache_generation: u64,
}
```

Update `new` to initialize `db: crate::db::CompactDatabase::default()` and `sources: HashMap::new()`, and remove the `cache` initializer. Add the method:

```rust
impl AnalysisHost {
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
}
```

Make `db` visible to the test (it is the same module, so private is fine).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p analyzer-core source_for_updates_input_on_edit_only`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/analysis.rs
git commit -m "feat(v2a): provision per-file SourceText inputs from the VFS"
```

---

## Task 3: Port `parsed` to a tracked query; back `analyze` with salsa

Move parsing/line-index/item-tree into salsa and delete the hand-rolled parse cache. `FileAnalysis` and `analyze`'s signature/return are unchanged.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (replace `parsed_len` with `parsed`)
- Modify: `crates/analyzer-core/src/analysis.rs:175-193` (`analyze`)

**Interfaces:**
- Consumes: `db::{Db, SourceText}`; `FileAnalysis` (unchanged, `analysis.rs:22-28`).
- Produces: `db::parsed<'db>(db: &'db dyn Db, src: SourceText) -> FileAnalysis`; `AnalysisHost::analyze(&mut self, FileId) -> Option<FileAnalysis>` (signature unchanged).

- [ ] **Step 1: Write the failing test**

Add to `db.rs` tests (import `FileAnalysis` via `crate::FileAnalysis`):

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core parsed_reuses_memo_across_unchanged_calls`
Expected: FAIL — `parsed` not found.

- [ ] **Step 3: Write minimal implementation**

In `db.rs`, delete `parsed_len` and its spike test, and add:

```rust
use compactp_ast::AstNode as _;
use crate::FileAnalysis;
use crate::item_tree::ItemTree;
use crate::line_index::LineIndex;

/// Parse + derive everything M1 knows about one file. Memoized on the
/// `SourceText` input; recomputed only when that file's text changes.
#[salsa::tracked(returns(clone))]
pub fn parsed<'db>(db: &'db dyn Db, src: SourceText) -> FileAnalysis {
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
```

`item_tree`/`line_index` are currently private modules — make them visible to `db.rs` by ensuring `mod item_tree;`/`mod line_index;` are crate-visible (they already are within the crate; `crate::item_tree::ItemTree` resolves).

In `analysis.rs`, replace the body of `analyze`:

```rust
pub fn analyze(&mut self, file: FileId) -> Option<FileAnalysis> {
    let src = self.source_for(file)?;
    Some(crate::db::parsed(&self.db, src))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core`
Expected: PASS — including the pre-existing oracle tests `unchanged_content_hits_the_cache`, `edit_invalidates_the_cache`, `forget_file_forces_recompute`, `valid_source_has_no_diagnostics`, `broken_source_reports_expected_colon`, `tree_is_lossless`, `unreadable_file_returns_none`.

> Note: `forget_file_forces_recompute` asserts a *fresh* diagnostics `Arc` after `forget_file`. Under salsa the memo is keyed on the input, not dropped by `forget_file`, so this test's assumption changes. Update it in Step 5 below to assert *value* equality (recompute is now transparent) rather than pointer inequality — this is a behavior-preserving change at the API level (a dropped disk file is handled by Task 5's eviction, not by parse-cache dropping).

- [ ] **Step 5: Adjust the one memo-semantics test, re-run, commit**

Edit `forget_file_forces_recompute` to assert `assert_eq!(first.diagnostics, second.diagnostics)` (value equality) and rename it to `forget_file_is_transparent_to_parse`. Re-run `cargo test -p analyzer-core` (PASS), then:

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/analysis.rs
git commit -m "feat(v2a): back analyze() with a salsa tracked parse query; drop the Arc-ptr parse cache"
```

---

## Task 4: Derived per-file queries — symbols and raw imports

Expose the item-tree-derived data that `index_file` needs as tracked queries, so the workspace layer consumes salsa outputs instead of re-walking trees ad hoc.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs`
- Test: `crates/analyzer-core/src/db.rs`

**Interfaces:**
- Consumes: `db::parsed`; `ItemTree`/`Symbol`/`SymbolKind` (`item_tree.rs`); `compactp_ast::SourceFile`.
- Produces:
  - `db::file_symbols<'db>(db, src: SourceText) -> Arc<[(String, u32)]>` — non-empty-named symbols with their ItemTree index.
  - `db::raw_imports<'db>(db, src: SourceText) -> Arc<[RawDep]>` where `pub struct RawDep { pub raw: String, pub is_include: bool }` — import/include targets *before* filesystem resolution, with `CompactStandardLibrary` and in-scope-module imports already filtered out (the pure half of today's `index_file` loop, `analysis.rs:219-253`).

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core raw_imports_filters_stdlib_and_local_modules`
Expected: FAIL — `raw_imports` not found.

- [ ] **Step 3: Write minimal implementation**

Port the pure extraction out of `index_file` into `db.rs`:

```rust
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RawDep {
    pub raw: String,
    pub is_include: bool,
}

#[salsa::tracked(returns(clone))]
pub fn file_symbols<'db>(db: &'db dyn Db, src: SourceText) -> Arc<[(String, u32)]> {
    let tree = crate::db::parsed(db, src).item_tree;
    tree.symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.name.is_empty())
        .map(|(idx, s)| (s.name.clone(), idx as u32))
        .collect()
}

#[salsa::tracked(returns(clone))]
pub fn raw_imports<'db>(db: &'db dyn Db, src: SourceText) -> Arc<[RawDep]> {
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p analyzer-core raw_imports_filters_stdlib_and_local_modules file_symbols`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/db.rs
git commit -m "feat(v2a): expose file_symbols and raw_imports as tracked queries"
```

---

## Task 5: Rewrite `index_file` on the salsa queries; keep disk resolution in the host

`index_file` now reads pure data from salsa (`file_symbols`, `raw_imports`) and performs only the impure step — `find_source_pathname` disk probes + interning — in the mutable host, then writes the resolved edges into the workspace aggregate. Behavior (which edges/symbols land in the index) is identical.

**Files:**
- Modify: `crates/analyzer-core/src/analysis.rs:200-255` (`index_file`)
- Test: `crates/analyzer-core/tests` are covered by existing `lsp_workspace.rs`; add a focused unit test here.

**Interfaces:**
- Consumes: `db::{file_symbols, raw_imports, RawDep}`; `find_source_pathname` (`source_path.rs`); `WorkspaceIndex::set_file`/`remove` (`workspace.rs`).
- Produces: `AnalysisHost::index_file(&mut self, FileId)` (signature unchanged).

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core index_file_records_resolved_cross_file_dep`
Expected: it may already PASS if `index_file` is unchanged — that's fine, it is the behavior-preserving oracle. Proceed to rewrite the implementation and keep it green.

- [ ] **Step 3: Rewrite the implementation**

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core && cargo test -p compact-analyzer --test lsp_workspace`
Expected: PASS — cross-file navigation/workspace-symbol integration tests unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/analysis.rs
git commit -m "feat(v2a): rebuild index_file on salsa queries; keep disk resolution in the host"
```

---

## Task 6: Confirm the workspace aggregate + source-path memo, then delete dead cache state

The `WorkspaceIndex` aggregate (symbol match, dependents) stays host-maintained — it is an incrementally-updated index, not a pure query, and Task 5 already feeds it from salsa outputs. The `source_path_cache` memoizes impure disk probes; it is **kept** (salsa cannot own disk existence) but is now the *only* remaining hand-rolled cache, so re-verify its three invalidation hooks still hold after the `cache` field removal.

**Files:**
- Modify: `crates/analyzer-core/src/analysis.rs` (remove any now-dead references to the deleted `cache`; keep `source_path_cache*`, `forget_file`, `cached_source_pathname`)

**Interfaces:**
- Produces: no API change. `forget_file` no longer touches a parse cache (there is none); it now only clears `source_path_cache` (hook 2 of 3) and drops the file's `sources` entry so a re-provision re-reads the VFS.

- [ ] **Step 1: Update `forget_file` and re-run the invalidation oracles**

Rewrite `forget_file`:

```rust
/// Drops per-file salsa provisioning state (e.g. file deleted on disk) so the
/// next access re-reads the VFS, and clears the disk-probe memo (hook 2 of 3).
pub fn forget_file(&mut self, file: FileId) {
    self.sources.remove(&file);
    self.source_path_cache.clear();
}
```

- [ ] **Step 2: Run the source-path + workspace oracle tests**

Run: `cargo test -p analyzer-core && cargo test -p compact-analyzer --test lsp_workspace`
Expected: PASS. Confirm `cargo build -p analyzer-core` reports no dead-code warnings for a leftover `cache` field.

- [ ] **Step 3: Commit**

```bash
git add crates/analyzer-core/src/analysis.rs
git commit -m "refactor(v2a): retire the parse cache from forget_file; keep the disk-probe memo"
```

---

## Task 7: Add an incremental-reuse regression test (salsa-specific)

Prove the new engine actually reuses work — the property the swap exists to guarantee — so future edits to `db.rs` can't silently regress it.

**Files:**
- Test: `crates/analyzer-core/src/db.rs`

**Interfaces:**
- Consumes: `db::parsed`, `SourceText`, `salsa::Setter`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn editing_one_file_does_not_recompute_another() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Count parses via a side channel: re-parse only happens on a memo miss.
    static PARSES: AtomicUsize = AtomicUsize::new(0);

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
    let _ = &PARSES; // documents intent; ptr_eq is the actual assertion
}
```

- [ ] **Step 2: Run test to verify it fails, then passes**

Run: `cargo test -p analyzer-core editing_one_file_does_not_recompute_another`
Expected: PASS immediately (salsa reuses `a`'s memo). If it FAILS (`a` recomputed), the input wiring is wrong — investigate before proceeding.

- [ ] **Step 3: Commit**

```bash
git add crates/analyzer-core/src/db.rs
git commit -m "test(v2a): lock in cross-file incremental reuse"
```

---

## Task 8: Preserve cooperative cancellation; explicitly do not adopt salsa Cancelled

Confirm the single-threaded `should_continue` cancellation path is untouched by the swap. No salsa snapshot/`Cancelled` machinery in v2a.

**Files:**
- Inspect: `crates/compact-analyzer/src/server.rs:342-356,422-428` and `analyzer-ide` `*_cancellable` entry points; `analysis.rs::discover_and_index:292-304`.

**Interfaces:**
- No change. `discover_and_index(&mut self, roots, should_continue: &dyn Fn() -> bool)` keeps its cooperative poll.

- [ ] **Step 1: Run the cancellation-relevant integration tests**

Run: `cargo test -p compact-analyzer --test lsp_workspace --test lsp_navigation`
Expected: PASS — references/rename workspace-wide behavior and early-abandon on superseding messages unchanged.

- [ ] **Step 2: Confirm no salsa cancellation was introduced**

Run: `grep -rn "Cancelled\|snapshot\|par_iter" crates/analyzer-core/src`
Expected: no matches. If any appear, remove them — cross-thread salsa cancellation is out of scope for v2a (single-threaded host).

- [ ] **Step 3: Commit (if any doc/comment tidy-ups were needed)**

```bash
git add -A crates/analyzer-core/src crates/compact-analyzer/src
git commit -m "docs(v2a): note cooperative (non-salsa) cancellation is preserved" || echo "nothing to commit"
```

---

## Task 9: Full behavior-preserving verification + corpus smoke + latency sanity

The gate for v2a as a whole.

**Files:** none (verification only).

- [ ] **Step 1: Whole-workspace test suite**

Run: `cargo test --workspace`
Expected: PASS with the **same test count** as before v2a (no tests silently dropped; the one renamed test from Task 3 aside). Compare against the pre-v2a baseline captured on `main`.

- [ ] **Step 2: Lints**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean (matches the repo's CI bar in `README.md:200-201`).

- [ ] **Step 3: Corpus smoke**

Run the workspace corpus smoke (the spec §8 / M2b corpus harness — the existing test that indexes the ~486-file corpus). Locate it with `grep -rn "corpus" crates/ --include=*.rs` and run that test target.
Expected: no panics, no out-of-bounds spans — identical to pre-v2a.

- [ ] **Step 4: Keystroke-latency sanity**

Confirm no gross regression: re-analyzing one edited file in a large workspace must not re-parse the whole workspace. Task 7 already asserts cross-file reuse at the unit level; if a benchmark harness exists (`grep -rn "criterion\|bench" crates/`), run it and compare; otherwise record that Task 7's reuse guarantee is the standing evidence.

- [ ] **Step 5: Final commit / branch is ready for review**

```bash
git add -A
git commit -m "test(v2a): full-suite + corpus behavior-preserving verification" || echo "nothing to commit"
```

---

## Self-Review

- **Spec coverage:** v2a's spec done-bar is "every v1 feature + test suite + corpus smoke pass on the new engine, no keystroke-latency regression, zero new user-facing behavior." Tasks 3/5 preserve `analyze`/`index_file` semantics; Task 8 preserves cancellation; Task 9 runs the full suite + corpus + latency evidence. The salsa-up-front architecture (spec §4) is realized by Tasks 1–5. ✓
- **Placeholder scan:** every step has concrete code or an exact command + expected output. The one deliberately open item — locating the corpus test target (Task 9 Step 3) and any benchmark (Step 4) — is expressed as an exact `grep` to run, not a vague instruction. ✓
- **Type consistency:** `SourceText`, `FileAnalysis`, `RawDep`, `file_symbols`, `raw_imports`, `parsed` are named identically across tasks; `source_for` returns `Option<SourceText>`; `analyze`/`index_file`/`forget_file` keep their pre-v2a signatures. ✓
- **Behavior-preserving carve-outs made explicit:** the `source_path_cache` is intentionally *kept* (impure disk probes can't be salsa queries), and cross-thread salsa cancellation is intentionally *excluded* (single-threaded host). Both are documented so a reviewer doesn't read them as omissions.

> **Note for the executing agent:** Task 1 is a genuine spike — if any salsa 0.28 macro detail here (e.g. `#[returns(clone)]`, the blanket `Db` impl, `salsa::Storage` field) does not compile against the pinned version, fix it against the actual 0.28 API and carry the correction forward into Tasks 3–7, which reuse the same patterns.
