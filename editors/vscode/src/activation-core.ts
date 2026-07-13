/**
 * Pure, vscode-FREE decision logic pulled out of `activate()`.
 *
 * `extension.ts` is one of only two modules permitted to import the `vscode`
 * module and is validated by the Task 10 E2E rather than by unit tests. To keep
 * that file thin and to make the fiddly, branch-heavy bits testable under plain
 * vitest with no extension host, every piece of logic that does NOT need the
 * host lives here: defensive manifest parsing, the post-handshake serverInfo
 * compatibility check, the coexistence-hint decision, and the acquire-failure
 * message formatter.
 *
 * Nothing here performs I/O or throws: callers feed it strings/plain objects and
 * receive plain data back, mirroring the no-throw posture the surrounding
 * activation path relies on.
 */

import type { ServerManifest } from "./download";
import { isCompatible } from "./version";

// ---------------------------------------------------------------------------
// Manifest parsing (defensive)
// ---------------------------------------------------------------------------

/**
 * Parse the bundled `server-manifest.json` text into a `ServerManifest`, or
 * return `null` when the text is missing, is not JSON, or does not have the
 * expected top-level shape.
 *
 * The check is deliberately shallow — `version` must be a non-empty string and
 * `artifacts` must be a plain object — because `download.ts` re-validates each
 * artifact entry's `name`/`sha256` before it is ever used. A `null` return tells
 * the caller "treat this as no download possible" (acquisition then relies on
 * the settings path, PATH, and the storage cache only).
 */
export function parseServerManifest(text: string): ServerManifest | null {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof raw !== "object" || raw === null) {
    return null;
  }
  const { version, artifacts } = raw as Record<string, unknown>;
  if (typeof version !== "string" || version === "") {
    return null;
  }
  // An array is typeof "object" too, so it is excluded explicitly.
  if (typeof artifacts !== "object" || artifacts === null || Array.isArray(artifacts)) {
    return null;
  }
  return { version, artifacts: artifacts as ServerManifest["artifacts"] };
}

// ---------------------------------------------------------------------------
// Post-handshake serverInfo compatibility check
// ---------------------------------------------------------------------------

/**
 * The three possible outcomes of the belt-and-braces `serverInfo.version` check
 * performed AFTER the LSP initialize handshake completes.
 *
 * `unknown` is not a failure: the pre-spawn `--version` probe in `acquire.ts`
 * already gated compatibility, so a server that simply omits its version from
 * the initialize result is accepted (the caller logs a note). Only a version
 * that is present AND incompatible is a hard `incompatible` result.
 */
export type ServerInfoCheck =
  | { kind: "compatible"; version: string }
  | { kind: "incompatible"; version: string; message: string }
  | { kind: "unknown" };

/**
 * Compare the server-reported `serverInfo.version` from the initialize result
 * against the extension's pinned version. Never throws; tolerates a missing
 * `serverInfo` or a missing/blank `version`.
 */
export function checkServerInfoCompatibility(
  serverInfo: { name?: string; version?: string } | undefined,
  pinned: string,
): ServerInfoCheck {
  const version = serverInfo?.version;
  if (typeof version !== "string" || version === "") {
    return { kind: "unknown" };
  }
  if (!isCompatible(version, pinned)) {
    return {
      kind: "incompatible",
      version,
      message:
        `The connected compact-analyzer language server reported version ${version}, which is ` +
        `incompatible with this extension's pinned version ${pinned}. The server has been stopped. ` +
        'Update the server (or the "compact-analyzer.serverPath" setting) to a compatible build, then ' +
        'run "Compact Analyzer: Restart Server".',
    };
  }
  return { kind: "compatible", version };
}

// ---------------------------------------------------------------------------
// Coexistence hint (OQ10)
// ---------------------------------------------------------------------------

/**
 * Decide whether to surface the one-time hint that the sunsetted official
 * Compact extension can be uninstalled: only when it is still installed AND the
 * hint has not been shown before. Never hard-blocks — this is only a nudge.
 */
export function shouldOfferCoexistenceHint(
  oldExtensionPresent: boolean,
  alreadyHinted: boolean,
): boolean {
  return oldExtensionPresent && !alreadyHinted;
}

// ---------------------------------------------------------------------------
// Acquire-failure message
// ---------------------------------------------------------------------------

/**
 * Build the single user-facing message shown when server acquisition fails,
 * combining the machine `reason` with the actionable `userGuidance` that
 * `acquire.ts` already tailored to the failure.
 */
export function formatAcquireFailureMessage(failure: {
  reason: string;
  userGuidance: string;
}): string {
  return `Compact Analyzer could not start its language server. ${failure.reason} ${failure.userGuidance}`;
}
