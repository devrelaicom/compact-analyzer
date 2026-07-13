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

  // Every read is shape-guarded before use. This module runs inside
  // `activate()`, so it must NEVER throw and must NEVER forward a wrong-typed
  // value: a mistyped setting (e.g. `null`, or a bare string where an array is
  // expected) is treated as absent rather than crashing activation or sending
  // the server garbage. This mirrors the server's own tolerant
  // `as_array()`/`as_str()`/`as_bool()` posture and keeps us robust even if VS
  // Code's schema validation is bypassed.

  const importSearchPath: unknown = read("importSearchPath");
  // G2 (load-bearing): an explicitly-sent `importSearchPath: []` tells the
  // server to use NO search path, which SUPPRESSES its `COMPACT_PATH` env
  // fallback. An ABSENT key means "fall back to COMPACT_PATH". So we omit the
  // key entirely for undefined/null/non-array/empty rather than helpfully
  // sending `[]`. Non-string members are dropped, mirroring the server's
  // `filter_map(as_str)`.
  if (Array.isArray(importSearchPath)) {
    const paths = importSearchPath.filter((entry): entry is string => typeof entry === "string");
    if (paths.length > 0) {
      options.importSearchPath = paths;
    }
  }

  const toolchainPath: unknown = read("toolchainPath");
  // Omitted when blank/mistyped so the server keeps its own toolchain
  // discovery; a non-empty string hands it an explicit file-or-directory path.
  if (typeof toolchainPath === "string" && toolchainPath !== "") {
    options.toolchainPath = toolchainPath;
  }

  // `compileOnSave`/`formatting` are sent EXPLICITLY (even at their default
  // `true`) to decouple the client's defaults from the server's own defaults.
  // A configured `false` is preserved; anything non-boolean (unset or
  // mistyped) defaults to `true`.
  const compileOnSave: unknown = read("compileOnSave");
  options.compileOnSave = typeof compileOnSave === "boolean" ? compileOnSave : true;
  const formatting: unknown = read("formatting");
  options.formatting = typeof formatting === "boolean" ? formatting : true;

  return options;
}

/**
 * Reads the client-only `serverPath` setting, collapsing a blank/unset value to
 * `undefined`. This drives server acquisition and is NEVER sent to the server.
 */
export function readServerPath(read: ConfigReader): string | undefined {
  // Shape-guarded like the rest: a mistyped `serverPath` reads back as `undefined`
  // rather than propagating a non-string into server acquisition.
  const serverPath: unknown = read("serverPath");
  return typeof serverPath === "string" && serverPath !== "" ? serverPath : undefined;
}
