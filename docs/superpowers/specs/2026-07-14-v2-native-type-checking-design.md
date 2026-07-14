# compact-analyzer v2 — Native Type Checking — Program Design

**Date:** 2026-07-14
**Status:** Approved
**Author:** Aaron Bassett (with Claude)

Program-level design that graduates **v2 (native type checking)** from "direction,
not commitment" (project design `docs/superpowers/specs/2026-07-06-compact-analyzer-design.md`
§9; milestone context `.superpowers/milestones/v2-native-type-checking.md`) into a
committed, decomposed program. It builds directly on v1 (M1–M5), whose code is merged
to `main`, and it **absorbs and resequences M6 (upstream automation)** so the heavy
half of M6 becomes v2's final phase.

This is a **program spec**, deliberately at roadmap altitude: it fixes goals, scope
boundaries, sequencing, and the cross-cutting disciplines. Each sub-milestone below
gets its own detailed spec + implementation plan later (as M2 → M2a/M2b did).

## 1. Context

v1's semantic ceiling is **name resolution**. Every type-level mistake a developer
makes waits for compile-on-save (M4, which needs the toolchain) and arrives as a
batch of plain-text `compactc` errors. Compact's type system is where the language is
genuinely unusual — the `Uint<0..n)` subtype lattice, mandatory explicit generic
specialization, ledger-ADT method typing — and it is exactly where a native analyzer
beats the compiler's batch-error UX with as-you-type feedback.

Two structural facts shape this program:

- **The incrementality decision comes due here.** v1 chose "Approach A" (memoized
  recompute) with deliberately query-shaped APIs *precisely so salsa could replace the
  engine later without re-architecture* (project design §2). v2 is where that option
  is exercised — see §4.
- **v2 creates a new upstream-mirroring surface.** The type-rule tables v2 extracts
  from the compiler join the existing mirror surfaces (stdlib stub, ledger-ADT table,
  stderr parser, grammar, version constants, corpus) that M6 was created to keep in
  sync. This is why M6's heavy half is resequenced to the tail of v2 rather than
  landing "any time after M4" — the automation can only cover the type tables once
  they exist. See §7.

## 2. Decisions (settled during brainstorming)

| Question | Decision |
|---|---|
| Deliverable shape | A **program spec** decomposing v2 into ordered sub-milestones, each of which later gets its own spec + plan. Not one monolithic implementation spec. |
| Sub-milestone count | **Four**: v2a (engine swap), v2b (type system), v2c (type-aware UX), v2-M6 (automation tail) — plus **M6-detect** landing early and independently, outside the numbered chain. |
| Incrementality engine | **Commit to the salsa swap up front**, as v2a, *before* any type code lands. Behavior-preserving. Rationale in §4. |
| UX ceiling | **All four** downstream features are in-scope for v2: type-aware diagnostics, completion, signature help, inlay hints. |
| M6 seam | **Split.** The cheap detection half (version-watch cron, drift report, differential fuzz) lands early and independently. Only the heavy half (regeneration scripts, agent-update skill, headless PR automation) is resequenced to the v2 tail as `v2-M6`. |
| Release bar | Differential agreement with `compactc` over the corpus **and** all four features integrated into and verified through the VS Code extension. See §8. |
| Ground truth | The compiler's Chez Scheme type checker at the pinned tag — never docs, never training data. Every rule pinned by a compiler-differential fixture *before* implementation. See §9. |
| Scope out (unchanged) | Disclosure/taint analysis (v3), witness↔TypeScript cross-language checking (opportunistic), const-eval beyond `Uint`/`Vector` needs, auto-merge of any M6 update, grammar changes (upstream in compactp). See §12. |

## 3. Program decomposition

