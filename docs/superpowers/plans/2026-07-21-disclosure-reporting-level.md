# Disclosure Reporting Level — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the boolean `compact-analyzer.disclosureDiagnostics` VS Code setting with a 3-level enum (`all` / `leaks-only` / `off`) so users can keep confirmed disclosure leaks (E3100) while suppressing the fail-closed advisories (U3100).

**Architecture:** Purely a server-side DISPLAY FILTER plus its client-side setting. The analyzer-core disclosure computation is UNCHANGED (its §0 corpus guarantees stand). The server filters which already-computed disclosure diagnostics it publishes. Init option, applied at server start (restart-required), matching the existing `typeDiagnostics`/`disclosureDiagnostics` settings.

**Tech Stack:** Rust (LSP server, `crates/compact-analyzer/src/server.rs`), TypeScript (VS Code client, `editors/vscode/src/config-core.ts`), VS Code `package.json` configuration contributions.

**Spec:** `docs/superpowers/specs/2026-07-21-disclosure-reporting-level-design.md`.

## Global Constraints

- Wire contract (the ONLY interface between the two tasks): `initializationOptions.disclosureDiagnostics` is a STRING, one of exactly `"all"`, `"leaks-only"`, `"off"`. Default / unknown / wrong-typed → `"all"` (fail-safe: show everything).
- Pre-release: NO back-compat. Fully replace the boolean form; do not keep a bool code path.
- Default is `"all"` — a user who never touches the setting keeps the full fail-closed view.
- `leaks-only` filter rule: keep disclosure diagnostics whose `code.prefix == "E"` (the E3100 confirmed leaks); drop U-family advisories INCLUDING the mid-edit "disclosure analysis paused" notice.
- Do NOT touch `crates/analyzer-core` or the disclosure computation / corpus gates. This is display-only.
- Branch: `v3-disclosure-interproc-ux` (PR #6). Do NOT create a new branch.

---

### Task 1: Rust server — `DisclosureLevel` enum, config parsing, publish filter, quick-fix gate

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs` (struct ~1588-1613, `toolchain_config_from` ~1615-1636, quick-fix gate ~604-627, publish block ~1011-1030, tests ~1705-1721 / ~2103 / ~2128)

**Interfaces:**
- Consumes: `initializationOptions.disclosureDiagnostics: "all" | "leaks-only" | "off"` (string; default/unknown → `all`).
- Produces (internal): `pub enum DisclosureLevel { All, LeaksOnly, Off }`; `ToolchainConfig.disclosure_level: DisclosureLevel` (replaces `disclosure_diagnostics: bool`); `fn keep_disclosure(level: DisclosureLevel, diag: &compactp_diagnostics::Diagnostic) -> bool`.

- [ ] **Step 1: Write failing tests for config parsing + the filter helper**

Replace the two existing tests `toolchain_config_reads_disclosure_diagnostics_flag` and `toolchain_config_disclosure_diagnostics_defaults_true` (server.rs ~1710-1721) with these, and add the `keep_disclosure` test. (`json!` is already imported in the test module; `compactp_diagnostics` is the diagnostics crate — confirm the import path used elsewhere in the file, e.g. `compactp_diagnostics::{Diagnostic, DiagnosticCode, Severity}`.)

```rust
    #[test]
    fn toolchain_config_reads_disclosure_level() {
        assert_eq!(
            toolchain_config_from(Some(&json!({ "disclosureDiagnostics": "all" }))).disclosure_level,
            DisclosureLevel::All
        );
        assert_eq!(
            toolchain_config_from(Some(&json!({ "disclosureDiagnostics": "leaks-only" })))
                .disclosure_level,
            DisclosureLevel::LeaksOnly
        );
        assert_eq!(
            toolchain_config_from(Some(&json!({ "disclosureDiagnostics": "off" }))).disclosure_level,
            DisclosureLevel::Off
        );
    }

    #[test]
    fn toolchain_config_disclosure_level_defaults_and_tolerates_garbage() {
        // Missing key, empty options, and None all default to All.
        assert_eq!(
            toolchain_config_from(Some(&json!({}))).disclosure_level,
            DisclosureLevel::All
        );
        assert_eq!(toolchain_config_from(None).disclosure_level, DisclosureLevel::All);
        // Unknown string and a wrong-typed (legacy bool) value → All (fail-safe).
        assert_eq!(
            toolchain_config_from(Some(&json!({ "disclosureDiagnostics": "bogus" })))
                .disclosure_level,
            DisclosureLevel::All
        );
        assert_eq!(
            toolchain_config_from(Some(&json!({ "disclosureDiagnostics": false })))
                .disclosure_level,
            DisclosureLevel::All
        );
    }

    #[test]
    fn keep_disclosure_filters_by_level() {
        let e = Diagnostic::error(
            DiagnosticCode { prefix: "E", number: 3100 },
            "leak".to_string(),
            text_size::TextRange::empty(0.into()),
        );
        let u = Diagnostic::warning(
            DiagnosticCode { prefix: "U", number: 3100 },
            "advisory".to_string(),
            text_size::TextRange::empty(0.into()),
        );
        // All keeps both.
        assert!(keep_disclosure(DisclosureLevel::All, &e));
        assert!(keep_disclosure(DisclosureLevel::All, &u));
        // LeaksOnly keeps the E leak, drops the U advisory.
        assert!(keep_disclosure(DisclosureLevel::LeaksOnly, &e));
        assert!(!keep_disclosure(DisclosureLevel::LeaksOnly, &u));
        // Off drops both (defensive — Off skips the block, but the helper is total).
        assert!(!keep_disclosure(DisclosureLevel::Off, &e));
        assert!(!keep_disclosure(DisclosureLevel::Off, &u));
    }
```

NOTE: `DiagnosticCode` construction and the `Diagnostic::error`/`warning` signatures — match the actual `compactp_diagnostics` API (see its use elsewhere in the codebase, e.g. `crates/analyzer-core/src/disclosure/leaks.rs`). Adjust the literal construction to the real field/constructor shape; the ASSERTIONS are what matters.

- [ ] **Step 2: Run the tests — expect compile failure**

Run: `cargo test -p compact-analyzer --lib server 2>&1 | head`
Expected: FAILS to compile (`DisclosureLevel`, `disclosure_level`, `keep_disclosure` undefined).

- [ ] **Step 3: Add the `DisclosureLevel` enum + swap the `ToolchainConfig` field**

Replace the `disclosure_diagnostics: bool` field (server.rs ~1596-1600) and its `Default` (~1610). Add the enum just above `ToolchainConfig`:

```rust
/// Reporting level for native disclosure (WPP) diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisclosureLevel {
    /// Publish confirmed leaks (E3100) AND fail-closed advisories (U3100).
    #[default]
    All,
    /// Publish only confirmed leaks (E3100); drop the U-family advisories.
    LeaksOnly,
    /// Publish no live disclosure diagnostics (compiler surfaces them on save).
    Off,
}
```

Field (replaces `pub disclosure_diagnostics: bool`):
```rust
    /// Reporting level for native disclosure (WPP) diagnostics. `All` = leaks +
    /// advisories (fail-closed default); `LeaksOnly` = confirmed E3100 leaks
    /// only; `Off` = suppress the channel and rely on compile-on-save. Read once
    /// at startup.
    pub disclosure_level: DisclosureLevel,
