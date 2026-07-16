# v2b.final — VS Code Type-Diagnostic Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface the native type diagnostics (E3001–E3006) that `analyzer-core` already computes in the VS Code **Problems panel**, gated by a `compact-analyzer.typeDiagnostics` native-vs-compiler preference toggle, verified **in-editor** in the extension host.

**Architecture:** `AnalysisHost::type_diagnostics(file)` is fully implemented, corpus-gate-green, and returns `Vec<Diagnostic>` — but the server never calls it. This milestone wires that one query into the server's two native-diagnostic-building sites through a single shared helper, gates its contribution behind a server config toggle fed from a new client setting, and proves a type diagnostic reaches the editor via the existing `@vscode/test-cli` e2e harness. Source tagging (`source: "compact-analyzer"` vs `"compactc"`), span-coincidence dedup (`merge_diagnostics`), per-file diagnostic capping (`cap_diagnostics`), and restart-to-apply (`onDidChangeConfiguration`) already exist and are inherited unchanged.

**Tech Stack:** Rust (`compact-analyzer` binary, `lsp-server`/`lsp-types = 0.95.1`), TypeScript thin client (`editors/vscode/`, `vscode-languageclient`), vitest (client unit) + `@vscode/test-cli`/mocha (client e2e).

## Global Constraints

- **In-editor verification is the done-bar** (program spec §8): the milestone is not done until a native **type** diagnostic is observed in the VS Code extension host, not merely over raw LSP integration tests.
- **Never a false positive** (v2b hard constraint): the LSP layer only forwards `type_diagnostics`, which is already corpus-gate-green. Do NOT filter, re-rank, or fuzzy-dedup its output — forward it verbatim.
- **No LSP types in `analyzer-core`;** byte offsets only. All `Ty`→protocol conversion stays in the `compact-analyzer` binary (`lsp_utils.rs`).
- **One vocabulary, distinct sources:** native diagnostics (parser E0xxx, resolution E9xxx, type E3xxx) are tagged `source: "compact-analyzer"`; compiler diagnostics `source: "compactc"`. Preserve this exactly — type diagnostics flow through the existing `diagnostic_to_lsp` and inherit `"compact-analyzer"` automatically.
- **Follow the existing config + restart-to-apply pattern** (v1/M5): a `compact-analyzer.*` boolean setting → `config-core.buildInitOptions` → `initializationOptions.typeDiagnostics` → server `ToolchainConfig` field read **once at startup** → gate. The client's `onDidChangeConfiguration("compact-analyzer")` handler already prompts a restart for any namespace change; no new lifecycle code.
- **Pre-release:** no back-compat shims, no dual code paths — clean wholesale edits (DRY the two duplicated native-building sites into one helper).
- **Toggle semantics:** `compact-analyzer.typeDiagnostics` (boolean, default `true`). `true` = native live type diagnostics on; `false` = suppress the native type-diagnostic contribution and rely on compile-on-save's `compactc` type errors ("compiler preference"). The toggle gates ONLY the E3xxx type contribution — parser and resolution diagnostics are always published.

---

## Background: the exact as-merged wiring (read once, do not re-derive)

**The two native-diagnostic-building sites** in `crates/compact-analyzer/src/server.rs`, both of which today build `parser + resolution` and omit type diagnostics:

1. `publish_file_diagnostics(&mut self, file)` (the live debounced path — driven by `did_open`/`did_change` → `republish_open_diagnostics` → `flush_pending_diagnostics`), currently at lines ~884–917.
2. `did_save(&mut self, params)` (the on-save path — builds `native` on the main thread, then hands it to an off-loop compile worker that merges `native + compiler` and publishes), currently at lines ~761–843.

