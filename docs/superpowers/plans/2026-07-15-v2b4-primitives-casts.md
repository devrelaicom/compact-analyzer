# v2b.4 — Primitives + Casts (`as`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the second distinctive type rule on the v2b spine — model the `Bytes<n>` primitive, type `as` cast expressions to their target type, and enforce cast legality — so the native checker agrees with `compactc` on Bytes assignability, cast-through-the-lattice return checks, and illegal casts.

**Architecture:** Extend the interned `Ty` universe with a `Bytes(u32)` kind; add Bytes to the subtype lattice (same-size only) and lower `Bytes<n>` annotations; type a cast expression `e as T` to its **target** type `T` (which then flows through the existing `is_subtype` return check); and add a separate `can_cast` legality relation that flags illegal casts (`Boolean`↔`Bytes`, `Bytes` size-changing) as a new diagnostic. All single-file, over v2b.3's `item_tree`/`parsed` inference entry point — no name resolution. The rule is pinned by live-`compactc` rule-tagged differential fixtures and stays inside the v2b.2 no-false-positive corpus gate.

**Tech Stack:** Rust 2024, salsa 0.28 (`#[salsa::interned]`, `#[salsa::tracked]`), `compactp_ast`/`compactp_syntax` (external CST), the v2b.2 `analyzer-toolchain` differential harness.

## Global Constraints

Every task's requirements implicitly include this section.

- **Verify-first.** The compiler at the pinned toolchain (`compact 0.5.1` / compiler `0.31.1` / language `0.23.0`) is ground truth — never docs, never training data. The cast + Bytes rules were extracted empirically from the live compiler (its accept/reject verdicts and its mismatch messages, which print its internal type representation directly) **before** any implementation; the exact rule and every accept/reject verdict are recorded in the *Extracted rule* section below, and re-asserted by the rule-tagged fixtures (Task 6).
- **No LSP types in `analyzer-core`;** byte offsets (`text_size`) only. `Ty` does **not** escape `analyzer-core` — the IDE layer gets the `ty_display` `String` projection.
- **Single-threaded**, cooperative `should_continue`; no salsa cross-thread `Cancelled`.
- **Diagnostics are tracked queries returning `Arc<[Diagnostic]>`** (not accumulators); the `no_eq` diagnostics precedent (`resolution_diagnostics_query`, `type_diagnostics_query`) is unchanged here.
- **Never a false positive.** For any source `compactc` accepts, the native checker must emit zero type diagnostics. `is_subtype` and `can_cast` are **conservative**: any pair not *confirmed* related/illegal resolves in the safe direction (subtype-check → accept; cast-legality → legal → suppress). Non-literal cast sizes and unmodeled types lower to `Unknown`, which suppresses. The v2b.2 binary corpus gate (`corpus_no_false_positives`) must stay green.
- **Pre-release.** No back-compat shims/migrations/dual code paths — clean wholesale replacement (`prerelease-no-back-compat`).
- **Never commit `[patch.crates-io]`.**
- **Editor integration is out of scope here.** Surfacing type diagnostics in VS Code + the native-vs-compiler toggle remains **v2b.final**. This plan only broadens what `AnalysisHost::type_diagnostics` reports.
- **Rule scope.** In scope: the `Bytes<n>` primitive type (annotation lowering + same-size assignability), typing cast expressions to their target type, and cast-legality (`can_cast`) applied to return-position casts whose operand type the checker knows (a primitive literal). **Out of scope (deliberate, recorded):** typing cast operands / illegal-cast detection for **non-literal** operands (parameters, locals, arbitrary expressions) and **non-return-position** casts — both need full expression inference (the same boundary v2b.3 drew for the Uint lattice); `Opaque`/`Vector`/`Tuple`/record/type-ref primitives (unmodeled → `Unknown` → suppress); `Bytes` well-formedness diagnostics. These are conservative false *negatives*, never false positives.

## Extracted rule (ground truth — `compact 0.5.1` / compiler `0.31.1`, verified 2026-07-15)

Captured empirically from the live compiler; every verdict below is reproduced by a Task-6 fixture or an inline unit test. Full source-of-truth notes live in the `v2b4-cast-primitives-rule` memory.

- **Cast result type.** `e as T` has result type **exactly `T`** (the annotated target), *independent of the operand type*, and then flows through the normal subtype lattice at its use site. Evidence: `x as Uint<8>` returned into `Uint<0..10>` → *"mismatch between actual return type `Uint<8>` and declared return type `Uint<0..10>`"*; `x as Uint<16>` into `Uint<8>` → mismatch; `5 as Field` into `Boolean` → *"actual `Field` … declared `Boolean`"*.
- **`Bytes<n>` primitive.** Surface `Bytes<n>` denotes a byte array of length `n`. Its display is `Bytes<n>`. **Implicit assignability** (subtyping, no cast): `Bytes<n> <: Bytes<m>` iff `n == m`; `Bytes` is otherwise **unrelated** to every other type — `Bytes<32>` is *not* assignable to `Field`, `Uint<8>` is *not* assignable to `Bytes<32>`, `Bytes<4>` is *not* assignable to `Bytes<32>`. Evidence: `Bytes<32>→Bytes<32>` accept; `Bytes<4>→Bytes<32>`, `Bytes<32>→Field`, `Uint<8>→Bytes<32>`, `return 5` (`Uint<0..6>`) `→Bytes<32>` all reject as return mismatches.
- **Cast legality `can_cast(S, T)`** (far more permissive than subtyping). Among modeled primitives {`Boolean`, `Field`, `Uint`, `Bytes`}, the **only** rejected casts:
  - `Boolean` ↔ `Bytes<n>` (either direction) — *"cannot cast from type Boolean to type Bytes<32>"*.
  - `Bytes<a>` ↔ `Bytes<b>` with `a != b` — *"cannot cast from type Bytes<32> to type Bytes<4>"* (Bytes casts must preserve size).
  Everything else casts freely: all `Boolean`/`Field`/`Uint` pairs interconvert; `Field`/`Uint` ↔ `Bytes<any>` both directions; `Bytes<n> as Bytes<n>` (identity); `Uint→Uint` ignores bounds (`Uint<0..10> as Uint<0..5>` accepts). `Field as Opaque<"…">` rejects (Opaque is unmodeled → treated as `Unknown` → suppressed, a safe false negative).
