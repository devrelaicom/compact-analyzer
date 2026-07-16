# v3 — Native Disclosure Analysis (program design)

> **Status:** Committed + decomposed (2026-07-16). This is the program-level design for the
> full v3 flagship. It supersedes the lean context in
> `.superpowers/milestones/v3-disclosure-analysis.md` — treat this document as truth.
>
> **Decomposed** (research-first, then vertical) into: **v3-R** (extract the compiler's
> disclosure pass + build the `compactc`-differential harness) → **v3a** (intraprocedural
> disclosure) → **v3b** (interprocedural summaries) → **v3c** (diagnostics UX + `disclose()`
> quick-fix) → **v3-M6** (the deferred M6 upstream-automation heavy tail, re-sequenced here
> 2026-07-16 to also cover v3's new upstream-mirrored surface).
>
> **v3a gets a detailed implementation plan; v3b/v3c/v3-M6 are draft outlines** that must be
> re-planned just-in-time against the as-merged state of prior phases (drift is expected).

## 1. Goal

A native, real-time, interprocedural analysis that flags **implicit disclosure of
witness-derived (private) data as you type** — the same verdicts the Compact compiler's
"Witness Protection Program" (WPP) produces on compile, but live in the editor, with the full
witness→sink path rendered as clickable related-information locations and a quick-fix that
inserts `disclose()` at the right expression.

This is the flagship differentiator — the feature no other Compact tool has, and the reason the
program chose "staged to full native" (v1 resolution → v2 types → v3 disclosure) over
delegating semantics to the compiler.

## 2. Why

Disclosure errors are Compact's hardest and most privacy-critical error class: witness-derived
data reaching a public sink (ledger write/update, exported-circuit return, cross-contract call)
must be explicitly acknowledged with `disclose()`, or the compiler rejects the program. Today a
developer discovers these only at compile-on-save (M4), one save at a time, as a textual path in
an error message. Live analysis with clickable source→sink paths and a one-click `disclose()`
fix turns the language's most confusing error into its best-supported one.

Disclosure analysis also depends on the two prior programs, which is why it is sequenced last:
- **v2a salsa engine** — interprocedural summaries are memoized salsa queries; recompute on edit
  is the engine's job, not hand-rolled. v3 inherits this; the engine question is already settled.
- **v2b types** — field/element-level taint precision requires the type of every value (a struct
  field vs. an array element are tracked differently — see §3.3).

## 3. Ground truth — the compiler's WPP (extracted from source, not docs, not training data)

**Source of truth:** `compiler/analysis-passes.ss`, pass `track-witness-data`, in the compiler
monorepo `LFDT-Minokawa/compact` at the pinned compiler tag (compiler 0.31.x / language 0.23.0
— the same tag v2/v2b pin; v3-R re-confirms the exact tag/commit before any rule is trusted).
Cross-referenced with `doc/compact-reference.mdx` §"Explicit disclosure",
`doc/explicit-disclosure.mdx`, and `CHANGELOG_compactc.md` (PM-15585 "extend WPP").

The WPP is the pass named, in its own header comment, the "witness-protection program". It runs
as the `track-witness-data` nanopass over `Lwithpaths` and is implemented as an **abstract
interpreter**: it propagates *abstract values* (which sets of witness values a concrete value may
contain) through every circuit body, and records a **leak** whenever a value carrying a nonempty
witness set reaches a sink without having passed through `disclose()`.

> ⚠️ v3-R must re-derive every rule below from source against a differential fixture before it is
> implemented. This section records the model as extracted on 2026-07-16; treat specifics (exact
> "nature" strings, per-expression rules, native/ledger disclosure-flag tables) as *starting
> points for extraction*, not as settled implementation constants.

### 3.1 Taint sources — exactly three (`Witness-Info` datatype)

A witness value "enters the contract" at one of three origins. (Originally only witness returns;
constructor + circuit arguments were added in PM-15585 — CHANGELOG confirms.)

| Source | `Witness-Info` variant | Error phrasing (`complain`) |
|---|---|---|
| A witness function's return value | `Witness-Return-Value function-name` | "the return value of witness `<f>` at `<loc>`" |
| A constructor argument | `Constructor-Argument argument-name` | "the value of parameter `<a>` of the constructor at `<loc>`" |
| An exported-circuit argument | `Circuit-Argument function-name argument-name` | "the value of parameter `<a>` of exported circuit `<f>` at `<loc>`" |

Note the milestone doc under-counted: **exported-circuit arguments and constructor arguments are
private too**, not just witness call results.

### 3.2 Sinks — with a source-dependent asymmetry

A leak is recorded (`record-leak!`, keyed by `(sink-src, nature-string)`) when a tainted value
reaches:

- **the public ledger** (a write/update / ledger-op that stores the value) — for **all three**
  source kinds;
- **an exported circuit's return value** — for **witness return values only** (an exported
  circuit's own arguments are already known to its caller, so returning them is not a *new*
  disclosure; a witness return value is genuinely private local data);
