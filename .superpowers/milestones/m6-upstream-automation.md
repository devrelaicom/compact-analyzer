# M6 — Upstream automation (version watch, drift detection, agent-driven updates)

## Goal

Automate the "new Compact release" maintenance loop as far as it can safely go:
scheduled detection of new compiler/language versions, mechanical drift detection
against everything we bundle or mimic, scripts that regenerate the mechanical parts,
and a repo-local skill that lets an agent (Claude Code, headless via GitHub Action)
perform the judgment parts and open a reviewable PR. A human reviews; nothing
auto-merges.

## Why

The analyzer mirrors upstream facts in at least five places, and upstream releases
roughly monthly — every release can silently invalidate any of them. The M2a research
proved the maintenance procedure is mostly mechanical (fetch tag, extract, diff,
verify against the real toolchain); doing it by hand each month doesn't scale and
won't happen reliably. Meanwhile the docs are *known* to drift from the shipped
compiler, so "read the release notes" is not a maintenance strategy.

## The maintenance surface (what a release can break)

| Asset | Ground truth | Owning milestone |
|---|---|---|
| Bundled stdlib stub (`assets/std_*.compact`) | `compiler/standard-library.compact` + `compiler/midnight-natives.ss` at the compiler release tag | M2a |
| Ledger ADT method table | compiler source at the tag | M3 |
| Compile-on-save stderr parser + fixtures | real `compactc` output | M4 |
| Grammar / parser | compactp (upstream repo), validated by corpus differential | upstream |
| Language-version policy constants + notice text | compiler `--language-version` / release metadata | M1/M2a |
| Test corpus (~486 files) | compiler repo test suite at the tag | spec §8 |

## Includes

**Detection (scheduled CI — fully mechanical):**
- Cron workflow: check for new compiler/toolchain releases (`compact check` exists and
  is cheap — 15m TTL cache — plus the GitHub releases API for tags); on a new version,
  open/update a tracking issue with the drift report attached.
- A drift suite runnable locally and in CI (`cargo xtask drift` or `scripts/`):
  - fetch the compiler sources at the new tag; diff extracted stdlib signatures
    against the bundled stub; diff ledger ADT method surfaces against our table;
  - run the new `compactc` over known-bad fixtures and diff stderr shape against the
    M4 parser's expectations;
  - corpus differential: files the new compiler accepts but compactp rejects (or vice
    versa) → parse drift, auto-filed upstream against compactp;
  - full regression: our test suite + corpus smoke under the new toolchain.
- Scheduled differential fuzzing (compactp vs compactc accept/reject), findings fed
  upstream.
- Output: one structured drift report (machine-readable + human summary) per release.

**Mechanical update scripts:**
- Stdlib stub skeleton regeneration from the compiler sources (signatures are
  mechanical; existing doc comments are preserved where the signature is unchanged).
- Corpus refresh from the new tag; version-constant bumps.

**Agent skill (repo-local, e.g. `.claude/skills/upstream-update/`):**
- Teaches an agent to consume the drift report, run the scripts, then do the
  inference-only parts (below), verify against the gates, and open a PR whose
  description embeds the drift report.
- Headless trigger: the cron workflow, on drift, invokes Claude Code via GitHub
  Action; the PR is the output, human review is the gate.

## What's mechanical vs. what needs inference

**Mechanical (scripts, no model):** version detection; source fetching at tags;
signature extraction and diffing; corpus differential; regression/fuzz runs; stub
skeleton regeneration; version bumps; every verification gate.

**Inference (agent + skill):** doc comments for new stdlib items; rename/deprecation
mapping (old→new, e.g. `CurvePoint` → `NativePoint` happened upstream); triaging drift
as benign vs breaking; interpreting ADT method-surface changes; updating the stderr
parser when wording shifts; deciding stub-updated-in-place vs a new versioned stub
(language version bump ⇒ new stub file); summarizing the change for the PR/changelog.

## Excludes

- Auto-merging anything. Every update lands as a PR a human reviews.
- Automating grammar changes — new syntax is real engineering in compactp; the
  automation's job is to detect parse drift and file the upstream issue with fixtures.
- Multi-version support (one grammar/stub set per supported language version stands).

## Facts / constraints to honor

- **Docs are not ground truth; compiler source is.** Already-observed drift: docs show
  camelCase stdlib fields (`isSome`) while 0.31 ships snake_case (`is_some`); docs list
  `ecNeg`/`keccak256` which don't exist at 0.31; `ecMul` takes `Field`, not the
  documented scalar type. The automation must never "correct" the stub from docs.
- **Verification gates are non-negotiable and mechanical:** a regenerated stub must
  parse with 0 errors under the pinned compactp; the full test suite and corpus smoke
  must pass; the differential must agree — an agent PR that fails a gate is not opened
  (or is marked draft with the failure attached).
- The training-data-unreliability rule applies doubly to an updating agent: the skill
  must mandate verify-against-source for every claim it acts on, mirroring the M2a
  research discipline (which this milestone essentially productionizes).
- The stdlib extraction procedure is already known and proven — encode exactly how the
  M2a stub was built (which files at which tag, how bodies are stubbed) rather than
  inventing a new one.

## Dependencies / notes

- **Version-watch CI can land any time** and is already useful (0.31.1 shipped while
  0.31.0 is what we verified against). Full scope wants M2a (stub), M3 (ADT table),
  and M4 (stderr parser) to exist — natural slot after M4, alongside M5.
- **Design principle to adopt now, not at M6:** from M2a onward, when a milestone
  creates an upstream-mirroring asset, record its exact regeneration procedure next to
  the asset (script it where cheap). M6 then automates orchestration, not archaeology.
