# v3b — Interprocedural Disclosure — DRAFT / OUTLINE (not execution-ready)

> ⚠️ **DRAFT OUTLINE — DO NOT EXECUTE AS-IS.** This is not a task-by-task plan; it is the
> scope + sequencing sketch for v3b. **Before generating v3b's detailed implementation plan, the
> executing agent MUST:**
> 1. Re-read spec Revision 2 §3.9, §4.2, §8 (esp. the summary-shape open question).
> 2. Review the **as-merged** state of v3-R + v3a for drift — the `Abs` lattice, the interpreter's
>    expr/stmt coverage, the leak table, the advisory channel, and the differential harness may
>    have diverged from what this outline projected. **Do not trust the v3a interfaces named here
>    without confirming them in the merged code.**
> 3. Re-confirm the compiler `handle-call`/`call-ht`/`return-value-discloses?` behavior at the
>    pinned tag against a fixture BEFORE implementing (verify-first).
> 4. Run `superpowers:writing-plans` to author the real plan.

## Goal

Extend v3a's intraprocedural interpreter to full **interprocedural** WPP: propagate taint across
circuit calls with per-call-site memoization and reach full-corpus differential parity with
`compactc`.

> ⚠️ **Scope-narrowing from R2 (2026-07-16, compiler-verified):** cross-contract calls are
> **"not yet supported" in compactc 0.31.1** — every program using one is unconditionally rejected,
> so the cross-contract-call SINK is NOT differential-testable against 0.31.1 and stays a
> **version-gated amber advisory** (like `emit`), NOT a confirmed leak, until the analyzer targets a
> compiler that implements the feature. v3b's real, testable deliverable is the **interprocedural
> circuit→circuit** memoization (which IS supported). Do not plan the cross-contract-call sink as a
> v3b confirmed-verdict item under a 0.31.1 target; re-confirm feature availability at v3b planning
> time (it may have landed in a newer compiler by then, like emit).

## Why this is its own phase

The compiler analyzes a non-exported helper only *through* its call sites, re-interpreting the
body under the caller's abstract arguments and ambient control witnesses, memoized on
`(circuit-uid, abstract-args, control-witnesses)` — with a post-lookup reprocessing gate for
disclosing (exported-root) contexts (spec §3.9, source-verified). A coarse per-circuit-identity
summary is **unsound** (spec §4.2). v3a deliberately fails closed (amber advisory) on any call it
cannot interpret through; v3b makes those calls analyzed, turning advisories into real verdicts.

## Scope (outline — re-derive details just-in-time)

- **`circuit_abstract(circuit, arg_shapes, control_witnesses)`** salsa query — the analyzer's
  analogue of the compiler's `call-ht` cell. Key includes the abstract argument shapes and the
  ambient control witnesses, NOT circuit identity alone. Returns the callee's abstract return
  value; records the callee-body leaks into the root-drained table as a side effect of the walk.
- **The disclosing-context reprocessing gate** — model `return-value-discloses?`: a call reached
  from an exported root reprocesses to (re-)report return-value leaks; an ordinary callee does not.
- **Cross-contract-call sink** — ⚠️ **version-gated (R2 finding): only if the target compiler
  supports cross-contract calls.** Under a 0.31.1 target this stays v3a's amber advisory (the feature
  is "not yet supported", so no compiling program exercises it and it can't be differentially
  verified). If/when retargeted to a compiler with the feature: implement the impure-call sink
  (contract reference + each arg + control witnesses), replacing the deferred-sink advisory (Task
  A8) with real verdicts, pinned by real fixtures then.
- **Interprocedural path points** — extend the witness→sink trail across call boundaries ("passed
  as argument to circuit `<f>`").
- **Salsa invalidation** — confirm cross-file summary invalidation covers source-**identity**
  changes (a function flipping witness↔circuit; an added exported/constructor parameter re-seeds
  sources), not only body edits (spec §8 risk #6).
- **Remove the v3a fail-closed advisories** that v3b now covers (un-interpreted callee,
  cross-contract sink) — but keep the invariant for anything still unhandled.

## The load-bearing open question (spec §8 risk #2) — resolve FIRST

Whether a coarser, differential-equivalent summary than full per-call-site memoization exists is
**unproven**. The plan's first task is a spike: either (a) replicate per-call-site memoization
(accepting shape-scoped invalidation — editing a circuit invalidates every keyed instance), or
(b) prove a coarser generic summary yields identical differential results over the corpus. Do not
assume (b); (a) is the sound baseline.

## Done-bar (draft)

- Full ~486-file corpus differential parity: for every corpus file, the native
  confirmed-leak verdict (E-family) matches `compactc`'s WPP accept/reject.
- Fail-closed invariant preserved for any still-unhandled surface.
- No non-terminating analysis (recursion is forbidden upstream; memoization + finite witness
  universe guarantee termination — confirm).

## Risks

- Shape-sensitive memoization weakens the clean "leaf edit invalidates transitive callers" salsa
  story; measure incremental performance on a large workspace.
- The global leak-table dedup interacting with per-call-site reprocessing (the same sink reached
  via multiple call paths) must match the compiler's `record-leak!` merge exactly.
