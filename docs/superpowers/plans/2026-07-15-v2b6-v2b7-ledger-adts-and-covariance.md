# v2b.6 Ledger-ADT Typing + v2b.7 Tuple/Vector Covariance — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the native checker's type universe with tuple/vector types and their covariance (v2b.7), then type the ledger ADT method surfaces and check ledger method-call arguments (v2b.6) — every rule pinned by a live-`compactc` differential fixture and provably free of false positives.

**Architecture:** v2b.7 lands first because v2b.6 reuses its tuple/vector types (`[]` unit and `Vector<n,T>` appear in ledger method signatures). Tuples and vectors are one *sequence* relation in `is_subtype` — `compactc` treats `Vector<n, T>` as sugar for the homogeneous n-element tuple, so both compare by **equal length + element-wise covariance** recursing through the existing `is_subtype`. v2b.6 parses the curated JSON method `sig` strings into typed signatures, replaces the ledger-field ADT name-match with resolution (fixing R1-M7), and adds a resolution-fed query that flags ledger method-call argument-type mismatches (`E3004`), mirroring v2b.5's `generic_diagnostics_query`.

**Tech Stack:** Rust (edition 2024), `salsa = "0.28"`, `compactp_ast`/`compactp_parser`/`compactp_syntax` CST/AST, the M2 resolver (`resolve.rs`, `item_tree.rs`), the v2b type universe (`ty.rs`, `infer.rs`).

## Global Constraints

- **Ground truth is live `compactc` at the pinned toolchain** (compact 0.5.1 / compiler 0.31.1 / language 0.23) — never docs, never training data. Every diagnostic rule is pinned by a differential fixture (native verdict vs `compactc`) that already agrees; a case where native suppresses but `compactc` rejects is a documented false-negative, **not** a fixture.
- **Never a false positive** (hard constraint): for any source `compactc` accepts, native must emit zero type diagnostics. Any uncertainty (unmodeled type, unresolved name, non-literal operand, generic type parameter) suppresses via `TyKind::Unknown` or an early `continue`.
- No LSP types in `analyzer-core`; `Ty`/`TyKind` stay in-crate. Byte offsets only.
- Diagnostic wording tracks `compactc`'s "where practical" but is **not** harness-gated (the harness compares accept/reject only, never wording or spans).
- Pre-release: replace M3's syntactic ADT machinery outright — no dual old/new path (see memory `prerelease-no-back-compat`).
- Analyzer-owned diagnostic codes so far: `E3001` return mismatch, `E3002` cast, `E3003` generic arity. This plan adds **`E3004`** (ledger method-call argument mismatch). Do not reuse a taken number.

## File Structure

- **Modify `crates/analyzer-core/src/ty.rs`** — add composite `TyKind` variants (`Tuple`, `Vector`), change `TyKind` from `Copy` to `Clone`, add the sequence subtyping rule and tuple/vector display. (Task 1)
- **Modify `crates/analyzer-core/src/infer.rs`** — lower `TupleType`/`VectorType` CST nodes and type tuple/array literals; update `Copy`-dependent call sites; merge the new ledger-call diagnostics into `type_diagnostics`. (Tasks 2, 6)
- **Modify `crates/analyzer-core/src/ledger_adts.rs`** — parse method `sig` strings into typed `LedgerMethodSig`; a `LazyLock` typed-surface accessor; resolution-based ADT identification superseding the name-match. (Tasks 4, 5)
- **Modify `crates/analyzer-core/src/db.rs`** — the resolution-fed `ledger_call_diagnostics_query` (`E3004`), mirroring `generic_diagnostics_query`. (Task 6)
- **Modify `crates/analyzer-core/src/resolve.rs`** — the `AnalysisHost::ledger_call_diagnostics` bridge, mirroring `AnalysisHost::generic_diagnostics`. (Task 6)
- **Create fixtures** under `crates/compact-analyzer/tests/fixtures/type/` and **modify `crates/compact-analyzer/tests/type_differential.rs`** — rule-tagged differential fixtures. (Tasks 3, 7)

---

## Task 1: Composite `TyKind` — tuple/vector representation, sequence subtyping, display

**Files:**
- Modify: `crates/analyzer-core/src/ty.rs`
- Modify: `crates/analyzer-core/src/infer.rs` (mechanical: update `Copy`-dependent call sites so the crate compiles)

**Interfaces:**
- Produces:
  - `TyKind` gains `Tuple(std::sync::Arc<[TyKind]>)` and `Vector(u32, std::sync::Arc<TyKind>)`. `TyKind` is now `Clone + PartialEq + Eq + Hash + Debug` but **no longer `Copy`**.
  - `pub(crate) fn is_subtype(sub: &TyKind, sup: &TyKind) -> bool`
  - `pub(crate) fn can_cast(src: &TyKind, tgt: &TyKind) -> bool`
  - `pub(crate) fn display_kind(kind: &TyKind) -> String` (pure; `ty_display` delegates to it)
  - `pub fn ty_display(db: &dyn Db, ty: Ty) -> String` (unchanged signature)
- Consumes: existing `uint_upper_le`, `TyKind::{Boolean,Field,Unknown,Uint,Bytes}`.

**Background — the extracted rule (memory `v2b7-tuple-vector-covariance-rule`):** `compactc` treats `Vector<n, T>` as the homogeneous n-element tuple `[T,…,T]`; they are mutually assignable and covariant across the two kinds. One rule for both: `seq_A <: seq_B` iff equal length AND for every index `i`, `elem_A(i) <: elem_B(i)` (recursing through `is_subtype`). A `Vector<n,T>` presents as `n` elements each of type `T`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/analyzer-core/src/ty.rs`:

```rust
#[test]
fn tuple_and_vector_display() {
    use std::sync::Arc;
    let db = CompactDatabase::default();
    let tup = TyKind::Tuple(Arc::from(vec![TyKind::Uint(Some(256)), TyKind::Boolean]));
    assert_eq!(ty_display(&db, Ty::new(&db, tup)), "[Uint<8>, Boolean]");
    let vec = TyKind::Vector(3, Arc::new(TyKind::Uint(Some(256))));
    assert_eq!(ty_display(&db, Ty::new(&db, vec)), "Vector<3, Uint<8>>");
    // Empty tuple is the unit type.
    assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Tuple(Arc::from(vec![])))), "[]");
}

