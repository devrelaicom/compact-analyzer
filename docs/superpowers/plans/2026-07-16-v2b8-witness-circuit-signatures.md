# v2b.8 Witness/Circuit Signature Checking — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add native type-phase signature checking for user-defined circuits and witnesses — a **duplicate-parameter-name** diagnostic (`E3005`) and a **call-argument mismatch** diagnostic (`E3006`, arg count + per-argument subtyping) — each pinned by a live-`compactc` differential fixture and provably free of false positives.

**Architecture:** Two independent rules extracted verify-first from `compact` 0.31.1 (probes) and the compiler's Chez Scheme type checker (`analysis-passes.ss` `compatible-args?` / `subtype?`, `frontend-passes.ss` `reject-duplicate!`). `E3005` (duplicate param) is a pure single-file CST walk — no resolution. `E3006` (call-arg) mirrors the existing `ledger_call_diagnostics_query` **exactly**: a resolution-fed tracked query that resolves a plain call's callee to a **same-file, unique, non-generic** user circuit/witness signature and checks each **typeable** positional argument against a **concretely-modeled** parameter type via the existing `is_subtype`. Every uncertainty suppresses.

**Tech Stack:** Rust (edition 2024), `salsa = "0.28"`, `compactp_ast`/`compactp_parser`/`compactp_syntax` CST/AST, the M2 resolver (`resolve.rs`, `db.rs`), the v2b type universe (`ty.rs`, `infer.rs`), the ledger-call machinery (`db.rs`).

## Global Constraints

- **Ground truth is live `compactc` at the pinned toolchain** (compact 0.5.1 / compiler 0.31.1 / language 0.23) — never docs, never training data. Every diagnostic rule is pinned by a differential fixture (native verdict vs `compactc`) that already agrees. A case where native suppresses but `compactc` rejects is a documented **safe false-negative**, not a fixture.
- **Never a false positive** (hard constraint): for any source `compactc` accepts, native must emit zero type diagnostics. Any uncertainty (overloaded name, generic callee, cross-file callee, non-literal argument, unmodeled type) suppresses via `TyKind::Unknown` or an early `continue`. The `corpus_no_false_positives` gate is the backstop.
- No LSP types in `analyzer-core`; `Ty`/`TyKind` stay in-crate. Byte offsets only.
- Diagnostic wording tracks `compactc`'s "where practical" but is **not** harness-gated (the harness compares accept/reject only, never wording or spans).
- Analyzer-owned diagnostic codes already taken: `E3001` return mismatch, `E3002` cast, `E3003` generic arity, `E3004` ledger method-call arg; `E9001`–`E9004` resolution. **This plan adds `E3005`** (duplicate parameter name) **and `E3006`** (call-argument mismatch). Do not reuse a taken number.
- Pre-release: no dual old/new path (memory `prerelease-no-back-compat`).

## The extracted rule (memory `v2b8-witness-circuit-signature-rule`)

- **Duplicate parameter name** — `frontend-passes.ss:278` `(reject-duplicate! src "parameter name" …)`: a circuit or witness signature with two same-named parameters is rejected (probes 09/26/64). Always provable from the parameter list.
- **Call-argument checking** — `analysis-passes.ss:1652` `compatible-args?` = `(and (= arg-count param-count) (andmap subtype? actuals params))`. The overload driver accepts a call iff **exactly one** in-scope same-named function is compatible (0 → "no compatible function", ≥2 → "call site ambiguity"). `subtype?` is the same relation used for returns — our existing `is_subtype` (Uint bound, Bytes same-size, tuple/vector covariance, `Uint <: Field`).
- **Native scope (never false-positive; safe FN OK):** `E3005` full. `E3006` only when the callee resolves to a **same-file (`df == file`), unique, non-generic** user circuit/witness — then arg-count mismatch → diag; per positional arg that `expr_ty_kind` types (literal/cast/array) into a concretely-modeled param → `!is_subtype` → diag. Suppress on: overloaded name (>1 same-named callable), generic callee, cross-file callee, spread arg, untypeable arg (param-ref/call — `expr_ty_kind` returns `Unknown`, auto-suppressing), `Unknown` param.

## Verified CST / AST facts (do not re-derive)