- **a cross-contract call** (passing the value to another contract) — per CHANGELOG / stdlib docs.

**Implicit flow counts.** A write/update/return that is *conditioned on* a witness value
discloses it — `if (w) { F.increment(1) }`, and `&&`/`||`/comparisons (`<`, `==`, …). The `Abs`
lattice tracks booleans specially (`Abs-boolean true? witness*`) precisely to carry this.

### 3.3 Granularity — the abstract-value lattice (`Abs` datatype)

*"struct and tuple fields are tracked individually; array elements are tracked in the aggregate."*

| `Abs` variant | Represents |
|---|---|
| `Abs-atomic witness*` | a scalar carrying a set of witness values |
| `Abs-boolean true? witness*` | a boolean (tracked specially for conditional/implicit flow) |
| `Abs-multiple abs*` | a **struct or tuple** — each field/element tracked **individually** |
| `Abs-single abs` | an **array/vector** — all elements tracked **in aggregate** (one `abs`) |

This is *not a design choice for us* — matching the compiler bit-for-bit requires this exact
shape. It is the reason v3 needs v2b types: the interpreter dispatches on a value's type to build
the right `Abs` shape (`default-value type`), and to know struct/tuple arity and field types.

### 3.4 The sanitizer — `disclose()` only

`disclose(e)` replaces the witness set at every level of `e`'s abstract value with the empty set
(the `disclose` helper maps `Abs-atomic/boolean/multiple/single` to the same shape with `witness*`
cleared). It has **no runtime effect** — it only tells the WPP "pretend this value contains no
witness data." A later `remove-disclose` pass strips the marker. **Hashing does not sanitize**:
`transientHash`/`persistentHash` of witness data stays tainted (the path records "a hash of" as
an *exposure*, but the witness set survives) and still needs `disclose()` at the sink.

### 3.5 Path tracking — the witness→sink trail

Each witness value carries `path*`: a sorted set of paths, each a list of `path-point`s. A
`path-point` has `src` (location), `description` (e.g. "the argument of transientHash"), and
`exposure` (e.g. "a hash of", or "" when the value passes through unmodified). Points are recorded
at: **calls passing the value as an argument, `const` bindings whose RHS derives from it,
conditionals (`if`/`&&`/`||`), and comparisons.** Points inside standard-library code collapse to
the stdlib **entry** site (the disclosure is reported where user code enters the stdlib, with the
full exposure explanation).

### 3.6 Exact diagnostic shape (`complain` + `doc/compact-reference.mdx`)

WPP reports **all** leaks it finds (not first-only, since PM-15585), sorted by sink source
position. Each leak renders as:

```
potential witness-value disclosure must be declared but is not:
    witness value potentially disclosed:
      <source phrasing from §3.1>
    nature of the disclosure:
      <nature-string> might disclose <exposure> the witness value
    via this path through the program:
      <path-point description> at <loc>
      ...
```

The compiler's on-save form is prefixed `Exception: <file> line L char C:`; the M4 stderr parser
already handles that envelope. v3 renders the same information as a **native LSP diagnostic at the
sink** plus one `related_information` entry per source and per path-point (§4.3).

### 3.7 Function kinds the interpreter distinguishes (`Fun` datatype)

- `Fun-circuit src name var-name* expr uid` — a user circuit; interpreted via its body `expr`
  (interprocedural). Non-exported circuits are analyzed at each call with the caller's abstract
  arguments; exported circuits are also analysis roots (their args are sources).
- `Fun-witness abs` — a witness; its abstract return value is the type's `default-value` seeded
  with a fresh `Witness-Return-Value` record at the leaves.