**Relevant `AnalysisHost` API** (all in `analyzer-core`, stable):
- `fn analyze(&mut self, file: FileId) -> Option<FileAnalysis>` — owned `FileAnalysis` (holds `diagnostics: Arc<Vec<Diagnostic>>`, `line_index: Arc<LineIndex>`); does NOT borrow `self` after return.
- `fn resolution_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic>`
- `fn type_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic>` — aggregates E3001–E3006 + generic-module suppression; returns `Vec::new()` when there is nothing to check (e.g. a pure parse-error file with no circuits/witnesses).
- `fn vfs(&self) -> &Vfs`; `Vfs::overlay_version(file) -> Option<i32>`.

**Diagnostic → LSP** (`crates/compact-analyzer/src/lsp_utils.rs`): `diagnostic_to_lsp(&Diagnostic, &LineIndex, &Url)` sets `source: Some("compact-analyzer")` and `code: Some(NumberOrString::String(d.code.to_string()))`. `DiagnosticCode`'s `Display` is `{prefix}{number:04}`, so a type diagnostic with `code.number == 3001` renders as `"E3001"`.

**Config** (`server.rs`): `ToolchainConfig { toolchain_path, compile_on_save, formatting }`, built by `toolchain_config_from(Option<&serde_json::Value>)` reading `initializationOptions`; stored on `GlobalState.toolchain_config`; read once at startup (line ~141). The client's `config-core.buildInitOptions` sends `compileOnSave` and `formatting` **explicitly even at default** (decoupling client/server defaults); `typeDiagnostics` follows that same "always sent" pattern.

**A real type-error fixture** (native emits E3001, `compactc` rejects — a genuine return-type mismatch, top-level so not generic-suppressed):
```
export circuit foo(): Field { return true; }
```
`true` is `Boolean`, which is not a subtype of the declared `Field` return → `type_diagnostics` emits one diagnostic with `code.number == 3001`. This is the fixture used by the integration and e2e tests below.

---

## File Structure

- **Modify** `crates/compact-analyzer/src/server.rs` — add `type_diagnostics: bool` to `ToolchainConfig`; parse `typeDiagnostics`; extract `build_native_diagnostics` helper (parser + resolution + gated type); re-point `publish_file_diagnostics` and `did_save` onto it. Add server-unit config tests.
- **Modify** `crates/compact-analyzer/tests/lsp_integration.rs` — integration tests: type diagnostic surfaces on `didOpen`; toggle-off suppresses it while keeping a parse error.
- **Modify** `editors/vscode/package.json` — add the `compact-analyzer.typeDiagnostics` configuration property.
- **Modify** `editors/vscode/src/config-core.ts` — add `typeDiagnostics` to `InitOptions` and `buildInitOptions` (always-sent, mirrors `compileOnSave`).
- **Modify** `editors/vscode/src/test/unit/config.test.ts` — update the exact-key-set assertions and add `typeDiagnostics` cases.
- **Create** `editors/vscode/src/test/e2e/fixture/type-error.compact` — the E3001 fixture.
- **Modify** `editors/vscode/src/test/e2e/activation.test.ts` — assert a type diagnostic (code `E3001`, source `compact-analyzer`) reaches the editor (the §8 done-bar).

---

### Task 1: Wire native type diagnostics into both publish paths (unconditional)

