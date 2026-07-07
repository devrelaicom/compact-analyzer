# v3 — Native disclosure analysis (post-v1 direction, flagship)

> Direction, not commitment (spec §9). The flagship differentiator — the feature no
> other Compact tool has and the reason "staged to full native" was chosen over
> delegating semantics to the compiler.

## Goal

Real-time, interprocedural taint tracking of witness-derived values, so the analyzer
flags implicit disclosure **as you type** — with the full witness-to-sink path rendered
as related-information locations, and a quick-fix code action that inserts `disclose()`.

## Why

Disclosure errors are Compact's hardest and most privacy-critical error class:
witness-derived (private) data reaching public sinks (ledger writes, returned values,
etc.) must be explicitly acknowledged with `disclose()`. Today developers discover
these only at compile time, one at a time, with a textual path in an error message.
Live analysis with clickable paths turns the language's most confusing error into its
best-supported one.

## Expected scope

- Taint sources: witness call results (and whatever else the compiler's analysis
  treats as witness-derived — extract the exact source/sink/sanitizer sets from the
  compiler's disclosure pass, not from docs).
- Interprocedural propagation through circuit calls (per-circuit summaries over the
  M2 item index; needs v2 types for field/element-level precision).
- Sinks: ledger writes, exported-circuit returns — again, exactly the compiler's set.
- `disclose()` as the sanitizer.
- UX: diagnostic at the sink matching **compiler wording**, related-information
  locations tracing witness → ... → sink, quick-fix inserting `disclose()` at the
  right expression.

## Explicitly out

- Any soundness claim beyond "agrees with the compiler": the compiler's own analysis
  is the spec. Differential-test against `compactc` over the corpus; disagreements are
  bugs on our side by definition (v1's dual-source diagnostic tagging already gives
  users the ground truth on save).
- Cross-language taint into TypeScript witness implementations.

## Facts / constraints to honor

- The compiler's disclosure pass (Chez Scheme) is the reference implementation —
  the same source-extraction discipline as M2's import semantics: read the pass,
  record the exact rules, build fixtures from real compiler accept/reject behavior
  at the pinned tag before implementing.
- Quick-fix placement is subtle (`disclose()` wraps an expression; the compiler
  dictates which expression is the disclosure point) — derive placement rules from
  compiler-accepted fixes, not intuition.
- Interprocedural analysis is the first feature whose cost may genuinely exceed
  memoized-recompute comfort; expected to lean on v2's re-examination of the
  incrementality engine.
