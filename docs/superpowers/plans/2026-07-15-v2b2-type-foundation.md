# v2b.2 — Type Foundation (Ty + Inference Infra + Differential Harness) — DRAFT / OUTLINE (not execution-ready)

> ⚠️ **DRAFT OUTLINE — DO NOT EXECUTE AS-IS.** Forward-context sketch captured alongside
> the v2b foundation design. **Not** a bite-sized, execution-ready plan.
>
> **This outline is highly likely to change** based on implementation decisions made while
> building **v2b.1 (resolution → salsa)** — above all the final shapes of the tracked
> def-map, the `FileDeps`/`Workspace` inputs, and the `item_tree` firewall, which are the
> substrate the `Ty` layer queries.
>
> **Before turning this into an actual implementation plan, the executing agent MUST:**
> 1. Re-read the v2b foundation design `docs/superpowers/specs/2026-07-15-v2b-foundation-design.md`
>    and the v2 program spec `docs/superpowers/specs/2026-07-14-v2-native-type-checking-design.md` §5, §9.
> 2. Review the **as-merged** v2b.1 implementation (start with the tracked resolution
>    queries + inputs in `crates/analyzer-core/src/db.rs`) and the current project state —
>    checking for **drift** between this outline's assumptions and what exists.
> 3. Run `superpowers:writing-plans` (and `superpowers:brainstorming` first if drift is
>    material) to produce the real plan.

**Goal (foundation spec §4, §5):** The type-checking spine — a salsa-interned `Ty`
universe, a single tracked inference entry point per checkable body, and the **hybrid
compiler-differential harness** — proven end-to-end by exactly one trivial rule
(primitive-literal typing). No distinctive type rules land here.

**Architecture (provisional):** `Ty` is a `#[salsa::interned]` type universe. Inference is
a tracked query over v2b.1's def-map + `item_tree`. The harness runs the native verdict
against `compactc`'s type-checking-phase verdict: a **binary accept/reject corpus gate**
(blocking) plus **per-fixture rule-tagged** checks.

## Global Constraints (inherited — reconfirm at plan time)

- **Verify-first:** `compactc` at the pinned tag is ground truth. The primitive-literal
  slice fixture is captured from real `compactc` accept/reject **before** implementing it.
- **Toolchain detail to pin FIRST (foundation spec §5):** the exact `compactc` invocation
  that isolates the type-checking phase (vs parse / ZK-codegen) and its accept/reject/exit
  semantics are **unverified** — pin them against the real toolchain (via `/verify` or a
  `midnight-verify` agent) as the opening task, before the harness is designed around them.
- No LSP types in `analyzer-core`; byte offsets only. `Ty` does not escape the crate — the
  IDE layer gets a display projection.
- Single-threaded; cooperative `should_continue`; no salsa `Cancelled`.
- Diagnostics are tracked queries returning `Arc<[Diagnostic]>` (not accumulators).

## Provisional phase outline (each becomes several TDD tasks)

1. **Pin the `compactc` type-phase invocation** against the real toolchain; record exit /
   stdout / stderr semantics as fixtures. *(Blocking prerequisite.)*
2. **`Ty` universe.** `#[salsa::interned] Ty` with the minimal constructors the trivial
   slice needs; a `Ty → String` display projection for the IDE.
3. **Inference entry point.** One tracked query per checkable body over the def-map;
   returns `Ty` for expressions/positions the slice covers; emits type diagnostics as a
   tracked `Arc<[Diagnostic]>` query merged with M4's dual-source tagging.
4. **Hybrid harness.** (a) Binary corpus runner: native-verdict vs `compactc`-type-phase
   verdict over the ~486-file corpus, blocking on disagreement; (b) rule-tagged fixture
   runner: per-fixture expected verdict + rule attribution. Wire into `cargo test`, gated
   on toolchain availability (skip when `compactc` absent).
5. **Trivial vertical slice — primitive-literal typing.** The one rule proving the whole
   path: a fixture captured from `compactc`, a native rule, agreement asserted both ways.

## Carry-forwards from v2b.1 (final whole-branch review, 2026-07-15)

- **Add a `FileId → SourceText` map to the `Workspace` salsa input (highest-value cheap
  win).** v2b.1's `source_text_for` (in `db.rs`) recovers a cross-file `SourceText` for a
  resolved `Definition` by scanning `ws.file_deps(db).values()` and reading each file's
  `deps` — which is both **O(files × deps)** and, worse, takes a salsa dependency on
  *every* file's `FileDeps`, so any resolution reaching that arm (transitive cross-file
  member access) re-executes whenever *any* file's `FileDeps` is republished. v2b's type
  layer will drive `resolve()`/`item_tree` cross-file reads far harder, so add a
  `file_srcs: Arc<BTreeMap<FileId, SourceText>>` field to `Workspace` (published alongside
  the existing `file_deps` map, on the same keyset-grow/shrink events) and make
  `source_text_for` an O(1), dependency-narrow map lookup. Behavior-preserving; do it early
  in v2b.2 before the type queries inherit the over-broad dependency.

## Key open questions to resolve at plan time

- **`Ty` constructor set for the slice** — kept minimal; the real universe grows per rule.
- **Corpus runner performance** — running `compactc` over ~486 files may be slow; decide
  caching of compiler verdicts vs re-running, and CI vs local-only execution.
- **Verdict isolation** — depends entirely on Phase 1's toolchain findings.
- **Diagnostics backdating** — `Diagnostic` has no `PartialEq` (drove v2a's `no_eq`); decide
  the same non-backdating vs a `PartialEq` projection, consistent with v2b.1's choice.
