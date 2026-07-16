# compact-analyzer v2b — Type-System Foundation — Design

**Date:** 2026-07-15
**Status:** Approved
**Author:** Aaron Bassett (with Claude)

Foundation design for **v2b (native type checking)**, the type-system sub-milestone of
the v2 program (`docs/superpowers/specs/2026-07-14-v2-native-type-checking-design.md`).
It supersedes the assumptions in the v2b draft outline
(`docs/superpowers/plans/2026-07-14-v2b-type-system.md`) that were invalidated by a
drift review against the **as-merged v2a** engine.

This spec deliberately covers the **foundation only** — the infrastructure every type
rule sits on. The distinctive type *rules* (the `Uint<0..n)` lattice, primitives/casts,
generic specialization, ledger-ADT typing, tuple/vector covariance, witness/circuit
signature checking) are **out of scope here**; each is planned just-in-time after its
semantics are extracted from compiler source (§9 of the v2 program spec). **This spec
asserts no Compact type semantics.**

## 1. Why this spec exists (drift from the v2b draft)

The v2b draft outline assumed *"type queries are salsa tracked functions over v2a's
`parsed`/`file_symbols` outputs **and the M2 resolver**."* A drift review of the merged
v2a code found that premise false in one material way:

- **The M2 resolver is not in salsa.** v2a was deliberately parse-only and
  behavior-preserving. The only tracked queries are `parsed`, `file_symbols`,
  `raw_imports` (`crates/analyzer-core/src/db.rs`). Name resolution is entirely
  imperative `impl AnalysisHost` `&mut self` code (`crates/analyzer-core/src/resolve.rs`):
  `resolve()` calls `self.analyze()`, walks includes across files with cycle detection,
  interns `FileId`s, and performs **disk I/O** via `cached_source_pathname`. A pure salsa
  tracked query cannot call it, yet type inference fundamentally needs name resolution.

Three smaller findings, confirmed accurate and folded into the design below:

- No `item_tree(src)` tracked query exists; `ItemTree`/`Symbol` derive `Clone, Debug`
  but not `PartialEq` (every field is already `PartialEq`, so the derive is trivial).
- The ledger-ADT table (`ledger_adts.rs`) stores unparsed `sig: String` strings and is
  accessed through a `&mut self` name-match with an accepted limitation (R1-M7).
- Parse diagnostics are inline in `FileAnalysis`; **resolution diagnostics are a
  separate host method** (`resolution_diagnostics`), not in salsa — so there is no single
  established precedent for where type diagnostics live.

## 2. Decisions (settled during brainstorming, 2026-07-15)

