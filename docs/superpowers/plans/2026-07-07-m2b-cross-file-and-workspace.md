# compact-analyzer M2b: Cross-file Resolution + Workspace Index Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend M2a's single-file resolver across file boundaries — filesystem `import`/`include` resolution mirroring the compiler, an eager workspace index, cross-file goto/references/rename/workspace-symbols, unresolvable-import diagnostics, and a corpus smoke test.

**Architecture:** `analyzer-core` gains a `source_path` module (the compiler's `find-source-pathname` mirror) and a `workspace` module (eager gitignore-aware index: per-file symbol table, forward/reverse dependency graph, generation counter). The resolver's two silent-`None` arms are filled to consult the filesystem. `analyzer-ide` makes find-references/rename workspace-wide (with per-use-site conflict detection) and adds workspace-symbols. The `compact-analyzer` binary consumes `initialize` params (roots + config), runs the crawl, registers a `didChangeWatchedFiles` watcher, wires `workspace/symbol`, and drives cooperative cancellation by draining the LSP queue at unit boundaries. `lsp-types` stays confined to the binary.

**Tech Stack:** Rust edition 2024 (rust-version 1.90); compactp `0.1.0-beta.1` (`compactp_ast`/`_parser`/`_syntax`/`_diagnostics`); rowan 0.16; text-size 1; lsp-server 0.8; lsp-types 0.95.1; **new:** `ignore` 0.4 (gitignore-aware crawl); tempfile (dev).

## Global Constraints

- Everything from the M1 and M2a Global Constraints still binds: never-die (per-request `catch_unwind`; malformed/adversarial input never kills the session; unresolvable positions answer null), stdout is protocol-only, byte offsets (`TextSize`/`TextRange`) + `FileId` in `analyzer-core`/`analyzer-ide`, `lsp-types` ONLY in the binary, UTF-16 ↔ byte conversion ONLY at the binary boundary, conventional commits.
- Per task: `cargo fmt` clean, `cargo clippy --workspace --all-targets --locked -- -D warnings` clean, all tests green. CI runs on ubuntu/macOS/windows with `--locked`.
- `lsp-types` pinned at `0.95.1` (0.96+ drops `url::Url`). `compactp_*` pinned at `0.1.0-beta.1`. **Never commit a `[patch.crates-io]`** (an uncommitted patch at `../compactp` is fine for iteration).
- Baseline before Task 1: **83 tests** green (analyzer-core 51, analyzer-ide 16, compact-analyzer 6 unit + 5 integration + 5 navigation).

### Verified compiler file-resolution semantics (empirically confirmed 2026-07-07 via `compactc --trace-search`; the implementation contract — do NOT re-derive)

- **`.compact` is ALWAYS appended** to the spec — for identifier imports, string-path imports, AND includes. The extension is omitted in source. (`import Foo;` → `Foo.compact`; `import "sub/Bar";` → `sub/Bar.compact`; `include "inc";` → `inc.compact`. Writing `include "inc.compact";` wrongly resolves `inc.compact.compact` and fails.)
- **Search order:** an absolute spec is used exactly; otherwise the importing/including file's own directory first, then each import-search-path entry left-to-right. Each file's imports/includes resolve **relative to that file's own directory** (matters for recursion).
- **Import search path** = the `--compact-path` value, defaulting to the `COMPACT_PATH` env var (colon-separated on Unix, semicolon on Windows), else empty.
- **Identifier `import Foo;`** consults in-scope modules FIRST, the filesystem second. A resolved `Foo.compact` must contain **exactly one top-level `module Foo {…}` and no other declarations** (compiler errors: *"X.compact defines module Y rather than expected module X"*, *"X.compact does not contain a (single) module defintion"*).
- **String-path `import "a/b";`** never consults in-scope modules; the expected module name is the **last path component** (`b`); the resolved file's single module must match it.
- **Includes** splice raw top-level content (no module wrapper, no single-module requirement); cycles are detected along the active include path only; duplicate includes are NOT deduplicated.
- `import CompactStandardLibrary;` is compiler-internal (never a file lookup) — already resolved to the bundled stub in M2a; keep that.

### Verified API facts (confirmed against `../compactp` @ `0.1.0-beta.1` and lsp-types `0.95.1`)

- compactp: `SourceFile::imports() -> impl Iterator<Item = Import>`, `SourceFile::includes() -> impl Iterator<Item = Include>`; `Import::{name() -> Option<SyntaxToken> (IDENT), path() -> Option<SyntaxToken> (STRING_LIT), prefix() -> Option<PrefixDecl>, generic_args()}`; `ImportSpecifier::name() -> Option<SyntaxToken>` (original, pre-alias); `PrefixDecl::name()`; `Include::path() -> Option<SyntaxToken>` (STRING_LIT). STRING_LIT `.text()` includes the surrounding double quotes.
- lsp-types `0.95.1` provides: `DidChangeWatchedFilesParams`, `FileEvent{uri, typ}`, `FileChangeType` (CREATED=1, CHANGED=2, DELETED=3), `FileSystemWatcher{glob_pattern}`, `GlobPattern`, `Registration`, `RegistrationParams`, `DidChangeWatchedFilesRegistrationOptions`, `WorkspaceSymbolParams`, `SymbolInformation`, `WorkspaceSymbol`, `WorkspaceSymbolResponse`, and the `workspace_symbol_provider` capability field. Dynamic registration is sent via the `client/registerCapability` server→client request.
- `ignore` crate resolves to `v0.4.27`, MIT/Apache-2.0 dual-licensed; `ignore::WalkBuilder` walks a root gitignore-aware.

## Shared Interfaces (single source of truth for signatures used across tasks)

New/changed public API added by this plan. Later tasks rely on exactly these names and types:

```rust
// analyzer-core::source_path  (Task 1)
pub fn find_source_pathname(from_dir: &Path, search_path: &[PathBuf], raw: &str) -> Option<PathBuf>;
pub fn string_lit_text(tok_text: &str) -> String;   // strips surrounding double quotes
pub fn path_module_name(raw: &str) -> Option<String>; // last '/'|'\\' component of raw

// analyzer-core::vfs::Vfs  (Task 4a)
pub fn generation(&self) -> u64;                     // bumped by set_overlay/remove_overlay/invalidate_disk
pub fn invalidate_disk(&mut self, file: FileId);     // drop cached Disk content; never touches an Overlay

// analyzer-core::AnalysisHost  (Tasks 4a/4b/4c/6)
pub fn generation(&self) -> u64;                     // delegates to vfs().generation()
pub fn set_import_search_path(&mut self, path: Vec<PathBuf>);
pub fn import_search_path(&self) -> Vec<PathBuf>;
pub fn discover_and_index(&mut self, roots: &[PathBuf], should_continue: &dyn Fn() -> bool);
pub fn index_file(&mut self, file: FileId);          // (re)build symbol + dep edges for one file
pub fn remove_workspace_file(&mut self, file: FileId); // eviction on delete
pub fn workspace_files(&self) -> Vec<FileId>;
pub fn dependents_of(&self, file: FileId) -> Vec<FileId>; // direct reverse-deps
pub fn workspace_symbols(&mut self, query: &str) -> Vec<(FileId, u32)>; // (file, symbol index)
pub fn resolution_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic>; // Task 6

// analyzer-core::workspace  (Task 4c) — free function
pub fn discover_compact_files(roots: &[PathBuf]) -> Vec<PathBuf>; // gitignore-aware, *.compact

// analyzer-ide  (Tasks 7/8/9) — cancellable variants; the existing zero-arg
// wrappers keep their signatures by passing `&|| true`.
pub fn find_references_cancellable(host, pos, include_declaration, should_continue: &dyn Fn() -> bool) -> Option<Vec<NavTarget>>;
pub fn rename_cancellable(host, pos, new_name, should_continue: &dyn Fn() -> bool) -> Result<Vec<SourceEdit>, RenameError>;
pub fn workspace_symbols(host: &mut AnalysisHost, query: &str) -> Vec<WorkspaceSymbolItem>;
pub struct WorkspaceSymbolItem { pub file: FileId, pub name: String, pub kind: analyzer_core::SymbolKind, pub name_range: TextRange, pub container: Option<String> }
```

## File Structure

- `crates/analyzer-core/src/source_path.rs` — **create.** Pure `find-source-pathname` mirror + string-literal/module-name helpers. No I/O beyond `Path::exists`.
- `crates/analyzer-core/src/workspace.rs` — **create.** `WorkspaceIndex` (file set, per-file symbol table, forward/reverse dep graph), the crawl, dirty propagation, resolution-diagnostic computation.
- `crates/analyzer-core/src/resolve.rs` — **modify.** Fill the two `resolve_through_import` `None` arms (identifier filesystem fallback + string path); add include resolution to `resolve_in_file_scope`.
- `crates/analyzer-core/src/vfs.rs` — **modify.** Generation counter; `invalidate_disk`.
- `crates/analyzer-core/src/analysis.rs` — **modify.** Own the `WorkspaceIndex`; cache re-keyed to avoid hash-on-hit + eviction (Task 5). Workspace methods delegate here.
- `crates/analyzer-core/src/line_index.rs` — **modify (tests only, Task 16).**
- `crates/analyzer-core/src/lib.rs` — **modify.** `mod source_path; mod workspace;` + re-exports.
- `crates/analyzer-ide/src/references.rs` — **modify.** Workspace-wide, cancellable.
- `crates/analyzer-ide/src/rename.rs` — **modify.** Workspace-wide + per-use-site conflicts, cancellable.
- `crates/analyzer-ide/src/workspace_symbols.rs` — **create.**
- `crates/analyzer-ide/src/lib.rs` — **modify.** Re-exports.
- `crates/compact-analyzer/src/server.rs` — **modify.** Initialize params, crawl, config, `workspace/symbol`, watched-files, cross-file publish, cancellation callback.
- `crates/compact-analyzer/src/lsp_utils.rs` — **modify.** `workspace_symbol_item` → `SymbolInformation`; resolution-diagnostic merge helper.
- `crates/compact-analyzer/tests/lsp_workspace.rs` — **create.** Black-box cross-file integration + workspace/symbol + watched-files.
- `crates/compact-analyzer/tests/corpus_smoke.rs` — **create.** Corpus smoke over the compactp corpus.

---

### Task 1: analyzer-core — source-path resolution (`find-source-pathname` mirror)

Pure path logic — no parsing, no host. This is the foundation Tasks 2/3/4 build on.

**Files:**
- Create: `crates/analyzer-core/src/source_path.rs`
- Modify: `crates/analyzer-core/src/lib.rs` (add `mod source_path;` + re-export)

**Interfaces:**
- Produces: `find_source_pathname(from_dir: &Path, search_path: &[PathBuf], raw: &str) -> Option<PathBuf>`, `string_lit_text(&str) -> String`, `path_module_name(&str) -> Option<String>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/analyzer-core/src/source_path.rs` with only the test module for now:

```rust
//! Mirror of the compiler's `find-source-pathname` (verified via
//! `compactc --trace-search`, 2026-07-07). `.compact` is ALWAYS appended;
//! the spec omits the extension. Search order: the importing/including file's
//! own directory first, then each import-search-path entry left to right.
//! Absolute specs are used exactly. Pure path logic — the only I/O is
//! `Path::exists`.

use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_compact_and_searches_from_dir_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let libs = root.join("libs");
        std::fs::create_dir_all(&libs).unwrap();
        std::fs::write(root.join("Foo.compact"), "module Foo {}").unwrap();
        std::fs::write(libs.join("Foo.compact"), "module Foo {}").unwrap();
        // Present in from_dir → found there; search path never consulted.
        let got = find_source_pathname(root, &[libs], "Foo").unwrap();
        assert_eq!(got, root.join("Foo.compact"));
    }

    #[test]
    fn falls_back_to_search_path_left_to_right() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("Qux.compact"), "module Qux {}").unwrap();
        // Not in from_dir, not in `a`, found in `b` (second search entry).
        let got = find_source_pathname(root, &[a, b.clone()], "Qux").unwrap();
        assert_eq!(got, b.join("Qux.compact"));
    }

    #[test]
    fn resolves_subdir_string_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/Bar.compact"), "module Bar {}").unwrap();
        let got = find_source_pathname(root, &[], "sub/Bar").unwrap();
        assert_eq!(got, root.join("sub/Bar.compact"));
    }

    #[test]
    fn absolute_spec_used_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Abs.compact"), "module Abs {}").unwrap();
        let abs_no_ext = root.join("Abs"); // absolute, no extension
        let got =
            find_source_pathname(Path::new("/nowhere"), &[], abs_no_ext.to_str().unwrap()).unwrap();
        assert_eq!(got, root.join("Abs.compact"));
    }

    #[test]
    fn returns_none_when_unresolvable() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_source_pathname(dir.path(), &[], "Missing").is_none());
    }

    #[test]
    fn string_lit_text_strips_quotes() {
        assert_eq!(string_lit_text("\"sub/Bar\""), "sub/Bar");
        assert_eq!(string_lit_text("sub/Bar"), "sub/Bar"); // tolerant if unquoted
    }

    #[test]
    fn module_name_is_last_component() {
        assert_eq!(path_module_name("sub/Bar").as_deref(), Some("Bar"));
        assert_eq!(path_module_name("Bar").as_deref(), Some("Bar"));
        assert_eq!(path_module_name("a/b/c").as_deref(), Some("c"));
        assert_eq!(path_module_name("").as_deref(), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core source_path`
Expected: FAIL to compile — `find_source_pathname`, `string_lit_text`, `path_module_name` not defined.

- [ ] **Step 3: Write the implementation**

Add above the `#[cfg(test)]` module in `source_path.rs`:

```rust
/// Resolves an import/include spec (`raw`, WITHOUT the `.compact` extension)
/// to an existing file. Absolute specs are used exactly; relative specs are
/// searched in `from_dir` first, then each `search_path` entry in order.
/// Returns the first candidate that exists on disk.
pub fn find_source_pathname(from_dir: &Path, search_path: &[PathBuf], raw: &str) -> Option<PathBuf> {
    let rel = format!("{raw}.compact");
    if Path::new(raw).is_absolute() {
        let cand = PathBuf::from(rel);
        return cand.exists().then_some(cand);
    }
    let first = from_dir.join(&rel);
    if first.exists() {
        return Some(first);
    }
    for entry in search_path {
        let cand = entry.join(&rel);
        if cand.exists() {
            return Some(cand);
        }
    }
    None
}

/// The text of a STRING_LIT token with its surrounding double quotes removed.
/// (compactp's `.text()` includes the quotes.) Tolerant if already unquoted.
pub fn string_lit_text(tok_text: &str) -> String {
    tok_text
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(tok_text)
        .to_string()
}

/// The module name a string-path import expects: the last path component,
/// ignoring any directory prefix. `None` for an empty spec.
pub fn path_module_name(raw: &str) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    (!last.is_empty()).then(|| last.to_string())
}
```

- [ ] **Step 4: Wire the module + re-exports**

In `crates/analyzer-core/src/lib.rs`, add `mod source_path;` alongside the other `mod` lines and add to the re-exports block:

```rust
pub use source_path::{find_source_pathname, path_module_name, string_lit_text};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p analyzer-core source_path`
Expected: PASS (6 tests).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count 51 → 57.

```bash
git add crates/analyzer-core/src/source_path.rs crates/analyzer-core/src/lib.rs
git commit -m "feat(core): find-source-pathname mirror for cross-file resolution"
```

---

### Task 2: analyzer-core — fill the resolver's filesystem import arms

Turn the two silent-`None` arms of `resolve_through_import` (identifier import with no in-file module; string-path import) into real cross-file resolution using Task 1. Also introduce the import-search-path config field (a small `Vec<PathBuf>` on `AnalysisHost`; the workspace machinery that populates it comes in Task 4).

**Files:**
- Modify: `crates/analyzer-core/src/analysis.rs` (add `import_search_path` field + accessors)
- Modify: `crates/analyzer-core/src/resolve.rs` (fill the two arms; add two helpers; add tests)

**Interfaces:**
- Consumes: `find_source_pathname`, `string_lit_text`, `path_module_name` (Task 1); `AnalysisHost::{analyze, vfs, vfs_mut}`, `ItemTree::{top_level, children_of}`, `SymbolKind::Module`, `Definition::Item` (M2a).
- Produces: `AnalysisHost::set_import_search_path`, `AnalysisHost::import_search_path`; cross-file `Definition` from use-site references through imports.

- [ ] **Step 1: Add the import-search-path field to AnalysisHost**

In `crates/analyzer-core/src/analysis.rs`, add `use std::path::PathBuf;` near the top, then add the field to the struct and initializer and two accessors:

```rust
pub struct AnalysisHost {
    vfs: Vfs,
    cache: HashMap<FileId, (u64, FileAnalysis)>,
    stdlib: Option<FileId>,
    import_search_path: Vec<PathBuf>,
}
```

In `new()` add `import_search_path: Vec::new(),`. Then add these methods to the `impl AnalysisHost` block:

```rust
    /// Directories consulted (after the importing file's own directory) when
    /// resolving non-absolute imports/includes. Mirrors `--compact-path`.
    pub fn set_import_search_path(&mut self, path: Vec<PathBuf>) {
        self.import_search_path = path;
    }

    pub fn import_search_path(&self) -> Vec<PathBuf> {
        self.import_search_path.clone()
    }
```

- [ ] **Step 2: Write the failing tests**

In `crates/analyzer-core/src/resolve.rs`, add to the existing `#[cfg(test)] mod tests` (or create one) a cross-file helper and tests:

```rust
    /// Writes `others` (relative path, contents) to a tempdir, writes the
    /// `$0`-marked `open` file (extension implied by `open_rel`) there too as
    /// an overlay, and returns (host, tempdir-guard, cursor position).
    fn cross_file(
        marked_open: &str,
        open_rel: &str,
        others: &[(&str, &str)],
    ) -> (AnalysisHost, tempfile::TempDir, FilePosition) {
        let (clean, offset) = crate::fixture::extract(marked_open);
        let dir = tempfile::tempdir().unwrap();
        for (rel, contents) in others {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, contents).unwrap();
        }
        let open_path = dir.path().join(open_rel);
        std::fs::create_dir_all(open_path.parent().unwrap()).unwrap();
        std::fs::write(&open_path, &clean).unwrap();
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(&open_path);
        host.vfs_mut().set_overlay(file, clean, 1);
        (host, dir, FilePosition { file, offset })
    }

    #[test]
    fn identifier_import_resolves_across_files() {
        let (mut host, _dir, pos) = cross_file(
            "import Foo;\ncircuit m(): Field { return f$0(); }",
            "main.compact",
            &[(
                "Foo.compact",
                "module Foo { export circuit f(): Field { return 0; } }",
            )],
        );
        let pos_file = pos.file;
        let def = host.resolve(pos).expect("resolves through identifier import");
        assert_eq!(host.def_name(&def).unwrap(), "f");
        assert_ne!(host.nav_info(&def).unwrap().0, pos_file); // lands in Foo.compact
    }

    #[test]
    fn string_path_import_resolves_across_files() {
        let (mut host, _dir, pos) = cross_file(
            "import \"sub/Bar\";\ncircuit m(): Field { return g$0(); }",
            "main.compact",
            &[(
                "sub/Bar.compact",
                "module Bar { export circuit g(): Field { return 0; } }",
            )],
        );
        let pos_file = pos.file;
        let def = host.resolve(pos).expect("resolves through string-path import");
        assert_eq!(host.def_name(&def).unwrap(), "g");
        assert_ne!(host.nav_info(&def).unwrap().0, pos_file);
    }

    #[test]
    fn import_with_wrong_module_name_is_unresolved() {
        let (mut host, _dir, pos) = cross_file(
            "import Foo;\ncircuit m(): Field { return f$0(); }",
            "main.compact",
            &[(
                "Foo.compact",
                "module NotFoo { export circuit f(): Field { return 0; } }",
            )],
        );
        assert!(host.resolve(pos).is_none());
    }

    #[test]
    fn missing_import_file_is_unresolved() {
        let (mut host, _dir, pos) = cross_file(
            "import Missing;\ncircuit m(): Field { return f$0(); }",
            "main.compact",
            &[],
        );
        assert!(host.resolve(pos).is_none());
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p analyzer-core resolve::tests::identifier_import_resolves_across_files resolve::tests::string_path_import_resolves_across_files`
Expected: FAIL — both currently resolve to `None` (the M2b arms are unimplemented).

- [ ] **Step 4: Add the resolution helpers**

In `resolve.rs`, add a free function near the other free helpers (e.g. after `enclosing_module`):

```rust
/// The sole top-level symbol iff it is exactly one and a module. Mirrors the
/// compiler's "must contain a (single) module definition" rule.
fn single_top_level_module(tree: &crate::ItemTree) -> Option<(u32, &crate::Symbol)> {
    let mut tops = tree.top_level();
    let first = tops.next()?;
    if tops.next().is_some() {
        return None; // more than one top-level declaration
    }
    (first.1.kind == crate::SymbolKind::Module).then_some(first)
}
```

And add this method inside `impl crate::AnalysisHost` (near `resolve_through_import`):

```rust
    /// Resolves `target_name` as an exported member of the single module in the
    /// file that `raw` resolves to, requiring that module's name to equal
    /// `expected_module` (compiler rule). Used by both the identifier and
    /// string-path import arms.
    fn resolve_external_module_member(
        &mut self,
        from_dir: &std::path::Path,
        raw: &str,
        expected_module: &str,
        target_name: &str,
    ) -> Option<Definition> {
        let search = self.import_search_path();
        let path = crate::find_source_pathname(from_dir, &search, raw)?;
        let file = self.vfs_mut().file_id(&path);
        let tree = self.analyze(file)?.item_tree.clone();
        let (module_idx, module_sym) = single_top_level_module(&tree)?;
        if module_sym.name != expected_module {
            return None;
        }
        let (idx, _) = tree
            .children_of(module_idx)
            .find(|(_, s)| s.exported && s.name == target_name)?;
        Some(Definition::Item { file, index: idx })
    }
```

- [ ] **Step 5: Fill the two import arms**

In `resolve_through_import`, replace the final `match import.name() { … }` arms for the identifier case and the string-path case. Replace this block:

```rust
            Some(module_token) => {
                // In-file module import (filesystem fallback is M2b).
                let (module_idx, _) = tree.top_level().find(|(_, s)| {
                    s.kind == crate::SymbolKind::Module && s.name == module_token.text()
                })?;
                let (idx, _) = tree
                    .children_of(module_idx)
                    .find(|(_, s)| s.exported && s.name == target_name)?;
                Some(Definition::Item { file, index: idx })
            }
            // String-path import: M2b.
            None => None,
```

with:

```rust
            Some(module_token) => {
                let module_name = module_token.text().to_string();
                // In-scope module first (verified: in-scope FIRST, filesystem
                // second). An in-scope module wins even if the member is absent.
                if let Some((module_idx, _)) = tree
                    .top_level()
                    .find(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == module_name)
                {
                    return tree
                        .children_of(module_idx)
                        .find(|(_, s)| s.exported && s.name == target_name)
                        .map(|(idx, _)| Definition::Item { file, index: idx });
                }
                // No in-scope module → filesystem: `<module_name>.compact`
                // containing exactly `module <module_name>`.
                let from_dir = self.vfs().path(file).parent()?.to_path_buf();
                self.resolve_external_module_member(
                    &from_dir,
                    &module_name,
                    &module_name,
                    &target_name,
                )
            }
            // String-path import: never consults in-scope modules; expected
            // module name is the last path component.
            None => {
                let raw = crate::string_lit_text(import.path()?.text());
                let expected = crate::path_module_name(&raw)?;
                let from_dir = self.vfs().path(file).parent()?.to_path_buf();
                self.resolve_external_module_member(&from_dir, &raw, &expected, &target_name)
            }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core resolve`
Expected: PASS (the 4 new tests plus all pre-existing resolve tests).

- [ ] **Step 7: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count 57 → 61.

> NOTE: goto-def on the *original name inside a cross-file `import { orig } from M;` specifier* (`resolve_import_specifier`) is intentionally left resolving to `None` in this task (M2a only handled in-file-module specifiers). References/rename from real use-sites go through `resolve_through_import`, which is now cross-file. Verify the selective-import surface syntax against `../compactp` before extending `resolve_import_specifier` in a later pass if desired.

```bash
git add crates/analyzer-core/src/analysis.rs crates/analyzer-core/src/resolve.rs
git commit -m "feat(core): resolve identifier and string-path imports across files"
```

---

### Task 3: analyzer-core — include resolution (textual splice)

An included file's top-level declarations become visible in the includer's file scope (verified: raw splice, no module wrapper, no export requirement). Resolve them, cycle-detected along the active include path, relative to the includer's own directory.

**Files:**
- Modify: `crates/analyzer-core/src/resolve.rs` (add `resolve_through_includes`; call it from `resolve_in_file_scope`; add tests)

**Interfaces:**
- Consumes: `find_source_pathname`, `string_lit_text` (Task 1); `SourceFile::includes()`, `Include::path()` (compactp); `AnalysisHost::{analyze, vfs, vfs_mut, import_search_path}`; the `cross_file` test helper (Task 2).
- Produces: include-visible `Definition`s from use-sites.

- [ ] **Step 1: Write the failing tests**

Add to `resolve.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn include_makes_top_level_decls_visible() {
        // `helper` is NOT exported in the included file — splice is raw.
        let (mut host, _dir, pos) = cross_file(
            "include \"inc\";\ncircuit m(): Field { return help$0er(); }",
            "main.compact",
            &[("inc.compact", "circuit helper(): Field { return 0; }")],
        );
        let pos_file = pos.file;
        let def = host.resolve(pos).expect("resolves into included file");
        assert_eq!(host.def_name(&def).unwrap(), "helper");
        assert_ne!(host.nav_info(&def).unwrap().0, pos_file);
    }

    #[test]
    fn transitive_includes_resolve() {
        let (mut host, _dir, pos) = cross_file(
            "include \"a\";\ncircuit m(): Field { return de$0ep(); }",
            "main.compact",
            &[
                ("a.compact", "include \"b\";"),
                ("b.compact", "circuit deep(): Field { return 0; }"),
            ],
        );
        let pos_file = pos.file;
        let def = host.resolve(pos).expect("resolves through transitive include");
        assert_eq!(host.def_name(&def).unwrap(), "deep");
        assert_ne!(host.nav_info(&def).unwrap().0, pos_file);
    }

    #[test]
    fn include_cycle_terminates() {
        let (mut host, _dir, pos) = cross_file(
            "include \"a\";\ncircuit m(): Field { return miss$0ing(); }",
            "main.compact",
            &[
                ("a.compact", "include \"b\";"),
                ("b.compact", "include \"a\";"), // a <-> b cycle
            ],
        );
        // Must terminate (no hang, no panic) and resolve to None.
        assert!(host.resolve(pos).is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core resolve::tests::include_makes_top_level_decls_visible`
Expected: FAIL — includes are not yet consulted.

- [ ] **Step 3: Implement include resolution**

In `resolve.rs`, add this method to `impl crate::AnalysisHost`:

```rust
    /// Resolves `name` against the top-level declarations of files reachable by
    /// `include` from `root` (the syntax tree of `file`), depth-first. `active`
    /// is the include path currently being expanded, for cycle detection.
    fn resolve_through_includes(
        &mut self,
        file: FileId,
        root: &SyntaxNode,
        name: &str,
        active: &mut Vec<FileId>,
    ) -> Option<Definition> {
        let source_file = compactp_ast::SourceFile::cast(root.clone())?;
        let from_dir = self.vfs().path(file).parent()?.to_path_buf();
        let search = self.import_search_path();
        // Collect target FileIds first (avoids a borrow across the loop body).
        let mut targets = Vec::new();
        for inc in source_file.includes() {
            if let Some(tok) = inc.path() {
                let raw = crate::string_lit_text(tok.text());
                if let Some(path) = crate::find_source_pathname(&from_dir, &search, &raw) {
                    targets.push(self.vfs_mut().file_id(&path));
                }
            }
        }
        for target in targets {
            if active.contains(&target) {
                continue; // cycle along the active include path
            }
            let Some(analysis) = self.analyze(target) else {
                continue; // unreadable include: skip, never die
            };
            if let Some((idx, _)) = analysis.item_tree.top_level().find(|(_, s)| s.name == name) {
                return Some(Definition::Item { file: target, index: idx });
            }
            let target_root = SyntaxNode::new_root(analysis.green.clone());
            active.push(target);
            let found = self.resolve_through_includes(target, &target_root, name, active);
            active.pop();
            if found.is_some() {
                return found;
            }
        }
        None
    }
```

- [ ] **Step 4: Call it from `resolve_in_file_scope`**

In `resolve_in_file_scope`, insert the include check between the top-level-items lookup and the imports loop. After the `// Top-level items.` block that returns for a top-level match, and before `// Imports, in order.`, add:

```rust
        // Included files' top-level declarations are spliced into this scope.
        if let Some(def) =
            self.resolve_through_includes(pos.file, &root, name, &mut vec![pos.file])
        {
            return Some(def);
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core resolve`
Expected: PASS (3 new tests + all prior).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count 61 → 64.

```bash
git add crates/analyzer-core/src/resolve.rs
git commit -m "feat(core): resolve include as a textual splice across files"
```

---

### Task 4a: analyzer-core — VFS generation counter + `invalidate_disk`

The generation counter is the incremental engine's revision number: every VFS mutation bumps it, so cached derived facts and in-flight operations can detect staleness. `invalidate_disk` lets the file watcher (Task 13) drop stale cached disk content without disturbing an open overlay.

**Files:**
- Modify: `crates/analyzer-core/src/vfs.rs`
- Modify: `crates/analyzer-core/src/analysis.rs` (delegate `generation()`)

**Interfaces:**
- Produces: `Vfs::generation()`, `Vfs::invalidate_disk()`, `AnalysisHost::generation()`.

- [ ] **Step 1: Write the failing tests**

Add to `vfs.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn generation_bumps_only_on_mutation() {
        let mut vfs = Vfs::new();
        let g0 = vfs.generation();
        let f = vfs.file_id(std::path::Path::new("/t/a.compact"));
        assert_eq!(vfs.generation(), g0); // interning is not a mutation
        vfs.set_overlay(f, "x".to_string(), 1);
        let g1 = vfs.generation();
        assert!(g1 > g0);
        vfs.remove_overlay(f);
        assert!(vfs.generation() > g1);
    }

    #[test]
    fn invalidate_disk_rereads_but_overlay_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.compact");
        std::fs::write(&path, "v1").unwrap();
        let mut vfs = Vfs::new();
        let f = vfs.file_id(&path);
        assert_eq!(vfs.read(f).unwrap().as_ref(), "v1"); // caches disk content

        std::fs::write(&path, "v2").unwrap();
        vfs.invalidate_disk(f);
        assert_eq!(vfs.read(f).unwrap().as_ref(), "v2"); // re-read after invalidate

        vfs.set_overlay(f, "edit".to_string(), 1);
        std::fs::write(&path, "v3").unwrap();
        vfs.invalidate_disk(f);
        assert_eq!(vfs.read(f).unwrap().as_ref(), "edit"); // overlay is untouched
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core vfs::tests::generation_bumps_only_on_mutation`
Expected: FAIL — `generation`/`invalidate_disk` not defined.

- [ ] **Step 3: Implement on `Vfs`**

Add a field to the `Vfs` struct: `generation: u64,` (the `#[derive(Default)]` keeps it 0). Then bump it in the two existing mutators and add the two new methods:

```rust
    pub fn set_overlay(&mut self, file: FileId, text: String, version: i32) {
        self.contents.insert(
            file,
            Content::Overlay {
                text: Arc::from(text),
                version,
            },
        );
        self.generation += 1;
    }

    pub fn remove_overlay(&mut self, file: FileId) {
        self.contents.remove(&file);
        self.generation += 1;
    }

    /// Monotonic revision, bumped by every content mutation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Drops cached *disk* content so the next `read` re-reads from disk. An
    /// active overlay is left untouched (it always wins). Called by the file
    /// watcher on an external change.
    pub fn invalidate_disk(&mut self, file: FileId) {
        if matches!(self.contents.get(&file), Some(Content::Disk { .. })) {
            self.contents.remove(&file);
        }
        self.generation += 1;
    }
```

- [ ] **Step 4: Delegate on `AnalysisHost`**

In `analysis.rs`, add to `impl AnalysisHost`:

```rust
    /// Current workspace revision (see `Vfs::generation`).
    pub fn generation(&self) -> u64 {
        self.vfs.generation()
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core vfs`
Expected: PASS (2 new + all prior vfs tests).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count 64 → 66.

```bash
git add crates/analyzer-core/src/vfs.rs crates/analyzer-core/src/analysis.rs
git commit -m "feat(core): VFS generation counter and invalidate_disk"
```

---

### Task 4b: analyzer-core — workspace index (symbol table + dependency graph)

The eager index: for every workspace file, a table of its named symbols and the set of files it imports/includes, with reverse edges for dirty propagation. `index_file` (re)builds one file's entry; the crawl (Task 4c) calls it for every discovered file.

**Files:**
- Create: `crates/analyzer-core/src/workspace.rs`
- Modify: `crates/analyzer-core/src/analysis.rs` (own a `WorkspaceIndex`; add public methods)
- Modify: `crates/analyzer-core/src/lib.rs` (`mod workspace;`)

**Interfaces:**
- Consumes: `find_source_pathname`, `string_lit_text` (Task 1); `AnalysisHost::{analyze, vfs, vfs_mut, import_search_path}` (Task 2); `ItemTree`, `SymbolKind::Module`.
- Produces: `AnalysisHost::{index_file, remove_workspace_file, workspace_files, dependents_of, workspace_symbols}`; `pub(crate)` `WorkspaceIndex`.

- [ ] **Step 1: Create the WorkspaceIndex module (with the subsequence matcher)**

Create `crates/analyzer-core/src/workspace.rs`:

```rust
//! Eager workspace index: per-file symbol tables plus a forward/reverse
//! dependency graph over `import`/`include` edges. Rebuilt one file at a time
//! by `AnalysisHost::index_file`.

use std::collections::{BTreeMap, BTreeSet};

use crate::FileId;

#[derive(Default)]
pub(crate) struct WorkspaceIndex {
    files: BTreeSet<FileId>,
    /// Every named symbol per file: (name, symbol index into the ItemTree).
    symbols: BTreeMap<FileId, Vec<(String, u32)>>,
    /// file → files it imports/includes.
    forward: BTreeMap<FileId, BTreeSet<FileId>>,
    /// file → files that import/include it (dependents).
    reverse: BTreeMap<FileId, BTreeSet<FileId>>,
}

impl WorkspaceIndex {
    pub(crate) fn set_file(
        &mut self,
        file: FileId,
        symbols: Vec<(String, u32)>,
        deps: BTreeSet<FileId>,
    ) {
        self.files.insert(file);
        self.symbols.insert(file, symbols);
        // Detach stale reverse edges from the previous forward set.
        if let Some(old) = self.forward.get(&file) {
            for &d in old {
                if let Some(r) = self.reverse.get_mut(&d) {
                    r.remove(&file);
                }
            }
        }
        for &d in &deps {
            self.reverse.entry(d).or_default().insert(file);
        }
        self.forward.insert(file, deps);
    }

    pub(crate) fn remove(&mut self, file: FileId) {
        self.files.remove(&file);
        self.symbols.remove(&file);
        if let Some(old) = self.forward.remove(&file) {
            for d in old {
                if let Some(r) = self.reverse.get_mut(&d) {
                    r.remove(&file);
                }
            }
        }
        // Reverse edges *pointing at* `file` are intentionally kept so its
        // dependents get re-checked (their imports of it are now unresolved).
    }

    pub(crate) fn files(&self) -> Vec<FileId> {
        self.files.iter().copied().collect()
    }

    pub(crate) fn dependents(&self, file: FileId) -> Vec<FileId> {
        self.reverse
            .get(&file)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn symbols_matching(&self, query: &str) -> Vec<(FileId, u32)> {
        let mut out = Vec::new();
        for (&file, syms) in &self.symbols {
            for (name, idx) in syms {
                if subsequence_ci(query, name) {
                    out.push((file, *idx));
                }
            }
        }
        out
    }
}

/// Case-insensitive subsequence match: every char of `needle` appears in
/// `haystack` in order. An empty needle matches everything.
fn subsequence_ci(needle: &str, haystack: &str) -> bool {
    let mut hs = haystack.chars().flat_map(char::to_lowercase);
    'outer: for nc in needle.chars().flat_map(char::to_lowercase) {
        for hc in hs.by_ref() {
            if hc == nc {
                continue 'outer;
            }
        }
        return false;
    }
    true
}
```

Add `mod workspace;` to `crates/analyzer-core/src/lib.rs` (no re-export needed — `WorkspaceIndex` is `pub(crate)`).

- [ ] **Step 2: Add the `WorkspaceIndex` field + public methods to AnalysisHost, with failing tests**

In `analysis.rs`, ensure `use compactp_ast::AstNode;` is imported (required for `SourceFile::cast` below). Add the field to the struct: `workspace: crate::workspace::WorkspaceIndex,` and init it in `new()` with `workspace: crate::workspace::WorkspaceIndex::default(),`. Then add these methods to `impl AnalysisHost`:

```rust
    /// (Re)builds the workspace-index entry for one file: its symbol table and
    /// its outgoing import/include dependency edges. Reads/parses the file
    /// (and any freshly-referenced dependency targets) via the VFS. An
    /// unreadable file is evicted from the index.
    pub fn index_file(&mut self, file: FileId) {
        let Some(analysis) = self.analyze(file) else {
            self.workspace.remove(file);
            return;
        };
        let tree = analysis.item_tree.clone();
        let root = crate::SyntaxNode::new_root(analysis.green.clone());

        let symbols: Vec<(String, u32)> = tree
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.name.is_empty())
            .map(|(idx, s)| (s.name.clone(), idx as u32))
            .collect();

        let from_dir = self.vfs().path(file).parent().map(|d| d.to_path_buf());
        let search = self.import_search_path();
        let mut deps = std::collections::BTreeSet::new();
        if let (Some(from_dir), Some(sf)) =
            (from_dir, compactp_ast::SourceFile::cast(root.clone()))
        {
            for import in sf.imports() {
                let raw = if let Some(n) = import.name() {
                    let nm = n.text().to_string();
                    // Compiler-internal; not a file dependency.
                    if nm == "CompactStandardLibrary" {
                        continue;
                    }
                    // An in-scope module satisfies the import → no file edge.
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
                if let Some(path) = crate::find_source_pathname(&from_dir, &search, &raw) {
                    deps.insert(self.vfs_mut().file_id(&path));
                }
            }
            for inc in sf.includes() {
                if let Some(tok) = inc.path() {
                    let raw = crate::string_lit_text(tok.text());
                    if let Some(path) = crate::find_source_pathname(&from_dir, &search, &raw) {
                        deps.insert(self.vfs_mut().file_id(&path));
                    }
                }
            }
        }
        self.workspace.set_file(file, symbols, deps);
    }

    /// Evicts a file (deleted on disk) from the workspace index.
    pub fn remove_workspace_file(&mut self, file: FileId) {
        self.workspace.remove(file);
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
```

Add tests to `workspace.rs`'s `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod tests {
    use crate::AnalysisHost;

    fn write(dir: &std::path::Path, rel: &str, contents: &str) -> std::path::PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn indexes_symbols_and_dependency_edges() {
        let dir = tempfile::tempdir().unwrap();
        let foo_p = write(
            dir.path(),
            "Foo.compact",
            "module Foo { export circuit ffff(): Field { return 0; } }",
        );
        let main_p = write(
            dir.path(),
            "main.compact",
            "import Foo;\ncircuit mmmm(): Field { return ffff(); }",
        );
        let mut host = AnalysisHost::new();
        let foo = host.vfs_mut().file_id(&foo_p);
        let main = host.vfs_mut().file_id(&main_p);
        host.index_file(foo);
        host.index_file(main);

        // Nested + top-level symbols are both indexed.
        assert!(host.workspace_symbols("ffff").iter().any(|&(f, _)| f == foo));
        assert!(host.workspace_symbols("mmmm").iter().any(|&(f, _)| f == main));
        // main imports Foo → Foo's dependents include main.
        assert!(host.dependents_of(foo).contains(&main));
    }

    #[test]
    fn eviction_removes_symbols_and_edges() {
        let dir = tempfile::tempdir().unwrap();
        let foo_p = write(dir.path(), "Foo.compact", "module Foo { export circuit ffff(): Field { return 0; } }");
        let main_p = write(dir.path(), "main.compact", "import Foo;\ncircuit mmmm(): Field { return ffff(); }");
        let mut host = AnalysisHost::new();
        let foo = host.vfs_mut().file_id(&foo_p);
        let main = host.vfs_mut().file_id(&main_p);
        host.index_file(foo);
        host.index_file(main);

        host.remove_workspace_file(main);
        assert!(host.workspace_files().contains(&foo));
        assert!(!host.workspace_files().contains(&main));
        assert!(!host.dependents_of(foo).contains(&main));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail, then pass**

Run: `cargo test -p analyzer-core workspace`
Expected first: FAIL to compile (methods missing) → after Step 2's method additions, PASS (2 tests).

- [ ] **Step 4: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count 66 → 68.

```bash
git add crates/analyzer-core/src/workspace.rs crates/analyzer-core/src/analysis.rs crates/analyzer-core/src/lib.rs
git commit -m "feat(core): workspace index — symbol table and dependency graph"
```

---

### Task 4c: analyzer-core — gitignore-aware crawl + discover_and_index

Discover every `.compact` under the workspace roots (respecting `.gitignore`) and index them, polling a cancellation predicate between files.

**Files:**
- Modify: `Cargo.toml` (root — add `ignore` to `[workspace.dependencies]`)
- Modify: `crates/analyzer-core/Cargo.toml` (add `ignore.workspace = true`)
- Modify: `crates/analyzer-core/src/workspace.rs` (add `discover_compact_files`)
- Modify: `crates/analyzer-core/src/analysis.rs` (add `discover_and_index`)
- Modify: `crates/analyzer-core/src/lib.rs` (re-export `discover_compact_files`)

**Interfaces:**
- Consumes: `ignore::WalkBuilder`; `AnalysisHost::{index_file, vfs_mut}` (Task 4b).
- Produces: `workspace::discover_compact_files(roots) -> Vec<PathBuf>`; `AnalysisHost::discover_and_index(roots, should_continue)`.

- [ ] **Step 1: Add the `ignore` dependency**

In the root `Cargo.toml` under `[workspace.dependencies]`, add (near `tempfile`): `ignore = "0.4"`. In `crates/analyzer-core/Cargo.toml` under `[dependencies]`, add: `ignore.workspace = true`.

Run: `cargo build -p analyzer-core --locked` (resolves `ignore` v0.4.27 into `Cargo.lock`).
Expected: builds; `Cargo.lock` gains `ignore` and its transitive deps.

- [ ] **Step 2: Write the failing tests**

Add to `workspace.rs`'s `#[cfg(test)] mod tests`:

```rust
    use crate::workspace::discover_compact_files;

    #[test]
    fn discover_respects_gitignore_and_extension() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.compact", "circuit a(): Field { return 0; }");
        write(dir.path(), "note.txt", "not compact");
        write(dir.path(), "vendored/b.compact", "circuit b(): Field { return 0; }");
        write(dir.path(), ".gitignore", "vendored/\n");

        let found = discover_compact_files(&[dir.path().to_path_buf()]);
        let names: Vec<String> = found
            .iter()
            .filter_map(|p| p.file_name()?.to_str().map(str::to_string))
            .collect();
        assert!(names.iter().any(|n| n == "a.compact"));
        assert!(!names.iter().any(|n| n == "b.compact")); // gitignored
        assert!(!names.iter().any(|n| n == "note.txt")); // wrong extension
    }

    #[test]
    fn discover_and_index_populates_and_can_cancel() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "one.compact", "circuit one(): Field { return 0; }");
        write(dir.path(), "two.compact", "circuit two(): Field { return 0; }");
        let mut host = AnalysisHost::new();

        // Cancel immediately: nothing indexed.
        host.discover_and_index(&[dir.path().to_path_buf()], &|| false);
        assert!(host.workspace_files().is_empty());

        // Run to completion: both indexed.
        host.discover_and_index(&[dir.path().to_path_buf()], &|| true);
        assert_eq!(host.workspace_files().len(), 2);
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p analyzer-core workspace::tests::discover_respects_gitignore_and_extension`
Expected: FAIL — `discover_compact_files` / `discover_and_index` not defined.

- [ ] **Step 4: Implement `discover_compact_files`**

Add to `workspace.rs` (above the tests):

```rust
use std::path::PathBuf;

/// Every `*.compact` file under `roots`, gitignore-aware. `require_git(false)`
/// applies `.gitignore` even outside a git repository (so tests and
/// not-yet-committed projects behave the same); hidden files/dirs (`.git`,
/// dotfiles) are skipped by default.
pub fn discover_compact_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        let walker = ignore::WalkBuilder::new(root).require_git(false).build();
        for entry in walker.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                && entry.path().extension().and_then(|e| e.to_str()) == Some("compact")
            {
                out.push(entry.path().to_path_buf());
            }
        }
    }
    out
}
```

- [ ] **Step 5: Implement `discover_and_index`**

Add to `analysis.rs`'s `impl AnalysisHost`:

```rust
    /// Discovers and indexes every `.compact` under `roots`. Polls
    /// `should_continue` before each file so a superseded crawl abandons early.
    pub fn discover_and_index(&mut self, roots: &[std::path::PathBuf], should_continue: &dyn Fn() -> bool) {
        for path in crate::workspace::discover_compact_files(roots) {
            if !should_continue() {
                return;
            }
            let file = self.vfs_mut().file_id(&path);
            self.index_file(file);
        }
    }
```

Add the re-export to `lib.rs`: extend the `pub use workspace::…` line (add one if none) with `pub use workspace::discover_compact_files;`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core workspace`
Expected: PASS (2 new + prior workspace tests).

- [ ] **Step 7: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count 68 → 70.

```bash
git add Cargo.toml Cargo.lock crates/analyzer-core/Cargo.toml crates/analyzer-core/src/workspace.rs crates/analyzer-core/src/analysis.rs crates/analyzer-core/src/lib.rs
git commit -m "feat(core): gitignore-aware workspace crawl and eager indexing"
```

---

### Task 5: analyzer-core — cache polish (Arc-pointer identity + eviction)

Replace the content-hash cache key with the returned `Arc<str>`'s pointer identity. This removes the hash-recomputed-on-every-hit cost AND the `DefaultHasher` 64-bit collision risk in one change: the pointer changes exactly when the VFS replaces a file's content, and cannot be reused while cached because the cache holds the `Arc` alive (via `line_index.text`). Add `forget_file` for eviction, wired into `remove_workspace_file`.

**Files:**
- Modify: `crates/analyzer-core/src/analysis.rs`

**Interfaces:**
- Produces: `AnalysisHost::forget_file(file)`. Preserves the existing `analyze` signature and cache-hit semantics.

- [ ] **Step 1: Write the failing test (eviction) — existing cache tests must still pass**

Add to `analysis.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn forget_file_forces_recompute() {
        let (mut host, file) = host_with("ledger count: Field;");
        let first = host.analyze(file).unwrap();
        host.forget_file(file);
        let second = host.analyze(file).unwrap();
        // Content is unchanged, but the cache entry was dropped → recomputed →
        // a fresh diagnostics Arc.
        assert!(!std::sync::Arc::ptr_eq(&first.diagnostics, &second.diagnostics));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p analyzer-core analysis::tests::forget_file_forces_recompute`
Expected: FAIL — `forget_file` not defined.

- [ ] **Step 3: Switch the cache to pointer identity**

In `analysis.rs`: remove `use std::collections::hash_map::DefaultHasher;` and `use std::hash::{Hash, Hasher};`, and delete the `fn content_hash` function. Change the cache field type from `HashMap<FileId, (u64, FileAnalysis)>` to `HashMap<FileId, (usize, FileAnalysis)>`. Rewrite `analyze`:

```rust
    /// Parses `file`, memoized on the VFS content's `Arc` identity (which
    /// changes exactly when the content is replaced). `None` if unreadable.
    pub fn analyze(&mut self, file: FileId) -> Option<FileAnalysis> {
        let text = self.vfs.read(file)?;
        let key = Arc::as_ptr(&text) as *const u8 as usize;
        if let Some((cached_key, analysis)) = self.cache.get(&file)
            && *cached_key == key
        {
            return Some(analysis.clone());
        }
        let result = compactp_parser::parse(&text);
        let root = compactp_syntax::SyntaxNode::new_root(result.green.clone());
        let analysis = FileAnalysis {
            green: result.green,
            diagnostics: Arc::new(result.errors),
            line_index: Arc::new(LineIndex::new(text)),
            item_tree: Arc::new(ItemTree::extract(&root)),
        };
        self.cache.insert(file, (key, analysis.clone()));
        Some(analysis)
    }
```

> Correctness note for the reviewer: the cache entry's `FileAnalysis.line_index` holds a clone of the same `Arc<str>` returned by `read()`, so the allocation stays alive while cached; the VFS's replacement `Arc` on the next edit therefore gets a distinct address (no ABA). `Arc::as_ptr` yields a `*const str` fat pointer; `as *const u8` drops the length metadata before `as usize`.

- [ ] **Step 4: Add `forget_file` and wire eviction**

Add to `impl AnalysisHost`:

```rust
    /// Drops the cached parse for `file` (e.g. deleted on disk), forcing a
    /// recompute on next access.
    pub fn forget_file(&mut self, file: FileId) {
        self.cache.remove(&file);
    }
```

Update `remove_workspace_file` (from Task 4b) to also evict the parse cache:

```rust
    pub fn remove_workspace_file(&mut self, file: FileId) {
        self.workspace.remove(file);
        self.forget_file(file);
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p analyzer-core analysis`
Expected: PASS — `forget_file_forces_recompute`, `unchanged_content_hits_the_cache`, `edit_invalidates_the_cache`, and the rest still green.

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count 70 → 71.

```bash
git add crates/analyzer-core/src/analysis.rs
git commit -m "refactor(core): pointer-identity parse cache; add eviction"
```

---

### Task 6: analyzer-core — unresolvable-import diagnostics

Error-severity diagnostics for imports/includes that fail resolution, anchored on the path/name token, only after the full search fails. Computed on demand in the resolver (a cross-file derived fact — deliberately NOT part of the content-hash parse cache).

**Files:**
- Modify: `crates/analyzer-core/src/resolve.rs` (add `resolution_diagnostics` + helpers; add tests)

**Interfaces:**
- Consumes: `find_source_pathname`, `string_lit_text`, `path_module_name` (Task 1); `single_top_level_module` (Task 2, make `pub(crate)`-visible within the module); `Diagnostic`, `DiagnosticCode` (re-exported from `compactp_diagnostics`).
- Produces: `AnalysisHost::resolution_diagnostics(file) -> Vec<Diagnostic>`.

- [ ] **Step 1: Write the failing tests**

Add to `resolve.rs`'s `#[cfg(test)] mod tests`:

```rust
    fn diag_host(files: &[(&str, &str)], open_rel: &str) -> (AnalysisHost, tempfile::TempDir, FileId) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, contents) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, contents).unwrap();
        }
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(&dir.path().join(open_rel));
        (host, dir, file)
    }

    #[test]
    fn unresolved_import_is_an_error() {
        let (mut host, _dir, file) = diag_host(
            &[("main.compact", "import Missing;\ncircuit m(): Field { return 0; }")],
            "main.compact",
        );
        let diags = host.resolution_diagnostics(file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, analyzer_core::Severity::Error);
        assert!(diags[0].message.contains("cannot resolve import"));
        assert!(diags[0].message.contains("Missing"));
    }

    #[test]
    fn resolvable_import_has_no_diagnostic() {
        let (mut host, _dir, file) = diag_host(
            &[
                ("Foo.compact", "module Foo { export circuit f(): Field { return 0; } }"),
                ("main.compact", "import Foo;\ncircuit m(): Field { return 0; }"),
            ],
            "main.compact",
        );
        assert!(host.resolution_diagnostics(file).is_empty());
    }

    #[test]
    fn module_name_mismatch_is_an_error() {
        let (mut host, _dir, file) = diag_host(
            &[
                ("Foo.compact", "module NotFoo { export circuit f(): Field { return 0; } }"),
                ("main.compact", "import Foo;\ncircuit m(): Field { return 0; }"),
            ],
            "main.compact",
        );
        let diags = host.resolution_diagnostics(file);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("rather than expected"));
    }

    #[test]
    fn unresolved_include_is_an_error() {
        let (mut host, _dir, file) = diag_host(
            &[("main.compact", "include \"nope\";\ncircuit m(): Field { return 0; }")],
            "main.compact",
        );
        let diags = host.resolution_diagnostics(file);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("cannot resolve include"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core resolve::tests::unresolved_import_is_an_error`
Expected: FAIL — `resolution_diagnostics` not defined.

- [ ] **Step 3: Make `single_top_level_module` visible + add imports**

Change `fn single_top_level_module` (Task 2) to `pub(crate) fn single_top_level_module`. At the top of `resolve.rs`, ensure these imports exist (add any missing): `use std::path::{Path, PathBuf};` and `use compactp_diagnostics::{Diagnostic, DiagnosticCode};`.

- [ ] **Step 4: Implement `resolution_diagnostics` + helpers**

Add to `impl crate::AnalysisHost` in `resolve.rs`:

```rust
    /// Error diagnostics for imports/includes in `file` that fail resolution.
    /// A cross-file derived fact — recomputed on demand, not cached with the
    /// per-file parse.
    pub fn resolution_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic> {
        let Some(analysis) = self.analyze(file) else {
            return Vec::new();
        };
        let tree = analysis.item_tree.clone();
        let root = SyntaxNode::new_root(analysis.green.clone());
        let Some(sf) = compactp_ast::SourceFile::cast(root) else {
            return Vec::new();
        };
        let Some(from_dir) = self.vfs().path(file).parent().map(Path::to_path_buf) else {
            return Vec::new();
        };
        let search = self.import_search_path();
        let mut diags = Vec::new();
        for import in sf.imports() {
            if let Some(name) = import.name() {
                let nm = name.text().to_string();
                if nm == "CompactStandardLibrary" {
                    continue;
                }
                if tree
                    .top_level()
                    .any(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == nm)
                {
                    continue; // satisfied by an in-scope module
                }
                match crate::find_source_pathname(&from_dir, &search, &nm) {
                    None => diags.push(unresolved_import_diag(&nm, &from_dir, &search, name.text_range())),
                    Some(path) => {
                        let target = self.vfs_mut().file_id(&path);
                        if let Some(d) = self.module_mismatch_diag(target, &nm, name.text_range()) {
                            diags.push(d);
                        }
                    }
                }
            } else if let Some(ptok) = import.path() {
                let raw = crate::string_lit_text(ptok.text());
                let span = ptok.text_range();
                let Some(expected) = crate::path_module_name(&raw) else {
                    continue;
                };
                match crate::find_source_pathname(&from_dir, &search, &raw) {
                    None => diags.push(unresolved_import_diag(&raw, &from_dir, &search, span)),
                    Some(path) => {
                        let target = self.vfs_mut().file_id(&path);
                        if let Some(d) = self.module_mismatch_diag(target, &expected, span) {
                            diags.push(d);
                        }
                    }
                }
            }
        }
        for inc in sf.includes() {
            if let Some(tok) = inc.path() {
                let raw = crate::string_lit_text(tok.text());
                if crate::find_source_pathname(&from_dir, &search, &raw).is_none() {
                    diags.push(unresolved_include_diag(&raw, &from_dir, &search, tok.text_range()));
                }
            }
        }
        diags
    }

    /// Error iff `target`'s single module does not match `expected`.
    fn module_mismatch_diag(
        &mut self,
        target: FileId,
        expected: &str,
        span: TextRange,
    ) -> Option<Diagnostic> {
        let tree = self.analyze(target)?.item_tree.clone();
        match single_top_level_module(&tree) {
            None => Some(Diagnostic::error(
                DiagnosticCode::new("E", 9003),
                format!("imported file does not contain a single `module {expected}`"),
                span,
            )),
            Some((_, sym)) if sym.name != expected => Some(Diagnostic::error(
                DiagnosticCode::new("E", 9002),
                format!(
                    "imported file defines module `{}` rather than expected `{expected}`",
                    sym.name
                ),
                span,
            )),
            Some(_) => None,
        }
    }
```

And add these free functions near the other helpers:

```rust
fn searched_list(from_dir: &Path, search: &[PathBuf]) -> String {
    let mut parts = vec![from_dir.display().to_string()];
    parts.extend(search.iter().map(|p| p.display().to_string()));
    parts.join(", ")
}

fn unresolved_import_diag(raw: &str, from_dir: &Path, search: &[PathBuf], span: TextRange) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 9001),
        format!(
            "cannot resolve import `{raw}` (searched: {})",
            searched_list(from_dir, search)
        ),
        span,
    )
}

fn unresolved_include_diag(raw: &str, from_dir: &Path, search: &[PathBuf], span: TextRange) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 9004),
        format!(
            "cannot resolve include `{raw}` (searched: {})",
            searched_list(from_dir, search)
        ),
        span,
    )
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core resolve`
Expected: PASS (4 new + all prior).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count 71 → 75.

```bash
git add crates/analyzer-core/src/resolve.rs
git commit -m "feat(core): error diagnostics for unresolvable imports and includes"
```

---

### Task 7: analyzer-ide — workspace-wide find-references (cancellable)

Extend find-references across the workspace: scan every indexed file whose text contains the name, re-resolve each candidate, keep matches. Cancellable via a `should_continue` predicate. The existing `find_references(host, pos, include_declaration)` is preserved as a wrapper passing `&|| true`, so M2a tests and the binary keep working unchanged.

**Files:**
- Modify: `crates/analyzer-ide/src/references.rs`
- Modify: `crates/analyzer-ide/src/lib.rs` (re-export `find_references_cancellable`)

**Interfaces:**
- Consumes: `AnalysisHost::{resolve, def_name, nav_info, analyze, workspace_files, vfs_mut}`; `Definition` (add to imports).
- Produces: `find_references_cancellable(host, pos, include_declaration, should_continue) -> Option<Vec<NavTarget>>`.

- [ ] **Step 1: Write the failing test**

Add to `references.rs`'s `#[cfg(test)] mod tests`:

```rust
    use analyzer_core::TextSize;

    #[test]
    fn references_span_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Foo.compact"),
            "module Foo { export circuit ff(): Field { return 0; } }",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("main.compact"),
            "import Foo;\ncircuit m(): Field { return ff() + ff(); }",
        )
        .unwrap();
        let mut host = AnalysisHost::new();
        host.discover_and_index(&[dir.path().to_path_buf()], &|| true);

        let foo = host.vfs_mut().file_id(&dir.path().join("Foo.compact"));
        let main = host.vfs_mut().file_id(&dir.path().join("main.compact"));
        let foo_text = host.vfs_mut().read(foo).unwrap();
        let off = foo_text.find("ff(").unwrap() as u32; // the `ff` declaration
        let refs = find_references(
            &mut host,
            FilePosition { file: foo, offset: TextSize::new(off) },
            true,
        )
        .unwrap();

        assert_eq!(refs.len(), 3); // declaration in Foo + two uses in main
        assert_eq!(refs.iter().filter(|r| r.file == main).count(), 2);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p analyzer-ide references::tests::references_span_the_workspace`
Expected: FAIL — only same-file references found (1, not 3).

- [ ] **Step 3: Rewrite `find_references` over the workspace**

Replace the body of `references.rs` (keep the module doc comment updated) with:

```rust
//! Find-references across the workspace. Each candidate IDENT with matching
//! text is re-resolved and kept only if it resolves to the same definition —
//! shadowing-aware and cross-file. Local bindings stay single-file.

use analyzer_core::{AnalysisHost, Definition, FilePosition, SyntaxKind, SyntaxNode};

use crate::NavTarget;

/// All references to the definition at `pos`, workspace-wide. `should_continue`
/// is polled per file; returning `false` abandons the scan (answers `None`).
pub fn find_references_cancellable(
    host: &mut AnalysisHost,
    pos: FilePosition,
    include_declaration: bool,
    should_continue: &dyn Fn() -> bool,
) -> Option<Vec<NavTarget>> {
    let def = host.resolve(pos)?;
    let name = host.def_name(&def)?;
    let (def_file, def_name_range, _) = host.nav_info(&def)?;

    // Local bindings are file-local; items are workspace-wide.
    let files: Vec<_> = if matches!(def, Definition::Local { .. }) {
        vec![pos.file]
    } else {
        let mut v = host.workspace_files();
        v.push(pos.file);
        v.push(def_file);
        v.sort();
        v.dedup();
        v
    };

    let mut out = Vec::new();
    for file in files {
        if !should_continue() {
            return None;
        }
        // Cheap name-presence prune: skip files that don't mention the name.
        let Some(text) = host.vfs_mut().read(file) else {
            continue;
        };
        if !text.contains(name.as_str()) {
            continue;
        }
        let Some(analysis) = host.analyze(file) else {
            continue;
        };
        let root = SyntaxNode::new_root(analysis.green.clone());
        let candidates: Vec<_> = root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == SyntaxKind::IDENT && t.text() == name)
            .map(|t| t.text_range())
            .collect();
        for range in candidates {
            let is_declaration = file == def_file && range == def_name_range;
            if is_declaration && !include_declaration {
                continue;
            }
            let resolved = host.resolve(FilePosition { file, offset: range.start() });
            if resolved.as_ref() == Some(&def) {
                out.push(NavTarget { file, name_range: range, full_range: range });
            }
        }
    }
    out.sort_by_key(|t| (t.file, t.name_range.start()));
    out.dedup();
    Some(out)
}

/// Non-cancellable convenience wrapper (used by tests and simple callers).
pub fn find_references(
    host: &mut AnalysisHost,
    pos: FilePosition,
    include_declaration: bool,
) -> Option<Vec<NavTarget>> {
    find_references_cancellable(host, pos, include_declaration, &|| true)
}
```

Keep the existing `#[cfg(test)] mod tests` block below this (add the new test from Step 1 to it).

- [ ] **Step 4: Re-export the cancellable variant**

In `crates/analyzer-ide/src/lib.rs`, change `pub use references::find_references;` to `pub use references::{find_references, find_references_cancellable};`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p analyzer-ide references`
Expected: PASS — the new cross-file test plus the four M2a single-file tests (which run with an empty workspace, so `files` collapses to the one open file).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-ide --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`
Expected: clean; ide test count 16 → 17.

```bash
git add crates/analyzer-ide/src/references.rs crates/analyzer-ide/src/lib.rs
git commit -m "feat(ide): workspace-wide find-references with cancellation"
```

---

### Task 8: analyzer-ide — workspace-wide rename + per-use-site conflicts

Rename produces edits across all files, and the conservative single-site conflict check (Errata E3) becomes full per-use-site detection: at every reference site, `new_name` must resolve to nothing or the same definition.

**Files:**
- Modify: `crates/analyzer-ide/src/rename.rs`
- Modify: `crates/analyzer-ide/src/lib.rs` (re-export `rename_cancellable`)

**Interfaces:**
- Consumes: `find_references_cancellable` (Task 7); `AnalysisHost::{resolve, nav_info, stdlib_file, resolve_name_at}`; `Definition`.
- Produces: `rename_cancellable(host, pos, new_name, should_continue) -> Result<Vec<SourceEdit>, RenameError>`.

- [ ] **Step 1: Write the failing test**

Add to `rename.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn renames_across_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Foo.compact"),
            "module Foo { export circuit ff(): Field { return 0; } }",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("main.compact"),
            "import Foo;\ncircuit m(): Field { return ff() + ff(); }",
        )
        .unwrap();
        let mut host = AnalysisHost::new();
        host.discover_and_index(&[dir.path().to_path_buf()], &|| true);
        let foo = host.vfs_mut().file_id(&dir.path().join("Foo.compact"));
        let main = host.vfs_mut().file_id(&dir.path().join("main.compact"));
        let foo_text = host.vfs_mut().read(foo).unwrap();
        let off = foo_text.find("ff(").unwrap() as u32;

        let edits = rename(
            &mut host,
            FilePosition { file: foo, offset: analyzer_core::TextSize::new(off) },
            "gg",
        )
        .unwrap();
        assert_eq!(edits.len(), 3); // declaration + two cross-file uses
        assert_eq!(edits.iter().filter(|e| e.file == main).count(), 2);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p analyzer-ide rename::tests::renames_across_files`
Expected: FAIL — only the single-file declaration edit is produced.

- [ ] **Step 3: Rewrite `rename` over the workspace with per-use-site conflicts**

Update the module doc comment at the top of `rename.rs` to:

```rust
//! Rename: validate the new name, gather all references workspace-wide via
//! `find_references_cancellable`, then run a per-use-site conflict check — at
//! every reference site the new name must resolve to nothing or to the same
//! definition. Edits span every file that references the symbol.
```

Change the imports line to `use analyzer_core::{AnalysisHost, Definition, FilePosition, TextRange};` and add `use crate::find_references_cancellable;` (replacing `use crate::find_references;`). Replace the `pub fn rename(…) { … }` body with these two functions:

```rust
pub fn rename_cancellable(
    host: &mut AnalysisHost,
    pos: FilePosition,
    new_name: &str,
    should_continue: &dyn Fn() -> bool,
) -> Result<Vec<SourceEdit>, RenameError> {
    if !is_valid_identifier(new_name) {
        return Err(RenameError::InvalidName(new_name.to_string()));
    }
    if KEYWORDS.contains(&new_name) {
        return Err(RenameError::Keyword(new_name.to_string()));
    }
    let def = host.resolve(pos).ok_or(RenameError::NotFound)?;
    let (def_file, _, _) = host.nav_info(&def).ok_or(RenameError::NotFound)?;
    if Some(def_file) == host.stdlib_file() {
        return Err(RenameError::BuiltinTarget);
    }
    let refs =
        find_references_cancellable(host, pos, true, should_continue).ok_or(RenameError::NotFound)?;

    // Per-use-site conflict detection: at each reference site, the new name
    // must not already resolve to a *different* definition (which the rename
    // would shadow or collide with). Works across files.
    for r in &refs {
        if let Some(existing) =
            host.resolve_name_at(FilePosition { file: r.file, offset: r.name_range.start() }, new_name)
            && existing != def
        {
            return Err(RenameError::Conflict(new_name.to_string()));
        }
    }

    Ok(refs
        .into_iter()
        .map(|t| SourceEdit {
            file: t.file,
            range: t.name_range,
            new_text: new_name.to_string(),
        })
        .collect())
}

/// Non-cancellable convenience wrapper (used by tests and simple callers).
pub fn rename(
    host: &mut AnalysisHost,
    pos: FilePosition,
    new_name: &str,
) -> Result<Vec<SourceEdit>, RenameError> {
    rename_cancellable(host, pos, new_name, &|| true)
}
```

Remove the now-unused `TextRange` import if clippy flags it (the conflict check no longer needs the def name range). Keep `is_valid_identifier`, `KEYWORDS`, `SourceEdit`, `RenameError` unchanged.

- [ ] **Step 4: Re-export the cancellable variant**

In `crates/analyzer-ide/src/lib.rs`, change `pub use rename::{RenameError, SourceEdit, rename};` to `pub use rename::{RenameError, SourceEdit, rename, rename_cancellable};`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p analyzer-ide rename`
Expected: PASS — new cross-file test plus the four M2a rename tests (`renames_definition_and_all_uses`, `refuses_keywords_and_invalid_names`, `refuses_conflicting_target_name`, `refuses_stdlib_targets`). The E3 conflict test still passes because the `b` binding is visible at the `return a + b` use site.

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-ide --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`
Expected: clean; ide test count 17 → 18.

```bash
git add crates/analyzer-ide/src/rename.rs crates/analyzer-ide/src/lib.rs
git commit -m "feat(ide): workspace-wide rename with per-use-site conflict detection"
```

---

### Task 9: analyzer-ide — workspace symbols

A feature-layer function over the core symbol index, producing editor-ready items (name, kind, location, container).

**Files:**
- Create: `crates/analyzer-ide/src/workspace_symbols.rs`
- Modify: `crates/analyzer-ide/src/lib.rs` (`mod` + re-export)

**Interfaces:**
- Consumes: `AnalysisHost::{workspace_symbols, analyze}` (Task 4b); `ItemTree.symbols`, `Symbol.{name, kind, name_range, parent}`.
- Produces: `WorkspaceSymbolItem`, `workspace_symbols(host, query) -> Vec<WorkspaceSymbolItem>`.

- [ ] **Step 1: Write the failing test**

Create `crates/analyzer-ide/src/workspace_symbols.rs`:

```rust
//! Workspace symbols: query the core symbol index and enrich each hit with its
//! kind, name range, and container (parent) name. User code only — the bundled
//! stdlib is excluded by the core index.

use analyzer_core::{AnalysisHost, FileId, SymbolKind, TextRange};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSymbolItem {
    pub file: FileId,
    pub name: String,
    pub kind: SymbolKind,
    pub name_range: TextRange,
    pub container: Option<String>,
}

/// Symbols whose name subsequence-matches `query` (case-insensitive), across
/// the whole workspace index.
pub fn workspace_symbols(host: &mut AnalysisHost, query: &str) -> Vec<WorkspaceSymbolItem> {
    let hits = host.workspace_symbols(query);
    let mut out = Vec::new();
    for (file, idx) in hits {
        let Some(analysis) = host.analyze(file) else {
            continue;
        };
        let Some(sym) = analysis.item_tree.symbols.get(idx as usize) else {
            continue;
        };
        let container = sym
            .parent
            .and_then(|p| analysis.item_tree.symbols.get(p as usize))
            .map(|s| s.name.clone());
        out.push(WorkspaceSymbolItem {
            file,
            name: sym.name.clone(),
            kind: sym.kind,
            name_range: sym.name_range,
            container,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_symbols_by_subsequence_across_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.compact"),
            "circuit transfer(): Field { return 0; }",
        )
        .unwrap();
        std::fs::write(dir.path().join("b.compact"), "struct Holder { x: Field; }").unwrap();
        let mut host = AnalysisHost::new();
        host.discover_and_index(&[dir.path().to_path_buf()], &|| true);

        let hits = workspace_symbols(&mut host, "trns"); // subsequence of "transfer"
        assert!(hits.iter().any(|s| s.name == "transfer"));
        // A struct field carries its struct as the container.
        let field = workspace_symbols(&mut host, "x")
            .into_iter()
            .find(|s| s.name == "x" && s.kind == SymbolKind::StructField);
        assert_eq!(field.and_then(|s| s.container).as_deref(), Some("Holder"));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p analyzer-ide workspace_symbols`
Expected: FAIL to compile — module not declared.

- [ ] **Step 3: Wire the module + re-exports**

In `crates/analyzer-ide/src/lib.rs`, add `mod workspace_symbols;` alongside the other `mod` lines and add `pub use workspace_symbols::{WorkspaceSymbolItem, workspace_symbols};`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p analyzer-ide workspace_symbols`
Expected: PASS.

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-ide --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`
Expected: clean; ide test count 18 → 19.

```bash
git add crates/analyzer-ide/src/workspace_symbols.rs crates/analyzer-ide/src/lib.rs
git commit -m "feat(ide): workspace symbols over the core index"
```

---

### Task 10: binary — consume `initialize` params, config, and run the crawl

Stop ignoring `initialize` params: derive workspace roots (`workspaceFolders`/`rootUri`), the import search path (`initializationOptions.importSearchPath`, defaulting to `COMPACT_PATH`), and whether the client supports dynamic watcher registration. Set the search path, remember the roots + capability, and run the initial crawl. Pure parsing helpers are unit-tested.

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs`

**Interfaces:**
- Consumes: `AnalysisHost::{set_import_search_path, discover_and_index}` (Tasks 2/4c); `lsp_utils::abs_path_from_uri`.
- Produces: `GlobalState` fields `workspace_roots`, `watch_supported`, `cancel_receiver`, `next_request_id`; helpers `workspace_roots_from_params`, `import_search_path_from`, `client_supports_watch`.

- [ ] **Step 1: Write the failing unit tests**

Add a `#[cfg(test)] mod tests` to `server.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workspace_roots_prefers_folders_then_root_uri() {
        let base = std::env::temp_dir();
        let a = base.join("wsA");
        let uri_a = Url::from_file_path(&a).unwrap();
        let params = json!({ "workspaceFolders": [{ "uri": uri_a, "name": "A" }] });
        assert_eq!(workspace_roots_from_params(&params), vec![a.clone()]);

        let params = json!({ "rootUri": uri_a });
        assert_eq!(workspace_roots_from_params(&params), vec![a]);

        assert!(workspace_roots_from_params(&json!({})).is_empty());
    }

    #[test]
    fn import_search_path_reads_options() {
        let opts = json!({ "importSearchPath": ["/libs/x", "/libs/y"] });
        assert_eq!(
            import_search_path_from(Some(&opts)),
            vec![PathBuf::from("/libs/x"), PathBuf::from("/libs/y")]
        );
    }

    #[test]
    fn client_watch_capability_is_detected() {
        let yes = json!({ "capabilities": { "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } } });
        assert!(client_supports_watch(&yes));
        assert!(!client_supports_watch(&json!({ "capabilities": {} })));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p compact-analyzer --lib`
Expected: FAIL to compile — helpers not defined.

- [ ] **Step 3: Add imports, fields, helpers**

At the top of `server.rs`, add `use std::path::PathBuf;` to the `std` imports. Add these fields to `struct GlobalState`:

```rust
    /// Roots to (re)crawl for `.compact` files.
    workspace_roots: Vec<PathBuf>,
    /// The client supports dynamic `didChangeWatchedFiles` registration.
    watch_supported: bool,
    /// A clone of the connection receiver, used only to test emptiness for
    /// cooperative cancellation (single-threaded — no concurrent consumer).
    cancel_receiver: crossbeam_channel::Receiver<Message>,
    /// Id counter for server→client requests (e.g. registerCapability).
    next_request_id: i32,
```

Change `GlobalState::new` to take the receiver and init the new fields:

```rust
    fn new(
        sender: crossbeam_channel::Sender<Message>,
        cancel_receiver: crossbeam_channel::Receiver<Message>,
    ) -> Self {
        Self {
            sender,
            host: AnalysisHost::new(),
            open_files: HashMap::new(),
            pending_diagnostics: HashSet::new(),
            debounce_deadline: None,
            workspace_roots: Vec::new(),
            watch_supported: false,
            cancel_receiver,
            next_request_id: 0,
        }
    }
```

Add these free functions near `panic_message`:

```rust
fn workspace_roots_from_params(params: &serde_json::Value) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(folders) = params.get("workspaceFolders").and_then(|v| v.as_array()) {
        for f in folders {
            if let Some(uri) = f.get("uri").and_then(|u| u.as_str())
                && let Ok(url) = Url::parse(uri)
                && let Some(p) = lsp_utils::abs_path_from_uri(&url)
            {
                roots.push(p);
            }
        }
    }
    if roots.is_empty()
        && let Some(uri) = params.get("rootUri").and_then(|u| u.as_str())
        && let Ok(url) = Url::parse(uri)
        && let Some(p) = lsp_utils::abs_path_from_uri(&url)
    {
        roots.push(p);
    }
    roots
}

fn import_search_path_from(options: Option<&serde_json::Value>) -> Vec<PathBuf> {
    if let Some(opts) = options
        && let Some(arr) = opts.get("importSearchPath").and_then(|v| v.as_array())
    {
        return arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(PathBuf::from)
            .collect();
    }
    std::env::var_os("COMPACT_PATH")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default()
}

fn client_supports_watch(params: &serde_json::Value) -> bool {
    params
        .pointer("/capabilities/workspace/didChangeWatchedFiles/dynamicRegistration")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
```

- [ ] **Step 4: Wire params + crawl into `run`**

Change `let (initialize_id, _initialize_params) = connection.initialize_start()?;` to `let (initialize_id, initialize_params) = connection.initialize_start()?;`. Change the state constructor to `let mut state = GlobalState::new(connection.sender.clone(), connection.receiver.clone());`. After the stdlib `materialize` block and before the main `loop {`, add:

```rust
    // M2b: consume workspace configuration and build the eager index.
    state.host.set_import_search_path(import_search_path_from(
        initialize_params.get("initializationOptions"),
    ));
    state.workspace_roots = workspace_roots_from_params(&initialize_params);
    state.watch_supported = client_supports_watch(&initialize_params);
    eprintln!(
        "compact-analyzer: indexing {} workspace root(s)",
        state.workspace_roots.len()
    );
    let roots = state.workspace_roots.clone();
    state.host.discover_and_index(&roots, &|| true);
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p compact-analyzer --lib`
Expected: PASS (3 new unit tests + the existing 6).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p compact-analyzer --all-targets --locked -- -D warnings && cargo test -p compact-analyzer`
Expected: clean; binary unit test count 6 → 9; integration/navigation unchanged.

```bash
git add crates/compact-analyzer/src/server.rs
git commit -m "feat(server): consume initialize params, config, and crawl the workspace"
```

---

### Task 11: binary — `workspace/symbol` capability + handler

Advertise and serve `workspace/symbol` from the core index.

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs`
- Modify: `crates/compact-analyzer/tests/support/mod.rs` (add a root-aware `initialize`)
- Create: `crates/compact-analyzer/tests/lsp_workspace.rs`

**Interfaces:**
- Consumes: `analyzer_ide::workspace_symbols` (Task 9); `symbol_kind_to_lsp`, `lsp_utils::range_to_lsp`.
- Produces: `GlobalState::workspace_symbol_infos`; a `workspace/symbol` LSP response; `Client::initialize_root` test helper.

- [ ] **Step 1: Add the root-aware initialize helper + failing integration test**

In `crates/compact-analyzer/tests/support/mod.rs`, add to `impl Client`:

```rust
    /// Initialize with an explicit workspace root and client capabilities.
    pub fn initialize_root(&mut self, root: &lsp_types::Url, caps: Value) {
        let response = self.request(
            "initialize",
            json!({ "capabilities": caps, "rootUri": root }),
        );
        assert!(response.get("result").is_some(), "initialize failed: {response}");
        self.notify("initialized", json!({}));
    }
```

Create `crates/compact-analyzer/tests/lsp_workspace.rs`:

```rust
//! Black-box workspace-feature integration tests: real binary, real stdio.
mod support;

use serde_json::json;
use support::Client;

#[test]
fn workspace_symbol_finds_indexed_declarations() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.compact"),
        "circuit transferValue(): Field { return 0; }",
    )
    .unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();

    let mut client = Client::start();
    client.initialize_root(&root, json!({}));
    let resp = client.request("workspace/symbol", json!({ "query": "transfer" }));
    let syms = resp["result"].as_array().expect("array result");
    assert!(
        syms.iter().any(|s| s["name"] == "transferValue"),
        "expected transferValue in {syms:?}"
    );
    client.shutdown();
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p compact-analyzer --test lsp_workspace`
Expected: FAIL — server returns `null`/`MethodNotFound` for `workspace/symbol`.

- [ ] **Step 3: Advertise the capability**

In `run`, add to the `ServerCapabilities { … }` initializer:

```rust
        workspace_symbol_provider: Some(lsp_types::OneOf::Left(true)),
```

- [ ] **Step 4: Add the handler + helper**

In `handle_request`, add a match arm:

```rust
            "workspace/symbol" => {
                let result = serde_json::from_value::<lsp_types::WorkspaceSymbolParams>(req.params)
                    .ok()
                    .map(|params| self.workspace_symbol_infos(&params.query));
                self.respond_ok(req.id, result.map(lsp_types::WorkspaceSymbolResponse::Flat));
            }
```

Add the helper method to `impl GlobalState`:

```rust
    #[allow(deprecated)] // lsp_types::SymbolInformation::deprecated
    fn workspace_symbol_infos(&mut self, query: &str) -> Vec<lsp_types::SymbolInformation> {
        let items = analyzer_ide::workspace_symbols(&mut self.host, query);
        let mut out = Vec::new();
        for it in items {
            let Ok(uri) = Url::from_file_path(self.host.vfs().path(it.file)) else {
                continue;
            };
            let Some(analysis) = self.host.analyze(it.file) else {
                continue;
            };
            out.push(lsp_types::SymbolInformation {
                name: it.name,
                kind: symbol_kind_to_lsp(it.kind),
                tags: None,
                deprecated: None,
                location: lsp_types::Location {
                    uri,
                    range: lsp_utils::range_to_lsp(&analysis.line_index, it.name_range),
                },
                container_name: it.container,
            });
        }
        out
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p compact-analyzer --test lsp_workspace`
Expected: PASS.

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p compact-analyzer --all-targets --locked -- -D warnings && cargo test -p compact-analyzer`
Expected: clean; new integration binary `lsp_workspace` (1 test).

```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests/support/mod.rs crates/compact-analyzer/tests/lsp_workspace.rs
git commit -m "feat(server): workspace/symbol capability and handler"
```

---

### Task 12: binary — `didChangeWatchedFiles` registration + handler

Register a `**/*.compact` watcher when the client supports it, and handle change events to keep the index current: create/change → invalidate disk + reindex; delete → evict.

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs`
- Modify: `crates/compact-analyzer/tests/lsp_workspace.rs` (add a watched-file test)

**Interfaces:**
- Consumes: `AnalysisHost::{index_file, remove_workspace_file, vfs_mut}`; `Vfs::invalidate_disk`; `lsp_utils::abs_path_from_uri`.
- Produces: `GlobalState::{register_file_watcher, send_request, did_change_watched_files, republish_open_diagnostics}`.

- [ ] **Step 1: Add the failing integration test**

Add to `crates/compact-analyzer/tests/lsp_workspace.rs`:

```rust
#[test]
fn watched_file_creation_updates_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();
    let mut client = Client::start();
    client.initialize_root(
        &root,
        json!({ "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }),
    );

    // Create a file after initialize, then notify the server of the change.
    let added = dir.path().join("added.compact");
    std::fs::write(&added, "circuit addedThing(): Field { return 0; }").unwrap();
    let added_uri = lsp_types::Url::from_file_path(&added).unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": added_uri, "type": 1 }] }), // 1 = CREATED
    );

    let resp = client.request("workspace/symbol", json!({ "query": "addedThing" }));
    let syms = resp["result"].as_array().unwrap();
    assert!(syms.iter().any(|s| s["name"] == "addedThing"));
    client.shutdown();
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p compact-analyzer --test lsp_workspace watched_file_creation_updates_the_index`
Expected: FAIL — the notification is ignored; the new symbol isn't indexed.

- [ ] **Step 3: Add the registration + request plumbing**

In the `lsp_types` import block of `server.rs`, add `DidChangeWatchedFilesParams`. Add these methods to `impl GlobalState`:

```rust
    /// Registers a `**/*.compact` file watcher via `client/registerCapability`.
    fn register_file_watcher(&mut self) {
        let registration = lsp_types::Registration {
            id: "watch-compact".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: serde_json::to_value(
                lsp_types::DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![lsp_types::FileSystemWatcher {
                        glob_pattern: lsp_types::GlobPattern::String("**/*.compact".to_string()),
                        kind: None,
                    }],
                },
            )
            .ok(),
        };
        self.send_request::<lsp_types::request::RegisterCapability>(lsp_types::RegistrationParams {
            registrations: vec![registration],
        });
    }

    /// Sends a server→client request. The client's response is ignored (the
    /// main loop drops `Message::Response`).
    fn send_request<R: lsp_types::request::Request>(&mut self, params: R::Params) {
        self.next_request_id += 1;
        let req = Request::new(
            lsp_server::RequestId::from(self.next_request_id),
            R::METHOD.to_string(),
            params,
        );
        let _ = self.sender.send(Message::Request(req));
    }

    fn did_change_watched_files(&mut self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            let Some(path) = lsp_utils::abs_path_from_uri(&change.uri) else {
                continue;
            };
            let file = self.host.vfs_mut().file_id(&path);
            if change.typ == lsp_types::FileChangeType::DELETED {
                self.host.remove_workspace_file(file);
            } else {
                self.host.vfs_mut().invalidate_disk(file);
                self.host.index_file(file);
            }
        }
        self.republish_open_diagnostics();
    }

    /// Re-schedules diagnostics for every open file (a cross-file change may
    /// have altered their resolution results).
    fn republish_open_diagnostics(&mut self) {
        let open: Vec<FileId> = self.open_files.values().copied().collect();
        if open.is_empty() {
            return;
        }
        for f in open {
            self.pending_diagnostics.insert(f);
        }
        self.debounce_deadline = Some(Instant::now() + DEBOUNCE);
    }
```

- [ ] **Step 4: Register at startup + route the notification**

In `run`, after the crawl block, add:

```rust
    if state.watch_supported {
        state.register_file_watcher();
    }
```

In `handle_notification`, add a match arm (before the catch-all `other =>`):

```rust
            "workspace/didChangeWatchedFiles" => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeWatchedFilesParams>(not.params)
                {
                    self.did_change_watched_files(params);
                }
            }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p compact-analyzer --test lsp_workspace`
Expected: PASS (both workspace tests).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p compact-analyzer --all-targets --locked -- -D warnings && cargo test -p compact-analyzer`
Expected: clean.

```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests/lsp_workspace.rs
git commit -m "feat(server): register and handle didChangeWatchedFiles"
```

---

### Task 13: binary — cross-file diagnostics (merge + re-publish open files)

Publish resolution diagnostics alongside parser diagnostics, and on any edit re-index the edited file and re-publish all open files (so a cross-file fix/break reflects immediately).

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs`
- Modify: `crates/compact-analyzer/tests/lsp_workspace.rs` (add a diagnostic test)

**Interfaces:**
- Consumes: `AnalysisHost::{resolution_diagnostics, index_file}` (Tasks 6/4b); `republish_open_diagnostics` (Task 12).

- [ ] **Step 1: Add the failing integration test**

Add to `crates/compact-analyzer/tests/lsp_workspace.rs` (add `use support::did_open;` to the imports at the top, i.e. `use support::{Client, did_open};`):

```rust
#[test]
fn unresolved_import_publishes_an_error_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();
    let mut client = Client::start();
    client.initialize_root(&root, json!({}));

    let main = dir.path().join("main.compact");
    let uri = lsp_types::Url::from_file_path(&main).unwrap();
    did_open(
        &mut client,
        &uri,
        1,
        "import Missing;\ncircuit m(): Field { return 0; }",
    );

    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("cannot resolve import")),
        "expected an unresolved-import diagnostic in {diags:?}"
    );
    client.shutdown();
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p compact-analyzer --test lsp_workspace unresolved_import_publishes_an_error_diagnostic`
Expected: FAIL — only parser diagnostics are published (none here), so no `cannot resolve import`.

- [ ] **Step 3: Merge resolution diagnostics into publishing**

In `publish_file_diagnostics`, replace the `let diagnostics = … .collect();` statement with:

```rust
        let mut diagnostics: Vec<lsp_types::Diagnostic> = analysis
            .diagnostics
            .iter()
            .map(|d| lsp_utils::diagnostic_to_lsp(d, &analysis.line_index, &uri))
            .collect();
        for d in self.host.resolution_diagnostics(file) {
            diagnostics.push(lsp_utils::diagnostic_to_lsp(&d, &analysis.line_index, &uri));
        }
```

- [ ] **Step 4: Re-index + re-publish on edits; drop the single-file scheduler**

In `did_open`, replace the final `self.schedule_diagnostics(file);` with:

```rust
        self.host.index_file(file);
        self.republish_open_diagnostics();
```

In `did_change`, replace the final `self.schedule_diagnostics(file);` with:

```rust
        self.host.index_file(file);
        self.republish_open_diagnostics();
```

Delete the now-unused `fn schedule_diagnostics` method (both call sites are gone; clippy `-D warnings` would otherwise flag it as dead code).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p compact-analyzer --test lsp_workspace && cargo test -p compact-analyzer --test lsp_integration`
Expected: PASS — the new diagnostic test, and the existing diagnostics integration tests (which now also route through `republish_open_diagnostics`).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -p compact-analyzer --all-targets --locked -- -D warnings && cargo test -p compact-analyzer`
Expected: clean.

```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests/lsp_workspace.rs
git commit -m "feat(server): publish resolution diagnostics; re-publish open files on edit"
```

---

### Task 14: binary — cooperative cancellation for references/rename

Switch the references and rename handlers to the cancellable IDE variants, backed by a predicate that reports whether the LSP receive queue is empty. When newer input has arrived mid-scan, the operation abandons (answers null / `NotFound`) and the queued messages are processed normally; the client re-requests. Single-threaded and safe: no message is drained or lost.

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs`
- Modify: `crates/compact-analyzer/tests/lsp_workspace.rs` (add a cross-file references-over-LSP test)

**Interfaces:**
- Consumes: `analyzer_ide::{find_references_cancellable, rename_cancellable}` (Tasks 7/8); `GlobalState::cancel_receiver` (Task 10).

- [ ] **Step 1: Add the failing integration test**

Add to `crates/compact-analyzer/tests/lsp_workspace.rs` (add `use support::lsp_position;` to the imports):

```rust
#[test]
fn references_are_workspace_wide_over_lsp() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Foo.compact"),
        "module Foo { export circuit ff(): Field { return 0; } }",
    )
    .unwrap();
    let main = dir.path().join("main.compact");
    let main_text = "import Foo;\ncircuit m(): Field { return ff() + ff(); }";
    std::fs::write(&main, main_text).unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();

    let mut client = Client::start();
    client.initialize_root(&root, json!({}));
    let uri = lsp_types::Url::from_file_path(&main).unwrap();
    did_open(&mut client, &uri, 1, main_text);

    let (line, col) = lsp_position(main_text, "ff");
    let resp = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col },
            "context": { "includeDeclaration": true }
        }),
    );
    let locs = resp["result"].as_array().expect("array result");
    assert_eq!(locs.len(), 3); // declaration in Foo + two uses in main
    client.shutdown();
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p compact-analyzer --test lsp_workspace references_are_workspace_wide_over_lsp`
Expected: FAIL — the non-cancellable handler still finds only the two same-file uses (2, not 3).

> Note: this fails on *count* only because the handler still calls the single-file-era path indirectly — after Step 3 it routes through the workspace-wide cancellable function.

- [ ] **Step 3: Route references through the cancellable variant**

In `handle_request`, replace the `"textDocument/references"` arm with:

```rust
            "textDocument/references" => {
                let recv = self.cancel_receiver.clone();
                let should_continue = || recv.is_empty();
                let result = serde_json::from_value::<lsp_types::ReferenceParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let include_decl = params.context.include_declaration;
                        let pos = self.position_to_file_position(
                            &params.text_document_position.text_document.uri,
                            params.text_document_position.position,
                        )?;
                        analyzer_ide::find_references_cancellable(
                            &mut self.host,
                            pos,
                            include_decl,
                            &should_continue,
                        )
                    })
                    .map(|targets| {
                        targets
                            .iter()
                            .filter_map(|t| lsp_utils::nav_target_to_location(&mut self.host, t))
                            .collect::<Vec<_>>()
                    });
                self.respond_ok(req.id, result);
            }
```

- [ ] **Step 4: Route rename through the cancellable variant**

In the `"textDocument/rename"` arm, replace the call `analyzer_ide::rename(&mut self.host, pos, &params.new_name)` with:

```rust
                let recv = self.cancel_receiver.clone();
                let should_continue = || recv.is_empty();
                match analyzer_ide::rename_cancellable(
                    &mut self.host,
                    pos,
                    &params.new_name,
                    &should_continue,
                ) {
```

(Leave the rest of the rename arm — the `Ok(edits) => { … }` / `Err(...)` handling — unchanged.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p compact-analyzer --test lsp_workspace && cargo test -p compact-analyzer --test lsp_navigation`
Expected: PASS — the cross-file references test, plus the existing single-file navigation tests (queue empty during a lone request → `should_continue` is true → full scan).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace`
Expected: clean; full workspace green.

```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests/lsp_workspace.rs
git commit -m "feat(server): cooperative cancellation for references and rename"
```

---

### Task 15: corpus smoke test

Run compactp's corpus through workspace indexing and the span-producing IDE features, asserting no panics and no out-of-bounds spans. Skips cleanly when the corpus is unavailable (CI has no compactp checkout).

**Files:**
- Create: `crates/compact-analyzer/tests/corpus_smoke.rs`

**Interfaces:**
- Consumes: `AnalysisHost::{discover_and_index, workspace_files, analyze, vfs, vfs_mut}`; `analyzer_ide::{document_symbols, goto_definition, hover, workspace_symbols}`; `DocSymbol`.

- [ ] **Step 1: Write the test**

Create `crates/compact-analyzer/tests/corpus_smoke.rs`:

```rust
//! Corpus smoke: run compactp's ~486-file corpus through workspace indexing
//! and the span-producing IDE features, asserting no panic and no
//! out-of-bounds spans. Skips cleanly when the corpus is unavailable — set
//! COMPACT_CORPUS_DIR, or check out ../compactp next to this repo.

use std::path::{Path, PathBuf};

use analyzer_core::{AnalysisHost, FileId, FilePosition, SyntaxKind, SyntaxNode, TextRange, TextSize};

fn corpus_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("COMPACT_CORPUS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../compactp/tests/corpus");
    p.is_dir().then_some(p)
}

fn assert_in_bounds(host: &mut AnalysisHost, file: FileId, range: TextRange, what: &str) {
    let len = host.vfs_mut().read(file).map(|t| t.len() as u32).unwrap_or(0);
    assert!(
        u32::from(range.end()) <= len,
        "{what}: span {range:?} exceeds file length {len} in {:?}",
        host.vfs().path(file)
    );
}

fn check_doc_symbol(host: &mut AnalysisHost, file: FileId, sym: &analyzer_ide::DocSymbol) {
    assert_in_bounds(host, file, sym.full_range, "doc symbol full_range");
    assert_in_bounds(host, file, sym.selection_range, "doc symbol selection_range");
    for child in &sym.children {
        check_doc_symbol(host, file, child);
    }
}

#[test]
fn corpus_smoke_no_panics_no_oob_spans() {
    let Some(dir) = corpus_dir() else {
        eprintln!("corpus smoke SKIPPED: no COMPACT_CORPUS_DIR and no ../compactp checkout");
        return;
    };
    let mut host = AnalysisHost::new();
    host.discover_and_index(&[dir], &|| true);
    let files = host.workspace_files();
    assert!(files.len() > 100, "expected a large corpus, got {}", files.len());

    for file in files {
        let Some(analysis) = host.analyze(file) else {
            continue;
        };
        for sym in analyzer_ide::document_symbols(&mut host, file) {
            check_doc_symbol(&mut host, file, &sym);
        }
        let root = SyntaxNode::new_root(analysis.green.clone());
        let offsets: Vec<TextSize> = root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| t.text_range().start())
            .collect();
        for off in offsets {
            let pos = FilePosition { file, offset: off };
            if let Some(nav) = analyzer_ide::goto_definition(&mut host, pos) {
                assert_in_bounds(&mut host, nav.file, nav.name_range, "goto");
            }
            let _ = analyzer_ide::hover(&mut host, pos);
        }
    }

    for s in analyzer_ide::workspace_symbols(&mut host, "") {
        assert_in_bounds(&mut host, s.file, s.name_range, "workspace symbol");
    }
}
```

- [ ] **Step 2: Run it (locally, with the corpus present)**

Run: `cargo test -p compact-analyzer --test corpus_smoke -- --nocapture`
Expected: PASS (indexes ~489 files; no panic, no OOB). If `../compactp` is absent, it prints `SKIPPED` and passes.

> NOTE for the implementer: this test is corpus-gated, so it runs green locally and skips in CI. If any file triggers a panic or an out-of-bounds span, that is a real bug in an earlier task — fix it there (systematic-debugging), don't weaken the assertion.

- [ ] **Step 3: Gate + commit**

Run: `cargo fmt && cargo clippy -p compact-analyzer --all-targets --locked -- -D warnings && cargo test -p compact-analyzer`
Expected: clean (corpus test passes locally / skips in CI).

```bash
git add crates/compact-analyzer/tests/corpus_smoke.rs
git commit -m "test(server): corpus smoke over indexing and IDE features"
```

---

### Task 16: analyzer-core — close resolver-edge test gaps

Lock in three previously-untested M2a resolver edges with corpus-proven fixtures: typed lambda params, and struct-pattern (`{a, b}`) bindings resolved from both the binding site and a use site.

**Files:**
- Modify: `crates/analyzer-core/src/resolve.rs` (tests only)

**Interfaces:**
- Consumes: `AnalysisHost::resolve`, `Definition::Local`; the `fixture::extract` + single-file host pattern.

- [ ] **Step 1: Write the tests**

Add to `resolve.rs`'s `#[cfg(test)] mod tests`. (Use the existing single-file overlay pattern — a small local helper if one is not already present:)

```rust
    fn local_host(marked: &str) -> (AnalysisHost, FilePosition) {
        let (clean, offset) = crate::fixture::extract(marked);
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        (host, FilePosition { file, offset })
    }

    #[test]
    fn typed_lambda_param_resolves() {
        // Corpus-proven form: `map((x: Field) => <body>, xs)`. Cursor on the
        // body use of the typed param.
        let (mut host, pos) =
            local_host("circuit c(): Field { return map((x: Field) => x$0, xs); }");
        match host.resolve(pos) {
            Some(Definition::Local { name, .. }) => assert_eq!(name, "x"),
            other => panic!("expected Local `x`, got {other:?}"),
        }
    }

    #[test]
    fn struct_pattern_binding_resolves_from_binding_site() {
        // Corpus-proven form: `const { a, b } = <expr>;`. Cursor on the `a`
        // binding in the pattern.
        let (mut host, pos) =
            local_host("circuit c(pt: Field): Field { const { a$0, b } = pt; return a + b; }");
        match host.resolve(pos) {
            Some(Definition::Local { name, .. }) => assert_eq!(name, "a"),
            other => panic!("expected Local `a`, got {other:?}"),
        }
    }

    #[test]
    fn struct_pattern_binding_resolves_from_use_site() {
        let (mut host, pos) =
            local_host("circuit c(pt: Field): Field { const { a, b } = pt; return a$0 + b; }");
        match host.resolve(pos) {
            Some(Definition::Local { name, .. }) => assert_eq!(name, "a"),
            other => panic!("expected Local `a`, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test -p analyzer-core resolve::tests::typed_lambda_param_resolves resolve::tests::struct_pattern_binding_resolves_from_binding_site resolve::tests::struct_pattern_binding_resolves_from_use_site`
Expected: PASS (these lock in existing M2a behavior).

> NOTE for the implementer: fixtures are corpus-proven to *parse*. If any assertion fails, that is a genuine resolver gap (not a fixture typo) — confirm the actual `Definition` compactp produces (`resolve_local_name` token-pick vs `ident_at_offset`), fix the resolver so the property holds, and record it. Do not delete the test.

- [ ] **Step 3: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count +3.

```bash
git add crates/analyzer-core/src/resolve.rs
git commit -m "test(core): cover typed-lambda-param and struct-pattern resolution"
```

---

### Task 17: analyzer-core — LineIndex + VFS edge-case tests

Close the remaining `LineIndex::offset` gaps (surrogate-mid-column, empty text, last line without a newline) and document the `Vfs::path` foreign-`FileId` precondition with a `should_panic` test.

**Files:**
- Modify: `crates/analyzer-core/src/line_index.rs` (tests only)
- Modify: `crates/analyzer-core/src/vfs.rs` (tests only)

- [ ] **Step 1: Write the LineIndex tests**

Add to `line_index.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn offset_col_inside_surrogate_pair_advances_to_next_char() {
        let li = index("\u{1F600}x"); // emoji (2 UTF-16 units), then x
        // Column 1 lands inside the emoji's surrogate pair → the scan clamps up
        // to the next char boundary (the `x` at byte 4).
        assert_eq!(li.offset(LineCol { line: 0, col: 1 }), Some(TextSize::new(4)));
    }

    #[test]
    fn offset_and_line_col_handle_empty_text() {
        let li = index("");
        assert_eq!(li.line_col(TextSize::new(0)), LineCol { line: 0, col: 0 });
        assert_eq!(li.offset(LineCol { line: 0, col: 0 }), Some(TextSize::new(0)));
        assert_eq!(li.offset(LineCol { line: 0, col: 5 }), Some(TextSize::new(0)));
        assert_eq!(li.offset(LineCol { line: 1, col: 0 }), None);
    }

    #[test]
    fn offset_on_last_line_without_newline_clamps_to_content_end() {
        let li = index("ab\ncde");
        assert_eq!(li.offset(LineCol { line: 1, col: 99 }), Some(TextSize::new(6)));
    }
```

- [ ] **Step 2: Write the VFS precondition test**

Add to `vfs.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    #[should_panic]
    fn path_panics_on_foreign_file_id() {
        // `path` indexes by FileId; an id minted by a different Vfs is out of
        // range and panics (documented precondition — callers pass ids from
        // the same Vfs).
        let mut a = Vfs::new();
        let _ = a.file_id(std::path::Path::new("/t/only.compact")); // a has id 0
        let mut b = Vfs::new();
        let _ = b.file_id(std::path::Path::new("/t/x.compact"));
        let foreign = b.file_id(std::path::Path::new("/t/y.compact")); // id 1 in b
        let _ = a.path(foreign); // id 1 is out of range for `a`
    }
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p analyzer-core line_index && cargo test -p analyzer-core vfs`
Expected: PASS (3 LineIndex + 1 VFS).

- [ ] **Step 4: Gate + commit**

Run: `cargo fmt && cargo clippy -p analyzer-core --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: clean; core test count +4.

```bash
git add crates/analyzer-core/src/line_index.rs crates/analyzer-core/src/vfs.rs
git commit -m "test(core): LineIndex offset edge cases and Vfs foreign-id precondition"
```

---

### Task 18: binary — per-file diagnostic debounce

Replace the single global debounce deadline with a per-file deadline map, so each file's diagnostics defer on its own trailing edge under continuous edits.

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs`
- Modify: `crates/compact-analyzer/tests/lsp_workspace.rs` (add a two-file diagnostics test)

**Interfaces:**
- Changes: `GlobalState.pending_diagnostics` becomes `HashMap<FileId, Instant>`; `debounce_deadline` is removed.

- [ ] **Step 1: Add the failing/guarding integration test**

Add to `crates/compact-analyzer/tests/lsp_workspace.rs`:

```rust
#[test]
fn two_open_files_each_receive_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();
    let mut client = Client::start();
    client.initialize_root(&root, json!({}));

    let ua = lsp_types::Url::from_file_path(dir.path().join("a.compact")).unwrap();
    let ub = lsp_types::Url::from_file_path(dir.path().join("b.compact")).unwrap();
    did_open(&mut client, &ua, 1, "ledger x Field;"); // missing colon → parse error
    did_open(&mut client, &ub, 1, "ledger y Field;");

    let mut seen = std::collections::HashSet::new();
    while seen.len() < 2 {
        let params = client.wait_for_notification("textDocument/publishDiagnostics");
        let uri = params["uri"].as_str().unwrap().to_string();
        let has_diags = params["diagnostics"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
        if has_diags {
            seen.insert(uri);
        }
    }
    client.shutdown();
}
```

- [ ] **Step 2: Run it (passes today via the global debounce; it guards the refactor)**

Run: `cargo test -p compact-analyzer --test lsp_workspace two_open_files_each_receive_diagnostics`
Expected: PASS (both files publish diagnostics).

- [ ] **Step 3: Convert to a per-file deadline map**

In `server.rs`: change `use std::collections::{HashMap, HashSet};` to `use std::collections::HashMap;` (HashSet becomes unused). In `struct GlobalState`, replace the two fields:

```rust
    /// Files with not-yet-published diagnostics, each with its own deadline.
    pending_diagnostics: HashMap<FileId, Instant>,
```

(Delete the `debounce_deadline: Option<Instant>` field.) In `new()`, replace their initializers with `pending_diagnostics: HashMap::new(),` (remove the `debounce_deadline` init).

Rewrite `republish_open_diagnostics`:

```rust
    fn republish_open_diagnostics(&mut self) {
        let deadline = Instant::now() + DEBOUNCE;
        let open: Vec<FileId> = self.open_files.values().copied().collect();
        for f in open {
            self.pending_diagnostics.insert(f, deadline);
        }
    }
```

Rewrite `flush_pending_diagnostics_inner` to flush only files whose deadline has passed:

```rust
    fn flush_pending_diagnostics_inner(&mut self) {
        let now = Instant::now();
        let due: Vec<FileId> = self
            .pending_diagnostics
            .iter()
            .filter(|&(_, &deadline)| deadline <= now)
            .map(|(&f, _)| f)
            .collect();
        for file in due {
            self.pending_diagnostics.remove(&file);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.publish_file_diagnostics(file)
            }));
            if let Err(panic) = result {
                eprintln!(
                    "compact-analyzer: panic while publishing diagnostics for one file: {}",
                    panic_message(panic.as_ref())
                );
            }
        }
    }
```

In `did_close`, the block that clears the deadline becomes just the map removal (already present as `self.pending_diagnostics.remove(&file);`); delete the now-obsolete `if self.pending_diagnostics.is_empty() { self.debounce_deadline = None; }` lines.

- [ ] **Step 4: Update the main loop to use the earliest pending deadline**

In `run`, replace the head of the `loop { … }` body. Change:

```rust
        let msg = if let Some(deadline) = state.debounce_deadline {
```

to:

```rust
        let next_deadline = state.pending_diagnostics.values().min().copied();
        let msg = if let Some(deadline) = next_deadline {
```

(The rest of the `recv_timeout`/`flush_pending_diagnostics`/`continue` body is unchanged — it now keys off `next_deadline`.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p compact-analyzer`
Expected: PASS — the two-file test, plus the existing diagnostics integration tests (`lsp_integration`) confirming no debounce regression.

- [ ] **Step 6: Final workspace gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace`
Expected: clean; full suite green.

```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests/lsp_workspace.rs
git commit -m "refactor(server): per-file diagnostic debounce"
```

---

## Status: M2b — Cross-file Resolution + Workspace Index

Tasks (20 sections; Task 4 is split into 4a/4b/4c): **1** source-path resolution · **2** fill import arms · **3** include resolution · **4a** VFS generation + invalidate_disk · **4b** workspace index · **4c** gitignore crawl · **5** cache polish · **6** import diagnostics · **7** workspace find-references · **8** workspace rename + per-use-site conflicts · **9** workspace symbols · **10** initialize params + crawl · **11** workspace/symbol handler · **12** didChangeWatchedFiles · **13** cross-file diagnostics publish · **14** cooperative cancellation · **15** corpus smoke · **16** resolver-edge tests · **17** LineIndex/VFS edge tests · **18** per-file debounce.

**Expected final tally:** analyzer-core ~82, analyzer-ide ~19, compact-analyzer ~9 unit + 5 `lsp_integration` + 5 `lsp_navigation` + 5 `lsp_workspace` + 1 `corpus_smoke` (corpus-gated). ~**126 tests**, up from 83. `cargo fmt` clean; `cargo clippy --workspace --all-targets --locked -- -D warnings` clean.

## Self-review notes (completed during planning)

- **Empirical verification (2026-07-07):** the compiler's file-resolution semantics were confirmed with real `compactc --trace-search` runs — `.compact` is appended uniformly (imports/string-imports/includes), search order is importing-file-dir-then-compact-path, identifier imports require exactly one matching `module`, string imports derive the module name from the last path component, and each file resolves relative to its own directory. The exact error strings were captured. This corrected the spec's earlier include-extension assumption (recorded in the spec's §8). compactp AST accessors, the `ignore` crate (v0.4.27), and the lsp-types `0.95.1` types were all confirmed. Lambda/struct-pattern fixtures are corpus-proven.
- **Deliberate simplifications (recorded):** find-references pruning is by workspace-membership + name-presence over all workspace files — a safe *superset* of the reverse-dependency-closure the spec's §4.2 describes. This is intentional: with `include` the true candidate set is the *transitive* reverse-dep closure (B includes C includes D still sees D), and scanning all files + name-presence avoids that transitivity trap entirely while staying correct. The forward/reverse dependency graph is still built and exposed (`dependents_of`) — it satisfies the "reverse-dependency tracking" requirement and is available for precise targeting, but the binary uses the simpler correct-superset "re-publish all open files" for cross-file diagnostics. Cross-file goto on the *original name inside a selective import specifier* stays `None` (Task 2 NOTE). The dependency graph over-approximates identifier-import edges only when an in-scope module shadows a same-named file (harmless extra reverse-dep).
- **Generation counter — realizability note (design refinement found during planning):** the approved design (§6) framed cancellation as "poll whether the operation's start-generation is still current," implying the callback applies pending edits (bumping the generation) mid-operation. That is not safely realizable single-threaded: the cancellation callback cannot read or mutate the host while the operation already holds `&mut host` (borrow-checker + the impossibility of mutating state the operation is reading). So cancellation is realized as **abort-if-the-receive-queue-is-non-empty** (Task 14): a message-loss-free proxy for "newer input arrived" — the operation answers null and the queued messages (including edits) are processed normally by the main loop, and the client re-requests. The generation counter (Task 4a) therefore stands as the **workspace revision** — bumped on every VFS mutation, exposed for staleness reasoning and a future salsa-style engine — rather than as the literal cancellation poll. This is a forced constraint, not a preference; flagged for the reviewer.
- **Layering held:** every `analyzer-core`/`analyzer-ide` addition speaks `FileId`/`TextSize`/`TextRange`; `lsp-types` appears only in the binary; UTF-16 conversion stays at the boundary.

## Errata (bugs in this plan's own text, found + fixed during execution)

- **E1 (Task 1 test count off-by-one; cascades to every downstream core count).** Task 1's test module contains **7** `#[test]` functions (`appends_compact_…`, `falls_back_…`, `resolves_subdir_…`, `absolute_spec_…`, `returns_none_…`, `string_lit_text_…`, `module_name_…`), not 6. The verbatim code therefore takes analyzer-core from **51 → 58** (+7), not 51 → 57. Steps 5/6 of Task 1 (“PASS (6 tests)”, “51 → 57”) undercount by one. Consequence: every later task's stated “expected core count” is **+1** higher than written (Task 2 → 62 not 61, Task 3 → 65 not 64, 4a → 67, 4b → 69, 4c → 71, Task 5 → 72, Task 6 → 76, Task 16 → 79, Task 17 → 83). Final analyzer-core tally is **83**, not the plan's 82; overall ≈ **127**, not 126. No code change — the verbatim tests are correct; only the plan's expected-count annotations are wrong.
- **E2 (Task 4c first build must NOT use `--locked`).** Task 4c Step 1 says “Run: `cargo build -p analyzer-core --locked` (resolves `ignore` v0.4.27 into `Cargo.lock`)”. This is backwards: `--locked` *forbids* modifying `Cargo.lock` and errors when a dependency was just added but is not yet in the lock (verified empirically: `ignore` is absent from `Cargo.lock` at this point). The first build after adding `ignore` to the manifests must be `cargo build -p analyzer-core` **without** `--locked` (or `cargo update -p ignore`) so the lock gains `ignore` and its transitive deps; only *after* that does the `--locked` clippy/test gate pass. No code change — the manifest edits and the rest of the task are unaffected.
- **E3 (Task 10 `--lib` runs no tests — `compact-analyzer` is bin-only).** Task 10 Steps 2/5 say `cargo test -p compact-analyzer --lib`, but the crate declares only a `[[bin]]` target (no `[lib]`), so `--lib` fails with "no library targets found" and runs NONE of the `server.rs` unit tests. Use `cargo test -p compact-analyzer` (which runs the bin's own `#[cfg(test)]` unit tests plus the `lsp_integration`/`lsp_navigation`/`lsp_workspace` integration binaries). No code change.
- **E4 (Task 10 forward-declared fields need `#[allow(dead_code)]`, not bare — and NOT `#[expect]`).** The plan's verbatim Task 10 `GlobalState` struct adds `cancel_receiver` (consumed by Task 14) and `next_request_id` (consumed by Task 12) with NO dead-code allowance — so the verbatim code FAILS `clippy -D warnings` at Task 10 (both fields are only written in `new()`, never read). Annotate BOTH with `#[allow(dead_code)]`. Do NOT use `#[expect(dead_code)]`: `#[expect]` fires `unfulfilled_lint_expectations` (warn-by-default → hard error under `-D warnings`) the moment Tasks 12/14 read the field, which would break THOSE tasks' gates with a confusing error pointing back at Task 10. `#[allow(dead_code)]` is inert whether or not the field is later read. (Reminder for Tasks 12/14 implementers: you do NOT need to touch these attributes — an `#[allow(dead_code)]` on a now-used field is harmless.)