- **Circuit params are `PARAM` nodes** (`IDENT_PAT` name + `COLON` + type). `compactp_ast::CircuitDef::params() -> impl Iterator<Item = Param>`; `Param::ty() -> Option<Type>`. The param **name** is the first `IDENT` token inside the `PARAM` node (the pattern precedes the type).
- **Witness params are `STRUCT_FIELD` nodes, NOT `PARAM`** (the witness grammar uses `patterns::arg`, shared with struct fields). `WitnessDecl` has **no** `params()` accessor. Read them as `witness.syntax().children().filter_map(compactp_ast::StructField::cast)`; `StructField::ty() -> Option<Type>`; the name is the `StructField`'s first `IDENT` token.
- **Plain call CST:** a `CALL_EXPR` with **no** `DOT` token child. `compactp_ast::expr::CallExpr::name() -> Option<SyntaxToken>` returns the callee `IDENT` for a direct call. Positional arguments are the `Expr` children after the `L_PAREN` token (same extraction as `ledger_call_diagnostics_query`). Generic args (`f<T>(x)`) are a `GenericArgList` node, not an `Expr`, so never counted as an argument. A `SPREAD_EXPR` (`...x`) argument makes arity indeterminate → suppress the whole call.
- **`Symbol`** (`item_tree.rs`): `name: String`, `kind: SymbolKind`, `name_range: TextRange`, `generic_param_count: u32`. Callable user kinds: `SymbolKind::{Circuit, Witness}`.
- **`Definition`** (`resolve.rs`): `Item { file: FileId, index: u32 }`.
- Reuse verbatim: `resolve_query`, `item_symbol` (`db.rs`); `expr_ty_kind`, `type_node_kind` (`infer.rs`, both `pub(crate)`); `is_subtype`, `display_kind` (`ty.rs`); the `FileDeps`/`Workspace` inputs and the `CALL_EXPR` walk shape of `ledger_call_diagnostics_query`.

## File Structure

- **Modify `crates/analyzer-core/src/infer.rs`** — a `signature_diagnostics_query(db, src)` for duplicate params (`E3005`, pure single-file walk); merge both new surfaces into `AnalysisHost::type_diagnostics`. (Tasks 1, 3)
- **Modify `crates/analyzer-core/src/db.rs`** — a `callee_sig` resolver helper + the resolution-fed `call_arg_diagnostics_query` (`E3006`), mirroring `ledger_adt_at` + `ledger_call_diagnostics_query`. (Tasks 2, 3)
- **Modify `crates/analyzer-core/src/resolve.rs`** — the `AnalysisHost::call_arg_diagnostics` bridge, mirroring `AnalysisHost::ledger_call_diagnostics`. (Task 3)
- **Create fixtures** under `crates/compact-analyzer/tests/fixtures/type/` and **modify `crates/compact-analyzer/tests/type_differential.rs`** — rule-tagged (`"witness-circuit-signature"`) differential fixtures. (Task 4)

---

## Task 1: Duplicate parameter name (`E3005`)

**Files:**
- Modify: `crates/analyzer-core/src/infer.rs`

**Interfaces:**
- Produces:
  - `#[salsa::tracked(returns(clone), no_eq)] pub fn signature_diagnostics_query(db: &dyn Db, src: SourceText) -> Arc<[Diagnostic]>`
- Consumes: `parsed` (`db.rs`); `compactp_ast::{CircuitDef, WitnessDecl, StructField, Param, AstNode}`; `compactp_syntax::{SyntaxNode, SyntaxKind}`.

**Rule:** For each `CircuitDef` and `WitnessDecl`, collect its parameter names in order; the first time a name repeats, emit `E3005` anchored at the repeated occurrence's name token. Circuit params are `PARAM` children (name = first `IDENT` token); witness params are `STRUCT_FIELD` children (name = first `IDENT` token). Pure single-file walk — no resolution, always provable.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/analyzer-core/src/infer.rs`:

```rust
#[test]
fn duplicate_param_circuit_and_witness_flagged() {
    let db = CompactDatabase::default();
    // Circuit with two params named `a` -> one E3005.
    let a = SourceText::new(
        &db,
        Arc::from("export circuit c(a: Uint<8>, a: Boolean): Boolean { return true; }"),
    );
    let da = signature_diagnostics_query(&db, a);
    assert_eq!(da.len(), 1);
    assert_eq!(da[0].code.number, 3005);
    // Witness with two params named `x` -> one E3005 (witness params are STRUCT_FIELD).
    let b = SourceText::new(&db, Arc::from("witness w(x: Uint<8>, x: Boolean): Boolean;"));
    let db2 = signature_diagnostics_query(&db, b);
    assert_eq!(db2.len(), 1);
    assert_eq!(db2[0].code.number, 3005);
}

#[test]
fn distinct_params_emit_nothing() {
    let db = CompactDatabase::default();
    let a = SourceText::new(
        &db,
        Arc::from("export circuit c(a: Uint<8>, b: Boolean): Boolean { return true; }"),
    );
    assert!(signature_diagnostics_query(&db, a).is_empty());
    let b = SourceText::new(&db, Arc::from("witness w(x: Uint<8>, y: Boolean): Boolean;"));
    assert!(signature_diagnostics_query(&db, b).is_empty());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p analyzer-core --lib infer::duplicate_param infer::distinct_params`
