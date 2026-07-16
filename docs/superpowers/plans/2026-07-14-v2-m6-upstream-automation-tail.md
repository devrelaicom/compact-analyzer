# v2-M6 — Upstream Automation Tail — DRAFT / OUTLINE (not execution-ready)

> ⚠️ **DRAFT OUTLINE — DO NOT EXECUTE AS-IS.** Forward-context sketch captured alongside
> the v2 program spec. **Not** a bite-sized, execution-ready plan.
>
> **This outline is highly likely to change** based on implementation decisions made while
> building the prior sub-milestones — especially **v2b**, which creates the new
> upstream-mirroring surface this milestone must automate: the **type-rule tables**. The
> exact shape of those tables (how types/ADT surfaces are represented and extracted from
> compiler source) is unknown until v2b lands, and it determines the regeneration scripts
> and drift gates here. This milestone also depends on whether **M6-detect** (the version-
> watch + drift-report + differential-fuzz half) has already landed independently.
>
> **Before turning this into an actual implementation plan, the executing agent MUST:**
> 1. Re-read the v2 program spec §7 `docs/superpowers/specs/2026-07-14-v2-native-type-checking-design.md`
>    and the M6 context `.superpowers/milestones/m6-upstream-automation.md`.
> 2. Review the **as-merged** implementation of v2a/v2b/v2c and the current project state —
>    in particular the type-rule tables and `ledger_adts.rs` as they actually landed, and
>    whether/how **M6-detect** shipped — checking for **drift** between this outline's
>    assumptions and reality.
> 3. Run `superpowers:writing-plans` (and `superpowers:brainstorming` first if drift is
>    material) to produce the real plan.

**Goal (from spec §7 + M6 context):** The **heavy half** of M6 — regeneration scripts, a
repo-local agent-update skill, and headless PR automation — now covering v2b's type-rule
tables **in addition to** the existing stdlib-stub / ledger-ADT / stderr-parser mirror
surfaces. Every update lands as a human-reviewed PR; nothing auto-merges.

**Architecture (provisional):** Mechanical scripts (`cargo xtask` or `scripts/`) fetch
compiler sources at a tag, extract + diff signatures/tables (including type rules),
regenerate the mechanical parts, and run the non-negotiable verification gates. A repo-local
skill (`.claude/skills/upstream-update/`) teaches an agent to consume the drift report,
run the scripts, do the inference-only parts, verify against the gates, and open a PR whose
body embeds the drift report. A cron GitHub Action invokes it headlessly on detected drift.

## Global Constraints (inherited — reconfirm at plan time)

- **Docs are never ground truth; compiler source is** (M6 records concrete observed drift:
  camelCase-vs-snake_case stdlib fields, non-existent `ecNeg`/`keccak256`, `ecMul` arg type).
  The automation must **never** "correct" an asset from docs.
- **Verification gates are non-negotiable and mechanical:** regenerated assets must parse
  with 0 errors under the pinned compactp; full test suite + corpus smoke pass; the
  differential agrees. A PR that fails a gate is not opened (or is marked draft with the
  failure attached).
- **Nothing auto-merges.** Human review is the gate.
- The training-data-unreliability rule applies **doubly** to an updating agent — the skill
  must mandate verify-against-source for every claim it acts on.
- Encode the *already-proven* extraction procedures (stdlib stub build, ADT table) rather
  than inventing new ones.

## Dependency: M6-detect (should already exist)

Per spec §7 the **detection half** — version-watch cron, drift report (stdlib/ADT/stderr),
scheduled differential fuzzing — lands **early and independently**, outside the v2 chain.
The real plan must first confirm M6-detect's state:
- **If landed:** v2-M6 consumes its drift-report format and extends it with a **type-rule
  drift** section; do not rebuild detection.
- **If not landed:** either pull the minimal detection needed as an explicit prerequisite,
  or coordinate — do **not** silently re-scope detection into v2-M6.

## Phase outline (coarse — each becomes several TDD/scripted tasks in the real plan)

1. **Extend the drift surface to type tables.** Teach the drift diff about v2b's type-rule
   tables + typed ADT surfaces (the new mirror surface), alongside the existing
   stdlib/ADT/stderr diffs.
2. **Regeneration scripts.** Stub/table skeleton regeneration from compiler sources at a
   tag (signatures mechanical; existing doc comments preserved where unchanged); version-
   constant bumps; corpus refresh. Now includes type-rule table regeneration.
3. **Verification gates as code.** Parse-clean-under-pinned-compactp, full-suite, corpus
   smoke, differential-agreement — each a scripted, blocking check.
4. **Agent-update skill** (`.claude/skills/upstream-update/`). Consumes the drift report,
   runs the scripts, performs the inference-only parts (doc comments for new items,
   rename/deprecation mapping, benign-vs-breaking triage, stderr-wording updates, stub-in-
   place-vs-new-versioned-stub decisions, PR/changelog summary), verifies gates, opens the PR.
5. **Headless trigger.** Cron GitHub Action → on drift, invoke Claude Code with the skill;
   the PR is the output; human review is the gate.

## Key open questions to resolve at plan time

- **Type-rule table representation:** entirely determined by v2b — the single biggest
  unknown. The regeneration/extraction scripts can't be designed until v2b's tables exist.
- **M6-detect boundary:** exact drift-report schema to extend vs re-emit (see Dependency above).
- **`xtask` vs `scripts/`:** which harness the repo standardizes on for the drift/regen suite.
- **Inference-vs-mechanical line for type tables:** which parts of type-table updates are
  pure regeneration vs need agent judgement (e.g., a new bounded-integer rule's semantics).
- **Grammar/syntax changes stay upstream** (spec §12): automation only detects parse drift
  and files upstream compactp issues with fixtures — confirm this boundary holds for any
  type-affecting syntax change too.
