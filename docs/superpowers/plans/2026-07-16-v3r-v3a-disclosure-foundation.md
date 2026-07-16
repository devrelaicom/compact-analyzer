# v3-R + v3a — Disclosure Analysis Foundation & Intraprocedural WPP — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `compactc`-disclosure differential harness + extracted rule index (v3-R), then a fail-closed **intraprocedural** Witness Protection Program (WPP) in `analyzer-core` that flags witness-derived data reaching a sink within a single circuit body (v3a).

**Architecture:** A new `analyzer-core/src/disclosure/` module implements an abstract interpreter (the `Abs` lattice) over the item tree + v2b types, exposed as a `#[salsa::tracked]` query mirroring `type_diagnostics_query`. Every point the interpreter cannot decide emits an **amber "unverified" advisory** (fail-closed), never silent green. A differential test harness (mirroring `tests/type_differential.rs`) pins every rule against real `compactc` before it is implemented.

**Tech Stack:** Rust, salsa (v2a engine), `compactp_syntax`/`compactp_ast` (CST/AST), `compactp_diagnostics` (`Diagnostic`/`Severity`), `analyzer-toolchain` (`compile_file`/`classify`), `lsp_types`.

## Global Constraints

- **Spec is authoritative:** `docs/superpowers/specs/2026-07-16-v3-disclosure-analysis-design.md` (Revision 2.1). Every §-reference below points there.
- **Ground truth is the R0 index:** `docs/superpowers/plans/2026-07-16-v3-wpp-rules-index.md` (extracted from the true 0.31.1 source). Interpreter tasks consume it, NOT the spec's 0.33-era table line numbers.
- **Rev-2.1 version correction (applied 2026-07-16):** target is **0.31.1** = tag `compactc-v0.31.1` = commit `0da5b045` (the earlier `03da643` pin was 0.33.107). **`emit` is OUT OF SCOPE** for v3-R/v3a — it does not exist in 0.31.1 (version-gated future sink). **`contract-call` is unconditional** (not `pure?`-gated) but is deferred to a v3a amber advisory regardless.
- **FAIL CLOSED (spec §0):** every unanalyzable point (unhandled expr, un-interpreted callee, unknown native/ledger op, `Unknown` v2b type, deferred sink class) MUST emit an amber advisory, NEVER silent green. This is the primary correctness bar; every v3a task's done-bar includes it.
- **Verify-first (spec §5):** every rule is extracted from `LFDT-Minokawa/compact` `compiler/analysis-passes.ss` `track-witness-data` at the pinned tag (`compactc-v0.31.1` = `0da5b045`, confirmed in R0) and pinned by a `compactc`-differential fixture BEFORE it is implemented. Never from training data (the training-data-unreliability rule applies doubly to disclosure).
- **No independent soundness claim:** agrees-with-`compactc` only; the differential harness is the arbiter. Wording/spans are not differentially compared (verdict only), matching `type_differential.rs`.
- **Compact/language version:** compiler 0.31.x / language 0.23.0; `pragma language_version >= 0.23;` in new fixtures.
- **Diagnostic code families:** confirmed disclosure leaks → `E3100`+ (`E` code family, red). Advisories → new `U3100`+ (`U` family, amber). Keep codes stable once assigned.
- **Salsa queries** return `Arc<[Diagnostic]>` with `#[salsa::tracked(returns(clone), no_eq)]` (`Diagnostic` has no `PartialEq` — same rationale as `type_diagnostics_query`).

---

## File Structure

- Create: `crates/analyzer-core/src/disclosure/mod.rs` — module root; the `disclosure_diagnostics_query` salsa query + `AnalysisHost::disclosure_diagnostics` accessor.
- Create: `crates/analyzer-core/src/disclosure/abs.rs` — the `Abs` lattice, `Witness`, `WitnessInfo`, `PathPoint`, join (`combine_abs`), sanitize (`disclose`), equality (`abs_equal`).
- Create: `crates/analyzer-core/src/disclosure/interp.rs` — the abstract interpreter over a circuit body (v3a: intraprocedural).
- Create: `crates/analyzer-core/src/disclosure/leaks.rs` — the root-drained leak table (`(sink_src, nature)` → witness set) + `DisclosureLeak` → `Diagnostic` rendering; the advisory emitter.
- Modify: `crates/analyzer-core/src/lib.rs` — `mod disclosure;` + re-exports.
- Modify: `crates/analyzer-core/src/analysis.rs` — add `disclosure_diagnostics(file)` to `AnalysisHost` (mirrors `type_diagnostics`).
- Modify: `crates/compact-analyzer/src/lsp_utils.rs:66-70` — map a new `Severity::Advisory` → `DiagnosticSeverity::WARNING` with `source = "compact-analyzer (unverified)"`.
- Modify: `compactp_diagnostics` `Severity` (if we own it) OR add an analyzer-core-local advisory flag — decided in Task A8 after reading the crate.
- Create: `crates/compact-analyzer/tests/disclosure_differential.rs` — the differential harness (mirrors `tests/type_differential.rs`).
- Create: `crates/compact-analyzer/tests/fixtures/disclosure/*.compact` — the rule-tagged fixtures.
- Create: `docs/superpowers/plans/2026-07-16-v3-wpp-rules-index.md` — the extracted WPP rule index (R0 deliverable).

