# v3b + v3c — Interprocedural Disclosure + UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend v3a's intraprocedural native Witness-Protection-Program (WPP) analyzer to full **interprocedural** taint (cross-circuit call propagation, module resolution) reaching corpus differential parity with `compactc`, then polish the diagnostics into a best-in-class editor UX (toggle, `disclose()` quick-fix, path rendering, advisory contract, version check, transient-parse retention, in-editor e2e).

**Architecture:** v3b keeps the existing single whole-file salsa query `disclosure_diagnostics_query` and makes cross-circuit calls **inline-interpreted** — the `CalleeClass::Circuit` arm recursively walks the callee body under the caller's abstract args + threaded control witnesses, with the return `Abs` flowing back and callee-body leaks recorded into the same root-drained `DisclosureSink` (mirroring the compiler's global `record-leak!`). A **walk-local `call-ht` memo** (keyed `(symbol, arg_shapes, control)`) plus a **recursion guard** (fail-closed amber on any cycle) bound the walk. Non-root callees run with `disclosing_fn = None` so the K7 return filter behaves. v3c gates the channel behind a `disclosureDiagnostics` toggle, adds a `textDocument/codeAction` quick-fix, and hardens the advisory/version/retention UX.

**Tech Stack:** Rust (analyzer-core `disclosure/` module, compact-analyzer LSP server, analyzer-toolchain), salsa; TypeScript (VS Code extension); `compactc` 0.31.1 as the differential oracle.

## Global Constraints

- **Target compiler 0.31.1 / language 0.23.0** = tag `compactc-v0.31.1` = commit `0da5b0452eb0c1053d42418bf34b12cc29c7d63e` of `LFDT-Minokawa/compact`. This is the ONLY trusted source of WPP truth — training-data knowledge of Compact is FORBIDDEN.
- **§0 FAIL CLOSED is the primary invariant:** every point the analyzer cannot fully decide → an amber `U`-family advisory (`Severity::Warning`, `U3100`, LSP source `"compact-analyzer (unverified)"`), NEVER a silent green (false negative) and NEVER a false-positive red (`E3100`).
- **Differential discipline:** the analyzer "agrees-with-`compactc`" only — no independent soundness claim. The corpus **no-false-positive gate** (`corpus_no_false_positive_disclosures`) is the primary guard; run it in the BACKGROUND (~5 min, 491 files, self-skips without a corpus). Corpus = clone `LFDT-Minokawa/compact`, point `COMPACT_CORPUS_DIR` at its `examples/`; local `compact` 0.31.1 is the arbiter.
- **Verify-first:** every WPP rule is extracted from compiler source at the pinned tag AND pinned by a `compactc`-differential fixture BEFORE implementation.
- **`emit` and cross-contract-call SINKS do not exist / are unsupported in 0.31.1** — keep them fail-closed to amber; do NOT make them confirmed leaks under this target.
- **Pre-release / no back-compat:** clean wholesale replacement is fine; no shims/migrations/dual paths.
- **Recursion is forbidden upstream** (`reject-recursive-circuits` runs before the WPP) — the analyzer must still fail-closed (amber, terminating) if a mid-edit/malformed program presents a cycle, never infinite-loop.
- **Specialist subagents** (per project convention): `midnight-verify:source-investigator` for compiler-source extraction; `devs:rust-dev` for analyzer-core/server Rust; `compact-core:compact-dev` for `.compact` fixtures; `devs:typescript-dev` for the VS Code extension; `devs:code-reviewer` for reviews (OPUS for the interpreter/memoization tasks B2/B3/B4 and the final whole-branch review; sonnet for mechanical tasks). New fixture dirs MUST be added to `.claude/compact-check.json` `exclude` (leak fixtures are intentionally compiler-rejected).

---

## File Structure

**v3b (interprocedural) — analyzer-core `crates/analyzer-core/src/disclosure/`:**
- `interp.rs` — the interpreter. Modified: `interp_call` `CalleeClass::Circuit` arm (recursive callee walk), `classify_callee`/`unresolved_callee_class` (module resolution), `discover_roots`/`is_module_nested` (module-nested roots), a new `interp_circuit_call` fn, a new walk-local `CallCache` + recursion guard threaded through the walk.
- `mod.rs` — the query. Modified: thread the `CallCache`/recursion-guard state through `interp_root`.
- `crates/compact-analyzer/tests/disclosure_differential.rs` — Modified: add Tier-2b reject-direction parity assertion + interprocedural fixtures.
- `crates/compact-analyzer/tests/fixtures/disclosure/` — new interprocedural + module fixtures.

**v3c (UX):**
- `crates/compact-analyzer/src/server.rs` — `disclosureDiagnostics` toggle in `ToolchainConfig` + `toolchain_config_from`; `codeActionProvider` capability + `textDocument/codeAction` handler; version-check advisory; transient-parse retention.
- `crates/compact-analyzer/src/lsp_utils.rs` — `codeDescription` on U-family diagnostics; quick-fix edit construction; stdlib-collapse of path points.
- `crates/analyzer-core/src/disclosure/mod.rs` / `leaks.rs` — advisory wording; quick-fix span metadata on leaks; stdlib-collapse.
- `crates/analyzer-toolchain/src/discovery.rs` or a new small module — the pinned-version constant + comparison for the version check.
- `editors/vscode/package.json`, `editors/vscode/src/config-core.ts` — the `disclosureDiagnostics` setting; e2e in `editors/vscode/src/test/e2e/`.

---

# PHASE v3b — Interprocedural WPP

## Task B0: Re-confirm the compiler's interprocedural call model @ 0.31.1 (verify-first gate)

**Dispatched during planning** to `midnight-verify:source-investigator` (brief: `.superpowers/sdd/task-B0-brief.md`; findings: `.superpowers/sdd/task-B0-findings.md`). This is the verify-first gate for all of v3b — B2/B3/B4 consume its findings. **Do not start B2 until `task-B0-findings.md` exists and confirms (or corrects) the model below.**

**Files:**
- Create: `.superpowers/sdd/task-B0-findings.md` (the subagent writes this)

**Interfaces:**
- Produces (the confirmed model B2+ rely on): `handle-call` threads the caller's `control-witness*` into the callee body and returns the callee body's abstract value to the caller; `call-ht` memoizes the returned `Abs` on key `(circuit-uid, abs*, control-witness*)`, HIT returns cached abs without re-walking; the disclosing-context gate makes exported-circuit roots bypass the cache (only roots set `disclosing-function-name?`; non-root callees run `#f`); recursion is rejected pre-WPP; leaks are a **global side effect** (so memoization is a performance optimization, not a correctness requirement, under `(src,nature)` dedup); the cross-circuit-call **argument path-point** wording; and re-confirmation that `contract-call` is unsupported in 0.31.1 (version-gated amber).

- [ ] **Step 1: Confirm the findings file exists and is decisive**

Run: `cat /home/devbox/projects/compact-analyzer/.superpowers/sdd/task-B0-findings.md`
Expected: a per-question CONFIRMED/REFUTED/NUANCED verdict with `analysis-passes.ss:<line>` anchors at `0da5b045`, and a bottom-line judgment on whether "recursively inline each callee body under the caller's abstract args + threaded control, return abs flows back, `disclosing_fn=None` for non-root callees, memoization as a walk-local `(uid,abs*,control)` performance cache" is a faithful 0.31.1 model.

- [ ] **Step 2: Reconcile any correction into this plan**