- **Diagnostics** (wording tracked "where practical", **not** harness-gated):
  - Illegal cast: **`E3002`** — `cannot cast from type {S} to type {T}` (`S`/`T` normalized display).
  - Return mismatch: unchanged **`E3001`** family — `mismatch between actual return type {A} and declared return type {D} of circuit {name}`.

**Representation.** Bytes sizes are small byte counts → model as `TyKind::Bytes(u32)`. A non-literal size (a generic numeric parameter) or one overflowing `u32` lowers to `Unknown` (suppresses; corpus-safe — accepted files with concrete `Bytes<n>` always have literal, small `n`). The `Uint(Option<u128>)` representation from v2b.3 is unchanged.

## Drift reconciliation (checked against as-merged v2b.3, commit `234ce4e`)

Checked this plan against the merged v2b.3 code (`crates/analyzer-core/src/ty.rs`, `infer.rs`, `crates/compact-analyzer/tests/type_differential.rs`). Findings folded in:

1. **`ty.rs` stays a single file.** v2b.3 kept `is_subtype`/`uint_upper_le` in `ty.rs` (deferring a `ty/` directory split as YAGNI until covariance/generics). `can_cast` joins them there — still one focused file. No split this rule.
2. **`parse_int_literal` already handles all four radices.** v2b.3's final review extended `parse_int_literal` to decimal/hex/**octal/binary** (`0o`/`0b`). `bytes_size` (Task 3) reuses it unchanged — a `Bytes<0o40>` size parses correctly for free.
3. **The shared return-walk is `circuit_return_kinds` → `Vec<(TextRange, TyKind)>`.** v2b.3 unified `infer_circuit_returns` and `type_diagnostics_query` onto it (one green-tree walk, discharging the O(n²) carry-forward). Task 4 extends this walk to type **casts** (keeping the `(span, kind)` shape); Task 5 refactors it to `circuit_returns → Vec<ReturnTy>` to also carry an illegal-cast attribution — the walk stays single and shared, so the v2b.3 perf property is preserved.
4. **`circuit_return_kinds` currently matches `Expr::Literal` only** and *skips* every other operand. Task 4 broadens the modeled set to `Literal | Cast` (via a new `expr_ty_kind`); all other operands still skip (unchanged behavior — they were, and stay, untyped). No foundation/`Uint` test asserts a count over a non-literal, non-cast operand, so this is additive.
5. **`TyKind` is `Copy, Eq, Hash`.** Adding `Bytes(u32)` keeps all three (satisfied by `u32`), so the interned `Ty` field (`#[returns(copy)] kind: TyKind`) is unaffected. The only exhaustive `TyKind` matches are `ty_display` + `is_subtype` (ty.rs) and `type_node_kind` (infer.rs) — each gets a `Bytes` arm; there is no exhaustive external match (`TyKind` is re-exported from `lib.rs` but only used opaquely).
6. **`infer_circuit_returns` is consumed only inside `infer.rs`** (its own tests + `type_diagnostics_query`), and only its `.1` (`TyKind`) is read. Its `Arc<[(TextRange, TyKind)]>` signature is preserved; Task 5 projects it from `circuit_returns`.
7. **New diagnostic code `E3002`.** `DiagnosticCode::new("E", 3002)` — the E-codes are the analyzer's own numbering (E3001 was chosen in the foundation), not compiler-assigned; wording tracks compactc but is not gated.

## File Structure

- `crates/analyzer-core/src/ty.rs` **(modify)** — add the `Bytes(u32)` kind, its `ty_display` arm, the `Bytes` arm in `is_subtype`, and the new `can_cast` legality relation. Responsibility: the type universe + its relations + display.
- `crates/analyzer-core/src/infer.rs` **(modify)** — add `bytes_size` (lower `Bytes<n>` annotations in `type_node_kind`); add `expr_ty_kind` (type literal + cast operands); extend the shared return-walk to type casts and (Task 5) carry illegal-cast attribution; add `cast_error` + `cast_error_diag`; emit `E3002` in `type_diagnostics_query`. Responsibility: inference + type diagnostics.
- `crates/compact-analyzer/tests/fixtures/type/*.compact` **(create)** — seven cast/Bytes fixtures captured from `compactc`.
- `crates/compact-analyzer/tests/type_differential.rs` **(modify)** — add the seven fixtures to the `FIXTURES` table (rule `"cast-primitives"`). The `corpus_no_false_positives` gate is unchanged and self-skips locally.

## Standard verification commands (used by every task)

- Build+test a crate: `cargo test -p <crate> -q`
- Whole workspace: `cargo test --workspace -q`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Format check: `cargo fmt --all --check`

---

### Task 1: `TyKind::Bytes(u32)` + display projection

Extend the type universe with the `Bytes` primitive and its display. No `Bytes` is *produced* yet (annotations are lowered in Task 3), so this task is additive and every existing test stays green.

**Files:**
- Modify: `crates/analyzer-core/src/ty.rs` (`TyKind` enum ~16-32; `ty_display` ~46-63; `mod tests`)

**Interfaces:**
- Consumes: `crate::db::Db`.
- Produces: `TyKind::Bytes(u32)` (the byte length); `ty_display` renders it as `Bytes<n>`.

- [ ] **Step 1: Write the failing test** — add to `ty.rs` `mod tests`:

```rust
    #[test]
    fn bytes_display_and_interning() {
        let db = CompactDatabase::default();
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Bytes(32))), "Bytes<32>");
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Bytes(1))), "Bytes<1>");
        // interns by size
        let a = Ty::new(&db, TyKind::Bytes(32));
        let b = Ty::new(&db, TyKind::Bytes(32));
        let c = Ty::new(&db, TyKind::Bytes(4));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
```

- [ ] **Step 2: Run it to verify it fails** — `cargo test -p analyzer-core ty::tests::bytes_display -q`. Expected: FAIL to compile (`TyKind::Bytes` undefined).