| # | Milestone | Goal | Done-bar |
|---|---|---|---|
| **v2a** | Salsa engine swap | Rebuild the query layer on salsa. **Behavior-preserving** — no new user-facing behavior. | Every v1 feature, the full test suite, and corpus smoke pass on the new engine, with no keystroke-latency regression. |
| **v2b** | Type system | Type representation + inference infra, the `Uint<0..n)` subtype lattice, primitives/casts, mandatory generic specialization, ledger-ADT typing (replaces M3's syntactic method table), tuple/vector covariance, witness/circuit signature checking; type-aware diagnostics emerge as rules land. | Compiler-differential suite agrees with `compactc` over the ~486-file corpus; each rule pinned by a fixture from real compiler accept/reject at the pinned tag; native type diagnostics surfaced in the VS Code extension. |
| **v2c** | Type-aware UX | Completion (typed member access on struct/ADT values + ranking), signature help, inlay hints (types of `const` bindings, etc.). Pure consumers of v2b. | All three work on a real multi-file project **and are verified in-editor in the VS Code extension** (capabilities advertised + client wiring), with no checker regressions. |
| **v2-M6** | Automation tail | Regeneration scripts + agent-update skill + headless PR automation, now covering v2's type-rule tables alongside the stdlib/ADT/stderr surfaces. | Regenerated tables pass all mechanical gates; an agent PR embeds the drift report; nothing auto-merges. |
| **M6-detect** | *(early, independent — not in the v2 chain)* | Version-watch cron + drift report (stdlib/ADT/stderr) + scheduled differential fuzzing (compactp vs compactc), findings fed upstream. | Lands whenever; already useful today (0.31.1 shipped while v1 verified against 0.31.0). |

Sequencing: `v2a → v2b → v2c → v2-M6`, with `M6-detect` parallel/independent and
landable before or during any of them.

## 4. Architecture: the salsa engine swap (v2a)

v2a replaces the ad-hoc memoized-recompute engine (content-hash parse cache,
dirty-flagged workspace index, generation-counter cancellation) with salsa **before**
any type code is written.

**Why up front rather than gated on evidence.** Type inference builds a *dense*
cross-declaration dependency graph — a type at one use site depends transitively on
declarations elsewhere, on generic specialization, and on the ledger-ADT table. On the
memoized engine that dependency tracking would have to be **hand-rolled**, and
hand-rolled dependency tracking is the single hardest thing to later rip out and
replace with salsa, because dependency tracking *is* salsa's core value proposition —
you would write, then delete, the exact machinery the framework provides. The cheapest
moment to swap is therefore before the type layer exists, not after. Committing now
also eliminates the risk of silent drift out of query shape (hidden caches, mutable
state closed over by queries) accumulating under v2b and making a later swap expensive.

**Behavior-preserving by design.** v2a introduces zero new user-facing behavior; its
entire done-bar is that the existing v1 feature set, test suite, and corpus smoke pass
unchanged on salsa. This keeps the risky infrastructure swap verifiable in isolation,
before any semantic work depends on it.

**Shared with v3.** The salsa engine becomes shared infrastructure that v3's
interprocedural disclosure analysis inherits natively — v3 was previously expected to
be where the engine question first came due; v2a resolves it earlier and once.

## 5. The type system (v2b)

v2b is the checker's spine: a type representation and inference infrastructure, plus
the rules that make Compact's type system distinctive. Core and distinctive rules are
fused into one sub-milestone because they are tightly coupled (generics, covariance,
and ADT typing interact constantly). Scope:

- **`Uint<0..n)` subtype lattice** — bounded-integer widening/narrowing, the single
  most distinctive part of the type system.
- **Primitives and casts** (`as`).
- **Mandatory explicit generic specialization** — Compact infers no generic args; the
  checker enforces and exploits that.
- **Ledger-ADT typing** — the builtin ADTs' method surfaces typed properly, *replacing*
  M3's purely syntactic method table.
- **Tuple/vector covariance.**
- **Witness/circuit signature checking** within Compact (the cross-language TypeScript
  binding remains out — §12).
- **Type-aware diagnostics** emerge incrementally as each rule lands; they are not a
  separate phase.

Every rule is extracted from the compiler's type checker source and pinned by a
compiler-differential fixture before implementation (§9). Diagnostic wording tracks
compiler wording where practical, so users see one vocabulary across native and
on-save diagnostics.

## 6. Type-aware UX (v2c)

Three features, all **pure consumers** of the v2b checker:

- **Type-aware completion** — member access on struct-typed and ADT-typed values,
  and type-informed ranking, building on M3's syntactic completion.
- **Signature help** — parameter hints at call sites; the server advertises
  `signatureHelpProvider` with `(` and `,` trigger characters.
- **Inlay hints** — types of `const` bindings and similar; the server advertises
  `inlayHintProvider`.

Because these are LSP capabilities the thin VS Code client largely gets for free once
advertised, the real work per feature is (a) computing the answer from the checker and
(b) the client-side wiring and in-editor verification — signature-help trigger
characters, the inlay-hint toggle VS Code gates behind a setting, and confirming each
behaves in the actual editor rather than only over LSP integration tests.

## 7. Upstream automation: the M6 split and the v2-M6 tail

M6 (`.superpowers/milestones/m6-upstream-automation.md`) is **split along its natural
seam**:

- **Detection half → `M6-detect`, early and independent.** The version-watch cron, the
  drift report (stdlib signatures, ledger-ADT surfaces, stderr shape), and scheduled
  differential fuzzing are cheap, fully mechanical, and *already useful* — drift exists
  today. This half is not gated on v2 and can land any time.
- **Heavy half → `v2-M6`, the program's final phase.** The regeneration scripts, the
  repo-local agent-update skill, and the headless PR automation are resequenced to the
  end of v2 because v2b creates a **new upstream-mirroring surface** — the type-rule
  tables — that this machinery must cover alongside the existing stdlib/ADT/stderr
  surfaces. Building the regeneration/verification automation before those tables exist
  would mean automating an incomplete surface and revisiting it immediately.

Everything M6 already commits to still holds: docs are never ground truth (compiler
source is), every verification gate is mechanical and non-negotiable, and **nothing
auto-merges** — every update lands as a human-reviewed PR. The `v2-M6` regeneration and
its gates simply extend to the type tables.

This resequencing supersedes M6's prior "natural slot after M4, alongside M5" note.

## 8. Release bar & VS Code integration

v2 is **done** when both hold:

1. **Correctness:** the native checker's verdicts **agree with `compactc`** across the
   ~486-file corpus. Disagreements are our bugs by definition; v1's dual-source
   diagnostic tagging still shows users compiler ground-truth on save.
2. **Delivered where users are:** all four features — type diagnostics, completion,
   signature help, inlay hints — are **integrated into the VS Code extension and
   verified in-editor**, not merely exercised over LSP integration tests. The VS Code
   extension is how most users interact with the project, so "works over raw LSP" is
   not "works for users."

The integration clause has per-milestone teeth: v2b's done-bar requires native type
diagnostics in the extension's Problems panel (with a `compact-analyzer.*` toggle for
native-vs-compiler diagnostic preference, following the existing config +
restart-to-apply pattern); v2c's done-bar requires signature help and inlay hints
advertised by the server and wired + verified in the extension (inlay hints behind the
usual VS Code setting toggle). Extension wiring is thin per-feature and folded into
each feature-bearing milestone rather than made a separate milestone.