- `Fun-native disclosure?* type` — a native/stdlib primitive; carries a **per-argument disclosure
  flag vector** (`native-entry-disclosure*`; `(discloses nothing)` vs. discloses). Ledger ADT ops
  likewise carry per-arg `discloses?` flags. These tables are an upstream-mirrored surface
  (v3-M6).

## 4. Architecture in the analyzer

### 4.1 Placement & dependencies

A new module in **`analyzer-core`** (e.g. `disclosure/`) implementing the abstract interpreter
over the existing HIR/item-tree, consuming:
- **v2b type inference** — `type_at`/the `Ty` universe — to build `Abs` shapes and drive
  field/element granularity;
- the **item tree** — to classify each callee as circuit / witness / native (the `Fun` split) and
  to identify exported circuits, the constructor, ledger fields, and cross-contract call sites;
- the **v2a salsa engine** — every query below is salsa-tracked and recomputed incrementally.

The analysis emits `analyzer_core::Diagnostic`s (with `secondary_spans` for the path), exactly
like the resolver and type checker; **no new diagnostic plumbing** is needed until the quick-fix.

### 4.2 Salsa query shape (interprocedural via summaries)

- `witness_disclosure_summary(circuit)` → an abstract transfer function: given abstract arguments,
  what abstract value does the circuit return, and which **internal** leaks does it record that
  depend only on its own body? Memoized per circuit; a caller's analysis depends on its callees'
  summaries, so editing a leaf circuit invalidates exactly its transitive callers.
- `disclosure_diagnostics(file)` → runs the interpreter from each **root** (exported circuit,
  constructor) in `file`, seeding `Circuit-Argument`/`Constructor-Argument` sources, and collects
  leaks. This is the query the LSP layer consumes.
- Cycle handling (recursive circuits) and summary fixpoint are v3b concerns; v3a is intraprocedural
  (single body, callees conservatively treated — see §7).

### 4.3 LSP surface

- **Diagnostics** (v3a+): native diagnostic at the sink; `related_information` = the source origin
  (§3.1) + one entry per non-stdlib path-point (§3.5), reusing the existing
  `secondary_spans → DiagnosticRelatedInformation` conversion in `lsp_utils.rs`.
- **Wording parity** (v3c): diagnostic text tracks the compiler's phrasing so users see one
  vocabulary across native (live) and `compactc` (on-save) diagnostics — same discipline as v2b.
- **Quick-fix** (v3c): a net-new `textDocument/codeAction` handler offering "Wrap in `disclose()`"
  at the flagged expression. Placement is subtle (the compiler dictates *which* expression is the
  disclosure point, and advises placing it *after* any obfuscation) — placement rules are derived
  from compiler-accepted fixes, differential-verified, not from intuition.
- **Toggle** (v3c): a `disclosureDiagnostics` on/off setting mirroring v2b.final's
  `typeDiagnostics`, so a user can fall back to `compactc`-on-save alone. Native and compiler
  diagnostics stay distinctly tagged; merge/dedup follows the existing `merge_diagnostics` rule.

## 5. Verify-first & differential strategy (non-negotiable)

Identical discipline to v2b (`v2b-rules-index.md`): **the compiler is the spec; disagreement is a
bug on our side.** No independent soundness claim.

1. **v3-R builds a disclosure differential harness** reusing v2b's `type_differential` corpus
   infrastructure: run every corpus file (and bespoke fixtures) through real `compactc` and record
   its WPP accept/reject + (where present) the leak set; run the native analyzer; assert agreement.
2. **Every rule is pinned by a fixture capturing real `compactc` behavior *before* it is
   implemented.** Rules are extracted from `analysis-passes.ss` at the pinned tag, never from docs
   or memory (docs are known to drift; training data is unreliable).
3. The corpus already contains a rich `examples/wpp/`, `examples/bugs/pm-*`, and
   `examples/**/*.compact` disclosure suite (positive and negative cases, e.g.
   `examples/bugs/pm-19907/negative/`), which seeds the fixture set.

## 6. Decomposition & roadmap

