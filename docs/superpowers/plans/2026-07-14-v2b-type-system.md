# v2b — Type System — DRAFT / OUTLINE (not execution-ready)

> ⚠️ **DRAFT OUTLINE — DO NOT EXECUTE AS-IS.** This is a forward-context sketch captured
> alongside the v2 program spec so we don't lose sight of what comes after v2a. It is
> **not** a bite-sized, execution-ready plan.
>
> **This outline is highly likely to change** based on implementation decisions made while
> building the prior sub-milestone(s) — above all **v2a (the salsa engine swap)**, whose
> final `db.rs` query shapes, tracked-struct choices, and input model directly determine
> how the type layer is expressed.
>
> **Before turning this into an actual implementation plan, the executing agent MUST:**
> 1. Re-read the v2 program spec `docs/superpowers/specs/2026-07-14-v2-native-type-checking-design.md`.
> 2. Review the **as-merged** implementation of every prior sub-milestone (start with
>    `crates/analyzer-core/src/db.rs` and `analysis.rs` as they actually landed) and the
>    current project state — checking for **drift** between what this outline assumes and
>    what exists.
> 3. Run `superpowers:brainstorming` if the drift is material, then `superpowers:writing-plans`
>    to produce the real, TDD, bite-sized plan.
>
> Treat every "Phase" below as a hypothesis to validate against reality, not a commitment.

**Goal (from spec §5):** A native Compact type checker — type representation + inference
infrastructure plus the language's distinctive rules — running on the v2a salsa engine,
with type-aware diagnostics that agree with `compactc`.

**Architecture (provisional):** Type queries are salsa tracked functions over v2a's
`parsed`/`file_symbols` outputs and the M2 resolver. Types are likely modelled as salsa
`#[salsa::interned]` structs (cheap equality, shared across the workspace) or a plain
interned arena — **decide against v2a's actual tracked-struct ergonomics**. Diagnostics
may move to a `#[salsa::accumulator]` or stay inline as in v2a — decide for consistency
with how v2a ended up surfacing parse diagnostics.

**Tech Stack:** Rust (edition 2024, 1.90+), `salsa = "0.28"`, `compactp_*` CST/AST, the
M2 resolver (`crates/analyzer-core/src/resolve.rs`, `item_tree.rs`).

## Global Constraints (inherited — reconfirm at plan time)

- **Ground truth is the compiler's Chez Scheme type checker at the pinned tag** — never
  docs, never training data. Every rule is pinned by a **compiler-differential fixture**
  (native verdict vs `compactc`) built *before* the rule is implemented (spec §9).
- No LSP types in `analyzer-core`; byte offsets only.
- Type-aware diagnostics must be **integrated into and verified in the VS Code extension**
  (spec §8), including a `compact-analyzer.*` toggle for native-vs-compiler diagnostic
  preference, following the existing config + restart-to-apply pattern.
- Never commit `[patch.crates-io]`.

## Provisional file structure

- New: `crates/analyzer-core/src/ty/` — type representation, inference queries, the
  `Uint` lattice, generic specialization, covariance, signature checking. (Split by
  responsibility; keep files focused.)
- Modify: `crates/analyzer-core/src/db.rs` — add type queries alongside v2a's parse queries.
- Modify: `crates/analyzer-core/src/ledger_adts.rs` — the curated method **table** is
  superseded by *typed* ADT method surfaces; reconcile (M3's syntactic table becomes the
  fallback/seed for the typed model).
- Modify: `crates/compact-analyzer/src/server.rs` + `lsp_utils.rs` — surface type
  diagnostics (merge/tag against compile-on-save diagnostics; reuse M4's dual-source tagging).
- Modify: `editors/vscode/` — native-vs-compiler diagnostic toggle setting + wiring.

## Phase outline (coarse — each becomes several TDD tasks in the real plan)

1. **Type representation + inference infra.** The `Ty` model, an inference/query entry
   point over the resolver, and the differential-test harness (native vs `compactc` over
   the corpus) — stand this up *first* so every subsequent rule is validated against it.
2. **`Uint<0..n)` subtype lattice.** Bounded-integer widening/narrowing — the most
   distinctive rule; extract exact bounds behavior from compiler source.
3. **Primitives + casts (`as`).**
4. **Mandatory explicit generic specialization.** Enforce (no inference of generic args)
   and exploit for typing.
5. **Ledger-ADT typing.** Replace M3's syntactic method table with typed method surfaces
   (Counter/Cell/Map/Set/List/MerkleTree/HistoricMerkleTree/Kernel).
6. **Tuple/vector covariance.**
7. **Witness/circuit signature checking** within Compact (cross-language TS binding stays
   out — spec §12).
8. **Type-aware diagnostics + VS Code integration.** Diagnostics emerge as rules land;
   this phase closes the loop into the extension and the native-vs-compiler toggle.

## Key open questions to resolve at plan time

- **Type modelling on salsa:** interned `Ty` vs tracked structs vs a plain arena — depends
  on v2a's realized ergonomics and on how hot type queries turn out to be.
- **Diagnostic surfacing:** salsa accumulator vs inline; must match v2a's final choice and
  M4's dual-source tagging so users see one vocabulary.
- **`ledger_adts.rs` reconciliation:** how much of the M3 table survives as data vs is
  regenerated from compiler source (this feeds v2-M6's type-table drift target).
- **Corpus differential scope:** which corpus files exercise which rules; how disagreements
  are triaged (ours-vs-compiler) given docs are known to drift from the shipped compiler.