```
`Default` for `ToolchainConfig` (replaces `disclosure_diagnostics: true`):
```rust
            disclosure_level: DisclosureLevel::All,
```

- [ ] **Step 4: Parse the string init option**

Replace the `disclosureDiagnostics` bool parse in `toolchain_config_from` (server.rs ~1632-1634) with:
```rust
    if let Some(s) = opts.get("disclosureDiagnostics").and_then(|v| v.as_str()) {
        config.disclosure_level = match s {
            "leaks-only" => DisclosureLevel::LeaksOnly,
            "off" => DisclosureLevel::Off,
            // "all" and any unknown string → All (fail-safe: show everything).
            _ => DisclosureLevel::All,
        };
    }
```

- [ ] **Step 5: Add the `keep_disclosure` filter helper**

Place it near `toolchain_config_from` (a free function). Use the same `Diagnostic` type `host.disclosure_diagnostics` returns (`compactp_diagnostics::Diagnostic`):
```rust
/// Whether a disclosure diagnostic is published at `level`. `All` keeps
/// everything; `LeaksOnly` keeps only confirmed leaks (E-family, E3100+) and
/// drops the U-family advisories (including the "analysis paused" notice);
/// `Off` keeps nothing (defensive — `Off` also skips the publish block).
fn keep_disclosure(level: DisclosureLevel, diag: &compactp_diagnostics::Diagnostic) -> bool {
    match level {
        DisclosureLevel::All => true,
        DisclosureLevel::LeaksOnly => diag.code.prefix == "E",
        DisclosureLevel::Off => false,
    }
}
```

- [ ] **Step 6: Apply the filter at the publish site + fix the quick-fix gate**

Publish block (server.rs ~1011-1030). Replace `if self.toolchain_config.disclosure_diagnostics {` with a level check, and filter each pushed diagnostic through `keep_disclosure` (this covers BOTH the paused notice and the normal set):
```rust
        if self.toolchain_config.disclosure_level != DisclosureLevel::Off {
            let level = self.toolchain_config.disclosure_level;
            if has_parse_errors {
                // The "paused" notice is a U-family advisory → shown only under All.
                let paused = disclosure_paused_diagnostic();
                if keep_disclosure(level, &paused) {
                    native.push(lsp_utils::diagnostic_to_lsp(&paused, line_index, uri));
                }
            } else {
                for d in self.host.disclosure_diagnostics(file) {
                    if keep_disclosure(level, &d) {
                        native.push(lsp_utils::diagnostic_to_lsp(&d, line_index, uri));
                    }
                }
            }
        }
```
(Keep the existing explanatory comment about the paused advisory above this block.)

Quick-fix gate (server.rs ~613). Replace `if !self.toolchain_config.disclosure_diagnostics {` with:
```rust
                        if self.toolchain_config.disclosure_level == DisclosureLevel::Off {
                            return Some(Vec::new());
                        }
```
(The disclose() quick-fix targets E3100 leaks, which are shown under both `All` and `LeaksOnly`; only `Off` suppresses it. Update the comment to say "off means no disclosure diagnostics are published".)

- [ ] **Step 7: Update the remaining references in existing tests**

- server.rs ~2103: `assert!(state.toolchain_config.disclosure_diagnostics)` → `assert_eq!(state.toolchain_config.disclosure_level, DisclosureLevel::All);`
- server.rs ~2128: `state.toolchain_config.disclosure_diagnostics = false;` → `state.toolchain_config.disclosure_level = DisclosureLevel::Off;`
- Grep the whole file for any remaining `disclosure_diagnostics` FIELD access (not the `host.disclosure_diagnostics(file)` METHOD call, which is unrelated and must stay) and update. Confirm: `grep -n "toolchain_config.disclosure_diagnostics\|\.disclosure_diagnostics:" crates/compact-analyzer/src/server.rs` returns nothing after this step.

- [ ] **Step 8: Run all server tests**

Run: `cargo test -p compact-analyzer --lib 2>&1 | tail -15`
Expected: PASS (new parsing + `keep_disclosure` tests green; the updated `disclosure_diagnostics_unaffected_by_well_formed_source` / off-suppression tests green).

- [ ] **Step 9: Add a publish-filter round-trip test (leaks-only drops U, keeps E)**

Find a fixture (or add one under the test fixtures the server tests use) whose source produces BOTH an E3100 leak AND a U3100 advisory in one file — e.g. a confirmed ledger-write leak PLUS a witness-carrying call to an unresolved/module circuit (the amber "cross-circuit call not interpreted" advisory). Use the existing `open_and_publish` helper (see `disclosure_diagnostics_unaffected_by_well_formed_source`, server.rs ~2149) with `state.toolchain_config.disclosure_level = DisclosureLevel::LeaksOnly;` set before publishing. Assert the published diagnostics contain the `E3100` and contain NO `U3100`. Add an `All` variant asserting BOTH appear. If constructing a dual-diagnostic fixture proves fiddly, rely on the `keep_disclosure` unit test (Step 1) as the filter proof and note it in the report — but make a genuine effort at the round-trip.

Run: `cargo test -p compact-analyzer --lib 2>&1 | tail -8`
Expected: PASS.

- [ ] **Step 10: clippy + fmt + commit**

Run: `cargo clippy -p compact-analyzer --all-targets -- -D warnings` (clean) and `cargo fmt` then `cargo fmt --check` (clean).
```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests 2>/dev/null; git add -A crates/compact-analyzer
git commit -m "feat(lsp): disclosure reporting level (all/leaks-only/off) server-side filter"
```

---

### Task 2: VS Code client — `package.json` enum setting + `config-core.ts` validation

**Files:**
- Modify: `editors/vscode/package.json` (the `compact-analyzer.disclosureDiagnostics` property ~100-104)
- Modify: `editors/vscode/src/config-core.ts` (`InitOptions` ~21-28, the reader ~83-85)
- Modify: `editors/vscode/src/test/unit/config.test.ts` (add cases)

**Interfaces:**
- Produces: `initializationOptions.disclosureDiagnostics: "all" | "leaks-only" | "off"` (the wire contract Task 1 consumes). Default / mistyped → `"all"`.

- [ ] **Step 1: Write failing config-core tests**

In `editors/vscode/src/test/unit/config.test.ts`, mirror the existing `buildInitOptions` tests (find the block that tests `disclosureDiagnostics`; replace/extend it). Add:
```ts
  it("maps disclosureDiagnostics enum values through unchanged", () => {
    for (const level of ["all", "leaks-only", "off"] as const) {
      const opts = buildInitOptions(fakeReader({ disclosureDiagnostics: level }));
      expect(opts.disclosureDiagnostics).toBe(level);
    }
  });

  it("defaults disclosureDiagnostics to 'all' when unset or mistyped", () => {
    expect(buildInitOptions(fakeReader({})).disclosureDiagnostics).toBe("all");
    // Legacy boolean and any other garbage → 'all'.
    expect(buildInitOptions(fakeReader({ disclosureDiagnostics: true })).disclosureDiagnostics).toBe("all");
    expect(buildInitOptions(fakeReader({ disclosureDiagnostics: "bogus" })).disclosureDiagnostics).toBe("all");
  });
```
NOTE: use the SAME `fakeReader` helper the existing tests in this file use (match its exact name/shape — read the file first). If existing `disclosureDiagnostics` boolean tests are present, DELETE them (the setting is no longer boolean).

- [ ] **Step 2: Run — expect failure**

Run: `cd editors/vscode && npm test -- config` (or the repo's configured vitest invocation; check `package.json` scripts).
Expected: FAIL (reader still returns a boolean / type mismatch).

- [ ] **Step 3: Update `InitOptions` + the reader in `config-core.ts`**

`InitOptions` (config-core.ts ~27): change
```ts
  disclosureDiagnostics?: boolean;
```
to
```ts
  disclosureDiagnostics?: "all" | "leaks-only" | "off";
```
Reader (config-core.ts ~83-85): replace
```ts
  const disclosureDiagnostics: unknown = read("disclosureDiagnostics");
  options.disclosureDiagnostics =
    typeof disclosureDiagnostics === "boolean" ? disclosureDiagnostics : true;
```
with
```ts
  // Reporting level: pass the three known values through; anything else
  // (unset, a legacy boolean, a typo) falls back to "all" — the fail-closed
  // full view, matching the server's own tolerant default.
  const disclosureDiagnostics: unknown = read("disclosureDiagnostics");
  options.disclosureDiagnostics =
    disclosureDiagnostics === "leaks-only" || disclosureDiagnostics === "off"
      ? disclosureDiagnostics
      : "all";
```

- [ ] **Step 4: Run config tests — expect pass**

Run: `cd editors/vscode && npm test -- config`
Expected: PASS.

- [ ] **Step 5: Update the `package.json` setting to the enum**

Replace the `compact-analyzer.disclosureDiagnostics` property (package.json ~100-104) with:
```json
        "compact-analyzer.disclosureDiagnostics": {
          "type": "string",
          "enum": ["all", "leaks-only", "off"],
          "enumDescriptions": [
            "Show all disclosure diagnostics: confirmed witness-disclosure leaks and the fail-closed 'unverified' advisories.",
            "Show only confirmed leaks; hide the fail-closed advisories. The advisories flag code the analyzer cannot verify, so hiding them means the editor no longer warns about unverifiable code — a clean editor is not a proof of privacy. The compiler on save (compileOnSave) remains authoritative.",
            "Publish no live disclosure diagnostics; disclosure is surfaced by the compiler on save (compileOnSave)."
          ],
          "default": "all",
          "markdownDescription": "Reporting level for native disclosure (WPP) diagnostics (as-you-type witness-disclosure analysis), sent to the server as `initializationOptions.disclosureDiagnostics`. `all` shows confirmed leaks and fail-closed advisories; `leaks-only` shows only confirmed leaks (hiding the 'unverified' advisories — note that a clean editor is then not a proof of privacy); `off` relies on the compiler on save (via `compileOnSave`). Changing this setting requires restarting the language server."
        }
```

- [ ] **Step 6: Verify the manifest is well-formed + build**

Run: `cd editors/vscode && node -e "JSON.parse(require('fs').readFileSync('package.json','utf8')); console.log('package.json OK')"` (valid JSON).
Run: `cd editors/vscode && npm run compile` (or the configured typecheck/build script — check `package.json` scripts; `tsc --noEmit` must be clean given the `InitOptions` type change).
Expected: both succeed.

- [ ] **Step 7: Run the full VS Code unit suite + commit**

Run: `cd editors/vscode && npm test` (all unit tests green — especially `config.test.ts` and any `gen-manifest`/`assets` test that snapshots `package.json`; update any such snapshot if the repo uses one).
```bash
git add editors/vscode/package.json editors/vscode/src/config-core.ts editors/vscode/src/test/unit/config.test.ts
git commit -m "feat(vscode): disclosure reporting-level setting (all/leaks-only/off enum)"
```