#[test]
fn sequence_subtyping_matches_compiler() {
    use std::sync::Arc;
    use TyKind::*;
    let u8k = || Uint(Some(256));
    let u16k = || Uint(Some(65536));
    let tup = |v: Vec<TyKind>| Tuple(Arc::from(v));
    let vec = |n, t| Vector(n, Arc::new(t));

    // Element-wise covariance: [Uint8, Uint8] <: [Uint16, Field]
    assert!(is_subtype(&tup(vec![u8k(), u8k()]), &tup(vec![u16k(), Field])));
    // Narrowing an element rejects: [Uint16, Uint16] </: [Uint8, Uint8]
    assert!(!is_subtype(&tup(vec![u16k(), u16k()]), &tup(vec![u8k(), u8k()])));
    // Non-covariant element rejects: [Uint8, Boolean] </: [Uint16, Field]
    assert!(!is_subtype(&tup(vec![u8k(), Boolean]), &tup(vec![u16k(), Field])));
    // Arity mismatch rejects.
    assert!(!is_subtype(&tup(vec![u8k(), u8k()]), &tup(vec![u8k()])));
    // Vector covariance + length: Vector<3,Uint8> <: Vector<3,Uint16>, not Vector<2,_>.
    assert!(is_subtype(&vec(3, u8k()), &vec(3, u16k())));
    assert!(!is_subtype(&vec(3, u16k()), &vec(3, u8k())));
    assert!(!is_subtype(&vec(3, u8k()), &vec(2, u8k())));
    // THE equivalence: Vector<3,Uint8> <-> [Uint8, Uint8, Uint8], both directions,
    // and cross-kind covariance.
    assert!(is_subtype(&vec(3, u8k()), &tup(vec![u8k(), u8k(), u8k()])));
    assert!(is_subtype(&tup(vec![u8k(), u8k(), u8k()]), &vec(3, u8k())));
    assert!(is_subtype(&vec(2, u8k()), &tup(vec![u16k(), Field])));
    assert!(is_subtype(&tup(vec![u8k(), u8k()]), &vec(2, Field)));
    // Heterogeneous tuple into a vector rejects when an element is non-covariant.
    assert!(!is_subtype(&tup(vec![u8k(), Boolean]), &vec(2, Field)));
    // Nested sequences recurse.
    assert!(is_subtype(&vec(2, tup(vec![u8k(), u8k()])), &vec(2, tup(vec![u16k(), Field]))));
    // Unknown element suppresses that position (never a false positive).
    assert!(is_subtype(&tup(vec![Unknown, Boolean]), &tup(vec![u8k(), Boolean])));
    // A sequence is unrelated to a scalar.
    assert!(!is_subtype(&tup(vec![u8k()]), &u8k()));
    assert!(!is_subtype(&u8k(), &vec(1, u8k())));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p analyzer-core --lib ty::`
Expected: FAIL — `TyKind::Tuple`/`Vector` do not exist; `is_subtype` takes `TyKind` by value not `&TyKind`.

- [ ] **Step 3: Change `TyKind` to composite + `Clone`**

In `crates/analyzer-core/src/ty.rs`, replace the `#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]` on `TyKind` with `#[derive(Clone, PartialEq, Eq, Hash, Debug)]` (drop `Copy`) and add the two variants:

```rust
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TyKind {
    Boolean,
    Field,
    Unknown,
    Uint(Option<u128>),
    Bytes(u32),
    /// `[T0, …, Tk]`. An empty tuple is the unit type `[]`. A tuple and a
    /// `Vector` of equal length compare element-wise (they are the same
    /// "sequence" type to the compiler).
    Tuple(Arc<[TyKind]>),
    /// `Vector<n, T>` — presents as `n` elements each of type `T`; equivalent
    /// to the homogeneous n-element tuple. A non-literal or overflowing size
    /// lowers to `Unknown` upstream, so `n` is always a real `u32` here.
    Vector(u32, Arc<TyKind>),
}
```

`#[salsa::interned] Ty` still holds `#[returns(copy)] kind: TyKind`. `returns(copy)` requires the field be `Copy`; since `TyKind` is no longer `Copy`, change that field attribute to `#[returns(clone)]`:

```rust
#[salsa::interned]
#[derive(Debug)]
pub struct Ty {
    #[returns(clone)]
    pub kind: TyKind,
}
```

- [ ] **Step 4: Add the sequence subtyping rule and update `is_subtype`/`can_cast` to borrow**

Replace `is_subtype` and `can_cast` signatures to take `&TyKind`, and add the sequence arm plus helpers:

```rust
pub(crate) fn is_subtype(sub: &TyKind, sup: &TyKind) -> bool {
    match (sub, sup) {
        (TyKind::Unknown, _) | (_, TyKind::Unknown) => true,
        (TyKind::Boolean, TyKind::Boolean) => true,
        (TyKind::Field, TyKind::Field) => true,
        (TyKind::Uint(_), TyKind::Field) => true,
        (TyKind::Uint(a), TyKind::Uint(b)) => uint_upper_le(*a, *b),
        (TyKind::Bytes(a), TyKind::Bytes(b)) => a == b,
        // Tuple & Vector are one sequence relation (all four combinations).
        (
            TyKind::Tuple(_) | TyKind::Vector(_, _),
            TyKind::Tuple(_) | TyKind::Vector(_, _),
        ) => seq_subtype(sub, sup),
        _ => false,
    }
}

/// Number of elements a sequence type presents, or `None` if not a sequence.
fn seq_len(t: &TyKind) -> Option<u32> {
    match t {
        TyKind::Tuple(v) => Some(v.len() as u32),
        TyKind::Vector(n, _) => Some(*n),
        _ => None,
    }
}

/// The element type at index `i` of a sequence (a Vector yields its single
/// element type for every index). Callers only pass `i < seq_len`.
fn seq_elem(t: &TyKind, i: u32) -> &TyKind {
    match t {
        TyKind::Tuple(v) => &v[i as usize],
        TyKind::Vector(_, e) => e,
        _ => unreachable!("seq_elem on non-sequence"),
    }
}

/// Equal length + element-wise covariance, recursing through `is_subtype`.
fn seq_subtype(sub: &TyKind, sup: &TyKind) -> bool {
    match (seq_len(sub), seq_len(sup)) {
        (Some(a), Some(b)) if a == b => {
            (0..a).all(|i| is_subtype(seq_elem(sub, i), seq_elem(sup, i)))
        }
        _ => false,
    }
}
```

`can_cast`: keep the current arms but borrow and dereference the `Bytes` sizes; a composite type falls into the permissive `_ => true` catch-all (cast legality of composites is not modeled — suppress, never a false positive):

```rust
pub(crate) fn can_cast(src: &TyKind, tgt: &TyKind) -> bool {
    match (src, tgt) {
        (TyKind::Unknown, _) | (_, TyKind::Unknown) => true,
        (TyKind::Boolean, TyKind::Bytes(_)) | (TyKind::Bytes(_), TyKind::Boolean) => false,
        (TyKind::Bytes(a), TyKind::Bytes(b)) => a == b,
        _ => true,
    }
}
```

- [ ] **Step 5: Add tuple/vector display via `display_kind`**

Refactor `ty_display` to delegate to a pure `display_kind`, and add the composite arms:

```rust
pub fn ty_display(db: &dyn Db, ty: Ty) -> String {
    display_kind(&ty.kind(db))
}

pub(crate) fn display_kind(kind: &TyKind) -> String {
    match kind {
        TyKind::Boolean => "Boolean".to_string(),
        TyKind::Field => "Field".to_string(),
        TyKind::Unknown => "?".to_string(),
        TyKind::Uint(Some(hi)) => {
            if hi.is_power_of_two() {
                let k = hi.trailing_zeros();
                if (1..=248).contains(&k) {
                    return format!("Uint<{k}>");
                }
            }
            format!("Uint<0..{hi}>")
        }
        TyKind::Uint(None) => "Uint<0..?>".to_string(),
        TyKind::Bytes(n) => format!("Bytes<{n}>"),
        TyKind::Tuple(elems) => {
            let inner = elems.iter().map(display_kind).collect::<Vec<_>>().join(", ");
            format!("[{inner}]")
        }
        TyKind::Vector(n, elem) => format!("Vector<{n}, {}>", display_kind(elem)),
    }
}
```

- [ ] **Step 6: Update `Copy`-dependent call sites in `ty.rs` tests and `infer.rs` so the crate compiles**

In `ty.rs`, the existing tests `subtype_lattice_matches_compiler`, `bytes_subtype_is_same_size_only`, and `can_cast_matches_compiler` call `is_subtype(A, B)` / `can_cast(A, B)` by value. Change each call to borrow: `is_subtype(&A, &B)`, `can_cast(&A, &B)`. (The `TyKind` values there are cheap scalars; just add `&`.)

In `crates/analyzer-core/src/infer.rs`, `TyKind` is used by value in several places relying on `Copy`. Make these compile under `Clone`:
- `is_subtype(ret.kind, declared)` → `is_subtype(&ret.kind, &declared)` (in `type_diagnostics_query`).
- `can_cast(src, tgt)` inside `cast_error` → `can_cast(&src, &tgt)`.
- `return_mismatch_diag`/`cast_error_diag` take `TyKind` by value and call `Ty::new(db, actual)`; `Ty::new` now needs an owned `TyKind` (fine — they own it). Where a caller passes a value it still holds afterward, clone it (e.g. `ret.cast_error` is `Option<(TyKind, TyKind)>` — destructure by value with `if let Some((s, t)) = ret.cast_error` still works because `ReturnTy` is consumed per-iteration; if the borrow checker complains, clone: `if let Some((s, t)) = ret.cast_error.clone()`).
- `ReturnTy.kind` and the `(span, kind)` mapping in `infer_circuit_returns`: `.map(|r| (r.span, r.kind))` moves `r.kind` — fine.

Do **not** change behavior; this step is purely making the existing logic borrow-correct.

- [ ] **Step 7: Run the whole crate's tests**

Run: `cargo test -p analyzer-core --lib`
Expected: PASS — the two new tests pass and every pre-existing `ty::`/`infer::` test still passes (behavior unchanged for scalars; composites are new).

- [ ] **Step 8: Commit**

```bash
git add crates/analyzer-core/src/ty.rs crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.7): composite TyKind — tuple/vector sequence subtyping + display

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 2: Lower tuple/vector CST types and type tuple/array literals

**Files:**
- Modify: `crates/analyzer-core/src/infer.rs`

**Interfaces:**
- Consumes: `TyKind::{Tuple, Vector}` (Task 1); `type_node_kind`, `expr_ty_kind`, `parse_int_literal` (existing).
- Produces: `type_node_kind` now maps `Type::Tuple`/`Type::Vector`; `expr_ty_kind` now types `Expr::Array` (tuple/array literals).
- AST facts (verified): `compactp_ast::TupleType::element_types() -> impl Iterator<Item=Type>`; `compactp_ast::VectorType::size() -> Option<TypeSize>` and `element_type() -> Option<Type>`; `Expr::Array(ArrayExpr)` where element expressions are the `Expr`-castable child nodes; a `Vector<size, type>` size is read via `size().syntax().text()`. `SpreadExpr` (`...x`) is a distinct `Expr` variant — an array containing one lowers to `Unknown` (arity/types indeterminate).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/analyzer-core/src/infer.rs`:

```rust
#[test]
fn tuple_and_vector_annotations_lower() {
    use std::sync::Arc;
    let db = CompactDatabase::default();
    // [Uint<8>, Boolean] -> Tuple([Uint(256), Boolean])
    let a = SourceText::new(&db, Arc::from("export circuit f(): [Uint<8>, Boolean] { }"));
    let ia = circuit_index(&db, a, "f");
    let ca = circuit_node_by_index(&db, a, ia).unwrap();
    assert_eq!(
        type_node_kind(&ca.return_type().unwrap()),
        TyKind::Tuple(Arc::from(vec![TyKind::Uint(Some(256)), TyKind::Boolean]))
    );
    // Vector<3, Uint<8>> -> Vector(3, Uint(256))
    let b = SourceText::new(&db, Arc::from("export circuit f(): Vector<3, Uint<8>> { }"));
    let ib = circuit_index(&db, b, "f");
    let cb = circuit_node_by_index(&db, b, ib).unwrap();
    assert_eq!(
        type_node_kind(&cb.return_type().unwrap()),
        TyKind::Vector(3, Arc::new(TyKind::Uint(Some(256))))
    );
    // Empty tuple -> unit.
    let c = SourceText::new(&db, Arc::from("export circuit f(): [] { }"));
    let ic = circuit_index(&db, c, "f");
    let cc = circuit_node_by_index(&db, c, ic).unwrap();
    assert_eq!(type_node_kind(&cc.return_type().unwrap()), TyKind::Tuple(Arc::from(vec![])));
    // Non-literal vector size -> Unknown (suppresses).
    let d = SourceText::new(&db, Arc::from("export circuit f(): Vector<N, Uint<8>> { }"));
    let id = circuit_index(&db, d, "f");
    let cd = circuit_node_by_index(&db, d, id).unwrap();
    assert_eq!(type_node_kind(&cd.return_type().unwrap()), TyKind::Unknown);
}

#[test]
fn tuple_literal_return_typed_and_checked() {
    use std::sync::Arc;
    let db = CompactDatabase::default();
    // return [1, true] types to Tuple([Uint(2), Boolean]).
    let a = SourceText::new(&db, Arc::from("export circuit f(): [Uint<8>, Boolean] { return [1, true]; }"));
    let ia = circuit_index(&db, a, "f");
    assert_eq!(
        infer_circuit_returns(&db, a, ia)[0].1,
        TyKind::Tuple(Arc::from(vec![TyKind::Uint(Some(2)), TyKind::Boolean]))
    );
    // In-range literal tuple into declared tuple -> accept.
    assert!(type_diagnostics_query(&db, a).is_empty());
    // Tuple literal into a Vector of equal length -> accept (the equivalence).
    let b = SourceText::new(&db, Arc::from("export circuit f(): Vector<3, Uint<8>> { return [1, 2, 3]; }"));
    assert!(type_diagnostics_query(&db, b).is_empty());
    // Element out of range -> reject (300 not <: Uint<8>).
    let c = SourceText::new(&db, Arc::from("export circuit f(): Vector<3, Uint<8>> { return [1, 2, 300]; }"));
    assert_eq!(type_diagnostics_query(&db, c).len(), 1);
    // Arity mismatch -> reject.
    let d = SourceText::new(&db, Arc::from("export circuit f(): [Uint<8>] { return [1, true]; }"));
    assert_eq!(type_diagnostics_query(&db, d).len(), 1);
    // A spread element makes the literal indeterminate -> Unknown -> suppress.
    let e = SourceText::new(&db, Arc::from("export circuit f(x: [Uint<8>, Uint<8>]): [Uint<8>, Uint<8>] { return [...x]; }"));
    assert!(type_diagnostics_query(&db, e).is_empty());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p analyzer-core --lib infer::`
Expected: FAIL — `type_node_kind` returns `Unknown` for tuple/vector; `expr_ty_kind` returns `Unknown` for `Expr::Array`.

- [ ] **Step 3: Lower tuple/vector type nodes**

In `type_node_kind` (in `infer.rs`), add arms before the `_ => TyKind::Unknown` catch-all:

```rust
        Type::Tuple(t) => {
            let elems: Vec<TyKind> = t.element_types().map(|e| type_node_kind(&e)).collect();
            TyKind::Tuple(std::sync::Arc::from(elems))
        }
        Type::Vector(v) => {
            let elem = v
                .element_type()
                .map(|e| type_node_kind(&e))
                .unwrap_or(TyKind::Unknown);
            match v
                .size()
                .and_then(|s| parse_int_literal(&s.syntax().text().to_string()))
            {
                Some(n) if n <= u32::MAX as u128 => {
                    TyKind::Vector(n as u32, std::sync::Arc::new(elem))
                }
                _ => TyKind::Unknown, // non-literal / overflowing size -> suppress
            }
        }
```

(`compactp_ast::Type` and `AstNode` are already imported; `Type::Tuple`/`Type::Vector` are existing variants.)

- [ ] **Step 4: Type tuple/array literals in `expr_ty_kind`**

Add an `Expr::Array` arm to `expr_ty_kind`:

```rust
        Expr::Array(arr) => {
            // A spread element makes arity/types indeterminate -> Unknown.
            if arr
                .syntax()
                .children()
                .any(|c| c.kind() == compactp_syntax::SyntaxKind::SPREAD_EXPR)
            {
                return TyKind::Unknown;
            }
            let elems: Vec<TyKind> = arr
                .syntax()
                .children()
                .filter_map(compactp_ast::expr::Expr::cast)
                .map(|e| expr_ty_kind(&e))
                .collect();
            TyKind::Tuple(std::sync::Arc::from(elems))
        }
```

Then, in `circuit_returns`, the operand-kind guard currently accepts only `Literal | Cast`. Add `Array` so tuple/array literals are modeled:

```rust
        if matches!(
            value,
            compactp_ast::expr::Expr::Literal(_)
                | compactp_ast::expr::Expr::Cast(_)
                | compactp_ast::expr::Expr::Array(_)
        ) {
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core --lib infer::`
Expected: PASS — annotations lower to composites, tuple literals type element-wise, and the sequence lattice accepts/rejects as asserted.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.7): lower tuple/vector types + type array literals

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 3: Tuple/vector differential fixtures

