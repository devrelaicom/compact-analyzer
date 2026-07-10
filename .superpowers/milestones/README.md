# Milestones

Context files for expected future milestones. Each file records the goal, scope
boundaries, and hard-won verified facts for a milestone **before** it has a spec-level
plan — enough to reconstruct intent and expectations, deliberately less detailed than
`docs/superpowers/plans/*` because earlier milestones may force changes and compromises.

When a milestone gets its real implementation plan (via superpowers:writing-plans),
that plan supersedes the file here; update the status line and treat the plan as truth.

## Status

| Milestone | Status | Where |
|---|---|---|
| M1 — core LSP + real-time syntax diagnostics | **Done** (merged to main 2026-07-06) | plan + errata: `docs/superpowers/plans/2026-07-06-m1-core-lsp-diagnostics.md` |
| M2a — single-file navigation + stdlib | **Done** (merged to main 2026-07-07) | plan: `docs/superpowers/plans/2026-07-06-m2a-single-file-navigation.md` |
| M2b — cross-file resolution + workspace | **Done** (merged to main 2026-07-07) | plan + errata: `docs/superpowers/plans/2026-07-07-m2b-cross-file-and-workspace.md`; spec: `docs/superpowers/specs/2026-07-07-m2b-cross-file-and-workspace-design.md` |
| M3 — completion + CST-derived features | **Done** (merged to main 2026-07-10) | plan + errata: `docs/superpowers/plans/2026-07-08-m3-completion-and-cst-features.md`; spec: `docs/superpowers/specs/2026-07-08-m3-completion-and-cst-features-design.md` |
| M4 — toolchain integration | **Done** (merged to main 2026-07-10) | plan: `docs/superpowers/plans/2026-07-10-m4-toolchain-integration.md` |
| M5 — VS Code extension + distribution | Future | [m5-vscode-extension-and-distribution.md](m5-vscode-extension-and-distribution.md) |
| M6 — upstream automation (version watch, drift detection, agent-driven updates) | Future (version-watch CI can land any time) | [m6-upstream-automation.md](m6-upstream-automation.md) |
| v2 — native type checking | Post-v1 direction | [v2-native-type-checking.md](v2-native-type-checking.md) |
| v3 — native disclosure analysis | Post-v1 direction (flagship) | [v3-disclosure-analysis.md](v3-disclosure-analysis.md) |

M1–M5 together constitute **v1**, whose release bar is: a real multi-file Compact
project is comfortably developed in VS Code with compact-analyzer every day
(daily-driver dogfood). Approved design spec:
`docs/superpowers/specs/2026-07-06-compact-analyzer-design.md`.

## Cross-cutting expectations (no single milestone owns these)

- **Language-version policy:** track the latest Compact language version (0.23 at
  design time; compiler 0.31.x). Parse `pragma language_version`; unsupported versions
  still get parsing + navigation plus an info diagnostic. One grammar per release.
  Stdlib stubs are versioned per language release; keeping them (and every other
  upstream-mirroring asset) current is M6's job — see
  [m6-upstream-automation.md](m6-upstream-automation.md). Until M6 lands, each
  milestone records the exact regeneration procedure next to any asset it creates.
- **Upstream compactp work** (land in compactp, not here): incremental reparse and
  expression-position recovery hardening when dogfooding surfaces them; accessor gaps
  found during M2a research (ModuleDef members, ImportSpecifier `as` alias,
  WitnessDecl params); the `@@@`-diagnostic-anchors-at-offset-0 quirk. Use an
  uncommitted `[patch.crates-io]` pointing at `../compactp` for iteration — **never
  commit a `[patch.crates-io]` section**.
- **Corpus smoke testing:** the spec commits to running compactp's ~486-file corpus
  through indexing, resolution, and every IDE feature (asserting no panics, no
  out-of-bounds spans). Expected to land with M2b's workspace index if not sooner.
- **Deferred small code chores** (tracked in the git-ignored ledger
  `.superpowers/sdd/progress.md`; re-listed here for durability): per-file debounce
  (currently global, trailing-edge deferral under continuous edits); `LineIndex::offset()`
  edge-case fixtures once it gains more callers; analysis-cache polish (hash recomputed
  on cache hits, DefaultHasher collision comment, no eviction); lsp_utils Warning/Note
  severity-branch tests; vfs `path()` panics on foreign FileId (documented precondition).
