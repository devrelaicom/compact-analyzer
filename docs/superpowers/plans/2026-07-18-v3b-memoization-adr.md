# ADR: memoization architecture for v3b interprocedural WPP

**Status:** Accepted
**Decides:** spec §8 risk #2 — "does a coarser summary than per-call-site memoization
exist, and is it sound?" This ADR is a prerequisite for Task B2/B3.
**Grounded in:** `.superpowers/sdd/task-B0-findings.md` (source-confirmed against
`LFDT-Minokawa/compact` @ `0da5b0452eb0c1053d42418bf34b12cc29c7d63e`,
`compiler/analysis-passes.ss`). All "B0 Qn" citations below are line anchors in that
file, not paraphrase.

## Context

v3a (merged, PR #5) implements *intraprocedural* WPP disclosure analysis: one query,
`disclosure_diagnostics_query` (`crates/analyzer-core/src/disclosure/mod.rs:46-75`),
walks every root (exported circuit / constructor) once, all in-process. v3b extends
this to cross-circuit calls: a helper circuit called from an exported root must have
its body interpreted too, so a leak that only manifests after a value crosses a call
boundary (`interproc_call_leak.compact` in the B2 brief) is still caught.

The naive implementation re-interprets a callee's body from scratch at every call
site. For programs with fan-in (multiple call sites reaching the same helper with
identical arguments) this is wasted work — potentially exponential in the depth of
the call graph if a helper calls other shared helpers. Some memoization is wanted.
The open question is architectural: memoize *how*, and does the compiler's own
memoization strategy (B0 Q2) map cleanly onto the analyzer's salsa-based query
architecture, or does salsa's incrementality make a different design pay for itself?

## Options considered

**Option A — walk-local `call-ht`.** A `HashMap`-backed cache, scoped to a single
invocation of `disclosure_diagnostics_query`, threaded through the interpreter as
part of `WalkState` (per the B2 brief: `WalkState { cache: CallCache, active:
HashSet<(FileId,u32)>, depth: u32 }`). Built fresh every time the query reruns,
discarded when it returns. This is a direct, structural port of the compiler's own
`call-ht` (B0 Q2): same key shape, same hit/miss semantics, scoped to one run of the
abstract interpreter exactly as the compiler's is scoped to one run of
`track-witness-data`.

**Option B — a separate `#[salsa::tracked] circuit_abstract` query.** Pull the
per-circuit interpretation out of the walk entirely and make it a first-class salsa
query, keyed on `(callee_file, callee_symbol, arg_shapes, control)`, so salsa's
incremental-recomputation machinery (not a hand-rolled `HashMap`) does the caching,
potentially *across* invocations of `disclosure_diagnostics_query` (e.g. shared
helpers called from multiple root files) and across incremental re-runs after an
unrelated edit.

## B0 evidence (the four facts the decision turns on)

1. **Leaks are a global side effect, not part of the memoized value (B0 Q6).**
   `call-ht`'s value slot holds only the returned `Abs` (`Call-processed abs`); leak
   recording (`record-leak!`) happens deep inside the sink arms during the walk and
   writes into a table that is separate from, and outside, the memo cache. A pure
   salsa query can only cache what it *returns* — so a `circuit_abstract` query
   would have to return `(Abs, Vec<DisclosureLeak>, Vec<Advisory>, uid_delta)` and
   the caller would have to re-apply the leaks/advisories/uid-delta on every cache
   hit exactly as if it had re-walked, for parity. That's strictly more moving parts
   (a tuple return, a uid-delta replay step, a place to apply the replay) than Option
   A's direct cache, for zero semantic gain: the leak set produced is identical
   either way (see Correctness argument, case (c)).
2. **The existing whole-file query is already coarse-grained.** `crates/analyzer-
   core/src/disclosure/mod.rs:46-53` — `disclosure_diagnostics_query` is
   `#[salsa::tracked(returns(clone), no_eq)]` keyed on `(file, src: SourceText, fd:
   FileDeps, ws: Workspace)`. Any edit to `src` invalidates and reruns the *entire*
   query — every root, every call, from scratch. A `circuit_abstract` query keyed
   finer than the file would only pay off if salsa could reuse a cached circuit
   interpretation across *edits*, but `src` (the whole file's text) is already an
   input to the enclosing query, so editing any line anywhere in the file busts the
   salsa dependency chain into `disclosure_diagnostics_query` regardless of whether
   a finer-grained query exists underneath it. A same-file circuit call — the only
   case B2/B3 implement — gets ~zero incremental benefit from Option B: it adds a
   salsa query boundary that will invalidate in lockstep with the query that already
   wraps it.
3. **Recursion is forbidden pre-WPP (B0 Q4).** `reject-recursive-circuits` runs
   seven passes before `track-witness-data` and rejects any circuit that reaches
   itself via the call stack. The call graph the WPP ever sees is therefore a
   finite DAG. Full inlining (re-walking every callee at every call site, no cache
   at all) already terminates — memoization is purely a performance bound, not a
   correctness or termination requirement. This weakens the case for either option
   being "necessary" and strengthens the case for picking the simpler one, since
   there is no soundness pressure pushing towards a heavier caching architecture.
4. **Option A mirrors the compiler bit-for-bit (B0 Q1/Q2).** `call-ht`
   (`analysis-passes.ss` ~5013-5030) is exactly a walk-local, run-scoped hash table
   keyed `(uid, abs*, control-witness*)` with `Call-unprocessed` / `Call-inprocess` /
   `Call-processed abs` states — precisely `WalkState.cache` + `WalkState.active` in
   the B2 design. Porting this structurally is the lowest-risk way to stay
   differentially faithful: every divergence from the compiler's own memoization
   behavior is a potential source of a corpus false-negative/false-positive, and
   Option A has none by construction (it IS the same structure).

## Decision

**Adopt Option A: walk-local `call-ht`.** Add `cache: CallCache` and `active:
HashSet<(FileId, u32)>` to the `WalkState` threaded through `interp_expr`/
`interp_stmt` (B2/B3), scoped to one call of `disclosure_diagnostics_query`. Do not
introduce a `#[salsa::tracked] circuit_abstract` query in v3b.

**Memo key: `(circuit_uid_or_symbol, arg_shapes: Vec<Abs>, control: Vec<Witness>)`**
— all three axes, matching `key-equal?` (B0 Q2): `eqv? uid` → symbol/uid identity;
`andmap abs-equal? abs*` → elementwise `Abs` equality over `arg_shapes`; `same-
witnesses?` → equality over the threaded `control` set. A coarser key — e.g. uid
alone, or uid+arg_shapes without control — is **unsound**: spec §4.2 requires
control-witness sensitivity, and two call sites reaching the same callee under
different ambient control (e.g. one inside a witness-gated branch, one not) can
legitimately produce different leak sets from the same body. Collapsing them onto
one cache entry would silently drop or fabricate leaks depending on evaluation
order — exactly the class of bug a coarser key would introduce with no compiler
grounding to justify it.

## Correctness / differential-equivalence argument

Claim: for any program accepted by `reject-recursive-circuits` (finite call DAG,
B0 Q4), the walk-local `call-ht` design produces the **same accumulated leak set**
as full inlining (no cache at all) would — and therefore the same leak set the
compiler itself produces, since the compiler's own `call-ht` has the identical
structure (fact 4 above). Three cases to check, matching the B1 brief and the plan's
Step 2 self-check:

**(a) Distinct `(arg_shapes, control)` keys never share a cache entry.**
The key is the full 3-tuple `(uid, arg_shapes, control)`; `key-equal?`/our
`HashMap`'s `Eq` impl requires all three components equal. Two call sites with the
same callee but different argument abstractions or different ambient control
witnesses hash and compare unequal, so they occupy distinct entries and each gets
its own walk. This is definitional, not argued — it falls directly out of using the
3-tuple as the key type rather than projecting onto a subset of it.

**(b) The disclosing-context gate is honored because roots never consult the cache.**
B0 Q3: only the root call (`(handle-call #f function-name … #t)` at the exported-
circuit entry point) passes `return-value-discloses?=#t`, and the gate that
reprocesses on a cache hit exists *specifically* for that case. In the analyzer's
model, roots (`discover_roots` → `interp_root`, `mod.rs:56-66`) are the outermost
drivers of the walk: they are never themselves the callee of an
`interp_circuit_call`, so they are never looked up in `WalkState.cache` — every root
is unconditionally interpreted exactly once, in full, by construction. There is no
code path by which a root's interpretation could be skipped by a stale cache hit,
because roots are never on the "lookup" side of the cache, only ever the origin of
a walk. The gate is honored trivially: it isn't that we replicate the compiler's
"reprocess if disclosing" branch, it's that our roots structurally never reach the
branch that would need it — a root is walked directly by `interp_root`, and only
*non-root callees* (`disclosing_fn = None`, B0 Q1's `(and return-value-discloses?
function-name)` collapsing to `#f`) are ever cache-looked-up at all.

**(c) A memo MISS that re-walks is safe because leak recording is idempotent.**
`DisclosureSink::record_leak` (`crates/analyzer-core/src/disclosure/leaks.rs:102-
125`) dedups by `(src, nature)`, unioning witnesses into the existing entry on a
repeat hit rather than duplicating a row (see also the unit test
`record_leak_dedups_by_src_and_nature_unioning_witnesses`, `leaks.rs:190`). This
means: whether a given `(uid, arg_shapes, control)` call is walked exactly once (all
call sites hit the cache after the first) or walked N times (cache never hits, i.e.
equivalent to full inlining), the resulting leak table is the **same set** — the
first walk populates every `(src, nature)` entry that will ever be produced for that
callee body under that key, and every subsequent walk of the identical key can only
re-derive entries that already exist (same body, same env, same control ⇒ same
sink call sites, same `src`/`nature` pairs) and merges into them, never producing a
new entry a cache-hit run wouldn't have. Consequently the accumulated leak set after
draining is identical whether the cache is warm, cold, or entirely absent (full
inlining) for that key. This matches B0 Q6's finding almost exactly (the compiler's
own `record-leak!` dedup gives it the same property) — the difference is that the
compiler's cache additionally *skips the re-walk on a non-disclosing hit* as a
genuine performance optimization, which is exactly what Option A replicates.

Taken together: (a) rules out cross-key contamination, (b) rules out the one case
where skipping a walk would drop a real leak (the disclosing root), and (c) shows
that for every other case, a cache hit vs. a cache miss are observationally
equivalent on the leak table. No case remains where walk-local `call-ht` could
diverge from full inlining, so the leak *set* is memoization-invariant, and by fact
4 (structural mirroring) it is also compiler-equivalent.

## Revisit-if

Reconsider this decision — specifically, introduce a `#[salsa::tracked]
circuit_abstract` query keyed on the same three axes — if **cross-file circuit
summary sharing becomes necessary for corpus parity** in B4 (cross-module calls) or
B5 (differential corpus wiring): e.g. if profiling on the real corpus shows the
walk-local cache isn't being reused across files that import a common shared
helper module and re-walking that helper per importing file becomes a measurable
cost, or if a correctness case emerges where the *same* callee needs to be recognized
as already-verified across separate `disclosure_diagnostics_query` invocations (e.g.
a future whole-workspace query that spans files). Until one of those triggers, the
walk-local design is both simpler and exactly as sound, so it is the default.