**Files:**
- Create: `crates/compact-analyzer/tests/fixtures/type/vec_covariant_ok.compact` (and 7 more, below)
- Modify: `crates/compact-analyzer/tests/type_differential.rs`

**Interfaces:**
- Consumes: the `FIXTURES` array and `Fixture { name, native_rejects, rule }` struct in `type_differential.rs`; `rule` tag for these is `"tuple-vector-covariance"`.
- Each fixture is a complete `.compact` file whose native verdict (does `host.type_diagnostics(file)` return anything?) equals `native_rejects`, and — when the toolchain is present — whose `compactc` verdict is `Accept` for `native_rejects=false` and `RejectPostParse` for `native_rejects=true`. All values below were captured from live `compactc` 0.31.1.

**Note on fixture form:** the native checker only types **literal** return operands (not parameters), so every fixture returns a literal, matching the probe captures.

- [ ] **Step 1: Create the eight fixture files**

Each file begins with `pragma language_version >= 0.23;` on line 1.

`vec_covariant_ok.compact` (accept — tuple literal into vector, element ranges fit):
```
pragma language_version >= 0.23;
export circuit f(): Vector<3, Uint<8>> { return [1, 2, 3]; }
```
`vec_len_mismatch.compact` (reject — 3 elements into length-2 vector):
```
pragma language_version >= 0.23;
export circuit f(): Vector<2, Uint<8>> { return [1, 2, 3]; }
```
`vec_elem_over_range.compact` (reject — 300 not `<: Uint<8>`):
```
pragma language_version >= 0.23;
export circuit f(): Vector<3, Uint<8>> { return [1, 2, 300]; }
```
`tuple_covariant_ok.compact` (accept — `[Uint, Boolean]` into `[Uint<8>, Boolean]`):
```
pragma language_version >= 0.23;
export circuit f(): [Uint<8>, Boolean] { return [1, true]; }
```
`tuple_arity_mismatch.compact` (reject — 2 elements into a 1-tuple):
```
pragma language_version >= 0.23;
export circuit f(): [Uint<8>] { return [1, true]; }
```
`tuple_elem_mismatch.compact` (reject — `Boolean` not `<: Field`):
```
pragma language_version >= 0.23;
export circuit f(): [Uint<8>, Field] { return [1, true]; }
```
`vec_tuple_equiv_ok.compact` (accept — the vector≡homogeneous-tuple equivalence):
```
pragma language_version >= 0.23;
export circuit f(): [Uint<8>, Uint<8>, Uint<8>] { return [1, 2, 3]; }
```
`nested_seq_ok.compact` (accept — nested vector of vectors):
```
pragma language_version >= 0.23;
export circuit f(): Vector<2, Vector<2, Uint<8>>> { return [[1, 2], [3, 4]]; }
```

