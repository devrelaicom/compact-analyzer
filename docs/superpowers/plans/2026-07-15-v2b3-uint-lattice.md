# v2b.3 — `Uint<0..n)` Subtype Lattice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the first distinctive type rule on the v2b.2 spine — Compact's bounded-integer `Uint` type, its numeric-literal typing, and its range-containment subtype lattice — so the native checker agrees with `compactc` on `Uint`/`Field`/`Boolean` return-position assignability.

**Architecture:** Extend the interned `Ty` universe with a `Uint` kind carrying its exclusive upper bound; type integer literals to their minimal `Uint`; replace the foundation's equality-based return check with a real subtype lattice (`is_subtype`). All single-file, over v2b.2's `item_tree`/`parsed` inference entry point — no name resolution. The rule is pinned by live-`compactc` rule-tagged differential fixtures and stays inside the v2b.2 no-false-positive corpus gate.

**Tech Stack:** Rust 2024, salsa 0.28 (`#[salsa::interned]`, `#[salsa::tracked]`), `compactp_ast`/`compactp_syntax` (external CST), the v2b.2 `analyzer-toolchain` differential harness.

## Global Constraints

Every task's requirements implicitly include this section.

- **Verify-first.** The compiler at the pinned toolchain (`compact 0.5.1` / compiler `0.31.1` / language `0.23.0`) is ground truth — never docs, never training data. The lattice was extracted empirically from the live compiler's own diagnostics (which print its internal type representation directly) **before** any implementation; the exact rule and every accept/reject verdict are recorded in the *Extracted rule* section below, and re-asserted by the rule-tagged fixtures (Task 6).
- **No LSP types in `analyzer-core`;** byte offsets (`text_size`) only. `Ty` does **not** escape `analyzer-core` — the IDE layer gets the `ty_display` `String` projection.
- **Single-threaded**, cooperative `should_continue`; no salsa cross-thread `Cancelled`.
- **Diagnostics are tracked queries returning `Arc<[Diagnostic]>`** (not accumulators); the `no_eq` diagnostics precedent (`resolution_diagnostics_query`, `type_diagnostics_query`) is unchanged here.
- **Never a false positive.** For any source `compactc` accepts, the native checker must emit zero type diagnostics. Whenever a `Uint` bound is not exactly representable (see *Representation*), the lattice check resolves **conservatively** (treated as a subtype → no diagnostic). The v2b.2 binary corpus gate (`corpus_no_false_positives`) must stay green.
- **Pre-release.** No back-compat shims/migrations/dual code paths — clean wholesale replacement (`prerelease-no-back-compat`). Foundation tests that encoded pre-`Uint` placeholder behavior are **updated in place** to the correct behavior (Tasks 4–5); this is expected, recorded drift, not a regression.
- **Never commit `[patch.crates-io]`.**
- **Editor integration is out of scope here.** Surfacing type diagnostics in VS Code + the native-vs-compiler toggle remains **v2b.final**. This plan only broadens what `AnalysisHost::type_diagnostics` reports.
- **Rule scope is the lattice, not everything `Uint`.** In scope: modeling the `Uint` type from annotations, typing integer literals, and the range-containment subtype relation applied at the return position. **Out of scope (deliberate, recorded):** `Uint` *well-formedness* diagnostics (width `1..=248`, range start `== 0`, range end `>= 1`) — a malformed `Uint` annotation is typed `Unknown` (suppresses the rule; corpus-safe because accepted files contain no malformed `Uint`s); arithmetic/expression operand typing beyond literals (needs full expression inference); and parameter/local reference typing (needs resolution). Literal operands alone exercise the full containment lattice, so none of these are required to land the rule.

## Extracted rule (ground truth — `compact 0.5.1` / compiler `0.31.1`, verified 2026-07-15)

Captured empirically from the live compiler; its mismatch messages print its internal type representation (`Uint<0..257>`, normalized `Uint<8>`), so representation *and* verdicts are both direct observations. Every verdict below is reproduced by a Task-6 fixture.

- **Representation.** A `Uint` type is a half-open range `[0, hi)`, `hi >= 1`. Two surface forms denote the *same* internal type:
  - `Uint<k>` (bit-width): `[0, 2^k)`; valid `1 <= k <= 248`.
  - `Uint<lo..hi>` (range): `[lo, hi)`; valid `lo == 0` and `hi >= 1` (`hi` exclusive).
  Canonical parameter = the **exclusive upper bound** `hi`. `Uint<0..256>` ≡ `Uint<8>`.
- **Literal typing.** Integer literal `n` (decimal `INT_LIT` or hex `HEX_LIT`) has type `Uint<0..n+1>` — the minimal range `[0, n]`. Evidence: `5`→`Uint<0..6>`, `256`→`Uint<0..257>`, `0`→`Uint<0..1>`, `999`→`Uint<0..1000>`. `true`/`false` stay `Boolean`.
- **Subtype lattice** (widening allowed, narrowing rejected):
  - `Uint<0..a> <: Uint<0..b>` iff `a <= b` (range containment; all lows are 0 → a total order on the upper bound).
  - `Uint<0..a> <: Field` for every `a`. `Field` is **not** a subtype of any `Uint`.
  - `Boolean` is isolated: `Boolean` and `Field` are mutually non-assignable; `Boolean` and any `Uint` are mutually non-assignable.
  - Reflexive within a kind.
- **Display / normalization.** `Uint<0..hi>` renders as `Uint<k>` iff `hi == 2^k` (`1 <= k <= 248`), else as `Uint<0..hi>`.
- **Mismatch diagnostic** (same family as the foundation, `E3001`): `mismatch between actual return type {A} and declared return type {D} of circuit {name}`, `A`/`D` in normalized display. Wording tracked "where practical", **not** harness-gated.

**Representation design (no bignum dependency; conservative on overflow).** Bounds reach `2^248`, which does not fit `u128`. Model the exclusive upper bound as `Option<u128>` where `None` means "≥ 2^128, not exactly representable". The subtype comparison is defined so it is **exact whenever both bounds fit `u128`** (all realistic code, all fixtures) and **never a false positive** otherwise:

| actual bound | declared bound | `actual <: declared`? | why |
|---|---|---|---|
| `Some(a)` | `Some(d)` | `a <= d` | exact |
| `Some(a)` | `None` | `true` | concrete `< 2^128 <=` huge declared |
| `None` | `Some(d)` | `false` | huge actual `>= 2^128 > d`; a **correct** reject (only ever coincides with a `compactc` reject) |
| `None` | `None` | `true` | undecidable → conservative accept (possible false *negative*, never a false positive) |

Bit-width `Uint<k>` → bound `Some(2^k)` if `k < 128`, else `None`. Integer literal `n` → bound `Some(n+1)` if it fits `u128`, else `None`. A `TypeSize` that is an identifier (a generic numeric parameter, not a literal) → the whole `Uint` is `Unknown`.

## Drift reconciliation (checked against as-merged v2b.2, commit `4e88e87`)

The v2b.3 outline (rules-index row + foundation spec) was checked against the merged v2b.2 code. Findings folded into this plan:

1. **`ty/` module vs `ty.rs`.** The rules index sketched a "new `ty/` lattice module"; v2b.2 shipped a single `crates/analyzer-core/src/ty.rs`. Decision: keep the single file and add the lattice (`is_subtype`) there. Splitting into a `ty/` directory is unwarranted for one function (YAGNI); revisit when covariance/generics land.
2. **Foundation tests encode pre-`Uint` placeholder behavior and must be updated.** Two `infer.rs` tests assert the *deliberate* foundation false-negatives that this rule corrects:
   - `non_primitive_literal_return_is_unknown` — asserts `return 1` types as `TyKind::Unknown`. After Task 5 it is `TyKind::Uint(Some(2))`. **Updated in Task 5.**
   - `unknown_operand_or_expectation_suppresses` — asserts `Uint<0..7> { return true; }` and `Boolean { return 1; }` each emit **no** diagnostic. `compactc` **rejects both** (`Uint<8> return true` and `Boolean return 5` are TYPE_REJECTs). Case (a) starts firing at Task 4 (annotation lowered), case (b) at Task 5 (literal typed). **Updated split across Tasks 4 and 5.**
   These updates are the intended landing of the rule, not regressions (pre-release, no dual paths).
3. **`circuit_node_by_index` re-walks the green tree per circuit (O(circuits × nodes)).** v2b.2's final-review note assigned the fix to "v2b.3's first non-trivial rule." `type_diagnostics_query` calls `circuit_node_by_index` per circuit *and* calls `infer_circuit_returns` (which re-walks) per circuit. **Task 1** discharges the carry-forward: one tree walk builds an index→`CircuitDef` map, and the body-typing loop is extracted into a shared `circuit_return_kinds` helper. Behavior-preserving.
4. **`TyKind` is `Copy`.** Adding `Uint(Option<u128>)` keeps `TyKind: Copy, Eq, Hash` (all satisfied by `Option<u128>`), so the interned `Ty` field (`#[returns(copy)] kind: TyKind`) is unaffected.
5. **Operand universe stays "literals only."** The foundation's `infer_circuit_returns` types only literal operands; that is unchanged. Literal-into-declared checks exercise the *entire* containment lattice (a literal's minimal range vs the declared range), so no resolution/expression inference is pulled in.

## File Structure

- `crates/analyzer-core/src/ty.rs` **(modify)** — add the `Uint(Option<u128>)` kind, its `ty_display` arm, and the `is_subtype` lattice function. Responsibility: the type universe + its relations + display.
- `crates/analyzer-core/src/infer.rs` **(modify)** — extract `circuit_return_kinds`; single-walk `circuit_node_map` in `type_diagnostics_query`; extend `type_node_kind` to lower `Uint` annotations (+`uint_bound`, `parse_int_literal` helpers); extend `literal_ty_kind` to type integer literals; switch the mismatch check to `is_subtype`; update the two drifted tests. Responsibility: inference + type diagnostics.
- `crates/compact-analyzer/tests/fixtures/type/*.compact` **(create)** — six `Uint`-lattice fixtures captured from `compactc`.
- `crates/compact-analyzer/tests/type_differential.rs` **(modify)** — add the six fixtures to the `FIXTURES` table (rule `"uint-lattice"`). The `corpus_no_false_positives` gate is unchanged and self-skips locally.

## Standard verification commands (used by every task)

- Build+test a crate: `cargo test -p <crate> -q`
- Whole workspace: `cargo test --workspace -q`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Format check: `cargo fmt --all --check`

---

### Task 1: Single-walk circuit map + `circuit_return_kinds` (v2b.2 carry-forward)

Discharge the O(circuits × nodes) carry-forward: build the index→`CircuitDef` association with **one** tree walk in `type_diagnostics_query`, and extract the return-body typing into a shared `circuit_return_kinds(&CircuitDef)` helper that both `infer_circuit_returns` and `type_diagnostics_query` use. Behavior-preserving — no diagnostic changes.

**Files:**
- Modify: `crates/analyzer-core/src/infer.rs` (`infer_circuit_returns` ~79-103; `type_diagnostics_query` ~114-145)

**Interfaces:**
- Consumes: `crate::db::{Db, SourceText, item_tree, parsed}`, `compactp_ast::{CircuitDef, AstNode}`, `crate::SymbolKind`, existing `circuit_node_by_index`, `type_node_kind`, `literal_ty_kind`.
- Produces: `fn circuit_return_kinds(circuit: &compactp_ast::CircuitDef) -> Vec<(text_size::TextRange, crate::ty::TyKind)>` (module-private); `fn circuit_node_map(db: &dyn Db, src: SourceText) -> Vec<(u32, compactp_ast::CircuitDef)>` (module-private). `infer_circuit_returns` and `type_diagnostics_query` keep their exact signatures.

- [ ] **Step 1: Write a failing guard test** — add to `infer.rs` `mod tests` a multi-circuit test that pins the correct index→node association (the invariant the single-walk map must preserve):