Expected: FAIL — `signature_diagnostics_query` does not exist.

- [ ] **Step 3: Implement the query**

In `crates/analyzer-core/src/infer.rs`, add (the `use` lines at the top already import `AstNode`, `Diagnostic`, `DiagnosticCode`, `Db`, `SourceText`, `parsed`):

```rust
/// The first `IDENT` token of a parameter node (the parameter name) and its
/// range. For a circuit `PARAM` the pattern precedes the type, and for a
/// witness `STRUCT_FIELD` the name precedes the `:`, so the first `IDENT`
/// token is the parameter name in both.
fn param_name_ident(node: &compactp_syntax::SyntaxNode) -> Option<(String, text_size::TextRange)> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == compactp_syntax::SyntaxKind::IDENT)
        .map(|t| (t.text().to_string(), t.text_range()))
}

/// Push an `E3005` for the first duplicate among `params` (name, range) pairs,
/// in declaration order. Only the first repeat is reported per signature
/// (matches compactc, which stops at the first duplicate).
fn check_duplicate_params(
    params: impl Iterator<Item = (String, text_size::TextRange)>,
    diags: &mut Vec<Diagnostic>,
) {
    let mut seen: Vec<String> = Vec::new();
    for (name, range) in params {
        if seen.contains(&name) {
            diags.push(Diagnostic::error(
                DiagnosticCode::new("E", 3005),
                format!("duplicate parameter name {name}"),
                range,
            ));
            return;
        }
        seen.push(name);
    }
}

/// Signature well-formedness diagnostics for `src`: a circuit or witness with
/// two parameters of the same name (`E3005`). Pure single-file CST walk — no
/// resolution. `no_eq`: `Diagnostic` has no `PartialEq` (same rationale as
/// `type_diagnostics_query`).
#[salsa::tracked(returns(clone), no_eq)]
pub fn signature_diagnostics_query(db: &dyn Db, src: SourceText) -> Arc<[Diagnostic]> {
    use compactp_syntax::SyntaxKind;
    let root = compactp_syntax::SyntaxNode::new_root(parsed(db, src).green);
    let mut diags = Vec::new();
    // Circuit params are PARAM nodes.
    for c in root.descendants().filter_map(compactp_ast::CircuitDef::cast) {
        let params = c
            .syntax()
            .children()
            .filter(|n| n.kind() == SyntaxKind::PARAM)
            .filter_map(|n| param_name_ident(&n));
        check_duplicate_params(params, &mut diags);
    }
    // Witness params are STRUCT_FIELD nodes (shared grammar with struct fields).
    for w in root.descendants().filter_map(compactp_ast::WitnessDecl::cast) {
        let params = w
            .syntax()
            .children()
            .filter(|n| n.kind() == SyntaxKind::STRUCT_FIELD)
            .filter_map(|n| param_name_ident(&n));
        check_duplicate_params(params, &mut diags);
    }
    Arc::from(diags)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core --lib infer::duplicate_param infer::distinct_params`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.8): duplicate parameter-name signature check (E3005)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 2: Callee signature resolver (`callee_sig`)

**Files:**
- Modify: `crates/analyzer-core/src/db.rs`

**Interfaces:**
- Produces:
  - `pub(crate) struct CalleeSig { pub params: Vec<crate::ty::TyKind> }`
  - `fn callee_sig(db: &dyn Db, file: FileId, src: SourceText, fd: FileDeps, ws: Workspace, callee_off: text_size::TextSize) -> Option<CalleeSig>`
- Consumes: `resolve_query`, `item_symbol` (`db.rs`); `item_tree` (`db.rs`); `crate::infer::type_node_kind`; `compactp_ast::{CircuitDef, WitnessDecl, StructField, Param, AstNode}`; `crate::SymbolKind`.

**Rule:** Resolve the identifier at `callee_off`. Return `Some` only when it resolves to a **same-file** (`df == file`) user **`Circuit`** or **`Witness`** that is **non-generic** (`generic_param_count == 0`) and whose name is **unique** among that file's callable symbols (exactly one `Circuit`/`Witness` of that name). Then read its declared parameter types (circuit: `Param` nodes; witness: `StructField` nodes) via `type_node_kind`. Any other situation → `None` (suppress). Same-file scoping sidesteps cross-file overload sets; uniqueness + non-generic sidesteps overload ambiguity and generic `T` params.