- [ ] **Step 3: Add the `Bytes` variant** — in `ty.rs`, add to `TyKind` (after the `Uint` variant):

```rust
    /// A `Bytes<n>` byte array of length `n`. Implicitly assignable only to
    /// `Bytes<n>` of the *same* length (see [`is_subtype`]); castable per
    /// [`can_cast`]. Byte counts are small, so the length is a `u32`; a
    /// non-literal or overflowing size lowers to `Unknown` upstream.
    Bytes(u32),
```

- [ ] **Step 4: Add the display arm** — in `ty_display`, add an arm (e.g. after the `Uint(None)` arm):

```rust
        TyKind::Bytes(n) => format!("Bytes<{n}>"),
```

- [ ] **Step 5: Run + verify** — `cargo test -p analyzer-core ty:: -q`. Expected: PASS (new + existing). Then `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/ty.rs
git commit -m "feat(v2b.4): TyKind::Bytes(u32) + display

The Bytes primitive carries its byte length; ty_display renders Bytes<n>.
No Bytes is produced yet (Task 3 lowers annotations), so additive.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: `Bytes` in the subtype lattice (`is_subtype`)

Add the extracted `Bytes` assignability edge (`Bytes<n> <: Bytes<m>` iff `n == m`; unrelated to everything else). Behavior-preserving at this point (no `Bytes` is produced yet), so all existing tests stay green.

**Files:**
- Modify: `crates/analyzer-core/src/ty.rs` (`is_subtype` ~69-81; `mod tests`)

**Interfaces:**
- Consumes: `crate::ty::TyKind`.
- Produces: `is_subtype` now relates `Bytes` to `Bytes` (same size) and to nothing else.

- [ ] **Step 1: Write the failing test** — add to `ty.rs` `mod tests`, transcribing the extracted Bytes edges:

```rust
    #[test]
    fn bytes_subtype_is_same_size_only() {
        use TyKind::*;
        // reflexive by size
        assert!(is_subtype(Bytes(32), Bytes(32)));
        assert!(is_subtype(Bytes(1), Bytes(1)));
        // different size is unrelated (both directions)
        assert!(!is_subtype(Bytes(4), Bytes(32)));
        assert!(!is_subtype(Bytes(32), Bytes(4)));
        // Bytes is unrelated to every other kind
        assert!(!is_subtype(Bytes(32), Field));
        assert!(!is_subtype(Field, Bytes(32)));
        assert!(!is_subtype(Bytes(32), Uint(Some(256))));
        assert!(!is_subtype(Uint(Some(256)), Bytes(32)));
        assert!(!is_subtype(Bytes(32), Boolean));
        assert!(!is_subtype(Boolean, Bytes(32)));
        // Unknown still suppresses in both directions
        assert!(is_subtype(Unknown, Bytes(32)));
        assert!(is_subtype(Bytes(32), Unknown));
    }
```

- [ ] **Step 2: Run it to verify it fails** — `cargo test -p analyzer-core ty::tests::bytes_subtype -q`. Expected: FAIL (`is_subtype(Bytes(32), Bytes(32))` currently hits the `_ => false` catch-all and returns `false`).

- [ ] **Step 3: Add the `Bytes` arm** — in `ty.rs` `is_subtype`, add an arm **before** the final `_ => false`:

```rust
        // Bytes<n> <: Bytes<m> iff n == m; Bytes is unrelated to all else.
        (TyKind::Bytes(a), TyKind::Bytes(b)) => a == b,
```

(The `(Unknown, _) | (_, Unknown) => true` arm already precedes this, so `Bytes` vs `Unknown` still suppresses; every `Bytes`-vs-other-kind pair falls through to `_ => false`, which is exactly the extracted "unrelated" behavior.)

- [ ] **Step 4: Run + verify** — `cargo test -p analyzer-core ty:: -q`. Expected: PASS (new + existing, including `subtype_lattice_matches_compiler`). Then `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/ty.rs
git commit -m "feat(v2b.4): Bytes assignability (same-size only) in is_subtype

Bytes<n> <: Bytes<m> iff n == m; Bytes is unrelated to every other kind,
matching compactc. Behavior-preserving until Bytes types start flowing.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: Lower `Bytes<n>` annotations to `TyKind::Bytes`

Teach `type_node_kind` to read a `Bytes<n>` annotation into `TyKind::Bytes(n)`, so a circuit's declared return type can be a real `Bytes`. This enables the first new true-positive class: a `Uint`/`Bytes`-of-wrong-size literal returned into a `Bytes<n>` now correctly rejects.

**Files:**
- Modify: `crates/analyzer-core/src/infer.rs` (`type_node_kind` ~25-33; add `bytes_size`; add lowering + end-to-end tests)

**Interfaces:**
- Consumes: `compactp_ast::{Type, BytesType, TypeSize, AstNode}`, `crate::ty::TyKind`, existing `parse_int_literal`.
- Produces: `type_node_kind` now maps `Type::Bytes` via `fn bytes_size(b: &compactp_ast::BytesType) -> TyKind` (module-private).

- [ ] **Step 1: Write the failing tests** — add to `infer.rs` `mod tests`:

```rust
    #[test]
    fn bytes_annotation_lowers_to_size() {
        let db = CompactDatabase::default();
        // Bytes<32> -> Bytes(32)
        let a = SourceText::new(&db, Arc::from("export circuit f(): Bytes<32> { }"));
        let ia = circuit_index(&db, a, "f");
        let ca = circuit_node_by_index(&db, a, ia).unwrap();
        assert_eq!(type_node_kind(&ca.return_type().unwrap()), TyKind::Bytes(32));
        // Bytes<1>
        let b = SourceText::new(&db, Arc::from("export circuit f(): Bytes<1> { }"));
        let ib = circuit_index(&db, b, "f");
        let cb = circuit_node_by_index(&db, b, ib).unwrap();
        assert_eq!(type_node_kind(&cb.return_type().unwrap()), TyKind::Bytes(1));
        // Generic/non-literal size -> Unknown (suppresses; not our diagnostic)
        let c = SourceText::new(&db, Arc::from("export circuit f(): Bytes<N> { }"));
        let ic = circuit_index(&db, c, "f");
        let cc = circuit_node_by_index(&db, c, ic).unwrap();
        assert_eq!(type_node_kind(&cc.return_type().unwrap()), TyKind::Unknown);
    }

    #[test]
    fn uint_literal_into_bytes_return_rejects() {
        let db = CompactDatabase::default();
        // return 5 (Uint<0..6>) into Bytes<32>: Uint not <: Bytes -> one diagnostic
        // (matches compactc: "mismatch ... actual Uint<0..6> ... declared Bytes<32>").
        let src = SourceText::new(&db, Arc::from("export circuit f(): Bytes<32> { return 5; }"));
        let diags = type_diagnostics_query(&db, src);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("Uint<0..6>") && diags[0].message.contains("Bytes<32>"),
            "message uses normalized display: {:?}",
            diags[0].message
        );
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p analyzer-core infer::tests::bytes_annotation -q` and `cargo test -p analyzer-core infer::tests::uint_literal_into_bytes -q`. Expected: FAIL (`Bytes<32>` currently lowers to `Unknown`, so both asserts fail).

- [ ] **Step 3: Add the `bytes_size` helper** — add to `infer.rs` (near `uint_bound`):

```rust
/// Lower a `Bytes<n>` annotation to `TyKind::Bytes(n)`. Byte counts are small,
/// so the length is a `u32`; a non-literal size (a generic numeric parameter)
/// or one overflowing `u32` lowers to `Unknown` (conservatively suppresses —
/// corpus-safe, since accepted files with a concrete `Bytes<n>` always have a
/// literal, small `n`). Reuses `parse_int_literal` (decimal/hex/octal/binary).
fn bytes_size(b: &compactp_ast::BytesType) -> TyKind {
    match b
        .size()
        .and_then(|s| parse_int_literal(&s.syntax().text().to_string()))
    {
        Some(n) if n <= u32::MAX as u128 => TyKind::Bytes(n as u32),
        _ => TyKind::Unknown,
    }
}
```

- [ ] **Step 4: Route `Type::Bytes` through `bytes_size`** — update `type_node_kind` (add a `Bytes` arm before the `_`):

```rust
pub(crate) fn type_node_kind(ty: &compactp_ast::Type) -> TyKind {
    use compactp_ast::Type;
    match ty {
        Type::Boolean(_) => TyKind::Boolean,
        Type::Field(_) => TyKind::Field,
        Type::Uint(u) => uint_bound(u),
        Type::Bytes(b) => bytes_size(b),
        _ => TyKind::Unknown,
    }
}
```

- [ ] **Step 5: Run + verify** — `cargo test -p analyzer-core infer:: -q`. Expected: PASS (new lowering + end-to-end tests, all unaffected tests). Then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.4): lower Bytes<n> annotations to TyKind::Bytes

type_node_kind reads Bytes<n> into TyKind::Bytes(n) via bytes_size (reusing
parse_int_literal). Non-literal/overflowing sizes lower to Unknown
(conservative). A Uint/wrong literal returned into Bytes<n> now correctly
rejects, matching compactc.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 4: Type cast expressions to their target type

Teach the shared return-walk to type an `e as T` cast operand: its result type is the **target** `T` (per the extracted rule), which then flows through the existing `is_subtype` return check. This is the headline cast deliverable — return-position checking now sees through casts.

**Files:**
- Modify: `crates/analyzer-core/src/infer.rs` (add `expr_ty_kind`; broaden `circuit_return_kinds` ~166-181 to model `Cast`; add cast-typing tests)

**Interfaces:**
- Consumes: `compactp_ast::expr::{Expr, CastExpr, LiteralExpr}`, existing `literal_ty_kind`, `type_node_kind`.
- Produces: `fn expr_ty_kind(expr: &compactp_ast::expr::Expr) -> TyKind` (module-private) — result type of a modeled operand (literal → its `TyKind`; cast → its target `TyKind`; else `Unknown`). `circuit_return_kinds` keeps its `Vec<(TextRange, TyKind)>` signature.

- [ ] **Step 1: Write the failing tests** — add to `infer.rs` `mod tests`:

```rust
    #[test]
    fn cast_return_types_to_target() {
        let db = CompactDatabase::default();
        // x as Uint<8> -> result type Uint<8> (exclusive upper 256), regardless of operand.
        let a = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Field): Field { return x as Uint<8>; }"),
        );
        let ia = circuit_index(&db, a, "f");
        assert_eq!(infer_circuit_returns(&db, a, ia)[0].1, TyKind::Uint(Some(256)));
        // x as Bytes<32> -> result type Bytes(32).
        let b = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Field): Bytes<32> { return x as Bytes<32>; }"),
        );
        let ib = circuit_index(&db, b, "f");
        assert_eq!(infer_circuit_returns(&db, b, ib)[0].1, TyKind::Bytes(32));
    }

    #[test]
    fn cast_flows_through_return_lattice() {
        let db = CompactDatabase::default();
        // Cast into a WIDER Uint: Uint<8> <: Uint<16> -> accept.
        let ok = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Field): Uint<16> { return x as Uint<8>; }"),
        );
        assert!(type_diagnostics_query(&db, ok).is_empty());
        // Cast into a NARROWER Uint: Uint<8> not <: Uint<0..10> -> reject.
        let bad = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Field): Uint<0..10> { return x as Uint<8>; }"),
        );
        let diags = type_diagnostics_query(&db, bad);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("Uint<8>") && diags[0].message.contains("Uint<0..10>"),
            "message: {:?}",
            diags[0].message
        );
        // Legal same-size Bytes cast into a DIFFERENT-size Bytes return: reject
        // (cast is legal identity -> Bytes<4>; Bytes<4> not <: Bytes<32>).
        let bytes = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Bytes<4>): Bytes<32> { return x as Bytes<4>; }"),
        );
        assert_eq!(type_diagnostics_query(&db, bytes).len(), 1);
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p analyzer-core infer::tests::cast_ -q`. Expected: FAIL (`circuit_return_kinds` currently matches `Expr::Literal` only and skips casts, so the `Cast` operand produces no entry / no diagnostic).