- [ ] **Step 2: Register the fixtures in the harness**

In `crates/compact-analyzer/tests/type_differential.rs`, append these entries to the `FIXTURES` array (before the closing `];`):

```rust
    Fixture { name: "vec_covariant_ok.compact", native_rejects: false, rule: "tuple-vector-covariance" },
    Fixture { name: "vec_len_mismatch.compact", native_rejects: true, rule: "tuple-vector-covariance" },
    Fixture { name: "vec_elem_over_range.compact", native_rejects: true, rule: "tuple-vector-covariance" },
    Fixture { name: "tuple_covariant_ok.compact", native_rejects: false, rule: "tuple-vector-covariance" },
    Fixture { name: "tuple_arity_mismatch.compact", native_rejects: true, rule: "tuple-vector-covariance" },
    Fixture { name: "tuple_elem_mismatch.compact", native_rejects: true, rule: "tuple-vector-covariance" },
    Fixture { name: "vec_tuple_equiv_ok.compact", native_rejects: false, rule: "tuple-vector-covariance" },
    Fixture { name: "nested_seq_ok.compact", native_rejects: false, rule: "tuple-vector-covariance" },
```

- [ ] **Step 3: Run the differential harness**

Run: `cargo test -p compact-analyzer --test type_differential rule_tagged_fixtures`
Expected: PASS. If `compactc` is on `PATH`, the live cross-check runs (each new fixture's native verdict must equal its `compactc` verdict); otherwise it prints "compactc absent; skipped live cross-check" and asserts only the native side.

- [ ] **Step 4: Commit**

```bash
git add crates/compact-analyzer/tests/fixtures/type/vec_*.compact \
        crates/compact-analyzer/tests/fixtures/type/tuple_*.compact \
        crates/compact-analyzer/tests/fixtures/type/nested_seq_ok.compact \
        crates/compact-analyzer/tests/type_differential.rs
git commit -m "test(v2b.7): rule-tagged tuple/vector covariance differential fixtures

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 4: Typed ledger method surfaces (`sig` string → `LedgerMethodSig`)

**Files:**
- Modify: `crates/analyzer-core/src/ledger_adts.rs`

**Interfaces:**
- Consumes: `type_node_kind` (from `infer.rs`, `pub(crate)`); `compactp_parser::parse`; `compactp_ast::{CircuitDef, Param, AstNode}`; `TyKind`.
- Produces:
  - `pub struct LedgerMethodSig { pub params: Vec<TyKind>, pub ret: TyKind }`
  - `pub(crate) fn parse_method_sig(sig: &str) -> LedgerMethodSig`
  - `pub(crate) fn ledger_method_sig(adt: &str, method: &str) -> Option<LedgerMethodSig>` — backed by a `std::sync::LazyLock` typed surface built once from the embedded JSON table.

**Approach — synthesize-and-reparse (DRY):** a method `sig` string like `increment(amount: Uint<16>): []` becomes valid Compact by prefixing `circuit ` and appending ` { }`: `circuit increment(amount: Uint<16>): [] { }`. Parse it with `compactp_parser::parse`, read the `CircuitDef`'s `params()` and `return_type()`, and lower each `Type` via `type_node_kind`. Types the universe does not model (bare generic params `T`/`K`/`V`, library types `Maybe<T>`/`MerkleTreeDigest`) lower to `Unknown` automatically — no special-casing.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/analyzer-core/src/ledger_adts.rs`:

```rust
#[test]
fn parse_method_sig_lowers_concrete_and_generic() {
    use crate::ty::TyKind;
    // Concrete param + unit return.
    let s = parse_method_sig("increment(amount: Uint<16>): []");
    assert_eq!(s.params, vec![TyKind::Uint(Some(65536))]);
    assert_eq!(s.ret, TyKind::Tuple(std::sync::Arc::from(vec![])));
    // Concrete return.
    let r = parse_method_sig("read(): Uint<64>");
    assert!(r.params.is_empty());
    assert_eq!(r.ret, TyKind::Uint(Some(1u128 << 64)));
    // Generic param/return -> Unknown (suppresses downstream).
    let g = parse_method_sig("lookup(key: K): V");
    assert_eq!(g.params, vec![TyKind::Unknown]);
    assert_eq!(g.ret, TyKind::Unknown);
    // Bytes param.
    let b = parse_method_sig("insertHash(hash: Bytes<32>): []");
    assert_eq!(b.params, vec![TyKind::Bytes(32)]);
}

#[test]
fn ledger_method_sig_reads_the_table() {
    use crate::ty::TyKind;
    let inc = ledger_method_sig("Counter", "increment").unwrap();
    assert_eq!(inc.params, vec![TyKind::Uint(Some(65536))]);
    // Unknown ADT or method -> None.
    assert!(ledger_method_sig("Counter", "bogus").is_none());
    assert!(ledger_method_sig("Nonsense", "increment").is_none());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p analyzer-core --lib ledger_adts::`
Expected: FAIL — `parse_method_sig` / `ledger_method_sig` / `LedgerMethodSig` do not exist.

- [ ] **Step 3: Implement the typed surfaces**

In `crates/analyzer-core/src/ledger_adts.rs`, add (near the top, after the existing `use` lines):

```rust
use crate::ty::TyKind;
use std::sync::LazyLock;

/// A ledger ADT method's typed signature. Types the universe does not model
/// (generic params `T`/`K`/`V`, library types) are `TyKind::Unknown`.
pub struct LedgerMethodSig {
    pub params: Vec<TyKind>,
    pub ret: TyKind,
}

/// Parse a curated `sig` string (`name(p: T, …): RET`) into a typed signature
/// by synthesizing `circuit <sig> { }`, reparsing, and lowering each `Type`
/// via `type_node_kind`. A malformed sig (no `CircuitDef`) yields an empty,
/// `Unknown`-return signature (suppresses — the asset is authoring-checked).
pub(crate) fn parse_method_sig(sig: &str) -> LedgerMethodSig {
    let source = format!("circuit {sig} {{ }}");
    let result = compactp_parser::parse(&source);
    let root = compactp_syntax::SyntaxNode::new_root(result.green);
    let Some(cd) = root.descendants().filter_map(compactp_ast::CircuitDef::cast).next() else {
        return LedgerMethodSig { params: Vec::new(), ret: TyKind::Unknown };
    };
    let params = cd
        .params()
        .map(|p| {
            p.ty()
                .map(|t| crate::infer::type_node_kind(&t))
                .unwrap_or(TyKind::Unknown)
        })
        .collect();
    let ret = cd
        .return_type()
        .map(|t| crate::infer::type_node_kind(&t))
        .unwrap_or(TyKind::Unknown);
    LedgerMethodSig { params, ret }
}

/// The typed method surface, parsed once from the embedded JSON table:
/// `adt -> method name -> typed signature`.
static SURFACES: LazyLock<
    std::collections::BTreeMap<String, std::collections::BTreeMap<String, LedgerMethodSig>>,
> = LazyLock::new(|| {
    let table = LedgerAdtTable::load();
    table
        .by_adt
        .iter()
        .map(|(adt, methods)| {
            let sigs = methods
                .iter()
                .map(|m| (m.name.clone(), parse_method_sig(&m.sig)))
                .collect();
            (adt.clone(), sigs)
        })
        .collect()
});

/// Typed signature for one ADT method, or `None` if the ADT or method is not
/// in the curated surface. The curated table omits js-only / coin methods, so
/// a `None` here must **suppress**, never emit an "unknown method" diagnostic.
pub(crate) fn ledger_method_sig(adt: &str, method: &str) -> Option<LedgerMethodSig> {
    let s = SURFACES.get(adt)?.get(method)?;
    Some(LedgerMethodSig { params: s.params.clone(), ret: s.ret.clone() })
}
```

Note: `LedgerAdtTable.by_adt` is currently a private field of a `pub(crate)` struct in the same module — it is directly accessible here. `crate::infer::type_node_kind` is `pub(crate)` and in the same crate.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core --lib ledger_adts::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/ledger_adts.rs
git commit -m "feat(v2b.6): typed ledger method surfaces (sig string -> LedgerMethodSig)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 5: Resolution-based ledger-field ADT identification (supersede the name-match, fix R1-M7)

**Files:**
- Modify: `crates/analyzer-core/src/ledger_adts.rs`

**Interfaces:**
- Consumes: `AnalysisHost::resolve` (existing, `resolve.rs`), `crate::Definition`, `crate::SymbolKind`, `crate::FilePosition`.
- Produces: `AnalysisHost::ledger_field_adt` unchanged in signature (`fn ledger_field_adt(&mut self, def: &crate::Definition) -> Option<String>`) but its head-name → ADT decision now resolves the head to distinguish a user-defined type (→ implicit `Cell`) from a builtin ADT.

**Background — the extracted rule (memory `v2b6-ledger-adt-typing-rule`):** ledger ADTs require `import CompactStandardLibrary;`. With the import present, a user `struct Map` is a duplicate-binding compile error, so the R1-M7 name-match hazard only bites in the **no-import** case, where a name like `Map` can only be the user type. The builtin ADT names are **not** in the file/workspace item tree (the standard library is not indexed), so they do not resolve. Therefore: **if the head name resolves to a user-defined struct/enum/type-alias, it is that user type (implicit `Cell`); otherwise apply the existing `KNOWN`-name check.**

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/analyzer-core/src/ledger_adts.rs`:

```rust
#[test]
fn user_struct_named_like_builtin_is_cell_not_builtin() {
    // R1-M7 fix: without the stdlib import, `struct Map` is the user type, so a
    // `ledger m: Map` field must map to implicit Cell — NOT the builtin Map ADT.
    let mut host = crate::AnalysisHost::new();
    let file = host.vfs_mut().file_id(std::path::Path::new("/t/shadow.compact"));
    host.vfs_mut().set_overlay(
        file,
        "struct Map { x: Field; }\nexport ledger m: Map;\n".to_string(),
        1,
    );
    let tree = host.analyze(file).unwrap().item_tree.clone();
    let idx = tree.symbols.iter().position(|s| s.name == "m").unwrap();
    assert_eq!(
        host.ledger_field_adt(&crate::Definition::Item { file, index: idx as u32 }),
        Some("Cell".to_string())
    );
}
```

Also confirm the existing `ledger_field_adt_generic_head_maps_to_declared_adt` test (which uses `ledger m: Map<Field, Field>` with **no** user `Map` and **no** import, expecting `"Map"`) still passes — under the new logic `Map` does not resolve to a user type, so the `KNOWN` check yields `"Map"`. Do not modify that test.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p analyzer-core --lib ledger_adts::user_struct_named_like_builtin_is_cell_not_builtin`
Expected: FAIL — the current name-match maps `Map` to the builtin, returning `Some("Map")`.

- [ ] **Step 3: Make the ADT decision resolution-based**

In `AnalysisHost::ledger_field_adt` (in `ledger_adts.rs`), replace the final `KNOWN`-based decision. After computing `head` (the type head name string) and its source range, resolve the head and check whether it lands on a user-defined type. Replace the block from `const KNOWN` through the returned `Some(...)` with:

```rust
        // Resolve the type head. If it resolves to a user-defined named type
        // (struct / enum / type alias), this field is that user type -> implicit
        // Cell. Builtin ADT names (Counter/Map/…) are not indexed, so they do
        // not resolve, and fall through to the KNOWN-name check below. This
        // supersedes the pure name-match (fixes R1-M7).
        let head_start = head_range.start();
        let resolves_to_user_type = matches!(
            self.resolve(crate::FilePosition { file: *file, offset: head_start }),
            Some(crate::Definition::Item { file: df, index: di })
                if self
                    .analyze(df)
                    .and_then(|a| a.item_tree.symbols.get(di as usize).map(|s| s.kind))
                    .is_some_and(|k| matches!(
                        k,
                        crate::SymbolKind::Struct
                            | crate::SymbolKind::Enum
                            | crate::SymbolKind::TypeAlias
                    ))
        );
        if resolves_to_user_type {
            return Some("Cell".to_string());
        }
        const KNOWN: &[&str] = &[
            "Counter", "Cell", "Map", "Set", "List",
            "MerkleTree", "HistoricMerkleTree", "Kernel",
        ];
        Some(if KNOWN.contains(&head.as_str()) {
            head
        } else {
            "Cell".to_string()
        })
```

You will need the head's source range. The `head` is read from `ledger.ty()` as `Type::Ref(r)` — capture `r.name()`'s `text_range()` alongside its text. Adjust the earlier `let head = match ledger.ty()? { ... }` so the `Type::Ref` arm binds both the name string and a `head_range: TextRange`; for the non-`Ref` (builtin scalar) arm, keep the early `return Some("Cell".to_string())`. Concretely, change that match to:

```rust
        let (head, head_range) = match ledger.ty()? {
            compactp_ast::Type::Ref(r) => {
                let tok = r.name()?;
                (tok.text().to_string(), tok.text_range())
            }
            _ => return Some("Cell".to_string()), // builtin scalar type -> Cell
        };
```

Remove the stale `R1-M7 (accepted limitation)` comment block that documented the old name-match hazard, since it is now fixed.

- [ ] **Step 4: Run the ledger tests to verify they pass**

Run: `cargo test -p analyzer-core --lib ledger_adts::`
Expected: PASS — the new shadow test passes and all pre-existing `ledger_adts::` tests still pass (implicit-Cell, generic head → `Map`, local/non-ledger → `None`, user struct → `Cell`).

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/ledger_adts.rs
git commit -m "feat(v2b.6): resolution-based ledger-field ADT id (supersede name-match, fix R1-M7)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 6: Ledger method-call argument type check (`E3004`)

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (the tracked query)
- Modify: `crates/analyzer-core/src/resolve.rs` (the host bridge)
- Modify: `crates/analyzer-core/src/infer.rs` (merge into `type_diagnostics`)

**Interfaces:**
- Consumes: `resolve_query` (`db.rs`), `item_tree`/`parsed` (`db.rs`), `ledger_method_sig` (Task 4), the resolution-based ADT decision (Task 5), `is_subtype`/`display_kind` (`ty.rs`), `expr_ty_kind`/`type_node_kind` (`infer.rs`, both `pub(crate)`), `FileDeps`/`Workspace` inputs (as `generic_diagnostics_query` uses them).
- Produces:
  - `db.rs`: `pub fn ledger_call_diagnostics_query(db: &dyn Db, file: FileId, src: SourceText, fd: FileDeps, ws: Workspace) -> Arc<[Diagnostic]>`
  - `resolve.rs`: `pub fn ledger_call_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic>`
  - `infer.rs`: `AnalysisHost::type_diagnostics` also extends with `self.ledger_call_diagnostics(file)`.

**The rule (verified against live `compactc`):** for a method call `recv.method(args)` where `recv` resolves to a ledger field whose ADT is a builtin, `method` is in that ADT's typed surface, an argument `args[i]` is a **typeable literal/cast** (via `expr_ty_kind`), and the corresponding parameter type is **concretely modeled** (not `Unknown`): if `!is_subtype(arg_i, param_i)` emit `E3004`. Every other situation suppresses. This is a strict subset of what `compactc` checks — generic-parameter methods (`Cell`/`Map`/`Set`/`List` with `T`/`K`/`V`) have `Unknown` params and are skipped (a safe false-negative; `compactc` substitutes the field's type, native does not — deferred). No "unknown method" diagnostic (the curated surface omits js-only/coin methods).

