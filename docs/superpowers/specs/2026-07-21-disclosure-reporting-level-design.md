# Disclosure Reporting Level — Design

**Goal:** Let a user cut disclosure-analysis noise by choosing to see only *confirmed* leaks
(E3100) and suppress the *advisory* "unverified" diagnostics (U3100), via a VS Code setting —
without weakening the analyzer's fail-closed guarantees.

**Context:** Today `compact-analyzer.disclosureDiagnostics` is a boolean (on = publish everything,
off = nothing live). The disclosure analyzer emits two diagnostic families: **E3100** confirmed
witness-disclosure leaks (red errors) and **U3100** fail-closed advisories ("unverified — a clean
editor is not a proof of privacy"). The advisories are deliberately conservative and can be noisy;
some users want the confirmed leaks without them. This adds a middle setting.

**Branch:** `v3-disclosure-interproc-ux` (PR #6) — extends the `disclosureDiagnostics` setting that
is part of that PR's disclosure UX work.

## The setting

Replace the boolean `compact-analyzer.disclosureDiagnostics` with a 3-value string enum
(pre-release — no migration/back-compat, per project policy):

| Value | Live diagnostics published | Notes |
|---|---|---|
| `"all"` **(default)** | E3100 leaks **and** U3100 advisories | Unchanged fail-closed default (== today's `true`) |
| `"leaks-only"` | E3100 confirmed leaks only | Cuts the "unverified" advisory noise |
| `"off"` | none | Compiler surfaces disclosure on save (== today's `false`) |

The default is `"all"`: a user who never touches the setting keeps the full fail-closed view.

## Semantics (display filter only)

The analyzer ALWAYS computes the complete E+U diagnostic set; the §0 corpus guarantees
(`false_positives=0`, `silent_on_reject=0`) are unaffected — this is purely a **server-side display
filter** on what gets published to the editor. No analyzer-core change.

Filter rule at the publish site (`crates/compact-analyzer/src/server.rs`, the
`if self.toolchain_config.disclosure_diagnostics { … }` block around L1011–1030):

- `All` → publish every disclosure diagnostic (current behavior).
- `LeaksOnly` → publish only disclosure diagnostics with `code.prefix == "E"` (the E3100 confirmed
  leaks). This drops the U3100 advisories AND the mid-edit "disclosure analysis paused" notice
  (`disclosure_paused_diagnostic()`, itself a U-family advisory) — consistent: a user who opted out
  of "unverified" notices also opts out of "I can't tell you right now".
- `Off` → publish no disclosure diagnostics (skip the whole block, current `false` behavior).

The disclose() quick-fix (gated at `server.rs` ~L613 on `disclosure_diagnostics`) is offered when the
level is `All` OR `LeaksOnly` (both surface E3100 leaks, which is what the quick-fix targets), and not
when `Off`.

## Honesty requirement (`leaks-only` description)

The U3100 advisories exist because the analysis is fail-closed: they flag code the analyzer *cannot
verify*. Hiding them means the editor stops warning about unverifiable code, so a clean editor is no
longer even a weak proof of privacy. The `leaks-only` `enumDescription` MUST say this plainly and
note the compiler on save (`compileOnSave`) remains authoritative — so users opt in knowingly rather
than mistaking silence for safety.

## The three layers (mirror the existing `typeDiagnostics`/`disclosureDiagnostics` plumbing)

1. **`editors/vscode/package.json`** — change `compact-analyzer.disclosureDiagnostics` from
   `type: "boolean"` to `type: "string"`, `enum: ["all","leaks-only","off"]`, `default: "all"`, with
   `enumDescriptions` (the `leaks-only` one carries the honesty note). Update the top-level
   `markdownDescription`. Keep "Changing this setting requires restarting the language server."

2. **`editors/vscode/src/config-core.ts`** — `InitOptions.disclosureDiagnostics` becomes
   `string` (one of the 3 values). The reader validates: if the raw value is one of the three literals
   use it, else fall back to `"all"`. Sent to the server as `initializationOptions.disclosureDiagnostics`.

3. **`crates/compact-analyzer/src/server.rs`** — replace `ToolchainConfig.disclosure_diagnostics: bool`
   with an enum `DisclosureLevel { All, LeaksOnly, Off }` (a field like `disclosure_level`).
   `toolchain_config_from` parses the string init option (`"all"|"leaks-only"|"off"`, default `All`;
   unknown/missing → `All`). Also accept legacy nothing-else — no bool path (pre-release). The publish
   gate and quick-fix gate switch on the enum.

## Restart behavior

Init option, applied at server start — a change requires a language-server restart, matching the
existing `typeDiagnostics`/`disclosureDiagnostics`/`compileOnSave` settings. Live application on
`didChangeConfiguration` (feasible since it's a pure display filter) is an explicit **non-goal** here,
noted as an easy follow-on.

## Testing

- **Rust (`server.rs` unit tests):** `toolchain_config_from` maps `"all"`→`All`, `"leaks-only"`→
  `LeaksOnly`, `"off"`→`Off`, missing→`All`, garbage string→`All`. Replaces the existing
  `toolchain_config_reads_disclosure_diagnostics_flag` / `…defaults_true` bool tests.
- **Rust (publish filter):** a source with BOTH an E3100 and a U3100 disclosure diagnostic →
  assert `LeaksOnly` publishes the E and drops the U; `Off` publishes neither; `All` publishes both.
  (Drive via the server's diagnostic-collection path or a focused filter helper; if a full server
  round-trip is heavy, extract the filter into a small pure function `fn keep_disclosure(level,
  &Diagnostic) -> bool` and unit-test that + assert the publish site calls it.)
- **TypeScript (`config-core` tests):** each of the 3 values passes through; a non-enum/garbage value
  and a missing value default to `"all"`.

## Non-goals

- Live (no-restart) application on config change — deferred follow-on.
- Finer advisory gradations (e.g. hide only "unresolved-callee" advisories) — deferred; `leaks-only`
  is the coarse cut users asked for.
- Any change to analyzer-core / the disclosure computation or its corpus gates.
