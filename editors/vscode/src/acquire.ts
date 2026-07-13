/**
 * Server acquisition orchestration.
 *
 * Decides which `compact-analyzer` server binary the extension should launch,
 * trying candidates in a milestone-fixed order and validating every one with a
 * `--version` handshake before accepting it:
 *
 *   settings path  ->  PATH  ->  storage cache  ->  pinned download
 *
 * This module is deliberately PURE Node: it never imports `vscode`. Every
 * external effect is either injected (`env`, `exec`, `fetchImpl`) or performed
 * through explicit `node:fs`/`node:path`/`node:child_process` calls, so it is
 * unit-testable with no extension host.
 *
 * It NEVER throws: `acquireServer` always resolves to either `Acquired` or
 * `AcquireFailure`. Callers render `reason`/`userGuidance` themselves.
 *
 * Departure D5 — respect user intent: an explicitly-configured `serverPath`
 * that is missing, unrunnable, or incompatible is a HARD failure. We never
 * silently fall through to PATH or a download when the user named a path.
 */

import { spawn, type ChildProcess } from "node:child_process";
import { statSync } from "node:fs";
import * as path from "node:path";

import {
  DownloadError,
  downloadAndInstall,
  type ServerManifest,
} from "./download";
import { currentTarget } from "./platform";
import {
  PINNED_SERVER_VERSION,
  isCompatible,
  parseServerVersion,
} from "./version";

// ---------------------------------------------------------------------------
// Public shapes (implemented exactly as the milestone interface prescribes).
// ---------------------------------------------------------------------------

export type ServerSource = "settings" | "path" | "storage" | "downloaded";

export type Acquired = {
  ok: true;
  source: ServerSource;
  binaryPath: string;
  version: string;
};

export type AcquireFailure = {
  ok: false;
  reason: string;
  userGuidance: string;
};

/**
 * Probe a candidate binary for its version. Resolves `{ ok, stdout }` — never
 * rejects. `ok` is false when the binary could not be run, timed out, or exited
 * non-zero; `stdout` is the collected standard output (empty on timeout).
 */
export type ExecProbe = (
  binary: string,
  args: string[],
  timeoutMs: number,
) => Promise<{ ok: boolean; stdout: string }>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/** Hard cap for a single `--version` handshake (milestone-fixed at 2s). */
const PROBE_TIMEOUT_MS = 2000;

/** Recovery guidance for platform/validation failures that carry no DownloadError. */
const MANUAL_INSTALL_GUIDANCE =
  'Install the compact-analyzer server manually and point the extension at it with the ' +
  '"compact-analyzer.serverPath" setting. Release archives and installers are available ' +
  "from the GitHub Releases page (https://github.com/devrelaicom/compact-analyzer/releases).";

/** Recovery guidance when the explicitly-configured server path is unusable. */
const CONFIGURED_PATH_GUIDANCE =
  'Set "compact-analyzer.serverPath" to a compact-analyzer binary compatible with version ' +
  `${PINNED_SERVER_VERSION}, or remove the setting to let the extension download and manage ` +
  "the pinned server automatically.";

// ---------------------------------------------------------------------------
// Validation of a single candidate
// ---------------------------------------------------------------------------

/** The outcome of probing one candidate binary. */
type Validation =
  | { status: "ok"; version: string }
  | { status: "unrunnable" }
  | { status: "unparsable" }
  | { status: "incompatible"; version: string };

/**
 * Run the `--version` handshake against `binary` and classify the result. This
 * helper never throws: an injected `exec` that rejects is treated as an
 * unrunnable candidate, keeping `acquireServer`'s no-throw guarantee intact.
 */
async function probeVersion(
  exec: ExecProbe,
  binary: string,
): Promise<Validation> {
  let outcome: { ok: boolean; stdout: string };
  try {
    outcome = await exec(binary, ["--version"], PROBE_TIMEOUT_MS);
  } catch {
    return { status: "unrunnable" };
  }
  if (!outcome.ok) {
    return { status: "unrunnable" };
  }
  const version = parseServerVersion(outcome.stdout);
  if (version === null) {
    return { status: "unparsable" };
  }
  if (!isCompatible(version)) {
    return { status: "incompatible", version };
  }
  return { status: "ok", version };
}

/** Render a non-ok validation as a short diagnostic phrase for logs/reasons. */
function describeValidation(v: Validation): string {
  switch (v.status) {
    case "unrunnable":
      return "could not be run (missing or not executable)";
    case "unparsable":
      return 'did not report a recognisable "compact-analyzer <version>" string';
    case "incompatible":
      return `reported incompatible version ${v.version} (requires ${PINNED_SERVER_VERSION})`;
    case "ok":
      return `reported compatible version ${v.version}`;
  }
}

