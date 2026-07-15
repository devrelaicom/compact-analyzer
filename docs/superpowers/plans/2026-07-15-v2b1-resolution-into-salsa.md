# v2b.1 — Resolution → Salsa Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lift `analyzer-core`'s name resolution (`resolve.rs`, currently imperative `&mut self` host methods that do disk I/O) into salsa tracked queries over a pure cross-file input, making the tracked def-map the single canonical resolver — **behavior-preserving**, with every existing M2 test as the oracle.

**Architecture:** The host keeps all impure work (VFS interning, `find_source_pathname` disk probes) — it already does this in `index_file` — and publishes each file's resolved `raw → (FileId, SourceText)` import/include edges (plus the stdlib handle) into salsa **inputs** (`FileDeps` per file, `Workspace` singleton). Resolution logic moves into tracked queries / pure `db`-taking helpers that read `item_tree(src)` and those inputs, so no query performs I/O. The host's public methods (`resolve`, `nav_info`, `def_name`, `resolution_diagnostics`, `scope_bindings_at`) keep their names/signatures and delegate to the tracked layer via a thin `FileId ↔ SourceText` bridge. The old imperative resolution internals are deleted outright (pre-release: no shims).

**Tech Stack:** Rust (edition 2024, rustc 1.90+), `salsa = "0.28"`, `rowan`/`compactp_*` CST, `text-size` offsets.

## Global Constraints

- **Behavior-preserving:** zero new user-facing behavior. The existing M2 test suite + corpus smoke are the oracle; a task is done only when they stay green. (Program spec §3 done-bar; foundation spec §3.)
- **Backdating firewall:** the def-map depends on a `PartialEq`-able `item_tree(src) -> Arc<ItemTree>` query, **not** on `parsed` (which embeds the green tree and can never backdate). Trivia-only edits must not re-resolve. (Foundation spec §3; v2a carry-forward.)
- **Impurity stays in the host:** no salsa tracked query calls the VFS, probes disk, or interns a path. All cross-file targets arrive pre-resolved via the `FileDeps`/`Workspace` inputs. (Foundation spec §3 seam.)
- **Input granularity:** per-file *text* stays on the existing per-file `SourceText` inputs; the new cross-file inputs carry only the *structural* graph, changing only on structural edits (import/include added/removed, target file appears/disappears) — never on in-body keystrokes. A monolithic all-content input is rejected. (Foundation spec §3.)
- **No LSP types in `analyzer-core`:** byte offsets (`TextSize`/`TextRange`) only. (`lib.rs:1-5`.)
- **Single-threaded** analysis with cooperative `should_continue` cancellation. Do **not** adopt salsa cross-thread `Cancelled`/snapshots. (v2a precedent.)
- **Salsa pinned at `0.28`.** API: `#[salsa::input]`, `#[salsa::tracked]`, `#[returns(clone)]`, `use salsa::Setter`, `#[salsa::db]`. Elide lifetimes on tracked fns (`fn f(db: &dyn Db, …)`, never `<'db>`) — `clippy::needless_lifetimes` fails under `-D warnings` (v2a finding).
- **Never commit a `[patch.crates-io]` section** (repo standing rule).
- Rust: `edition = "2024"`, `rust-version = "1.90"`, `license = "MIT"`.

---

## New salsa surface (defined once; Task 1 spikes it, later tasks fill it)

All added to `crates/analyzer-core/src/db.rs`. Exact macro details are validated by the Task 1 spike against the real 0.28 API — if a shape below does not compile, fix it there and carry the correction forward (same discipline as v2a Task 1).

```rust
// A cross-file resolved edge: one import/include target after host-side disk
// resolution. `target: None` means the raw string did not resolve to a file.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ResolvedDep {
    pub raw: String,
    pub is_include: bool,
    pub target: Option<(crate::FileId, SourceText)>,
}

// Per-file cross-file environment, set by the host from index_file's
// resolution pass. One handle per FileId, stored host-side like `sources`.
#[salsa::input]
pub struct FileDeps {
    #[returns(clone)]
    pub deps: Arc<[ResolvedDep]>,
}

// Workspace singleton: the stdlib file's (FileId, SourceText), or None.
#[salsa::input]
pub struct Workspace {
    #[returns(clone)]
    pub stdlib: Option<(crate::FileId, SourceText)>,
}

// Standalone item-tree projection — PartialEq-able, so trivia-only edits
// backdate and downstream resolution/type queries skip re-execution.
#[salsa::tracked(returns(clone))]
pub fn item_tree(db: &dyn Db, src: SourceText) -> Arc<ItemTree> { … }
```

`FileId` is a plain `Copy + Eq + Hash` value (`vfs.rs:14`), so it may appear inside input/query values directly; only text/graph *entities* are salsa inputs.

**The `FileId ↔ SourceText` bridge (host side).** `Definition::Item { file: FileId, .. }` carries a `FileId`; tracked queries need the matching `SourceText` to read a target's `item_tree`. The host already owns `sources: HashMap<FileId, (usize, SourceText)>`. Add one helper the public bridge methods use:

```rust
// analysis.rs — impure: provisions the input if needed. Used only by the thin
// host wrappers (resolve/nav_info/def_name/resolution_diagnostics), never inside
// a tracked query.
fn src_of(&mut self, file: FileId) -> Option<crate::db::SourceText> { self.source_for(file) }
```

---

## File Structure