```rust
    #[test]
    fn multi_circuit_reports_only_the_mismatched_one() {
        let db = CompactDatabase::default();
        let src = SourceText::new(
            &db,
            Arc::from(
                "export circuit good(): Boolean { return true; }\n\
                 export circuit bad(): Field { return true; }",
            ),
        );
        let diags = type_diagnostics_query(&db, src);
        assert_eq!(diags.len(), 1, "only `bad` mismatches");
        assert!(
            diags[0].message.contains("bad"),
            "diagnostic names the right circuit: {:?}",
            diags[0].message
        );
    }
```

- [ ] **Step 2: Run it** — `cargo test -p analyzer-core infer::tests::multi_circuit -q`. Expected: PASS already (v2b.2 behavior is correct here). This test is a **refactor guard** — it must stay green through the rewrite.

- [ ] **Step 3: Extract `circuit_return_kinds`** — in `infer.rs`, add the shared body-typing helper and re-point `infer_circuit_returns` at it. Replace the existing `infer_circuit_returns` body:

```rust
/// Types the primitive-literal operand of each `return` in a circuit body:
/// `(return-statement range, TyKind)`. The shared core of the inference entry
/// point and the diagnostics query — both call this on an already-located
/// `CircuitDef`, so the file's green tree is walked once per consumer, not once
/// per circuit.
fn circuit_return_kinds(
    circuit: &compactp_ast::CircuitDef,
) -> Vec<(text_size::TextRange, TyKind)> {
    let Some(body) = circuit.body() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for stmt in body.stmts() {
        let compactp_ast::Stmt::Return(ret) = stmt else {
            continue;
        };
        let Some(compactp_ast::expr::Expr::Literal(lit)) = ret.value() else {
            continue;
        };
        out.push((ret.syntax().text_range(), literal_ty_kind(&lit)));
    }
    out
}

/// Inference entry point for one circuit body: `(return-statement range,
/// TyKind)` for each `return` whose operand is a primitive literal the checker
/// can type. Carries the `'static` `TyKind` (not `Ty<'db>`): salsa forbids a
/// tracked return of `Arc<[(_, Ty<'db>)]>`. `Ty` is interned downstream, at the
/// display boundary.
#[salsa::tracked(returns(clone))]
pub fn infer_circuit_returns(
    db: &dyn Db,
    src: SourceText,
    circuit_index: u32,
) -> Arc<[(text_size::TextRange, TyKind)]> {
    let Some(circuit) = circuit_node_by_index(db, src, circuit_index) else {
        return Arc::from(Vec::new());
    };
    Arc::from(circuit_return_kinds(&circuit))
}
```

- [ ] **Step 4: Add the single-walk `circuit_node_map`** — add to `infer.rs` (near `circuit_node_by_index`):

```rust
/// `(item-tree index, CircuitDef)` for every circuit in `src`, built with a
/// single walk of the file's green tree. Replaces per-circuit
/// `circuit_node_by_index` calls (each of which re-walked the tree) in the hot
/// diagnostics path. Association is by the symbol's `name_range` matching the
/// `CircuitDef`'s name-token range — identical to `circuit_node_by_index`, so
/// nested circuits are covered the same way.
fn circuit_node_map(db: &dyn Db, src: SourceText) -> Vec<(u32, compactp_ast::CircuitDef)> {
    let tree = item_tree(db, src);
    let index_of: std::collections::HashMap<text_size::TextRange, u32> = tree
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == crate::SymbolKind::Circuit)
        .map(|(i, s)| (s.name_range, i as u32))
        .collect();
    let root = compactp_syntax::SyntaxNode::new_root(parsed(db, src).green);
    root.descendants()
        .filter_map(compactp_ast::CircuitDef::cast)
        .filter_map(|c| {
            let range = c.name().map(|n| n.text_range())?;
            index_of.get(&range).map(|&idx| (idx, c))
        })
        .collect()
}
```

- [ ] **Step 5: Rewrite `type_diagnostics_query` to use the map + shared helper** — replace its body:

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
        if declared == TyKind::Unknown {
            continue; // only fire on return types the checker models
        }
        let name = &tree.symbols[idx as usize].name;
        for (range, kind) in circuit_return_kinds(&circuit) {
            let actual = kind;
            if actual != TyKind::Unknown && actual != declared {
                diags.push(return_mismatch_diag(db, actual, declared, name, range));
            }
        }
    }
    Arc::from(diags)
}
```

(The mismatch condition is still equality here — Task 3 swaps it for `is_subtype`. `compactp_syntax` is already needed; add `use compactp_syntax;`-style path via the fully-qualified `compactp_syntax::SyntaxNode` as written, matching `db.rs`.)

- [ ] **Step 6: Run the full infer suite + guard** — `cargo test -p analyzer-core infer:: -q`. Expected: PASS (all existing infer tests + the new `multi_circuit_reports_only_the_mismatched_one`). Then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/infer.rs
git commit -m "perf(v2b.3): single-walk circuit map; extract circuit_return_kinds

Discharges the v2b.2 carry-forward: type_diagnostics_query walks the green
tree once (index->CircuitDef map) instead of per-circuit via
circuit_node_by_index, and shares the return-body typing loop with
infer_circuit_returns. Behavior-preserving.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: `TyKind::Uint` + display projection

Extend the type universe with the bounded-integer kind and its normalized display. No `Uint` is *produced* yet (annotations/literals are lowered in Tasks 4–5), so this task is additive and every existing test stays green.

**Files:**
- Modify: `crates/analyzer-core/src/ty.rs` (`TyKind` enum ~16-26; `ty_display` ~40-46; `mod tests`)

**Interfaces:**
- Consumes: `crate::db::Db`.
- Produces: `TyKind::Uint(Option<u128>)` (the exclusive upper bound; `None` = "≥ 2^128, not exactly representable"); `ty_display` renders it as `Uint<k>` / `Uint<0..hi>` / `Uint<0..?>`.

- [ ] **Step 1: Write the failing test** — add to `ty.rs` `mod tests`:

```rust
    #[test]
    fn uint_display_normalizes_power_of_two() {
        let db = CompactDatabase::default();
        // hi == 2^k -> Uint<k>
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Uint(Some(256)))), "Uint<8>");
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Uint(Some(2)))), "Uint<1>");
        // non power of two -> Uint<0..hi>
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Uint(Some(10)))), "Uint<0..10>");
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Uint(Some(1)))), "Uint<0..1>");
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Uint(Some(257)))), "Uint<0..257>");
        // not exactly representable
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Uint(None))), "Uint<0..?>");
    }

    #[test]
    fn uint_interns_by_bound() {
        let db = CompactDatabase::default();
        let a = Ty::new(&db, TyKind::Uint(Some(256)));
        let b = Ty::new(&db, TyKind::Uint(Some(256)));
        let c = Ty::new(&db, TyKind::Uint(Some(255)));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
```

- [ ] **Step 2: Run it to verify it fails** — `cargo test -p analyzer-core ty::tests::uint -q`. Expected: FAIL to compile (`TyKind::Uint` undefined).

- [ ] **Step 3: Add the `Uint` variant** — in `ty.rs`, add to `TyKind`:

```rust
    /// A bounded unsigned integer `Uint<0..upper>` (half-open: values `[0, upper)`).
    /// The payload is the **exclusive upper bound**, or `None` when it exceeds
    /// `u128` and is not exactly representable — in which case the lattice
    /// compares it conservatively (never a false positive). `Uint<k>` and
    /// `Uint<lo..hi>` both lower to this single canonical form.
    Uint(Option<u128>),
```

- [ ] **Step 4: Add the display arm** — in `ty_display`, add before/after the existing arms:

```rust
        TyKind::Uint(Some(hi)) => {
            // hi == 2^k for 1 <= k <= 248 renders as the bit-width form.
            if hi.is_power_of_two() {
                let k = hi.trailing_zeros();
                if (1..=248).contains(&k) {
                    return format!("Uint<{k}>");
                }
            }
            format!("Uint<0..{hi}>")
        }
        TyKind::Uint(None) => "Uint<0..?>".to_string(),
```

(`ty_display` returns `String`; the `return` inside the match arm is fine. If clippy prefers, restructure to a `let` — either compiles.)

- [ ] **Step 5: Run + verify** — `cargo test -p analyzer-core ty:: -q`. Expected: PASS (new + existing). Then `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/ty.rs
git commit -m "feat(v2b.3): TyKind::Uint(Option<u128>) + normalized display

The bounded-integer kind carries its exclusive upper bound; ty_display
renders Uint<k> for powers of two, else Uint<0..hi>, and Uint<0..?> for
bounds beyond u128. No Uint is produced yet (Tasks 4-5), so additive.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: The subtype lattice (`is_subtype`) + apply it at the return position

Add the extracted range-containment relation and switch `type_diagnostics_query`'s mismatch check from equality to `!is_subtype`. Behavior-preserving at this point (no `Uint` is produced yet, and `is_subtype` reproduces the old equality outcomes for `Boolean`/`Field`), so all existing tests stay green — this is a clean checkpoint before `Uint` starts flowing.

**Files:**
- Modify: `crates/analyzer-core/src/ty.rs` (add `is_subtype` + tests)
- Modify: `crates/analyzer-core/src/infer.rs` (`type_diagnostics_query` mismatch condition; import `is_subtype`)

**Interfaces:**
- Consumes: `crate::ty::TyKind`.
- Produces: `pub(crate) fn is_subtype(sub: TyKind, sup: TyKind) -> bool` — `true` iff a value of `sub` may be used where `sup` is expected. `Unknown` on either side ⇒ `true` (suppress the rule). Consumed by `type_diagnostics_query`.

- [ ] **Step 1: Write the failing test** — add to `ty.rs` `mod tests`, transcribing the extracted lattice edges:

```rust
    #[test]
    fn subtype_lattice_matches_compiler() {
        use TyKind::*;
        // reflexive within a kind
        assert!(is_subtype(Boolean, Boolean));
        assert!(is_subtype(Field, Field));
        // Uint containment (a <: b iff a <= b)
        assert!(is_subtype(Uint(Some(6)), Uint(Some(10))));   // 0..5 into 0..10 -> literal 5 case
        assert!(!is_subtype(Uint(Some(12)), Uint(Some(10)))); // 0..12 into 0..10 -> literal 11 case
        assert!(is_subtype(Uint(Some(10)), Uint(Some(10))));
        assert!(is_subtype(Uint(Some(256)), Uint(Some(65536))));  // Uint8 into Uint16
        assert!(!is_subtype(Uint(Some(65536)), Uint(Some(256)))); // Uint16 into Uint8
        // Uint <: Field, one way only
        assert!(is_subtype(Uint(Some(256)), Field));
        assert!(!is_subtype(Field, Uint(Some(256))));
        // Boolean isolated
        assert!(!is_subtype(Boolean, Field));
        assert!(!is_subtype(Field, Boolean));
        assert!(!is_subtype(Boolean, Uint(Some(256))));
        assert!(!is_subtype(Uint(Some(256)), Boolean));
        // conservative on non-representable bounds (never a false positive)
        assert!(is_subtype(Uint(Some(5)), Uint(None)));  // concrete fits huge
        assert!(!is_subtype(Uint(None), Uint(Some(5)))); // huge cannot fit concrete
        assert!(is_subtype(Uint(None), Uint(None)));     // undecidable -> conservative accept
        assert!(is_subtype(Uint(None), Field));          // any Uint <: Field
        // Unknown suppresses in both directions
        assert!(is_subtype(Unknown, Field));
        assert!(is_subtype(Boolean, Unknown));
    }
```

- [ ] **Step 2: Run it to verify it fails** — `cargo test -p analyzer-core ty::tests::subtype -q`. Expected: FAIL to compile (`is_subtype` undefined).

- [ ] **Step 3: Implement `is_subtype`** — add to `ty.rs`:

```rust
/// Assignability: `true` iff a value of `sub` may be used where `sup` is
/// expected, per the compiler's type checker (verified against compact 0.5.1).
/// `Unknown` on either side yields `true` — the checker only *suppresses* on an
/// unmodeled type, it never manufactures a mismatch from one.
pub(crate) fn is_subtype(sub: TyKind, sup: TyKind) -> bool {
    match (sub, sup) {
        (TyKind::Unknown, _) | (_, TyKind::Unknown) => true,
        (TyKind::Boolean, TyKind::Boolean) => true,
        (TyKind::Field, TyKind::Field) => true,
        // Every Uint is a subtype of Field (widening to Field always allowed).
        (TyKind::Uint(_), TyKind::Field) => true,
        // Uint<0..a> <: Uint<0..b> iff a <= b (range containment).
        (TyKind::Uint(a), TyKind::Uint(b)) => uint_upper_le(a, b),
        // Everything else (Boolean/Field/Uint cross, Field->Uint) is unrelated.
        _ => false,
    }
}

