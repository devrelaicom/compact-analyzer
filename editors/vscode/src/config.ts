// Public settings surface for the extension. This is one of the only two
// modules permitted to import the `vscode` module (the layering rule); the
// pure mapping it delegates to lives in the vscode-free `config-core`.
//
// As a thin client, this module carries NO analysis logic: it only reads the
// `compact-analyzer.*` workspace settings and maps them onto the server's
// `initializationOptions` (plus the client-only `serverPath`).

import * as vscode from "vscode";

import { buildInitOptions, readServerPath, type ConfigReader, type InitOptions } from "./config-core";

export type { InitOptions } from "./config-core";

/** Builds a `ConfigReader` backed by the live `compact-analyzer` configuration. */
function configReader(): ConfigReader {
  const config = vscode.workspace.getConfiguration("compact-analyzer");
  return <T>(key: string): T | undefined => config.get<T>(key);
}

/**
 * The server `initializationOptions` derived from the current settings.
 * Config changes require a server restart; the restart listener lands in a
 * later task, so this simply reflects the settings at call time.
 */
export function initializationOptionsFromConfig(): InitOptions {
  return buildInitOptions(configReader());
}

/** The configured client-side `serverPath`, or `undefined` when left blank. */
export function configuredServerPath(): string | undefined {
  return readServerPath(configReader());
}