- **Modify** `crates/analyzer-core/src/db.rs` — add `item_tree` query, `ResolvedDep`, `FileDeps`, `Workspace` inputs, and (Tasks 4–9) the tracked resolution queries + pure `db`-helpers.
- **Modify** `crates/analyzer-core/src/item_tree.rs` — derive `PartialEq, Eq` on `Symbol` and `ItemTree`.
- **Modify** `crates/analyzer-core/src/resolve.rs` — the `impl AnalysisHost` resolution methods become thin bridges delegating to `db.rs`; pure free helpers (`scope_bindings_at`, `resolve_local_name`, `single_top_level_module`, `enclosing_module`, diagnostic builders, `ident_at_offset`, `import_specifier_alias`) move to `db.rs` or stay as pure fns callable from queries; imperative internals deleted.
- **Modify** `crates/analyzer-core/src/analysis.rs` — `index_file` also publishes `FileDeps`; `register_stdlib` publishes `Workspace`; add `dep_inputs`/`workspace_input` host fields + the `src_of` bridge.
- **Modify** `crates/analyzer-core/src/lib.rs` — re-exports unchanged (public surface is stable: `Binding`, `Definition`, `FilePosition`, `scope_bindings_at` stay).

Task boundaries are drawn so a reviewer could reject one task while approving its neighbor. Each ends with `cargo test -p analyzer-core` green (plus named integration tests where cross-crate behavior is touched).

---

## Task 1: Spike the cross-file salsa surface

De-risk the 0.28 API for (a) an input holding another input inside a value (`ResolvedDep.target: Option<(FileId, SourceText)>`) and (b) a tracked query that reads a *second* file's `item_tree` via a `FileDeps` entry — proving cross-file resolution is pure. Throwaway; replaced by real queries in later tasks.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs`

**Interfaces:**
- Produces: `ResolvedDep`, `FileDeps`, `Workspace` (kept); a throwaway `spike_cross_file` tracked fn (deleted in Task 5).

- [ ] **Step 1: Write the failing test**

Add to `db.rs` tests:

```rust
#[test]
fn spike_reads_another_files_item_tree_purely() {
    use salsa::Setter as _;
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
```

Add a **test-only** `FileId` constructor (real `FileId` is minted only by the `Vfs`; the spike needs a value). In `vfs.rs`, under `#[cfg(test)]`:

```rust
impl FileId {
    #[cfg(test)]
    pub(crate) fn from_raw_for_test(n: u32) -> FileId { FileId(n) }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core spike_reads_another_files_item_tree_purely`
Expected: FAIL to compile — `FileDeps`/`ResolvedDep`/`spike_cross_file` not found.

- [ ] **Step 3: Write minimal implementation**

In `db.rs`, add `ResolvedDep`, `FileDeps`, `Workspace` (from the "New salsa surface" section above) and the spike:

```rust
#[salsa::tracked(returns(clone))]
pub fn spike_cross_file(db: &dyn Db, fd: FileDeps, raw: String) -> Option<String> {
    let dep = fd.deps(db).iter().find(|d| d.raw == raw)?.clone();
    let (_, target_src) = dep.target?;
    let tree = crate::db::item_tree(db, target_src);
    tree.top_level().next().map(|(_, s)| s.name.clone())
}
```

This forces the `item_tree` query to exist minimally too — add it now (full version + `PartialEq` derives land in Task 2; a temporary body that rebuilds from `parsed` is fine here):

