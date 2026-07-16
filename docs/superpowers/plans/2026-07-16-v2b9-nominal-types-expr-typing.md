# v2b.9 — Nominal Struct/Enum Types + Expression-Level Typing (v2c foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend v2b's checker with nominal `struct`/`enum` types and an expression-at-offset typing query, so v2c can offer typed member completion, richer inlay hints, and signature help. This is the type-system extension the user chose over a consumer-scoped v2c.

**Architecture:** Today `TyKind` has no struct/enum and `expr_ty_kind` types only literals/casts/arrays; everything else is `Unknown`. This plan adds nominal `Struct`/`Enum` to `TyKind`, a resolution-fed query that lowers a struct/enum type reference to its nominal `TyKind` (reading fields/variants from the decl), and a tracked `expr_ty` query that types an expression at a byte offset (identifier refs via the resolver → declared type; member access via struct field lookup; call results via the callee's return type; plus the existing literal/cast/array cases). The IDE (v2c) consumes `expr_ty` — it does not escape `analyzer-core`. All rules are extracted verify-first (see `memory/v2b9-nominal-struct-enum-expr-typing-rule.md`) and pinned by `compactc`-differential fixtures before implementation.

**Tech Stack:** Rust (edition 2024), `salsa = "0.28"`, `compactp_ast`/`compactp_syntax` CST, the v2b.1 tracked def-map/resolver.

## Global Constraints

- **Verify-first (mandatory).** Every new type rule is extracted from live `compactc` + the compiler's Chez Scheme type checker at the pinned tag, NEVER from training data, and pinned by a `compactc`-differential fixture **before** implementation. Extraction is already done and recorded in `memory/v2b9-nominal-struct-enum-expr-typing-rule.md` and the scratch probes; each task re-confirms its specific rule with a fixture whose verdict is captured from the real toolchain.
- **Never a false positive (HARD).** For any source `compactc` accepts, native emits zero type diagnostics. New typing widens what `infer_circuit_returns`/`callee_sig` can type, so the **corpus binary gate must stay `false_positives=0`** after every task. Any uncertainty types to `TyKind::Unknown` (which suppresses). Deeper/uncoverable cases are safe false-negatives.
- **No LSP types in `analyzer-core`.** `Ty`/`TyKind` stay in-crate; the IDE gets a display string or a small typed projection. Byte offsets only.
- **Salsa-tracked, single-threaded.** New queries are `#[salsa::tracked]` over `SourceText`/def-map inputs; long walks poll `should_continue`. No cross-thread `Cancelled`.
- **Pre-release.** No back-compat shims, migrations, or dual paths — clean wholesale edits.
- **Nominal typing.** Structs compare by name (+ field types); enums by name (+ variant set). `Enum<E, A, B>` is the display form (verified). A struct/enum reference that cannot be resolved to a decl types to `Unknown` (suppress, never guess).

## Background: extracted rules (do not re-derive — re-confirm each with a fixture)

From live `compactc 0.31.1` probes + `analysis-passes.ss` (`elt-ref` :2516-2539, `enum-ref` :2587, struct subtype :355):
- **Struct field access `expr.field`** types as the field's **declared type**; missing field → "structure S has no field named X"; non-struct receiver → "expected structure type, received T".
- **`const x = <init>`** flows the initializer's type; **`const x: T = <init>`** checks init against T ("mismatch between actual type … and declared type … of const binding").
- A const bound to a **struct value** is typed that struct (field access then works); bound to a **call result** is typed by the callee's **declared return type**.
- **Enum**: `E.A` types as the whole enum type, displayed `Enum<E, A, B>`.
- **Return mismatch** wording (existing E3001): "mismatch between actual return type … and declared return type … of circuit …".

## Existing internals this plan builds on (verified)

- `crates/analyzer-core/src/ty.rs`: `enum TyKind { Boolean, Field, Unknown, Uint(Option<u128>), Bytes(u32), Tuple(Arc<[TyKind]>), Vector(u32, Arc<TyKind>) }`; `#[salsa::interned] struct Ty { kind: TyKind }`; `pub(crate) fn is_subtype(&TyKind,&TyKind)->bool`; `pub(crate) fn display_kind(&TyKind)->String` (delegated to by `pub fn ty_display(db,Ty)->String`); `pub(crate) fn can_cast(...)`.
- `crates/analyzer-core/src/infer.rs`: `pub(crate) fn type_node_kind(&compactp_ast::Type) -> TyKind` (lowers a syntactic `Type`; currently returns `Unknown` for a named struct/enum type ref); `pub(crate) fn expr_ty_kind(&Expr) -> TyKind` (literal/cast/array only); `pub fn infer_circuit_returns(db, src, idx)`.
- `crates/analyzer-core/src/item_tree.rs`: `Symbol { name, kind, name_range, signature, .. }`; `SymbolKind::{Struct, StructField, Enum, EnumVariant, ..}`; `ItemTree::children_of(parent) -> impl Iterator<Item=(u32,&Symbol)>`. Struct/enum decls carry their fields/variants as child symbols; a `StructField`'s CST node exposes `.name()` and `.ty()` (a `compactp_ast::Type`).
- `crates/analyzer-core/src/resolve.rs`: `enum Definition { Item { file, index }, Local { name, detail, .. } }`; `AnalysisHost::resolve(FilePosition) -> Option<Definition>`; params/locals resolve to `Definition::Local` with a `detail` string `"name: ty"`.
- `crates/analyzer-core/src/db.rs`: `item_tree(db,src)`, `resolve_query(...)`, `callee_sig(db, file, src, fd, ws, callee_off) -> Option<CalleeSig{ params: Vec<TyKind> }>` (same-file/unique/non-generic circuit/witness).

---

## File Structure

- **Modify** `crates/analyzer-core/src/ty.rs` — add `TyKind::Struct`/`TyKind::Enum`; extend `display_kind` + `is_subtype`. (Task 1)
- **Modify** `crates/analyzer-core/src/infer.rs` — nominal type lowering in `type_node_kind` (resolution-fed) + the `expr_ty` typing function + host entry. (Tasks 2–3)
- **Modify** `crates/analyzer-core/src/db.rs` — tracked `struct_type` / `enum_type` lowering query + tracked `expr_ty` query. (Tasks 2–3)
- **Modify** `crates/analyzer-core/src/lib.rs` — re-export the new host method (`type_at`) and any public types.
- **Create** `crates/compact-analyzer/tests/fixtures/type/nom_*.compact` — differential fixtures. (Tasks 1–4)
- **Modify** `crates/compact-analyzer/tests/type_differential.rs` — register fixtures with rule tag `"nominal-expr-typing"`.

---

### Task 1: Nominal `Struct` & `Enum` in `TyKind` (+ display + subtype)

Add the two nominal variants and extend the pure `display_kind` and `is_subtype` functions. No inference yet — this is the type model + its lattice/display, pinned by the extracted subtype rule.

**Files:**
- Modify: `crates/analyzer-core/src/ty.rs`
- Test: `crates/analyzer-core/src/ty.rs` (unit tests), plus differential fixtures in Task 4.

**Interfaces:**
- Produces: `TyKind::Struct { name: Arc<str>, fields: Arc<[(Arc<str>, TyKind)]> }`, `TyKind::Enum { name: Arc<str>, variants: Arc<[Arc<str>]> }`; extended `display_kind`/`is_subtype` covering them.

- [ ] **Step 1: Verify-first — pin the nominal subtype rule with probes**

Before coding, confirm the struct/enum subtype rule against the live toolchain (record outputs in the report). Run each through `compact compile --skip-zk --vscode <file> <out>`:
- Same struct, assignable to itself (accept): `struct S { a: Field; } circuit c(s: S): S { return s; }` → ACCEPT.
- Two structurally-identical but differently-named structs are NOT interchangeable (nominal): `struct A { x: Field; } struct B { x: Field; } circuit c(a: A): B { return a; }` → expect REJECT. If it ACCEPTS, structs are structural not nominal — STOP and report (the `is_subtype` rule below must change).
- Enum to itself (accept): `enum E { A, B } circuit c(e: E): E { return e; }` → ACCEPT.

Record the exact verdicts; they justify the `is_subtype` arms below.

- [ ] **Step 2: Write the failing unit tests**

Add to `ty.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn struct_display_is_its_name() {
    let s = TyKind::Struct { name: Arc::from("S"), fields: Arc::from([(Arc::<str>::from("a"), TyKind::Field)]) };
    assert_eq!(display_kind(&s), "S");
}

#[test]
fn enum_display_lists_variants() {
    let e = TyKind::Enum { name: Arc::from("E"), variants: Arc::from([Arc::<str>::from("A"), Arc::<str>::from("B")]) };
    assert_eq!(display_kind(&e), "Enum<E, A, B>");
}

#[test]
fn struct_subtype_is_nominal_same_name() {
    let s1 = TyKind::Struct { name: Arc::from("S"), fields: Arc::from([(Arc::<str>::from("a"), TyKind::Field)]) };
    let s2 = s1.clone();
    let other = TyKind::Struct { name: Arc::from("T"), fields: Arc::from([(Arc::<str>::from("a"), TyKind::Field)]) };
    assert!(is_subtype(&s1, &s2));
    assert!(!is_subtype(&s1, &other));      // nominal: different name -> not assignable
    assert!(is_subtype(&s1, &TyKind::Unknown)); // Unknown always suppresses
}

#[test]
fn enum_subtype_is_nominal_same_name() {
    let e1 = TyKind::Enum { name: Arc::from("E"), variants: Arc::from([Arc::<str>::from("A")]) };
    let e2 = e1.clone();
    let f = TyKind::Enum { name: Arc::from("F"), variants: Arc::from([Arc::<str>::from("A")]) };
    assert!(is_subtype(&e1, &e2));
    assert!(!is_subtype(&e1, &f));
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p analyzer-core --lib ty::` — expected FAIL to compile (variants don't exist).

- [ ] **Step 4: Add the variants**

In `enum TyKind` add (keeping the existing derives — confirm `TyKind` derives `Clone, Debug, PartialEq, Eq, Hash`; `Arc<str>`/`Arc<[..]>` satisfy all):

```rust
    /// A nominal `struct` type: its declared name plus its ordered
    /// `(field-name, field-type)` list. Assignability is nominal (same name),
    /// verified against compactc — a differently-named struct of identical
    /// shape is NOT assignable. Fields carry types so member access can type
    /// `s.field` (v2c). An unresolvable struct reference lowers to `Unknown`.
    Struct { name: Arc<str>, fields: Arc<[(Arc<str>, TyKind)]> },
    /// A nominal `enum` type: its declared name plus its ordered variant names.
    /// Displays as `Enum<Name, V0, V1, …>` (verified). Nominal assignability.
    Enum { name: Arc<str>, variants: Arc<[Arc<str>]> },
```

- [ ] **Step 5: Extend `display_kind` and `is_subtype`**

In `display_kind`, before the closing `}`:

```rust
        TyKind::Struct { name, .. } => name.to_string(),
        TyKind::Enum { name, variants } => {
            let mut out = format!("Enum<{name}");
            for v in variants.iter() {
                out.push_str(", ");
                out.push_str(v);
            }
            out.push('>');
            out
        }
```

In `is_subtype`, add arms BEFORE the final `_ => false` (Unknown arm already covers the suppress case at the top):

```rust
        // Nominal: structs assignable iff same name (compactc is nominal — a
        // differently-named struct of identical shape is not assignable, Step 1).
        (TyKind::Struct { name: a, .. }, TyKind::Struct { name: b, .. }) => a == b,
        (TyKind::Enum { name: a, .. }, TyKind::Enum { name: b, .. }) => a == b,
```

Ensure `use std::sync::Arc;` is in scope in the tests module (mirror existing).

- [ ] **Step 6: Run tests + lints**

Run: `cargo test -p analyzer-core --lib` (new + all existing green), `cargo clippy -p analyzer-core --all-targets -- -D warnings`, `cargo fmt --all --check`.
Note: adding non-`Copy` fields to `TyKind` — confirm no code relied on `TyKind: Copy` (the memory notes v2b.7 already moved `TyKind` Copy→Clone, so this is safe). If a `Copy` bound surfaces, fix the call site to clone; report it.

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/ty.rs
git commit -m "$(cat <<'EOF'
feat(v2b.9): nominal Struct/Enum in TyKind (display + nominal subtype)

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Resolution-fed nominal type lowering

Lower a struct/enum **type reference** (a `NAMED_TYPE` naming a struct/enum decl) to its nominal `TyKind`, by resolving the name to its decl and reading the fields/variants. This is what makes a param `s: S` type to `TyKind::Struct{...}`. It is resolution-fed (like `callee_sig`), so it lives as a tracked query in `db.rs` with a thin `infer.rs` wrapper.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (tracked `named_type_ty` query)
- Modify: `crates/analyzer-core/src/infer.rs` (make `type_node_kind` delegate to it for named types)
- Test: `crates/analyzer-core/src/db.rs` unit tests + differential fixtures (Task 4)

**Interfaces:**
- Consumes: `item_tree`, `resolve_query`/def-map, `TyKind::Struct/Enum` (Task 1), `type_node_kind` (recursively for field types).
- Produces: `db::named_type_ty(db, file, src, ws, name_offset) -> TyKind` — the nominal `TyKind` for the type-name token at `name_offset`, or `Unknown` if it does not resolve to a struct/enum decl.

- [ ] **Step 1: Verify-first fixture (accept) capturing param-of-struct field typing**

Confirm live: `struct S { a: Field; b: Boolean; } export circuit c(s: S): Boolean { return s.b; }` → ACCEPT (already probed as P9; re-run and record). And the reject twin `... : Field { return s.b; }` → REJECT "actual return type Boolean". These pin that a param typed `S` yields field `b: Boolean`.

- [ ] **Step 2: Write the failing unit test**

In `db.rs` tests, assert the lowering builds the nominal type (use the existing test-db pattern in this file — mirror `callee_sig`/`item_tree` tests for constructing `SourceText`/workspace input and locating an offset):

```rust
#[test]
fn named_type_lowers_struct_to_nominal() {
    // A circuit param `s: S` — the `S` type-name token lowers to
    // TyKind::Struct { name: "S", fields: [("a", Field), ("b", Boolean)] }.
    // (Construct db + SourceText for the source below, find the byte offset of
    // the `S` in `s: S`, call named_type_ty, assert the TyKind.)
    // source:
    // "struct S { a: Field; b: Boolean; }\nexport circuit c(s: S): Boolean { return s.b; }"
    // ... assert kind == TyKind::Struct { name:"S", fields:[("a",Field),("b",Boolean)] }
}
```

Fill in the db/offset scaffolding exactly like the nearest existing `db.rs` test. Expected FAIL (query doesn't exist).

- [ ] **Step 3: Implement the tracked lowering query**

Add to `db.rs`:

```rust
/// Lowers the struct/enum type-name token at `name_offset` in `file` to its
/// nominal `TyKind`, by resolving the name to a Struct/Enum decl and reading
/// its child fields/variants from the item tree. Returns `TyKind::Unknown` when
/// the name does not resolve to a struct/enum decl (suppress, never guess) —
/// this preserves never-a-false-positive. Resolution-fed, mirroring callee_sig.
#[salsa::tracked]
pub(crate) fn named_type_ty(
    db: &dyn Db,
    file: FileId,
    src: SourceText,
    ws: Workspace,          // match callee_sig's workspace-input parameter shape
    name_offset: TextSize,
) -> TyKind {
    // 1. Resolve the name token at `name_offset` to a Definition::Item.
    // 2. Fetch that item's Symbol from the target file's item_tree; require
    //    SymbolKind::Struct or SymbolKind::Enum (else Unknown).
    // 3. For a Struct: iterate item_tree.children_of(struct_index); for each
    //    SymbolKind::StructField child, read its CST node's `.ty()` and lower it
    //    with type_node_kind (recursively — a field may itself be a struct);
    //    collect (Arc<str> name, TyKind). Build TyKind::Struct.
    //    For an Enum: collect EnumVariant child names into TyKind::Enum.
    // 4. Guard recursion depth / self-reference with should_continue and a
    //    visited-set or depth cap so a self-referential struct can't loop
    //    (a struct field of its own type -> lower the inner ref to a
    //    name-only Struct with empty/opaque fields, or Unknown; pick the
    //    Unknown-safe option and note it). Never a false positive.
    todo!("implement per the numbered steps; keep every unresolved/uncertain branch -> TyKind::Unknown")
}
```

Resolve exact helper names (`resolve_query`, the def-map accessor, `Workspace` input type, `children_of`) against the current `db.rs`/`resolve.rs`; the numbered comments are the contract. **Recursion/cycle safety is load-bearing** — a struct that (transitively) contains its own type must not infinitely recurse; cap depth and lower the back-edge to `Unknown`. Record the chosen cycle strategy in the report.

- [ ] **Step 4: Delegate `type_node_kind` for named types**

In `infer.rs`, `type_node_kind` currently returns `Unknown` for a named (struct/enum) type. Named-type lowering needs resolution (db + file + workspace), which `type_node_kind(&Type)` does not have. Introduce a resolution-aware entry the callers use where a named type must become nominal (e.g. param typing for `expr_ty`, Task 3), delegating to `named_type_ty`. Keep the pure `type_node_kind` for primitive/sequence lowering; add a `type_kind_resolved(db, file, src, ws, &Type) -> TyKind` that returns `type_node_kind` for primitives and `named_type_ty(...)` for a `NAMED_TYPE`. Do NOT change `type_node_kind`'s existing callers that only need primitive lowering unless they must now see nominal types (call those out in the report).

- [ ] **Step 5: Run tests + corpus gate**

Run: `cargo test -p analyzer-core --lib`; then the corpus gate:
`COMPACT_CORPUS_DIR=/tmp/tmp.wexEHos1Kf/compact/examples cargo test -p compact-analyzer --test type_differential corpus_no_false_positives -- --nocapture` — MUST print `false_positives=0`. (Nominal lowering can only widen `infer_circuit_returns`; verify it introduced no false positive.)

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/infer.rs
git commit -m "$(cat <<'EOF'
feat(v2b.9): resolution-fed nominal struct/enum type lowering

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Expression-at-offset typing query (`expr_ty` + `type_at` host entry)

Type an expression at a byte offset: identifier refs (resolve → declared type), member access (struct field lookup), call results (callee return type), and the existing literal/cast/array cases. This is the query v2c consumes for member completion, inlay hints, and signature-help detail.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (tracked `expr_ty` query)
- Modify: `crates/analyzer-core/src/infer.rs` (typing logic) + host method `type_at`
- Modify: `crates/analyzer-core/src/lib.rs` (export `type_at`)
- Test: unit tests in `db.rs`/`infer.rs` + differential fixtures (Task 4)

**Interfaces:**
- Consumes: `named_type_ty` (Task 2), `callee_sig`, `resolve_query`, `type_kind_resolved`, `expr_ty_kind`.
- Produces: `AnalysisHost::type_at(&mut self, file: FileId, offset: TextSize) -> Option<TyKind>` (IDE-facing; `None` when unmodeled/`Unknown`). A tracked `db::expr_ty(db, file, src, ws, offset) -> TyKind`.

- [ ] **Step 1: Verify-first fixtures**

Re-confirm and record (already probed P2/P4/P10): `const x = s.a` (Field), `const y = s; y.a` (Field via local-of-struct), `const s = mk(); s.a` where `mk(): S` (via call result). Capture accept/reject twins for Task 4.

- [ ] **Step 2: Write the failing unit tests**

In `db.rs`/`infer.rs` tests, assert `expr_ty` at chosen offsets. Mirror the existing test-db scaffolding. Cases:
- identifier `s` (param of `S`) → `TyKind::Struct{name:"S", ..}`.
- member `s.a` → the field's type.
- call `mk()` (mk returns `S`) → `TyKind::Struct{name:"S", ..}`.
- unmodeled (e.g. a witness call to a generic) → `TyKind::Unknown` (suppress).

Expected FAIL (query doesn't exist).

- [ ] **Step 3: Implement `expr_ty`**

Add a tracked `expr_ty(db, file, src, ws, offset) -> TyKind` that finds the enclosing `Expr` node at `offset` and types it:

```text
match expr {
  Literal/Cast/Array            => expr_ty_kind(expr)               // existing
  NAME_EXPR (identifier ref)    => resolve to Definition:
      Item{struct/enum decl}    => named_type_ty(that decl)         // a type used as a value? rare; else Unknown
      Item{circuit/witness}     => Unknown (a bare fn ref isn't a value here)
      Local{param}              => type_kind_resolved(param's Type annotation)
      Local{const/local}        => expr_ty(its initializer)  (bounded recursion; annotated const -> its annotation)
  MEMBER_EXPR (recv.field)      => let r = expr_ty(recv);
                                   if r == Struct{fields} && field in fields => that field's TyKind
                                   else Unknown
  CALL_EXPR (callee(args))      => callee's declared return type
                                   (reuse callee_sig's callee-resolution to find the return Type; lower it)
  _                             => Unknown
}
```

Every unhandled/uncertain branch returns `Unknown`. Bound recursion (identifier→initializer→…, member chains) with `should_continue` + a depth cap; a cycle or over-deep chain returns `Unknown` (safe FN). Param typing reuses `type_kind_resolved` (Task 2). Const/local initializer typing recurses through `expr_ty`. **Do not** attempt cross-file locals or generic instantiation — `Unknown`.

- [ ] **Step 4: Host entry `type_at`**

Add `AnalysisHost::type_at(&mut self, file, offset) -> Option<TyKind>` returning `None` for `TyKind::Unknown`, else `Some(kind)`. Export it in `lib.rs`. (The IDE will render it via `display_kind`/`ty_display` and enumerate `Struct.fields` for member completion — that is v2c, next plan.)

- [ ] **Step 5: Run tests + corpus gate**

`cargo test -p analyzer-core --lib`; corpus gate `false_positives=0` (as Task 2). `expr_ty` is IDE-only (no new diagnostics), so the corpus verdict must be unchanged — confirm.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/infer.rs crates/analyzer-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(v2b.9): expression-at-offset typing query (expr_ty + type_at host entry)

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Differential fixtures + corpus gate (rule-tagged)

Pin the nominal/expression rules with `compactc`-differential fixtures and prove the corpus gate stays green.

**Files:**
- Create: `crates/compact-analyzer/tests/fixtures/type/nom_*.compact`
- Modify: `crates/compact-analyzer/tests/type_differential.rs`

- [ ] **Step 1: Author fixtures with live-captured verdicts**

For each, run `compact compile --skip-zk --vscode <file> <out>` and record the verdict; register with `rule: "nominal-expr-typing"`:
- `nom_field_access_ok.compact` — `struct S { a: Field; } export circuit c(s: S): Field { return s.a; }` (accept).
- `nom_field_access_mismatch.compact` — same but `: Boolean` (reject; E3001-attributable once native types `s.a`).
- `nom_const_flow_mismatch.compact` — `... { const x = s.a; return x; }` with `: Boolean` (reject).
- `nom_nested_field_ok.compact` — nested struct `t.s.a` (accept the matching-return twin).
- `nom_enum_value_ok.compact` — `enum E { A, B } export circuit c(): E { return E.A; }` (accept).
- `nom_bad_field.compact` — `s.nope` (reject) — native currently can't attribute this (no member-access diagnostic); register `native_rejects: false` if native stays silent, matching the "safe false-negative" posture, OR add a member-access diagnostic only if it stays corpus-green (decide + note; default: leave as FN, native silent, fixture asserts compiler-reject only where the harness supports it).

Confirm each fixture's tag/verdict matches the harness's `FIXTURES` shape (mirror the v2b.8 `sig_*` registrations).

- [ ] **Step 2: Run rule-tagged + corpus gate**

`cargo test -p compact-analyzer --test type_differential rule_tagged_fixtures`; then the corpus gate `false_positives=0`.

- [ ] **Step 3: Commit**

```bash
git add crates/compact-analyzer/tests/fixtures/type/nom_*.compact crates/compact-analyzer/tests/type_differential.rs
git commit -m "$(cat <<'EOF'
test(v2b.9): rule-tagged nominal/expr-typing differential fixtures

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

## Final verification (whole-milestone)

- [ ] `cargo test --workspace` green (lib + all integration).
- [ ] Corpus gate `false_positives=0` (the load-bearing gate — nominal typing widened inference).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all --check` clean.
- [ ] `AnalysisHost::type_at` returns nominal `Struct`/`Enum` for params/locals/const/member/call in a manual smoke, `None` for unmodeled — the v2c consumer contract.

## What this unblocks (next plan: v2c)

With `type_at` returning nominal `Struct{fields}`/`Enum{variants}` and the display form, v2c becomes a pure consumer:
- **Typed member completion**: `receiver.⎸` → `type_at(receiver)`; if `Struct{fields}`, offer the field names (kind `Field`) with `display_kind(field_ty)` detail.
- **Inlay hints**: `const x = <init>` → `type_at(init)` → `ty_display` (now rich, not just literals).
- **Signature help**: existing `callee_sig` + item-tree signature + CST active-param.

## Notes / risks for the reviewer

- **Never-a-false-positive is the gate.** Nominal lowering widens `infer_circuit_returns`; the corpus gate (`false_positives=0`) is the non-negotiable proof after Tasks 2–4.
- **Cycle safety** in `named_type_ty` and `expr_ty` (self-referential structs, `const a = b; const b = a;`) must be bounded — depth cap → `Unknown`. Called out in Tasks 2/3.
- **Nominal vs structural** hinges on the Task 1 Step 1 probe; if `compactc` proves structural, the `is_subtype` arms change to fieldwise comparison — do not assume, verify.
- Bad-field / member-on-non-struct / const-binding-mismatch are *available* new diagnostics but are **out of scope here** (v2b.9 delivers the typing query for v2c, not new diagnostic surface) unless a fixture shows they stay corpus-green and the reviewer/human approves.
