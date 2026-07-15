# v2b.5 — Mandatory Explicit Generic Specialization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the third distinctive type rule — Compact's requirement that a generic type is always specialized with the exact number of generic arguments its declaration takes — so the native checker agrees with `compactc` on generic **type-reference** arity (missing args, too many args, args on a non-generic type).

**Architecture:** This is the first type diagnostic that consults **name resolution**: to know a referenced type's declared generic arity, the checker resolves the type name to its definition (v2b.1's tracked def-map) and reads a generic-parameter count recorded on the definition's `Symbol`. A new resolution-fed tracked query `generic_diagnostics_query` walks every `TypeRef` in a file, resolves its name, and compares the use-site generic-argument count to the declared count. It is merged into `AnalysisHost::type_diagnostics` alongside the return/cast checks so the differential harness picks it up. The rule is pinned by live-`compactc` rule-tagged differential fixtures and stays inside the v2b.2 no-false-positive corpus gate.

**Tech Stack:** Rust 2024, salsa 0.28 (`#[salsa::interned]`, `#[salsa::tracked]`), `compactp_ast`/`compactp_syntax` (external CST), the v2b.1 tracked def-map/resolution (`resolve_query`, `FileDeps`, `Workspace`), the v2b.2 `analyzer-toolchain` differential harness.

## Global Constraints

Every task's requirements implicitly include this section.

- **Verify-first.** The compiler at the pinned toolchain (`compact 0.5.1` / compiler `0.31.1` / language `0.23.0`) is ground truth — never docs, never training data. The rule was extracted empirically from the live compiler (its accept/reject verdicts and its mismatch messages, which print the arity directly) **before** any implementation; the exact rule and every accept/reject verdict are recorded in the *Extracted rule* section below, and re-asserted by the rule-tagged fixtures (Task 5).
- **No LSP types in `analyzer-core`;** byte offsets (`text_size`) only. Diagnostics stay `compactp_diagnostics::Diagnostic`.
- **Single-threaded**, cooperative `should_continue`; no salsa cross-thread `Cancelled`.
- **Diagnostics are tracked queries returning `Arc<[Diagnostic]>`** (not accumulators); mirror the existing `resolution_diagnostics_query` (a resolution-fed diagnostics query) and `type_diagnostics_query`.
- **Never a false positive.** For any source `compactc` accepts, the native checker must emit zero type diagnostics. The arity check fires **only** when the type name resolves to a known named-type definition whose declared generic-parameter count the checker can read; every other case (unresolved name, resolves to a local/generic-param binding, resolves to a non-type kind, cross-file target not materialized) **suppresses** (no diagnostic). The v2b.2 binary corpus gate (`corpus_no_false_positives`) must stay green.
- **Pre-release.** No back-compat shims/migrations/dual code paths — clean wholesale replacement (`prerelease-no-back-compat`). Adding a field to `Symbol` is a def-map schema change: every construction site is updated in place.
- **Never commit `[patch.crates-io]`.**
- **Editor integration is out of scope here.** Surfacing type diagnostics in VS Code + the native-vs-compiler toggle remains **v2b.final**. This plan only broadens what `AnalysisHost::type_diagnostics` reports.
- **Rule scope.** In scope: **generic type-reference arity** — at a `TypeRef` (a named type in type position) whose name resolves to a struct / enum / type-alias definition, the number of generic arguments must equal the definition's declared generic-parameter count. **Out of scope (deliberate, recorded):** generic **call-site** checking (`foo<Field>(x)` — `compactc` reports the fuzzier "one function is incompatible with the supplied generic values", which folds in type-vs-nat *kind* matching; deferred, and the def-map count added here is its seed); generic-argument **kind** mismatches at equal count (a type where a `#n` nat is expected, or vice-versa — a conservative false *negative*); and any check requiring the resolved target's SourceText when it is not reachable from `(src, fd)`. These are conservative false negatives, never false positives.

## Extracted rule (ground truth — `compact 0.5.1` / compiler `0.31.1`, verified 2026-07-15)

Captured empirically from the live compiler; every verdict below is reproduced by a Task-5 fixture. The compiler's message prints the arity directly.

