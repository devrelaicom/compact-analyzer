/**
 * Pure settings → `initializationOptions` mapping.
 *
 * This module is deliberately vscode-FREE: it takes an injected `ConfigReader`
 * and performs no I/O, so the mapping is unit-testable under plain vitest with
 * a fake reader and no extension host. The vscode-backed public surface lives
 * in `config.ts`, which is one of the two modules permitted to import the
 * `vscode` module (the layering rule); this core stays out of that boundary.
 *
 * As a thin client, this carries NO analysis logic — it only maps the
 * `compact-analyzer.*` workspace settings onto the exact key set the server
 * reads from its `initializationOptions`.
 */

/**
 * The EXACT set of keys the server reads from `initializationOptions` (G1).
 *
 * The server reads ONLY these four keys, so the client sends nothing else. The
 * client-only settings (`serverPath`, `trace.server`) are deliberately absent.
 */
export interface InitOptions {
  importSearchPath?: string[];
  toolchainPath?: string;
  compileOnSave?: boolean;
  formatting?: boolean;
}

/**
 * Reads a single `compact-analyzer.*` setting by its unprefixed key, returning
 * `undefined` when the setting is unset. Injected so the mapping can be driven
 * by a fake reader in tests; `config.ts` supplies a vscode-backed default.
 */
export type ConfigReader = <T>(key: string) => T | undefined;

/**
 * Maps the configured `compact-analyzer.*` settings onto the server's
 * `initializationOptions`, applying the load-bearing omit rules.
 */
export function buildInitOptions(read: ConfigReader): InitOptions {
  const options: InitOptions = {};

  const importSearchPath = read<string[]>("importSearchPath");
  // G2 (load-bearing): an explicitly-sent `importSearchPath: []` tells the
  // server to use NO search path, which SUPPRESSES its `COMPACT_PATH` env
  // fallback. An ABSENT key means "fall back to COMPACT_PATH". So when the
  // setting is empty we omit the key entirely rather than helpfully sending
  // `[]`.
  if (importSearchPath !== undefined && importSearchPath.length > 0) {
    options.importSearchPath = importSearchPath;
  }

  const toolchainPath = read<string>("toolchainPath");
  // Omitted when blank so the server keeps its own toolchain discovery; a
  // non-empty value hands the server an explicit file-or-directory path.
  if (toolchainPath !== undefined && toolchainPath !== "") {
    options.toolchainPath = toolchainPath;
  }

  // `compileOnSave`/`formatting` are sent EXPLICITLY (even at their default
  // `true`) to decouple the client's defaults from the server's own defaults.
  // `??` fills only the unset case; a configured `false` is preserved.
  options.compileOnSave = read<boolean>("compileOnSave") ?? true;
  options.formatting = read<boolean>("formatting") ?? true;

  return options;
}

/**
 * Reads the client-only `serverPath` setting, collapsing a blank/unset value to
 * `undefined`. This drives server acquisition and is NEVER sent to the server.
 */
export function readServerPath(read: ConfigReader): string | undefined {
  const serverPath = read<string>("serverPath");
  return serverPath !== undefined && serverPath !== "" ? serverPath : undefined;
}
