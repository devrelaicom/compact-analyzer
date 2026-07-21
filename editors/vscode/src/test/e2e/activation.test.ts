// Activation smoke E2E (Task 10). Runs inside a real VS Code extension host via
// `@vscode/test-cli`, driving the published extension only through observable
// surfaces: the `ExtensionApi` returned by `activate()`, the VS Code diagnostics
// collection, and the `restartServer` command. As a thin-client smoke test it
// asserts NO analysis behaviour of its own — it simply proves that a locally
// built server binary, wired in through `compact-analyzer.serverPath`, brings
// the client up and that native diagnostics reach the editor, and that a bad
// configured path degrades cleanly to an unavailable session.
//
// Two hermetic scenarios (NO network), differing only by the injected
// `serverPath`:
//   1. serverPath -> a freshly built `target/debug/compact-analyzer`
//      => opening the invalid fixture yields >=1 diagnostic whose `source`
//         contains "compact-analyzer" within 30s, and serverStatus() is running.
//   2. serverPath -> an absolute path that cannot exist
//      => activation completes, serverStatus() is unavailable, no unhandled
//         rejection escapes.
//
// The valid serverPath for scenario 1 is written into the fixture's
// `.vscode/settings.json` by `.vscode-test.mjs` BEFORE the host launches, so the
// very first acquire uses the settings source and never reaches the (network)
// download leg. Scenario 2 then rewrites the setting in-place and restarts.

import * as assert from "node:assert";

import * as vscode from "vscode";

/** The extension's public activation surface (mirrors Task 9's `ExtensionApi`). */
interface ExtensionApi {
  serverStatus(): "running" | "unavailable";
  serverSource(): "settings" | "path" | "storage" | "downloaded" | null;
}

const EXTENSION_ID = "aaronbassett.compact-analyzer";
const RESTART_COMMAND = "compact-analyzer.restartServer";
const DIAGNOSTIC_TIMEOUT_MS = 30_000;

/** An absolute path that cannot exist, so the settings-source acquire hard-fails (D5). */
const NONEXISTENT_SERVER_PATH = "/nonexistent/compact-analyzer-xyz";

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/** Resolve a fixture file URI relative to the opened workspace folder. */
function fixtureUri(name: string): vscode.Uri {
  const folder = vscode.workspace.workspaceFolders?.[0];
  assert.ok(folder, "expected the fixture workspace folder to be open");
  return vscode.Uri.joinPath(folder.uri, name);
}

/**
 * Poll the diagnostics for `uri` until at least one carries a `source` that
 * contains "compact-analyzer", or the timeout elapses. Diagnostics arrive
 * asynchronously over LSP, so a poll (rather than a single read) is required.
 * Returns whatever diagnostics are present when it stops.
 */
async function waitForCompactDiagnostic(uri: vscode.Uri, timeoutMs: number): Promise<vscode.Diagnostic[]> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const diagnostics = vscode.languages.getDiagnostics(uri);
    if (diagnostics.some((d) => (d.source ?? "").includes("compact-analyzer"))) {
      return diagnostics;
    }
    if (Date.now() >= deadline) {
      return diagnostics;
    }
    await delay(200);
  }
}

/**
 * An `InlayHint.label` is either a plain string or `InlayHintLabelPart[]`;
 * normalize to a string so callers can do a simple substring match.
 */
function normalizeInlayHintLabel(label: string | vscode.InlayHintLabelPart[]): string {
  return typeof label === "string" ? label : label.map((part) => part.value).join("");
}

/**
 * A `vscode.Diagnostic.code` renders as a plain string/number UNLESS the
 * server also attaches an LSP `codeDescription` (v3c C2, U-family advisories
 * only) — `vscode-languageclient`'s `asDiagnostic` then folds the two into
 * `{ value, target }` (see its `protocolConverter.js`). Normalize to the bare
 * code string so callers can compare/filter without caring which shape
 * arrived.
 */
function diagnosticCodeValue(code: vscode.Diagnostic["code"]): string | undefined {
  if (code === undefined) {
    return undefined;
  }
  return typeof code === "object" ? String(code.value) : String(code);
}