// ---------------------------------------------------------------------------
// Platform-derived probing helpers
// ---------------------------------------------------------------------------

/**
 * The OS binary basename, derived from the platform rather than from
 * `currentTarget()` — a user on an unsupported architecture who placed their
 * own binary on PATH must still be discoverable.
 */
function osBinaryBasename(platform: NodeJS.Platform): string {
  return platform === "win32" ? "compact-analyzer.exe" : "compact-analyzer";
}

/**
 * Candidate filenames to probe inside each PATH directory. On Unix this is just
 * the basename. On Windows the canonical name is `compact-analyzer.exe`, but we
 * also honour `PATHEXT` over the `compact-analyzer` stem so a differently
 * suffixed executable on PATH is still found.
 */
function candidateNames(
  platform: NodeJS.Platform,
  basename: string,
  env: NodeJS.ProcessEnv,
): string[] {
  if (platform !== "win32") {
    return [basename];
  }
  const names = new Set<string>([basename]);
  const pathext = (env.PATHEXT ?? ".COM;.EXE;.BAT;.CMD")
    .split(";")
    .map((ext) => ext.trim())
    .filter((ext) => ext.length > 0);
  for (const ext of pathext) {
    names.add(`compact-analyzer${ext.toLowerCase()}`);
  }
  return [...names];
}

/** True if `candidate` exists and is a regular file (symlinks are followed). */
function isFile(candidate: string): boolean {
  try {
    return statSync(candidate).isFile();
  } catch {
    return false;
  }
}

/**
 * Resolve every existing on-PATH candidate binary. `env.PATH` is split on the
 * OS delimiter; each directory is probed for each candidate filename.
 */
function pathCandidates(
  platform: NodeJS.Platform,
  env: NodeJS.ProcessEnv,
  basename: string,
): string[] {
  const dirs = (env.PATH ?? "")
    .split(path.delimiter)
    .filter((dir) => dir.length > 0);
  const names = candidateNames(platform, basename, env);
  const hits: string[] = [];
  for (const dir of dirs) {
    for (const name of names) {
      const candidate = path.join(dir, name);
      if (isFile(candidate)) {
        hits.push(candidate);
      }
    }
  }
  return hits;
}

// ---------------------------------------------------------------------------
// Result builders
// ---------------------------------------------------------------------------

function acquired(
  source: ServerSource,
  binaryPath: string,
  version: string,
): Acquired {
  return { ok: true, source, binaryPath, version };
}

function describeError(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Append collected fall-through breadcrumbs to a terminal failure reason. */
function withNotes(reason: string, notes: string[]): string {
  return notes.length === 0
    ? reason
    : `${reason} (Prior candidates: ${notes.join("; ")}.)`;
}

/** Build the hard failure for an unusable explicitly-configured server path. */
function configuredPathFailure(
  configured: string,
  v: Validation,
): AcquireFailure {
  const detail =
    v.status === "incompatible"
      ? `reports version ${v.version}, which is incompatible with the required version ${PINNED_SERVER_VERSION}`
      : v.status === "unparsable"
        ? 'did not report a recognisable "compact-analyzer <version>" string'
        : "could not be run (it is missing or not executable)";
  return {
    ok: false,
    reason:
      `The server at the configured "compact-analyzer.serverPath" (${configured}) ${detail}. ` +
      'Update the "compact-analyzer.serverPath" setting to point at a compatible binary, or clear ' +
      "it to let the extension download the pinned server.",
    userGuidance: CONFIGURED_PATH_GUIDANCE,
  };
}

// ---------------------------------------------------------------------------
// Default ExecProbe — a hard-timeout, always-killing spawn wrapper
// ---------------------------------------------------------------------------

/**
 * The production `ExecProbe`. Spawns `binary` with `args`, collects stdout, and
 * enforces `timeoutMs` with a hard kill of the child — mirroring the server's
 * M4 hand-rolled-poll precedent. It NEVER rejects:
 *
 *   - a spawn error (e.g. ENOENT for a missing binary) resolves `{ok:false, stdout:""}`;
 *   - a timeout kills the child and resolves `{ok:false, stdout:""}`;
 *   - otherwise it resolves `{ok: exitCode === 0, stdout}` once the child closes.
 */
export const defaultExecProbe: ExecProbe = (binary, args, timeoutMs) =>
  new Promise((resolve) => {
    let child: ChildProcess;
    try {
      child = spawn(binary, args, { stdio: ["ignore", "pipe", "ignore"] });
    } catch {
      // A synchronous spawn failure is rare but possible — treat as unrunnable.
      resolve({ ok: false, stdout: "" });
      return;
    }

    let settled = false;
    let stdout = "";

    // Hoisted so the timer callback (created below) can reference it; only ever
    // invoked asynchronously, by which point `timer` is initialised.
    function finish(result: { ok: boolean; stdout: string }): void {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timer);
      resolve(result);
    }

    const timer = setTimeout(() => {
      // Hard timeout: kill the child and report the candidate as not runnable.
      // Any stdout collected so far is deliberately discarded.
      child.kill("SIGKILL");
      finish({ ok: false, stdout: "" });
    }, timeoutMs);
    // Never let this timer keep the event loop alive on its own.
    timer.unref();

    child.stdout?.on("data", (chunk: Buffer) => {
      stdout += chunk.toString("utf8");
    });
    child.on("error", () => {
      // e.g. ENOENT — the binary does not exist. Resolve not-ok, never throw.
      finish({ ok: false, stdout: "" });
    });
    child.on("close", (code) => {
      finish({ ok: code === 0, stdout });
    });
  });

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