Extract the duplicated native-diagnostic assembly into one `build_native_diagnostics` helper that also includes `type_diagnostics`, and re-point both publish sites onto it. This task wires them **unconditionally** (the toggle is Task 2), so the gate is not yet present.

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs` (`publish_file_diagnostics` ~884–917; `did_save` ~761–843; add helper)
- Test: `crates/compact-analyzer/tests/lsp_integration.rs`

**Interfaces:**
- Consumes: `AnalysisHost::{analyze, resolution_diagnostics, type_diagnostics, vfs}`, `lsp_utils::diagnostic_to_lsp`.
- Produces: `GlobalState::build_native_diagnostics(&mut self, file: FileId, uri: &Url, line_index: &LineIndex) -> Vec<lsp_types::Diagnostic>` (used by Task 2's gate and both publish paths).

- [ ] **Step 1: Write the failing integration test**

Add to `crates/compact-analyzer/tests/lsp_integration.rs` (uses the existing `support::{Client, did_open, temp_doc}` harness):

```rust
#[test]
fn publishes_native_type_diagnostic_on_open() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    // Verified type error: `true` is Boolean, not a subtype of the declared
    // Field return -> native emits E3001, source "compact-analyzer".
    did_open(
        &mut client,
        &uri,
        1,
        "export circuit foo(): Field { return true; }",
    );
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d["code"] == "E3001" && d["source"] == "compact-analyzer"),
        "expected an E3001 type diagnostic from compact-analyzer, got {diags:#?}"
    );
    client.shutdown();
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p compact-analyzer --test lsp_integration publishes_native_type_diagnostic_on_open`
Expected: FAIL — no E3001 diagnostic is published (type diagnostics are not yet wired).

- [ ] **Step 3: Add the shared `build_native_diagnostics` helper**

Add this method to the `impl GlobalState` block in `server.rs` (place it immediately above `publish_file_diagnostics`). Import `LineIndex` if not already in scope (`use analyzer_core::LineIndex;` — check the existing `use` block; `FileId` and `Url` are already imported):

```rust
/// Assembles the full native diagnostic set for `file` — parser + resolution
/// + (when `typeDiagnostics` is enabled) type diagnostics — each converted to
/// LSP and tagged `source = "compact-analyzer"`. This is the SINGLE place
/// native diagnostics are built, shared by the live publish path
/// (`publish_file_diagnostics`) and the on-save path (`did_save`), so the two
/// can never drift apart. `type_diagnostics` is already corpus-gate-green and
/// is forwarded verbatim (never re-filtered here).
fn build_native_diagnostics(
    &mut self,
    file: FileId,
    uri: &Url,
    line_index: &LineIndex,
) -> Vec<lsp_types::Diagnostic> {
    let mut native: Vec<lsp_types::Diagnostic> = match self.host.analyze(file) {
        Some(analysis) => analysis
            .diagnostics
            .iter()
            .map(|d| lsp_utils::diagnostic_to_lsp(d, line_index, uri))
            .collect(),
        None => Vec::new(),
    };
    for d in self.host.resolution_diagnostics(file) {
        native.push(lsp_utils::diagnostic_to_lsp(&d, line_index, uri));
    }
    for d in self.host.type_diagnostics(file) {
        native.push(lsp_utils::diagnostic_to_lsp(&d, line_index, uri));
    }
    native
}
```

- [ ] **Step 4: Re-point `publish_file_diagnostics` onto the helper**

Replace the body of `publish_file_diagnostics` between obtaining `uri` and building `diagnostics`. The current native-building block is:

```rust
        let Some(analysis) = self.host.analyze(file) else {
            return;
        };
        let version = self.host.vfs().overlay_version(file);
        let mut native: Vec<lsp_types::Diagnostic> = analysis
            .diagnostics
            .iter()
            .map(|d| lsp_utils::diagnostic_to_lsp(d, &analysis.line_index, &uri))
            .collect();
        for d in self.host.resolution_diagnostics(file) {
            native.push(lsp_utils::diagnostic_to_lsp(&d, &analysis.line_index, &uri));
        }
```

Replace it with (obtain the line index, drop the analysis borrow, then delegate):

```rust
        let Some(analysis) = self.host.analyze(file) else {
            return;
        };
        let line_index = analysis.line_index.clone();
        drop(analysis);
        let version = self.host.vfs().overlay_version(file);
        let native = self.build_native_diagnostics(file, &uri, &line_index);
```

Leave the rest of `publish_file_diagnostics` (the `compiler` re-merge, `merge_diagnostics`, and `publish`) unchanged.

- [ ] **Step 5: Re-point `did_save` onto the helper**

In `did_save`, the current native-building block is:

```rust
        let line_index = analysis.line_index.clone();
        // Native diagnostics, computed on the main thread (the worker can't read
        // `host`): both the parser set and the import/include resolution set,
        // exactly as `publish_file_diagnostics` builds them.
        let mut native: Vec<lsp_types::Diagnostic> = analysis
            .diagnostics
            .iter()
            .map(|d| lsp_utils::diagnostic_to_lsp(d, &line_index, &uri))
            .collect();
        drop(analysis);
        for d in self.host.resolution_diagnostics(file) {
            native.push(lsp_utils::diagnostic_to_lsp(&d, &line_index, &uri));
        }
