/**
 * Download, checksum-verify, and install the pinned `compact-analyzer` server
 * binary into the extension's storage directory.
 *
 * This module is the security trust anchor of the extension: it is the only
 * code that fetches an executable from the network and places it on disk.
 * Two invariants are non-negotiable and are enforced by the ordering below:
 *
 *   1. The sha256 of the RAW ARCHIVE bytes is verified BEFORE the archive is
 *      decompressed, extracted, or written anywhere. Unverified bytes never
 *      reach `zlib` or the filesystem.
 *   2. The final binary is placed via a temp file on the same filesystem
 *      followed by an atomic `rename`. A failed or mismatched download leaves
 *      NOTHING at the final binary path.
 *
 * It is deliberately a PURE Node module — it never imports `vscode`. Every
 * external effect goes through an injected `fetchImpl` (defaulting to the
 * global `fetch`) or through explicit `node:fs`/`node:crypto`/`node:zlib`
 * calls, so it is unit-testable with no extension host.
 */

import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import {
  chmodSync,
  mkdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import * as path from "node:path";
import { gunzipSync } from "node:zlib";

import type { PlatformTarget } from "./platform";

/**
 * The server-artifact manifest baked into the VSIX at release time (Task 11).
 *
 * `version` matches `PINNED_SERVER_VERSION`; `artifacts` maps a Rust target
 * triple to the release archive's filename and the sha256 of that archive.
 */
export interface ServerManifest {
  version: string;
  artifacts: Record<string /* rustTriple */, { name: string; sha256: string }>;
}

/**
 * An error raised anywhere in the download/verify/install pipeline. Carries
 * `userGuidance` naming the manual-install alternatives, so callers can show
 * an actionable message without re-deriving the recovery paths.
 */
export class DownloadError extends Error {
  readonly userGuidance: string;

  constructor(
    message: string,
    userGuidance: string,
    options?: { cause?: unknown },
  ) {
    super(message, options);
    this.name = "DownloadError";
    this.userGuidance = userGuidance;
  }
}

/**
 * The default GitHub Releases download base. The release tag is `v{version}`
 * (the real dist tag format), and the direct `releases/download/<tag>/<name>`
 * URL is preferred over the API: it is unmetered and 302-redirects to the CDN,
 * which the default `fetch` follows transparently.
 */
function defaultBaseUrl(version: string): string {
  return `https://github.com/devrelaicom/compact-analyzer/releases/download/v${version}/`;
}

/**
 * Recovery guidance attached to every `DownloadError`. Names each manual
 * alternative to the automatic download so the user is never stuck: the
 * Homebrew tap, the shell/PowerShell installers, the GitHub Releases page, and
 * the `compact-analyzer.serverPath` setting that points the extension at an
 * already-installed binary.
 */
const MANUAL_INSTALL_GUIDANCE =
  "Install the server manually instead: `brew install aaronbassett/homebrew-tap/compact-analyzer`, " +
  "run the shell installer (compact-analyzer-installer.sh) or PowerShell installer " +
  "(compact-analyzer-installer.ps1), or download an archive from the GitHub Releases page " +
  "(https://github.com/devrelaicom/compact-analyzer/releases). Then point the extension at the " +
  "binary with the `compact-analyzer.serverPath` setting.";

// ---------------------------------------------------------------------------
// sha256 verification
// ---------------------------------------------------------------------------

/**
 * Verify that `data` hashes to `expectedHex` under sha256.
 *
 * The comparison is over the FULL 32-byte digest — never a truncated or prefix
 * compare — and is case-insensitive on the expected hex. A malformed expected
 * value (not exactly 64 hex characters) is rejected outright. The final
 * comparison uses `timingSafeEqual` for a constant-time-ish check.
 */
export function verifySha256(data: Uint8Array, expectedHex: string): boolean {
  // Defensive: a malformed baked manifest could supply a non-string here
  // (e.g. a number or `undefined`). Fail closed rather than throwing a raw
  // `TypeError` from `.trim()`.
  if (typeof expectedHex !== "string") {
    return false;
  }
  const expected = expectedHex.trim().toLowerCase();
  // Anything that is not a full-length lowercased hex digest cannot match; the
  // regex both normalises and forecloses truncated/prefix values.
  if (!/^[0-9a-f]{64}$/.test(expected)) {
    return false;
  }
  const actualDigest = createHash("sha256").update(data).digest();
  const expectedDigest = Buffer.from(expected, "hex");
  // Both buffers are exactly 32 bytes here, so `timingSafeEqual` never throws.
  return timingSafeEqual(actualDigest, expectedDigest);
}

// ---------------------------------------------------------------------------
// Minimal tar reader (ustar, regular files only)
// ---------------------------------------------------------------------------

interface TarEntry {
  name: string;
  data: Uint8Array;
}

function readString(buf: Uint8Array, off: number, len: number): string {
  const slice = buf.subarray(off, off + len);
  const end = slice.indexOf(0);
  return new TextDecoder().decode(end === -1 ? slice : slice.subarray(0, end));
}

/**
 * Iterate the regular-file entries of an uncompressed tar buffer.
 *
 * Non-regular entries (directories, symlinks, and pax/GNU extended headers)
 * are skipped but their blocks are still stepped over, so a well-formed archive
 * that interleaves such entries is traversed correctly. Iteration stops at the
 * end-of-archive zero block or when fewer than 512 bytes remain.
 */
export function* tarEntries(buf: Uint8Array): Generator<TarEntry> {
  let off = 0;
  while (off + 512 <= buf.length) {
    const header = buf.subarray(off, off + 512);
    if (header.every((b) => b === 0)) break; // end-of-archive
    const name = readString(header, 0, 100);
    const size = Number.parseInt(readString(header, 124, 12).trim() || "0", 8);
    const typeflag = header[156];
    const prefix = readString(header, 345, 155); // ustar long-name prefix
    const full = prefix ? `${prefix}/${name}` : name;
    if (typeflag === 0x30 || typeflag === 0) {
      // '0' or NUL = regular file
      yield { name: full, data: buf.subarray(off + 512, off + 512 + size) };
    }
    off += 512 + Math.ceil(size / 512) * 512;
  }
}

// ---------------------------------------------------------------------------
// Extraction (the localised format boundary)
// ---------------------------------------------------------------------------

/**
 * Decompress a `.tar.gz` archive and return the bytes of the server binary
 * within it.
 *
 * Keeping the ".tar.gz containing a nested binary" decision inside this one
 * function localises the format choice. It performs NO filesystem writes: the
 * caller verifies the archive's sha256 before invoking it, and a write-free
 * extractor makes "never write unverified bytes" a structural property rather
 * than merely an ordering convention.
 *
 * The binary is located by matching the POSIX basename of each tar entry
 * against `binaryName`, NOT by assuming it is the first/top-level entry: dist
 * archives nest the binary one directory deep under `compact-analyzer-<triple>/`
 * alongside README/LICENCE files. Throws `DownloadError` if the bytes are not a
 * valid gzip stream or if no entry's basename matches.
 */
export function extractArchive(
  archiveBytes: Uint8Array,
  binaryName: string,
): Uint8Array {
  let tar: Uint8Array;
  try {
    tar = gunzipSync(archiveBytes);
  } catch (cause) {
    throw new DownloadError(
      "Failed to decompress the downloaded archive: it is not a valid gzip stream.",
      MANUAL_INSTALL_GUIDANCE,
      { cause },
    );
  }

  for (const entry of tarEntries(tar)) {
    if (path.posix.basename(entry.name) === binaryName) {
      // Copy out of the tar buffer so the returned array does not retain a view
      // onto the whole (potentially large) decompressed archive.
      return Uint8Array.from(entry.data);
    }
  }

  throw new DownloadError(
    `The downloaded archive did not contain the expected binary "${binaryName}".`,
    MANUAL_INSTALL_GUIDANCE,
  );
}

// ---------------------------------------------------------------------------
// Download + verify + install
// ---------------------------------------------------------------------------

function describeError(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/**
 * Download the pinned artifact for `target`, verify its sha256 against the
 * manifest, extract the nested binary, and install it atomically to
 * `<destDir>/<version>/<binaryName>`. Resolves to the absolute binary path.
 *
 * Throws `DownloadError` (with `userGuidance`) on any failure, and never leaves
 * a partial binary at the final path.
 */
export async function downloadAndInstall(opts: {
  manifest: ServerManifest;
  target: PlatformTarget;
  destDir: string;
  baseUrl?: string;
  fetchImpl?: typeof fetch;
}): Promise<string> {
  const { manifest, target, destDir } = opts;
  const fetchImpl = opts.fetchImpl ?? fetch;
  const baseUrl = opts.baseUrl ?? defaultBaseUrl(manifest.version);

  // (d) Resolve the artifact for this triple BEFORE any network activity.
  const artifact = manifest.artifacts[target.rustTriple];
  if (!artifact) {
    throw new DownloadError(
      `The server manifest (version ${manifest.version}) has no artifact for target "${target.rustTriple}".`,
      MANUAL_INSTALL_GUIDANCE,
    );
  }
  // Validate the baked entry's shape. A corrupt manifest (wrong types) must
  // yield a clean, user-actionable error, never a raw crash mid-pipeline; the
  // checks are fail-closed, so we never proceed to fetch/extract/write.
  if (
    typeof artifact.name !== "string" ||
    artifact.name.length === 0 ||
    typeof artifact.sha256 !== "string"
  ) {
    throw new DownloadError(
      `The server manifest entry for target "${target.rustTriple}" is malformed: expected a non-empty "name" and a "sha256" string.`,
      MANUAL_INSTALL_GUIDANCE,
    );
  }

  // Defence-in-depth: refuse anything that is not HTTPS before fetching. The
  // default base is HTTPS; this guards a caller that overrides `baseUrl`.
  let parsedBase: URL;
  try {
    parsedBase = new URL(baseUrl);
  } catch (cause) {
    throw new DownloadError(
      `Malformed download base URL: ${baseUrl}`,
      MANUAL_INSTALL_GUIDANCE,
      { cause },
    );
  }
  if (parsedBase.protocol !== "https:") {
    throw new DownloadError(
      `Refusing to download the server over a non-HTTPS URL: ${baseUrl}`,
      MANUAL_INSTALL_GUIDANCE,
    );
  }

  const url = `${baseUrl}${artifact.name}`;

  // Fetch. The default global `fetch` follows the 302 redirect from the
  // release-asset URL to the CDN transparently.
  let response: Response;
  try {
    response = await fetchImpl(url);
  } catch (cause) {
    throw new DownloadError(
      `Could not reach ${url}: ${describeError(cause)}`,
      MANUAL_INSTALL_GUIDANCE,
      { cause },
    );
  }
  if (!response.ok) {
    throw new DownloadError(
      `Download of ${url} failed with HTTP ${response.status}${
        response.statusText ? ` ${response.statusText}` : ""
      }.`,
      MANUAL_INSTALL_GUIDANCE,
    );
  }

  let archiveBytes: Uint8Array;
  try {
    archiveBytes = new Uint8Array(await response.arrayBuffer());
  } catch (cause) {
    throw new DownloadError(
      `Failed to read the response body from ${url}: ${describeError(cause)}`,
      MANUAL_INSTALL_GUIDANCE,
      { cause },
    );
  }

  // SECURITY: verify the RAW ARCHIVE bytes before decompressing, extracting, or
  // touching the filesystem. On mismatch we throw immediately — nothing is
  // gunzipped and nothing is written.
  if (!verifySha256(archiveBytes, artifact.sha256)) {
    throw new DownloadError(
      `Checksum mismatch for ${artifact.name}: the downloaded archive does not match the ` +
        "expected sha256 in the manifest. The download may be corrupt or tampered with.",
      MANUAL_INSTALL_GUIDANCE,
    );
  }

  // Only now, on a verified archive, decompress and locate the binary.
  const binaryBytes = extractArchive(archiveBytes, target.binaryName);

  // Defence-in-depth: refuse a binary name that contains a path separator, so
  // the install can only ever write inside `destDir` regardless of caller
  // discipline. Not reachable via the current platform.ts, but this keeps the
  // write-inside-destDir safety independent of it.
  if (path.basename(target.binaryName) !== target.binaryName) {
    throw new DownloadError(
      `Refusing to install: binary name "${target.binaryName}" contains a path separator.`,
      MANUAL_INSTALL_GUIDANCE,
    );
  }

  // Atomic install: write to a uniquely-named temp file inside the version dir
  // (same filesystem, so `rename` is atomic), set the executable bit on Unix,
  // then rename into place. The whole block — directory creation included — is
  // guarded so ANY filesystem failure (e.g. EACCES/ENOSPC on a read-only or
  // full storage dir) surfaces as a `DownloadError` carrying the manual-install
  // guidance, and every throw path cleans up the temp file. The final path is
  // therefore either the fully-installed binary or nothing.
  const versionDir = path.join(destDir, manifest.version);
  const finalPath = path.join(versionDir, target.binaryName);
  const tmpPath = path.join(
    versionDir,
    `.${target.binaryName}.${process.pid}.${randomBytes(6).toString("hex")}.tmp`,
  );

  try {
    mkdirSync(versionDir, { recursive: true });
    writeFileSync(tmpPath, binaryBytes, { mode: 0o755 });
    // V9: make the binary executable on Unix; skip on Windows, where the mode
    // bits are not meaningful and `.exe` is handled via `target.binaryName`.
    if (process.platform !== "win32") {
      chmodSync(tmpPath, 0o755);
    }
    // `rename` replaces an existing destination on POSIX, which makes re-install
    // idempotent. On Windows it also replaces, EXCEPT when the old binary is
    // currently executing (locked) — then it fails with EPERM/EBUSY, which the
    // catch below converts into a `DownloadError` (fails safe: the running
    // binary is left intact and no partial file remains).
    renameSync(tmpPath, finalPath);
  } catch (cause) {
    // Best-effort cleanup so no partial temp file lingers next to the target.
    // `force: true` ignores ENOENT, so this is safe even if mkdir/write never
    // created the temp file.
    try {
      rmSync(tmpPath, { force: true });
    } catch {
      // Ignore cleanup failures — the primary error is what matters.
    }
    throw new DownloadError(
      `Failed to install the server binary to ${finalPath}: ${describeError(cause)}`,
      MANUAL_INSTALL_GUIDANCE,
      { cause },
    );
  }

  return finalPath;
}
