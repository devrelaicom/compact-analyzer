# v3-M6 — Upstream-Automation Tail (heavy half of M6) — DRAFT / OUTLINE (not execution-ready)

> ⚠️ **DRAFT OUTLINE — DO NOT EXECUTE AS-IS.** Scope sketch, not a task-by-task plan.
> **Before generating v3-M6's detailed implementation plan, the executing agent MUST:**
> 1. Re-read `.superpowers/milestones/m6-upstream-automation.md` (the full M6 context) and spec
>    Revision 2 §3.6, §4.3 (version check), §6.
> 2. Review the **as-merged** state of ALL prior phases (v2b type tables; v3-R/v3a/v3b/v3c
>    disclosure-flag tables + WPP rule constants + the version check) for drift — the set of
>    upstream-mirrored surfaces this must automate may have grown or changed shape. **Confirm the
>    actual mirrored assets in merged code; do not trust this outline's list.**
> 3. Re-read the existing `M6-detect` slice IF it has landed by then (it is early/independent and
>    may already exist — check `.github/workflows/` and `xtask`/`scripts/`); v3-M6 builds on it.
> 4. Run `superpowers:writing-plans`.

> **Re-sequencing note (2026-07-16):** the heavy half of M6 was re-deferred from the end of v2 to
> the end of v3 (`.superpowers/milestones/README.md`). It now also covers v3's new
> upstream-mirrored surface — the disclosure-flag tables and WPP rule constants — alongside v2b's
> type tables and the original stdlib/ADT/stderr surfaces.

## Goal

Automate the "new Compact release" maintenance loop as far as it can safely go, now including v3's
disclosure surfaces: scheduled detection, mechanical drift detection + regeneration scripts, and a
repo-local agent skill that opens a reviewable PR. A human reviews; nothing auto-merges.

## The maintenance surface this must cover (confirm the full list at planning time)

- Bundled stdlib stub (`assets/std_*.compact`) — M2a asset.
- Ledger ADT method table — M3 / v2b.6.
- Compile-on-save stderr parser + fixtures — M4.
- Language-version policy constants + notice text — M1/M2a.
- Test corpus (~486 files) — spec §8.
- **v2b type-rule tables.**
- **v3 disclosure-flag tables (native `disclose?*` + ledger `discloses?`) and WPP rule constants**
  (spec §3.6) — NEW; these directly feed the analyzer's leak verdicts, so drift is a
  correctness/privacy risk, and the analyzer-tag-vs-installed-compiler **version check**
  (spec §4.3) is its runtime guard.

## Scope (outline — from `m6-upstream-automation.md`, extended for v3)

- **Detection (scheduled CI):** version-watch cron (`compact check` + GitHub releases API); on a
  new version, open/update a tracking issue with a drift report. (If `M6-detect` already shipped,
  extend it; else build it here.)
- **Drift suite (`cargo xtask drift` or `scripts/`):** fetch compiler sources at the new tag; diff
  extracted stdlib signatures, ledger ADT surfaces, **type-rule tables, and disclosure-flag
  tables** against the analyzer's copies; run new `compactc` over known-bad fixtures and diff
  stderr shape; corpus differential (type AND disclosure) under the new toolchain.
- **Regeneration scripts:** stdlib stub skeleton; corpus refresh; version-constant bumps; **the
  disclosure-flag tables** regenerated from `natives.ss`/`ledger.ss`/`analysis-passes.ss` at the
  new tag (mechanical extraction, mirroring R0's manual extraction).
- **Version-compat wiring:** connect the regenerated pinned-tag constant to the runtime version
  check (spec §4.3) so a mismatch surfaces the "tables may be stale" advisory.
- **Agent skill (`.claude/skills/upstream-update/`):** consume the drift report, run the scripts,
  do the inference-only parts (doc comments, rename/deprecation mapping, triaging benign vs
  breaking, updating the stderr parser + WPP rule constants when wording shifts), verify against
  the gates, open a PR embedding the drift report.
- **Headless trigger:** cron → Claude Code via GitHub Action → reviewable PR. Human review is the
  gate; nothing auto-merges.

## What's mechanical vs. inference (from m6-upstream-automation.md)

- **Mechanical:** version detection, source fetch at tags, signature/flag-table extraction +
  diffing, corpus differential (type + disclosure), regression/fuzz, stub regen, version bumps,
  every verification gate.
- **Inference (agent + skill):** doc comments; rename/deprecation mapping; benign-vs-breaking
  triage; interpreting WPP rule / disclosure-flag changes; updating the stderr parser + nature
  strings when compiler wording shifts; stub-in-place vs new versioned stub; PR/changelog summary.

## Done-bar (draft)

- A new-release event produces a drift report + regenerated tables (type + disclosure) + a
  reviewable PR, all gates green (differential type + disclosure parity, corpus smoke, regression),
  nothing auto-merged.
- The disclosure version check is wired to the regenerated pinned-tag constant.

## Non-goals (from m6-upstream-automation.md)

- Auto-merging anything. Automating grammar changes (detect parse drift + file upstream instead).
  Multi-version support (one grammar/table set per supported language version).