```

Replace it with:

```rust
        let line_index = analysis.line_index.clone();
        drop(analysis);
        // Native diagnostics (parser + resolution + type), computed on the main
        // thread because the compile worker cannot read `host`. Built through
        // the shared helper so the live and on-save paths never diverge.
        let native = self.build_native_diagnostics(file, &uri, &line_index);
```

Leave the rest of `did_save` (the `search_path`, generation bump, `CompileJob` construction using `native` and `line_index`, and thread spawn) unchanged.

- [ ] **Step 6: Run the new test and the full integration suite**

Run: `cargo test -p compact-analyzer --test lsp_integration`
Expected: PASS — the new `publishes_native_type_diagnostic_on_open` passes AND every pre-existing integration test still passes. In particular `publishes_diagnostics_then_clears_after_fix` (a pure parse-error file, then a clean `ledger count: Field;`) must still see exactly 1 then 0 diagnostics: a parse-error file has no circuits/witnesses so `type_diagnostics` is empty, and the clean ledger decl has no checkable body.

- [ ] **Step 7: Run the whole workspace build + lints**

Run: `cargo test -p compact-analyzer && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: all green, fmt clean.

- [ ] **Step 8: Commit**

```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests/lsp_integration.rs
git commit -m "$(cat <<'EOF'
feat(v2b.final): surface native type diagnostics in the publish path

Extract the duplicated parser+resolution native-diagnostic assembly into a
single build_native_diagnostics helper that also includes type_diagnostics,
and re-point publish_file_diagnostics and did_save onto it.

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `typeDiagnostics` server config toggle

Add the `type_diagnostics: bool` field to `ToolchainConfig` (default `true`), parse it from `initializationOptions.typeDiagnostics`, and gate the type-diagnostic contribution in `build_native_diagnostics`. Parser and resolution diagnostics remain unconditional.

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs` (`ToolchainConfig`, `toolchain_config_from`, `build_native_diagnostics`; server-unit tests)
- Test: `crates/compact-analyzer/tests/lsp_integration.rs`

**Interfaces:**
- Consumes: `GlobalState.toolchain_config` (`ToolchainConfig`), the Task 1 helper.
- Produces: `ToolchainConfig.type_diagnostics: bool`; the gate `if self.toolchain_config.type_diagnostics { … }` inside `build_native_diagnostics`.

- [ ] **Step 1: Write the failing server-unit config test**

Add to the `#[cfg(test)] mod tests` in `server.rs` (next to `toolchain_config_reads_options`):

```rust
#[test]
fn toolchain_config_reads_type_diagnostics_flag() {
    let opts = json!({ "typeDiagnostics": false });
    let config = toolchain_config_from(Some(&opts));
    assert!(!config.type_diagnostics);
}

#[test]
fn toolchain_config_type_diagnostics_defaults_true() {
    assert!(toolchain_config_from(Some(&json!({}))).type_diagnostics);
    assert!(toolchain_config_from(None).type_diagnostics);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p compact-analyzer --lib 2>&1 | head -30` (or `cargo test -p compact-analyzer --test ...` — these are `#[cfg(test)]` in the binary crate, so `cargo test -p compact-analyzer` covers them).
Expected: FAIL to COMPILE — `ToolchainConfig` has no `type_diagnostics` field.

- [ ] **Step 3: Add the field + default + parse**

In `ToolchainConfig` (the `pub(crate) struct`), add the field (drop the `#[allow(dead_code)]` markers are per-field; add the new field without one since it is now read):

