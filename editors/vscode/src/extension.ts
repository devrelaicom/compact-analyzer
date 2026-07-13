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
 * Acquire a server binary and, on success, start a fresh stdio LanguageClient,
 * setting the module status/source. The acquire-failure path shows one message
 * and returns (never throws); a thrown `start()` propagates to the caller.
 */
async function startServer(
  context: vscode.ExtensionContext,
  manifest: ServerManifest,
  out: vscode.LogOutputChannel,
): Promise<void> {
  // Wrap the whole acquire so a last-resort download surfaces progress; the
  // common (already-installed) path resolves before the notification lingers.
  const result = await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Notification, title: `Downloading compact-analyzer ${PINNED_SERVER_VERSION}…` },
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
  client = lc;
  await lc.start();

  // Belt-and-braces beyond the pre-spawn probe: re-check the negotiated version.
  const check = checkServerInfoCompatibility(lc.initializeResult?.serverInfo, PINNED_SERVER_VERSION);
  if (check.kind === "incompatible") {
    out.error(check.message);
    await lc.stop();
    client = undefined;
    status = "unavailable";
    source = null;
    void vscode.window.showErrorMessage(check.message);
    return;
  }
  if (check.kind === "unknown") {
    out.warn("The language server did not report its version during initialize; skipping the post-handshake check.");
  }
  status = "running";
  source = result.source;
}

/** Dispose the current client, RE-acquire (settings may have changed), then start afresh. */
async function restartServer(
  context: vscode.ExtensionContext,
  manifest: ServerManifest,
  out: vscode.LogOutputChannel,
): Promise<void> {
  try {
    if (client) {
      const previous = client;
      client = undefined;
      await previous.dispose();
    }
    status = "unavailable";
    source = null;
    await startServer(context, manifest, out);
  } catch (error) {
    status = "unavailable";
    source = null;
    out.error(`Restart failed: ${describe(error)}`);
    void vscode.window.showErrorMessage(`Compact Analyzer: restart failed. ${describe(error)}`);
  }
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

    await startServer(context, manifest, out);

    // The restart command is registered even when acquisition failed, so a user
    // who installs the server afterwards can pick it up without reloading.
    context.subscriptions.push(
      vscode.commands.registerCommand(RESTART_COMMAND, () => restartServer(context, manifest, out)),
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

    void maybeOfferCoexistenceHint(context);
  } catch (error) {
    // Never let activation throw: degrade to unavailable with one message.
    status = "unavailable";
    source = null;
    void vscode.window.showErrorMessage(
      `Compact Analyzer failed to start its language server. ${describe(error)}`,
    );
  }

  return api;
}

export function deactivate(): Thenable<void> | undefined {
  // The server has a FAST-EXIT shutdown, so a plain stop needs no timeout.
  return client?.stop();
}
