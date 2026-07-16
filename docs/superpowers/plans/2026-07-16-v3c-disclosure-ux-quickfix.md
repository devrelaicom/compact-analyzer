# v3c — Disclosure UX + `disclose()` Quick-Fix — DRAFT / OUTLINE (not execution-ready)

> ⚠️ **DRAFT OUTLINE — DO NOT EXECUTE AS-IS.** This is a scope sketch, not a task-by-task plan.
> **Before generating v3c's detailed implementation plan, the executing agent MUST:**
> 1. Re-read spec Revision 2 §3.8 (diagnostic shape), §4.3 (LSP surface), §8 risk #5.
> 2. Review the **as-merged** state of v3-R + v3a + v3b for drift — the diagnostic wording, the
>    related-info path rendering, the advisory channel, and the leak table may have changed. **Do
>    not trust the interfaces named here without confirming them in merged code.**
> 3. Derive `disclose()` placement rules from **compiler-accepted fixes** (differential-verified),
>    never from intuition (spec §4.3).
> 4. Run `superpowers:writing-plans`.

## Goal

Polish the disclosure diagnostics into a best-in-class editor experience: compiler-wording parity,
a faithful witness→sink related-information trail, a **minimal-scope `disclose()` quick-fix**, the
`disclosureDiagnostics` toggle, the advisory-UX contract, transient-parse retention, and a version
check — all verified in a real VS Code host.

## Scope (outline — re-derive details just-in-time)

- **Wording parity** — align each diagnostic's nature string + source phrasing + path-point
  descriptions with the compiler's exact text (spec §3.8), so native (live) and `compactc`
  (on-save) diagnostics read as one vocabulary.
- **Related-information path** — render one `related_information` entry per source origin and per
  non-stdlib path point; stdlib-internal points collapse to the stdlib entry site (display-only —
  never collapse the leak; spec §3.7). Decide truncation UX for long paths ("fix the first
  reported disclosure") WITHOUT ever reducing verdict parity (spec §8 risk #5).
- **`disclose()` quick-fix** — a `textDocument/codeAction` (net-new LSP surface) that wraps the
  **minimal** tainted expression at the exact sink in `disclose()`. NEVER a convenient enclosing
  expression (a broad wrap clears taint for more than the flagged sink and can silently sanitize a
  second real leak — spec §4.3, §8). Placement differential-verified against compiler-accepted
  fixes. Framed as an intentional privacy decision ("Reveal this value"), NOT offered via
  fix-all / fix-on-save.
- **Advisory-UX contract** — hover text on amber advisories + docs state "advisory; a clean editor
  is not a proof of privacy — compile is the authoritative gate" (spec §4.3). Ensure the amber
  `U`-family diagnostics carry the `codeDescription` "why unverified" link.
- **`disclosureDiagnostics` toggle** — on/off setting mirroring `v2b.final`'s `typeDiagnostics`;
  the `merge_diagnostics` path must be one-directional (a native absence at a sink can never
  suppress a `compactc`-on-save diagnostic there — add the explicit test).
- **Transient-parse retention** — during unparseable mid-edit states, do NOT blank disclosure
  diagnostics to green; keep last-known (stale/greyed) or show an "analysis paused" advisory
  (spec §4.3).
- **Version check** — compare the analyzer's pinned extraction tag against the installed `compact`
  version; on mismatch surface an advisory that the disclosure tables may be stale.
- **In-editor e2e** — extend the VS Code test-host e2e (as v2c did) to exercise: a red leak +
  related-info navigation, the quick-fix, an amber advisory, and the toggle.

## Done-bar (draft)

- e2e green in a real VS Code host (xvfb): leak diagnostic + related-info + quick-fix + advisory +
  toggle all exercised.
- Quick-fix places the minimal expression and its result is differential-verified (compactc
  accepts the fixed source; no second leak is masked).
- One-directional merge test passes (native absence never hides a compactc diagnostic).
- Advisory hover copy + version-check advisory present.

## Risks

- Quick-fix placement fidelity (the compiler dictates which expression is the disclosure point);
  differential-gate every placement rule.
- Path-point noise vs. fidelity on aggregate (array) granularity — a leak on `a[j]` may render a
  path at `a[i]`; a display concern, never a verdict concern.