```rust
pub(crate) struct ToolchainConfig {
    #[allow(dead_code)]
    pub toolchain_path: Option<PathBuf>,
    #[allow(dead_code)]
    pub compile_on_save: bool,
    pub formatting: bool,
    /// Whether native type diagnostics (E3001–E3006) are published. `true` =
    /// live native type checking; `false` = suppress them and rely on
    /// compile-on-save's `compactc` type errors. Read once at startup.
    pub type_diagnostics: bool,
}
```

In `impl Default for ToolchainConfig`, add `type_diagnostics: true,`.

In `toolchain_config_from`, after the `formatting` block, add:

```rust
    if let Some(flag) = opts.get("typeDiagnostics").and_then(|v| v.as_bool()) {
        config.type_diagnostics = flag;
    }
```

- [ ] **Step 4: Gate the contribution in `build_native_diagnostics`**

Wrap the `type_diagnostics` loop added in Task 1:

```rust
    if self.toolchain_config.type_diagnostics {
        for d in self.host.type_diagnostics(file) {
            native.push(lsp_utils::diagnostic_to_lsp(&d, line_index, uri));
        }
    }
```

- [ ] **Step 5: Write the failing integration test for the toggle**

The existing `support` harness's `Client::initialize()` sends default `initializationOptions`. Check `crates/compact-analyzer/tests/support/mod.rs` for a helper that initializes with custom options; if none exists, add one mirroring the existing `initialize` (send `initialize` with a `params` object carrying `initializationOptions`). Then add:

```rust
#[test]
fn type_diagnostics_toggle_off_suppresses_type_but_keeps_parse() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    // Initialize with typeDiagnostics disabled.
    client.initialize_with_options(json!({ "typeDiagnostics": false }));

    // A file with BOTH a parse error and (were it parseable) a type error.
    // The missing colon is a parse error (E0001) that must still surface;
    // type diagnostics are suppressed by the toggle.
    did_open(&mut client, &uri, 1, "ledger count Field;");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    assert!(
        diags.iter().any(|d| d["source"] == "compact-analyzer"),
        "parse/resolution diagnostics must still publish with the toggle off, got {diags:#?}"
    );
    assert!(
        diags.iter().all(|d| d["code"] != "E3001"),
        "no type diagnostic (E3xxx) should be published with the toggle off, got {diags:#?}"
    );
    client.shutdown();
}
```

If you add `initialize_with_options`, keep the existing `initialize()` as `initialize_with_options(json!({}))` or a thin wrapper — do NOT duplicate the handshake logic.

- [ ] **Step 6: Run to verify it fails, then passes after the gate**