**CST facts (verified):** `a.b(c, d)` parses to `CALL_EXPR` with children in order: receiver `NAME_EXPR` (the `Expr` before the `DOT` token), `DOT` token, method-name `IDENT` (direct token child, returned by `CallExpr::name()`), `L_PAREN`, argument `Expr`/`NAMED_ARG` children, `R_PAREN`. A plain call `foo(x)` has **no** `DOT` child. Named args (`field = 42`) are `NAMED_ARG` nodes that do not cast to `Expr` and are skipped. The receiver is the `Expr` child whose range ends at/before the `DOT`; positional args are the `Expr` children after `L_PAREN`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/analyzer-core/src/infer.rs` (the merged surface is `AnalysisHost::type_diagnostics`):

```rust
#[test]
fn ledger_call_arg_mismatch_emits_e3004() {
    use crate::AnalysisHost;
    use std::path::Path;
    let mut host = AnalysisHost::new();
    let file = host.vfs_mut().file_id(Path::new("/t/lc.compact"));
    host.vfs_mut().set_overlay(
        file,
        "pragma language_version >= 0.23;\nimport CompactStandardLibrary;\n\
         export ledger c: Counter;\nexport circuit f(): [] { c.increment(true); }".to_string(),
        1,
    );
    let diags = host.type_diagnostics(file);
    assert!(
        diags.iter().any(|d| d.code.number == 3004),
        "expected E3004, got {:?}",
        diags.iter().map(|d| d.code.number).collect::<Vec<_>>()
    );
}