- [ ] **Step 3: Add `expr_ty_kind`** — add to `infer.rs` (near `literal_ty_kind`):

```rust
/// The result `TyKind` of an expression the checker models: a primitive
/// literal (its own type), or a cast `e as T` (its *target* type `T`, per the
/// extracted rule — a cast's result is exactly its annotation, independent of
/// the operand). Any other expression -> `Unknown` (suppresses). Cast operands
/// nest naturally (an `(e as A) as B` types to `B`).
fn expr_ty_kind(expr: &compactp_ast::expr::Expr) -> TyKind {
    use compactp_ast::expr::Expr;
    match expr {
        Expr::Literal(lit) => literal_ty_kind(lit),
        Expr::Cast(cast) => cast
            .ty()
            .map(|t| type_node_kind(&t))
            .unwrap_or(TyKind::Unknown),
        _ => TyKind::Unknown,
    }
}
```

- [ ] **Step 4: Broaden `circuit_return_kinds` to model casts** — replace its body so it types `Literal | Cast` operands via `expr_ty_kind` (other operands still skip):

```rust
fn circuit_return_kinds(circuit: &compactp_ast::CircuitDef) -> Vec<(text_size::TextRange, TyKind)> {
    let Some(body) = circuit.body() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for stmt in body.stmts() {
        let compactp_ast::Stmt::Return(ret) = stmt else {
            continue;
        };
        let Some(value) = ret.value() else {
            continue;
        };
        // Only operands the checker can type: primitive literals and casts.
        // Everything else stays untyped (skipped), as before.
        if matches!(
            value,
            compactp_ast::expr::Expr::Literal(_) | compactp_ast::expr::Expr::Cast(_)
        ) {
            out.push((ret.syntax().text_range(), expr_ty_kind(&value)));
        }
    }
    out
}
```

- [ ] **Step 5: Run + verify** — `cargo test -p analyzer-core infer:: -q`. Expected: PASS (new cast tests + all existing). Then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.4): type cast expressions to their target type

expr_ty_kind types a literal to its own type and a cast e as T to its
target T (independent of operand), per compactc. circuit_return_kinds now
models Literal | Cast operands, so return-position checking sees through
casts and flows them through the is_subtype lattice.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: `can_cast` legality + illegal-cast diagnostic (`E3002`)

Add the extracted cast-legality relation and emit `E3002` for an illegal return-position cast whose operand type the checker knows (a primitive literal). Refactor the shared return-walk to `circuit_returns` so it carries an illegal-cast attribution alongside each return's result type, keeping a single shared walk.

**Files:**
- Modify: `crates/analyzer-core/src/ty.rs` (add `can_cast` + a `can_cast_matches_compiler` test)
- Modify: `crates/analyzer-core/src/infer.rs` (add `ReturnTy` + `circuit_returns` replacing `circuit_return_kinds`; add `cast_error` + `cast_error_diag`; wire `type_diagnostics_query`; project `infer_circuit_returns`; add illegal-cast tests)

**Interfaces:**
- Consumes: `crate::ty::{TyKind, is_subtype, can_cast, ty_display}`, `compactp_ast::expr::{Expr, CastExpr}`, `compactp_diagnostics::{Diagnostic, DiagnosticCode}`.
- Produces:
  - `pub(crate) fn can_cast(src: TyKind, tgt: TyKind) -> bool` (ty.rs) — `true` iff `src as tgt` is accepted by compactc; `Unknown` on either side ⇒ `true`; any pair not *confirmed* illegal ⇒ `true`.
  - `struct ReturnTy { span: text_size::TextRange, kind: TyKind, cast_error: Option<(TyKind, TyKind)> }` (infer.rs, module-private) and `fn circuit_returns(circuit: &compactp_ast::CircuitDef) -> Vec<ReturnTy>` replacing `circuit_return_kinds`.
  - `fn cast_error(cast: &compactp_ast::expr::CastExpr) -> Option<(TyKind, TyKind)>` (infer.rs, module-private).

- [ ] **Step 1: Write the failing `can_cast` test** — add to `ty.rs` `mod tests`, transcribing the extracted legality relation:

```rust
    #[test]
    fn can_cast_matches_compiler() {
        use TyKind::*;
        // The only illegal casts among modeled primitives:
        assert!(!can_cast(Boolean, Bytes(32))); // Boolean -> Bytes
        assert!(!can_cast(Bytes(32), Boolean)); // Bytes -> Boolean
        assert!(!can_cast(Bytes(32), Bytes(4))); // Bytes size change
        assert!(!can_cast(Bytes(4), Bytes(32)));
        // Everything else is castable:
        assert!(can_cast(Bytes(32), Bytes(32))); // identity
        assert!(can_cast(Boolean, Field));
        assert!(can_cast(Field, Boolean));
        assert!(can_cast(Boolean, Uint(Some(256))));
        assert!(can_cast(Uint(Some(6)), Boolean));
        assert!(can_cast(Uint(Some(6)), Bytes(32))); // Uint <-> Bytes any size
        assert!(can_cast(Bytes(32), Uint(Some(256))));
        assert!(can_cast(Field, Bytes(4)));
        assert!(can_cast(Bytes(4), Field));
        assert!(can_cast(Uint(Some(256)), Uint(Some(6)))); // Uint->Uint ignores bounds
        // Unknown on either side -> legal (suppress).
        assert!(can_cast(Unknown, Bytes(32)));
        assert!(can_cast(Boolean, Unknown));
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p analyzer-core ty::tests::can_cast -q`. Expected: FAIL to compile (`can_cast` undefined).

- [ ] **Step 3: Implement `can_cast`** — add to `ty.rs` (near `is_subtype`):