`callee_sig` is a private, resolution-fed helper that is not independently observable without `AnalysisHost`'s internal `file_deps`/`workspace` accessors, so it carries **no standalone unit test** — its behavior is the failing-first tests in Task 3 (which won't pass until `callee_sig` and the query both exist). This task's deliverable is verified by `cargo build`.

- [ ] **Step 1: Implement `callee_sig`**

In `crates/analyzer-core/src/db.rs`, near `ledger_adt_at`, add:

```rust
/// A resolved callee's typed parameter list. Only populated for a same-file,
/// unique, non-generic user circuit/witness (see `callee_sig`); param types the
/// universe does not model lower to `TyKind::Unknown`.
pub(crate) struct CalleeSig {
    pub params: Vec<crate::ty::TyKind>,
}

/// The typed signature of the user circuit/witness the call at `callee_off`
/// resolves to, or `None` to suppress. `Some` only when the callee resolves to
/// a **same-file** (`df == file`) `Circuit`/`Witness` that is **non-generic**
/// and **unique** among the file's callable symbols. This restriction makes the
/// check overload- and cross-file-safe: with a single same-file candidate there
/// is no overload set to disambiguate, so native's verdict cannot diverge from
/// compactc's on any accepted file.
fn callee_sig(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    callee_off: text_size::TextSize,
) -> Option<CalleeSig> {
    use compactp_ast::AstNode;
    let def = resolve_query(db, file, src, fd, ws, callee_off)?;
    let crate::Definition::Item { file: df, index } = def else {
        return None;
    };
    if df != file {
        return None; // cross-file callee -> suppress (safe false-negative)
    }
    let sym = item_symbol(db, file, src, fd, df, index)?;
    if !matches!(sym.kind, crate::SymbolKind::Circuit | crate::SymbolKind::Witness) {
        return None;
    }
    if sym.generic_param_count > 0 {
        return None; // generic callee -> params may be `T` -> suppress
    }
    // Uniqueness: exactly one callable symbol of this name in the file. More than
    // one is an overload set (or an export/redefinition error) -> suppress.
    let tree = item_tree(db, src);
    let same_name_callables = tree
        .symbols
        .iter()
        .filter(|s| {
            s.name == sym.name
                && matches!(s.kind, crate::SymbolKind::Circuit | crate::SymbolKind::Witness)
        })
        .count();
    if same_name_callables != 1 {
        return None;
    }
    // Read the declared parameter types from the decl CST node (same file).
    let root = compactp_syntax::SyntaxNode::new_root(crate::db::parsed(db, src).green);
    let params: Vec<crate::ty::TyKind> = match sym.kind {
        crate::SymbolKind::Circuit => {
            let cd = root
                .descendants()
                .filter_map(compactp_ast::CircuitDef::cast)
                .find(|c| c.name().map(|n| n.text_range()) == Some(sym.name_range))?;
            cd.params()
                .map(|p| {
                    p.ty()
                        .map(|t| crate::infer::type_node_kind(&t))
                        .unwrap_or(crate::ty::TyKind::Unknown)
                })
                .collect()
        }
        crate::SymbolKind::Witness => {
            let wd = root
                .descendants()
                .filter_map(compactp_ast::WitnessDecl::cast)
                .find(|w| w.name().map(|n| n.text_range()) == Some(sym.name_range))?;
            // Witness params are STRUCT_FIELD nodes (no typed `params()` accessor).
            wd.syntax()
                .children()
                .filter_map(compactp_ast::StructField::cast)
                .map(|sf| {
                    sf.ty()
                        .map(|t| crate::infer::type_node_kind(&t))
                        .unwrap_or(crate::ty::TyKind::Unknown)
                })
                .collect()
        }
        _ => unreachable!("kind checked above"),
    };
    Some(CalleeSig { params })
}
```

> **Implementer note:** confirm `item_symbol`'s exact parameter order against its definition in `db.rs` (it is the same call `ledger_adt_at` makes — copy that call shape). `compactp_ast::StructField::ty()` and `WitnessDecl::name()` are verified to exist. `type_node_kind` is `pub(crate)` in `infer.rs`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p analyzer-core`
Expected: compiles clean (a `dead_code` warning on `callee_sig`/`CalleeSig` is fine — Task 3 wires them in this same session; if the repo gates on `-D warnings` mid-build, add a temporary `#[allow(dead_code)]` on both and remove it in Task 3).

- [ ] **Step 3: Commit**