| Question | Decision |
|---|---|
| Resolution strategy | **Lift resolution into salsa** — a tracked def-map / scope layer. Not host-side type-checking (which would forfeit v2a's entire incremental rationale). |
| Migration scope | **Wholesale.** The tracked layer becomes the single canonical resolver; the existing host `resolve()` / `resolution_diagnostics()` are re-pointed at it. Behavior-preserving; **no shims/migrations** (pre-release project). |
| `Ty` model | **salsa `#[salsa::interned]`** — id-equality, workspace-shared, native to tracked queries. |
| Differential harness | **Hybrid.** Binary accept/reject verdict over the corpus as the blocking gate; per-fixture **rule-tagged** checks for precision (not message-identical). |
| Decomposition | **Two foundation plans + just-in-time rule plans.** v2b.1 (resolution→salsa) execution-ready now; v2b.2 (Ty + inference + harness + trivial slice), the per-rule plans, and v2b.final (VS Code integration) as outlines. |
| Diagnostics surfacing | **Tracked query returning `Arc<[Diagnostic]>`** (lift `resolution_diagnostics` into a tracked query too); **not** salsa accumulators. |

## 3. Architecture: resolution into salsa + the workspace-input seam

The central problem is crossing files inside a *pure* tracked query when file-finding is
*impure* (disk probes, VFS interning). The seam:

- **The host keeps all impure work.** VFS interning, `find_source_pathname` disk probes,
  and building the dependency graph already live in `AnalysisHost::index_file` /
  `cached_source_pathname`. They stay there.
- **The host publishes the resolved graph into a salsa input.** A workspace /
  dependency-graph input carries the resolved import/include edges and the
  `FileId → SourceText` mapping. The host `set`s it whenever the workspace index changes
  (the same events that already drive `index_file`).
- **Tracked resolution and type queries read that input purely** and cross files by
  calling other tracked queries (e.g. resolving an imported name reads the target file's
  tracked def-map). No query performs I/O.

**Input granularity (avoid over-invalidation).** Per-file *text* stays on the existing
per-file `SourceText` inputs from v2a; the new workspace input carries only the
**structural** graph — resolved import/include edges and the `FileId → SourceText`
mapping — which changes solely on structural events (a file added/removed, an
import/include edited), not on ordinary in-body keystrokes. A single-file body edit
therefore bumps only that file's `SourceText`, leaving the graph input (and every
unaffected file's def-map) backdated. This granularity is what preserves the "no
keystroke-latency regression" done-bar; a monolithic input keyed on all content would
violate it and is explicitly rejected.

This is rust-analyzer's `CrateGraph`-as-input pattern: impurity is quarantined at the
host→input boundary; everything above it is a pure, incrementally-tracked function of
that input.

**Backdating firewall.** v2b.1 derives `PartialEq` on `Symbol`/`ItemTree` and adds a
tracked `item_tree(src) -> Arc<ItemTree>` query with normal `eq`. The def-map depends on
`item_tree`, **not** on `parsed` (which embeds the full green tree and can never
backdate). Trivia-only edits therefore leave `item_tree` unchanged, backdate, and skip
re-resolution and re-checking.

**Wholesale re-point.** Once the tracked def-map exists, `resolve()`, `nav_info()`,
`resolution_diagnostics()`, and the other M2 host entry points are re-implemented on top
of it. The done-bar for this migration is that **every existing M2 test stays green** —
goto-definition, hover, cross-file imports/includes, stdlib resolution, workspace
diagnostics — with zero new user-facing behavior. Because the project is pre-release,
the old imperative resolution internals are deleted outright rather than kept behind a
shim.

## 4. The `Ty` model and inference infrastructure

- `Ty` is a salsa `#[salsa::interned]` type universe: each distinct type is interned
  once, equality is id comparison, and types are shared workspace-wide.
- Inference exposes a single tracked entry point per checkable body (circuit / witness /
  constructor body, and type-level positions). It depends on the tracked def-map and the
  `item_tree`/workspace input — never on the host.
- The IDE layer converts `Ty` into a display string for hover / completion detail;
  `Ty` values do not escape `analyzer-core`.
- **Single-threaded + cooperative cancellation is preserved.** The engine stays
  single-threaded; long walks continue to poll `should_continue`. No salsa cross-thread
  `Cancelled` is introduced.
- The foundation ships exactly **one** trivial rule — primitive-literal typing — as a
  vertical slice, purely to exercise the inference entry point and the harness
  end-to-end. It is not a claim about Compact's primitive semantics beyond what the slice
  fixture (captured from `compactc`) pins.

## 5. Compiler-differential harness (hybrid)

Ground truth is `compactc`'s **type-checking-phase** verdict at the pinned tag — never
docs, never training data. Two tiers:

1. **Binary corpus gate (blocking).** For each of the ~486 corpus files: native reports
   a type error **iff** `compactc` rejects at the type-checking phase. Any disagreement
   is our bug. This is the release-level gate from the program spec §8.1.
2. **Per-fixture rule-tagged checks (precision).** Each rule ships small fixtures whose
   expected verdict is captured from real `compactc` accept/reject behavior *before* the
   rule is implemented. The fixture asserts not just the verdict but that a rejection is
   **attributable to the rule under test** (rule-tagged), without demanding
   message-identical output.

Diagnostic **wording** tracks compiler wording "where practical" but is **not** gated by
the harness — so wording/span drift never manufactures a false disagreement.

**Toolchain dependency (verify at plan time).** The exact `compactc` invocation that
isolates the type-checking phase from parse and ZK-codegen phases, and its
accept/reject/exit semantics, are **unverified in this spec** and must be pinned against
the real toolchain (via `/verify` or a `midnight-verify` agent) during v2b.2 planning.
The harness *design* (binary + rule-tagged) is toolchain-agnostic; only the invocation
detail is deferred.

## 6. Diagnostics surfacing

Type and resolution diagnostics are produced by **tracked queries returning
`Arc<[Diagnostic]>`**. `resolution_diagnostics` is lifted from a host method into such a
query so resolution and type diagnostics share one shape. Salsa **accumulators are not
used** — they buy cross-query diagnostic collection that a single-threaded model does not
need, and inline returns match v2a's existing precedent. Type diagnostics merge with
M4's compile-on-save diagnostics through the existing dual-source tagging so users see
one vocabulary.

`compactp_diagnostics::Diagnostic` has no `PartialEq` (it drove v2a's `no_eq`), so a
diagnostics-returning query either wraps results so downstream consumers backdate on the
`PartialEq`-able projection, or accepts non-backdating for the diagnostics query
specifically (as `parsed` already does). The plan picks the concrete shape; either is
consistent with v2a.

## 7. Ledger-ADT typing (foundation touch-point, rule deferred)

The typed ADT method surfaces that *replace* M3's syntactic table are a **rule**, planned
just-in-time (it requires re-reading `midnight-ledger.ss` at the pinned tag). The
foundation only notes the integration point: the JSON `sig` strings in
`assets/ledger_adts_*.json` will be parsed into `Ty`s, and `ledger_field_adt`'s
name-only match (accepted limitation R1-M7) is superseded by type-driven resolution.
No ADT typing is specified here.

## 8. Decomposition & deliverables

| Plan | Content | Done-bar | Written now |
|---|---|---|---|
| **v2b.1 — Resolution → salsa** | `item_tree` query + `PartialEq` derives; workspace/dependency-graph salsa input; tracked def-map/scope layer; `resolve()`/`nav_info()`/`resolution_diagnostics()` re-pointed; old imperative internals deleted. | Every M2 test green; no new user-facing behavior; no keystroke-latency regression. | **Execution-ready** |
| **v2b.2 — Type foundation** | Interned `Ty`; inference entry point; hybrid differential harness; trivial primitive-literal vertical slice. Pins the `compactc` type-phase invocation. | Harness runs green over the corpus binary gate with the trivial slice; slice fixture rule-tagged. | Outline |
| **v2b.3 … N — per rule** | One plan each: `Uint` lattice, primitives/casts, generic specialization, ledger-ADT typing, tuple/vector covariance, witness/circuit signatures. | Rule's fixtures agree with `compactc`; corpus gate stays green. | Outline (named + sequenced) |
| **v2b.final — VS Code integration** | Native type diagnostics in the Problems panel; native-vs-compiler toggle (existing config + restart-to-apply pattern). | Diagnostics verified in-editor in the extension; toggle works. | Outline |

Each per-rule plan (v2b.3…N) is authored **just-in-time**: after the prior plan lands
and after that rule is extracted from compiler source under the §9 verify-first
procedure. No rule plan is written ahead of its extraction.

## 9. Inherited constraints (reconfirm at each plan)

- **Verify-first.** Compiler source at the pinned tag is ground truth; every rule is
  pinned by a `compactc` differential fixture built before implementation. Training data
  about Compact is unreliable and never a source.
- **No LSP types in `analyzer-core`;** byte offsets only. Protocol conversion stays in
  the `compact-analyzer` binary.
- **VS Code done-bar.** v2b is not done until native type diagnostics are integrated into
  and verified in the extension (program spec §8).
- **Single-threaded** analysis with cooperative `should_continue` cancellation.
- **Pre-release.** No backwards-compatibility shims, migrations, or dual code paths —
  clean wholesale replacement.
- **Never commit `[patch.crates-io]`.**

## 10. Out of scope (this spec)

- All distinctive type *rules* and their semantics (deferred to just-in-time rule plans).
- Type-aware UX — completion, signature help, inlay hints (that is v2c).
- Disclosure/taint analysis (v3); witness↔TypeScript cross-language checking; const-eval
  beyond `Uint`/`Vector` needs; salsa input GC / LRU (noted by v2a as a future concern,
  addressed only if it bites during v2b).