/// `a <= b` on exclusive upper bounds, conservative when a bound is not exactly
/// representable (`None` = "≥ 2^128"). Exact whenever both fit `u128`. See the
/// plan's *Representation design* table: the only non-exact case is
/// `(None, None)`, resolved to `true` (a possible false negative, never a false
/// positive).
fn uint_upper_le(a: Option<u128>, b: Option<u128>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a <= b,
        (Some(_), None) => true,  // concrete < 2^128 <= huge
        (None, Some(_)) => false, // huge >= 2^128 > concrete
        (None, None) => true,     // undecidable -> conservative accept
    }
}
```

- [ ] **Step 4: Run the lattice test** — `cargo test -p analyzer-core ty::tests::subtype -q`. Expected: PASS.

- [ ] **Step 5: Switch the mismatch check to the lattice** — in `infer.rs`, add `is_subtype` to the ty import (`use crate::ty::{Ty, TyKind, is_subtype, ty_display};`) and replace the condition in `type_diagnostics_query`:

```rust
        for (range, kind) in circuit_return_kinds(&circuit) {
            if !is_subtype(kind, declared) {
                diags.push(return_mismatch_diag(db, kind, declared, name, range));
            }
        }
```

(The prior `actual != Unknown && actual != declared` guard is subsumed: `is_subtype` returns `true` whenever `kind` is `Unknown`, so an unmodeled operand still emits nothing. The `if declared == Unknown { continue; }` early-out above stays as a per-circuit optimization.)

- [ ] **Step 6: Run everything** — `cargo test -p analyzer-core -q` then `cargo test --workspace -q`. Expected: all green (every existing `infer.rs` diagnostic test still passes — `Boolean`/`Field` outcomes are identical under `is_subtype`). Then `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/ty.rs crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.3): Uint subtype lattice; return check uses is_subtype

is_subtype encodes the extracted relation (Uint range containment, every
Uint <: Field, Boolean isolated, conservative on non-representable bounds,
Unknown suppresses). type_diagnostics_query fires on !is_subtype instead
of inequality. Behavior-preserving until Uint types start flowing.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 4: Lower `Uint<...>` annotations to `TyKind::Uint`

Teach `type_node_kind` to read a `Uint<k>` / `Uint<lo..hi>` annotation into its canonical bound, so a circuit's declared return type can be a real `Uint`. This flips case (a) of the drifted foundation test.

**Files:**
- Modify: `crates/analyzer-core/src/infer.rs` (`type_node_kind` ~25-32; add `uint_bound` + `parse_int_literal`; update `unknown_operand_or_expectation_suppresses`; add lowering tests)

**Interfaces:**
- Consumes: `compactp_ast::{Type, UintType, TypeSize, AstNode}`, `crate::ty::TyKind`.
- Produces: `type_node_kind` now maps `Type::Uint` via `fn uint_bound(u: &compactp_ast::UintType) -> TyKind` (module-private) and `fn parse_int_literal(text: &str) -> Option<u128>` (module-private, reused by Task 5).

- [ ] **Step 1: Write the failing tests** — add to `infer.rs` `mod tests`:

```rust
    #[test]
    fn uint_annotation_lowers_to_bound() {
        let db = CompactDatabase::default();
        // Uint<8> -> exclusive upper 2^8 = 256
        let a = SourceText::new(&db, Arc::from("export circuit f(): Uint<8> { }"));
        let ia = circuit_index(&db, a, "f");
        let ca = circuit_node_by_index(&db, a, ia).unwrap();
        assert_eq!(type_node_kind(&ca.return_type().unwrap()), TyKind::Uint(Some(256)));
        // Uint<0..10> -> exclusive upper 10
        let b = SourceText::new(&db, Arc::from("export circuit f(): Uint<0..10> { }"));
        let ib = circuit_index(&db, b, "f");
        let cb = circuit_node_by_index(&db, b, ib).unwrap();
        assert_eq!(type_node_kind(&cb.return_type().unwrap()), TyKind::Uint(Some(10)));
        // Malformed (non-zero start) -> Unknown (suppresses; not our diagnostic)
        let c = SourceText::new(&db, Arc::from("export circuit f(): Uint<3..10> { }"));
        let ic = circuit_index(&db, c, "f");
        let cc = circuit_node_by_index(&db, c, ic).unwrap();
        assert_eq!(type_node_kind(&cc.return_type().unwrap()), TyKind::Unknown);
        // Width beyond 127 -> not exactly representable
        let d = SourceText::new(&db, Arc::from("export circuit f(): Uint<200> { }"));
        let id = circuit_index(&db, d, "f");
        let cd = circuit_node_by_index(&db, d, id).unwrap();
        assert_eq!(type_node_kind(&cd.return_type().unwrap()), TyKind::Uint(None));
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p analyzer-core infer::tests::uint_annotation -q`. Expected: FAIL (`Uint<8>` currently lowers to `Unknown`, so the asserts fail).

- [ ] **Step 3: Add the parsing + bound helpers** — add to `infer.rs`:

```rust
/// Parse a `Uint` size or an integer-literal token's text to a `u128`.
/// Accepts decimal and `0x`/`0X` hex. `None` if the text is not a plain integer
/// literal (e.g. a generic numeric parameter identifier) or overflows `u128`.
fn parse_int_literal(text: &str) -> Option<u128> {
    let t = text.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u128::from_str_radix(hex, 16).ok()
    } else {
        t.parse::<u128>().ok()
    }
}

/// Lower a `Uint<...>` annotation to its canonical `TyKind::Uint(exclusive
/// upper bound)`, per the extracted rule. A malformed or non-literal annotation
/// lowers to `Unknown` (conservatively suppresses the lattice — corpus-safe,
/// since accepted files have no malformed Uints). `Uint<k>` denotes `[0, 2^k)`
/// for `1 <= k <= 248`; `Uint<lo..hi>` denotes `[lo, hi)` with `lo == 0`,
/// `hi >= 1`.
fn uint_bound(u: &compactp_ast::UintType) -> TyKind {
    let sizes: Vec<compactp_ast::TypeSize> = u.sizes().collect();
    match sizes.as_slice() {
        // Bit-width form: Uint<k>.
        [k] => match parse_int_literal(&k.syntax().text().to_string()) {
            Some(k) if (1..=248).contains(&k) => {
                let bound = if k < 128 { Some(1u128 << k) } else { None };
                TyKind::Uint(bound)
            }
            _ => TyKind::Unknown, // out of range or non-literal
        },
        // Range form: Uint<lo..hi>.
        [lo, hi] => {
            if parse_int_literal(&lo.syntax().text().to_string()) != Some(0) {
                return TyKind::Unknown; // range start must be 0 (else malformed)
            }
            match parse_int_literal(&hi.syntax().text().to_string()) {
                Some(0) => TyKind::Unknown,      // range end must be >= 1
                Some(hi) => TyKind::Uint(Some(hi)),
                None => TyKind::Uint(None),      // hi is huge (>= 2^128) or non-literal-huge
            }
        }
        _ => TyKind::Unknown,
    }
}
```

Note: `TypeSize` has no value accessor in `compactp_ast`; reading `.syntax().text().to_string()` and parsing is the established pattern for size params. If `text()` returns a `rowan::SyntaxText`, `.to_string()` is available; `parse_int_literal` trims. A `None` from a *non-numeric* `hi` (an identifier) also yields `TyKind::Uint(None)` — conservative (treated as a huge, undecidable bound), which never produces a false positive.

- [ ] **Step 4: Route `Type::Uint` through `uint_bound`** — update `type_node_kind`:

```rust
pub(crate) fn type_node_kind(ty: &compactp_ast::Type) -> TyKind {
    use compactp_ast::Type;
    match ty {
        Type::Boolean(_) => TyKind::Boolean,
        Type::Field(_) => TyKind::Field,
        Type::Uint(u) => uint_bound(u),
        _ => TyKind::Unknown,
    }
}
```

- [ ] **Step 5: Update the drifted foundation test — case (a)** — in `infer.rs` `mod tests`, edit `unknown_operand_or_expectation_suppresses`. Case (a) (`Uint<0..7> { return true; }`) now **rejects** (`Boolean` is not assignable to `Uint`), matching `compactc`. Case (b) (`Boolean { return 1; }`) still emits nothing at this task (the literal is typed in Task 5). Replace the test with:

```rust
    #[test]
    fn unknown_operand_or_expectation_suppresses() {
        let db = CompactDatabase::default();
        // Boolean literal into a Uint return: Boolean </: Uint -> rejects
        // (matches compactc: `Uint<8> return true` is a type error).
        let a = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Uint<0..7> { return true; }"),
        );
        assert_eq!(type_diagnostics_query(&db, a).len(), 1);
        // A return type the checker does not model (Bytes) still suppresses.
        let b = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Bytes<32> { return true; }"),
        );
        assert!(type_diagnostics_query(&db, b).is_empty());
    }