Run: `cargo test -p compact-analyzer --test lsp_integration type_diagnostics_toggle_off_suppresses_type_but_keeps_parse`
Expected after Steps 3–4: PASS. (Before the gate, all type diagnostics published unconditionally; for `ledger count Field;` there is no circuit body so E3001 wouldn't appear anyway — that is why the fixture is a parse-error ledger decl: it proves parse diagnostics survive the toggle. The `all(code != "E3001")` assertion is the guard; if you want a stronger guard, add a second `did_open` of `export circuit foo(): Field { return true; }` under the same disabled session and assert no `E3001` appears.)

Strengthen the test by appending, under the same disabled-session client:

```rust
    // A clean-parsing file whose ONLY error is a type error: with the toggle
    // off it must publish ZERO compact-analyzer diagnostics.
    let (_dir2, uri2) = temp_doc();
    did_open(&mut client, &uri2, 1, "export circuit foo(): Field { return true; }");
    let params2 = client.wait_for_notification("textDocument/publishDiagnostics");
    // The publish for uri2:
    let d2 = params2["diagnostics"].as_array().unwrap();
    assert!(
        d2.iter().all(|d| d["code"] != "E3001"),
        "type diagnostic must be suppressed with the toggle off, got {d2:#?}"
    );
```

(If `wait_for_notification` may return the wrong file's publish, filter on `params2["uri"] == json!(uri2)` — check how the existing tests disambiguate; `temp_doc` gives a unique uri per file.)

- [ ] **Step 7: Full suite + lints**

Run: `cargo test -p compact-analyzer && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: green.

- [ ] **Step 8: Commit**

```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests/lsp_integration.rs crates/compact-analyzer/tests/support/mod.rs
git commit -m "$(cat <<'EOF'
feat(v2b.final): typeDiagnostics server toggle gates the type contribution

Add ToolchainConfig.type_diagnostics (default true), parse it from
initializationOptions.typeDiagnostics, and gate only the E3xxx contribution in
build_native_diagnostics. Parser and resolution diagnostics stay unconditional.

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: VS Code client config — the `typeDiagnostics` setting

Add the client setting and map it onto `initializationOptions`, following the always-sent `compileOnSave`/`formatting` pattern exactly. Restart-to-apply is already covered by the existing `onDidChangeConfiguration("compact-analyzer")` handler — no lifecycle code changes.

**Files:**
- Modify: `editors/vscode/package.json` (`contributes.configuration.properties`)
- Modify: `editors/vscode/src/config-core.ts` (`InitOptions`, `buildInitOptions`)
- Test: `editors/vscode/src/test/unit/config.test.ts`

**Interfaces:**
- Consumes: `ConfigReader` (injected).
- Produces: `InitOptions.typeDiagnostics?: boolean` (always sent, defaulting non-boolean to `true`).

- [ ] **Step 1: Update the failing client unit tests**

In `editors/vscode/src/test/unit/config.test.ts`, update the exact-key-set assertions to include `typeDiagnostics`, and add coverage. Change the defaults test:

```ts
it("(a) at defaults sends EXACTLY compileOnSave + formatting + typeDiagnostics", () => {
  const result = buildInitOptions(fakeReader({}));
  expect(Object.keys(result).sort()).toEqual(["compileOnSave", "formatting", "typeDiagnostics"]);
  expect(result).toEqual({ compileOnSave: true, formatting: true, typeDiagnostics: true });
});
```

Update the other `Object.keys(result).sort()` assertions in this file (the `importSearchPath: []` / `toolchainPath: ""` omit case and the `importSearchPath: null` case) from `["compileOnSave", "formatting"]` to `["compileOnSave", "formatting", "typeDiagnostics"]`. Add:

```ts
it("preserves a configured typeDiagnostics=false", () => {
  const result = buildInitOptions(fakeReader({ typeDiagnostics: false }));
  expect(result.typeDiagnostics).toBe(false);
});

it("defaults a non-boolean typeDiagnostics to true", () => {
  const result = buildInitOptions(fakeReader({ typeDiagnostics: "yes" }));
  expect(result.typeDiagnostics).toBe(true);
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd editors/vscode && npx vitest run config` (the unit suite is `npm test` → `vitest run`).
Expected: FAIL — `buildInitOptions` does not yet emit `typeDiagnostics`.

- [ ] **Step 3: Add `typeDiagnostics` to `InitOptions` and `buildInitOptions`**

In `config-core.ts`, extend the `InitOptions` interface:

```ts
export interface InitOptions {
  importSearchPath?: string[];
  toolchainPath?: string;
  compileOnSave?: boolean;
  formatting?: boolean;
  typeDiagnostics?: boolean;
}
```

Update the comment on the interface (it says "server reads ONLY these four keys") to say **five**. In `buildInitOptions`, after the `formatting` line, add (mirroring the `compileOnSave`/`formatting` always-sent handling):

```ts
  const typeDiagnostics: unknown = read("typeDiagnostics");
  options.typeDiagnostics = typeof typeDiagnostics === "boolean" ? typeDiagnostics : true;
```

- [ ] **Step 4: Add the package.json configuration property**

In `editors/vscode/package.json`, under `contributes.configuration.properties`, add (place it after `compact-analyzer.formatting`):

```json
"compact-analyzer.typeDiagnostics": {
  "type": "boolean",
  "default": true,
  "markdownDescription": "Publish native type diagnostics (as-you-type type checking) in the Problems panel, sent to the server as `initializationOptions.typeDiagnostics`. When disabled, only parse and name-resolution diagnostics are shown live and type errors are surfaced by the compiler on save (via `compileOnSave`). Changing this setting requires restarting the language server."
}
```

- [ ] **Step 5: Run the client unit suite + typecheck + lint**

Run: `cd editors/vscode && npm test && npm run check && npm run lint`
(`npm test` = `vitest run`; `npm run check` = `tsc --noEmit` for both the extension and the e2e tsconfig; `npm run lint` = `eslint src`.)
Expected: green; TypeScript typechecks; eslint clean.

- [ ] **Step 6: Commit**

```bash
git add editors/vscode/package.json editors/vscode/src/config-core.ts editors/vscode/src/test/unit/config.test.ts
git commit -m "$(cat <<'EOF'
feat(v2b.final): compact-analyzer.typeDiagnostics client setting

Add the boolean setting and map it onto initializationOptions.typeDiagnostics
(always sent, default true), mirroring compileOnSave/formatting. Restart-to-apply
is already handled by the existing onDidChangeConfiguration handler.

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: In-editor verification (the §8 done-bar)

Prove a native **type** diagnostic reaches the VS Code Problems panel in the real extension host, via the existing `@vscode/test-cli` e2e harness. This is the milestone's actual done-bar — "works over raw LSP" is not "works for users."

**Files:**
- Create: `editors/vscode/src/test/e2e/fixture/type-error.compact`
- Modify: `editors/vscode/src/test/e2e/activation.test.ts`

**Interfaces:**
- Consumes: the existing `waitForCompactDiagnostic` poll + `fixtureUri` helpers and the built `target/debug/compact-analyzer` binary wired via `serverPath` (already set up by `.vscode-test.mjs`).
- Produces: a passing e2e assertion that a type diagnostic with code `E3001` and source `compact-analyzer` appears.

- [ ] **Step 1: Create the type-error fixture**

`editors/vscode/src/test/e2e/fixture/type-error.compact`:

```
// `true` is Boolean, not a subtype of the declared Field return, so the native
// analyser reports E3001 ("return type mismatch") with source "compact-analyzer".
// This is a pure type error that needs no Compact toolchain — the smoke test
// stays hermetic and proves the v2b type checker reaches the Problems panel.
export circuit foo(): Field { return true; }
```

> Verify-first: before relying on `E3001`, confirm the live verdict with `compact compile --skip-zk --vscode <path/to/type-error.compact> <scratch-out>` exits non-zero (compactc rejects), and confirm `type_diagnostics` emits `code.number == 3001` for this exact source (a unit assertion of this already exists in `infer.rs` — `generic_module_body_errors...` / return-mismatch tests use the same shape). If the code differs, use the code the analyzer actually emits.

- [ ] **Step 2: Add a failing e2e assertion for the type diagnostic**

In `activation.test.ts`, extend scenario 1 (the valid-serverPath, server-running scenario) with a new `test` that opens `type-error.compact` and waits for a diagnostic whose `source` contains `compact-analyzer` **and** whose `code` is `E3001`. Add a small helper mirroring `waitForCompactDiagnostic` but keyed on the code, or extend the existing one. Concretely, add after the existing scenario-1 test:

```ts
test("scenario 1b: a native type diagnostic (E3001) reaches the Problems panel", async function () {
  this.timeout(DIAGNOSTIC_TIMEOUT_MS + 15_000);
  assert.strictEqual(api.serverStatus(), "running", "server should be running for the type-diagnostic check");

  const uri = fixtureUri("type-error.compact");
  const document = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(document);

  const deadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
  let typeDiags: vscode.Diagnostic[] = [];
  for (;;) {
    const diagnostics = vscode.languages.getDiagnostics(uri);
    typeDiags = diagnostics.filter(
      (d) => (d.source ?? "").includes("compact-analyzer") && String(d.code) === "E3001",
    );
    if (typeDiags.length >= 1 || Date.now() >= deadline) {
      break;
    }
    await delay(200);
  }
  assert.ok(
    typeDiags.length >= 1,
    `expected an E3001 native type diagnostic within ${DIAGNOSTIC_TIMEOUT_MS}ms, ` +
      `got ${JSON.stringify(vscode.languages.getDiagnostics(uri).map((d) => ({ source: d.source, code: d.code, message: d.message })))}`,
  );
});
```

> Note: VS Code exposes the LSP `code` string on `vscode.Diagnostic.code`. It may be a bare string (`"E3001"`) or `{ value, target }`; `String(d.code)` normalizes the bare-string case. If the harness wraps it, compare `String((d.code as { value?: unknown }).value ?? d.code)`. Confirm the actual shape when the test first runs and adjust the comparison — do NOT weaken the assertion to "any compact-analyzer diagnostic" (scenario 1 already covers that; this test must specifically prove a **type** diagnostic).

- [ ] **Step 3: Run the e2e suite to verify it fails, then passes**

First rebuild the server binary so the e2e harness wires the current build via `serverPath`: `cargo build -p compact-analyzer`. Then run: `cd editors/vscode && npm run test:e2e` (`pretest:e2e` runs `npm run build`; it also downloads VS Code, so allow several minutes).
Expected: after Tasks 1–2's changes are present in `target/debug/compact-analyzer`, the new `scenario 1b` passes.

- [ ] **Step 4: Full client verification**

Run: `cd editors/vscode && npm test && npm run check && npm run lint && npm run test:e2e`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add editors/vscode/src/test/e2e/fixture/type-error.compact editors/vscode/src/test/e2e/activation.test.ts
git commit -m "$(cat <<'EOF'
test(v2b.final): e2e proof a native type diagnostic reaches the Problems panel

Add a type-error fixture (E3001) and an extension-host assertion that a
compact-analyzer type diagnostic surfaces in the editor — the §8 in-editor
done-bar for v2b.

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

## Final verification (whole-milestone)

Run before finishing the branch:

- [ ] `cargo test --workspace` — full Rust suite green (lib + all integration tests, including `lsp_integration`).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` — clean.
- [ ] **Corpus gate stays green** — the LSP wiring does not touch `type_diagnostics`' logic, but confirm no regression: `COMPACT_CORPUS_DIR=<corpus> cargo test -p compact-analyzer --test type_differential corpus_no_false_positives -- --nocapture` prints `false_positives=0`. (Ask the human for the corpus path if the env var is unset; if genuinely unavailable, note it — the wiring is logically corpus-neutral.)
- [ ] `cd editors/vscode && npm test && npm run check && npm run lint && npm run test:e2e` — client unit + typecheck + lint + e2e green.
- [ ] Manual sanity (optional but recommended for the §8 done-bar): open `type-error.compact` in a dev-host VS Code, confirm the E3001 squiggle shows in the Problems panel sourced `compact-analyzer`; set `compact-analyzer.typeDiagnostics: false`, accept the restart prompt, confirm the squiggle disappears while a parse error in another file still shows.

## Notes carried from the drift review (for the reviewer)

- **Merge precedence is unchanged and intentional.** `merge_diagnostics` dedups compiler-vs-native only on exact-range coincidence (native wins). Native (live) and `compactc` (on-save) are distinctly tagged, so a type error may briefly appear from both sources at slightly different spans after a save. This matches the pre-existing behavior for resolution vs compiler diagnostics; fuzzy cross-source dedup is explicitly out of scope (YAGNI + risk). The `typeDiagnostics: false` toggle is the user's control to rely on `compactc` alone.
- **`cap_diagnostics` still bounds the merged payload** — type diagnostics count toward the `MAX_DIAGNOSTICS_PER_FILE` cap, which is correct.
- **`ToolchainConfig` naming** now also carries a non-toolchain flag (`type_diagnostics`); it is really "server config from initializationOptions." A rename is deferred (larger blast radius, no functional benefit) — noted, not done.