---

## Phase v3-R — Extraction + Differential Harness (no feature code)

### Task R0: Extract the WPP rule index from compiler source

**Files:**
- Create: `docs/superpowers/plans/2026-07-16-v3-wpp-rules-index.md`

**Interfaces:**
- Produces: a rule-by-rule index (name, source clause + line anchor, exact behavior, exact nature string, fixture name) that every v3a interpreter task consumes. No code.

- [ ] **Step 1: Re-confirm the pinned tag.** Read `compiler/analysis-passes.ss` at the tag the analyzer targets (`compact compile --version` locally → the matching `LFDT-Minokawa/compact` release/commit; Revision-2 verified `03da643`). Record the exact commit SHA at the top of the index.

- [ ] **Step 2: Transcribe each WPP clause** from `track-witness-data` into the index, one row per rule, copying the exact source snippet + line and the exact nature string. Cover, at minimum: the three sources (`Witness-Info`), each sink (`public-ledger`, `emit`, `return`+`filter-witnesses`, `contract-call`+`pure?`), the `disclose` sanitizer, `Abs` shapes (`default-value`), the native `Fun-native` conduit + `disclose?*` table, the ledger-op `discloses?` immediate-leak table, `handle-comparison`→`Abs-atomic`, the `if` branch/join (`combine-abs`), the `fold` `abs-equal?` fixpoint, `default<T>`/cast/serialize passthrough. Mark each row's fixture name.

- [ ] **Step 3: Mark the fail-closed default for each partial surface** (unknown native → conduit-taint; unknown ledger op → sink; unhandled expr → advisory) explicitly in the index.

- [ ] **Step 4: Commit.**
```bash
git add docs/superpowers/plans/2026-07-16-v3-wpp-rules-index.md
git commit -m "docs(v3-R): extracted WPP rule index from compiler source at <sha>"
```

### Task R1: Disclosure differential harness (native baseline + compactc WPP verdict)

**Files:**
- Create: `crates/compact-analyzer/tests/disclosure_differential.rs`
- Create: `crates/compact-analyzer/tests/fixtures/disclosure/` (dir)

**Interfaces:**
- Consumes: `analyzer_core::AnalysisHost::disclosure_diagnostics(file) -> Vec<Diagnostic>` (stubbed to `vec![]` until Task A1 — the harness must compile and pass as a no-op baseline first). `analyzer_toolchain::{Toolchain, compile_file, CompileOutcome, CompileStatus}`.
- Produces: `native_discloses(text, path) -> bool`, `compiler_discloses(tc, source) -> bool` (WPP-specific: a compile error whose stderr names a disclosure), the `FIXTURES` table + `rule_tagged_disclosure_fixtures` + `corpus_no_false_positive_disclosures` tests.

- [ ] **Step 1: Add a temporary stub** so the harness compiles before A1. In `crates/analyzer-core/src/analysis.rs`, add to `impl AnalysisHost`:
```rust
/// Disclosure (WPP) diagnostics for `file`. v3a stub: empty until the
/// interpreter lands (Task A1 replaces the body). Never panics.
pub fn disclosure_diagnostics(&mut self, _file: crate::FileId) -> Vec<compactp_diagnostics::Diagnostic> {
    Vec::new()
}
```