If B0 REFUTES or NUANCES any load-bearing point (esp. the cross-circuit path-point wording in Task B3, or whether the caller's control threads into the callee in B2), edit the affected task's code/strings in this plan file to match the verbatim source before implementing it. Record the reconciliation in `.superpowers/sdd/progress-v3bc.md`. No commit (planning artifact only).

**B0 RESULT (2026-07-18, `task-B0-findings.md`) — reconciled:**
- **CONFIRMED:** memo key `(uid, abs*, control)`; caller's `control-witness*` threads unchanged into the callee; callee's return `Abs` flows back; `disclosing_fn=None` for ALL non-root callees (only the exported-circuit root passes `#t`; both nested dispatch sites — the `call` expression and the `fref` used by `map`/`fold` — pass `#f`); recursion rejected pre-WPP (`Call-inprocess ⇒ assert cannot-happen`), so the active-set guard in B2 is the fail-closed analogue; leaks are a global side effect ⇒ memoization is a performance optimization (idempotent under `(src,nature)` dedup), the disclosing-gate carve-out being the sole non-performance role and it applies only to roots (which we don't memoize).
- **CORRECTION (already reflected in B2):** the callee runs under a **FRESH `empty-env`** extended with `param→arg` — NOT the caller's env. B2 builds a fresh `callee_env: Env = HashMap::new()`; do NOT inherit the caller's scope. ✓
- **Cross-circuit arg path-point (B3):** CONFIRMED verbatim — same `add-path-point` + `"the ~@[~:r ~]argument to ~a"` template as natives (v3a's `arg_description`), with an EMPTY exposure `""` (descriptive-only, not a taint gate). Single-arg call → no ordinal ("the argument to `f`"); multi-arg → 1-based ordinals. ✓
- **REFUTED — contract-call (K3–K5):** B0 claims `contract-call` from an impure, non-constructor circuit IS supported + WPP-processed in 0.31.1 (only pure-circuit and constructor callers are rejected), contradicting R0/spec/handover. This is under EMPIRICAL settlement (`task-Q7-brief.md` → `task-Q7-findings.md`, dispatched to `compact-core:compact-dev` against local `compactc` 0.31.1). **Task B7 (below) is CONDITIONAL on that result.** Until settled, contract-call stays a fail-closed amber advisory (the handover's constraint holds by default).

---

## Task B1: Memoization-architecture spike (resolve spec §8 risk #2 FIRST)

Decide, with a written ADR, whether v3b uses **walk-local `call-ht` memoization inside the existing whole-file query** (the sound baseline) or a **separate `#[salsa::tracked] circuit_abstract` query**. The load-bearing open question (spec §8 risk #2): does a coarser, differential-equivalent summary than full per-call-site memoization exist? Per-call-site is the sound baseline; do not assume a coarser one works.

**Files:**
- Create: `docs/superpowers/plans/2026-07-18-v3b-memoization-adr.md`

**Interfaces:**
- Produces: the decision B2/B3 implement. Default recommendation (validate or overturn against B0's findings): **walk-local `call-ht`**, because (a) leaks are a global side effect (B0 Q6) so a pure salsa query would have to return `(Abs, leaks, advisories, uid-delta)` and re-thread uid determinism — complexity with no correctness gain; (b) the existing `disclosure_diagnostics_query` is ALREADY whole-file-keyed, so a per-call-site salsa query adds ~zero intraprocedural incrementality; (c) recursion is forbidden (B0 Q4) so the call tree is finite and full inlining terminates — memoization is purely a performance bound; (d) it mirrors the compiler bit-for-bit.

- [ ] **Step 1: Write the ADR** capturing: the two options; B0's leak-side-effect + recursion findings; the incrementality analysis (whole-file query already reruns wholesale on edit); the differential-equivalence argument (walk-local `call-ht` records the SAME `(src,nature)`-keyed leaks the compiler's global table does, so corpus verdicts are identical); and the decision + rationale. Include a "revisit if" trigger (e.g. cross-file summary sharing becomes necessary for corpus parity in B4/B5).

- [ ] **Step 2: Self-check the equivalence claim** — enumerate the cases where walk-local memoization could diverge from full inlining: (a) a callee reached with two different `(arg_shapes, control)` keys must NOT share a cache entry (key includes both — confirm); (b) the disclosing-context gate (roots bypass cache) — confirm roots are never memoized in our model (they are the top-level drivers, never cache-looked-up); (c) idempotent leak re-recording under `(src,nature)` dedup means even a memo MISS that re-walks is safe. Write these into the ADR as the correctness argument.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/plans/2026-07-18-v3b-memoization-adr.md
git commit -m "docs(v3b): ADR — walk-local call-ht memoization for interprocedural WPP"
```

---

## Task B2: Cross-circuit call interpretation (the core)

Replace the `CalleeClass::Circuit` fail-closed amber arm with recursive interpretation of the callee body. This is the heart of v3b. **OPUS rust-dev + OPUS reviewer.**

**Files:**
- Modify: `crates/analyzer-core/src/disclosure/interp.rs` — `interp_call` (`CalleeClass::Circuit` arm, ~L661); add `interp_circuit_call`; extend `CalleeClass::Circuit` to carry the resolved callee's `(file, symbol)`; add a recursion-guard set + call-depth to the walk state.
- Modify: `crates/analyzer-core/src/disclosure/mod.rs` — construct + thread the walk state.
- Create: `crates/compact-analyzer/tests/fixtures/disclosure/interproc_call_leak.compact` (reject), `.../interproc_call_ok.compact` (accept, callee `disclose()`s).
- Test: `crates/analyzer-core/src/disclosure/interp.rs` (unit) + `crates/compact-analyzer/tests/disclosure_differential.rs` (fixture rows, wired in B5).

**Interfaces:**
- Consumes (from v3a, confirmed in merged code): `Abs`, `Witness`, `WitnessInfo`, `PathPoint` (`abs.rs`); `DisclosureSink` with `record_leak`/`emit_advisory`/`next_witness_uid`/`drain_*` (`leaks.rs`); `InterpCtx { db, file, src, fd, ws, disclosing_fn }` (Copy); `interp_expr(ctx, sink, env, control, expr) -> Abs`; `interp_stmt(ctx, sink, env, control, stmt)`; `seed_sources(...)`; `abs_of_type(sink, &TyKind, &[Witness]) -> Abs`; `resolve_query`/`item_symbol`/`item_tree`; `Env = HashMap<String, Abs>`.
- Produces (for B3/B4): `CalleeClass::Circuit { file: FileId, symbol: u32 }`; `fn interp_circuit_call(ctx, sink, state, env, control, call, callee_file, callee_symbol, src) -> Abs`; a `WalkState { cache: CallCache, active: HashSet<(FileId,u32)>, depth: u32 }` threaded through `interp_expr`/`interp_stmt` (B3 fills `cache`; B2 uses `active`+`depth` for the recursion guard).

**Design (faithful to B0's confirmed model — reconcile strings/threading with `task-B0-findings.md`):**
1. `classify_callee` returns `CalleeClass::Circuit { file, symbol }` when the callee resolves to a `SymbolKind::Circuit` (same-file for B2; cross-module in B4).
2. `interp_circuit_call`: interpret each argument expression → `arg_shapes: Vec<Abs>` (keeping taint). Locate the callee's `CircuitDef` (params + body) from `(file, symbol)`. If param count ≠ arg count, or body/params can't be read → **fail closed** (amber advisory + clean `Atomic([])`), do not guess.
3. **Recursion guard:** if `(callee_file, callee_symbol)` is already in `state.active` (or `state.depth` exceeds a hard bound) → amber advisory `"recursive/deeply-nested circuit call not analyzed"` + clean result. Otherwise insert into `active`, recurse, remove on return.
4. Build a fresh `Env` binding each param name → its `arg_shape`. Interpret the callee body with `disclosing_fn = None` (non-root callee: its `return` must NOT leak — the K7 filter) and the CALLER's `control` threaded in (B0 Q1). Capture the callee's returned `Abs` (the body's value) and return it to the caller so caller-side taint propagates. Callee-body ledger/return sinks record into the shared `sink`.

- [ ] **Step 1: Write the failing differential fixtures**

`interproc_call_leak.compact` — an exported circuit passes a witness-derived value to a non-exported helper circuit that writes it to the ledger (compactc REJECTS with the WPP banner):

```compact
pragma language_version >= 0.23;
import CompactStandardLibrary;

export ledger store: Field;

circuit stash(v: Field): [] { store = v; }

export circuit leak(secret: Field): [] { stash(secret); }
```

`interproc_call_ok.compact` — the helper `disclose()`s before the write (compactc ACCEPTS):

```compact
pragma language_version >= 0.23;
import CompactStandardLibrary;

export ledger store: Field;

circuit stash(v: Field): [] { store = disclose(v); }

export circuit leak(secret: Field): [] { stash(secret); }
```

Validate both against real `compactc` (dispatch `compact-core:compact-dev` or run `compact compile`): leak → REJECT with `"potential witness-value disclosure must be declared but is not:"`; ok → ACCEPT. Add both dirs' parent to `.claude/compact-check.json` if not already excluded (it is: `tests/fixtures/disclosure/`).

- [ ] **Step 2: Write the failing unit test** (`interp.rs` tests)

```rust
#[test]
fn cross_circuit_call_propagates_taint_to_callee_ledger_sink() {
    // leak(secret) -> stash(secret) -> `store = v` is a confirmed E3100 leak.
    let db = crate::db::CompactDatabase::default();
    let text = "import CompactStandardLibrary;\n\
                export ledger store: Field;\n\
                circuit stash(v: Field): [] { store = v; }\n\
                export circuit leak(secret: Field): [] { stash(secret); }\n";
    let (host, file) = crate::disclosure::tests_support::host_with(&db, text); // see Step 3
    let diags = host_disclosure_e_leaks(&host, file);
    assert!(!diags.is_empty(), "cross-circuit witness->ledger must confirm a leak");
}

#[test]
fn cross_circuit_callee_disclose_sanitizes() {
    let db = crate::db::CompactDatabase::default();
    let text = "import CompactStandardLibrary;\n\
                export ledger store: Field;\n\
                circuit stash(v: Field): [] { store = disclose(v); }\n\
                export circuit leak(secret: Field): [] { stash(secret); }\n";
    let (host, file) = crate::disclosure::tests_support::host_with(&db, text);
    assert!(host_disclosure_e_leaks(&host, file).is_empty(),
        "callee disclose() must sanitize — no leak");
}
```

(If a `tests_support` helper does not already exist, inline the `disclosure_diagnostics_query` call as `mod.rs`'s existing tests do — construct `SourceText`/`FileDeps`/`empty_ws`/`FileId::from_raw_for_test(0)` and filter `d.code.prefix == "E" && d.code.number >= 3100`.)

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p analyzer-core disclosure::interp::tests::cross_circuit -- --nocapture`
Expected: FAIL — the current `CalleeClass::Circuit` arm returns clean + amber, so no E-leak is confirmed (first test fails).

- [ ] **Step 4: Implement — extend `CalleeClass::Circuit` and add `interp_circuit_call`**

In `classify_callee`, carry the resolved target:

```rust
enum CalleeClass {
    Witness { name: String, ret: TyKind },
    Circuit { file: crate::FileId, symbol: u32 },
    Native,
}
```

Populate it in `classify_callee` (the `SymbolKind::Circuit` arm) with `df` (the resolved def file) and the symbol index. Thread a `&mut WalkState` through `interp_expr`/`interp_stmt`/`interp_call` (add the parameter; update all call sites). Replace the `CalleeClass::Circuit` arm of `interp_call`:

```rust
CalleeClass::Circuit { file: cf, symbol } => {
    interp_circuit_call(ctx, sink, state, env, control, call, cf, symbol, src)
}
```

Add `interp_circuit_call` (faithful to B0 Q1/Q3 — a non-root callee runs `disclosing_fn = None`, caller `control` threads in, return abs flows back):

```rust
fn interp_circuit_call(
    ctx: &InterpCtx, sink: &mut DisclosureSink, state: &mut WalkState,
    env: &Env, control: &[Witness], call: &CallExpr,
    callee_file: crate::FileId, callee_symbol: u32, src: TextRange,
) -> Abs {
    // Recursion / depth guard (§0 fail-closed, terminating).
    let key = (callee_file, callee_symbol);
    if state.depth >= MAX_CALL_DEPTH || state.active.contains(&key) {
        sink.emit_advisory(src, "recursive or deeply-nested circuit call not analyzed".to_string());
        return Abs::Atomic(Vec::new());
    }
    // Resolve the callee's params + body (same-file for B2; B4 adds cross-module).
    let Some(callee) = resolve_circuit_def(ctx, callee_file, callee_symbol) else {
        sink.emit_advisory(src, "circuit call target could not be resolved".to_string());
        return Abs::Atomic(Vec::new());
    };
    let arg_shapes: Vec<Abs> = arg_exprs(call).iter()
        .map(|a| interp_expr(ctx, sink, state, env, control, a)).collect();
    if arg_shapes.len() != callee.params.len() {
        sink.emit_advisory(src, "circuit call arity mismatch not analyzed".to_string());
        return Abs::Atomic(Vec::new());
    }
    // (B3 inserts the call-ht memo lookup HERE, keyed (key, arg_shapes, control).)
    let mut callee_env: Env = HashMap::new();
    for (p, a) in callee.params.iter().zip(arg_shapes.iter()) {
        callee_env.insert(p.clone(), a.clone()); // (B3 adds the arg path-point)
    }
    let callee_ctx = InterpCtx { file: callee_file, disclosing_fn: None, ..*ctx };
    state.active.insert(key);
    state.depth += 1;
    let ret = interp_body_returning(&callee_ctx, sink, state, &mut callee_env, control, &callee.body);
    state.depth -= 1;
    state.active.remove(&key);
    ret
}
```

Notes for the implementer: `resolve_circuit_def` mirrors `root_body`/`param_name_and_type` (locate the `CircuitDef` by the symbol's `name_range`, read params + body). `interp_body_returning` walks the body block and returns the `Abs` of its `return`/tail expression — factor this out of `interp_stmt`'s block handling so a callee's returned value is capturable (the root path can keep discarding it). `MAX_CALL_DEPTH` is a defensive constant (e.g. 256) — recursion is forbidden upstream, so the `active`-set guard is the real cycle catch; the depth bound is belt-and-suspenders for pathological nesting. `WalkState { cache: CallCache, active: HashSet<(FileId,u32)>, depth: u32 }` is threaded from `interp_root` (Step 5).

- [ ] **Step 5: Thread `WalkState` from the query**

In `interp_root` (or `disclosure_diagnostics_query`), construct one `WalkState::default()` per root walk and pass `&mut state` into `interp_stmt`. (Cache is per-analysis-run; sharing it across roots in one file is fine and matches the compiler's single `call-ht`.) Update `mod.rs` accordingly.

- [ ] **Step 6: Run the unit + fixture tests to verify they pass**

Run: `cargo test -p analyzer-core disclosure && cargo test -p compact-analyzer --test disclosure_differential rule_tagged`
Expected: PASS — cross-circuit leak confirmed, callee `disclose()` sanitizes, existing 45 disclosure tests still green.

- [ ] **Step 7: Run clippy + fmt + the corpus gate in the background**

Run (foreground): `cargo clippy -p analyzer-core --all-targets -- -D warnings && cargo fmt --check`
Run (background): `COMPACT_CORPUS_DIR=<corpus>/examples cargo test -p compact-analyzer --test disclosure_differential corpus_no_false_positive -- --nocapture`
Expected: clippy/fmt clean; corpus gate `false_positives=0` (interprocedural interpretation must not introduce a false positive — a callee that `disclose()`s must clear taint before the caller's sink).

- [ ] **Step 8: Commit**

```bash
git add crates/analyzer-core/src/disclosure/ crates/compact-analyzer/tests/fixtures/disclosure/interproc_call_*.compact
git commit -m "feat(disclosure): interprocedural cross-circuit call interpretation (v3b B2)"
```

---

## Task B3: Walk-local `call-ht` memoization + cross-circuit path points

Add the memo cell (performance bound per B1's ADR) and the witness→sink trail across the call boundary. **OPUS reviewer.**

**Files:**
- Modify: `crates/analyzer-core/src/disclosure/interp.rs` — `WalkState.cache`; memo lookup/insert in `interp_circuit_call`; arg path-point on the bound param.
- Create: `crates/compact-analyzer/tests/fixtures/disclosure/interproc_path.compact` (reject; asserts the path renders across the call).
- Test: `interp.rs` unit tests.

**Interfaces:**
- Consumes: `interp_circuit_call` (B2), `add_path_point` / `PathPoint` (v3a).
- Produces: `type CallCache = HashMap<(crate::FileId, u32, Vec<Abs>, Vec<Witness>), Abs>` (the walk-local `call-ht`; `Abs`/`Witness` derive `Hash` for the key — add derives if missing, or use a stable serialization of the key). Cross-circuit path-point wording = the string B0 Q5 confirms (default assumption: `"the Nth argument to <f>"`, mirroring the native N1 `add-path-point`).

- [ ] **Step 1: Write the failing memo test** — a helper called twice with the same `(arg_shapes, control)` is interpreted once:

```rust
#[test]
fn call_ht_memoizes_identical_call_context() {
    // Instrument via a counter or by asserting a single leak row (dedup) — the
    // observable contract: two identical-context calls to a leaking helper yield
    // exactly ONE (src,nature) leak entry, and the second call is a cache hit.
    // (Assert the leak table has one row for the helper's ledger sink.)
}
```

Prefer an observable assertion (leak-row count / return-abs equality) over private counter inspection.

- [ ] **Step 2: Write the failing path-point fixture + test**

`interproc_path.compact` — witness → passed to helper → helper writes to ledger; the rendered `E3100` must carry a secondary span for the argument-passing point. Validate REJECT against `compactc`. Unit test asserts the confirmed leak's witnesses carry a `PathPoint` whose `description`/`exposure` matches B0 Q5's confirmed wording.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p analyzer-core disclosure::interp::tests::call_ht_memoizes disclosure::interp::tests::interproc_path`
Expected: FAIL (no memo; no cross-call path point yet).

- [ ] **Step 4: Implement the memo + path point** in `interp_circuit_call`:

```rust
    let cache_key = (callee_file, callee_symbol, arg_shapes.clone(), control.to_vec());
    if let Some(cached) = state.cache.get(&cache_key) {
        return cached.clone(); // HIT: leaks were already recorded on the miss; (src,nature) dedup makes re-walk unnecessary
    }
    // ... bind params, adding the cross-circuit arg path point:
    for (i, (p, a)) in callee.params.iter().zip(arg_shapes.iter()).enumerate() {
        let pp = PathPoint {
            src: call.syntax().text_range(),
            description: arg_description(&callee.name, i, callee.params.len()), // reuse v3a helper
            exposure: String::new(),
        };
        callee_env.insert(p.clone(), add_path_point(a.clone(), &pp));
    }
    // ... interpret body -> ret ...
    state.cache.insert(cache_key, ret.clone());
    ret
```

(If B0 Q5 confirms a distinct cross-circuit wording rather than the native `arg_description`, use that verbatim string instead.) Add `#[derive(Hash)]` to `Abs`/`Witness`/`WitnessInfo`/`PathPoint` (or implement a canonical key) so `CallCache`'s key type is hashable — verify no floating/`TextRange` non-`Hash` field blocks it (`TextRange` is `Hash`).

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p analyzer-core disclosure && cargo test -p compact-analyzer --test disclosure_differential rule_tagged`
Expected: PASS.

- [ ] **Step 6: clippy/fmt + background corpus gate** (as B2 Step 7). Expected `false_positives=0`.

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/disclosure/ crates/compact-analyzer/tests/fixtures/disclosure/interproc_path.compact
git commit -m "feat(disclosure): walk-local call-ht memo + cross-circuit path points (v3b B3)"
```

---

## Task B4: Module import/export-chain resolution

Turn v3a's two module fail-closed amber points into real analysis: (a) module-nested `export circuit`s re-exported to the top level become disclosing roots; (b) module-imported/unresolved callees resolve to their circuit body instead of amber. Keep fail-closed for genuinely unresolvable chains. **OPUS reviewer.** **Dispatch a `midnight-verify:source-investigator` sub-step first** to confirm how 0.31.1 determines which module circuits are analysis roots (the `id-exported?` + module re-export semantics) — do NOT infer module visibility from training data.

**Files:**
- Modify: `crates/analyzer-core/src/disclosure/interp.rs` — `discover_roots`/`is_module_nested` (include re-exported module circuits); `classify_callee`/`unresolved_callee_class` (resolve module-imported circuit calls to `Circuit { file, symbol }`).
- Create: `crates/compact-analyzer/tests/fixtures/disclosure/module_circuit_leak.compact` (reject — a module circuit reachable from a top-level export leaks), `.../module_call_leak.compact` (reject — a top-level export calls a module-imported circuit that leaks).
- Test: `interp.rs` unit + differential fixtures.

**Interfaces:**
- Consumes: the resolution machinery (`resolve_query`/`item_symbol`/`ItemTree::symbols`/`Symbol::parent`), `interp_circuit_call` (B2/B3).
- Produces: root discovery that includes re-exported module circuits; `classify_callee` resolving module-imported circuits. The existing `module_reexport_ok.compact` accept fixture MUST stay accepted (no regression).

- [ ] **Step 1: source sub-step** — dispatch `midnight-verify:source-investigator` to confirm, at `0da5b045`, (i) the exact `id-exported?` predicate and how a module's `export circuit` becomes a top-level analysis root (or not) when the module is imported/re-exported, and (ii) that a call to a module-imported circuit is analyzed through its body (same `handle-call` path). Write to `.superpowers/sdd/task-B4-findings.md`. **Reconcile this task's logic to those findings before coding.**

- [ ] **Step 2: Write failing fixtures + tests** — `module_circuit_leak.compact` and `module_call_leak.compact`, validated REJECT against `compactc`; unit tests asserting each now confirms an `E3100`. Also add a regression assertion that `module_reexport_ok.compact` stays leak-free (accept).

- [ ] **Step 3: Run to verify failure** — the module circuits currently produce amber advisories, not E-leaks.

Run: `cargo test -p analyzer-core disclosure::interp::tests::module`
Expected: FAIL.

- [ ] **Step 4: Implement** per B4 findings — extend `discover_roots` to include re-exported module circuits as `Root::Circuit` (guarded by the confirmed export predicate), and `classify_callee`/`unresolved_callee_class` to resolve module-imported circuit calls to `CalleeClass::Circuit { file, symbol }` (feeding `interp_circuit_call`). Anything still unresolvable → keep the amber advisory (§0). Keep the `is_module_nested`-not-reexported case fail-closed.

- [ ] **Step 5: Run to verify pass + regression**

Run: `cargo test -p analyzer-core disclosure && cargo test -p compact-analyzer --test disclosure_differential rule_tagged`
Expected: PASS, including `module_reexport_ok` staying accept.

- [ ] **Step 6: clippy/fmt + background corpus gate.** Expected `false_positives=0` (this is where v3a's FP #2 lived — the corpus is the guard that module resolution didn't reintroduce it).

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/disclosure/ crates/compact-analyzer/tests/fixtures/disclosure/module_*.compact .superpowers/sdd/task-B4-findings.md
git commit -m "feat(disclosure): module import/export-chain resolution (v3b B4)"
```

---

## Task B5: Differential parity harness — reject-direction + interprocedural fixtures + full corpus

v3a's corpus gate only checks the no-false-positive direction. v3b's done-bar is corpus **parity**, so assert the reject direction too: on every compactc-REJECTED (WPP-banner) file, native must emit `E`-or-`U` (never silent green). Wire the new fixtures into the Tier-1 table. **sonnet rust-dev; OPUS reviewer** (parity is correctness-critical).

**Files:**
- Modify: `crates/compact-analyzer/tests/disclosure_differential.rs` — add `native_reports_something` helper; extend the corpus loop to assert the reject direction; add the B2/B3/B4 fixtures to `FIXTURES` with `discloses`/`native_confirms`.
- Test: the harness itself.

**Interfaces:**
- Consumes: `native_discloses` (E-only), `compiler_discloses` (`Some(true)` = WPP reject), `Fixture`.
- Produces: `fn native_reports_something(text, path) -> bool` (any `E`≥3100 OR any `U`-family diagnostic); a `silent_on_reject: Vec<PathBuf>` gate.

- [ ] **Step 1: Write the failing harness assertion** — add to `corpus_no_false_positive_disclosures` (or a sibling `corpus_no_silent_green_on_reject`):

```rust
fn native_reports_something(text: &str, path: &Path) -> bool {
    let mut host = AnalysisHost::new();
    let file: FileId = host.vfs_mut().file_id(path);
    host.vfs_mut().set_overlay(file, text.to_string(), 1);
    host.disclosure_diagnostics(file).iter().any(|d| {
        (d.code.prefix == "E" && d.code.number >= 3100) || d.code.prefix == "U"
    })
}
```

In the corpus loop, for `Some(true)` (compactc WPP-reject) files, push to `silent_on_reject` when `!native_reports_something(...)`, and assert `silent_on_reject.is_empty()` (fail-closed parity: native may amber, but never silent-green on a file compactc rejects for disclosure).

- [ ] **Step 2: Add the interprocedural fixtures to `FIXTURES`**

```rust
Fixture { name: "interproc_call_leak.compact",  rule: "B2 cross-circuit call -> ledger sink",           discloses: true,  native_confirms: true },
Fixture { name: "interproc_call_ok.compact",    rule: "B2 callee disclose() sanitizes",                 discloses: false, native_confirms: false },
Fixture { name: "interproc_path.compact",       rule: "B3 cross-circuit path point",                    discloses: true,  native_confirms: true },
Fixture { name: "module_circuit_leak.compact",  rule: "B4 re-exported module circuit root leaks",       discloses: true,  native_confirms: true },
Fixture { name: "module_call_leak.compact",     rule: "B4 module-imported circuit call leaks",          discloses: true,  native_confirms: true },
```

- [ ] **Step 3: Run to verify** — Tier-1 fixtures pass; the reject-direction assertion compiles.

Run: `cargo test -p compact-analyzer --test disclosure_differential rule_tagged`
Expected: PASS.

- [ ] **Step 4: Run the FULL corpus gate (background, both directions)**

Run (background): `COMPACT_CORPUS_DIR=<corpus>/examples cargo test -p compact-analyzer --test disclosure_differential -- --nocapture --test-threads=1`
Expected: `false_positives=0` AND `silent_on_reject` empty. If any file is silent-green on a compactc WPP-reject, root-cause it (an unhandled interprocedural form must fail closed to amber) before proceeding — this is the parity done-bar. Record the final `accepted/confirmed_leak/indeterminate/false_positives` line in `.superpowers/sdd/progress-v3bc.md`.

- [ ] **Step 5: Commit**

```bash
git add crates/compact-analyzer/tests/disclosure_differential.rs
git commit -m "test(disclosure): reject-direction parity + interprocedural corpus fixtures (v3b B5)"
```

---

## Task B6: Salsa invalidation on source-identity changes

Confirm (and test) that editing that changes a source's IDENTITY — a function flipping `witness`↔`circuit`, an added exported-circuit/constructor parameter (which re-seeds sources) — invalidates the disclosure diagnostics. The whole-file query keys on `src` so text edits already invalidate; this task pins the identity-flip cases with regression tests and closes any gap. **sonnet.**

**Files:**
- Test: `crates/analyzer-core/src/disclosure/mod.rs` tests (or an integration test) — edit-then-recompute assertions.

**Interfaces:**
- Consumes: `AnalysisHost::disclosure_diagnostics`, VFS overlay editing.

- [ ] **Step 1: Write failing/regression tests** — (a) a `witness getW()` used in a circuit that leaks → flip `getW` from `witness` to `circuit` → the leak verdict must change on recompute; (b) add an exported-circuit parameter that becomes a new `CircuitArg` source → a previously-clean sink now leaks. Assert the query returns the updated verdict after `set_overlay`.

- [ ] **Step 2: Run** — if the whole-file `src`-keyed query already invalidates correctly (likely), these pass immediately and stand as regression pins. If any identity flip is memoized stale (e.g. a cross-file summary in a future variant), fix the key.

Run: `cargo test -p analyzer-core disclosure::mod::tests::identity`
Expected: PASS (regression pins).

- [ ] **Step 3: Commit**

```bash
git add crates/analyzer-core/src/disclosure/mod.rs
git commit -m "test(disclosure): pin salsa invalidation on source-identity changes (v3b B6)"
```

---

## Task B7 (CONDITIONAL): Cross-contract-call sink (K3/K4/K5)

**Gate:** implement this task ONLY IF `.superpowers/sdd/task-Q7-findings.md` concludes YES — i.e. an impure, non-constructor circuit making a `contract-call` with a witness-derived argument compiles far enough that local `compactc` 0.31.1 emits the WPP disclosure banner (making the sink differentially testable). **If Q7 concludes NO** (feature rejected regardless), SKIP this task: keep `contract-call` a fail-closed amber advisory (as v3a does), record the empirical evidence in `progress-v3bc.md`, and note in the PR that the handover's amber-gating was empirically upheld. **sonnet rust-dev; OPUS reviewer** (new confirmed-leak sink). Dispatch `compact-core:compact-dev` for the fixtures.

**Files (if Q7 = YES):**
- Modify: `crates/analyzer-core/src/disclosure/interp.rs` — add a `contract-call` sink arm: K3 control-witness leak (`"making this contract call"`), K4 contract-reference leak (`"contract call contract reference"`), K5 per-arg leak (`"contract call argument N"`, 1-based). Result = `default-value` of the call's return type (clean). Restrict to the callable case Q7 confirmed (impure, non-constructor).
- Create: `crates/compact-analyzer/tests/fixtures/disclosure/contract_call_leak.compact` (reject) + `contract_call_ok.compact` (accept) — the exact Q7-validated fixture pair.
- Modify: `crates/compact-analyzer/tests/disclosure_differential.rs` — add both fixtures to `FIXTURES` (`contract_call_leak`: discloses true / native_confirms true; `contract_call_ok`: false/false).

**Interfaces:**
- Consumes: `record_leak` (K3/K4/K5 nature strings, verbatim from R0), `witnesses_of`, `abs_of_type`, `interp_expr`.
- Produces: a confirmed `E3100` on witness data reaching a cross-contract call.

- [ ] **Step 1 (gate):** read `task-Q7-findings.md`. If NO → skip to the SKIP action above (record + no code). If YES → proceed.
- [ ] **Step 2:** drop the Q7-validated `contract_call_leak.compact` (reject) + `contract_call_ok.compact` (accept) into `tests/fixtures/disclosure/`; confirm each against `compactc` (leak → WPP banner; ok → accept).
- [ ] **Step 3: Write the failing test** — native must confirm an `E3100` on `contract_call_leak` and none on `contract_call_ok`.
- [ ] **Step 4: Run to verify failure.** Run: `cargo test -p analyzer-core disclosure::interp::tests::contract_call`. Expected: FAIL (currently amber, not E).
- [ ] **Step 5: Implement** the `contract-call` sink arm (K3/K4/K5) with the verbatim R0 nature strings, gated to the impure/non-constructor case Q7 confirmed. Remove the v3a `contract-call → amber` advisory for the now-analyzed case; keep amber for any residual (e.g. a pure-circuit/constructor caller, which compactc rejects earlier anyway).
- [ ] **Step 6: Run to verify pass** + wire the two fixtures into `FIXTURES`. Run: `cargo test -p compact-analyzer --test disclosure_differential rule_tagged`. Expected: PASS.
- [ ] **Step 7: clippy/fmt + background corpus gate** (both directions). Expected `false_positives=0`, `silent_on_reject` empty.
- [ ] **Step 8: Commit**

```bash
git add crates/analyzer-core/src/disclosure/ crates/compact-analyzer/tests/
git commit -m "feat(disclosure): cross-contract-call sink K3/K4/K5 (v3b B7, Q7-empirically-enabled)"
```

---

# PHASE v3c — Disclosure UX

## Task C1: `disclosureDiagnostics` on/off toggle (TOP priority)

The amber channel is deliberately noisy on multi-circuit contracts; users must be able to turn the whole disclosure channel off. Mirror `typeDiagnostics` exactly. **sonnet rust-dev + typescript-dev.**

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs` — `ToolchainConfig { disclosure_diagnostics: bool }` (default `true`); `toolchain_config_from` reads `"disclosureDiagnostics"`; gate the `disclosure_diagnostics(file)` loop (~L956) behind `if self.toolchain_config.disclosure_diagnostics`.
- Modify: `editors/vscode/package.json` — add `compact-analyzer.disclosureDiagnostics` (boolean, default true, markdownDescription mirroring `typeDiagnostics`).
- Modify: `editors/vscode/src/config-core.ts` — read + default `disclosureDiagnostics` in `buildInitOptions`.
- Test: `server.rs` unit tests; `editors/vscode/src/test/unit/config.test.ts`.

**Interfaces:**
- Consumes: `ToolchainConfig`, `toolchain_config_from`, `buildInitOptions`.
- Produces: `ToolchainConfig.disclosure_diagnostics` (default true); init-option key `disclosureDiagnostics`.

- [ ] **Step 1: Write failing Rust tests** (mirroring `toolchain_config_reads_type_diagnostics_flag` / `_defaults_true`):

```rust
#[test]
fn toolchain_config_reads_disclosure_diagnostics_flag() {
    let opts = json!({ "disclosureDiagnostics": false });
    let config = toolchain_config_from(Some(&opts));
    assert!(!config.disclosure_diagnostics);
}
#[test]
fn toolchain_config_disclosure_diagnostics_defaults_true() {
    assert!(toolchain_config_from(Some(&json!({}))).disclosure_diagnostics);
    assert!(toolchain_config_from(None).disclosure_diagnostics);
}
```

- [ ] **Step 2: Write failing TS test** (`config.test.ts`) — default sends `disclosureDiagnostics: true`; a configured `false` is preserved; a non-boolean defaults to true. Update the two `Object.keys(result).sort()` assertions to include `"disclosureDiagnostics"`.

- [ ] **Step 3: Run both to verify failure**

Run: `cargo test -p compact-analyzer toolchain_config_reads_disclosure && (cd editors/vscode && npm test -- config)`
Expected: FAIL (field/key absent).

- [ ] **Step 4: Implement** — add the field (default true) + `toolchain_config_from` read; gate the disclosure loop; add the package.json setting + `config-core.ts` read/default.

```rust
// server.rs ToolchainConfig
pub disclosure_diagnostics: bool,
// Default impl:
disclosure_diagnostics: true,
// toolchain_config_from:
if let Some(flag) = opts.get("disclosureDiagnostics").and_then(|v| v.as_bool()) {
    config.disclosure_diagnostics = flag;
}
// publish path (~L956):
if self.toolchain_config.disclosure_diagnostics {
    for d in self.host.disclosure_diagnostics(file) { /* existing push + source-tag */ }
}
```

- [ ] **Step 5: Run both to verify pass**

Run: `cargo test -p compact-analyzer toolchain_config && (cd editors/vscode && npm test -- config)`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/compact-analyzer/src/server.rs editors/vscode/package.json editors/vscode/src/config-core.ts editors/vscode/src/test/unit/config.test.ts
git commit -m "feat(vscode): disclosureDiagnostics on/off toggle (v3c C1)"
```

---

## Task C2: Advisory wording + `codeDescription` + advisory-UX contract

Make the amber advisories read as advisories, not warnings, and carry the "compile is authoritative" contract. **sonnet.**

**Files:**
- Modify: `crates/analyzer-core/src/disclosure/leaks.rs` — `advisory_to_diagnostic` wording.
- Modify: `crates/compact-analyzer/src/lsp_utils.rs` — attach `codeDescription` (a "why unverified" href) to `U`-family diagnostics in `diagnostic_to_lsp`.
- Test: `lsp_utils.rs` unit tests.

**Interfaces:**
- Consumes: `advisory_to_diagnostic`, `diagnostic_to_lsp`, the `U`/`E` source-tagging (already: `"compact-analyzer (unverified)"` for `U`).
- Produces: a `codeDescription.href` on `U`-family LSP diagnostics; advisory message copy stating "advisory — a clean editor is not a proof of privacy; compile is the authoritative gate."

- [ ] **Step 1: Write failing test** (`lsp_utils.rs`) — a `U3100` diagnostic maps to an LSP diagnostic whose `code_description` is `Some(href)` and whose message conveys the advisory contract; an `E3100` has `code_description == None`.

- [ ] **Step 2: Run to verify failure.** Run: `cargo test -p compact-analyzer lsp_utils`. Expected: FAIL.

- [ ] **Step 3: Implement** — in `diagnostic_to_lsp`, when `code.prefix == "U"`, set `code_description = Some(CodeDescription { href: <docs url or file link> })`; adjust `advisory_to_diagnostic` message to include the contract clause (keep it concise — the hover/docs carry the full text).

- [ ] **Step 4: Run to verify pass.** Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/disclosure/leaks.rs crates/compact-analyzer/src/lsp_utils.rs
git commit -m "feat(disclosure): advisory wording + codeDescription contract (v3c C2)"
```

---

## Task C3: `disclose()` minimal-scope quick-fix

A `textDocument/codeAction` that wraps the **minimal** tainted expression at the sink in `disclose(...)` — never a broad enclosing wrap (which could silently sanitize a second real leak). Placement differential-verified against compiler-accepted fixes. **OPUS reviewer** (over-disclosure is a security risk). **Dispatch `compact-core:compact-dev`** to differential-verify each placement against `compactc`.

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs` — add `code_action_provider` to `ServerCapabilities`; handle `textDocument/codeAction`.
- Create: `crates/compact-analyzer/src/code_action.rs` (or extend `lsp_utils.rs`) — build the `disclose(<expr>)` `WorkspaceEdit` from an `E3100` leak's primary span (the minimal sink expression).
- Modify: `crates/analyzer-core/src/disclosure/leaks.rs` / `mod.rs` — ensure each `DisclosureLeak` carries the minimal tainted expression's exact `TextRange` (the primary span already anchors the sink; confirm it is the minimal expression, not the whole statement).
- Test: `server.rs`/`code_action.rs` unit tests + an e2e (C7).

**Interfaces:**
- Consumes: `DisclosureLeak.src` (the sink's minimal expression range), the LSP request/response plumbing in `server.rs`.
- Produces: a `CodeAction` titled "Reveal this value with disclose()" of kind `quickfix`, with a `WorkspaceEdit` inserting `disclose(` before and `)` after the minimal tainted expression; offered only for `E3100` diagnostics at the cursor range; NOT a fix-all/fix-on-save.

- [ ] **Step 1: Write the failing test** — given a document with a confirmed `E3100` leak (e.g. `store = secret;`), a `textDocument/codeAction` at the leak range returns exactly one `quickfix` whose edit turns `secret` into `disclose(secret)` (the minimal expression), and whose result, when applied, is accepted by `compactc` (differential check via `compact-core:compact-dev` on the fixed text). Assert the edit does NOT wrap the whole statement or a broader expression.

- [ ] **Step 2: Run to verify failure** (no code-action handler yet). Expected: FAIL.

- [ ] **Step 3: Implement** — advertise `code_action_provider: Some(CodeActionProviderCapability::Simple(true))` (or `Options` with `code_action_kinds: [QUICKFIX]`); in the handler, for each `E3100` diagnostic overlapping the requested range, emit a `CodeAction { kind: QUICKFIX, edit: insert "disclose(" at span.start + ")" at span.end }`. The minimal expression is the leak's primary span — verify (with `compact-core:compact-dev`) that this span is exactly the disclosure point `compactc` accepts; if a rule requires a narrower/different sub-expression for a given sink class, encode that placement rule (differential-verified) rather than guessing.

- [ ] **Step 4: Run to verify pass** + differential-verify the fixed text compiles. Expected: PASS; `compactc` accepts the quick-fixed source.

- [ ] **Step 5: clippy/fmt.** Run: `cargo clippy -p compact-analyzer --all-targets -- -D warnings && cargo fmt --check`.

- [ ] **Step 6: Commit**

```bash
git add crates/compact-analyzer/src/ crates/analyzer-core/src/disclosure/
git commit -m "feat(lsp): minimal-scope disclose() quick-fix code action (v3c C3)"
```

---

## Task C4: Path rendering polish + stdlib-collapse (display-only)

Render one `related_information` entry per source origin and per non-stdlib path point; collapse stdlib-internal path points to the stdlib entry site. **Display-only — never collapse the leak itself** (§3.7). **sonnet.**

**Files:**
- Modify: `crates/analyzer-core/src/disclosure/mod.rs` (`leak_to_diagnostic`) and/or `crates/compact-analyzer/src/lsp_utils.rs` — collapse consecutive stdlib-sourced path points to their entry site when building secondary spans.
- Modify: `crates/analyzer-core/src/disclosure/interp.rs` — if not already tracked, tag path points that originate inside stdlib (K8 remapping) so the renderer can collapse them.
- Test: unit test asserting a stdlib-internal path collapses to one entry while the leak (and its non-stdlib points) survive.

**Interfaces:**
- Consumes: `PathPoint`, `leak_to_diagnostic`, the K8 stdlib-circuit remapping (v3a).
- Produces: a rendered secondary-span list with stdlib-internal points collapsed to the entry site; the leak's `(src,nature)` unchanged.

- [ ] **Step 1: Write the failing test** — a leak whose witness path includes ≥2 stdlib-internal points renders exactly one collapsed "…via standard-library circuit `<name>`" secondary span at the user entry site, plus the user-code points; the number of confirmed `E3100` leaks is unchanged by collapsing.

- [ ] **Step 2: Run to verify failure.** Expected: FAIL.

- [ ] **Step 3: Implement** the display-only collapse in the renderer (keep leak recording untouched — the collapse operates on the rendered `secondary_spans`, not on `record_leak`).

- [ ] **Step 4: Run to verify pass.** Expected: PASS; corpus gate unaffected (display-only).

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/disclosure/ crates/compact-analyzer/src/lsp_utils.rs
git commit -m "feat(disclosure): stdlib-collapse path rendering (display-only) (v3c C4)"
```

---

## Task C5: Version check (pinned tag vs installed compiler)

On a mismatch between the analyzer's pinned extraction version and the installed `compact` version, surface an advisory that the disclosure tables may be stale (a drifted native/ledger flag table is a §0 unknown). **sonnet.**

**Files:**
- Create: `crates/analyzer-core/src/disclosure/version.rs` (or a const in `mod.rs`) — `pub const PINNED_COMPILER_VERSION: &str = "0.31.1";`.
- Modify: `crates/compact-analyzer/src/server.rs` — compare `Toolchain.version` (already discovered) against `PINNED_COMPILER_VERSION`; on mismatch, publish a single workspace/file advisory (`U`-family, `"compact-analyzer (unverified)"`) once per session or per file, stating the disclosure tables were extracted for 0.31.1 and may be stale.
- Test: `server.rs` unit test — mismatch → advisory present; match → no advisory.

**Interfaces:**
- Consumes: `Toolchain::discover(..).version`, the diagnostic publish path.
- Produces: `PINNED_COMPILER_VERSION`; a version-mismatch advisory.

- [ ] **Step 1: Write the failing test** — a fake toolchain version `"0.33.0"` yields a version-mismatch advisory; `"0.31.1"` yields none.
- [ ] **Step 2: Run to verify failure.** Expected: FAIL.
- [ ] **Step 3: Implement** the constant + comparison (compare `MAJOR.MINOR.PATCH`; a differing version → advisory). Emit at most one such advisory (dedup).
- [ ] **Step 4: Run to verify pass.** Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/disclosure/ crates/compact-analyzer/src/server.rs
git commit -m "feat(disclosure): compiler-version drift advisory (v3c C5)"
```

---

## Task C6: Transient-parse retention (no blank-to-green mid-edit)

During unparseable mid-edit states, disclosure diagnostics must NOT silently blank to green (a blank reads as "safe"). **sonnet.**

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs` — in the publish path, when the current parse is broken/empty AND disclosure diagnostics are enabled, either retain the last-known disclosure diagnostics (stale) or publish a single "disclosure analysis paused (unparseable)" `U`-family advisory, rather than publishing an empty disclosure set.
- Test: `server.rs` unit test — a transition from a leak state to an unparseable edit does not publish an empty disclosure set (retains stale or shows the paused advisory).

**Interfaces:**
- Consumes: the parse status (existing parser diagnostics / `parsed(db, src)` error signal), the last-published disclosure diagnostics per file.
- Produces: a retention rule (documented: stale-retain OR paused-advisory).

- [ ] **Step 1: Decide the retention rule** — default to a **"disclosure analysis paused (unparseable source)"** `U`-family advisory (simpler and honest; avoids stale spans drifting off their ranges). Document the choice in the code comment + `progress-v3bc.md`.
- [ ] **Step 2: Write the failing test** — after a leak is published, an edit that makes the source unparseable results in NOT an empty disclosure channel: either the paused advisory is present or the prior diagnostics are retained.
- [ ] **Step 3: Run to verify failure.** Expected: FAIL (currently blanks to green).
- [ ] **Step 4: Implement** the paused-advisory retention.
- [ ] **Step 5: Run to verify pass.** Expected: PASS.
- [ ] **Step 6: Commit**

```bash
git add crates/compact-analyzer/src/server.rs
git commit -m "feat(disclosure): transient-parse retention (no blank-to-green) (v3c C6)"
```

---

## Task C7: In-editor VS Code e2e

Exercise the full surface in a real VS Code host (xvfb), as v2c did: a red leak + related-info navigation, the quick-fix, an amber advisory, and the toggle. **typescript-dev.**

**Files:**
- Modify: `editors/vscode/src/test/e2e/activation.test.ts` — add scenarios; reuse `disclosure-leak.compact` / `disclosure-advisory.compact` / `uxfeatures.compact` fixtures (extend if needed).
- Test: the e2e itself (`npm run test:e2e` / the xvfb harness).

**Interfaces:**
- Consumes: the running language server (C1–C6 wired), the existing e2e scaffolding.
- Produces: e2e scenarios: (1) an `E3100` leak diagnostic appears with related-info; (2) the `disclose()` quick-fix is offered and applies the minimal edit; (3) a `U3100` advisory appears with source `"compact-analyzer (unverified)"`; (4) toggling `disclosureDiagnostics: false` removes the disclosure diagnostics while parser/resolution diagnostics remain.

- [ ] **Step 1: Write the failing e2e scenarios** (extend `activation.test.ts`).
- [ ] **Step 2: Run to verify failure/coverage.** Run: `cd editors/vscode && xvfb-run -a npm run test:e2e` (or the repo's e2e command). Expected: new scenarios FAIL until wired.
- [ ] **Step 3: Wire** any missing extension glue (quick-fix command registration is server-driven via `code_action`, so mostly assertions).
- [ ] **Step 4: Run to verify pass.** Expected: all e2e scenarios green.
- [ ] **Step 5: Commit**

```bash
git add editors/vscode/src/test/e2e/ editors/vscode/src/test/e2e/fixture/
git commit -m "test(vscode): disclosure e2e — leak, quick-fix, advisory, toggle (v3c C7)"
```

---

## Task B/C-FINAL: Whole-branch review, corpus parity, and PR

**Files:** none new — verification + PR.

- [ ] **Step 1: Full test sweep** — `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `(cd editors/vscode && npm test)`. All green.
- [ ] **Step 2: Full corpus gate (both directions), background** — `COMPACT_CORPUS_DIR=<corpus>/examples cargo test -p compact-analyzer --test disclosure_differential -- --nocapture --test-threads=1`. Expected: `false_positives=0` AND `silent_on_reject` empty (corpus parity done-bar). Record the counts in `progress-v3bc.md`.
- [ ] **Step 3: Whole-branch review** — dispatch `devs:code-reviewer` (OPUS) over the full v3b+v3c diff vs `main`. Focus: §0 fail-closed upheld end-to-end (no silent-green, no false-red); interprocedural termination (recursion guard + finite call tree); memo key correctness (arg_shapes + control both in key); quick-fix minimal-scope (no over-disclosure); toggle one-directional merge preserved; version/retention advisories are `U`-family only. Address Critical/Important findings; defer/record the rest.
- [ ] **Step 4: Update `.claude/compact-check.json`** if any new fixture dir was added outside `tests/fixtures/disclosure/` (all new fixtures are inside it — already excluded).
- [ ] **Step 5: Open the PR**

```bash
git push -u origin v3-disclosure-interproc-ux
gh pr create --title "v3b+v3c — Interprocedural disclosure + UX (WPP)" --body "$(cat <<'EOF'
Extends v3a's intraprocedural WPP to interprocedural (cross-circuit call propagation, module resolution) with corpus differential parity, and polishes the diagnostics UX (disclosureDiagnostics toggle, disclose() quick-fix, path rendering + stdlib-collapse, advisory contract, version check, transient-parse retention, in-editor e2e).

- §0 fail-closed upheld: every unanalyzable point → amber U3100, never silent green, never false-positive red.
- Corpus gate: false_positives=0 and no silent-green on WPP-rejected files (parity).
- Memoization: walk-local call-ht (ADR: docs/superpowers/plans/2026-07-18-v3b-memoization-adr.md).

Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-Review (controller, against spec Rev 2.1)

- **§4.2 per-call-site memoization / §8 risk #2 (summary shape):** B1 ADR resolves it (walk-local `call-ht`, keyed on `(symbol, arg_shapes, control)` — both shape axes, not identity alone); B2/B3 implement it. ✓
- **§3.9 disclosing-context gate:** roots run `disclosing_fn=Some`, callees `None` (B2) — the reprocessing gate collapses to "roots never memoized, callees always." ✓
- **§3.7 cross-circuit path points:** B3 (wording from B0 Q5). ✓
- **Module resolution:** B4 (v3a's two amber points → real analysis, fail-closed residue). ✓
- **Cross-contract-call sink:** stays version-gated amber (Global Constraints; B0 Q7) — NOT a v3b confirmed leak. ✓
- **Corpus parity done-bar:** B5 adds the reject-direction assertion the v3a harness lacked. ✓
- **Salsa identity invalidation (§8 risk #6):** B6. ✓
- **§4.3 toggle:** C1 (top priority). **Advisory contract:** C2. **Minimal quick-fix:** C3. **Path/stdlib-collapse:** C4. **Version check:** C5. **Transient-parse retention:** C6. **e2e:** C7. ✓
- **Deferred v3a findings addressed:** fold aggregated-fixpoint verbosity (accepted as terminating/fail-closed — no new task; note in ADR); N2 table maintenance (M6, out of scope); `merge_diagnostics` exact-range monitor (noted in C1's scope; corpus proves 0 false-E). ✓
- **Placeholder scan:** code shown for every code step; test bodies concrete; strings marked "reconcile with B0/B4 findings" where source-verified wording is required. ✓