```rust
#[salsa::tracked(returns(clone))]
pub fn item_tree(db: &dyn Db, src: SourceText) -> Arc<ItemTree> {
    crate::db::parsed(db, src).item_tree
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p analyzer-core spike_reads_another_files_item_tree_purely`
Expected: PASS. If any salsa macro shape (input-holding-input, multi-arg tracked fn) does not compile against 0.28, fix it here and note the corrected shape for later tasks.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/vfs.rs
git commit -m "feat(v2b.1): spike the cross-file salsa surface (FileDeps/Workspace/item_tree)"
```

---

## Task 2: `item_tree` backdating firewall — `PartialEq` derives + standalone query

Make `item_tree` a real backdating firewall: derive `PartialEq, Eq` on `Symbol`/`ItemTree` so a trivia-only edit (which leaves items unchanged) backdates and skips re-resolution. Re-point `file_symbols` at `item_tree` so the whole derived chain shares the firewall.

**Files:**
- Modify: `crates/analyzer-core/src/item_tree.rs:26-46`
- Modify: `crates/analyzer-core/src/db.rs` (`file_symbols` reads `item_tree`; add backdating test)

**Interfaces:**
- Consumes: `parsed` (`db.rs`).
- Produces: `item_tree(db, src) -> Arc<ItemTree>` (final form); `Symbol: PartialEq + Eq`, `ItemTree: PartialEq + Eq`.

- [ ] **Step 1: Write the failing test**

Add to `db.rs` tests (a trivia-only edit must reuse the `item_tree` memo):

> **Salsa-mechanics correction (verified during execution).** `item_tree` is a *direct*
> dependent of the `no_eq` `parsed` query, so it re-executes on *every* edit and returns a
> freshly-allocated `Arc` — `Arc::ptr_eq` on `item_tree` itself is therefore never
> satisfiable. Backdating rewrites `item_tree`'s *revision stamp* (using the new
> `PartialEq`), not its stored `Arc`; the firewall is observable one hop **downstream**,
> where `file_symbols` skips re-execution. The test asserts `item_tree` *value*-equality
> (proves the derive salsa compares on) **and** `Arc::ptr_eq` on `file_symbols` (proves the
> firewall).

```rust
#[test]
fn trivia_only_edit_backdates_downstream_of_item_tree() {
    use salsa::Setter as _;
    let mut db = CompactDatabase::default();
    let src = SourceText::new(&db, Arc::from("export circuit c(): [] {}"));
    let t1 = crate::db::item_tree(&db, src);
    let s1 = crate::db::file_symbols(&db, src);
    // Add only a trailing comment: items are byte-identical.
    src.set_text(&mut db).to(Arc::from("export circuit c(): [] {} // note"));
    let t2 = crate::db::item_tree(&db, src);
    let s2 = crate::db::file_symbols(&db, src);
    // item_tree re-runs (fresh Arc) but its VALUE is Eq → salsa backdates its revision.
    assert_eq!(*t1, *t2);
    // Downstream firewall: file_symbols skips re-execution → SAME Arc allocation.
    assert!(Arc::ptr_eq(&s1, &s2));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core trivia_only_edit_backdates_downstream_of_item_tree`
Expected: FAIL — with `item_tree` still `no_eq` (or `ItemTree` lacking `PartialEq`), salsa cannot backdate `item_tree`'s revision, so `file_symbols` re-executes and the `Arc::ptr_eq(&s1, &s2)` assertion fails.

- [ ] **Step 3: Write minimal implementation**

In `item_tree.rs`, add `PartialEq, Eq` to both derives:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol { /* unchanged fields */ }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemTree { pub symbols: Vec<Symbol> }
```

(`SymbolKind` already derives `PartialEq, Eq`; `TextRange`/`String`/`bool`/`Option<u32>` are all `Eq` — the derive is total.)

In `db.rs`, keep the real `item_tree` body and re-point `file_symbols`:

```rust
#[salsa::tracked(returns(clone))]
pub fn item_tree(db: &dyn Db, src: SourceText) -> Arc<ItemTree> {
    crate::db::parsed(db, src).item_tree
}

#[salsa::tracked(returns(clone))]
pub fn file_symbols(db: &dyn Db, src: SourceText) -> Arc<[(String, u32)]> {
    let tree = crate::db::item_tree(db, src); // was: parsed(db, src).item_tree
    tree.symbols.iter().enumerate()
        .filter(|(_, s)| !s.name.is_empty())
        .map(|(idx, s)| (s.name.clone(), idx as u32))
        .collect()
}
```

> Note the backdating chain: `parsed` stays `no_eq` (it embeds the green tree — can never backdate). `item_tree` projects the green tree down to the `Eq`-able `ItemTree`, so it *is* the firewall: on a trivia-only edit `parsed` re-runs but `item_tree` backdates, and `file_symbols` + all Task 4–9 resolution queries downstream of `item_tree` skip re-execution.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core`
Expected: PASS — new backdating test plus all existing `db`/`item_tree` tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/item_tree.rs crates/analyzer-core/src/db.rs
git commit -m "feat(v2b.1): make item_tree a PartialEq backdating firewall; file_symbols reads it"
```

---

## Task 3: Host publishes the cross-file inputs (`FileDeps`, `Workspace`)

`index_file` already resolves each raw dep to a `FileId` to build the workspace graph. Extend it to also publish those edges as a `FileDeps` input (raw → `(FileId, SourceText)`), and make `register_stdlib` publish `Workspace`. This is the impure→input boundary; resolution queries (Tasks 5–9) read these.

**Files:**
- Modify: `crates/analyzer-core/src/analysis.rs:32-70` (fields + `new`), `:109-117` (`register_stdlib`), `:197-216` (`index_file`)

**Interfaces:**
- Consumes: `db::{FileDeps, Workspace, ResolvedDep, raw_imports, SourceText}`; `find_source_pathname`; `source_for`.
- Produces: `AnalysisHost::file_deps(&mut self, FileId) -> db::FileDeps` (provisions/updates); `Workspace` handle on the host set by `register_stdlib`; `index_file` unchanged in signature/observable graph behavior.

- [ ] **Step 1: Write the failing test**

```rust
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
    let deps = crate::db::FileDeps::deps(&fd, &host.db);
    // The string-path import resolves to Foo; the include does not resolve.
    let foo_dep = deps.iter().find(|d| d.raw == "Foo").unwrap();
    assert!(matches!(foo_dep.target, Some((f, _)) if f == foo));
    let miss = deps.iter().find(|d| d.raw == "missing").unwrap();
    assert!(miss.is_include && miss.target.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core index_file_publishes_resolved_dep_edges`
Expected: FAIL — no method `file_deps`.

- [ ] **Step 3: Write minimal implementation**

Add host fields (init in `new`):

```rust
dep_inputs: HashMap<FileId, crate::db::FileDeps>,
workspace_input: Option<crate::db::Workspace>,
```

Add the provisioning + update in `index_file` (after it computes `deps`), and a `file_deps` accessor. Reuse the resolution loop already in `index_file`, capturing `(raw, is_include, target)` alongside the `BTreeSet<FileId>` it already builds:

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
    let mut edges = BTreeSet::new();
    let mut resolved: Vec<crate::db::ResolvedDep> = Vec::new();
    if let Some(from_dir) = from_dir {
        for dep in raw_deps.iter() {
            let target = crate::find_source_pathname(&from_dir, &search, &dep.raw)
                .map(|path| {
                    let fid = self.vfs_mut().file_id(&path);
                    (fid, self.source_for(fid).expect("resolved path is readable"))
                });
            if let Some((fid, _)) = target { edges.insert(fid); }
            resolved.push(crate::db::ResolvedDep {
                raw: dep.raw.clone(), is_include: dep.is_include, target,
            });
        }
    }
    self.publish_file_deps(file, resolved);
    self.workspace.set_file(file, symbols, edges);
}
```

Add the publish helper (create-or-update the input, keyed like `sources`):

```rust
fn publish_file_deps(&mut self, file: FileId, resolved: Vec<crate::db::ResolvedDep>) {
    use salsa::Setter as _;
    let arc: std::sync::Arc<[crate::db::ResolvedDep]> = std::sync::Arc::from(resolved);
    match self.dep_inputs.get(&file).copied() {
        Some(fd) => { fd.set_deps(&mut self.db).to(arc); }
        None => { let fd = crate::db::FileDeps::new(&self.db, arc); self.dep_inputs.insert(file, fd); }
    }
}

