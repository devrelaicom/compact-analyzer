import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { gunzipSync } from "node:zlib";

import {
  afterAll,
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
} from "vitest";

import type { PlatformTarget } from "../../platform";
import {
  DownloadError,
  downloadAndInstall,
  extractArchive,
  tarEntries,
  verifySha256,
  type ServerManifest,
} from "../../download";

// ---------------------------------------------------------------------------
// Fixtures
//
// Fixtures are generated HERMETICALLY at test time with the system `tar`
// (sanctioned by the milestone plan), using `--format ustar` for a
// deterministic ustar archive — precisely the format the minimal reader
// targets. Nothing is checked in as pre-built bytes.
//
// Each archive MIRRORS the real dist layout harvested in Task 3: the binary is
// nested one directory deep under `compact-analyzer-<triple>/`, alongside a
// sibling `README.md`. The README is written and listed FIRST so it is the
// first regular-file entry in the tarball — a reader that naively took the
// first entry would extract the README, not the binary. The manifest sha256 is
// the sha256 of the generated archive, computed here.
// ---------------------------------------------------------------------------

/** Exact bytes of the dummy server binary (a runnable shebang script). */
const BINARY_BODY = Buffer.from('#!/bin/sh\necho "compact-analyzer 0.1.0"\n');
/** Exact bytes of the sibling README that precedes the binary in the archive. */
const README_BODY = Buffer.from("# compact-analyzer\n");

interface Fixture {
  triple: string;
  binaryName: string;
  /** Versionless archive filename, matching dist's real naming. */
  archiveName: string;
  archiveBytes: Uint8Array;
  /** Lowercase hex sha256 of `archiveBytes`. */
  sha256: string;
  binaryBody: Buffer;
}

const buildDirs: string[] = [];

function makeFixture(triple: string, binaryName: string): Fixture {
  const buildDir = mkdtempSync(path.join(tmpdir(), "ca-dl-fx-"));
  buildDirs.push(buildDir);

  const stem = `compact-analyzer-${triple}`;
  const stemDir = path.join(buildDir, stem);
  mkdirSync(stemDir, { recursive: true });
  writeFileSync(path.join(stemDir, "README.md"), README_BODY);
  writeFileSync(path.join(stemDir, binaryName), BINARY_BODY, { mode: 0o755 });

  const archivePath = path.join(buildDir, "fixture.tar.gz");
  execFileSync(
    "tar",
    [
      "--format",
      "ustar",
      "-czf",
      archivePath,
      "-C",
      buildDir,
      // README first, binary second — order is load-bearing (see header note).
      `${stem}/README.md`,
      `${stem}/${binaryName}`,
    ],
    { stdio: "pipe" },
  );

  const archiveBytes = new Uint8Array(readFileSync(archivePath));
  const sha256 = createHash("sha256").update(archiveBytes).digest("hex");
  return {
    triple,
    binaryName,
    archiveName: `compact-analyzer-${triple}.tar.gz`,
    archiveBytes,
    sha256,
    binaryBody: BINARY_BODY,
  };
}

/**
 * Build a fixture whose only entry matching `binaryName` is a SYMLINK (tar
 * typeflag '2'), not a regular file — plus a real sibling README. There is NO
 * regular-file binary of that name. A correct extractor skips the symlink via
 * its typeflag filter and finds no binary, so it must throw. This locks in the
 * defence against a future refactor that drops the typeflag check.
 */
function makeSymlinkFixture(triple: string, binaryName: string): Fixture {
  const buildDir = mkdtempSync(path.join(tmpdir(), "ca-dl-sl-"));
  buildDirs.push(buildDir);

  const stem = `compact-analyzer-${triple}`;
  const stemDir = path.join(buildDir, stem);
  mkdirSync(stemDir, { recursive: true });
  writeFileSync(path.join(stemDir, "README.md"), README_BODY);
  // A symlink whose basename equals the wanted binary, pointing at a sensitive
  // path. Nothing reads the target — the extractor must never even reach it.
  symlinkSync("/etc/passwd", path.join(stemDir, binaryName));

  const archivePath = path.join(buildDir, "fixture.tar.gz");
  execFileSync(
    "tar",
    [
      "--format",
      "ustar",
      "-czf",
      archivePath,
      "-C",
      buildDir,
      `${stem}/README.md`,
      `${stem}/${binaryName}`,
    ],
    { stdio: "pipe" },
  );

  const archiveBytes = new Uint8Array(readFileSync(archivePath));
  const sha256 = createHash("sha256").update(archiveBytes).digest("hex");
  return {
    triple,
    binaryName,
    archiveName: `compact-analyzer-${triple}.tar.gz`,
    archiveBytes,
    sha256,
    binaryBody: BINARY_BODY,
  };
}