suite("Compact Analyzer activation smoke", () => {
  let api: ExtensionApi;
  const rejections: unknown[] = [];
  const onRejection = (reason: unknown): void => {
    rejections.push(reason);
  };

  suiteSetup(async function () {
    // Downloading and unpacking VS Code plus the initial LSP handshake can take a
    // while on a cold CI runner; give the whole suite room beyond the 30s poll.
    this.timeout(120_000);
    process.on("unhandledRejection", onRejection);

    const extension = vscode.extensions.getExtension(EXTENSION_ID);
    assert.ok(extension, `extension ${EXTENSION_ID} must be present in the host`);

    // Activation must never throw; it resolves to the ExtensionApi once the
    // initial acquire/start has run against the pre-injected valid serverPath.
    api = (await extension.activate()) as ExtensionApi;
    assert.strictEqual(typeof api.serverStatus, "function", "activate() must return the ExtensionApi");
  });

  suiteTeardown(() => {
    process.off("unhandledRejection", onRejection);
  });

  test("scenario 1: valid serverPath brings the server up and surfaces a native diagnostic", async function () {
    this.timeout(DIAGNOSTIC_TIMEOUT_MS + 15_000);

    // The pre-injected settings serverPath is the valid built binary, so the
    // server should already be running after activation via the settings source.
    assert.strictEqual(api.serverStatus(), "running", "server should be running with a valid serverPath");
    assert.strictEqual(api.serverSource(), "settings", "the running server should come from the settings source");

    const uri = fixtureUri("invalid.compact");
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(document);

    const diagnostics = await waitForCompactDiagnostic(uri, DIAGNOSTIC_TIMEOUT_MS);
    const fromAnalyzer = diagnostics.filter((d) => (d.source ?? "").includes("compact-analyzer"));
    assert.ok(
      fromAnalyzer.length >= 1,
      `expected >=1 diagnostic with source containing "compact-analyzer" within ${DIAGNOSTIC_TIMEOUT_MS}ms, ` +
        `got ${diagnostics.length} diagnostic(s): ${JSON.stringify(diagnostics.map((d) => ({ source: d.source, message: d.message })))}`,
    );
    // The server must still be running once diagnostics have flowed.
    assert.strictEqual(api.serverStatus(), "running", "server should remain running after publishing diagnostics");
  });

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

  test("scenario 1f: a disclosure leak (E3100) reaches the Problems panel as a red diagnostic", async function () {
    this.timeout(DIAGNOSTIC_TIMEOUT_MS + 15_000);
    assert.strictEqual(api.serverStatus(), "running", "server should be running for the disclosure-leak check");

    const uri = fixtureUri("disclosure-leak.compact");
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(document);

    const deadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
    let leakDiag: vscode.Diagnostic | undefined;
    for (;;) {
      leakDiag = vscode.languages
        .getDiagnostics(uri)
        .find((d) => d.source === "compact-analyzer" && String(d.code) === "E3100");
      if (leakDiag || Date.now() >= deadline) {
        break;
      }
      await delay(200);
    }
    assert.ok(
      leakDiag,
      `expected an E3100 disclosure leak within ${DIAGNOSTIC_TIMEOUT_MS}ms, ` +
        `got ${JSON.stringify(vscode.languages.getDiagnostics(uri).map((d) => ({ source: d.source, code: d.code, message: d.message })))}`,
    );
    assert.strictEqual(leakDiag.severity, vscode.DiagnosticSeverity.Error, "an E3100 leak must be an error");

    // The witness->sink trail (spec §3.7): `leak_to_diagnostic` attaches one
    // secondary span per witness origin, which the client renders as
    // `relatedInformation`. `getW()`'s own declaration is the only witness
    // origin on this fixture's single-hop leak.
    assert.ok(
      leakDiag.relatedInformation && leakDiag.relatedInformation.length >= 1,
      `expected the leak to carry a witness->sink relatedInformation trail, got ${JSON.stringify(leakDiag.relatedInformation)}`,
    );
    assert.ok(
      leakDiag.relatedInformation.some((info) => info.message.includes("getW")),
      `expected a relatedInformation entry pointing at the witness "getW", got ${JSON.stringify(
        leakDiag.relatedInformation.map((info) => info.message),
      )}`,
    );
  });

  test("scenario 1g: a fail-closed advisory (U3100) reaches the Problems panel as an amber diagnostic", async function () {
    this.timeout(DIAGNOSTIC_TIMEOUT_MS + 15_000);
    assert.strictEqual(api.serverStatus(), "running", "server should be running for the disclosure-advisory check");

    const uri = fixtureUri("disclosure-advisory.compact");
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(document);

    const deadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
    let advisoryDiag: vscode.Diagnostic | undefined;
    for (;;) {
      advisoryDiag = vscode.languages
        .getDiagnostics(uri)
        .find((d) => d.source === "compact-analyzer (unverified)" && diagnosticCodeValue(d.code) === "U3100");
      if (advisoryDiag || Date.now() >= deadline) {
        break;
      }
      await delay(200);
    }
    assert.ok(
      advisoryDiag,
      `expected a U3100 advisory within ${DIAGNOSTIC_TIMEOUT_MS}ms, ` +
        `got ${JSON.stringify(vscode.languages.getDiagnostics(uri).map((d) => ({ source: d.source, code: d.code, message: d.message })))}`,
    );
    assert.strictEqual(
      advisoryDiag.severity,
      vscode.DiagnosticSeverity.Warning,
      "a U3100 advisory must be a warning (amber), distinguished by source rather than severity",
    );

    // The advisory-UX contract (v3c C2): a U-family diagnostic carries an LSP
    // `codeDescription` href ("clean editor is not a proof of privacy; compile
    // is authoritative"). `vscode-languageclient` folds `code` + `codeDescription`
    // into `code = { value, target }`, so its presence as an object (rather than
    // the bare string an E-family code carries) IS the codeDescription signal.
    assert.strictEqual(
      typeof advisoryDiag.code,
      "object",
      `expected a U3100 advisory to carry a codeDescription (code as {value,target}), got ${JSON.stringify(advisoryDiag.code)}`,
    );
    const advisoryCode = advisoryDiag.code as { value: string | number; target: vscode.Uri };
    assert.strictEqual(String(advisoryCode.value), "U3100");
    assert.ok(
      advisoryCode.target.toString().includes("disclosure-advisories.md"),
      `expected the codeDescription target to point at the advisory-contract docs, got ${advisoryCode.target.toString()}`,
    );
  });

  test("scenario 1h: the disclose() quick-fix wraps the minimal expression, not the whole statement", async function () {
    this.timeout(DIAGNOSTIC_TIMEOUT_MS + 15_000);
    assert.strictEqual(api.serverStatus(), "running", "server should be running for the quick-fix check");

    const uri = fixtureUri("disclosure-leak.compact");
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(document);

    // Recover the E3100 leak established by scenario 1f (same open document;
    // diagnostics persist across tests in this suite).
    const deadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
    let leakDiag: vscode.Diagnostic | undefined;
    for (;;) {
      leakDiag = vscode.languages
        .getDiagnostics(uri)
        .find((d) => d.source === "compact-analyzer" && diagnosticCodeValue(d.code) === "E3100");
      if (leakDiag || Date.now() >= deadline) {
        break;
      }
      await delay(200);
    }
    assert.ok(leakDiag, `expected the E3100 leak from scenario 1f to still be present within ${DIAGNOSTIC_TIMEOUT_MS}ms`);

    const originalText = document.getText();
    // Pin the flagged range to the minimal RHS expression `getW()` — NOT the
    // whole `c = getW();` statement (differential-verified against `compactc`:
    // wrapping the whole statement is actively REJECTED, not merely imprecise).
    assert.strictEqual(
      document.getText(leakDiag.range),
      "getW()",
      `expected the leak's flagged range to be exactly "getW()", got ${JSON.stringify(document.getText(leakDiag.range))}`,
    );

    const actions =
      (await vscode.commands.executeCommand<(vscode.CodeAction | vscode.Command)[]>(
        "vscode.executeCodeActionProvider",
        uri,
        leakDiag.range,
      )) ?? [];
    const quickfixes = actions.filter(
      (a): a is vscode.CodeAction => a instanceof vscode.CodeAction && a.title === "Reveal this value with disclose()",
    );
    assert.strictEqual(
      quickfixes.length,
      1,
      `expected exactly one "Reveal this value with disclose()" quick-fix, got ${JSON.stringify(actions.map((a) => a.title))}`,
    );
    const fix = quickfixes[0];
    assert.ok(fix, "expected a quick-fix CodeAction");
    assert.strictEqual(fix.kind?.value, vscode.CodeActionKind.QuickFix.value, "the fix must be a quickfix kind");
    assert.ok(fix.edit, "the quick-fix must carry a WorkspaceEdit");

    const applied = await vscode.workspace.applyEdit(fix.edit);
    assert.ok(applied, "applying the quick-fix edit must succeed");

    const editedText = document.getText();
    assert.ok(
      editedText.includes("disclose(getW())"),
      `expected the document to now contain "disclose(getW())", got: ${editedText}`,
    );
    assert.ok(
      !editedText.includes("disclose(c = getW())"),
      "the quick-fix must wrap only the minimal expression, never the whole assignment statement",
    );

    // Re-analysis clears the leak once the value is disclosed.
    const clearDeadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
    let stillLeaking = true;
    for (;;) {
      stillLeaking = vscode.languages
        .getDiagnostics(uri)
        .some((d) => d.source === "compact-analyzer" && diagnosticCodeValue(d.code) === "E3100");
      if (!stillLeaking || Date.now() >= clearDeadline) {
        break;
      }
      await delay(200);
    }
    assert.ok(!stillLeaking, "the E3100 leak should clear once disclose() wraps the value");

    // Revert the in-memory buffer to its original fixture content — this only
    // edits the open document (never touches disk), so later scenarios that
    // reopen this same fixture (the toggle test) see the leak unmodified.
    const fullRange = new vscode.Range(document.positionAt(0), document.positionAt(editedText.length));
    const revert = new vscode.WorkspaceEdit();
    revert.replace(uri, fullRange, originalText);
    const reverted = await vscode.workspace.applyEdit(revert);
    assert.ok(reverted, "reverting the quick-fix edit must succeed");
    assert.strictEqual(
      document.getText(),
      originalText,
      "the document must be restored to its original fixture content",
    );

    // Wait for re-analysis of the reverted (once again leaking) text to
    // publish the E3100 diagnostic again, so later scenarios that reopen this
    // fixture (the toggle test) see it in its original, leaking state.
    const republishDeadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
    let leakRestored = false;
    for (;;) {
      leakRestored = vscode.languages
        .getDiagnostics(uri)
        .some((d) => d.source === "compact-analyzer" && diagnosticCodeValue(d.code) === "E3100");
      if (leakRestored || Date.now() >= republishDeadline) {
        break;
      }
      await delay(200);
    }
    assert.ok(leakRestored, "the E3100 leak should be republished once the document is reverted");
  });

  test("scenario 1i: disclosureDiagnostics=\"off\" removes disclosure diagnostics while parse diagnostics remain", async function () {
    this.timeout(60_000);
    assert.strictEqual(api.serverStatus(), "running", "server should be running for the toggle check");

    const leakUri = fixtureUri("disclosure-leak.compact");
    const leakDocument = await vscode.workspace.openTextDocument(leakUri);
    await vscode.window.showTextDocument(leakDocument);
    // The reverted-but-still-open document from scenario 1h must be back to
    // its original, leaking content before this scenario turns the toggle
    // off — otherwise there is nothing for the toggle to remove.
    const preToggleLeak = vscode.languages
      .getDiagnostics(leakUri)
      .find((d) => d.source === "compact-analyzer" && diagnosticCodeValue(d.code) === "E3100");
    assert.ok(preToggleLeak, "expected the E3100 leak to be present before toggling disclosureDiagnostics off");

    // A pure-parse-error fixture (scenario 1's own check): its E0001 is
    // published by the ALWAYS-ON parser path (server.rs `build_native_diagnostics`),
    // never gated by `disclosureDiagnostics` — proving "parse diagnostics remain".
    const invalidUri = fixtureUri("invalid.compact");
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(invalidUri));
    const preToggleParseDiags = await waitForCompactDiagnostic(invalidUri, DIAGNOSTIC_TIMEOUT_MS);
    assert.ok(
      preToggleParseDiags.some((d) => (d.source ?? "").includes("compact-analyzer")),
      "expected invalid.compact's parse diagnostic to be present before the toggle",
    );

    const config = vscode.workspace.getConfiguration("compact-analyzer");
    await config.update("disclosureDiagnostics", "off", vscode.ConfigurationTarget.Workspace);
    // "Changing this setting requires restarting the language server"
    // (package.json markdownDescription) — mirror scenario 2's restart idiom.
    await vscode.commands.executeCommand(RESTART_COMMAND);
    assert.strictEqual(api.serverStatus(), "running", "server should come back up after the toggle restart");

    try {
      const deadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
      let leakGone = false;
      for (;;) {
        leakGone = !vscode.languages
          .getDiagnostics(leakUri)
          .some((d) => d.source === "compact-analyzer" && diagnosticCodeValue(d.code) === "E3100");
        if (leakGone || Date.now() >= deadline) {
          break;
        }
        await delay(200);
      }
      assert.ok(
        leakGone,
        `expected the E3100 disclosure diagnostic to be gone once disclosureDiagnostics="off", got ${JSON.stringify(
          vscode.languages.getDiagnostics(leakUri).map((d) => ({ source: d.source, code: d.code })),
        )}`,
      );

      // Restarting the server clears and re-publishes diagnostics for every
      // reopened document asynchronously, same as any post-restart diagnostic
      // — poll rather than reading a single snapshot.
      const parseDiags = await waitForCompactDiagnostic(invalidUri, DIAGNOSTIC_TIMEOUT_MS);
      assert.ok(
        parseDiags.some((d) => (d.source ?? "").includes("compact-analyzer")),
        `expected invalid.compact's parse diagnostic to remain while disclosure diagnostics are toggled off, got ${JSON.stringify(
          parseDiags.map((d) => ({ source: d.source, code: d.code })),
        )}`,
      );
    } finally {
      // Restore the default so later scenarios (and re-runs) start clean.
      await config.update("disclosureDiagnostics", "all", vscode.ConfigurationTarget.Workspace);
      await vscode.commands.executeCommand(RESTART_COMMAND);
      assert.strictEqual(api.serverStatus(), "running", "server should come back up after restoring the toggle");
    }
  });

  // Task 4 (v2c §8 done-bar): the three type-aware editor features are LSP
  // capabilities the thin `vscode-languageclient` inherits once the server
  // advertises them (Tasks 1-3) — no client code drives them. These tests
  // prove they work in a REAL extension host via `vscode.commands.execute*`,
  // not merely over an LSP integration harness. All three share one fixture,
  // `uxfeatures.compact`:
  //
  //   struct S { a: Field; b: Boolean; }
  //   circuit add(x: Field, y: Field): Field { return x + y; }
  //   export circuit c(s: S): Field { const n = add(1, 2); return s.a + n; }
  //
  // which is verified to compile-accept live via `compact compile --skip-zk
  // --vscode`. Positions below are 0-indexed line/character offsets into line
  // 8 (the `export circuit c(...)` line), computed from the fixture text.

  test("scenario 1c: inlay hints render the const binding's inferred Field type", async function () {
    this.timeout(DIAGNOSTIC_TIMEOUT_MS + 15_000);
    assert.strictEqual(api.serverStatus(), "running", "server should be running for the inlay-hint check");

    const uri = fixtureUri("uxfeatures.compact");
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(document);

    const fullRange = new vscode.Range(new vscode.Position(0, 0), document.lineAt(document.lineCount - 1).range.end);

    const deadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
    let hints: vscode.InlayHint[] = [];
    let hasFieldHint = false;
    for (;;) {
      hints =
        (await vscode.commands.executeCommand<vscode.InlayHint[]>(
          "vscode.executeInlayHintProvider",
          uri,
          fullRange,
        )) ?? [];
      hasFieldHint = hints.some((hint) => normalizeInlayHintLabel(hint.label).includes("Field"));
      if (hasFieldHint || Date.now() >= deadline) {
        break;
      }
      await delay(200);
    }

    assert.ok(
      hasFieldHint,
      `expected an inlay hint whose label contains "Field" (the const n = add(1, 2) binding) within ` +
        `${DIAGNOSTIC_TIMEOUT_MS}ms, got ${JSON.stringify(
          hints.map((hint) => ({ label: normalizeInlayHintLabel(hint.label), position: hint.position })),
        )}`,
    );
  });

  test("scenario 1d: signature help reports the active parameter inside add(1, 2)", async function () {
    this.timeout(DIAGNOSTIC_TIMEOUT_MS + 15_000);
    assert.strictEqual(api.serverStatus(), "running", "server should be running for the signature-help check");

    const uri = fixtureUri("uxfeatures.compact");
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(document);

    // Line 8: `export circuit c(s: S): Field { const n = add(1, 2); return s.a + n; }`
    // Character 46 is the gap between `add(` and the `1` argument — inside the call.
    const positionInsideAddCall = new vscode.Position(8, 46);

    const deadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
    let help: vscode.SignatureHelp | undefined;
    for (;;) {
      help = await vscode.commands.executeCommand<vscode.SignatureHelp>(
        "vscode.executeSignatureHelpProvider",
        uri,
        positionInsideAddCall,
        "(",
      );
      if ((help?.signatures.length ?? 0) >= 1 || Date.now() >= deadline) {
        break;
      }
      await delay(200);
    }

    assert.ok(
      help && help.signatures.length >= 1,
      `expected >=1 signature within ${DIAGNOSTIC_TIMEOUT_MS}ms, got ${JSON.stringify(help)}`,
    );
    assert.strictEqual(
      typeof help?.activeParameter,
      "number",
      `expected activeParameter to be a number, got ${JSON.stringify(help?.activeParameter)}`,
    );
  });

  test("scenario 1e: typed member completion on `s.` offers the struct's fields", async function () {
    this.timeout(DIAGNOSTIC_TIMEOUT_MS + 15_000);
    assert.strictEqual(api.serverStatus(), "running", "server should be running for the member-completion check");

    const uri = fixtureUri("uxfeatures.compact");
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(document);

    // Line 8: `export circuit c(s: S): Field { const n = add(1, 2); return s.a + n; }`
    // Character 62 sits immediately after the `.` in `s.a`.
    const positionAfterDot = new vscode.Position(8, 62);

    const deadline = Date.now() + DIAGNOSTIC_TIMEOUT_MS;
    let labels: string[] = [];
    for (;;) {
      const list = await vscode.commands.executeCommand<vscode.CompletionList>(
        "vscode.executeCompletionItemProvider",
        uri,
        positionAfterDot,
      );
      labels = (list?.items ?? []).map((item) => (typeof item.label === "string" ? item.label : item.label.label));
      if ((labels.includes("a") && labels.includes("b")) || Date.now() >= deadline) {
        break;
      }
      await delay(200);
    }

    assert.ok(
      labels.includes("a") && labels.includes("b"),
      `expected completion items "a" and "b" (struct S's fields) within ${DIAGNOSTIC_TIMEOUT_MS}ms, ` +
        `got ${JSON.stringify(labels)}`,
    );
  });

  test("scenario 2: a nonexistent serverPath degrades to an unavailable session", async function () {
    this.timeout(60_000);

    // Rewrite the configured path to one that cannot exist, then restart. The
    // settings source hard-fails (D5) with no fall-through to PATH or download.
    const config = vscode.workspace.getConfiguration("compact-analyzer");
    await config.update("serverPath", NONEXISTENT_SERVER_PATH, vscode.ConfigurationTarget.Workspace);
    await vscode.commands.executeCommand(RESTART_COMMAND);

    assert.strictEqual(api.serverStatus(), "unavailable", "a nonexistent serverPath must yield an unavailable server");
    assert.strictEqual(api.serverSource(), null, "an unavailable server must report a null source");

    // Give any deferred microtask rejection a beat to surface before asserting.
    await delay(100);
    assert.deepStrictEqual(rejections, [], `no unhandled rejection should escape activation: ${JSON.stringify(rejections.map(String))}`);
  });
});