```rust
/// Cast legality: `true` iff `src as tgt` is accepted by the compiler's type
/// checker (verified against compact 0.5.1 / compiler 0.31.1). Casts are far
/// more permissive than subtyping ([`is_subtype`]) — every `Boolean`/`Field`/
/// `Uint` pair interconverts, and `Field`/`Uint` interconvert with `Bytes` of
/// any size. The *only* rejected casts among modeled primitives:
///   - `Boolean` <-> `Bytes` (either direction), and
///   - `Bytes<a>` <-> `Bytes<b>` with `a != b` (Bytes casts must preserve size).
/// `Unknown` on either side yields `true` (the checker only suppresses on an
/// unmodeled type; it never manufactures a cast error from one). Any pair not
/// *confirmed* illegal is treated as legal — conservative, never a false
/// positive.
pub(crate) fn can_cast(src: TyKind, tgt: TyKind) -> bool {
    match (src, tgt) {
        (TyKind::Unknown, _) | (_, TyKind::Unknown) => true,
        // Boolean cannot cast to or from Bytes.
        (TyKind::Boolean, TyKind::Bytes(_)) | (TyKind::Bytes(_), TyKind::Boolean) => false,
        // Bytes-to-Bytes must preserve size.
        (TyKind::Bytes(a), TyKind::Bytes(b)) => a == b,
        // Every other pair among modeled primitives is castable.
        _ => true,
    }
}
```

- [ ] **Step 4: Run the `can_cast` test** — `cargo test -p analyzer-core ty::tests::can_cast -q`. Expected: PASS.

- [ ] **Step 5: Write the failing illegal-cast diagnostic tests** — add to `infer.rs` `mod tests`:

```rust
    #[test]
    fn illegal_cast_from_literal_emits_diagnostic() {
        let db = CompactDatabase::default();
        // true as Bytes<32>: Boolean -> Bytes is illegal -> E3002.
        let src = SourceText::new(
            &db,
            Arc::from("export circuit f(): Bytes<32> { return true as Bytes<32>; }"),
        );
        let diags = type_diagnostics_query(&db, src);
        assert_eq!(diags.len(), 1, "one cast error");
        assert_eq!(diags[0].code.prefix, "E");
        assert_eq!(diags[0].code.number, 3002);
        assert!(
            diags[0].message.contains("cannot cast")
                && diags[0].message.contains("Boolean")
                && diags[0].message.contains("Bytes<32>"),
            "message: {:?}",
            diags[0].message
        );
    }

    #[test]
    fn legal_literal_casts_emit_nothing() {
        let db = CompactDatabase::default();
        // Uint literal casts are all legal; Boolean -> non-Bytes is legal.
        for code in [
            "export circuit f(): Bytes<32> { return 5 as Bytes<32>; }", // Uint -> Bytes ok
            "export circuit f(): Boolean { return 5 as Boolean; }",     // Uint -> Boolean ok
            "export circuit f(): Uint<8> { return true as Uint<8>; }",  // Boolean -> Uint ok
        ] {
            let src = SourceText::new(&db, Arc::from(code));
            assert!(
                type_diagnostics_query(&db, src).is_empty(),
                "expected no diagnostic for: {code}"
            );
        }
    }
```

- [ ] **Step 6: Run to verify they fail** — `cargo test -p analyzer-core infer::tests::illegal_cast -q`. Expected: FAIL (no cast-error machinery yet; `true as Bytes<32>` currently types to `Bytes<32>`, matches the `Bytes<32>` return, and emits nothing).

- [ ] **Step 7: Add `cast_error` + `cast_error_diag`; refactor the walk to `circuit_returns`** — in `infer.rs`:

First, add `can_cast` to the `ty` import:

```rust
use crate::ty::{Ty, TyKind, can_cast, is_subtype, ty_display};
```

Add the cast-error helpers (near `expr_ty_kind`):

```rust
/// If `cast` is an `src as tgt` the checker can attribute as **illegal**,
/// returns `(src, tgt)` for the diagnostic. The operand's source type must be
/// known — only a primitive-literal operand is typed here (a parameter / local
/// / arbitrary expression types to `Unknown`, and `can_cast(Unknown, _)` is
/// `true`, so it suppresses). The operand is the `CAST_EXPR`'s `Expr` child
/// (the target `Type` child does not cast to `Expr`).
fn cast_error(cast: &compactp_ast::expr::CastExpr) -> Option<(TyKind, TyKind)> {
    let tgt = type_node_kind(&cast.ty()?);
    let operand = cast
        .syntax()
        .children()
        .find_map(compactp_ast::expr::Expr::cast)?;
    let src = expr_ty_kind(&operand);
    (!can_cast(src, tgt)).then_some((src, tgt))
}

/// The illegal-cast diagnostic (`E3002`). Wording tracks compactc's
/// ("cannot cast from type X to type Y"); wording is not gated by the harness.
fn cast_error_diag(
    db: &dyn Db,
    src: TyKind,
    tgt: TyKind,
    span: text_size::TextRange,
) -> Diagnostic {
    let src = ty_display(db, Ty::new(db, src));
    let tgt = ty_display(db, Ty::new(db, tgt));
    Diagnostic::error(
        DiagnosticCode::new("E", 3002),
        format!("cannot cast from type {src} to type {tgt}"),
        span,
    )
}
```

Replace `circuit_return_kinds` with `ReturnTy` + `circuit_returns`:

```rust
/// One `return` operand the checker models: its span, its result type, and —
/// when the operand is an illegal cast the checker can attribute — the
/// `(src, tgt)` of that cast. Both the inference entry point and the
/// diagnostics query read this single per-body walk (preserving the v2b.3
/// one-walk property).
struct ReturnTy {
    span: text_size::TextRange,
    /// Result type of the operand (a cast's result is its target type).
    kind: TyKind,
    /// `Some((src, tgt))` iff the operand is an illegal `src as tgt` cast with
    /// a known (literal) source. The return-lattice check is skipped for such
    /// operands — compactc reports only the cast error.
    cast_error: Option<(TyKind, TyKind)>,
}

/// Types each `return` operand the checker models (primitive literals and
/// casts) in a circuit body. One green-tree walk per consumer.
fn circuit_returns(circuit: &compactp_ast::CircuitDef) -> Vec<ReturnTy> {
    let Some(body) = circuit.body() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for stmt in body.stmts() {
        let compactp_ast::Stmt::Return(ret) = stmt else {
            continue;
        };
        let Some(value) = ret.value() else {
            continue;
        };
        let cast_error = match &value {
            compactp_ast::expr::Expr::Cast(cast) => cast_error(cast),
            _ => None,
        };
        // Only model literals and casts; skip operands the checker can't type.
        if matches!(
            value,
            compactp_ast::expr::Expr::Literal(_) | compactp_ast::expr::Expr::Cast(_)
        ) {
            out.push(ReturnTy {
                span: ret.syntax().text_range(),
                kind: expr_ty_kind(&value),
                cast_error,
            });
        }
    }
    out
}
```