#[test]
fn ledger_call_good_arg_and_generic_method_suppress() {
    use crate::AnalysisHost;
    use std::path::Path;
    // Correct arg type -> no diagnostic.
    let mut host = AnalysisHost::new();
    let file = host.vfs_mut().file_id(Path::new("/t/ok.compact"));
    host.vfs_mut().set_overlay(
        file,
        "pragma language_version >= 0.23;\nimport CompactStandardLibrary;\n\
         export ledger c: Counter;\nexport circuit f(): [] { c.increment(1); }".to_string(),
        1,
    );
    assert!(host.type_diagnostics(file).iter().all(|d| d.code.number != 3004));

    // Generic-parameter method (Map.insert with K/V) -> params Unknown -> suppress,
    // even with a literal that would mismatch a concrete type.
    let mut host2 = AnalysisHost::new();
    let file2 = host2.vfs_mut().file_id(Path::new("/t/gen.compact"));
    host2.vfs_mut().set_overlay(
        file2,
        "pragma language_version >= 0.23;\nimport CompactStandardLibrary;\n\
         export ledger m: Map<Uint<8>, Boolean>;\nexport circuit f(): [] { m.insert(1, true); }".to_string(),
        1,
    );
    assert!(host2.type_diagnostics(file2).iter().all(|d| d.code.number != 3004));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p analyzer-core --lib infer::ledger_call`
Expected: FAIL — `E3004` is never emitted (`type_diagnostics` does not yet consult ledger calls).

- [ ] **Step 3: Implement the tracked query in `db.rs`**

Add to `crates/analyzer-core/src/db.rs`, mirroring `generic_diagnostics_query`. Reuse `item_symbol` (already in `db.rs`) and `resolve_query`. Add a small `E3004` diagnostic constructor and a receiver→ADT helper:

```rust
/// The ledger-call argument-mismatch diagnostic (`E3004`). Wording tracks
/// compactc's ("expected first argument of M to have type T but received U");
/// wording is not gated by the harness.
fn ledger_arg_diag(
    pos: usize,
    method: &str,
    expected: &str,
    actual: &str,
    span: text_size::TextRange,
) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 3004),
        format!(
            "expected argument {} of {method} to have type {expected} but received {actual}",
            pos + 1
        ),
        span,
    )
}