- **A generic type must be specialized with exactly its declared number of generic arguments.** A `TypeRef` naming a type declared with `N` generic parameters must carry exactly `N` generic arguments — no inference, no defaulting.
- **Type-reference arity diagnostic** (the rule this plan implements): `mismatch between actual number {A} and declared number {D} of generic parameters for {Name}`, where `A` = generic arguments supplied at the use site and `D` = generic parameters the declaration takes. Fires symmetrically:
  - Missing args: `struct Box<T>` used as `Box` → `actual number 0 and declared number 1 … for Box` (reject).
  - Too few: `struct Pair<A, B>` used as `Pair<Field>` → `actual number 1 and declared number 2 … for Pair` (reject).
  - Too many on a non-generic type: `struct Point { … }` used as `Point<Field>` → `actual number 1 and declared number 0 … for Point` (reject).
  - Correct: `Box<Field>` (1 = 1), `Point` (0 = 0), nested `Box<Box<Field>>` all accept.
- **Kinds of generic parameters.** A declaration's generic parameter list mixes **type** params (`T`) and **nat** params (`#n`), e.g. `struct MerkleTreePath<#n, T>`. For the arity (count) check both kinds count equally — `MerkleTreePath<n, T>` supplies 2 arguments for 2 parameters. (Kind *matching* — a type where a nat is expected — is out of scope here; see Rule scope.)
- **Enums are not generic** in Compact (`enum Color { Red, Green }` takes no parameters); their declared count is always 0, so `Color<Field>` correctly rejects via the same symmetric count check.
- **Call sites are different** (out of scope). A generic **circuit/function** call with wrong/omitted generic values (`id(y)`, `id<Field, Field>(y)`, `f<Field>(v)` where `#n` expected) rejects with a *different* message — `one function is incompatible with the supplied generic values` — which mixes count and type-vs-nat kind. Deferred to a later rule; recorded here for the next increment.

**Diagnostic code.** `E3003` (E3001 = return mismatch, E3002 = illegal cast, this rule = E3003). Wording tracks `compactc` "where practical"; **not** harness-gated.

## Drift reconciliation (checked against as-merged v2b.4, commit `741a037`)

Checked this plan against the merged tree (`crates/analyzer-core/src/{item_tree.rs,resolve.rs,db.rs,infer.rs}`). Findings folded in:

1. **`Symbol` does not record generic arity.** `Symbol` (`item_tree.rs`) carries `name`, `kind`, ranges, `signature`, `doc`, `exported`, `parent` — no generic info. **Task 1** adds `generic_param_count: u32`, populated during item lowering from each def's `generic_params()`. `Symbol`/`ItemTree` derive `PartialEq` (the v2b.1 backdating firewall); a `u32` field keeps the derive trivial and backdating intact.
2. **Resolution is already tracked and cross-file.** `resolve_query(db, file, src, fd, ws, offset) -> Option<Definition>` (`db.rs`) is a pure salsa query; the host bridge `AnalysisHost::resolve` provisions `fd`/`ws`. `Definition::Item { file, index }` names a symbol in some file's `item_tree`. A `TypeRef`'s name token resolves through this (tested: `resolves_type_reference_to_struct`). **Task 2** calls `resolve_query` per `TypeRef`.
3. **`resolution_diagnostics_query` is the exact template** for a resolution-fed diagnostics query: signature `(db, src, fd, from_dir_display, search_display) -> Arc<[Diagnostic]>`, walks the `SourceFile`, emits `Arc<[Diagnostic]>`; the host bridge (`resolution_diagnostics`) computes `fd`/`from_dir`/`search` and calls in. **Task 2**'s `generic_diagnostics_query` mirrors this shape but additionally takes `file`/`ws` (needed by `resolve_query`).
4. **Mapping `Definition::Item` → target `Symbol`.** `def_name`/`nav_info` (`resolve.rs`) do `self.src_of(file)` then `item_tree(src).symbols[index]` — a **host** lookup. In a pure query, the current file's `Symbol`s come from `item_tree(src)`; a cross-file target's `SourceText` is reachable via `fd.deps(db)` (each `ResolvedDep.target` is `(FileId, SourceText)`, exactly as `resolve_external_module_member` uses). **Task 2** adds a module-private `item_symbol` helper that resolves a `Definition::Item` to its `Symbol` from `(src, fd)`; a target neither in the current file nor in `fd` suppresses (conservative).
5. **`type_diagnostics` merge.** `AnalysisHost::type_diagnostics` (`infer.rs`) currently returns `type_diagnostics_query(db, src).to_vec()`. **Task 4** extends it to also run `generic_diagnostics_query` (provisioning `fd`/`ws`/display strings like `resolution_diagnostics` does) and concatenate. The differential harness calls `host.type_diagnostics(file)`, so generic diagnostics are cross-checked once merged.
6. **New diagnostic code `E3003`.** `DiagnosticCode::new("E", 3003)`; analyzer-owned numbering; wording tracked, not gated.