/// Provisions FileDeps if absent (e.g. a file resolved but never indexed).
pub(crate) fn file_deps(&mut self, file: FileId) -> crate::db::FileDeps {
    if let Some(fd) = self.dep_inputs.get(&file).copied() { return fd; }
    let fd = crate::db::FileDeps::new(&self.db, std::sync::Arc::from(Vec::new()));
    self.dep_inputs.insert(file, fd);
    fd
}
```

In `register_stdlib`, publish `Workspace`:

```rust
pub fn register_stdlib(&mut self, path: &std::path::Path) -> FileId {
    use salsa::Setter as _;
    let file = self.vfs.file_id(path);
    self.stdlib = Some(file);
    let src = self.source_for(file);
    let stdlib = src.map(|s| (file, s));
    match self.workspace_input {
        Some(ws) => ws.set_stdlib(&mut self.db).to(stdlib),
        None => { self.workspace_input = Some(crate::db::Workspace::new(&self.db, stdlib)); }
    }
    file
}

/// The Workspace singleton, provisioned empty if the stdlib was never registered.
pub(crate) fn workspace(&mut self) -> crate::db::Workspace {
    if let Some(ws) = self.workspace_input { return ws; }
    let ws = crate::db::Workspace::new(&self.db, None);
    self.workspace_input = Some(ws);
    ws
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core index_file_publishes_resolved_dep_edges && cargo test -p analyzer-core`
Expected: PASS — new test plus every existing workspace/index test (`indexes_symbols_and_dependency_edges`, `eviction_removes_symbols_and_edges`, `index_file_records_resolved_cross_file_dep`).

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/analysis.rs
git commit -m "feat(v2b.1): publish resolved cross-file edges + stdlib as salsa inputs"
```

---

## Task 4: Move pure resolution helpers into the query layer

`scope_bindings_at`, `resolve_local_name`, `single_top_level_module`, `enclosing_module`, `ident_at_offset`, `import_specifier_alias`, `generic_definition`, `local_from_pattern_site`, and the diagnostic builders are already **pure free functions** over a `SyntaxNode`/`ItemTree`. They need no host and can be called from tracked queries as-is. This task relocates none of the logic — it only confirms they are `db`-free and makes them visible to `db.rs` (via `pub(crate)`), so Tasks 5–9 call them from queries.

**Files:**
- Modify: `crates/analyzer-core/src/resolve.rs` (widen visibility of the pure helpers to `pub(crate)`; no body changes)
- Test: existing `resolve.rs` local-resolution unit tests are the oracle (`resolves_circuit_param`, `resolves_const_with_shadowing`, `resolves_for_loop_binding_only_inside_body`, `resolves_tuple_pattern_binding`, `resolves_lambda_param`, `resolves_multi_const_binding`, …).

**Interfaces:**
- Produces (visibility only): `pub(crate) fn resolve_local_name(file, root, offset, name) -> Option<Definition>`; `pub(crate) fn single_top_level_module`, `enclosing_module`, `ident_at_offset`, `import_specifier_alias`, `unresolved_import_diag`, `unresolved_include_diag`, `module-mismatch` builder logic. `scope_bindings_at` stays `pub`.

- [ ] **Step 1: Widen visibility (no logic change)**

Change `fn resolve_local_name` → `pub(crate) fn`, and likewise for `single_top_level_module` (already `pub(crate)`), `enclosing_module`, `ident_at_offset`, `import_specifier_alias`, `generic_definition`, `local_from_pattern_site`, `unresolved_import_diag`, `unresolved_include_diag`, `searched_list`. Leave every body byte-for-byte unchanged.

- [ ] **Step 2: Run the local-resolution oracle**

Run: `cargo test -p analyzer-core resolve::tests`
Expected: PASS — no behavior changed; this is a visibility-only refactor confirming these helpers carry no host state.

- [ ] **Step 3: Commit**

```bash
git add crates/analyzer-core/src/resolve.rs
git commit -m "refactor(v2b.1): expose pure resolution helpers to the query layer (visibility only)"
```

---

## Task 5: Tracked file-scope resolution (`resolve_in_file` + include walk)

Port `resolve_in_file_scope` + `resolve_through_includes` into the query layer. **Transformation recipe (applies to Tasks 5–9):**
- `self.analyze(f)?.item_tree` → `crate::db::item_tree(db, src_f)` where `src_f` is the target's `SourceText` (from a `ResolvedDep.target` or the query's own `src`).
- `self.cached_source_pathname(from_dir, raw)` + `self.vfs_mut().file_id` → read the matching `ResolvedDep` from `fd.deps(db)`.
- `self.stdlib_file()` → `ws.stdlib(db)`.
- Recursion across includes stays a **plain `db`-taking helper** (not a `#[salsa::tracked]` fn) carrying the explicit `active: &mut Vec<FileId>` cycle guard, so the cyclic include graph never becomes a salsa cycle. It still benefits from `item_tree` memoization.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (add `resolve_in_file` query + `resolve_includes` helper; delete `spike_cross_file`)

**Interfaces:**
- Consumes: `item_tree`, `FileDeps`, `Workspace`, `enclosing_module`, `Definition`, `SymbolKind`.
- Produces:
  - `pub fn resolve_in_file(db: &dyn Db, file: FileId, src: SourceText, fd: FileDeps, ws: Workspace, offset: TextSize, name: &str) -> Option<Definition>` (tracked).
  - `pub(crate) fn resolve_includes(db: &dyn Db, file: FileId, src: SourceText, fd: FileDeps, name: &str, active: &mut Vec<FileId>) -> Option<Definition>` (plain helper).

- [ ] **Step 1: Write the failing test**

Drive it through the host (added in Task 7 the dispatcher will call this; here test the query directly with a hand-built `FileDeps`). Add to `db.rs` tests:

```rust
#[test]
fn resolve_in_file_finds_top_level_and_enclosing_module() {
    let db = CompactDatabase::default();
    let src = SourceText::new(&db, Arc::from(
        "circuit helper(): [] {}\nmodule M { circuit inner(): [] {} }"));
    let fd = FileDeps::new(&db, Arc::from(Vec::new()));
    let ws = Workspace::new(&db, None);
    // Top-level `helper` resolves from anywhere.
    let d = resolve_in_file(&db, crate::FileId::from_raw_for_test(0), src, fd, ws,
        0u32.into(), "helper");
    assert!(matches!(d, Some(Definition::Item { .. })));
    // A name that does not exist resolves to None.
    assert!(resolve_in_file(&db, crate::FileId::from_raw_for_test(0), src, fd, ws,
        0u32.into(), "nope").is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core resolve_in_file_finds_top_level_and_enclosing_module`
