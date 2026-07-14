# v2 — Native type checking (committed program)

> **Committed and decomposed** as of 2026-07-14. Program design:
> `docs/superpowers/specs/2026-07-14-v2-native-type-checking-design.md` — treat it as
> truth; this file is the lean context behind it.
>
> Decomposed into four sub-milestones plus an early, independent detection slice:
> **v2a** salsa engine swap (behavior-preserving) → **v2b** type system (core +
> distinctive rules + type-aware diagnostics) → **v2c** type-aware UX (completion,
> signature help, inlay hints) → **v2-M6** upstream-automation tail (the heavy half of
> M6, now covering v2's type-rule tables). **M6-detect** (version-watch + drift report +
> differential fuzz) lands early and independently, outside this chain — see
> [m6-upstream-automation.md](m6-upstream-automation.md).
>
> Settled decisions: salsa is adopted **up front** as v2a, before any type code, so the
> dense type-dependency graph is salsa-tracked from day one rather than hand-rolled and
> later ripped out; **all four** UX features are in-scope; the release bar requires
> differential agreement with `compactc` over the corpus **and** in-editor integration
> in the VS Code extension.
>
> **Implementation plans** (in `docs/superpowers/plans/`, 2026-07-14): v2a
> `2026-07-14-v2a-salsa-engine-swap.md` is the **execution-ready** plan; v2b/v2c/v2-M6
> (`…-v2b-type-system.md`, `…-v2c-type-aware-ux.md`, `…-v2-m6-upstream-automation-tail.md`)
> are **draft outlines** — an agent must review prior sub-milestones' as-merged state for
> drift and re-run writing-plans before executing them.

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
- **The salsa swap is settled: it happens up front, as v2a, behavior-preserving.** The
  spec's Approach A kept query-shaped APIs precisely so salsa could slot in; v2 exercises
  that option before any type code lands, because the type layer's dense cross-declaration
  dependency graph would otherwise be hand-rolled and painful to replace later. v3
  inherits the salsa engine.
- Diagnostic wording should track compiler wording where practical, so users see one
  vocabulary across native and on-save diagnostics.