```bash
git add crates/analyzer-core/src/db.rs
git commit -m "feat(v2b.8): callee_sig — same-file unique non-generic circuit/witness signature

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 3: Call-argument mismatch (`E3006`)

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (the tracked query)
- Modify: `crates/analyzer-core/src/resolve.rs` (the host bridge)
- Modify: `crates/analyzer-core/src/infer.rs` (merge into `type_diagnostics`)

**Interfaces:**
- Consumes: `callee_sig` (Task 2); `expr_ty_kind` (`infer.rs`); `is_subtype`, `display_kind` (`ty.rs`); the `CALL_EXPR` walk of `ledger_call_diagnostics_query`.
- Produces:
  - `db.rs`: `#[salsa::tracked(returns(clone), no_eq)] pub fn call_arg_diagnostics_query(db, file, src, fd, ws) -> Arc<[Diagnostic]>`
  - `resolve.rs`: `pub fn call_arg_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic>`
  - `infer.rs`: `AnalysisHost::type_diagnostics` extends with `self.call_arg_diagnostics(file)` and `signature_diagnostics_query`.

**Rule:** For each **plain** call `g(a0, a1, …)` (a `CALL_EXPR` with **no** `DOT` child) whose callee `g` yields a `callee_sig` (Task 2): if the positional-argument count ≠ the parameter count → emit `E3006` (count) at the call range and `continue`; otherwise for each positional argument that `expr_ty_kind` types (≠ `Unknown`) against a concretely-modeled parameter (≠ `Unknown`), if `!is_subtype(actual, param)` → emit `E3006` (type) at the argument range. A `SPREAD_EXPR` argument makes arity indeterminate → suppress the whole call. Method calls (with a `DOT`) are `E3004`'s job and are skipped here.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/analyzer-core/src/infer.rs` (the merged surface is `AnalysisHost::type_diagnostics`):

```rust
#[test]
fn call_arg_type_mismatch_emits_e3006() {
    use crate::AnalysisHost;
    use std::path::Path;
    let mut host = AnalysisHost::new();
    let file = host.vfs_mut().file_id(Path::new("/t/ca.compact"));
    host.vfs_mut().set_overlay(
        file,
        "circuit f(x: Boolean): Boolean { return x; }\n\
         export circuit c(): Boolean { return f(5); }"
            .to_string(),
        1,
    );
    let diags = host.type_diagnostics(file);
    assert!(
        diags.iter().any(|d| d.code.number == 3006),
        "expected E3006, got {:?}",
        diags.iter().map(|d| d.code.number).collect::<Vec<_>>()
    );
}

#[test]
fn call_arg_count_mismatch_emits_e3006() {
    use crate::AnalysisHost;
    use std::path::Path;
    let mut host = AnalysisHost::new();
    let file = host.vfs_mut().file_id(Path::new("/t/cc.compact"));
    host.vfs_mut().set_overlay(
        file,
        "circuit f(x: Boolean, y: Boolean): Boolean { return x; }\n\
         export circuit c(): Boolean { return f(true); }"
            .to_string(),
        1,
    );
    assert!(host.type_diagnostics(file).iter().any(|d| d.code.number == 3006));
}

#[test]
fn call_arg_good_overloaded_and_generic_suppress() {
    use crate::AnalysisHost;
    use std::path::Path;
    // Correct arg -> no E3006.
    let mut h1 = AnalysisHost::new();
    let f1 = h1.vfs_mut().file_id(Path::new("/t/ok.compact"));
    h1.vfs_mut().set_overlay(
        f1,
        "circuit f(x: Uint<8>): Boolean { return true; }\n\
         export circuit c(): Boolean { return f(5); }"
            .to_string(),
        1,
    );
    assert!(h1.type_diagnostics(f1).iter().all(|d| d.code.number != 3006));

    // Overloaded name (two `f`) -> suppress even though f(5) mismatches the Boolean one.
    let mut h2 = AnalysisHost::new();
    let f2 = h2.vfs_mut().file_id(Path::new("/t/ovl.compact"));
    h2.vfs_mut().set_overlay(
        f2,
        "circuit f(x: Boolean): Boolean { return x; }\n\
         circuit f(x: Uint<8>): Boolean { return true; }\n\
         export circuit c(): Boolean { return f(5); }"
            .to_string(),
        1,
    );
    assert!(h2.type_diagnostics(f2).iter().all(|d| d.code.number != 3006));

    // Generic callee -> param is `T` (Unknown) -> suppress.
    let mut h3 = AnalysisHost::new();
    let f3 = h3.vfs_mut().file_id(Path::new("/t/gen.compact"));
    h3.vfs_mut().set_overlay(
        f3,
        "circuit f<T>(x: T): Boolean { return true; }\n\
         export circuit c(): Boolean { return f<Uint<8>>(5); }"
            .to_string(),
        1,
    );
    assert!(h3.type_diagnostics(f3).iter().all(|d| d.code.number != 3006));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p analyzer-core --lib infer::call_arg`