Expected: FAIL — `resolve_in_file` not found.

- [ ] **Step 3: Port the implementation**

In `db.rs`, delete `spike_cross_file` and its test, and add (transforming `resolve_in_file_scope`/`resolve_through_includes` bodies per the recipe; `enclosing_module`/`resolve::single_top_level_module` are called as pure fns):

```rust
#[salsa::tracked(returns(clone))]
pub fn resolve_in_file(
    db: &dyn Db, file: crate::FileId, src: SourceText, fd: FileDeps, ws: Workspace,
    offset: text_size::TextSize, name: String,
) -> Option<crate::Definition> {
    let tree = crate::db::item_tree(db, src);
    // Inside a module, siblings first.
    if let Some(m) = crate::resolve::enclosing_module(&tree, offset)
        && let Some((idx, _)) = tree.children_of(m).find(|(_, s)| s.name == name)
    { return Some(crate::Definition::Item { file, index: idx }); }
    // Top-level items.
    if let Some((idx, _)) = tree.top_level().find(|(_, s)| s.name == name)
    { return Some(crate::Definition::Item { file, index: idx }); }
    // Included files' top-level decls.
    if let Some(d) = resolve_includes(db, file, src, fd, &name, &mut vec![file])
    { return Some(d); }
    // Imports, in order (Task 6 fills `resolve_through_import`).
    resolve_imports(db, file, src, fd, ws, &name)
}

pub(crate) fn resolve_includes(
    db: &dyn Db, file: crate::FileId, src: SourceText, fd: FileDeps,
    name: &str, active: &mut Vec<crate::FileId>,
) -> Option<crate::Definition> {
    for dep in fd.deps(db).iter().filter(|d| d.is_include) {
        let Some((tfile, tsrc)) = dep.target else { continue };
        if active.contains(&tfile) { continue; }
        let ttree = crate::db::item_tree(db, tsrc);
        if let Some((idx, _)) = ttree.top_level().find(|(_, s)| s.name == name) {
            return Some(crate::Definition::Item { file: tfile, index: idx });
        }
        // Recurse into the include's own includes.
        let tfd = /* target's FileDeps: threaded via a workspace-wide map — see note */;
        active.push(tfile);
        let found = resolve_includes(db, tfile, tsrc, tfd, name, active);
        active.pop();
        if found.is_some() { return found; }
    }
    None
}
```

> **Cross-file `FileDeps` access note.** `resolve_includes`/imports need the *target's* `FileDeps`, not just the current file's. Two options — pick one in this task and keep it for Tasks 6–9:
> **(a) carry a `FileDeps` map in `Workspace`:** change `Workspace` to also hold `#[returns(clone)] pub file_deps: Arc<BTreeMap<FileId, FileDeps>>`, republished by the host whenever `dep_inputs` changes; queries look a target's `FileDeps` up there. Simple, one extra input field.
> **(b) resolve one include level per query hop:** make `resolve_includes` a tracked query keyed on the target and let salsa memoize per file, passing only the target's own `FileDeps` fetched via (a)'s map.
> **Recommended: (a)** — it is the minimal seam and keeps the recursive helper plain. Implement (a): add the `file_deps` map field to `Workspace`, have the host publish it in `index_file`/`publish_file_deps`, and replace the `/* … */` above with `ws`-threaded lookup: pass `ws` into `resolve_includes` and do `let tfd = ws.file_deps(db).get(&tfile).copied().unwrap_or_else(|| empty_file_deps(db));`.