## File Structure

- `crates/analyzer-core/src/item_tree.rs` **(modify)** — add `Symbol.generic_param_count`; populate it at every `Symbol { … }` construction site (from `def.generic_params()` where the def can be generic, else `0`). Responsibility: the def-map.
- `crates/analyzer-core/src/db.rs` **(modify)** — add `generic_diagnostics_query` (resolution-fed `TypeRef` arity walk) + the `item_symbol` helper + the arity diagnostic constructor. Responsibility: tracked resolution + diagnostics.
- `crates/analyzer-core/src/resolve.rs` **(modify)** — add `AnalysisHost::generic_diagnostics` host bridge (mirrors `resolution_diagnostics`: provision `fd`/`ws`/display strings, call the query). Responsibility: host provisioning.
- `crates/analyzer-core/src/infer.rs` **(modify)** — `AnalysisHost::type_diagnostics` merges `generic_diagnostics`. Responsibility: the single type-diagnostics surface the harness consumes.
- `crates/compact-analyzer/tests/fixtures/type/*.compact` **(create)** — six generic-arity fixtures captured from `compactc`.
- `crates/compact-analyzer/tests/type_differential.rs` **(modify)** — add the six fixtures to `FIXTURES` (rule `"generic-specialization"`).

## Standard verification commands (used by every task)

- Build+test a crate: `cargo test -p <crate> -q`
- Whole workspace: `cargo test --workspace -q`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Format check: `cargo fmt --all --check`

---

### Task 1: Record generic-parameter count on `Symbol`

Extend the def-map so every definition carries its declared generic-parameter count. Populated during item lowering, where the def node (and its `generic_params()`) is already in hand. Additive to consumers (nothing reads it yet), but it is a schema change: **every** `Symbol { … }` construction site must set the new field.

**Files:**
- Modify: `crates/analyzer-core/src/item_tree.rs` (`Symbol` struct ~27-40; every `self.push(Symbol { … })` site: ~126, ~154, ~177, ~200, ~249; `mod tests`)

**Interfaces:**
- Consumes: `compactp_ast::{AstNode, GenericParamList}` and each def node's `generic_params()` accessor (circuits, functions/witnesses, structs, enums, type aliases, contracts — every node type that has one).
- Produces: `Symbol.generic_param_count: u32` — the number of generic parameters the declaration takes (`0` when it has none or cannot be generic).

- [ ] **Step 1: Write the failing test** — add to `item_tree.rs` `mod tests`:

```rust
    #[test]
    fn records_generic_param_count() {
        let t = tree_of(
            "export struct Box<T> { v: T; }\n\
             export struct Pair<A, B> { a: A; b: B; }\n\
             export struct Point { x: Field; y: Field; }\n\
             export enum Color { Red, Green }\n\
             export circuit id<T>(x: T): T { return x; }\n\
             export circuit plain(x: Field): Field { return x; }",
        );
        assert_eq!(find(&t, "Box").generic_param_count, 1);
        assert_eq!(find(&t, "Pair").generic_param_count, 2);
        assert_eq!(find(&t, "Point").generic_param_count, 0);
        assert_eq!(find(&t, "Color").generic_param_count, 0);
        assert_eq!(find(&t, "id").generic_param_count, 1);
        assert_eq!(find(&t, "plain").generic_param_count, 0);
    }
```

