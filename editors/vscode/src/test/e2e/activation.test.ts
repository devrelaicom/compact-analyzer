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