/**
 * Resolve a usable `compact-analyzer` server binary, trying settings, PATH, the
 * storage cache, then a pinned download in that order and validating each with
 * a `--version` handshake. Always resolves — never throws.
 */
export async function acquireServer(deps: {
  configuredPath: string | undefined;
  storageDir: string;
  manifest: ServerManifest;
  env?: NodeJS.ProcessEnv;
  exec?: ExecProbe;
  fetchImpl?: typeof fetch;
}): Promise<Acquired | AcquireFailure> {
  const env = deps.env ?? process.env;
  const exec = deps.exec ?? defaultExecProbe;
  const platform = process.platform;
  const basename = osBinaryBasename(platform);
  // Breadcrumbs from soft misses (PATH/storage), folded into a terminal failure
  // reason if acquisition ultimately fails; discarded on success.
  const notes: string[] = [];

  try {
    // 1) Explicit settings path (D5). A blank/whitespace value counts as unset.
    const configured = deps.configuredPath?.trim();
    if (configured) {
      const v = await probeVersion(exec, configured);
      if (v.status === "ok") {
        return acquired("settings", configured, v.version);
      }
      // Respect user intent: hard failure, no fall-through to PATH/download.
      return configuredPathFailure(configured, v);
    }

    // 2) PATH — first compatible hit wins; misses log and fall through.
    for (const candidate of pathCandidates(platform, env, basename)) {
      const v = await probeVersion(exec, candidate);
      if (v.status === "ok") {
        return acquired("path", candidate, v.version);
      }
      notes.push(`PATH candidate ${candidate} ${describeValidation(v)}`);
    }

    // 3) Storage cache — exactly where a prior download landed the binary.
    const cachePath = path.join(deps.storageDir, PINNED_SERVER_VERSION, basename);
    if (isFile(cachePath)) {
      const v = await probeVersion(exec, cachePath);
      if (v.status === "ok") {
        return acquired("storage", cachePath, v.version);
      }
      notes.push(`cached server ${cachePath} ${describeValidation(v)}`);
    }

    // 4) Download the pinned artefact — but only for a platform we can serve.
    const target = currentTarget();
    if (target === null) {
      return {
        ok: false,
        reason: withNotes(
          "No compatible compact-analyzer server was found on the configured path, on PATH, " +
            `or in the extension cache, and this platform (${platform}/${process.arch}) has no ` +
            "downloadable server build.",
          notes,
        ),
        userGuidance: MANUAL_INSTALL_GUIDANCE,
      };
    }

    let installedPath: string;
    try {
      installedPath = await downloadAndInstall({
        manifest: deps.manifest,
        target,
        destDir: deps.storageDir,
        fetchImpl: deps.fetchImpl,
      });
    } catch (cause) {
      if (cause instanceof DownloadError) {
        return {
          ok: false,
          reason: withNotes(cause.message, notes),
          userGuidance: cause.userGuidance,
        };
      }
      return {
        ok: false,
        reason: withNotes(
          `The server download failed: ${describeError(cause)}`,
          notes,
        ),
        userGuidance: MANUAL_INSTALL_GUIDANCE,
      };
    }

    // Re-validate the freshly-downloaded binary via the same handshake.
    const v = await probeVersion(exec, installedPath);
    if (v.status === "ok") {
      return acquired("downloaded", installedPath, v.version);
    }
    return {
      ok: false,
      reason: `The server was downloaded to "${installedPath}" but failed the version handshake: it ${describeValidation(v)}.`,
      userGuidance: MANUAL_INSTALL_GUIDANCE,
    };
  } catch (cause) {
    // Belt and braces: acquireServer must NEVER throw, whatever an injected
    // dependency or the filesystem does.
    return {
      ok: false,
      reason: `Unexpected error while acquiring the compact-analyzer server: ${describeError(cause)}`,
      userGuidance: MANUAL_INSTALL_GUIDANCE,
    };
  }
}