Project `infer_circuit_returns` from `circuit_returns`:

```rust
#[salsa::tracked(returns(clone))]
pub fn infer_circuit_returns(
    db: &dyn Db,
    src: SourceText,
    circuit_index: u32,
) -> Arc<[(text_size::TextRange, TyKind)]> {
    let Some(circuit) = circuit_node_by_index(db, src, circuit_index) else {
        return Arc::from(Vec::new());
    };
    Arc::from(
        circuit_returns(&circuit)
            .into_iter()
            .map(|r| (r.span, r.kind))
            .collect::<Vec<_>>(),
    )
}
```

Rewrite `type_diagnostics_query` to emit cast errors and gate the return check:

```rust
#[salsa::tracked(returns(clone), no_eq)]
pub fn type_diagnostics_query(db: &dyn Db, src: SourceText) -> Arc<[Diagnostic]> {
    let tree = item_tree(db, src);
    let mut diags = Vec::new();
    for (idx, circuit) in circuit_node_map(db, src) {
        let declared = circuit
            .return_type()
            .map(|t| type_node_kind(&t))
            .unwrap_or(TyKind::Unknown);
        let name = &tree.symbols[idx as usize].name;
        for ret in circuit_returns(&circuit) {
            // An illegal cast is reported on its own (compactc reports only the
            // cast error) and suppresses the return-mismatch for that operand —
            // regardless of whether the declared return type is modeled.
            if let Some((s, t)) = ret.cast_error {
                diags.push(cast_error_diag(db, s, t, ret.span));
                continue;
            }
            // Return-mismatch only fires on a return type the checker models.
            if declared != TyKind::Unknown && !is_subtype(ret.kind, declared) {
                diags.push(return_mismatch_diag(db, ret.kind, declared, name, ret.span));
            }
        }
    }
    Arc::from(diags)
}
```

(Note the `declared == Unknown` early-`continue` from v2b.3 is folded into the inline `declared != Unknown` guard so illegal casts are still caught under an unmodeled return type. `name` is now computed unconditionally — cheap, and needed by `return_mismatch_diag`.)

- [ ] **Step 8: Run + verify** — `cargo test -p analyzer-core -q` (all `ty::` and `infer::` tests, including the new cast-error tests and every prior test). Expected: all green. Then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 9: Commit**

```bash
git add crates/analyzer-core/src/ty.rs crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.4): can_cast legality + illegal-cast diagnostic (E3002)

can_cast encodes the extracted relation (Boolean<->Bytes and size-changing
Bytes<->Bytes illegal, all else legal, Unknown suppresses). The shared
return-walk becomes circuit_returns, carrying an illegal-cast attribution;
type_diagnostics_query emits E3002 'cannot cast from type X to type Y' for a
return-position cast with a known (literal) illegal source, and skips the
return-mismatch for it.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 6: Rule-tagged differential fixtures for casts + primitives

Pin the rule against the live compiler: seven fixtures whose expected native verdict was captured from `compactc` accept/reject, added to the existing `type_differential` harness (with its live cross-check). The `corpus_no_false_positives` gate is unchanged and must stay green (self-skips locally, runs in CI with `COMPACT_CORPUS_DIR`).

**Files:**
- Create: `crates/compact-analyzer/tests/fixtures/type/cast_into_wider_uint_ok.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/cast_into_narrower_uint.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/cast_bytes_into_field_ok.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/illegal_cast_bool_to_bytes.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/bytes_return_ok.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/bytes_return_size_mismatch.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/uint_literal_into_bytes.compact`
- Modify: `crates/compact-analyzer/tests/type_differential.rs` (`FIXTURES` table ~47-93)

**Interfaces:**
- Consumes: the existing `rule_tagged_fixtures` runner, `Fixture { name, native_rejects, rule }`, `native_rejects`, `compiler_verdict`.
- Produces: seven new `FIXTURES` rows tagged `"cast-primitives"`.

- [ ] **Step 1: Create the fixtures** (verdicts captured from `compactc 0.31.1` — see *Extracted rule*; all seven were compiled live during planning).

`cast_into_wider_uint_ok.compact` — `compactc` **accepts** (`Uint<8> <: Uint<16>`):
```
export circuit f(x: Field): Uint<16> {
  return x as Uint<8>;
}
```

`cast_into_narrower_uint.compact` — `compactc` **rejects** (`Uint<8>` ⊄ `Uint<0..10>`):
```
export circuit f(x: Field): Uint<0..10> {
  return x as Uint<8>;
}
```

`cast_bytes_into_field_ok.compact` — `compactc` **accepts** (`Bytes as Field` legal; `Field <: Field`):
```
export circuit f(x: Bytes<4>): Field {
  return x as Field;
}
```

`illegal_cast_bool_to_bytes.compact` — `compactc` **rejects** (`Boolean` cannot cast to `Bytes`):
```
export circuit f(): Bytes<32> {
  return true as Bytes<32>;
}
```

`bytes_return_ok.compact` — `compactc` **accepts** (identity cast; `Bytes<32> <: Bytes<32>`):
```
export circuit f(x: Bytes<32>): Bytes<32> {
  return x as Bytes<32>;
}
```

`bytes_return_size_mismatch.compact` — `compactc` **rejects** (legal same-size cast → `Bytes<4>`; `Bytes<4>` ⊄ `Bytes<32>`):
```
export circuit f(x: Bytes<4>): Bytes<32> {
  return x as Bytes<4>;
}
```

`uint_literal_into_bytes.compact` — `compactc` **rejects** (`Uint<0..6>` ⊄ `Bytes<32>`, no cast):
```
export circuit f(): Bytes<32> {
  return 5;
}
```

- [ ] **Step 2: Add the fixtures to the table** — in `type_differential.rs`, extend `FIXTURES` (keep all existing `primitive-literal-return` and `uint-lattice` rows):

```rust
    Fixture { name: "cast_into_wider_uint_ok.compact", native_rejects: false, rule: "cast-primitives" },
    Fixture { name: "cast_into_narrower_uint.compact", native_rejects: true, rule: "cast-primitives" },
    Fixture { name: "cast_bytes_into_field_ok.compact", native_rejects: false, rule: "cast-primitives" },
    Fixture { name: "illegal_cast_bool_to_bytes.compact", native_rejects: true, rule: "cast-primitives" },
    Fixture { name: "bytes_return_ok.compact", native_rejects: false, rule: "cast-primitives" },
    Fixture { name: "bytes_return_size_mismatch.compact", native_rejects: true, rule: "cast-primitives" },
    Fixture { name: "uint_literal_into_bytes.compact", native_rejects: true, rule: "cast-primitives" },