| Phase | Scope | Done-bar |
|---|---|---|
| **v3-R** | Extract the full WPP from source: exact sink "nature" strings, the per-expression abstract-interpretation rules, native + ledger-op `discloses?` flag tables, stdlib-entry collapsing, multi-leak/dedup keying. Build the `compactc`-disclosure differential harness on the corpus. **No feature/UX code.** | Harness runs green as a no-op baseline (native analyzer absent ⇒ fixtures record only `compactc` truth); every source/sink/sanitizer rule captured in a rule-tagged fixture. |
| **v3a** | **Intraprocedural** interpreter over a single circuit body: 3 sources, ledger + exported-return sinks, `disclose()` sanitizer, field/tuple/array granularity, implicit-flow via conditionals, native disclosure flags; native diagnostics + related-info path. Callees handled conservatively (§7). | Differential agreement with `compactc` on the single-circuit corpus subset; each rule fixture-pinned. |
| **v3b** | **Interprocedural**: per-circuit salsa summaries, propagation across circuit calls, recursion/fixpoint, cross-contract-call sinks. | Full ~486-file corpus differential agreement (WPP verdict parity). |
| **v3c** | **UX**: compiler-wording parity, path rendering polish, `disclose()` **quick-fix** (codeAction) with differential-verified placement, `disclosureDiagnostics` toggle, in-editor e2e in the VS Code host. | e2e green (real VS Code host); toggle + merge parity; wording matches compiler. |
| **v3-M6** | The deferred M6 heavy automation tail (regeneration scripts, repo-local upstream-update skill, headless PR automation) — now **also** covering v3's upstream-mirrored surfaces: the native/ledger **disclosure-flag tables** and any WPP rule constants, alongside v2b's type tables and the stdlib/ADT/stderr surfaces. | Per `m6-upstream-automation.md`; nothing auto-merges. |

## 7. Explicitly out of scope

- **Any soundness claim beyond "agrees with `compactc`."** The compiler's WPP is the spec;
  disagreements are our bugs. v1's dual-source diagnostic tagging already gives users the ground
  truth on save.
- **Cross-language taint into TypeScript witness implementations.** v3 analyzes Compact only; the
  witness body is opaque (it *is* a source, not a thing to analyze).
- **v3a interprocedural precision.** In v3a, calls to user circuits are handled conservatively
  (documented approximation, chosen so it never *under*-reports vs. compiler but may defer some
  cases to v3b); full summaries land in v3b. The exact conservative rule is fixed in v3a's plan
  against fixtures so it never diverges from `compactc` in the *accept* direction.
- **Runtime/ZK semantics.** v3 is purely the static WPP verdict, not proof generation.

## 8. Risks & open questions (for the phase plans to resolve, verify-first)

1. **Exact "nature" sink strings** and the full per-expression rule set are not yet transcribed —
   v3-R's first task. The model in §3 is extracted and confident; the string/rule *constants* are
   extraction targets.
2. **Abstract-value equality/merge & fixpoint termination** for recursive circuits (v3b) — the
   compiler's `merge-witnesses`/`sort by uid` invariant and path-set dedup must be mirrored so
   summaries converge; needs a source read of the merge/join logic not yet fully transcribed.
3. **Path-point fidelity vs. cost.** Rendering every path-point as related-information is faithful
   but potentially noisy; v3c decides on truncation/"first N" UX (the compiler itself advises
   fixing the first reported disclosure). Never at the cost of *verdict* parity.
4. **Quick-fix placement** — subtle; derived only from compiler-accepted fixes, differential-gated.
5. **Pinned-tag confirmation** — v3-R re-confirms the exact compiler tag/commit the analyzer
   targets and pins all extraction to it.

## 9. References

- `LFDT-Minokawa/compact` `compiler/analysis-passes.ss` — `track-witness-data` (WPP): the `Abs`,
  `witness`, `Witness-Info`, `path-point`, `Fun` datatypes; `record-leak!`/`get-leaks`/`complain`.
- `compiler/natives.ss`, `compiler/ledger.ss`, `compiler/midnight-natives.ss` — `(discloses …)`
  per-arg disclosure flags and `parse-disclosure`.
- `doc/compact-reference.mdx` §"Explicit disclosure", `doc/explicit-disclosure.mdx` — human-facing
  spec + exact error format.
- `CHANGELOG_compactc.md` PM-15585 — WPP extension to constructor/circuit args + implicit flow +
  multi-leak reporting + path tracking.
- v2 program design: `docs/superpowers/specs/2026-07-14-v2-native-type-checking-design.md` (§9
  verify-first discipline, inherited here).
- Milestone context: `.superpowers/milestones/v3-disclosure-analysis.md`,
  `.superpowers/milestones/m6-upstream-automation.md`.
