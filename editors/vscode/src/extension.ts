// Extension entry point: acquires the `compact-analyzer` server binary, launches
// a stdio LanguageClient, and manages its lifecycle. As a thin client it holds
// NO analysis logic — every feature is negotiated from the server over LSP, and
// all host-free decision logic lives in the unit-tested `activation-core`. This
// is one of only two modules permitted to import `vscode` (it also owns the
// `vscode-languageclient` import). Activation must NEVER throw: on any failure it
// degrades to a grammar/snippets-only session with one clear message.

import { readFileSync } from "node:fs";
import * as path from "node:path";

import * as vscode from "vscode";
import { LanguageClient, type Executable, type LanguageClientOptions } from "vscode-languageclient/node";

import { acquireServer, type AcquireFailure, type ServerSource } from "./acquire";
import {
  checkServerInfoCompatibility,
  formatAcquireFailureMessage,
  parseServerManifest,
  shouldOfferCoexistenceHint,
} from "./activation-core";
import { configuredServerPath, initializationOptionsFromConfig } from "./config";
import type { ServerManifest } from "./download";
import { PINNED_SERVER_VERSION } from "./version";

/** The stable surface consumed by the Task 10 E2E to assert activation outcomes. */
export interface ExtensionApi {
  serverStatus(): "running" | "unavailable";
  serverSource(): ServerSource | null;
}

const RESTART_COMMAND = "compact-analyzer.restartServer";
const OLD_EXTENSION_ID = "midnightnetwork.compact";
const COEXISTENCE_HINT_FLAG = "compact-analyzer.coexistenceHintShown";
const INSTALL_URL = "https://github.com/devrelaicom/compact-analyzer#installation";

// Module-level lifecycle state: `deactivate` and the restart command both need
// the live client, and the ExtensionApi reflects the latest status/source.
let client: LanguageClient | undefined;
let status: "running" | "unavailable" = "unavailable";
let source: ServerSource | null = null;

// Every lifecycle transition (initial start, restart, deactivate teardown) runs
// through this single serial chain, so overlapping invocations — e.g. two rapid
// "Restart server" clicks, or a restart racing deactivate — can never leave a
// started server orphaned. A rejected op never wedges the chain (the tail is
// caught), while the returned promise still surfaces that op's own outcome.
let lifecycle: Promise<void> = Promise.resolve();
function enqueue(op: () => Promise<void>): Promise<void> {
  const run = lifecycle.then(op, op);
  lifecycle = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}

const describe = (error: unknown): string => (error instanceof Error ? error.message : String(error));

/**
 * Load the bundled server manifest defensively. A missing or malformed file is
 * NOT fatal: it collapses to an empty-artefact manifest (no download possible),
 * so acquisition falls back to the settings path, PATH, and the storage cache.
 */
function loadManifest(context: vscode.ExtensionContext): ServerManifest {
  const fallback: ServerManifest = { version: PINNED_SERVER_VERSION, artifacts: {} };
  try {
    const text = readFileSync(path.join(context.extensionPath, "server-manifest.json"), "utf8");
    return parseServerManifest(text) ?? fallback;
  } catch {
    return fallback;
  }
}

/** Show the single acquire-failure message with a link to the install docs. */
async function showAcquireFailure(failure: AcquireFailure): Promise<void> {
  const choice = await vscode.window.showErrorMessage(formatAcquireFailureMessage(failure), "How to install");
  if (choice === "How to install") {
    void vscode.env.openExternal(vscode.Uri.parse(INSTALL_URL));
  }
}

/**
 * Acquire a server binary and, on success, start a fresh stdio LanguageClient.
 * Fully self-contained: it NEVER throws, sets the module status/source itself,
 * and adopts `client` ONLY after `start()` resolves AND the compatibility gate
 * passes — so a rejected start or an incompatible server leaves `client`
 * undefined and shows exactly one message. Always run via `enqueue`.
 */
async function startServer(
  context: vscode.ExtensionContext,
  manifest: ServerManifest,
  out: vscode.LogOutputChannel,
): Promise<void> {
  // Neutral title: acquire only downloads as a last resort, so the common
  // already-installed path (a settings/PATH/storage probe, no download) must not
  // claim to be "downloading". A real download still surfaces under this same
  // progress notification.
  const result = await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Notification, title: "Starting compact-analyzer…" },
    () =>
      acquireServer({
        configuredPath: configuredServerPath(),
        storageDir: context.globalStorageUri.fsPath,
        manifest,
      }),
  );

  if (!result.ok) {
    status = "unavailable";
    source = null;
    void showAcquireFailure(result);
    return;
  }

  // Launch with NO args: the server serves LSP on stdio and writes its startup
  // line + diagnostics info to stderr, which the client routes to `out`.
  const executable: Executable = { command: result.binaryPath, args: [] };
  const options: LanguageClientOptions = {
    documentSelector: [{ language: "compact" }],
    initializationOptions: initializationOptionsFromConfig(),
    outputChannel: out,
  };
  const lc = new LanguageClient("compact-analyzer", "Compact Analyzer", executable, options);

  // Do NOT adopt `lc` before start() resolves: a binary can pass the pre-spawn
  // --version probe yet still crash/hang on LSP `initialize`, rejecting start().
  // Adopting first would leave a StartFailed client that deactivate cannot stop.
  try {
    await lc.start();
  } catch (error) {
    await lc.dispose().catch(() => {}); // a StartFailed client rejects on dispose
    client = undefined;
    status = "unavailable";
    source = null;
    out.error(`The language server failed to start: ${describe(error)}`);
    void vscode.window.showErrorMessage(
      `Compact Analyzer's language server failed to start. ${describe(error)} ` +
        'Run "Compact Analyzer: Restart Server" once the problem is resolved.',
    );
    return;
  }

  // Belt-and-braces beyond the pre-spawn probe: re-check the negotiated version.
  const check = checkServerInfoCompatibility(lc.initializeResult?.serverInfo, PINNED_SERVER_VERSION);
  if (check.kind === "incompatible") {
    out.error(check.message);
    await lc.stop().catch(() => {}); // never let a teardown rejection escape
    client = undefined; // never adopted; keep it that way
    status = "unavailable";
    source = null;
    void vscode.window.showErrorMessage(check.message);
    return;
  }
  if (check.kind === "unknown") {
    out.warn(
      "The language server did not report a parseable version during initialize; skipping the post-handshake check.",
    );
  }

  // Success: adopt the client only now.
  client = lc;
  status = "running";
  source = result.source;
}