```

- [ ] **Step 6: Run + verify** — `cargo test -p analyzer-core infer:: -q`. Expected: PASS (new lowering tests, the updated suppress test, and all unaffected tests). Then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.3): lower Uint<...> annotations to canonical bound

type_node_kind reads Uint<k> and Uint<lo..hi> into TyKind::Uint(exclusive
upper), via uint_bound + parse_int_literal. Malformed/non-literal/out-of-
range annotations lower to Unknown (conservative). Boolean-into-Uint
returns now correctly reject, matching compactc.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: Type integer literals to their minimal `Uint`

Teach `literal_ty_kind` to type a decimal/hex integer literal `n` as `TyKind::Uint(Some(n+1))` (minimal range `[0, n]`), the exact rule the compiler applies. This completes the lattice for literal operands and flips the remaining drifted assertions.

**Files:**
- Modify: `crates/analyzer-core/src/infer.rs` (`literal_ty_kind` ~37-50; update `non_primitive_literal_return_is_unknown` and case (b) of the suppress test; add literal-typing + end-to-end tests)

**Interfaces:**
- Consumes: `compactp_ast::expr::LiteralExpr`, `compactp_syntax::SyntaxKind` (`INT_LIT`, `HEX_LIT`, `TRUE_KW`, `FALSE_KW`), `parse_int_literal` (Task 4).
- Produces: `literal_ty_kind` returns `TyKind::Uint(Some(n+1))` for an integer literal `n` (or `TyKind::Uint(None)` when `n+1` overflows `u128`), `TyKind::Boolean` for `true`/`false`, else `TyKind::Unknown`.

- [ ] **Step 1: Write the failing tests** — add to `infer.rs` `mod tests`:

```rust
    #[test]
    fn integer_literal_types_to_minimal_uint() {
        let db = CompactDatabase::default();
        // 5 -> Uint<0..6>
        let a = SourceText::new(&db, Arc::from("export circuit f(): Field { return 5; }"));
        let ia = circuit_index(&db, a, "f");
        assert_eq!(infer_circuit_returns(&db, a, ia)[0].1, TyKind::Uint(Some(6)));
        // 0 -> Uint<0..1>
        let b = SourceText::new(&db, Arc::from("export circuit f(): Field { return 0; }"));
        let ib = circuit_index(&db, b, "f");
        assert_eq!(infer_circuit_returns(&db, b, ib)[0].1, TyKind::Uint(Some(1)));
        // hex 0xFF -> Uint<0..256>
        let c = SourceText::new(&db, Arc::from("export circuit f(): Field { return 0xFF; }"));
        let ic = circuit_index(&db, c, "f");
        assert_eq!(infer_circuit_returns(&db, c, ic)[0].1, TyKind::Uint(Some(256)));
    }

    #[test]
    fn literal_into_uint_range_accept_and_reject() {
        let db = CompactDatabase::default();
        // In range: 5 (Uint<0..6>) <: Uint<0..10> -> no diagnostic
        let ok = SourceText::new(&db, Arc::from("export circuit f(): Uint<0..10> { return 5; }"));
        assert!(type_diagnostics_query(&db, ok).is_empty());
        // Out of range: 11 (Uint<0..12>) not <: Uint<0..10> -> one diagnostic
        let bad = SourceText::new(&db, Arc::from("export circuit f(): Uint<0..10> { return 11; }"));
        let diags = type_diagnostics_query(&db, bad);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("Uint<0..12>") && diags[0].message.contains("Uint<0..10>"),
            "message uses normalized Uint display: {:?}",
            diags[0].message
        );
        // Uint literal into Field is always fine.
        let field = SourceText::new(&db, Arc::from("export circuit f(): Field { return 5; }"));
        assert!(type_diagnostics_query(&db, field).is_empty());
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p analyzer-core infer::tests::integer_literal -q`. Expected: FAIL (integer literals currently type to `Unknown`).

- [ ] **Step 3: Type integer literals** — replace `literal_ty_kind`:

```rust
/// The `TyKind` of a literal expression. `true`/`false` -> `Boolean`; a decimal
/// or hex integer literal `n` -> `TyKind::Uint(Some(n+1))` (the minimal range
/// `[0, n]`), or `Uint(None)` when `n+1` overflows `u128`; other literals
/// (string, etc.) -> `Unknown`.
fn literal_ty_kind(lit: &compactp_ast::expr::LiteralExpr) -> TyKind {
    use compactp_syntax::SyntaxKind;
    let tokens: Vec<_> = lit
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .collect();
    if tokens
        .iter()
        .any(|t| matches!(t.kind(), SyntaxKind::TRUE_KW | SyntaxKind::FALSE_KW))
    {
        return TyKind::Boolean;
    }
    if let Some(tok) = tokens
        .iter()
        .find(|t| matches!(t.kind(), SyntaxKind::INT_LIT | SyntaxKind::HEX_LIT))
    {
        // n -> minimal range [0, n] = exclusive upper n+1. Overflow -> None
        // (still a Uint; e.g. a 2^200 literal is Uint(None) <: Field).
        let bound = parse_int_literal(tok.text()).and_then(|n| n.checked_add(1));
        return TyKind::Uint(bound);
    }
    TyKind::Unknown
}
```

- [ ] **Step 4: Update `non_primitive_literal_return_is_unknown`** — this foundation test asserted the pre-`Uint` placeholder. Rename + retarget it to the correct behavior:

```rust
    #[test]
    fn integer_literal_return_is_typed_uint() {
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("export circuit foo(): Field { return 1; }"));
        let idx = circuit_index(&db, src, "foo");
        let returns = infer_circuit_returns(&db, src, idx);
        assert_eq!(returns.len(), 1);
        // 1 -> minimal range [0, 1] -> exclusive upper 2.
        assert_eq!(returns[0].1, TyKind::Uint(Some(2)));
    }
```

- [ ] **Step 5: Update the drifted suppress test — case (b)** — extend `unknown_operand_or_expectation_suppresses` (edited in Task 4) so case (b) now reflects that a numeric literal into `Boolean` **rejects** (matches `compactc`). Replace it with:

```rust
    #[test]
    fn unknown_operand_or_expectation_suppresses() {
        let db = CompactDatabase::default();
        // Boolean literal into a Uint return: rejects (Boolean </: Uint).
        let a = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Uint<0..7> { return true; }"),
        );
        assert_eq!(type_diagnostics_query(&db, a).len(), 1);
        // Integer literal into a Boolean return: rejects (Uint </: Boolean),
        // matching compactc (`Boolean return 5` is a type error).
        let b = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Boolean { return 1; }"),
        );
        assert_eq!(type_diagnostics_query(&db, b).len(), 1);
        // A return type the checker does not model (Bytes) still suppresses.
        let c = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Bytes<32> { return true; }"),
        );
        assert!(type_diagnostics_query(&db, c).is_empty());
    }
```

- [ ] **Step 6: Run everything** — `cargo test -p analyzer-core infer:: -q` then `cargo test --workspace -q`. Expected: all green. Then `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.3): type integer literals to minimal Uint

literal_ty_kind types decimal/hex integer literal n as Uint(Some(n+1)) —
the minimal range [0, n] the compiler assigns — or Uint(None) on u128
overflow. Completes the lattice for literal operands; literal-into-range
returns now accept/reject exactly as compactc does.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 6: Rule-tagged differential fixtures for the `Uint` lattice

Pin the rule against the live compiler: six fixtures whose expected native verdict was captured from `compactc` accept/reject, added to the existing `type_differential` harness (with its live cross-check). The `corpus_no_false_positives` gate is unchanged and must stay green (self-skips locally, runs in CI with `COMPACT_CORPUS_DIR`).

**Files:**
- Create: `crates/compact-analyzer/tests/fixtures/type/uint_in_range_ok.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/uint_over_range.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/uint_bitwidth_max_ok.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/uint_bitwidth_over.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/uint_literal_into_field_ok.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/boolean_into_uint.compact`
- Modify: `crates/compact-analyzer/tests/type_differential.rs` (`FIXTURES` table ~47-58)

**Interfaces:**
- Consumes: the existing `rule_tagged_fixtures` runner, `Fixture { name, native_rejects, rule }`, `native_rejects`, `compiler_verdict`.
- Produces: six new `FIXTURES` rows tagged `"uint-lattice"`.

- [ ] **Step 1: Create the fixtures** (verdicts captured from `compactc 0.31.1` — see *Extracted rule*).

`uint_in_range_ok.compact` — `compactc` **accepts**:
```
export circuit f(): Uint<0..10> {
  return 5;
}
```

`uint_over_range.compact` — `compactc` **rejects** (`Uint<0..11>` ⊄ `Uint<0..10>`):
```
export circuit f(): Uint<0..10> {
  return 10;
}
```