(Use the module's existing test harness for building an `ItemTree` from source and the existing `find(&t, name)` helper — both are already present in `item_tree.rs` `mod tests` (see the existing `find` at ~334 and its callers at ~348-353). If the helper that parses source to an `ItemTree` has a different name than `tree_of`, use that name.)

- [ ] **Step 2: Run it to verify it fails** — `cargo test -p analyzer-core item_tree::tests::records_generic -q`. Expected: FAIL to compile (`generic_param_count` field does not exist).

- [ ] **Step 3: Add the field** — in `item_tree.rs`, add to `Symbol` (after `parent`):

```rust
    /// Number of generic parameters the declaration takes (`0` when it has
    /// none or cannot be generic). Read by the generic-arity check to compare
    /// against a use site's supplied generic-argument count.
    pub generic_param_count: u32,
```

- [ ] **Step 4: Add a counting helper and set the field at every construction site** — add a module-private helper near the lowering code:

```rust
/// The number of generic parameters a definition node declares, or `0`.
/// `T` and `#n` parameters both count. Generic over any node exposing
/// `generic_params()` (circuits, functions/witnesses, structs, enums, type
/// aliases, contracts).
fn generic_count(params: Option<compactp_ast::GenericParamList>) -> u32 {
    params.map(|p| p.params().count() as u32).unwrap_or(0)
}
```

Then, at **each** `self.push(Symbol { … })` site, set `generic_param_count`. For a def node `n` that has a `generic_params()` accessor:

```rust
                    generic_param_count: generic_count(n.generic_params()),
```

For construction sites whose item **cannot** be generic (struct **fields**, enum **variants**, constructors, modules, contract circuits — anything without a `generic_params()` accessor), set:

```rust
                    generic_param_count: 0,
```

Match each site to the node variable it lowers (the plan cannot name every local; the compiler will flag any `Symbol { … }` missing the field, so add it to all of them — count-bearing sites use `generic_count(<node>.generic_params())`, the rest use `0`).

- [ ] **Step 5: Run + verify** — `cargo test -p analyzer-core item_tree:: -q`. Expected: PASS (new + existing). Then `cargo test --workspace -q` (confirm no other `Symbol { … }` literal elsewhere broke — if `Symbol` is constructed outside `item_tree.rs`, the compiler will have flagged it; add the field there too), `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/item_tree.rs
git commit -m "feat(v2b.5): record generic_param_count on Symbol

Each definition now carries its declared generic-parameter count (T and #n
both count; 0 when non-generic), computed during item lowering. Def-map
schema seed for the generic-arity check. Additive — no consumer yet.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: `generic_diagnostics_query` — resolve `TypeRef`s and check arity

Add the resolution-fed tracked query that walks every `TypeRef`, resolves its name to a definition, and emits the arity diagnostic when the supplied generic-argument count differs from the resolved definition's declared count. Mirrors `resolution_diagnostics_query`'s shape; uses `resolve_query` for name resolution.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (add `generic_diagnostics_query`, `item_symbol`, `generic_arity_diag`; near `resolution_diagnostics_query`)

**Interfaces:**
- Consumes: `crate::db::{Db, SourceText, FileDeps, Workspace, item_tree, parsed, resolve_query}`, `crate::{Definition, SymbolKind}`, `compactp_ast::{AstNode, SourceFile, TypeRef, Type, GenericArgList}`, `compactp_syntax::SyntaxNode`, `compactp_diagnostics::{Diagnostic, DiagnosticCode}`.
- Produces:
  - `#[salsa::tracked] pub fn generic_diagnostics_query(db: &dyn Db, file: crate::FileId, src: SourceText, fd: FileDeps, ws: Workspace) -> Arc<[Diagnostic]>` — one diagnostic per `TypeRef` whose supplied generic-arg count differs from its resolved named-type definition's declared count; suppresses on unresolved / non-type / target-unreachable.
  - `fn item_symbol(db, file, src, fd, def_file, index) -> Option<crate::Symbol>` (module-private) — resolves a `Definition::Item` to its `Symbol` from `(src, fd)`.
  - `fn generic_arity_diag(actual: u32, declared: u32, name: &str, span: text_size::TextRange) -> Diagnostic` (module-private).

- [ ] **Step 1: Write the failing test** — add to `db.rs` `mod tests` (or a new `#[cfg(test)] mod generic_tests`), using the existing `AnalysisHost` test pattern (overlay files, `file_id`). Because the query is resolution-fed, drive it through a host bridge added in this task — add a **temporary** test-only bridge if `AnalysisHost::generic_diagnostics` is not yet present (it is added in Task 3); simplest is to write the assertions against the host method and add the host method here. To keep Task boundaries clean, add the host bridge `AnalysisHost::generic_diagnostics` in **this** task's Step 4 (Task 3 only *merges* it into `type_diagnostics`). Test:

```rust
    #[test]
    fn generic_typeref_arity_is_checked() {
        use crate::AnalysisHost;
        use std::path::Path;

        fn diags(src: &str) -> Vec<compactp_diagnostics::Diagnostic> {
            let mut host = AnalysisHost::new();
            let file = host.vfs_mut().file_id(Path::new("/t/a.compact"));
            host.vfs_mut().set_overlay(file, src.to_string(), 1);
            host.generic_diagnostics(file)
        }

        // Correct arity: no diagnostic.
        assert!(
            diags("struct Box<T> { v: T; }\nexport circuit m(): Box<Field> { return default<Box<Field>>; }")
                .is_empty()
        );
        // Missing args (0 vs 1): one diagnostic naming Box.
        let d = diags("struct Box<T> { v: T; }\nexport circuit m(): Box { return default<Box>; }");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("actual number 0") && d[0].message.contains("declared number 1") && d[0].message.contains("Box"));
        // Wrong count (1 vs 2).
        assert_eq!(
            diags("struct Pair<A, B> { a: A; b: B; }\nexport circuit m(): Pair<Field> { return default<Pair<Field>>; }").len(),
            1
        );
        // Args on a non-generic struct (1 vs 0).
        assert_eq!(
            diags("struct Point { x: Field; }\nexport circuit m(): Point<Field> { return default<Point<Field>>; }").len(),
            1
        );
        // Unresolved name suppresses (never a false positive).
        assert!(diags("export circuit m(): Nope<Field> { return default<Nope<Field>>; }").is_empty());
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p analyzer-core generic_typeref_arity -q`. Expected: FAIL to compile (`generic_diagnostics` / query undefined).

- [ ] **Step 3: Implement the query + helpers** — add to `db.rs` (near `resolution_diagnostics_query`):

```rust
/// Resolve a `Definition::Item` to its `Symbol`. The current file's symbols
/// come from `item_tree(src)`; a cross-file target's `SourceText` is reached
/// via a materialized `fd` dep (as `resolve_external_module_member` does). A
/// target neither in the current file nor in `fd` yields `None` (suppresses).
fn item_symbol(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    def_file: crate::FileId,
    index: u32,
) -> Option<crate::Symbol> {
    if def_file == file {
        return item_tree(db, src).symbols.get(index as usize).cloned();
    }
    let (_, tsrc) = fd
        .deps(db)
        .iter()
        .filter_map(|d| d.target)
        .find(|(f, _)| *f == def_file)?;
    item_tree(db, tsrc).symbols.get(index as usize).cloned()
}

/// The generic-arity mismatch diagnostic (`E3003`). Wording tracks compactc's
/// ("mismatch between actual number A and declared number D of generic
/// parameters for Name"); wording is not gated by the harness.
fn generic_arity_diag(
    actual: u32,
    declared: u32,
    name: &str,
    span: text_size::TextRange,
) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 3003),
        format!(
            "mismatch between actual number {actual} and declared number {declared} of generic parameters for {name}"
        ),
        span,
    )
}

/// Generic-specialization diagnostics for `src`: every `TypeRef` whose name
/// resolves to a named-type definition (struct / enum / type alias) must carry
/// exactly the definition's declared number of generic arguments. Resolution
/// is required (arity lives on the *definition*), so this query is
/// resolution-fed like `resolution_diagnostics_query`. Suppresses whenever the
/// name does not resolve to a countable named-type definition — never a false
/// positive.
#[salsa::tracked(returns(clone), no_eq)]
pub fn generic_diagnostics_query(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
) -> Arc<[Diagnostic]> {
    let green = crate::db::parsed(db, src).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let mut diags = Vec::new();

    for tref in root.descendants().filter_map(compactp_ast::TypeRef::cast) {
        let Some(name_tok) = tref.name() else {
            continue;
        };
        // Resolve the type name to its definition.
        let def = resolve_query(db, file, src, fd, ws, name_tok.text_range().start());
        let Some(crate::Definition::Item {
            file: def_file,
            index,
        }) = def
        else {
            continue; // unresolved, or a local/generic-param binding -> suppress
        };
        let Some(sym) = item_symbol(db, file, src, fd, def_file, index) else {
            continue; // target unreachable -> suppress
        };
        // Only named *type* declarations carry a checkable generic arity.
        if !matches!(
            sym.kind,
            crate::SymbolKind::Struct | crate::SymbolKind::Enum | crate::SymbolKind::TypeAlias
        ) {
            continue;
        }
        let actual = tref
            .generic_args()
            .map(|a| a.args().count() as u32)
            .unwrap_or(0);
        if actual != sym.generic_param_count {
            diags.push(generic_arity_diag(
                actual,
                sym.generic_param_count,
                &name_tok.text().to_string(),
                tref.syntax().text_range(),
            ));
        }
    }
    Arc::from(diags)
}
```

Notes for the implementer to verify against the CST while wiring: `TypeRef::name()` returns the name token (`TypeRef` at `nodes.rs:829`); `TypeRef::generic_args() -> Option<GenericArgList>` and `GenericArgList::args()` iterate the supplied arguments (`nodes.rs:835`, `928-935`). If `TypeRef::name()` returns a `SyntaxToken`, `name_tok.text_range().start()` is its offset for `resolve_query`; adjust if the accessor differs. The `resolve_query` import path is `crate::db::resolve_query` (same module).

- [ ] **Step 4: Add the host bridge `AnalysisHost::generic_diagnostics`** — add to `resolve.rs` (next to `resolution_diagnostics`, reusing its `fd`/`ws` provisioning). It must provision the same inputs `resolve_query` needs. Mirror `AnalysisHost::resolve`'s provisioning for `fd`/`ws`:

```rust
    /// Generic-specialization diagnostics for `file` (type-reference arity).
    /// Resolution-fed (arity lives on the referenced definition), so this
    /// indexes the file to materialize its dependency edges, then calls the
    /// tracked `generic_diagnostics_query`. Merged into `type_diagnostics`
    /// (v2b.5); editor surfacing is v2b.final.
    pub fn generic_diagnostics(&mut self, file: crate::FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        self.ensure_indexed(file);
        let fd = self.file_deps(file);
        let ws = self.workspace_input();
        crate::db::generic_diagnostics_query(self.db_ref(), file, src, fd, ws).to_vec()
    }
```

The exact accessors for `fd` and `ws` are whatever `AnalysisHost::resolve` uses to call `resolve_query(self.db_ref(), file, src, fd, ws, offset)` — read `resolve()`'s body and reuse the identical provisioning (`file_deps`, and the workspace-input accessor; the name `workspace_input()` above is a placeholder — use the real accessor `resolve()` uses). `ensure_indexed` materializes the include/import edges before reading `file_deps`, exactly as `resolution_diagnostics` does.

- [ ] **Step 5: Run + verify** — `cargo test -p analyzer-core generic_typeref_arity -q` (expected: PASS), then `cargo test -p analyzer-core -q` (all resolution/def-map/type tests still green), `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/resolve.rs
git commit -m "feat(v2b.5): generic_diagnostics_query — TypeRef arity check

Resolution-fed tracked query: every TypeRef whose name resolves to a
struct/enum/type-alias must carry exactly the definition's declared number
of generic arguments, else E3003 (mismatch between actual number A and
declared number D of generic parameters for Name). Suppresses on any
unresolved / non-type / unreachable target — never a false positive. Adds
the AnalysisHost::generic_diagnostics bridge.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: Merge generic diagnostics into `AnalysisHost::type_diagnostics`

Make the single type-diagnostics surface the differential harness consumes (`type_diagnostics`) include the generic-arity diagnostics, so they are cross-checked against `compactc` and (later) surfaced in the editor.

**Files:**
- Modify: `crates/analyzer-core/src/infer.rs` (`AnalysisHost::type_diagnostics` ~252-264; add a merge test)

**Interfaces:**
- Consumes: `AnalysisHost::generic_diagnostics` (Task 2), the existing `type_diagnostics_query`.
- Produces: `AnalysisHost::type_diagnostics` now returns the return/cast diagnostics **and** the generic-arity diagnostics.

- [ ] **Step 1: Write the failing test** — add to `infer.rs` `mod tests`:

```rust
    #[test]
    fn type_diagnostics_includes_generic_arity() {
        use crate::AnalysisHost;
        use std::path::Path;
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(Path::new("/t/g.compact"));
        // Box<T> used with no args -> generic-arity diagnostic surfaces via
        // the merged type_diagnostics.
        host.vfs_mut().set_overlay(
            file,
            "struct Box<T> { v: T; }\nexport circuit m(): Box { return default<Box>; }".to_string(),
            1,
        );
        let diags = host.type_diagnostics(file);
        assert!(
            diags.iter().any(|d| d.code.number == 3003),
            "expected an E3003 generic-arity diagnostic, got: {:?}",
            diags.iter().map(|d| d.code.number).collect::<Vec<_>>()
        );
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p analyzer-core infer::tests::type_diagnostics_includes_generic -q`. Expected: FAIL (`type_diagnostics` returns only `type_diagnostics_query` output; no `E3003`).

- [ ] **Step 3: Merge** — update `AnalysisHost::type_diagnostics` in `infer.rs`:

```rust
    pub fn type_diagnostics(&mut self, file: crate::FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        let mut diags = type_diagnostics_query(self.db_ref(), src).to_vec();
        diags.extend(self.generic_diagnostics(file));
        diags
    }
```

(Order is not gated. `generic_diagnostics` re-derives `src`/`fd`/`ws`; that is cheap and keeps the two surfaces independent. Update the method's doc comment to note it now also reports generic-specialization diagnostics.)

- [ ] **Step 4: Run + verify** — `cargo test -p analyzer-core infer:: -q` (new merge test + all prior), then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.5): merge generic-arity diagnostics into type_diagnostics

AnalysisHost::type_diagnostics now concatenates generic_diagnostics with the
return/cast checks, so the one surface the differential harness (and, later,
the editor) consumes reports E3003 generic-specialization diagnostics too.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 4: Rule-tagged differential fixtures for generic specialization

Pin the rule against the live compiler: six fixtures whose expected native verdict was captured from `compactc` accept/reject, added to the existing `type_differential` harness (live cross-check). The `corpus_no_false_positives` gate is unchanged and must stay green.

**Files:**
- Create: `crates/compact-analyzer/tests/fixtures/type/generic_type_ok.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/generic_type_missing_args.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/generic_type_wrong_count.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/generic_args_on_nongeneric.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/nongeneric_type_ok.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/generic_type_nested_ok.compact`
- Modify: `crates/compact-analyzer/tests/type_differential.rs` (`FIXTURES` table)

**Interfaces:**
- Consumes: the existing `rule_tagged_fixtures` runner, `Fixture { name, native_rejects, rule }`.
- Produces: six new `FIXTURES` rows tagged `"generic-specialization"`.

- [ ] **Step 1: Create the fixtures** (verdicts captured from `compactc 0.31.1` — see *Extracted rule*; all six were compiled live during planning).

`generic_type_ok.compact` — `compactc` **accepts** (1 = 1):
```
struct Box<T> { v: T; }
export circuit f(): Box<Field> {
  return default<Box<Field>>;
}
```

`generic_type_missing_args.compact` — `compactc` **rejects** (`actual 0`, `declared 1`):
```
struct Box<T> { v: T; }
export circuit f(): Box {
  return default<Box>;
}
```

`generic_type_wrong_count.compact` — `compactc` **rejects** (`actual 1`, `declared 2`):
```
struct Pair<A, B> { a: A; b: B; }
export circuit f(): Pair<Field> {
  return default<Pair<Field>>;
}
```

`generic_args_on_nongeneric.compact` — `compactc` **rejects** (`actual 1`, `declared 0`):
```
struct Point { x: Field; y: Field; }
export circuit f(): Point<Field> {
  return default<Point<Field>>;
}
```

`nongeneric_type_ok.compact` — `compactc` **accepts** (0 = 0):
```
struct Point { x: Field; y: Field; }
export circuit f(): Point {
  return default<Point>;
}
```

`generic_type_nested_ok.compact` — `compactc` **accepts** (nested, all arities correct):
```
struct Box<T> { v: T; }
export circuit f(): Box<Box<Field>> {
  return default<Box<Box<Field>>>;
}
```

- [ ] **Step 2: Add the fixtures to the table** — in `type_differential.rs`, extend `FIXTURES` (keep every existing row):

```rust
    Fixture { name: "generic_type_ok.compact", native_rejects: false, rule: "generic-specialization" },
    Fixture { name: "generic_type_missing_args.compact", native_rejects: true, rule: "generic-specialization" },
    Fixture { name: "generic_type_wrong_count.compact", native_rejects: true, rule: "generic-specialization" },
    Fixture { name: "generic_args_on_nongeneric.compact", native_rejects: true, rule: "generic-specialization" },
    Fixture { name: "nongeneric_type_ok.compact", native_rejects: false, rule: "generic-specialization" },
    Fixture { name: "generic_type_nested_ok.compact", native_rejects: false, rule: "generic-specialization" },
```

- [ ] **Step 3: Run the fixture runner** — `cargo test -p compact-analyzer --test type_differential rule_tagged_fixtures -q`. Expected: PASS. The toolchain is present in this sandbox, so the live cross-check runs for every fixture and each native verdict must match the `compactc` verdict (`RejectPostParse` for `native_rejects: true`, `Accept` for `false`).

- [ ] **Step 4: Run the corpus gate + full suite** — `cargo test -p compact-analyzer --test type_differential -q` (confirm the `corpus_no_false_positives` SKIP line + pass). Then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`. To exercise the gate against a real corpus: `COMPACT_CORPUS_DIR=/path/to/compactp/tests/corpus cargo test -p compact-analyzer --test type_differential corpus_no_false_positives -q -- --nocapture` (must report `false_positives=0`).

- [ ] **Step 5: Commit**

```bash
git add crates/compact-analyzer/tests/type_differential.rs crates/compact-analyzer/tests/fixtures/type
git commit -m "test(v2b.5): rule-tagged generic-specialization differential fixtures

Six fixtures pinning generic type-reference arity against compactc: correct
arity accept, missing args reject, wrong count reject, args-on-nongeneric
reject, non-generic accept, nested-generic accept. Live compactc cross-check
runs when the toolchain is present; corpus gate stays green.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage** (rules-index v2b.5 row "Mandatory explicit generic specialization" + foundation spec §3/§4 + program spec §5/§9):
- Generic type modeled with a declared arity in the def-map → Task 1 (`Symbol.generic_param_count`). ✓
- Explicit-specialization enforcement (no inference of generic args) applied at use sites → Task 2 (`generic_diagnostics_query` over `TypeRef`s), Task 3 (merged into `type_diagnostics`). ✓
- Uses the v2b.1 tracked def-map + resolution ("def-map generic params") → Task 2 (`resolve_query`, `item_symbol` over `(src, fd)`). ✓
- Ground truth = compiler at the pinned toolchain; rule pinned by a `compactc` differential **before** implementation → *Extracted rule* (captured live first), Task 4 (fixtures + live cross-check). ✓
- Binary corpus gate stays green; no false positives → Global Constraints + suppress-on-any-uncertainty in Task 2 + Task 4 Step 4. ✓
- No LSP types; diagnostics are tracked `Arc<[Diagnostic]>`; single-threaded; pre-release → Global Constraints; the query mirrors `resolution_diagnostics_query`. ✓
- Editor surfacing deferred to v2b.final → stated; no `server.rs` change. ✓
- Call-site generic checking + equal-count kind mismatches explicitly out of scope, with false-negative-only rationale and the def-map count recorded as the call-site seed → Global Constraints (Rule scope) + *Extracted rule* (call-site note). ✓

**2. Placeholder scan:** No "TBD"/"handle edge cases"/"similar to Task N". Every code step carries complete code; every test step shows assertions; every run step gives the command + expected outcome. Two steps direct the implementer to read an existing site and reuse its exact accessor rather than hardcode a name that might drift — Task 1 Step 4 (every `Symbol { … }` site; the compiler enforces completeness) and Task 2 Step 4 (`fd`/`ws` provisioning: reuse whatever `AnalysisHost::resolve` uses, since `resolve_query` is the same call). These are verification-against-reality instructions, not placeholders — the surrounding code is fully specified and the tests pin the behavior.

**3. Type consistency:** `Symbol.generic_param_count: u32` (Task 1) is read in `generic_diagnostics_query` and compared to `GenericArgList::args().count() as u32` (Task 2). `item_symbol(db, file, src, fd, def_file, index) -> Option<Symbol>` and `generic_arity_diag(actual: u32, declared: u32, name: &str, span) -> Diagnostic` (Task 2) are consumed within `generic_diagnostics_query` (Task 2). `generic_diagnostics_query(db, file, src, fd, ws) -> Arc<[Diagnostic]>` (Task 2) is consumed by `AnalysisHost::generic_diagnostics` (Task 2), which is consumed by `AnalysisHost::type_diagnostics` (Task 3) and the test in Task 2. The `Fixture { name, native_rejects, rule }` shape (Task 4) matches the existing harness struct. `E3003` is used only by `generic_arity_diag` and asserted in Task 3's test.

**Sequencing rationale:** Task 1 lands the def-map arity (additive, no consumer). Task 2 builds the whole checking path (query + host bridge) and tests it directly through `generic_diagnostics` — the first resolution-fed type diagnostic. Task 3 folds it into the one surface the harness and editor consume. Task 4 pins the rule against the live compiler and confirms the corpus gate. Each task ends with an independently testable, independently reviewable deliverable.
