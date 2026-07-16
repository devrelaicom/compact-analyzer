# v3 — Native Disclosure Analysis (program design)

> **Status:** Committed + decomposed (2026-07-16). **Revision 2.1** (2026-07-16, version
> correction) — see the banner immediately below. **Revision 2** (2026-07-16) incorporated a
> four-agent design review (2× compact-dev, 2× security-reviewer) and a five-agent source
> verification pass.
>
> ---
>
> > ### ⚠️ Revision 2.1 — pinned-commit correction (2026-07-16)
> >
> > Revision 2 was verified against commit **`03da643`**, which is in fact **Toolchain 0.33.107 /
> > language 0.25.102** — an unreleased dev revision ~2 minor versions AHEAD of the **0.31.1** the
> > analyzer actually targets and differential-tests against. The correct 0.31.1 source is tag
> > **`compactc-v0.31.1`** = commit **`0da5b0452eb0c1053d42418bf34b12cc29c7d63e`** (`compiler 0 31 1`
> > / `language 0 23 0`, confirmed via `compiler-version.ss`/`language-version.ss` at that ref).
> >
> > **Authoritative 0.31.1 ground truth is now the extracted rule index:**
> > `docs/superpowers/plans/2026-07-16-v3-wpp-rules-index.md` (Task R0, ~40 rules, each with a
> > `0da5b045` source anchor + verbatim nature string). Where a §3 table below carries a `03da643`
> > line number, that anchor is 0.33-derived — **trust the R0 index's line anchors, not the table's**,
> > for implementation.
> >
> > **Three substantive 0.31.1 differences from Rev-2 (decided: target 0.31.1):**
> > 1. **`emit` sink does NOT exist in 0.31.1.** Events/`emit` landed in PR #470 the day *after* the
> >    0.31.1 release. Every "emit" sink row below is a **version-gated FUTURE sink** — spec'd, but
> >    out of scope for v3 until the analyzer targets a compiler with events. Not implemented, not
> >    fixture-pinned in v3-R/v3a.
> > 2. **`contract-call` is NOT gated on `pure?`** in the 0.31.1 WPP: it records leaks
> >    unconditionally (`analysis-passes.ss:5542`); purity only marks the *calling* circuit in the
> >    earlier `identify-pure-circuits` pass — it does not gate the sink. (Low impact: v3a defers
> >    contract-call to a v3b amber advisory regardless.)
> > 3. A ledger op-arg with **no** `discloses` clause defaults to `""`, which is **truthy** in the
> >    WPP's `when discloses?` test (`ledger.ss:147`) — so ordinary Cell/Counter/Map/Set/List/
> >    MerkleTree writes leak witnesses despite carrying no annotation. (This confirms, not
> >    contradicts, §3.2's "Public ledger" row.)
>
> ---
>
> This is the program-level design for the full v3 flagship. It supersedes the lean context in
> `.superpowers/milestones/v3-disclosure-analysis.md` — treat this document as truth (as corrected
> by the Revision 2.1 banner above and the R0 index).
>
> **Decomposed** (research-first, then vertical) into: **v3-R** (extract the compiler's
> disclosure pass + build the `compactc`-differential harness) → **v3a** (intraprocedural
> disclosure) → **v3b** (interprocedural summaries) → **v3c** (diagnostics UX + `disclose()`
> quick-fix) → **v3-M6** (the deferred M6 upstream-automation heavy tail, re-sequenced here
> 2026-07-16 to also cover v3's new upstream-mirrored surface).
>
> **v3a gets a detailed implementation plan; v3b/v3c/v3-M6 are draft outlines** that must be
> re-planned just-in-time against the as-merged state of prior phases (drift is expected).

## 0. The one non-negotiable invariant: FAIL CLOSED

**Every point at which the analyzer lacks the information to decide MUST degrade to a visible
"unverified" state — never to silent green.** This is a privacy tool; a missing red squiggle
reads to a developer as "this is private," so an unknown that fails *open* actively manufactures
false confidence. This invariant is load-bearing and binds every phase's done-bar (§6).

The unknowns that MUST fail closed (each becomes an **amber "analysis-incomplete" advisory**, §4.3):
- an expression/statement form the interpreter does not yet handle (§8);
- a call to a user circuit the current phase cannot analyze through (a non-exported helper whose
  body v3a hasn't interpreted; any callee in a not-yet-implemented interprocedural path);
- a native or ledger op absent from the mirrored disclosure-flag table (§3.6), or a table lagging
  the installed compiler (§4.3 version check);
- a value whose v2b type is `Unknown`, so the `Abs` shape can't be built (§4.1);
- any sink class not yet implemented in the current phase (e.g. cross-contract calls before v3b).

"Agrees with `compactc`, no independent soundness claim" (§5) is an acceptable security posture
**only because** compile is the authoritative gate AND this invariant guarantees the live layer
never *downgrades* the developer's belief below "unverified." The old Revision-1 phrasing
"v3a's conservative-callee handling never under-reports vs. compiler" was an unproven (and, for
the internal-helper-leak case, false) assertion; it is **replaced** by this enforceable invariant.

## 1. Goal

A native, real-time, interprocedural analysis that flags **disclosure of witness-derived
(private) data as you type** — the same verdicts the Compact compiler's "Witness Protection
Program" (WPP) produces on compile, but live in the editor, with the full witness→sink path
rendered as clickable related-information locations and a quick-fix that inserts `disclose()` at
the right expression.

This is the flagship differentiator — the feature no other Compact tool has, and the reason the
program chose "staged to full native" (v1 resolution → v2 types → v3 disclosure) over
delegating semantics to the compiler.

## 2. Why

Disclosure errors are Compact's hardest and most privacy-critical error class: witness-derived
data reaching a public sink (ledger write/update, exported-circuit return, cross-contract call —
plus **event `emit`** once the analyzer targets a compiler with events, §Rev-2.1) must be
explicitly acknowledged with `disclose()`, or the compiler
rejects the program. Today a developer discovers these only at compile-on-save (M4), one save at
a time, as a textual path in an error message. Live analysis with clickable source→sink paths and
a one-click `disclose()` fix turns the language's most confusing error into its best-supported one.

Disclosure analysis depends on the two prior programs, which is why it is sequenced last:
- **v2a salsa engine** — the interprocedural memoization (§4.2) is a set of salsa queries;
  recompute on edit is the engine's job. v3 inherits this.
- **v2b types** — field/element-level taint precision requires the type of every value (a struct
  field vs. an array element are tracked differently — §3.4); the interpreter builds `Abs` shapes
  by dispatching on type. An `Unknown` type therefore fails closed (§0), not open.

## 3. Ground truth — the compiler's WPP (extracted from source; every claim source-verified)

**Source of truth:** `compiler/analysis-passes.ss`, pass `track-witness-data`, in
`LFDT-Minokawa/compact` at tag **`compactc-v0.31.1`** = commit **`0da5b045`** (compiler 0.31.1 /
language 0.23.0 — re-confirmed by v3-R/Task R0; the earlier `03da643` pin was 0.33.107, see the
Rev-2.1 banner). The extracted **R0 rule index** carries the authoritative per-rule line anchors
for this commit; the line numbers in the tables below are pre-correction 0.33 anchors. Cross-referenced with
`compiler/natives.ss`, `compiler/ledger.ss`, `compiler/midnight-natives.ss`,
`doc/compact-reference.mdx`, and `CHANGELOG_compactc.md` (PM-15585). Line numbers below are
anchors as of that commit.

The WPP runs as the `track-witness-data` nanopass over `Lwithpaths` and is an **abstract
interpreter**: the interpreter functions have signature `(ir p control-witness* disclosing-function-name?)`
and propagate *abstract values* (`Abs`, §3.4) through every circuit body, recording a **leak**
whenever a value carrying a nonempty witness set reaches a sink without having passed through
`disclose()`. Leaks are recorded as a **global side effect**, not returned per body (§3.9).

> ⚠️ v3-R re-derives each rule against a differential fixture before implementation. Revision 2's
> specifics were source-verified 2026-07-16, but treat the exact per-expression rule enumeration
> and the native/ledger flag *tables* as extraction targets to re-confirm at the pinned tag.

### 3.1 Taint sources — exactly three (`Witness-Info` datatype, ~L5017-5020)

| Source | `Witness-Info` variant | Error phrasing (`complain`) |
|---|---|---|
| A witness function's return value | `Witness-Return-Value function-name` | "the return value of witness `<f>` at `<loc>`" |
| A constructor argument | `Constructor-Argument argument-name` | "the value of parameter `<a>` of the constructor at `<loc>`" |
| An exported-circuit argument | `Circuit-Argument function-name argument-name` | "the value of parameter `<a>` of exported circuit `<f>` at `<loc>`" |

Constructor arguments and exported-circuit arguments are private too — not just witness call
results (added in PM-15585). All three must be seeded as sources (§0: omitting one leaks data).

### 3.2 Sinks — four kinds, one gate, one asymmetry (source-verified)

A leak is recorded (`record-leak!`, keyed by `(sink-src, nature-string)`) when a tainted value
reaches one of these. Note there are **two independent leaks at every sink**: one for the sink's
*data* (the value's own witnesses) and one for *implicit flow* (the ambient `control-witness*` —
§3.3), each with its own nature string.

| Sink | Gate | Data nature-string | Control nature-string | Source note |
|---|---|---|---|---|
| **Public ledger** write/update/op | per-arg `discloses?` on the ledger ADT op (§3.6) | "ledger operation" | "performing this ledger operation" | all 3 sources |
| **Event `emit`** — ⚠️ **NOT PRESENT in 0.31.1** (Rev-2.1); version-gated future sink | *(n/a in 0.31.1)* | "emit operation" | "performing this emit operation" | all 3 sources |
| **Exported-circuit return** (`(return …)`, R0 anchor `5561`/`5573`) | only when the enclosing fn is a disclosing root | "the value returned from exported circuit `<f>`" | "returning this value from exported circuit `<f>`" | **witness-returns only** (asymmetry below) |
| **Cross-contract call** (`(contract-call …)`, R0 anchor `5542`-`5550`) | **unconditional** in 0.31.1 (Rev-2.1: NOT `pure?`-gated); v3a defers this sink → amber advisory | "contract call contract reference" (the callee ref) + "contract call argument `<n>`" (each arg) | "making this contract call" | all 3 sources |

**Corrections history:** Rev-2 vs Rev-1 flagged `emit` as a missing sink and the contract
reference as a distinct sink separate from each argument. **Rev-2.1 (0.31.1 target)** then
established: `emit` does not exist in 0.31.1 (version-gated future sink, out of scope for v3);
and cross-contract calls are **unconditional**, NOT `pure?`-gated (the Rev-2 "unless pure?" claim
was 0.33-era / incorrect for the WPP sink — purity is a separate pass). See the Rev-2.1 banner.

**Source-dependent return asymmetry (verified faithful).** At the exported-circuit-return sink,
`filter-witnesses` keeps only `Witness-Return-Value` witnesses and drops `Constructor-Argument`
and `Circuit-Argument` (source comment: "don't report exposure of an exported circuit's own
arguments via the circuit's return value"). So a circuit returning its *own* argument is not a
disclosure; returning a *witness* value is. This filter must be applied per-witness by inspecting
each witness's `Witness-Info` variant — a coarse or buggy filter here silently *suppresses* a real
`Witness-Return-Value` leak (§0 / §8 dedicated fixture: witness-return and circuit-arg co-mingled
in a returned struct).

### 3.3 Implicit flow — a SEPARATE control-witness leak (Rev-2 rewrite)

Conditioning a sink on a witness value discloses that witness — but the mechanism is **not** what
Rev-1 described. Verified behavior:

- The interpreter threads a **`control-witness*` set** as an explicit parameter. At an `if`, the
  condition's witnesses are merged into `control-witness*` and passed down to the branch(es):
  `control-witness* := merge(abs->witnesses(condition), control-witness*)`.
- At **every** sink, in addition to the data leak, the interpreter records a **separate**
  `record-leak!` keyed on the current `control-witness*` with its own "performing/making/returning
  this X" nature string (§3.2). So conditioning a ledger write on a witness taints **neither the
  written value nor the ledger field** — it emits a distinct control-flow leak on the *gating*
  witness. An implementation that instead taints the written value diverges from the compiler.
- **`Abs-boolean` is NOT the implicit-flow carrier** (Rev-1 got this wrong). Comparisons
  (`<`,`<=`,`==`,…) produce `Abs-atomic` via `handle-comparison`, not `Abs-boolean`. `Abs-boolean`
  is constructed only from compile-time-constant `#t`/`#f`, and `combine-abs` decays it to
  `Abs-atomic` the moment it joins with anything non-constant. Its sole purpose is
  **precision/termination**: a statically-constant condition takes only the live branch
  (dead-branch elimination); a **non-constant** condition interprets **both** branches and joins
  their results with `combine-abs` (witness-set **union**). Treating `Abs-boolean` as "the"
  implicit-flow mechanism would diverge on every non-constant condition — i.e. the common case.

The control-flow merge/join is always a **union** (`combine-abs` → `merge-witnesses`); a join that
dropped or intersected witnesses would lose taint on one path (§0).

### 3.4 Granularity — the abstract-value lattice (`Abs` datatype, ~L4985-4989)

*"struct and tuple fields are tracked individually; array elements are tracked in the aggregate."*

| `Abs` variant | Represents |
|---|---|
| `Abs-atomic witness*` | a scalar carrying a set of witness values |
| `Abs-boolean true? witness*` | a **compile-time-constant** boolean (precision/dead-branch only — §3.3) |
| `Abs-multiple abs*` | a **struct or tuple** — each field/element tracked **individually** |
| `Abs-single abs` | an **array/vector** — all elements tracked **in aggregate** (one `abs`) |

Not a design choice: matching `compactc` requires this exact shape. The interpreter builds it via
`default-value type`, which is why v3 needs v2b types (and why `Unknown` fails closed — §0). Enums
(`tenum`) carry no per-variant payload and fall through to `Abs-atomic`; `Maybe`/`Either` are plain
structs (`Abs-multiple`) — both already subsumed, no separate rule needed.

### 3.5 The sanitizer — `disclose()` only

`disclose(e)` maps `e`'s abstract value through the `disclose` helper, clearing `witness*` to `'()`
at every level (`Abs-atomic/boolean/multiple/single`). No runtime effect; a later `remove-disclose`
pass strips it. **Hashing does not sanitize** (verified): `transientHash` declares its argument
`(discloses "a hash of")`, which makes it a *conduit* (§3.6) — the witness set survives into the
result tagged "a hash of" and still needs `disclose()` at the eventual sink.

### 3.6 Native vs. ledger disclosure flags — TWO mechanisms (Rev-2 correction)

Both use a per-argument flag whose value is **`#f | exposure-string`** (`parse-disclosure`,
`natives.ss:59-62`: `(discloses nothing)`→`#f`, `(discloses "…")`→the string), used *dually* as the
truthiness gate and the path-point exposure text. But they do opposite things:

- **Ledger ADT op (`discloses?`) → immediate sink** (`analysis-passes.ss` ~L5814-5836): when the
  flag is set on an argument position, a tainted arg there fires `record-leak!` *at the op* ("ledger
  operation"), and the op's return value is reseeded empty (`default-value type`). A sink.
- **Native/stdlib (`disclosure?*`, `Fun-native`) → return-value conduit** (~L5387-5399): when the
  flag is set, the argument's witnesses are folded **into the native's return-value taint** (tagged
  with the exposure string) and **no leak is recorded**. A conduit, not a sink.

Rev-1 conflated these under "flag ⇒ sink," which would make `transientHash(witness)` an incorrect
immediate leak instead of correctly-tainted output. These flag tables are the upstream-mirrored
surface owned by v3-M6; an unmirrored/lagging entry MUST fail closed (§0) — default an unknown
native to "conduit that taints its result" and an unknown ledger op to "sink," never to
"discloses nothing."

Verified-clean passthroughs (pin as fixtures): `default<T>` seeds empty witnesses; all cast forms
and `serialize`/`deserialize` pass the abstract value through unchanged.

### 3.7 Path tracking — the witness→sink trail

Each witness carries `path*` (sorted paths of `path-point`s: `src` + `description` + `exposure`).
Points are recorded at calls passing the value as an argument, `const` bindings deriving from it,
conditionals, and comparisons. Stdlib-internal points collapse to the **stdlib entry** site (the
disclosure is reported where user code enters the stdlib). **Collapsing is display-only** — it must
never collapse the *leak* itself (§0 keeps "shorten the displayed trail" and "record the leak"
strictly separate).

### 3.8 Diagnostic shape (`complain` + `doc/compact-reference.mdx`)

WPP reports **all** leaks (not first-only), sorted by sink position. Each renders as:

```
potential witness-value disclosure must be declared but is not:
    witness value potentially disclosed:
      <source phrasing from §3.1>
    nature of the disclosure:
      <nature-string from §3.2> might disclose <exposure> the witness value
    via this path through the program:
      <path-point description> at <loc>
      ...
```

v3 renders this as a native diagnostic **at the sink** plus one `related_information` entry per
source and per non-stdlib path-point (§4.3). Wording tracks the compiler's nature strings for
parity.

### 3.9 Function kinds & the interprocedural model (`Fun` datatype; Rev-2 correction)

- `Fun-circuit` — interpreted via its body. **Only exported circuits and the constructor are
  analysis roots** (`Program-Element`, ~L5542-5561; the circuit clause is gated by
  `(when (id-exported? …))`). A non-exported helper is analyzed **only** when reached through a
  call from a root, with the abstract argument shapes that reach it at that call site.
- `Fun-witness` — abstract return = the type's `default-value` seeded with a fresh
  `Witness-Return-Value` record at the leaves.
- `Fun-native` — carries the per-arg `disclosure?*` conduit flags (§3.6).

**Calls are re-interpreted per call site, memoized on the full context** (`call-ht`, key =
`(circuit-uid, abs*, control-witness*)` via `key-equal?` comparing all three; ~L5305-5331). On a
miss, `handle-call` re-interprets the callee body under `(extend-env … var-name* abs*)`. There is
an **additional post-lookup gate**: a *disclosing* context (`return-value-discloses?` = true, set
only for exported-circuit roots) never reuses a cached result — it reprocesses to (re-)report
return-value leaks (~L5350-5357). **Recursion is forbidden** (`reject-recursive-circuits` runs
before the WPP; the WPP `assert`s cannot-happen on an in-process call), so there is no cross-circuit
fixpoint. The only fixpoint is **intraprocedural**: `fold` over a homogeneous aggregate array
iterates to an `abs-equal?` fixed point bounded by `len` (~L5739-5751); the heterogeneous
`Abs-multiple` case fully unrolls.

**Consequence for the analyzer (settles the Rev-1 §4.2 design):** a coarse "per-circuit summary
keyed on circuit identity alone" is **unsound** — leaks depend on (a) the abstract argument shapes
reaching each parameter and (b) the disclosing-vs-ordinary call context. The memoization key must
carry both.

## 4. Architecture in the analyzer

### 4.1 Placement & dependencies

A new module in **`analyzer-core`** (`disclosure/`) implementing the abstract interpreter over the
existing HIR/item-tree, consuming **v2b types** (to build `Abs` shapes and drive granularity), the
**item tree** (to classify callees as circuit/witness/native, identify roots, ledger fields,
`emit`, and contract-call sites), and the **v2a salsa engine** (incremental recompute). v3's
soundness is bounded by v2b's; a `type_at`→`Unknown` fails closed (§0). Emits
`analyzer_core::Diagnostic`s with `secondary_spans` for the path.

### 4.2 Salsa query shape — per-call-site memoization + root-drained global leaks (Rev-2 redesign)

The Rev-1 "one reusable transfer function per circuit, invalidates exactly its transitive callers"
model is **wrong** (§3.9). Replaced by:

- **`circuit_abstract(circuit, arg_shapes, control_witnesses)`** — the analyzer's analogue of the
  compiler's `call-ht` cell: given the abstract argument shapes and ambient control witnesses at a
  call site, returns the circuit's abstract return value, memoized on that full key. Distinct call
  sites with distinct taint shapes are distinct salsa keys — so incremental reuse is *shape-scoped*,
  not the clean "leaf edit invalidates callers" story; editing a circuit invalidates every keyed
  instance of it (acceptable; correctness first).
- **Leaks are collected at the root, not returned per body.** `disclosure_diagnostics(file)` drives
  each root (exported circuit, constructor), and leaks accumulate into a per-analysis table keyed by
  `(sink-src, nature-string)` — mirroring the compiler's global `record-leak!`/`get-leaks` dedup —
  drained once after the whole root is walked. The disclosing-context reprocessing gate (§3.9) is
  modeled explicitly so return-value leaks are reported iff the circuit is reached as a disclosing
  root.
- **Open question deferred to v3b** (§8): whether a coarser, differential-equivalent summary exists
  is unproven and *not assumed*; v3b either replicates per-call-site memoization or proves
  equivalence against the corpus.

### 4.3 LSP surface

- **Two diagnostic channels (Rev-2):**
  - **Confirmed leak** → `Severity::Error` → `DiagnosticSeverity::ERROR` (**red**), `E…` code,
    `source = "compact-analyzer"`.
  - **Analysis-incomplete / unverified advisory** (the §0 fail-closed output) →
    `DiagnosticSeverity::WARNING` (**amber**). LSP binds color to severity (only 4 colors: red /
    amber / blue / faint), so the amber advisory is disambiguated from an ordinary warning in the
    **non-color channels**: a distinct `source` (e.g. `"compact-analyzer (unverified)"`), a distinct
    `code` family (e.g. `U…`) with a `codeDescription` "why this is unverified" link, and advisory
    wording. No LSP protocol change is needed — `analyzer_core::Severity` already maps
    `Warning → WARNING` (`lsp_utils.rs`); given the pre-release/no-back-compat stance, adding an
    explicit `Advisory`/`Unverified` severity variant that maps to `WARNING` is the clean option.
- **Advisory-UX contract (Rev-2):** the editor's *absence of red is not a proof of privacy*. The
  diagnostics UX (hover text on advisories, and docs) must state "the analyzer is advisory; a clean
  editor is not a guarantee — **compile is the authoritative privacy gate**." This matters most in
  the toggle-off and recompute-lag windows.
- **Transient-parse retention (Rev-2):** during unparseable mid-edit states, disclosure diagnostics
  must NOT silently blank to green (a blank reads as "safe"). Define an explicit retention rule
  (e.g. keep last-known diagnostics as stale/greyed, or show an "analysis paused" advisory) rather
  than inheriting the type-checker's blanking behavior.
- **Wording parity (v3c):** nature strings track §3.2/§3.8.
- **Quick-fix (v3c):** a `textDocument/codeAction` that wraps the **minimal** tainted expression at
  the exact sink in `disclose()` — never a convenient enclosing expression (a broad wrap clears
  taint for MORE than the flagged sink and can silently sanitize a *second* real leak). Placement is
  differential-verified against compiler-accepted fixes. Framed as an intentional privacy decision
  ("reveal this value"), **not** offered via fix-all / fix-on-save.
- **Toggle (v3c):** a `disclosureDiagnostics` on/off setting mirroring v2b.final's `typeDiagnostics`.
  Native and compiler diagnostics stay distinctly tagged; the merge (`merge_diagnostics`) must be
  one-directional — a native *absence* at a sink can never suppress `compactc`'s on-save diagnostic
  there (v3c test).
- **Version check (Rev-2):** compare the analyzer's pinned extraction tag against the *installed*
  `compact` version; on mismatch, surface an advisory that the disclosure tables may be stale (a
  drifted native/ledger flag table is a §0 unknown).

## 5. Verify-first & differential strategy (non-negotiable)

Identical discipline to v2b: **the compiler is the spec; disagreement is a bug on our side.** No
independent soundness claim — acceptable **only** under §0 (fail-closed) + the §4.3 advisory
contract (compile is authoritative).

1. **v3-R builds a disclosure differential harness** reusing v2b's `type_differential` corpus
   infrastructure: run every corpus file + bespoke fixtures through real `compactc`, record its WPP
   accept/reject (and, where available, the leak set); run the native analyzer; assert agreement.
2. **Every rule is fixture-pinned from real `compactc` behavior before implementation.** The
   differential only catches expression/sink forms present in the corpus — unknown forms fail closed
   (§0), never green.
3. The corpus already ships a rich disclosure suite (`examples/wpp/`, `examples/bugs/pm-*`,
   `examples/**` positive+negative) to seed fixtures.

## 6. Decomposition & roadmap (done-bars bound to §0)

| Phase | Scope | Done-bar (all include: every unanalyzable point emits an amber advisory, never silent green) |
|---|---|---|
| **v3-R** | Extract the full WPP: exact per-expression rules, the native + ledger disclosure-flag **tables**, control-witness leak wording, stdlib-entry collapse, leak dedup keying. Build the `compactc`-disclosure differential harness on the corpus. **No feature/UX code.** | Harness runs green as a baseline (native absent ⇒ fixtures record only `compactc` truth); every source/sink/sanitizer/flag rule captured in a rule-tagged fixture. |
| **v3a** | **Intraprocedural** interpreter over a single circuit body: 3 sources; **ledger + exported-return sinks** (`emit` version-gated / absent in 0.31.1 per Rev-2.1; cross-contract deferred → amber advisory in v3a); `disclose()` sanitizer; §3.4 granularity; implicit-flow control-witness leaks (§3.3); native conduit vs ledger sink flags (§3.6); **intraprocedural `fold`/`abs-equal?` fixpoint** (Rev-2: this is v3a, not v3b); diagnostics + related-info path. Any call to a non-exported helper it cannot interpret through, and any unhandled form → amber advisory. | Differential agreement with `compactc` on the single-circuit corpus subset; each rule fixture-pinned; fail-closed verified (deferred sinks / unknown callees / `Unknown` types all surface amber, never green). |
| **v3b** | **Interprocedural**: the per-call-site memoized `circuit_abstract` query (§4.2), cross-circuit propagation, the disclosing-context gate, cross-contract-call sinks. Resolve the §8 summary-shape open question (replicate per-call-site memoization or prove a coarser equivalent). | Full ~486-file corpus differential agreement (WPP verdict parity). |
| **v3c** | **UX**: compiler-wording parity; path rendering + stdlib-collapse (display-only); `disclose()` **minimal-scope quick-fix**; `disclosureDiagnostics` toggle + one-directional merge; advisory-UX contract + transient-parse retention + version check; in-editor e2e. | e2e green (real VS Code host); toggle/merge parity; quick-fix places minimally + differential-verified; advisory wording present. |
| **v3-M6** | The deferred M6 heavy automation tail — now **also** mirroring v3's disclosure-flag tables + WPP rule constants (with a version-compat check feeding §4.3), alongside v2b's type tables and the stdlib/ADT/stderr surfaces. | Per `m6-upstream-automation.md`; nothing auto-merges. |

## 7. Explicitly out of scope

- **Any soundness claim beyond "agrees with `compactc`"** — the compiler's WPP is the spec.
  Acceptable only under §0 + the §4.3 advisory contract.
- **Cross-language taint into TypeScript witness implementations** — v3 analyzes Compact only; a
  witness body is opaque (it *is* a source).
- **Runtime/ZK semantics** — v3 is purely the static WPP verdict.
- (Rev-1's "v3a never under-reports vs compiler" is **removed** — replaced by §0 fail-closed;
  under-analysis is *disclosed as amber*, not silently accepted as green.)

## 8. Risks & open questions (for the phase plans, verify-first)

1. **Exact per-expression rule enumeration** — v3-R transcribes every WPP Expression/Effect clause
   from source into fixtures; any operator missed from the implicit-flow/conduit set must fail
   closed (§0), not default to untainted.
2. **§4.2 summary shape (v3b)** — whether a coarser, differential-equivalent summary exists is
   *unproven and not assumed*; the per-call-site memoization is the sound baseline.
3. **`combine-abs` join & `abs-equal?` fixpoint fidelity** — the union-join and the fold
   termination loop must be mirrored exactly (v3a for fold; both are monotone over a finite witness
   universe ⇒ terminate). Source read of `combine-abs`/`merge-witnesses` invariants required.
4. **Return-asymmetry per-witness filtering** — the one place suppression is *intended* (§3.2); a
   coarse filter silently hides a real `Witness-Return-Value` leak. Dedicated co-mingled-source
   fixture.
5. **Path-point fidelity vs. noise (v3c)** — array/vector aggregate granularity can render a path
   pointing at `a[i]` for a leak on `a[j]` (display fidelity, not a verdict miss); truncation UX
   must never reduce *verdict* parity.
6. **Salsa staleness / async-LSP window** — the recompute-lag amber/stale state must be honestly
   surfaced (§4.3); confirm cross-file invalidation covers source-**identity** changes (a function
   flipping witness↔circuit, an added exported-circuit/constructor parameter re-seeds sources).
7. **Pinned-tag confirmation** — ✅ **DONE by Task R0 (Rev-2.1):** the Rev-2 pin `03da643` was
   0.33.107; corrected to tag `compactc-v0.31.1` = `0da5b045`. All extraction + the §4.3 version
   check pin to it. This risk materialized — the confirmation caught a real mismatch.

## 9. References

- `LFDT-Minokawa/compact` `compiler/analysis-passes.ss` — `track-witness-data` (WPP): `Abs`,
  `witness`, `Witness-Info`, `path-point`, `Fun` datatypes; `record-leak!`/`get-leaks`/`complain`;
  `handle-call`/`call-ht`; `handle-comparison`; the `if`/`emit`/`contract-call`/`return`/`fold`
  clauses; `combine-abs`/`merge-witnesses`; `reject-recursive-circuits` (pipeline).
- `compiler/natives.ss` (`parse-disclosure`), `compiler/ledger.ss`, `compiler/midnight-natives.ss`
  (`transientHash … (discloses "a hash of")`) — the disclosure-flag tables.
- `doc/compact-reference.mdx` §"Explicit disclosure"; `CHANGELOG_compactc.md` PM-15585.
- v2 program design: `docs/superpowers/specs/2026-07-14-v2-native-type-checking-design.md`
  (verify-first discipline, inherited).
- Milestones: `.superpowers/milestones/v3-disclosure-analysis.md`, `…/m6-upstream-automation.md`.

## 10. Review & verification provenance (2026-07-16)

Revision 2 folds in a four-agent design review — 2× `compact-core:compact-dev` (Sonnet + Opus),
2× `compact-core:security-reviewer` (Sonnet + Opus) — and a five-agent verification pass.

- **compact-dev review** flagged: missing `emit` sink; pure cross-contract-call exemption;
  `Abs-boolean` mis-characterization; `fold` fixpoint belongs in v3a; §4.2 per-circuit-summary
  infeasibility; native-vs-ledger flag conflation. Confirmed clean: 3 sources, granularity,
  disclose()-only, return asymmetry, `Maybe`/`Either`/enum subsumption, `default<T>`/casts.
- **security review** flagged: no global fail-closed principle (headline); v3a shipping live with a
  missing sink class + deferred callees; "never under-reports" overclaim; native-flag table drift;
  no user-facing "advisory; compile authoritative" contract; quick-fix over-disclosure; transient
  parse-state blanking; real-time framing eroding compile-reliance.
- **Source verification** (4 agents via `/midnight-verify:verify-by-source`, against `03da643`):
  **all claims CONFIRMED, zero refutations**, with the nuance that the disclosing-context axis is a
  post-lookup reprocessing gate (not a hash-key member). ⚠️ **Rev-2.1 caveat:** this pass verified
  against `03da643` = **0.33.107**, not the 0.31.1 target — so it validated the *model shape* but
  not the *version*. Task R0 re-extracted against the true 0.31.1 (`0da5b045`); its rule index
  supersedes this pass for per-rule anchors, and surfaced the three 0.31.1 deltas (no `emit`;
  contract-call unconditional; ledger empty-`discloses`⇒truthy). The model structure held.
- **VS Code research** (1 agent): amber = `DiagnosticSeverity::WARNING`, already wired; color is
  bound to severity (4 colors max), so error-vs-unverified is red-vs-amber + `source`/`code` text.

All of the above are incorporated: §0 (fail-closed), §3.2 (sinks + contract-ref; emit version-gated
& contract-call unconditional per Rev-2.1),
§3.3 (implicit-flow rewrite), §3.6 (two flag mechanisms), §3.9/§4.2 (memoization redesign),
§4.3 (amber advisory + advisory contract + retention + minimal quick-fix + version check).