Expected: FAIL — `E3006` is never emitted (`type_diagnostics` does not yet consult call args).

- [ ] **Step 3: Implement the tracked query in `db.rs`**

In `crates/analyzer-core/src/db.rs`, add an `E3006` constructor and the query, reusing the `ledger_call_diagnostics_query` walk shape:

```rust
/// The call-argument mismatch diagnostic (`E3006`). Wording tracks compactc's
/// family ("no compatible function …") loosely; wording is not gated.
fn call_arg_diag(msg: String, span: text_size::TextRange) -> Diagnostic {
    Diagnostic::error(DiagnosticCode::new("E", 3006), msg, span)
}

/// Call-argument type checks (`E3006`) for `src`. For each plain call
/// `g(args)` (a `CALL_EXPR` with no `DOT`) whose callee resolves to a same-file,
/// unique, non-generic user circuit/witness (`callee_sig`): an arg-count
/// mismatch is reported at the call; otherwise each positional argument that is
/// a typeable literal/cast/array is checked against a concretely-modeled
/// parameter. Suppresses on every uncertainty. Resolution-fed like
/// `ledger_call_diagnostics_query`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn call_arg_diagnostics_query(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
) -> Arc<[Diagnostic]> {
    use compactp_ast::AstNode;
    let root = compactp_syntax::SyntaxNode::new_root(crate::db::parsed(db, src).green);
    let mut diags = Vec::new();

    for call in root
        .descendants()
        .filter_map(compactp_ast::expr::CallExpr::cast)
    {
        // A method call has a DOT child (that is E3004's job); a plain call does not.
        let has_dot = call
            .syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == compactp_syntax::SyntaxKind::DOT);
        if has_dot {
            continue;
        }
        let Some(name_tok) = call.name() else {
            continue;
        };
        let callee_off = name_tok.text_range().start();
        let Some(sig) = callee_sig(db, file, src, fd, ws, callee_off) else {
            continue;
        };

        // Positional argument Exprs after the L_PAREN.
        let Some(lparen_start) = call
            .syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == compactp_syntax::SyntaxKind::L_PAREN)
            .map(|t| t.text_range().start())
        else {
            continue;
        };
        let args: Vec<compactp_ast::expr::Expr> = call
            .syntax()
            .children()
            .filter_map(compactp_ast::expr::Expr::cast)
            .filter(|e| e.syntax().text_range().start() >= lparen_start)
            .collect();

        // A spread argument makes arity indeterminate -> suppress this call.
        if args
            .iter()
            .any(|e| e.syntax().kind() == compactp_syntax::SyntaxKind::SPREAD_EXPR)
        {
            continue;
        }

        // Arg-count mismatch -> one diagnostic at the call; skip per-arg checks.
        if args.len() != sig.params.len() {
            diags.push(call_arg_diag(
                format!(
                    "call to {} supplies {} argument(s) but its signature declares {}",
                    name_tok.text(),
                    args.len(),
                    sig.params.len()
                ),
                call.syntax().text_range(),
            ));
            continue;
        }

        for (i, arg) in args.iter().enumerate() {
            let param = &sig.params[i];
            if *param == crate::ty::TyKind::Unknown {
                continue; // unmodeled param -> suppress
            }
            let actual = crate::infer::expr_ty_kind(arg);
            if actual == crate::ty::TyKind::Unknown {
                continue; // non-literal/cast/array arg (incl. param-ref) -> suppress
            }
            if !crate::ty::is_subtype(&actual, param) {
                diags.push(call_arg_diag(
                    format!(
                        "argument {} of {} has type {} but the parameter type is {}",
                        i + 1,
                        name_tok.text(),
                        crate::ty::display_kind(&actual),
                        crate::ty::display_kind(param),
                    ),
                    arg.syntax().text_range(),
                ));
            }
        }
    }
    Arc::from(diags)
}
```

> **Implementer note:** `display_kind` and `is_subtype` are `pub(crate)` in `ty.rs`; `expr_ty_kind` is `pub(crate)` in `infer.rs`. If `DiagnosticCode`/`Diagnostic` are not already imported in `db.rs`, they are used by the existing `ledger_arg_diag` — reuse those imports.

- [ ] **Step 4: Implement the host bridge in `resolve.rs`**