## 9. Verify-first discipline & differential testing (cross-cutting)

Ground truth is the compiler's **Chez Scheme type checker at the pinned tag** — never
docs, never training data (the repo's standing rule, and the training-data-unreliability
rule applies doubly here). The discipline mirrors M2's import-semantics work and v3's
disclosure work:

1. Read the relevant part of the compiler's type-checking pass at the pinned tag.
2. Record the exact rule (source/sink/lattice/specialization behavior).
3. Build a **compiler-differential fixture** — native checker verdict vs `compactc`
   verdict — from real compiler accept/reject behavior, *before* implementing the rule.
4. Run the differential over the corpus; any disagreement is a bug on our side.

This discipline is *also the reason* v2-M6 belongs at the tail: the type-rule tables it
regenerates and verifies are extracted under exactly this procedure, and M6's
automation productionizes the archaeology rather than inventing it.

## 10. Ledger & context-file changes this program triggers

This spec is accompanied by edits to the milestone tracking (all authored here, not
deferred):

- `.superpowers/milestones/v2-native-type-checking.md` — flips from "direction" to
  committed; records the four-way decomposition, the salsa-up-front decision, and the
  all-four-UX ceiling; links this spec.
- `.superpowers/milestones/m6-upstream-automation.md` — records the **split**:
  detection half → early/independent (`M6-detect`); heavy half → resequenced as
  `v2-M6`, superseding the "slot after M4 / alongside M5" note.
- `.superpowers/milestones/v3-disclosure-analysis.md` — back-reference: v3 now
  **inherits** the salsa engine from v2a rather than being where the engine question
  first comes due.
- `.superpowers/milestones/README.md` — status table + cross-cutting notes updated for
  the new ordering (v2 committed and decomposed; M6 split and resequenced).

## 11. Dependencies, sequencing & relationship to v3

- **Builds on v1** (M1–M5), merged to `main`: the workspace index, resolver, and
  query-shaped APIs are the substrate v2a migrates and v2b consumes.
- **Internal order:** `v2a → v2b → v2c → v2-M6`. v2c cannot start before v2b (it
  consumes the checker); v2-M6 cannot complete before v2b (it automates v2b's tables).
  `M6-detect` is independent and can land before or during any of them.
- **v3 relationship:** v3 (native disclosure analysis) inherits salsa from v2a and
  needs v2b's types for field/element-level taint precision. This program does not
  commit v3, but it removes v3's biggest architectural unknown by settling the engine.

## 12. Out of scope (even in v2)

- **Disclosure/taint analysis** — stays v3.
- **Witness↔TypeScript cross-language navigation/checking** — opportunistic (project
  design §9), not v2.
- **Const-expression evaluation** beyond what `Uint` bounds and `Vector` sizes require.
- **Auto-merging any M6 update** — human-reviewed PRs only.
- **Grammar / new-syntax support** — real engineering in compactp, upstream; the
  automation only detects parse drift and files upstream issues with fixtures.
- **Multi-version support** — one grammar/stub/type-table set per supported language
  version stands.