Adopt option (a) now: thread `ws: Workspace` into `resolve_includes`, add the `file_deps` map to `Workspace`, and add a tiny `empty_file_deps(db) -> FileDeps` cached via a `#[salsa::tracked]` returning a `FileDeps::new(db, Arc::from(vec![]))` is **not** valid (inputs can't be created in queries) — instead the host guarantees every referenced `FileId` has a published `FileDeps` (it indexes every discovered + resolved file), so `ws.file_deps(db).get(&tfile)` is `Some`; treat `None` as "target not indexed" → skip (`continue`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core resolve_in_file_finds_top_level_and_enclosing_module`
Expected: PASS. (Full include-chain behavior is re-verified against the M2 oracle once the dispatcher is wired in Task 7.)

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/db.rs
git commit -m "feat(v2b.1): tracked file-scope resolution + pure include walk"
```

---

## Task 6: Tracked import resolution (`resolve_imports` / `resolve_through_import`)

Port `resolve_through_import`, `resolve_external_module_member` into `db.rs`, reading targets from `FileDeps`/`Workspace`. Prefix stripping, selective specifier lists with `as` aliases, stdlib/in-scope-module/external/string-path arms, and the `expected == path_module_name(raw)` module-name check are all pure once targets arrive via the input.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs`

**Interfaces:**
- Consumes: `item_tree`, `FileDeps`, `Workspace`, `resolve::import_specifier_alias`, `resolve::single_top_level_module`, `path_module_name`, `string_lit_text`, `Import` AST.
- Produces: `pub(crate) fn resolve_imports(db, file, src, fd, ws, name: &str) -> Option<Definition>`; `pub(crate) fn resolve_external_module_member(db, ws, fd, raw, expected, target_name) -> Option<Definition>`.

- [ ] **Step 1: Write the failing test** (in-scope module member through an identifier import)

```rust
#[test]
fn resolve_imports_in_scope_module_member() {
    let db = CompactDatabase::default();
    let src = SourceText::new(&db, Arc::from(
        "module M { export circuit ex(): [] {} }\nimport M;"));
    let fd = FileDeps::new(&db, Arc::from(Vec::new()));
    let ws = Workspace::new(&db, None); // no file_deps map entries needed here
    let d = resolve_imports(&db, crate::FileId::from_raw_for_test(0), src, fd, ws, "ex");
    assert!(matches!(d, Some(Definition::Item { index, .. }) if index != 0));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core resolve_imports_in_scope_module_member`
Expected: FAIL — `resolve_imports` not found.

- [ ] **Step 3: Port the implementation**

Port `resolve_through_import` verbatim into a `db`-taking `resolve_imports` that iterates `SourceFile::cast(root)?.imports()` (root rebuilt from `item_tree`'s source — reuse `parsed(db, src).green`), applying the transformation recipe: the two `resolve_external_module_member` disk-probe call sites read the matching `ResolvedDep` from `fd`/target `FileDeps` (via `ws.file_deps`) instead of `cached_source_pathname`; the `CompactStandardLibrary` arm reads `ws.stdlib(db)`. Keep prefix/specifier/alias logic byte-identical (calls `resolve::import_specifier_alias`). Full ported body:

```rust
pub(crate) fn resolve_imports(
    db: &dyn Db, file: crate::FileId, src: SourceText, fd: FileDeps, ws: Workspace, name: &str,
) -> Option<crate::Definition> {
    let tree = crate::db::item_tree(db, src);
    let green = crate::db::parsed(db, src).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let sf = compactp_ast::SourceFile::cast(root)?;
    for import in sf.imports() {
        if let Some(d) = resolve_one_import(db, file, &tree, fd, ws, &import, name) {
            return Some(d);
        }
    }
    None
}
```

Write `resolve_one_import` as the transformed `resolve_through_import` (prefix → specifier list → the four target arms). For the external-module and string-path arms, look the raw string up in `fd.deps(db)` to get `(FileId, SourceText)` and call:

```rust
pub(crate) fn resolve_external_module_member(
    db: &dyn Db, fd: FileDeps, raw: &str, expected_module: &str, target_name: &str,
) -> Option<crate::Definition> {
    let dep = fd.deps(db).iter().find(|d| d.raw == raw && !d.is_include)?.clone();
    let (tfile, tsrc) = dep.target?;
    let ttree = crate::db::item_tree(db, tsrc);
    let (module_idx, module_sym) = crate::resolve::single_top_level_module(&ttree)?;
    if module_sym.name != expected_module { return None; }
    let (idx, _) = ttree.children_of(module_idx)
        .find(|(_, s)| s.exported && s.name == target_name)?;
    Some(crate::Definition::Item { file: tfile, index: idx })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p analyzer-core resolve_imports_in_scope_module_member`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/db.rs
git commit -m "feat(v2b.1): tracked import resolution over FileDeps/Workspace inputs"
```

---

## Task 7: Tracked `resolve` dispatcher + member/struct-field/specifier arms; re-point host `resolve()`

Port the top-level `resolve()` dispatcher, `resolve_name_at`, `resolve_member`, `resolve_struct_literal_field`, `resolve_import_specifier`, `item_declaration_at`, `field_of` into the query layer, and make the host `AnalysisHost::resolve` a thin bridge that provisions inputs and calls the tracked dispatcher.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (dispatcher query + arms)
- Modify: `crates/analyzer-core/src/resolve.rs` (`impl AnalysisHost::resolve` becomes a bridge)

**Interfaces:**
- Consumes: everything from Tasks 5–6; `resolve_local_name`, `ident_at_offset`, `generic_definition`, `local_from_pattern_site`.
- Produces:
  - `pub fn resolve_query(db, file, src, fd, ws, offset: TextSize) -> Option<Definition>` (tracked).
  - `AnalysisHost::resolve(&mut self, FilePosition) -> Option<Definition>` (signature unchanged — bridge).

- [ ] **Step 1: Write the failing test** (host-level, exercises the whole bridge — this is a real M2 behavior)

Reuse the existing `resolve.rs` oracle by keeping those tests; add one host bridge smoke test to `resolve.rs` tests:

```rust
#[test]
fn host_resolve_bridges_to_tracked_query() {
    let (mut host, pos) = position_of("circuit helper(): [] {}\ncircuit main(): [] { helper(); }§");
    // `§` marks the offset on `helper` in the call (see position_of helper).
    let def = host.resolve(pos).unwrap();
    assert!(matches!(def, Definition::Item { .. }));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p analyzer-core host_resolve_bridges_to_tracked_query`
Expected: FAIL until the bridge is wired (or compile error on `resolve_query`).

- [ ] **Step 3: Port dispatcher + re-point host**

In `db.rs`, add `resolve_query` porting `resolve()`'s dispatch (the `parent.kind()` match), calling `resolve_in_file`/`resolve_imports`/`resolve_local_name` and the member/struct-field/specifier arms (each transformed per the recipe; `resolve_name_at` becomes `resolve_local_name(...) .or_else(|| resolve_in_file(...))`). In `resolve.rs`, replace the whole `impl AnalysisHost` resolution block's `resolve` with the bridge:

```rust
pub fn resolve(&mut self, pos: FilePosition) -> Option<Definition> {
    let src = self.src_of(pos.file)?;
    let fd = self.file_deps(pos.file);
    let ws = self.workspace();
    crate::db::resolve_query(&self.db, pos.file, src, fd, ws, pos.offset)
}
```

Delete the ported imperative methods (`resolve_name_at`, `resolve_in_file_scope`, `resolve_through_includes`, `resolve_through_import`, `resolve_external_module_member`, `resolve_struct_literal_field`, `resolve_member`, `resolve_import_specifier`, `field_of`, `item_declaration_at`) from `impl AnalysisHost`.

- [ ] **Step 4: Run the full M2 resolution oracle**

Run: `cargo test -p analyzer-core resolve::tests && cargo test -p compact-analyzer --test lsp_navigation`
Expected: PASS — every goto-definition/local/import/member/enum-variant/stdlib test green, unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/resolve.rs
git commit -m "feat(v2b.1): tracked resolve dispatcher; host resolve() is a thin bridge"
```

---

## Task 8: Re-point `nav_info` / `def_name` (item-tree reads via the bridge)

`nav_info`, `def_name` only read a resolved definition's `item_tree`. Convert them to bridge methods that map `FileId → SourceText` and call `item_tree`.

**Files:**
- Modify: `crates/analyzer-core/src/resolve.rs`

**Interfaces:**
- Produces: `AnalysisHost::nav_info(&mut self, &Definition) -> Option<(FileId, TextRange, TextRange)>`; `AnalysisHost::def_name(&mut self, &Definition) -> Option<String>` (signatures unchanged).

- [ ] **Step 1: Rewrite as bridges**

```rust
pub fn def_name(&mut self, def: &Definition) -> Option<String> {
    match def {
        Definition::Item { file, index } => {
            let src = self.src_of(*file)?;
            Some(crate::db::item_tree(&self.db, src).symbols.get(*index as usize)?.name.clone())
        }
        Definition::Local { name, .. } => Some(name.clone()),
    }
}

pub fn nav_info(&mut self, def: &Definition) -> Option<(FileId, TextRange, TextRange)> {
    match def {
        Definition::Item { file, index } => {
            let src = self.src_of(*file)?;
            let sym = crate::db::item_tree(&self.db, src).symbols.get(*index as usize)?.clone();
            Some((*file, sym.name_range, sym.full_range))
        }
        Definition::Local { file, name_range, .. } => Some((*file, *name_range, *name_range)),
    }
}
```

- [ ] **Step 2: Run the navigation/hover oracle**

Run: `cargo test -p analyzer-core && cargo test -p compact-analyzer --test lsp_navigation`
Expected: PASS — hover, goto-definition, references, rename navigation unchanged.

- [ ] **Step 3: Commit**

```bash
git add crates/analyzer-core/src/resolve.rs
git commit -m "refactor(v2b.1): nav_info/def_name read item_tree via the bridge"
```

---

## Task 9: Tracked `resolution_diagnostics`

Port `resolution_diagnostics` + `module_mismatch_diag` into a tracked query over `(src, fd)`, reading unresolved/mismatch state from `FileDeps` instead of probing disk. The diagnostic wording, codes (E9001–E9004), and spans are byte-identical.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (query), `crates/analyzer-core/src/resolve.rs` (host bridge)

**Interfaces:**
- Consumes: `item_tree`, `FileDeps`, `parsed` (for the green tree), `single_top_level_module`, `path_module_name`, the diagnostic builders.
- Produces: `pub fn resolution_diagnostics_query(db, src, fd, from_dir_display: Arc<str>, search_display: Arc<str>) -> Arc<[Diagnostic]>` (tracked); `AnalysisHost::resolution_diagnostics(&mut self, FileId) -> Vec<Diagnostic>` (signature unchanged — bridge).

> The "searched:" message text embeds `from_dir` + the search path (impure display strings). Precompute them host-side and pass as `Arc<str>` so the query stays pure. (Only the *display* of the search path enters diagnostics; resolution itself already happened in `FileDeps`.)

- [ ] **Step 1: Write the failing test** (through the host — an unresolved import yields E9001)

```rust
#[test]
fn resolution_diagnostics_report_unresolved_import() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main.compact");
    std::fs::write(&main, "import \"DoesNotExist\";").unwrap();
    let mut host = AnalysisHost::new();
    let id = host.vfs_mut().file_id(&main);
    host.index_file(id);
    let diags = host.resolution_diagnostics(id);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, crate::DiagnosticCode::new("E", 9001));
}
```

- [ ] **Step 2: Run test to verify it fails, then port**

Run: `cargo test -p analyzer-core resolution_diagnostics_report_unresolved_import`
Expected: FAIL first (bridge not wired). Port `resolution_diagnostics` into `resolution_diagnostics_query`, transforming each `cached_source_pathname(from_dir, raw).is_none()` / `Some(path)` branch into a lookup of the matching `ResolvedDep.target` (`None` → unresolved diag; `Some((tfile, tsrc))` → `module_mismatch` check via `item_tree(db, tsrc)`). Host bridge:

```rust
pub fn resolution_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic> {
    let Some(src) = self.src_of(file) else { return Vec::new() };
    let fd = self.file_deps(file);
    let from_dir = self.vfs().path(file).parent().map(|d| d.display().to_string()).unwrap_or_default();
    let search = self.import_search_path().iter().map(|p| p.display().to_string())
        .collect::<Vec<_>>().join(", ");
    crate::db::resolution_diagnostics_query(&self.db, src, fd,
        Arc::from(from_dir.as_str()), Arc::from(search.as_str())).to_vec()
}
```

- [ ] **Step 3: Run the diagnostics oracle**

Run: `cargo test -p analyzer-core && cargo test -p compact-analyzer --test lsp_workspace`
Expected: PASS — E9001–E9004 import/include/module-mismatch diagnostics identical (existing integration tests are the oracle).

- [ ] **Step 4: Commit**

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/resolve.rs
git commit -m "feat(v2b.1): tracked resolution_diagnostics over FileDeps"
```

---

## Task 10: Delete dead imperative internals; simplify the disk-probe memo; clippy

With resolution reading `FileDeps`, the resolver no longer calls `cached_source_pathname`. Confirm its only remaining caller is `index_file`'s publish pass, and decide the `source_path_cache`'s fate: `index_file` currently calls `find_source_pathname` **directly** (uncached), so `cached_source_pathname` + `source_path_cache` + `source_path_cache_generation` + their three invalidation hooks are now **dead** and are deleted. (The per-index probe cost is unchanged from today — `index_file` was already uncached.)

**Files:**
- Modify: `crates/analyzer-core/src/analysis.rs` (delete `cached_source_pathname`, `source_path_cache*` fields + hooks; `forget_file`/`set_import_search_path` lose their cache-clear lines but keep `sources`/`dep_inputs` cleanup)
- Modify: `crates/analyzer-core/src/resolve.rs` (delete any now-unused imports)

**Interfaces:**
- Produces: `forget_file` now drops `sources` + `dep_inputs` entries for the file; `set_import_search_path` just stores the path (re-indexing republishes `FileDeps`).

- [ ] **Step 1: Delete dead state**

Remove `source_path_cache`, `source_path_cache_generation`, `cached_source_pathname`, and the generation-hook logic. Update:

```rust
pub fn forget_file(&mut self, file: FileId) {
    self.sources.remove(&file);
    self.dep_inputs.remove(&file);
}

pub fn set_import_search_path(&mut self, path: Vec<PathBuf>) {
    self.import_search_path = path; // re-indexing republishes FileDeps under the new path
}
```

> The old `cached_source_pathname` doc-comment described a 3-hook invalidation contract for the memo. That contract dies with the memo; no correctness property is lost because `FileDeps` is republished by `index_file` on every content/structure change and by `set_import_search_path`-triggered re-indexing.

- [ ] **Step 2: Run the full oracle + lints**

Run: `cargo test -p analyzer-core && cargo test -p compact-analyzer && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS + clippy clean (watch for `needless_lifetimes` on any tracked fn — elide).

- [ ] **Step 3: Commit**

```bash
git add crates/analyzer-core/src/analysis.rs crates/analyzer-core/src/resolve.rs
git commit -m "refactor(v2b.1): delete the now-dead disk-probe memo and imperative resolution state"
```

---

## Task 11: Incrementality regressions + full behavior-preserving verification

Prove the swap actually reuses work and preserves every v1 behavior — the whole point of the migration.

**Files:**
- Test: `crates/analyzer-core/src/db.rs`

- [ ] **Step 1: Cross-file resolution memo reuse**

```rust
#[test]
fn editing_importer_body_does_not_reresolve_imported_file() {
    use salsa::Setter as _;
    let db_host = /* build a 2-file host: lib.compact (module L exporting `ex`) + main importing L */;
    // Resolve `ex` from main; capture the imported file's item_tree Arc.
    // Edit main's circuit *body* (not its imports); re-resolve.
    // Assert lib's item_tree Arc is pointer-identical (its memo survived).
}
```

> Build this with the host (`AnalysisHost`) so it exercises the real input plumbing; assert `Arc::ptr_eq` on `item_tree(&host.db, lib_src)` before and after a main-body edit.

- [ ] **Step 2: Trivia-only edit does not re-resolve**

Assert that after a comment-only edit to a file, `resolve_query` returns an equal
`Definition`, and prove the firewall **downstream** (not on `item_tree` itself, which always
re-executes — see Task 2's salsa-mechanics correction): assert `Arc::ptr_eq` on a
`PartialEq`-returning derived query that rode the backdate — `file_symbols(&db, src)` before
and after the trivia edit must be the same `Arc` allocation (unchanged item_tree revision →
downstream skipped). Do **not** assert `Arc::ptr_eq` on `item_tree` itself.

- [ ] **Step 3: Whole-suite + corpus + lints**

Run: `cargo test --workspace`
Expected: PASS with the **same M2 test count** as before v2b.1 (no test silently dropped; renamed bridge smoke tests aside).

Run: `cargo clippy --workspace --all-targets -- -D warnings` → clean.

Run the corpus smoke (locate with `grep -rn "corpus" crates/ --include=*.rs`) → no panics, no OOB spans, identical to pre-v2b.1.

- [ ] **Step 4: Keystroke-latency sanity**

Confirm re-resolving one edited file in a multi-file workspace does not re-run `item_tree`/resolution for unaffected files — Step 1 asserts this at the unit level; record it as the standing evidence (no benchmark harness required).

- [ ] **Step 5: Final commit**

```bash
git add crates/analyzer-core/src/db.rs
git commit -m "test(v2b.1): lock in cross-file resolution reuse + trivia backdating; full verification"
```

---

## Self-Review

- **Spec coverage:** foundation spec §3 (resolution into salsa, wholesale, behavior-preserving) → Tasks 5–10; the workspace-input seam → Tasks 1, 3; the backdating firewall (`item_tree` + `PartialEq`) → Task 2; impurity-stays-in-host → Task 3 (only impure site) + Task 10 (dead memo removed); single-threaded/no-`Cancelled` → unchanged (constraint restated). Done-bar "every M2 test green" → Tasks 4,7,8,9,11 name the oracle suites. ✓
- **Placeholder scan:** the one genuinely deferred detail — cross-file `FileDeps` access (Task 5's note) — is resolved *in-task* to option (a) with concrete steps, not left open. The Task 11 Step 1 host-construction is described precisely (2-file host, ptr-eq assertion) rather than left vague. ✓
- **Type consistency:** `ResolvedDep`, `FileDeps`, `Workspace`, `item_tree`, `resolve_in_file`, `resolve_imports`, `resolve_external_module_member`, `resolve_query`, `resolution_diagnostics_query`, `src_of`, `file_deps`, `workspace()` are named identically across tasks; `resolve`/`nav_info`/`def_name`/`resolution_diagnostics`/`scope_bindings_at` keep their pre-v2b.1 signatures. ✓
- **Salsa-API risk is quarantined** in Task 1 (spike), mirroring v2a: if input-holding-input or multi-arg tracked fns need a different 0.28 shape, the correction is made there and carried forward.

> **Note for the executing agent:** this is a behavior-preserving migration — the existing M2 tests in `resolve.rs`/`lsp_navigation`/`lsp_workspace` are your oracle and must never be weakened to pass. If a ported query disagrees with the oracle, the port is wrong, not the test. Port one capability per task and keep the suite green before moving on.