- [ ] **Step 2: Write the harness.** Mirror `tests/type_differential.rs`. The compiler side detects a **disclosure-specific** rejection (WPP errors are compile errors — exit 255 — whose stderr contains the WPP banner):
```rust
use std::path::{Path, PathBuf};
use std::time::Duration;
use analyzer_core::{AnalysisHost, FileId};
use analyzer_toolchain::{CompileStatus, Toolchain, compile_file};

/// Native side: does the WPP analyzer report any *confirmed leak* (E-family
/// disclosure diagnostic)? Advisories (U-family) are NOT rejections — they are
/// the fail-closed "unverified" signal and are excluded from the verdict.
fn native_discloses(text: &str, path: &Path) -> bool {
    let mut host = AnalysisHost::new();
    let file: FileId = host.vfs_mut().file_id(path);
    host.vfs_mut().set_overlay(file, text.to_string(), 1);
    host.disclosure_diagnostics(file)
        .iter()
        .any(|d| d.code.as_ref().map(|c| c.letter == 'E' && c.number >= 3100).unwrap_or(false))
}

/// compactc side: a compile error naming an undeclared disclosure.
fn compiler_discloses(tc: &Toolchain, source: &Path) -> Option<bool> {
    let scratch = tempfile::tempdir().expect("scratch");
    let outcome = compile_file(tc, source, scratch.path(), &[], Duration::from_secs(30), None);
    match outcome.status {
        CompileStatus::CompileError => Some(
            outcome.stderr.contains("witness-value disclosure must be declared"),
        ),
        CompileStatus::Ok => Some(false),
        _ => None, // parse/invocation/timeout: indeterminate, not a disclosure verdict
    }
}
```
(Confirm the exact `DiagnosticCode` field names — `letter`/`number` — against `compactp_diagnostics` while writing; adjust if they differ.)

- [ ] **Step 3: Add `FIXTURES` (empty for now) + the two tests** (`rule_tagged_disclosure_fixtures`, `corpus_no_false_positive_disclosures`), copying the skip-when-absent structure and the `COMPACT_CORPUS_DIR`/`../compactp` corpus discovery verbatim from `type_differential.rs`. The corpus gate asserts: for every corpus file `compactc` accepts, the native analyzer emits **zero** confirmed-leak (E-family) diagnostics (advisories allowed).

- [ ] **Step 4: Run the baseline.**
Run: `cargo test -p compact-analyzer --test disclosure_differential --locked`
Expected: PASS (native no-op ⇒ zero false positives; fixture loop empty).

- [ ] **Step 5: Commit.**
```bash
git add crates/compact-analyzer/tests/disclosure_differential.rs crates/analyzer-core/src/analysis.rs
git commit -m "test(v3-R): compactc-disclosure differential harness (no-op baseline)"
```

### Task R2: Seed the rule-tagged fixture set

**Files:**
- Create: `crates/compact-analyzer/tests/fixtures/disclosure/*.compact`
- Modify: `crates/compact-analyzer/tests/disclosure_differential.rs` (`FIXTURES`)

**Interfaces:**
- Consumes: R0 rule index (one fixture per rule row).
- Produces: fixture files + a **two-field** `Fixture { name, rule, discloses: bool, native_confirms: bool }` and `FIXTURES` entries. **`discloses`** = compactc's ground-truth WPP verdict (validated against real `compactc` NOW). **`native_confirms`** = whether the CURRENT native analyzer is expected to emit a confirmed E-leak (starts `false` for every fixture — native is a no-op stub; each v3a task flips its fixtures `false→true` as it implements the sink). This **green pending-flip** model (user-approved 2026-07-16) supersedes the plan's original single-field committed-RED baseline: the branch stays green at every step while every fixture is still compiler-validated up front.

- [ ] **Step 1: Refactor the harness to the two-field model.** In `disclosure_differential.rs`, change `struct Fixture` to `{ name: &'static str, rule: &'static str, discloses: bool, native_confirms: bool }`. Update `rule_tagged_disclosure_fixtures`: assert `native_discloses(text, path) == fx.native_confirms` (the native side matches its CURRENT expectation), and when the toolchain is present, cross-check `compiler_discloses(tc, path) == Some(fx.discloses)` (a `None`/indeterminate still just `eprintln!`s and skips, as today). Keep `corpus_no_false_positive_disclosures` unchanged.