```

- [ ] **Step 3: Run the fixture runner** — `cargo test -p compact-analyzer --test type_differential rule_tagged_fixtures -q`. Expected: PASS. The toolchain is present in this sandbox, so the live cross-check runs for every fixture and each native verdict must match the `compactc` verdict (`RejectPostParse` for `native_rejects: true`, `Accept` for `false`).

- [ ] **Step 4: Run the corpus gate + full suite** — `cargo test -p compact-analyzer --test type_differential -q` (the `corpus_no_false_positives` gate self-skips locally — confirm it prints the SKIP line and passes). Then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`. To exercise the gate against a real corpus: `COMPACT_CORPUS_DIR=/path/to/compactp/tests/corpus cargo test -p compact-analyzer --test type_differential corpus_no_false_positives -q -- --nocapture` (must report `false_positives=0`).

- [ ] **Step 5: Commit**

```bash
git add crates/compact-analyzer/tests/type_differential.rs crates/compact-analyzer/tests/fixtures/type
git commit -m "test(v2b.4): rule-tagged cast + primitive differential fixtures

Seven fixtures pinning the rule against compactc: cast into wider Uint
(accept) / narrower (reject), Bytes cast into Field (accept), illegal
Boolean->Bytes cast (reject), Bytes identity-cast return (accept), Bytes
size mismatch (reject), and Uint literal into Bytes (reject). Live compactc
cross-check runs when the toolchain is present; corpus gate stays green.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage** (rules-index v2b.4 row "Primitives + casts (`as`)" + foundation spec §4/§5 + program spec §5/§9):
- **Primitives** — the `Bytes<n>` primitive modeled in the `Ty` universe → Task 1 (`TyKind::Bytes(u32)` + display), Task 2 (subtype edge), Task 3 (annotation lowering). ✓
- **Casts (`as`)** — cast expressions typed to their target and flowed through the lattice → Task 4 (`expr_ty_kind` + `circuit_returns` modeling `Cast`). ✓
- **Cast rules** — legality (`can_cast`) extracted from the compiler and enforced as a diagnostic → Task 5 (`can_cast` + `E3002`). ✓
- Ground truth = compiler at the pinned toolchain; rule pinned by `compactc` differential **before** implementation → *Extracted rule* (captured first, live-compiled), Task 6 (fixtures + live cross-check). ✓
- Binary corpus gate stays green; no false positives → Global Constraints + conservative `is_subtype`/`can_cast`/`Unknown` lowering + Task 6 Step 4. ✓
- No LSP types in core; `Ty` stays in-crate (display projection only); single-threaded; pre-release → Global Constraints; `Bytes`/`can_cast` are in-crate. ✓
- Editor surfacing deferred to v2b.final → stated; no `server.rs` change. ✓
- Non-literal cast-operand typing, non-return-position casts, and other primitives (Opaque/Vector/Tuple/record) explicitly out of scope, with false-negative-only rationale → Global Constraints (Rule scope). ✓

**2. Placeholder scan:** No "TBD"/"handle edge cases"/"similar to Task N". Every code step carries complete code; every test step shows assertions; every run step gives the command + expected outcome. The note about the `CAST_EXPR` `Expr` child (Task 5 `cast_error`) is a concrete extraction instruction against the CST, verified by the `illegal_cast_from_literal_emits_diagnostic` test, not a placeholder.

**3. Type consistency:** `TyKind::Bytes(u32)` (Task 1) is used with that exact shape in `is_subtype` (Task 2), `bytes_size` (Task 3), `can_cast` (Task 5), and every `Bytes(_)` test. `expr_ty_kind(&Expr) -> TyKind` (Task 4) is consumed by `circuit_return_kinds`/`circuit_returns` (Tasks 4/5) and `cast_error` (Task 5). `can_cast(src: TyKind, tgt: TyKind) -> bool` (Task 5) is consumed by `cast_error` (Task 5). `ReturnTy { span, kind, cast_error }` + `circuit_returns` (Task 5) replace `circuit_return_kinds` and are consumed by both `infer_circuit_returns` and `type_diagnostics_query` (Task 5). `cast_error_diag`/`return_mismatch_diag` both take `(db, …, span)` and return `Diagnostic`. The `Fixture { name, native_rejects, rule }` shape (Task 6) matches the existing harness struct. `parse_int_literal` (v2b.3) is reused unchanged by `bytes_size`.

**Sequencing rationale:** Task 1 adds the `Bytes` kind + display (additive, all green). Task 2 adds its subtype edge while still behavior-preserving (no Bytes flows). Task 3 lowers `Bytes` annotations — the first point Bytes reaches a declared return type, landing the "Uint/wrong literal into Bytes" true-positive. Task 4 types casts to their target, flowing them through the lattice (the headline cast deliverable) while keeping the shared-walk shape. Task 5 adds the distinctive cast-legality rule + `E3002`, refactoring the walk to `circuit_returns` (one walk, both consumers, illegal-cast precision). Task 6 pins the whole rule against the live compiler and confirms the corpus gate. Each task ends with an independently testable, independently reviewable deliverable.