In `crates/analyzer-core/src/resolve.rs`, immediately after `AnalysisHost::ledger_call_diagnostics` (inside the same `impl AnalysisHost` block), add the exact twin — identical to `ledger_call_diagnostics`/`generic_diagnostics` except the query name:

```rust
    /// Call-argument mismatch diagnostics (`E3006`) for `file`. Resolution-fed
    /// (callee resolution), so it indexes the file to materialize dependency
    /// edges, then calls the tracked `call_arg_diagnostics_query`. Merged into
    /// `type_diagnostics` (v2b.8); editor surfacing is v2b.final.
    pub fn call_arg_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        self.ensure_indexed(file);
        let fd = self.file_deps(file);
        let ws = self.workspace();
        crate::db::call_arg_diagnostics_query(self.db_ref(), file, src, fd, ws).to_vec()
    }
```

- [ ] **Step 5: Merge both new surfaces into `type_diagnostics`**

In `crates/analyzer-core/src/infer.rs`, extend `AnalysisHost::type_diagnostics` so the single type-diagnostics surface also carries duplicate-param and call-arg diagnostics:

```rust
    pub fn type_diagnostics(&mut self, file: crate::FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        let mut diags = type_diagnostics_query(self.db_ref(), src).to_vec();
        diags.extend(signature_diagnostics_query(self.db_ref(), src).to_vec());
        diags.extend(self.generic_diagnostics(file));
        diags.extend(self.ledger_call_diagnostics(file));
        diags.extend(self.call_arg_diagnostics(file));
        diags
    }
```

(`signature_diagnostics_query` takes only `src` and is already in scope in `infer.rs`; the `&mut self` bridges must be called after the immutable `db_ref()` borrows end — keep the ordering above, which appends the `&mut self` calls last.)

> **Implementer note:** if the borrow checker rejects interleaving `self.db_ref()` (immutable) with `self.call_arg_diagnostics(file)` (`&mut self`), collect the two query results into owned `Vec`s first (as `.to_vec()` already does), then run the `&mut self` bridges — exactly as the current method already sequences `generic_diagnostics`/`ledger_call_diagnostics` after the `type_diagnostics_query` `.to_vec()`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core --lib`
Expected: PASS — the new `infer::call_arg*` tests pass and every pre-existing test still passes.

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/resolve.rs crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.8): call-argument mismatch check (E3006)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 4: Witness/circuit signature differential fixtures

**Files:**
- Create: eight files under `crates/compact-analyzer/tests/fixtures/type/`
- Modify: `crates/compact-analyzer/tests/type_differential.rs`

**Interfaces:**
- Consumes: the `FIXTURES` array and `Fixture { name, native_rejects, rule }` struct in `type_differential.rs`; `rule` tag for these is `"witness-circuit-signature"`.
- Each fixture is a complete `.compact` file whose native verdict (`!host.type_diagnostics(file).is_empty()`) equals `native_rejects`, and — when the toolchain is present — whose `compactc` verdict is `Accept` for `native_rejects=false` and `RejectPostParse` for `native_rejects=true`. All verdicts below were captured from live `compact` 0.31.1.

- [ ] **Step 1: Create the eight fixture files**

Each begins with `pragma language_version >= 0.23;` on line 1.

`sig_dup_param_circuit.compact` (reject — duplicate circuit param, `E3005`):
```
pragma language_version >= 0.23;
export circuit c(a: Uint<8>, a: Boolean): Boolean { return true; }
```
`sig_dup_param_witness.compact` (reject — duplicate witness param, `E3005`):
```
pragma language_version >= 0.23;
witness w(x: Uint<8>, x: Boolean): Boolean;
```
`sig_distinct_params_ok.compact` (accept — distinct params):
```
pragma language_version >= 0.23;
export circuit c(a: Uint<8>, b: Boolean): Boolean { return true; }
```
`sig_arg_type_mismatch.compact` (reject — `5` into `Boolean` param, `E3006`):
```
pragma language_version >= 0.23;
circuit f(x: Boolean): Boolean { return x; }
export circuit c(): Boolean { return f(5); }
```
`sig_arg_count_mismatch.compact` (reject — one arg into two params, `E3006`):
```
pragma language_version >= 0.23;
circuit f(x: Boolean, y: Boolean): Boolean { return x; }
export circuit c(): Boolean { return f(true); }
```
`sig_arg_ok.compact` (accept — `5` into `Uint<8>` param):
```
pragma language_version >= 0.23;
circuit f(x: Uint<8>): Boolean { return true; }
export circuit c(): Boolean { return f(5); }
```
`sig_overload_ok.compact` (accept — overloaded `f`, native suppresses, compactc picks the `Uint<8>` one):
```
pragma language_version >= 0.23;
circuit f(x: Boolean): Boolean { return x; }
circuit f(x: Uint<8>): Boolean { return true; }
export circuit c(): Boolean { return f(5); }
```
`sig_arg_tuple_ok.compact` (accept — tuple literal arg fits the tuple param):
```
pragma language_version >= 0.23;
circuit f(t: [Uint<8>, Boolean]): Boolean { return true; }
export circuit c(): Boolean { return f([5, true]); }
```

- [ ] **Step 2: Verify each fixture's `compactc` verdict matches the claim**

For each file, run `compact compile --skip-zk --vscode <file> <scratch-out-dir>` and confirm: the four `_ok`/`distinct`/`overload`/`tuple` files exit 0 (accept); the `dup_param`/`arg_type`/`arg_count` files exit non-zero with a post-parse type error (duplicate-parameter or no-compatible-function). If any verdict differs from the claim, fix the fixture (not the harness) and re-record.

Expected reference (captured 0.31.1): `sig_dup_param_circuit`→reject; `sig_dup_param_witness`→reject; `sig_distinct_params_ok`→accept; `sig_arg_type_mismatch`→reject; `sig_arg_count_mismatch`→reject; `sig_arg_ok`→accept; `sig_overload_ok`→accept; `sig_arg_tuple_ok`→accept.

- [ ] **Step 3: Register the fixtures in the harness**

In `crates/compact-analyzer/tests/type_differential.rs`, append to the `FIXTURES` array (before the closing `];`):

```rust
    Fixture { name: "sig_dup_param_circuit.compact", native_rejects: true, rule: "witness-circuit-signature" },
    Fixture { name: "sig_dup_param_witness.compact", native_rejects: true, rule: "witness-circuit-signature" },
    Fixture { name: "sig_distinct_params_ok.compact", native_rejects: false, rule: "witness-circuit-signature" },
    Fixture { name: "sig_arg_type_mismatch.compact", native_rejects: true, rule: "witness-circuit-signature" },
    Fixture { name: "sig_arg_count_mismatch.compact", native_rejects: true, rule: "witness-circuit-signature" },
    Fixture { name: "sig_arg_ok.compact", native_rejects: false, rule: "witness-circuit-signature" },
    Fixture { name: "sig_overload_ok.compact", native_rejects: false, rule: "witness-circuit-signature" },
    Fixture { name: "sig_arg_tuple_ok.compact", native_rejects: false, rule: "witness-circuit-signature" },