- [ ] **Step 2: Write one accept + one reject fixture per source/sink/sanitizer rule.** Each is minimal and derived from the R0 index / corpus `examples/`. Verify each `discloses` value against local `compactc` in Step 3:
  - `ledger_leak.compact` (discloses:true) / `ledger_disclosed.compact` (discloses:false, `disclose()` added) — ledger sink, from spec §3.8's `setf` example.
  - ~~`emit_leak`/`emit_disclosed`~~ — **DROPPED (Rev-2.1):** `emit` does not exist in 0.31.1. Do not create these fixtures.
  - `return_witness_leak.compact` (discloses:true) / `return_arg_ok.compact` (discloses:false — returning a circuit arg is NOT a disclosure, §3.2 asymmetry).
  - `contract_call_witness_leak.compact` (discloses:**true** — contract-call is an **unconditional** sink in 0.31.1, NOT `pure?`-gated per Rev-2.1) / `contract_call_no_witness.compact` (discloses:false — a cross-contract call passing no witness data). **The reject fixture keeps `native_confirms: false` through ALL of v3a** — v3a deliberately fails closed to an amber advisory for the deferred contract-call sink (never a confirmed E-leak), so its native-confirm verdict stays false while `discloses:true` is compiler-validated. A8 separately asserts the advisory is emitted; v3b later flips it. This is the §0 fail-closed divergence, expressed cleanly by the two-field split.
  - `implicit_flow_leak.compact` (discloses:true — `if (w) { F.increment(1); }`) / `implicit_flow_disclosed.compact` (discloses:false).
  - `hash_then_leak.compact` (discloses:true — `transientHash(w)` into ledger; hashing doesn't sanitize, §3.5).
  - `constructor_arg_leak.compact` (discloses:true) — constructor-argument source.
  - `comingled_return.compact` (discloses:true) — witness-return + circuit-arg co-mingled in a returned struct (spec §8 risk #4).

- [ ] **Step 3: Capture ground truth from real `compactc`** and set each `discloses` to match; set **`native_confirms: false` for every fixture** (the native side is still a no-op). (If `compactc` is absent locally, `discloses` is derived from the R0 index and the live cross-check self-skips — same as `type_differential.rs`.)
Run: `for f in crates/compact-analyzer/tests/fixtures/disclosure/*.compact; do echo "== $f"; compact compile --skip-zk --vscode "$f" "$(mktemp -d)"; done`
Expected: `discloses:true` fixtures print the exact WPP banner (`potential witness-value disclosure must be declared but is not:` — confirm it matches `WPP_BANNER`); `discloses:false` fixtures exit 0. This is also the first LIVE validation of the R0-extracted banner string against a real reject.

- [ ] **Step 4: Run the harness — expect GREEN.** Native no-op emits nothing ⇒ `native_discloses == false == native_confirms` for every fixture; the live cross-check confirms `compiler == discloses`. So the fixtures are compiler-validated now and the suite is green.
Run: `cargo test -p compact-analyzer --test disclosure_differential --locked`
Expected: PASS (both tests). `cargo fmt --check` + `cargo clippy -p compact-analyzer --tests -- -D warnings` clean.

- [ ] **Step 5: Commit** (green, compiler-validated fixture baseline).
```bash
git add crates/compact-analyzer/tests/fixtures/disclosure crates/compact-analyzer/tests/disclosure_differential.rs
git commit -m "test(v3-R): rule-tagged disclosure fixtures (compiler-validated, green pending-flip baseline)"
```

---

## Phase v3a — Intraprocedural WPP (fail-closed)

> Each interpreter task below CONSUMES the R0 rule index and pins its behavior with the matching R2 fixture. Where an implementation walks the `compactp_ast`/`compactp_syntax` tree, Step 1 of the task is always: **read the as-merged AST API for that node kind** (the exact `Expr`/`Stmt` variant accessors) — do not assume shapes. This mirrors v2b's per-rule just-in-time discipline.
>
> **Green pending-flip protocol (R2 model):** each fixture carries `discloses` (compiler truth, fixed by R2) and `native_confirms` (starts `false`). When a v3a task implements the sink/rule that a fixture pins, it **flips that fixture's `native_confirms` to `true`** in the same commit, so `native_discloses == native_confirms` stays exact and the suite stays green. The contract-call reject fixture stays `native_confirms: false` through all of v3a (deferred to v3b; A8 asserts its amber advisory instead). Never leave `native_confirms` stale — a flipped-late fixture makes the harness assert the wrong thing.

### Task A1: `disclosure/` scaffold, `Abs` lattice, and empty query

**Files:**
- Create: `crates/analyzer-core/src/disclosure/mod.rs`, `abs.rs`, `leaks.rs`
- Modify: `crates/analyzer-core/src/lib.rs`, `crates/analyzer-core/src/analysis.rs`

**Interfaces:**
- Produces: `Abs` (+ `Witness`, `WitnessInfo`, `PathPoint`), `disclosure_diagnostics_query(db, src) -> Arc<[Diagnostic]>`, `AnalysisHost::disclosure_diagnostics(file)` (replacing the R1 stub).

- [ ] **Step 1: Write the `Abs` lattice** in `abs.rs`, translated faithfully from the verified Scheme model (spec §3.4):
```rust
/// A witness value's origin (spec §3.1). Sorted/deduped by `uid` in a set.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WitnessInfo {
    WitnessReturn { function: String },
    ConstructorArg { arg: String },
    CircuitArg { function: String, arg: String },
}

/// One tracked witness value: where it entered + the path it has taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Witness {
    pub uid: u32,
    pub src: text_size::TextRange,
    pub info: WitnessInfo,
    pub path: Vec<PathPoint>, // spec §3.7
}

/// A point on the witness→sink trail (spec §3.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathPoint {
    pub src: text_size::TextRange,
    pub description: String, // e.g. "the argument to the ledger write"
    pub exposure: String,    // e.g. "a hash of"; "" if unmodified
}

/// The abstract value lattice (spec §3.4). Struct/tuple fields individual;
/// arrays aggregate; booleans only for compile-time constants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Abs {
    Atomic(Vec<Witness>),
    Boolean { value: bool, witnesses: Vec<Witness> },
    Multiple(Vec<Abs>),
    Single(Box<Abs>),
}
```

- [ ] **Step 2: Write a failing test** for the sanitizer + join (pure logic, no salsa):
```rust
#[test]
fn disclose_clears_all_levels_and_join_unions() {
    let w = Witness { uid: 1, src: text_size::TextRange::empty(0.into()),
        info: WitnessInfo::CircuitArg { function: "f".into(), arg: "x".into() }, path: vec![] };
    let tainted = Abs::Multiple(vec![Abs::Atomic(vec![w.clone()]), Abs::Single(Box::new(Abs::Atomic(vec![w.clone()])))]);
    assert!(!witnesses_of(&disclose(&tainted)).is_empty() == false); // disclose ⇒ empty everywhere
    let joined = combine_abs(&Abs::Atomic(vec![w.clone()]), &Abs::Atomic(vec![]));
    assert_eq!(witnesses_of(&joined).len(), 1); // union, not intersection
}
```

- [ ] **Step 3: Run it to confirm it fails** (functions undefined).
Run: `cargo test -p analyzer-core disclosure::abs -- --nocapture`
Expected: FAIL (no `disclose`/`combine_abs`/`witnesses_of`).

- [ ] **Step 4: Implement `disclose` (clear `witness*` at every level), `combine_abs` (union — `merge_witnesses`, decaying `Boolean`→`Atomic` on non-matching join per §3.3), `abs_equal`, `witnesses_of`.** Pin the union direction and Boolean-decay exactly to the R0 index rows for `disclose`/`combine-abs`.

- [ ] **Step 5: Run the test — PASS.** `cargo test -p analyzer-core disclosure::abs`

- [ ] **Step 6: Add the empty salsa query + host accessor.** In `mod.rs`, mirror `type_diagnostics_query`:
```rust
#[salsa::tracked(returns(clone), no_eq)]
pub fn disclosure_diagnostics_query(db: &dyn Db, src: SourceText) -> Arc<[Diagnostic]> {
    let _tree = item_tree(db, src);
    Arc::from(Vec::new()) // filled by A2+
}
```
Replace the R1 stub in `analysis.rs` with a call to it (mirror how `type_diagnostics` calls `type_diagnostics_query`). Add `mod disclosure;` to `lib.rs`.

- [ ] **Step 7: Run the differential baseline — still green (no-op), harness unchanged.** `cargo test -p compact-analyzer --test disclosure_differential --locked corpus_no_false`

- [ ] **Step 8: Commit.**
```bash
git add crates/analyzer-core/src/disclosure crates/analyzer-core/src/lib.rs crates/analyzer-core/src/analysis.rs
git commit -m "feat(v3a): disclosure module scaffold + Abs lattice + empty query"
```

### Task A2: Root discovery + source seeding (fail-closed on Unknown type)

**Files:** Modify: `crates/analyzer-core/src/disclosure/mod.rs`, `interp.rs` (create)

**Interfaces:**
- Consumes: `item_tree(db, src)`, `circuit_node_map(db, src)`, the v2b type surface for a declared param type → `TyKind` (as used in `type_diagnostics_query`), `default<T>`-style `Abs`-shape builder.
- Produces: `seed_sources(circuit) -> Env` (param name → `Abs` seeded with the right `WitnessInfo`); `abs_of_type(kind, seed: &[Witness]) -> Abs` (the `default-value` analogue, §3.4); the root set (exported circuits + constructor).

- [ ] **Step 1: Read the as-merged item-tree API** for: is-exported flag on a circuit, the constructor node, param names+types. (Grep `id_exported`/`exported`/`constructor` in `item_tree.rs`/`resolve.rs`.)
- [ ] **Step 2: Write a failing test** `abs_of_type_builds_struct_multiple_array_single` (a struct type → `Abs::Multiple`, an array/vector → `Abs::Single`, scalar → `Abs::Atomic`), seeded witnesses at the leaves.
- [ ] **Step 3: Run → FAIL.** `cargo test -p analyzer-core disclosure::interp::abs_of_type`
- [ ] **Step 4: Implement `abs_of_type`** dispatching on `TyKind` (struct/tuple → `Multiple`, vector/array → `Single`, else `Atomic`), and **`Unknown` → record a fail-closed advisory + `Atomic([])`** (never silently untainted-without-notice — the advisory is the signal). Implement `seed_sources` (exported-circuit params → `CircuitArg`; constructor params → `ConstructorArg`) and root discovery.
- [ ] **Step 5: Run → PASS.**
- [ ] **Step 6: Commit.** `feat(v3a): root discovery + type-driven source seeding (Unknown⇒advisory)`

### Task A3: Intraprocedural interpreter — expression/statement walk + `disclose()`

**Files:** Modify: `crates/analyzer-core/src/disclosure/interp.rs`

**Interfaces:**
- Consumes: A1 `Abs`/`combine_abs`/`disclose`, A2 seeding, the `compactp_ast` expr/stmt API, the R0 index.
- Produces: `interp_expr(env, control: &[Witness], expr) -> Abs` and `interp_stmt(...)` covering: identifiers/const bindings (path point), `disclose(e)` (sanitize), literals (`Atomic([])`), tuples/structs (`Multiple`), member/index access (project `Multiple`/`Single`), casts/serialize (passthrough), comparisons (`Atomic`, union), `if` (branch/join per §3.3), `default<T>` (empty). **Every unhandled expr kind → fail-closed advisory + a conservative `Atomic(union-of-subexpr-witnesses)`** (never drop taint).

- [ ] **Step 1: Read the as-merged `compactp_ast` expr/stmt variant API.** List the `Expr` variants and their accessors; note which the R0 index covers.
- [ ] **Step 2: Write failing tests** (one per handled form) as **pure interpreter unit tests** over parsed fixtures: `disclose_sanitizes`, `member_projects_struct_field`, `cast_passthrough`, `comparison_unions_atomic`, `if_nonconstant_joins_both`, `unhandled_form_emits_advisory_and_keeps_taint`.
- [ ] **Step 3: Run → FAIL.**
- [ ] **Step 4: Implement the walk**, one match arm per R0 index row; the catch-all arm emits the advisory and returns the conservative union. Each arm adds a `PathPoint` where the index says so.
- [ ] **Step 5: Run → PASS.**
- [ ] **Step 6: Commit.** `feat(v3a): intraprocedural expr/stmt interpreter + disclose sanitizer (unhandled⇒advisory)`

### Task A4: Sinks — ledger, return (with asymmetry filter); confirmed-leak diagnostics
> **Rev-2.1:** `emit` is dropped (absent in 0.31.1). This task implements the ledger and return sinks only. The `PathPoint` example wording "the argument to emit" (A1) becomes e.g. "the argument to the ledger write".

**Files:** Modify: `crates/analyzer-core/src/disclosure/interp.rs`, `leaks.rs`, `mod.rs`

**Interfaces:**
- Consumes: A3 interpreter, the R0 sink rows + nature strings, the root-drained leak table.
- Produces: `record_leak(table, src, nature, witnesses)`; sink handling for ledger write/update and `return`; `filter_witnesses` (return asymmetry, §3.2); `LeakTable::drain() -> Vec<DisclosureLeak>`; `leak_to_diagnostic(leak) -> Diagnostic` (E3100, wording per §3.8, `secondary_spans` = source + path points).

- [ ] **Step 1: Read** how ledger writes / `return` appear in the AST (statement vs expr position).
- [ ] **Step 2: Write failing differential-backed tests:** wire `disclosure_diagnostics_query` to run the interpreter from each root and drain the table; assert the R2 fixtures `ledger_leak`, `return_witness_leak` now produce an E3100, and `return_arg_ok` does NOT (asymmetry). **Flip `native_confirms → true`** on the fixtures this task turns confirmable (`ledger_leak`, `return_witness_leak`, `constructor_arg_leak` [constructor source seeded in A2 + ledger sink here], `comingled_return` [return sink + filter]) in the SAME commit. Leave `hash_then_leak` (needs A6 conduit) and `implicit_flow_leak` (A5) unflipped.
- [ ] **Step 3: Run → FAIL** (native emits nothing yet, but `native_confirms` is now true for the flipped fixtures ⇒ mismatch).
- [ ] **Step 4: Implement** `record_leak` (dedup by `(src, nature)`, union witnesses — §4.2), the **two sinks** (ledger write/update, return), `filter_witnesses` (keep only `WitnessReturn` at the return sink), the leak table drain in the query, and `leak_to_diagnostic` with related-info `secondary_spans`.
- [ ] **Step 5: Run the interpreter tests + the differential fixtures.**
Run: `cargo test -p analyzer-core disclosure && cargo test -p compact-analyzer --test disclosure_differential --locked rule_tagged`
Expected: GREEN — `ledger_leak`/`return_witness_leak`/`constructor_arg_leak`/`comingled_return` now native-confirm (matching their flipped `native_confirms:true`); `*_ok`/`*_disclosed` still accept; `hash_then_leak`/`implicit_flow_leak` still `native_confirms:false` (green, pending A6/A5).
- [ ] **Step 6: Commit.** `feat(v3a): ledger/return sinks + confirmed-leak diagnostics with path related-info`

### Task A5: Implicit flow — threaded control witnesses + separate control-leaks

**Files:** Modify: `crates/analyzer-core/src/disclosure/interp.rs`

**Interfaces:**
- Consumes: A3/A4, R0 rows for `if`/`&&`/`||` + the control nature strings (§3.2).
- Produces: `control: &[Witness]` threading through `interp_*`; at each sink, a SECOND `record_leak` keyed on the control witnesses with the control nature string.

- [ ] **Step 1: Write a failing test** on `implicit_flow_leak.compact` (`if (w) { F.increment(1); }`): expect a control-flow leak with nature "performing this ledger operation" and NO taint on the written literal.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** control-witness merge at `if`/`&&`/`||` (union the condition's witnesses into `control`), and the second `record_leak` at every sink (§3.3). Confirm the written-value leak and the control leak are distinct table entries.
- [ ] **Step 4: Run → PASS** + differential `implicit_flow_leak`/`implicit_flow_disclosed`.
- [ ] **Step 5: Commit.** `feat(v3a): implicit-flow control-witness leaks (separate from data leaks)`

### Task A6: Native conduit vs ledger sink flags (fail-closed on unknown)

**Files:** Modify: `crates/analyzer-core/src/disclosure/interp.rs`; the native/ledger flag tables (extracted in R0) as a data module.

**Interfaces:**
- Consumes: R0 native `disclose?*` + ledger `discloses?` tables; the item tree's callee classification (circuit/witness/native).
- Produces: native-call handling (fold flagged args' witnesses into the return `Abs` with the exposure `PathPoint`; NO leak — §3.6); ledger-op handling (flagged arg ⇒ immediate leak). **Unknown native ⇒ conduit-taint all args into result + advisory; unknown ledger op ⇒ treat as sink + advisory** (§0/§3.6).

- [ ] **Step 1: Encode the native/ledger flag tables** as a Rust lookup (from R0), each entry `name → per-arg (Option<&str exposure>)`.
- [ ] **Step 2: Failing test** `transientHash_is_conduit_not_leak` (`hash_then_leak.compact`): the hash call records NO leak; the ledger write of its result DOES (E3100), path shows "a hash of".
- [ ] **Step 3: Run → FAIL.**
- [ ] **Step 4: Implement** both mechanisms + the unknown-fail-closed defaults + advisory.
- [ ] **Step 5: Run → PASS** + differential.
- [ ] **Step 6: Commit.** `feat(v3a): native conduit vs ledger sink flags (unknown⇒fail-closed advisory)`

### Task A7: Intraprocedural `fold`/`map` fixpoint over aggregates

**Files:** Modify: `crates/analyzer-core/src/disclosure/interp.rs`

**Interfaces:**
- Consumes: A1 `abs_equal`/`combine_abs`, R0 `fold`/`map` rows (§3.9): `Abs::Multiple` (tuple/struct) unrolls; `Abs::Single` (array) iterates to an `abs_equal` fixed point bounded by length.
- Produces: `interp_fold` / `interp_map` with the bounded fixpoint loop.

- [ ] **Step 1: Read** the `map`/`fold` AST shape; **Step 2:** failing test `fold_over_witness_array_reaches_fixpoint` (a fold accumulating witness taint stabilizes, and a leak of the accumulator is flagged). **Step 3:** FAIL. **Step 4:** implement the `abs_equal`-bounded loop (aggregate) + full unroll (heterogeneous). **Step 5:** PASS. **Step 6:** commit `feat(v3a): intraprocedural fold/map fixpoint over aggregates`.

### Task A8: The amber "unverified" advisory channel (fail-closed made visible)

**Files:** Modify: `crates/analyzer-core/src/disclosure/leaks.rs`, `crates/compact-analyzer/src/lsp_utils.rs:66-70`; `compactp_diagnostics` `Severity` OR an analyzer-core-local advisory flag.

**Interfaces:**
- Consumes: every `emit_advisory(src, reason)` call sites from A2/A3/A6 (Unknown type, unhandled expr, unknown native/ledger, deferred cross-contract sink).
- Produces: advisory `Diagnostic`s (`U3100`, `Severity::Advisory`) → `DiagnosticSeverity::WARNING`, `source = "compact-analyzer (unverified)"`, `codeDescription` link; the LSP mapping.

- [ ] **Step 1: Read `compactp_diagnostics` `Severity`** (3 variants: Error/Warning/Note). Decide: add an `Advisory` variant (pre-release/no-back-compat ⇒ OK if we own the crate) OR an analyzer-core-local wrapper. Record the decision.
- [ ] **Step 2: Failing test** in `lsp_utils.rs` tests: an advisory diagnostic maps to `DiagnosticSeverity::WARNING` with `source == "compact-analyzer (unverified)"` and a `U`-family code.
- [ ] **Step 3: Run → FAIL.**
- [ ] **Step 4: Implement** the `Severity::Advisory → WARNING` mapping (extend the match at `lsp_utils.rs:66-70`) + advisory construction in `leaks.rs`. Ensure advisories are excluded from the differential `native_discloses` verdict (only E-family counts — already coded in R1).
- [ ] **Step 5: Failing test** `deferred_cross_contract_call_emits_advisory_not_green`: `impure_call_leak.compact` in v3a produces a `U3100` advisory (cross-contract sink deferred to v3b), NOT silent green and NOT a false-positive E-leak.
- [ ] **Step 6: Implement** the deferred-sink advisory at the contract-call site; run → PASS.
- [ ] **Step 7: Run the full differential + corpus gate.**
Run: `COMPACT_CORPUS_DIR=../compactp/tests/corpus cargo test -p compact-analyzer --test disclosure_differential --locked`
Expected: `rule_tagged_disclosure_fixtures` GREEN; `corpus_no_false_positive_disclosures` GREEN (advisories don't count as leaks ⇒ no false positives).
- [ ] **Step 8: Commit.** `feat(v3a): amber unverified-advisory channel; deferred cross-contract sink ⇒ advisory`

### Task A9: Wire disclosure diagnostics into the editor surface + e2e smoke

**Files:** Modify: `crates/compact-analyzer/src/server.rs` (diagnostic aggregation), `crates/analyzer-core/src/analysis.rs`.

**Interfaces:**
- Consumes: `AnalysisHost::disclosure_diagnostics`.
- Produces: disclosure diagnostics merged into the published set alongside resolution/type diagnostics, distinctly `source`-tagged; the one-directional merge invariant (native absence never suppresses a compactc diagnostic — that path is compile-on-save, separate source).

- [ ] **Step 1: Read** how `server.rs` aggregates and publishes diagnostics (where `type_diagnostics` is merged).
- [ ] **Step 2: Failing test** (server-level, mirroring existing diagnostic tests): a file with a ledger leak publishes an `E3100` error diagnostic at the sink with related-info; a file with an unknown-native call publishes a `U3100` amber advisory.
- [ ] **Step 3: Run → FAIL. Step 4: Implement** the merge (add `disclosure_diagnostics` to the published set). **Step 5: Run → PASS.**
- [ ] **Step 6: Full gate.**
Run: `cargo test --workspace --locked && cargo clippy --workspace --locked && cargo fmt --check`
Expected: all green.
- [ ] **Step 7: Commit.** `feat(v3a): publish disclosure diagnostics (E leaks + U advisories) in the LSP surface`

---

## Self-Review (against spec Revision 2)

- **§0 fail-closed:** A2 (Unknown type), A3 (unhandled expr), A6 (unknown native/ledger), A8 (deferred cross-contract) all emit advisories — covered. Corpus gate counts only E-family ⇒ advisories never cause false positives.
- **§3.1 sources:** A2 seeds all three. **§3.2 sinks:** A4 (ledger/return + asymmetry; emit out of scope per Rev-2.1), A8 (cross-contract deferred⇒advisory). **§3.3 implicit flow:** A5. **§3.4 granularity:** A1/A2 `Abs`. **§3.5 sanitizer:** A1/A3 `disclose`. **§3.6 flags:** A6. **§3.9 fold fixpoint:** A7.
- **§4.3 amber channel:** A8. (Advisory-UX hover copy, quick-fix, toggle, version check, transient-parse retention are **v3c** — not in this plan; correct per decomposition.)
- **§5 differential:** R1/R2 + gates in A4/A8.
- **Placeholder scan:** interpreter tasks (A3–A7) intentionally pin implementation to the R0 index + a fixture + an "read the as-merged AST API" step rather than inlining fabricated match arms over an unread AST — this is the repo's verify-first just-in-time discipline, not a placeholder. Every task has a concrete failing test, a named source clause, and a commit.
- **Type consistency:** `Abs`/`Witness`/`WitnessInfo`/`PathPoint` defined in A1 and used unchanged through A9; `disclosure_diagnostics_query`/`disclosure_diagnostics`/`record_leak`/`emit_advisory`/`leak_to_diagnostic` names stable across tasks.

## Execution Handoff

Two execution options (choose at execution time): **Subagent-Driven** (fresh subagent per task + two-stage review — recommended, given the fail-closed correctness bar) or **Inline Execution** (batch with checkpoints). Before executing v3a's interpreter tasks (A3+), the R0 rule index MUST exist.