`uint_bitwidth_max_ok.compact` — `compactc` **accepts** (`255` fits `Uint<8>` = `[0,256)`):
```
export circuit f(): Uint<8> {
  return 255;
}
```

`uint_bitwidth_over.compact` — `compactc` **rejects** (`256` = `Uint<0..257>` ⊄ `Uint<8>`):
```
export circuit f(): Uint<8> {
  return 256;
}
```

`uint_literal_into_field_ok.compact` — `compactc` **accepts** (`Uint <: Field`):
```
export circuit f(): Field {
  return 5;
}
```

`boolean_into_uint.compact` — `compactc` **rejects** (`Boolean` ⊄ `Uint`):
```
export circuit f(): Uint<8> {
  return true;
}
```

- [ ] **Step 2: Add the fixtures to the table** — in `type_differential.rs`, extend `FIXTURES` (keep the two existing `primitive-literal-return` rows):

```rust
    Fixture { name: "uint_in_range_ok.compact", native_rejects: false, rule: "uint-lattice" },
    Fixture { name: "uint_over_range.compact", native_rejects: true, rule: "uint-lattice" },
    Fixture { name: "uint_bitwidth_max_ok.compact", native_rejects: false, rule: "uint-lattice" },
    Fixture { name: "uint_bitwidth_over.compact", native_rejects: true, rule: "uint-lattice" },
    Fixture { name: "uint_literal_into_field_ok.compact", native_rejects: false, rule: "uint-lattice" },
    Fixture { name: "boolean_into_uint.compact", native_rejects: true, rule: "uint-lattice" },
```

- [ ] **Step 3: Run the fixture runner** — `cargo test -p compact-analyzer --test type_differential rule_tagged_fixtures -q`. Expected: PASS. The toolchain is present in this sandbox, so the live cross-check runs for all eight fixtures and each native verdict must match the `compactc` verdict (`RejectPostParse` for `native_rejects: true`, `Accept` for `false`).

- [ ] **Step 4: Run the corpus gate + full suite** — `cargo test -p compact-analyzer --test type_differential -q` (the `corpus_no_false_positives` gate self-skips locally — confirm it prints the SKIP line and passes). Then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`. To exercise the gate against a real corpus: `COMPACT_CORPUS_DIR=/path/to/compactp/tests/corpus cargo test -p compact-analyzer --test type_differential corpus_no_false_positives -q -- --nocapture` (must report `false_positives=0`).

- [ ] **Step 5: Commit**

```bash
git add crates/compact-analyzer/tests/type_differential.rs crates/compact-analyzer/tests/fixtures/type
git commit -m "test(v2b.3): rule-tagged Uint-lattice differential fixtures

Six fixtures pinning the lattice against compactc: in-range accept,
over-range reject, bit-width upper-inclusive accept + over reject, Uint
literal into Field accept, Boolean into Uint reject. Live compactc
cross-check runs when the toolchain is present; corpus gate stays green.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage** (rules-index v2b.3 row + foundation spec §4/§5 + program spec §5/§9):
- `Uint<0..n)` / `Uint<n>` modeled in the `Ty` universe → Task 2 (`TyKind::Uint(Option<u128>)`), Task 4 (annotation lowering). ✓
- The subtype lattice (widening/narrowing) extracted from the compiler and applied at the inference entry point → Task 3 (`is_subtype`), Task 5 (literal typing exercises it end-to-end). ✓
- Ground truth = compiler at the pinned toolchain; rule pinned by a `compactc` differential fixture **before** implementation → *Extracted rule* (captured first), Task 6 (fixtures + live cross-check). ✓
- Binary corpus gate stays green; no false positives → Global Constraints + conservative `Option<u128>` representation + Task 6 Step 4. ✓
- No LSP types in core; `Ty` stays in-crate (display projection only); single-threaded; pre-release → Global Constraints; `is_subtype`/`Uint` are in-crate. ✓
- Carry-forward reconciliation (O(n²) circuit walk) → Task 1. ✓
- Editor surfacing deferred to v2b.final → stated; no `server.rs` change. ✓
- Well-formedness diagnostics + parameter/expression typing explicitly out of scope, with corpus-safety rationale → Global Constraints (Rule scope). ✓

**2. Placeholder scan:** No "TBD"/"handle edge cases"/"similar to Task N". Every code step carries complete code; every test step shows assertions; every run step gives the command + expected outcome. The two notes about `TypeSize.syntax().text()` and non-numeric bounds (Task 4 Step 3) are verification instructions against the CST's established size-reading pattern, not placeholders.

**3. Type consistency:** `TyKind::Uint(Option<u128>)` (Task 2) is used with that exact shape in `is_subtype`/`uint_upper_le` (Task 3), `uint_bound` (Task 4), and `literal_ty_kind` (Task 5). `is_subtype(sub: TyKind, sup: TyKind) -> bool` (Task 3) is consumed with that signature in `type_diagnostics_query` (Task 3 Step 5). `circuit_return_kinds(&CircuitDef) -> Vec<(TextRange, TyKind)>` and `circuit_node_map(db, src) -> Vec<(u32, CircuitDef)>` (Task 1) are consumed in the rewritten `type_diagnostics_query` and `infer_circuit_returns` (Task 1). `parse_int_literal(&str) -> Option<u128>` (Task 4) is reused by `literal_ty_kind` (Task 5). `ty_display` (Task 2) renders the `Uint` display asserted in Task 5's message test. The `Fixture { name, native_rejects, rule }` shape (Task 6) matches the existing v2b.2 struct.

**Sequencing rationale:** Task 1 lands the behavior-preserving carry-forward on the current (`Boolean`/`Field`) behavior, before any `Uint` flows. Task 2 adds the kind + display (additive, all green). Task 3 adds the lattice and switches the check while it is still behavior-preserving (clean checkpoint). Tasks 4→5 introduce `Uint` types (annotations, then literals), each updating exactly the foundation test whose behavior it corrects. Task 6 pins the whole rule against the live compiler and confirms the corpus gate. Each task ends with an independently testable, independently reviewable deliverable.