/** Dispose the current client (if any), resetting state. Never throws. */
async function disposeCurrentClient(): Promise<void> {
  const previous = client;
  client = undefined;
  status = "unavailable";
  source = null;
  if (previous) {
    // dispose() fully tears the old client down before a fresh one is built; a
    // StartFailed/already-stopped client rejects, so the rejection is swallowed.
    await previous.dispose().catch(() => {});
  }
}

/** Restart: dispose the current client, RE-acquire (settings may have changed), start afresh. */
async function restartServer(
  context: vscode.ExtensionContext,
  manifest: ServerManifest,
  out: vscode.LogOutputChannel,
): Promise<void> {
  await disposeCurrentClient();
  await startServer(context, manifest, out);
}

/** Nudge (once) that the sunsetted official extension can be uninstalled (OQ10). */
async function maybeOfferCoexistenceHint(context: vscode.ExtensionContext): Promise<void> {
  const present = vscode.extensions.getExtension(OLD_EXTENSION_ID) !== undefined;
  const alreadyHinted = context.globalState.get<boolean>(COEXISTENCE_HINT_FLAG) === true;
  if (!shouldOfferCoexistenceHint(present, alreadyHinted)) {
    return;
  }
  await context.globalState.update(COEXISTENCE_HINT_FLAG, true);
  void vscode.window.showInformationMessage(
    'The official "Compact" extension is still installed. Compact Analyzer is its successor — you can ' +
      "uninstall the old extension to avoid duplicate language features.",
  );
}

export async function activate(context: vscode.ExtensionContext): Promise<ExtensionApi> {
  const api: ExtensionApi = { serverStatus: () => status, serverSource: () => source };

  try {
    const out = vscode.window.createOutputChannel("Compact Analyzer", { log: true });
    context.subscriptions.push(out);
    const manifest = loadManifest(context);

    // Register the restart command and config listener UNCONDITIONALLY and BEFORE
    // the first acquire/start, so they ALWAYS exist regardless of its outcome —
    // making restart the reliable in-session recovery path even after a failed
    // initial start (otherwise a palette "Restart Server" would error with
    // "command not found" and only a window reload could recover).
    context.subscriptions.push(
      vscode.commands.registerCommand(RESTART_COMMAND, () =>
        enqueue(() => restartServer(context, manifest, out)),
      ),
    );

    // G3: the server reads its configuration ONLY at startup and has no
    // didChangeConfiguration handler, so any `compact-analyzer.*` change needs an
    // explicit restart to take effect — the prompt is mandatory, not a courtesy.
    context.subscriptions.push(
      vscode.workspace.onDidChangeConfiguration(async (event) => {
        if (!event.affectsConfiguration("compact-analyzer")) {
          return;
        }
        const choice = await vscode.window.showInformationMessage(
          "Compact Analyzer settings changed. Restart the language server to apply them?",
          "Restart server",
        );
        if (choice === "Restart server") {
          await vscode.commands.executeCommand(RESTART_COMMAND);
        }
      }),
    );

    // Initial start, serialised on the same lifecycle chain. startServer never
    // throws, so a failed acquire/start degrades to "unavailable" and returns.
    await enqueue(() => startServer(context, manifest, out));

    // One-time coexistence nudge, guarded so a globalState rejection can never
    // become an unhandled rejection.
    void maybeOfferCoexistenceHint(context).catch(() => {});
  } catch (error) {
    // Belt-and-braces: nothing above is expected to throw, but activation must
    // never propagate one. Degrade to unavailable with a single message.
    status = "unavailable";
    source = null;
    void vscode.window.showErrorMessage(`Compact Analyzer failed to activate. ${describe(error)}`);
  }

  return api;
}

export function deactivate(): Thenable<void> {
  // Serialised teardown: waits for any in-flight start/restart, then stops the
  // current client. Safe when `client` is undefined (resolves as a no-op). The
  // server has a FAST-EXIT shutdown, so a plain stop needs no timeout.
  return enqueue(async () => {
    const previous = client;
    client = undefined;
    status = "unavailable";
    source = null;
    if (previous) {
      await previous.stop().catch(() => {});
    }
  });
}
