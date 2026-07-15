# v2b Distinctive Type Rules — JUST-IN-TIME PLAN INDEX — DRAFT / OUTLINE (not execution-ready)

> ⚠️ **DRAFT OUTLINE / INDEX — DO NOT EXECUTE AS-IS.** This is not a plan; it is the
> ordered list of per-rule plans that come **after** v2b.2, each authored just-in-time.
>
> **Each rule below has UNKNOWN semantics until extracted from compiler source.** This
> index deliberately records **only the rule's name, its integration point, and its
> extraction target** — never the rule's behavior. Writing rule semantics here from
> training data is forbidden (v2 program spec §9; the training-data-unreliability rule
> applies doubly to type rules).
>
> **To author a rule's real plan, the executing agent MUST, per rule:**
> 1. Re-read v2 program spec §5, §9 and the v2b foundation design.
> 2. Review the **as-merged** v2b.2 harness + `Ty` universe and every prior rule for drift.
> 3. **Extract the rule from the compiler's Chez Scheme type checker at the pinned tag**
>    (`LFDT-Minokawa/compact`), record the exact rule, and build a `compactc`-differential
>    fixture (rule-tagged) that captures real accept/reject **before** implementing it.
> 4. Run `superpowers:writing-plans` for that single rule.

**Sequencing rationale:** foundational/most-distinctive rules first, because later rules
lean on the `Ty` machinery they establish (bounded integers underpin casts; generics and
covariance interact with ADT typing and signatures).

## Rule plan order (one plan each, just-in-time)

| # | Rule | Integration point in the tree | Extraction target |
|---|---|---|---|
| v2b.3 | **`Uint<0..n)` / `Uint<n>` subtype lattice** | new `ty/` lattice module; inference entry point | compiler type checker's bounded-integer widening/narrowing pass |
| v2b.4 | **Primitives + casts (`as`)** | `ty/` primitives; cast expr in inference | compiler's primitive type + cast rules |
| v2b.5 | **Mandatory explicit generic specialization** | `ty/` generics; def-map generic params (v2b.1) | compiler's generic-arg handling (no inference of args) |
| v2b.6 | **Ledger-ADT typing** (replaces M3's syntactic table) | `ledger_adts.rs` `sig: String` → parsed `Ty`; supersedes `ledger_field_adt` name-match (R1-M7) | `midnight-ledger.ss` `declare-ledger-adt` forms at the pinned tag |
| v2b.7 | **Tuple/vector covariance** | `ty/` covariance; assignability | compiler's tuple/vector variance rules |
| v2b.8 | **Witness/circuit signature checking** (in-Compact only; TS binding stays out — §12) | inference over witness/circuit decls | compiler's signature-checking pass |

Type-aware diagnostics are **not** a separate rule — each rule emits its diagnostics as it
lands (foundation spec §6). The VS Code integration + native-vs-compiler toggle is the
separate closing plan **v2b.final** (`2026-07-15-v2b-final-vscode-integration.md`).

## Standing constraints for every rule plan

- Ground truth is compiler source; a rule with no `compactc`-differential fixture is not
  started. The binary corpus gate must stay green after every rule.
- No LSP types in `analyzer-core`; `Ty` stays in-crate.
- Diagnostic wording tracks compiler wording "where practical" but is not harness-gated.
- Pre-release: replace M3's syntactic ADT table outright (no dual old/new path).
