# v2 — Native type checking (post-v1 direction)

> Direction, not commitment (spec §9). Scope will be re-planned after v1 ships and
> dogfooding shows where type errors hurt most.

## Goal

A native type checker for Compact running as-you-type, unlocking the feature tier that
v1 deliberately excluded: real-time type errors, type-aware completion, signature help,
and inlay hints.

## Why

v1's semantic ceiling is name resolution; every type-level mistake waits for
compile-on-save (M4, requires the toolchain). Compact's type system is where the
language is genuinely unusual, and where an analyzer can beat the compiler's
batch-error UX.

## Expected scope

- The **`Uint<0..n)` / `Uint<n>` subtype lattice** — bounded-integer widening/narrowing
  rules (the single most distinctive part of the type system).
- **Mandatory explicit generic specialization** — Compact has no inference of generic
  args; the checker enforces and exploits that.
- **Ledger ADT typing** — the builtin ADTs' method surfaces, typed properly (replacing
  M3's purely syntactic method table).
- Tuple/vector covariance rules.
- Witness/circuit signature checking within Compact (cross-language TS binding is
  separate — see below).
- Downstream features: type errors as native diagnostics, type-aware completion
  (member access on struct-typed values, ranking), signature help, inlay hints
  (types of `const` bindings, etc.), and code actions where obvious.

## Explicitly out (even in v2)

- Disclosure/taint analysis (v3).
- Witness-to-TypeScript cross-language navigation/checking (opportunistic, spec §9).
- Const-expression evaluation beyond what `Uint` bounds and `Vector` sizes require.

## Facts / constraints to honor

- **Ground truth is the compiler's Scheme type checker at the pinned tag** — the same
  verify-first discipline as the resolver: extract the actual rules from source, build
  a compiler-differential test suite (native checker vs `compactc` verdicts over the
  corpus) before trusting any rule from docs or training data.
- Type-bearing queries must stay memoized-recompute-shaped (the spec's Approach A kept
  query-shaped APIs precisely so salsa could slot in if type checking makes
  recomputation too hot — this is the milestone where that decision gets re-examined).
- Diagnostic wording should track compiler wording where practical, so users see one
  vocabulary across native and on-save diagnostics.