```

- [ ] **Step 4: Run the differential harness**

Run: `cargo test -p compact-analyzer --test type_differential rule_tagged_fixtures`
Expected: PASS. With `compactc` on `PATH` the live cross-check runs (each new fixture's native verdict must equal its `compactc` verdict); otherwise it prints "compactc absent; skipped live cross-check" and asserts only the native side.

- [ ] **Step 5: Commit**

```bash
git add crates/compact-analyzer/tests/fixtures/type/sig_*.compact \
        crates/compact-analyzer/tests/type_differential.rs
git commit -m "test(v2b.8): rule-tagged witness/circuit signature differential fixtures

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 5: Corpus no-false-positive gate

**Files:** none (verification only)

**Rule:** The blocking release-direction gate — for every corpus file `compactc` accepts, native emits zero type diagnostics. The two new surfaces (`E3005`, `E3006`) must not false-positive on any accepted corpus file.

- [ ] **Step 1: Run the corpus gate with a real corpus**

Run: `COMPACT_CORPUS_DIR=<path-to-corpus> cargo test -p compact-analyzer --test type_differential corpus_no_false_positives -- --nocapture`
Expected: PASS with `false_positives=0`. It prints `accepted=… compiler_rejected(post-parse)=… indeterminate=… false_positives=0`. If it self-skips ("compactc absent" / "no COMPACT_CORPUS_DIR"), locate the corpus (the `../../../compactp/tests/corpus` checkout or a `COMPACT_CORPUS_DIR`) and re-run — the gate must actually execute before v2b.8 is considered done.

- [ ] **Step 2: If any false positive appears, fix and re-run**

A false positive means native flagged a file `compactc` accepts — by definition a bug in `callee_sig`/`call_arg_diagnostics_query`/`signature_diagnostics_query` (e.g. an overload set not suppressed, a modeled type that should be `Unknown`). Tighten the suppression (add an early `continue`), add a regression test in `infer.rs`, and re-run until `false_positives=0`. Do not weaken the fixtures.

- [ ] **Step 3: Full suite green**

Run: `cargo test --workspace`
Expected: PASS. Then `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` (the repo gates on both).
```