/// If the identifier at `offset` resolves to a ledger field, its builtin ADT
/// key (Counter/Map/…), or `None` when it is not a ledger field or its declared
/// type resolves to a user-defined type (implicit Cell / not a builtin). Pure
/// resolution-context twin of `AnalysisHost::ledger_field_adt`.
fn ledger_adt_at(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    offset: text_size::TextSize,
) -> Option<String> {
    let def = resolve_query(db, file, src, fd, ws, offset)?;
    let crate::Definition::Item { file: df, index } = def else {
        return None;
    };
    let sym = item_symbol(db, file, src, fd, df, index)?;
    if sym.kind != crate::SymbolKind::Ledger {
        return None;
    }
    // Read the ledger field's declared type head from its own file.
    let (_, dsrc) = if df == file {
        (df, src)
    } else {
        fd.deps(db).iter().filter_map(|d| d.target).find(|(f, _)| *f == df)?
    };
    let green = crate::db::parsed(db, dsrc).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let ledger = root
        .descendants()
        .filter_map(compactp_ast::LedgerDecl::cast)
        .find(|d| d.name().is_some_and(|t| t.text_range() == sym.name_range))?;
    let head_tok = match ledger.ty()? {
        compactp_ast::Type::Ref(r) => r.name()?,
        _ => return None, // scalar-typed -> implicit Cell (no builtin ADT methods checked)
    };
    // If the head resolves to a user-defined named type, it is not a builtin ADT.
    let head_off = head_tok.text_range().start();
    if let Some(crate::Definition::Item { file: hf, index: hi }) =
        resolve_query(db, df, dsrc, fd, ws, head_off)
    {
        if let Some(hs) = item_symbol(db, df, dsrc, fd, hf, hi) {
            if matches!(
                hs.kind,
                crate::SymbolKind::Struct | crate::SymbolKind::Enum | crate::SymbolKind::TypeAlias
            ) {
                return None;
            }
        }
    }
    const KNOWN: &[&str] = &[
        "Counter", "Cell", "Map", "Set", "List",
        "MerkleTree", "HistoricMerkleTree", "Kernel",
    ];
    let head = head_tok.text().to_string();
    KNOWN.contains(&head.as_str()).then_some(head)
}

/// Ledger method-call argument type checks (`E3004`) for `src`. For each method
/// call `recv.method(args)` whose receiver resolves to a builtin-ADT ledger
/// field and whose `method` is in that ADT's typed surface, any positional
/// argument that is a typeable literal/cast is checked against a concretely
/// modeled parameter type. Suppresses on every uncertainty (never a false
/// positive). Resolution-fed like `generic_diagnostics_query`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn ledger_call_diagnostics_query(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
) -> Arc<[Diagnostic]> {
    use compactp_ast::AstNode;
    let green = crate::db::parsed(db, src).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let mut diags = Vec::new();

    for call in root.descendants().filter_map(compactp_ast::expr::CallExpr::cast) {
        // A method call has a DOT token child; a plain call does not.
        let Some(dot) = call
            .syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == compactp_syntax::SyntaxKind::DOT)
        else {
            continue;
        };
        let dot_start = dot.text_range().start();
        // Receiver: the Expr child ending at/before the DOT. Method name: CallExpr::name().
        let Some(recv) = call
            .syntax()
            .children()
            .filter_map(compactp_ast::expr::Expr::cast)
            .find(|e| e.syntax().text_range().end() <= dot_start)
        else {
            continue;
        };
        // Only a bare NAME_EXPR receiver is identified; anything else -> suppress.
        if recv.syntax().kind() != compactp_syntax::SyntaxKind::NAME_EXPR {
            continue;
        }
        let recv_off = recv.syntax().text_range().start();
        let Some(method_tok) = call.name() else { continue };
        let method = method_tok.text().to_string();

        let Some(adt) = ledger_adt_at(db, file, src, fd, ws, recv_off) else { continue };
        let Some(sig) = crate::ledger_adts::ledger_method_sig(&adt, &method) else { continue };

        // Positional argument Exprs after the L_PAREN (skip NAMED_ARG etc.).
        let lparen_start = call
            .syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == compactp_syntax::SyntaxKind::L_PAREN)
            .map(|t| t.text_range().start());
        let Some(lparen_start) = lparen_start else { continue };
        let args: Vec<compactp_ast::expr::Expr> = call
            .syntax()
            .children()
            .filter_map(compactp_ast::expr::Expr::cast)
            .filter(|e| e.syntax().text_range().start() >= lparen_start)
            .collect();

        for (i, arg) in args.iter().enumerate() {
            let Some(param) = sig.params.get(i) else { break };
            if *param == crate::ty::TyKind::Unknown {
                continue; // generic / unmodeled param -> suppress
            }
            let actual = crate::infer::expr_ty_kind(arg);
            if actual == crate::ty::TyKind::Unknown {
                continue; // non-literal/cast arg -> suppress
            }
            if !crate::ty::is_subtype(&actual, param) {
                diags.push(ledger_arg_diag(
                    i,
                    &method,
                    &crate::ty::display_kind(param),
                    &crate::ty::display_kind(&actual),
                    arg.syntax().text_range(),
                ));
            }
        }
    }
    Arc::from(diags)
}
```

You must make the helpers `expr_ty_kind`, `is_subtype`, and `display_kind` reachable from `db.rs`:
- `crate::infer::expr_ty_kind` — change `fn expr_ty_kind` in `infer.rs` from private to `pub(crate)`.
- `crate::ty::is_subtype` and `crate::ty::display_kind` — already `pub(crate)`.
- `crate::ledger_adts::ledger_method_sig` — `pub(crate)` (Task 4).
- `compactp_ast::expr::CallExpr` and `LedgerDecl`/`Type` — confirm the imports/paths compile (`CallExpr` lives in `compactp_ast::expr`; `LedgerDecl` and `Type` in `compactp_ast`).

- [ ] **Step 4: Add the host bridge in `resolve.rs`**

After `AnalysisHost::generic_diagnostics` in `crates/analyzer-core/src/resolve.rs`, add the parallel bridge:

```rust
    /// Ledger method-call argument type diagnostics for `file` (`E3004`).
    /// Resolution-fed (receiver + type head resolution), so it indexes the file
    /// to materialize dependency edges, then calls the tracked
    /// `ledger_call_diagnostics_query`. Merged into `type_diagnostics` (v2b.6).
    pub fn ledger_call_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        self.ensure_indexed(file);
        let fd = self.file_deps(file);
        let ws = self.workspace();
        crate::db::ledger_call_diagnostics_query(self.db_ref(), file, src, fd, ws).to_vec()
    }