/** Unix fixture: binary "compact-analyzer" nested under its archive stem. */
let unixFx: Fixture;
/** Windows fixture: binary "compact-analyzer.exe" — proves `.exe` basename pick. */
let winFx: Fixture;

beforeAll(() => {
  unixFx = makeFixture("aarch64-apple-darwin", "compact-analyzer");
  winFx = makeFixture("x86_64-pc-windows-msvc", "compact-analyzer.exe");
});

afterAll(() => {
  for (const dir of buildDirs) {
    rmSync(dir, { recursive: true, force: true });
  }
});

// A fresh install destination per test, cleaned up afterwards.
let destDir: string;
beforeEach(() => {
  destDir = mkdtempSync(path.join(tmpdir(), "ca-dl-dest-"));
});
afterEach(() => {
  rmSync(destDir, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// Test doubles for the injected `fetchImpl`.
// ---------------------------------------------------------------------------

/** A fetch that serves `bytes` as a 200 response (redirects already followed). */
const serve =
  (bytes: Uint8Array): typeof fetch =>
  async () =>
    new Response(bytes, { status: 200 });

/** A fetch that returns an HTTP 404 with no body. */
const serve404: typeof fetch = async () => new Response(null, { status: 404 });

const targetFor = (triple: string, binaryName: string): PlatformTarget => ({
  rustTriple: triple,
  archiveExt: ".tar.gz",
  binaryName,
});

const manifestFor = (fx: Fixture, sha256: string = fx.sha256): ServerManifest => ({
  version: "0.1.0",
  artifacts: { [fx.triple]: { name: fx.archiveName, sha256 } },
});

// ---------------------------------------------------------------------------
// (g) verifySha256
// ---------------------------------------------------------------------------

describe("verifySha256", () => {
  const data = new Uint8Array([1, 2, 3, 4, 5]);
  const digest = createHash("sha256").update(data).digest("hex");

  it("accepts the correct lowercase digest", () => {
    expect(verifySha256(data, digest)).toBe(true);
  });

  it("accepts the correct digest in uppercase (case-insensitive)", () => {
    expect(verifySha256(data, digest.toUpperCase())).toBe(true);
  });

  it("tolerates surrounding whitespace", () => {
    expect(verifySha256(data, `  ${digest}\n`)).toBe(true);
  });

  it("rejects a wrong digest of the correct length", () => {
    const wrong = "0".repeat(64);
    expect(verifySha256(data, wrong)).toBe(false);
  });

  it("rejects a truncated/prefix digest (never a partial compare)", () => {
    // A correct prefix must NOT satisfy the check.
    expect(verifySha256(data, digest.slice(0, 32))).toBe(false);
    expect(verifySha256(data, digest.slice(0, 8))).toBe(false);
  });

  it("rejects non-hex and malformed input", () => {
    expect(verifySha256(data, "")).toBe(false);
    expect(verifySha256(data, "not-a-hex-digest")).toBe(false);
    expect(verifySha256(data, `${digest}extra`)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// (f) tarEntries round-trip
// ---------------------------------------------------------------------------

describe("tarEntries", () => {
  it("yields the nested binary entry with the correct name and bytes", () => {
    const tar = gunzipSync(unixFx.archiveBytes);
    const entries = [...tarEntries(tar)];
    const names = entries.map((e) => e.name);

    expect(names).toContain("compact-analyzer-aarch64-apple-darwin/README.md");
    expect(names).toContain(
      "compact-analyzer-aarch64-apple-darwin/compact-analyzer",
    );

    const binary = entries.find(
      (e) => path.posix.basename(e.name) === "compact-analyzer",
    );
    expect(binary).toBeDefined();
    expect(Buffer.from(binary!.data).equals(BINARY_BODY)).toBe(true);

    // The binary is NOT the first entry — the README precedes it.
    expect(path.posix.basename(entries[0]!.name)).toBe("README.md");
  });
});

// ---------------------------------------------------------------------------
// (a) happy path
// ---------------------------------------------------------------------------

describe("downloadAndInstall — happy path", () => {
  it("installs to <destDir>/<version>/<binaryName> and returns that path", async () => {
    const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
    const result = await downloadAndInstall({
      manifest: manifestFor(unixFx),
      target,
      destDir,
      baseUrl: "https://example.test/download/",
      fetchImpl: serve(unixFx.archiveBytes),
    });

    const expected = path.join(destDir, "0.1.0", "compact-analyzer");
    expect(result).toBe(expected);
    expect(existsSync(result)).toBe(true);
  });

  it("extracts the NESTED binary bytes, not the sibling README", async () => {
    const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
    const result = await downloadAndInstall({
      manifest: manifestFor(unixFx),
      target,
      destDir,
      baseUrl: "https://example.test/download/",
      fetchImpl: serve(unixFx.archiveBytes),
    });

    const got = readFileSync(result);
    expect(got.equals(BINARY_BODY)).toBe(true);
    expect(got.equals(README_BODY)).toBe(false);
  });

  // V9 (Unix exec bit): the installed binary is mode 0o755 on Unix.
  it.skipIf(process.platform === "win32")(
    "sets the installed binary mode to 0o755 on Unix",
    async () => {
      const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
      const result = await downloadAndInstall({
        manifest: manifestFor(unixFx),
        target,
        destDir,
        baseUrl: "https://example.test/download/",
        fetchImpl: serve(unixFx.archiveBytes),
      });
      expect(statSync(result).mode & 0o777).toBe(0o755);
    },
  );
});

// ---------------------------------------------------------------------------
// (b) checksum mismatch
// ---------------------------------------------------------------------------

describe("downloadAndInstall — checksum mismatch", () => {
  it("throws DownloadError and leaves NOTHING at the final path", async () => {
    const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
    const badManifest = manifestFor(unixFx, "0".repeat(64));
    const finalPath = path.join(destDir, "0.1.0", "compact-analyzer");
    const versionDir = path.join(destDir, "0.1.0");

    await expect(
      downloadAndInstall({
        manifest: badManifest,
        target,
        destDir,
        baseUrl: "https://example.test/download/",
        fetchImpl: serve(unixFx.archiveBytes),
      }),
    ).rejects.toBeInstanceOf(DownloadError);

    // No final binary, and (verify precedes any fs write) no version dir at
    // all — hence no partial temp file left behind either.
    expect(existsSync(finalPath)).toBe(false);
    expect(existsSync(versionDir)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// (c) HTTP 404
// ---------------------------------------------------------------------------

describe("downloadAndInstall — HTTP failure", () => {
  it("throws DownloadError whose guidance names manual install AND serverPath", async () => {
    const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
    let caught: unknown;
    try {
      await downloadAndInstall({
        manifest: manifestFor(unixFx),
        target,
        destDir,
        baseUrl: "https://example.test/download/",
        fetchImpl: serve404,
      });
    } catch (err) {
      caught = err;
    }
    expect(caught).toBeInstanceOf(DownloadError);
    const guidance = (caught as DownloadError).userGuidance;
    expect(guidance).toContain("compact-analyzer.serverPath");
    expect(guidance).toContain("brew");
    // The message should surface the HTTP status for diagnosis.
    expect((caught as DownloadError).message).toContain("404");
  });
});

// ---------------------------------------------------------------------------
// (d) missing artifact for the target triple — throws before any fetch
// ---------------------------------------------------------------------------

describe("downloadAndInstall — missing artifact", () => {
  it("throws DownloadError before performing any fetch", async () => {
    const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
    // Manifest advertises a DIFFERENT triple, so the target is absent.
    const manifest: ServerManifest = {
      version: "0.1.0",
      artifacts: {
        "x86_64-unknown-linux-gnu": {
          name: "compact-analyzer-x86_64-unknown-linux-gnu.tar.gz",
          sha256: "0".repeat(64),
        },
      },
    };

    let fetchCalls = 0;
    const failIfCalled: typeof fetch = async () => {
      fetchCalls += 1;
      return new Response(null, { status: 500 });
    };

    await expect(
      downloadAndInstall({
        manifest,
        target,
        destDir,
        baseUrl: "https://example.test/download/",
        fetchImpl: failIfCalled,
      }),
    ).rejects.toBeInstanceOf(DownloadError);
    expect(fetchCalls).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// (e) idempotent re-install
// ---------------------------------------------------------------------------

describe("downloadAndInstall — idempotent re-install", () => {
  it("succeeds when <destDir>/<version>/ already holds an install", async () => {
    const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
    const opts = {
      manifest: manifestFor(unixFx),
      target,
      destDir,
      baseUrl: "https://example.test/download/",
      fetchImpl: serve(unixFx.archiveBytes),
    };

    const first = await downloadAndInstall(opts);
    const second = await downloadAndInstall(opts);

    expect(second).toBe(first);
    expect(existsSync(second)).toBe(true);
    expect(readFileSync(second).equals(BINARY_BODY)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// (V9) Windows `.exe` handling — basename pick, no chmod dependency
// ---------------------------------------------------------------------------

describe("downloadAndInstall — Windows .exe artifact", () => {
  it("locates and installs the nested compact-analyzer.exe by basename", async () => {
    const target = targetFor("x86_64-pc-windows-msvc", "compact-analyzer.exe");
    const result = await downloadAndInstall({
      manifest: manifestFor(winFx),
      target,
      destDir,
      baseUrl: "https://example.test/download/",
      fetchImpl: serve(winFx.archiveBytes),
    });

    expect(result).toBe(path.join(destDir, "0.1.0", "compact-analyzer.exe"));
    expect(readFileSync(result).equals(BINARY_BODY)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// (V10) Default base URL is the direct, unmetered GitHub release-asset URL.
// The injected fetch simulates a redirect-followed 200 (the real default
// `fetch` follows the 302 to the CDN before returning this response).
// ---------------------------------------------------------------------------

describe("downloadAndInstall — default GitHub release URL", () => {
  it("builds the tag-pinned releases/download URL and installs the served bytes", async () => {
    const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
    let requested: string | undefined;
    const recording: typeof fetch = async (input) => {
      requested = String(input);
      return new Response(unixFx.archiveBytes, { status: 200 });
    };

    const result = await downloadAndInstall({
      manifest: manifestFor(unixFx),
      target,
      destDir,
      // baseUrl omitted -> default GitHub Releases download URL.
      fetchImpl: recording,
    });

    expect(requested).toBe(
      "https://github.com/devrelaicom/compact-analyzer/releases/download/v0.1.0/compact-analyzer-aarch64-apple-darwin.tar.gz",
    );
    expect(readFileSync(result).equals(BINARY_BODY)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Security: HTTPS-only (defence-in-depth). A non-HTTPS baseUrl is rejected
// BEFORE any fetch is attempted.
// ---------------------------------------------------------------------------

describe("downloadAndInstall — HTTPS only", () => {
  it("rejects a non-HTTPS baseUrl without fetching", async () => {
    const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
    let fetchCalls = 0;
    const failIfCalled: typeof fetch = async () => {
      fetchCalls += 1;
      return new Response(unixFx.archiveBytes, { status: 200 });
    };

    await expect(
      downloadAndInstall({
        manifest: manifestFor(unixFx),
        target,
        destDir,
        baseUrl: "http://example.test/download/",
        fetchImpl: failIfCalled,
      }),
    ).rejects.toBeInstanceOf(DownloadError);
    expect(fetchCalls).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// extractArchive: the localised format boundary. Verifies it locates the
// nested binary and rejects an archive that lacks it.
// ---------------------------------------------------------------------------

describe("extractArchive", () => {
  it("returns the nested binary bytes for a matching basename", () => {
    const bytes = extractArchive(unixFx.archiveBytes, "compact-analyzer");
    expect(Buffer.from(bytes).equals(BINARY_BODY)).toBe(true);
  });

  it("throws DownloadError when no entry matches the binary name", () => {
    expect(() => extractArchive(unixFx.archiveBytes, "does-not-exist")).toThrow(
      DownloadError,
    );
  });

  it("throws DownloadError when the bytes are not a valid gzip stream", () => {
    expect(() =>
      extractArchive(new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7]), "compact-analyzer"),
    ).toThrow(DownloadError);
  });
});

// ---------------------------------------------------------------------------
// (I2) Symlink / non-regular-entry traversal defence.
//
// A malicious archive could carry a symlink (or hardlink/directory) whose
// basename equals the wanted binary, pointing at a sensitive path. The minimal
// reader's typeflag filter must skip such entries: only a REGULAR file may be
// installed. Gated off Windows, where creating the symlink fixture needs
// elevated privileges. Verified non-vacuous: removing the typeflag filter makes
// the reader yield the symlink, so these assertions fail.
// ---------------------------------------------------------------------------

describe.skipIf(process.platform === "win32")(
  "symlink-entry defence",
  () => {
    let symlinkFx: Fixture;
    beforeAll(() => {
      symlinkFx = makeSymlinkFixture("aarch64-apple-darwin", "compact-analyzer");
    });

    it("extractArchive ignores a symlink whose basename matches the binary", () => {
      expect(() =>
        extractArchive(symlinkFx.archiveBytes, "compact-analyzer"),
      ).toThrow(DownloadError);
      expect(() =>
        extractArchive(symlinkFx.archiveBytes, "compact-analyzer"),
      ).toThrow(/did not contain the expected binary/);
    });

    it("downloadAndInstall throws and installs nothing when only a symlink matches", async () => {
      const target = targetFor("aarch64-apple-darwin", "compact-analyzer");
      const finalPath = path.join(destDir, "0.1.0", "compact-analyzer");

      await expect(
        downloadAndInstall({
          manifest: manifestFor(symlinkFx),
          target,
          destDir,
          baseUrl: "https://example.test/download/",
          fetchImpl: serve(symlinkFx.archiveBytes),
        }),
      ).rejects.toThrow(DownloadError);
      expect(existsSync(finalPath)).toBe(false);
    });
  },
);