```

- [ ] **Step 5: Merge into `type_diagnostics`**

In `crates/analyzer-core/src/infer.rs`, `AnalysisHost::type_diagnostics`, add the ledger-call diagnostics after the generic ones:

```rust
        diags.extend(self.generic_diagnostics(file));
        diags.extend(self.ledger_call_diagnostics(file));
        diags
```

Update the method's doc comment to note it now also merges ledger method-call argument checks.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p analyzer-core --lib`
Expected: PASS — `E3004` fires on `c.increment(true)`, suppresses on `c.increment(1)` and on the generic `Map.insert` call, and all prior tests are green.

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/resolve.rs crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.6): ledger method-call argument type check (E3004)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 7: Ledger method-call differential fixtures

**Files:**
- Create: `crates/compact-analyzer/tests/fixtures/type/ledger_arg_ok.compact` (and 3 more, below)
- Modify: `crates/compact-analyzer/tests/type_differential.rs`

**Interfaces:**
- Consumes: the `FIXTURES` array; `rule` tag `"ledger-adt-typing"`.
- All verdicts captured from live `compactc` 0.31.1. Fixtures require `import CompactStandardLibrary;`.

- [ ] **Step 1: Create the four fixture files**

`ledger_arg_ok.compact` (accept — correct arg type):
```
pragma language_version >= 0.23;
import CompactStandardLibrary;
export ledger c: Counter;
export circuit f(): [] { c.increment(1); }
```
`ledger_arg_mismatch.compact` (reject — `Boolean` into `Uint<16>`, `E3004`):
```
pragma language_version >= 0.23;
import CompactStandardLibrary;
export ledger c: Counter;
export circuit f(): [] { c.increment(true); }
```
`ledger_decrement_mismatch.compact` (reject — second concrete method):
```
pragma language_version >= 0.23;
import CompactStandardLibrary;
export ledger c: Counter;
export circuit f(): [] { c.decrement(true); }
```
`ledger_generic_method_ok.compact` (accept — generic `Map.insert`; native suppresses, `compactc` accepts — proves no false positive on generic-param methods):
```
pragma language_version >= 0.23;
import CompactStandardLibrary;
export ledger m: Map<Uint<8>, Boolean>;
export circuit f(): [] { m.insert(1, true); }
```

- [ ] **Step 2: Register the fixtures in the harness**

Append to the `FIXTURES` array in `crates/compact-analyzer/tests/type_differential.rs`:

```rust
    Fixture { name: "ledger_arg_ok.compact", native_rejects: false, rule: "ledger-adt-typing" },
    Fixture { name: "ledger_arg_mismatch.compact", native_rejects: true, rule: "ledger-adt-typing" },
    Fixture { name: "ledger_decrement_mismatch.compact", native_rejects: true, rule: "ledger-adt-typing" },
    Fixture { name: "ledger_generic_method_ok.compact", native_rejects: false, rule: "ledger-adt-typing" },
```

- [ ] **Step 3: Run the differential harness**

Run: `cargo test -p compact-analyzer --test type_differential rule_tagged_fixtures`
Expected: PASS. With `compactc` present, each ledger fixture's native verdict matches its `compactc` verdict (accept/reject); the generic-method fixture confirms native does not false-positive where `compactc` accepts.

- [ ] **Step 4: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS — all crates green, including the `corpus_no_false_positives` gate (which self-skips if `COMPACT_CORPUS_DIR` and the `../compactp` checkout are both absent — note this skip in the final review).

- [ ] **Step 5: Commit**

```bash
git add crates/compact-analyzer/tests/fixtures/type/ledger_*.compact \
        crates/compact-analyzer/tests/type_differential.rs
git commit -m "test(v2b.6): rule-tagged ledger method-call differential fixtures

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Deferred (documented safe false-negatives, not gaps to fix now)

These are cases where `compactc` rejects but native suppresses — safe (never a false positive), and consistent with the incremental approach (v2b.5 likewise deferred call-site generic checking):

- **Ledger method-call return typing.** `return c.read()` into a wrong return type — `compactc` flows the method return type into the return lattice; native's return lattice is single-file (no receiver resolution), so it does not type method-call returns. Would require making the return lattice resolution-fed.
- **Generic-parameter substitution.** `Cell.write`/`Map.insert`/etc. with `T`/`K`/`V` params — `compactc` substitutes the field's declared type args (e.g. `bal: Uint<64>` makes `write` expect `Uint<64>`); native leaves them `Unknown` and suppresses.
- **Unknown-method detection.** `c.bogusMethod()` — `compactc` rejects; native cannot, because the curated surface intentionally omits js-only/coin methods, so a "method not found" diagnostic would false-positive on real omitted methods.
- **Cast legality of composites.** `can_cast` returns `true` for any pair involving a tuple/vector (permissive suppression).

## Follow-ups (operational, not code)

- Run the `corpus_no_false_positives` gate with a real `COMPACT_CORPUS_DIR` before release (it self-skips here). This is the primary guard that the new `E3004` and sequence lattice never fire on a `compactc`-accepted file.

---

## Self-Review

**1. Spec/rule coverage.**
- v2b.7 tuple/vector covariance — Tasks 1 (subtyping/display), 2 (lowering/literals), 3 (fixtures). The `Vector<n,T>` ≡ homogeneous-tuple equivalence is covered by the cross-kind arm and `vec_tuple_equiv_ok.compact`. ✓
- v2b.6 typed method surfaces — Task 4. ✓
- v2b.6 supersede `ledger_field_adt` name-match (R1-M7) — Task 5. ✓
- v2b.6 ledger method-call type check (the observable rule with fixtures) — Tasks 6, 7. ✓
- No false positives — every diagnostic path suppresses on `Unknown`/unresolved/non-literal/generic; Task 7 Step 4 runs the corpus gate. ✓

**2. Placeholder scan.** No "TBD"/"handle edge cases"/"similar to Task N"; every code step shows complete code; every test step shows the assertions and the exact `cargo` command. ✓

**3. Type consistency.** `LedgerMethodSig { params: Vec<TyKind>, ret: TyKind }`, `parse_method_sig`, `ledger_method_sig`, `ledger_call_diagnostics_query`/`ledger_call_diagnostics`, `is_subtype(&TyKind, &TyKind)`, `can_cast(&TyKind, &TyKind)`, `display_kind(&TyKind)`, `expr_ty_kind` (now `pub(crate)`), `TyKind::Tuple(Arc<[TyKind]>)`/`Vector(u32, Arc<TyKind>)` — used consistently across Tasks 1, 4, 6. `Uint<64>` exclusive upper is `1u128 << 64` (Task 4 test), matching `uint_bound`'s `k < 128 => Some(1 << k)`. ✓
