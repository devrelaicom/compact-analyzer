import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import type { ServerManifest } from "../../download";
import { currentTarget } from "../../platform";
import { PINNED_SERVER_VERSION } from "../../version";
import { acquireServer, defaultExecProbe, type ExecProbe } from "../../acquire";

// ---------------------------------------------------------------------------
// Shared helpers
//
// The acquisition logic probes real binaries via the default `spawn`-backed
// `ExecProbe`, so the exec-dependent suites create runnable `#!/bin/sh`
// scripts on disk and are gated off Windows (where the shebang trick and the
// executable bit do not apply). The pure-logic suites — a timed-out candidate,
// a failed download — inject an `ExecProbe`/`fetch` and run everywhere.
// ---------------------------------------------------------------------------

const isWindows = process.platform === "win32";
/** OS binary basename, mirroring the acquisition module's own derivation. */
const basename = isWindows ? "compact-analyzer.exe" : "compact-analyzer";

/** Temp directories created during a test, torn down afterwards. */
const tempDirs: string[] = [];
function tempDir(prefix: string): string {
  const dir = mkdtempSync(path.join(tmpdir(), prefix));
  tempDirs.push(dir);
  return dir;
}
afterEach(() => {
  for (const dir of tempDirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true });
  }
});

/**
 * Write a runnable fake server that prints `compact-analyzer <version>` to
 * stdout, exactly the handshake the probe parses. Returns its absolute path.
 */
function makeFakeServer(dir: string, name: string, version: string): string {
  const filePath = path.join(dir, name);
  writeFileSync(filePath, `#!/bin/sh\necho "compact-analyzer ${version}"\n`, {
    mode: 0o755,
  });
  chmodSync(filePath, 0o755);
  return filePath;
}

/** A `fetch` that must never be called; records calls and throws if invoked. */
function makeNoFetch(): { fetchImpl: typeof fetch; calls: () => number } {
  let calls = 0;
  const fetchImpl: typeof fetch = () => {
    calls += 1;
    throw new Error("download must not be attempted");
  };
  return { fetchImpl, calls: () => calls };
}

/** A `fetch` that serves `bytes` as a redirect-followed 200 response. */
const serve =
  (bytes: Uint8Array): typeof fetch =>
  async () =>
    new Response(bytes, { status: 200 });

/** A `fetch` that returns HTTP 404 with no body. */
const serve404: typeof fetch = async () => new Response(null, { status: 404 });

/** A manifest with no artefacts — the download step never reaches the network. */
const emptyManifest: ServerManifest = {
  version: PINNED_SERVER_VERSION,
  artifacts: {},
};

/**
 * Build a `.tar.gz` fixture mirroring the real dist layout: the binary nested
 * one directory deep under `compact-analyzer-<triple>/` beside a README that
 * precedes it. Returns the archive bytes plus its sha256, for feeding an
 * injected `fetch`.
 */
function makeFixture(
  triple: string,
  binaryName: string,
): { archiveName: string; archiveBytes: Uint8Array; sha256: string } {
  const buildDir = tempDir("ca-acq-fx-");
  const stem = `compact-analyzer-${triple}`;
  const stemDir = path.join(buildDir, stem);
  mkdirSync(stemDir, { recursive: true });
  writeFileSync(path.join(stemDir, "README.md"), "# compact-analyzer\n");
  writeFileSync(
    path.join(stemDir, binaryName),
    `#!/bin/sh\necho "compact-analyzer ${PINNED_SERVER_VERSION}"\n`,
    { mode: 0o755 },
  );

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
  return { archiveName: `${stem}.tar.gz`, archiveBytes, sha256 };
}

// ---------------------------------------------------------------------------
// (a)-(d) Resolution order over real binaries: settings -> PATH -> storage.
// ---------------------------------------------------------------------------

describe.skipIf(isWindows)("acquireServer — real-binary resolution", () => {
  it("(a) accepts a compatible configured serverPath as source 'settings'", async () => {
    const bin = makeFakeServer(tempDir("ca-acq-a-"), basename, PINNED_SERVER_VERSION);
    const noFetch = makeNoFetch();

    const result = await acquireServer({
      configuredPath: bin,
      storageDir: tempDir("ca-acq-a-store-"),
      manifest: emptyManifest,
      env: {},
      fetchImpl: noFetch.fetchImpl,
    });

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.source).toBe("settings");
      expect(result.binaryPath).toBe(bin);
      expect(result.version).toBe(PINNED_SERVER_VERSION);
    }
    expect(noFetch.calls()).toBe(0);
  });

  it("(b) rejects an incompatible configured serverPath and never downloads", async () => {
    // Wrong minor: pre-1.0 the minor is the breaking axis, so 0.2.0 is incompatible.
    const bin = makeFakeServer(tempDir("ca-acq-b-"), basename, "0.2.0");
    const noFetch = makeNoFetch();

    const result = await acquireServer({
      configuredPath: bin,
      storageDir: tempDir("ca-acq-b-store-"),
      manifest: emptyManifest,
      env: {},
      fetchImpl: noFetch.fetchImpl,
    });

    expect(result.ok).toBe(false);
    if (!result.ok) {
      // The reason must name BOTH the observed version and the setting.
      expect(result.reason).toContain("0.2.0");
      expect(result.reason).toContain("compact-analyzer.serverPath");
    }
    // D5: an explicit path never silently falls through to a download.
    expect(noFetch.calls()).toBe(0);
  });

  it("(c) finds a compatible binary on PATH as source 'path'", async () => {
    const pathDir = tempDir("ca-acq-c-");
    const bin = makeFakeServer(pathDir, basename, "0.1.4");
    const noFetch = makeNoFetch();

    const result = await acquireServer({
      configuredPath: undefined,
      storageDir: tempDir("ca-acq-c-store-"),
      manifest: emptyManifest,
      env: { PATH: pathDir },
      fetchImpl: noFetch.fetchImpl,
    });

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.source).toBe("path");
      expect(result.binaryPath).toBe(bin);
      expect(result.version).toBe("0.1.4");
    }
    expect(noFetch.calls()).toBe(0);
  });

  it("(d) falls through an incompatible PATH binary to a valid storage cache", async () => {
    const pathDir = tempDir("ca-acq-d-path-");
    makeFakeServer(pathDir, basename, "0.2.0"); // incompatible: logged, fall through

    const storageDir = tempDir("ca-acq-d-store-");
    const cacheDir = path.join(storageDir, PINNED_SERVER_VERSION);
    mkdirSync(cacheDir, { recursive: true });
    const cacheBin = makeFakeServer(cacheDir, basename, "0.1.0"); // compatible
    const noFetch = makeNoFetch();

    const result = await acquireServer({
      configuredPath: undefined,
      storageDir,
      manifest: emptyManifest,
      env: { PATH: pathDir },
      fetchImpl: noFetch.fetchImpl,
    });

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.source).toBe("storage");
      expect(result.binaryPath).toBe(cacheBin);
      expect(result.version).toBe("0.1.0");
    }
    expect(noFetch.calls()).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// (e) Nothing anywhere -> download the pinned artefact, then re-validate it.
// ---------------------------------------------------------------------------

const downloadTarget = currentTarget();
describe.skipIf(isWindows || downloadTarget === null)(
  "acquireServer — download fallback",
  () => {
    it("(e) downloads, installs, re-validates, and reports source 'downloaded'", async () => {
      const target = downloadTarget!; // non-null: guarded by skipIf
      const fixture = makeFixture(target.rustTriple, target.binaryName);
      const manifest: ServerManifest = {
        version: PINNED_SERVER_VERSION,
        artifacts: {
          [target.rustTriple]: {
            name: fixture.archiveName,
            sha256: fixture.sha256,
          },
        },
      };
      const storageDir = tempDir("ca-acq-e-store-");

      const result = await acquireServer({
        configuredPath: undefined,
        storageDir,
        manifest,
        env: {},
        // No `exec` injected: the REAL default probe re-validates the installed
        // binary by running the freshly-downloaded script.
        fetchImpl: serve(fixture.archiveBytes),
      });

      expect(result.ok).toBe(true);
      if (result.ok) {
        expect(result.source).toBe("downloaded");
        expect(result.binaryPath).toBe(
          path.join(storageDir, PINNED_SERVER_VERSION, target.binaryName),
        );
        expect(result.version).toBe(PINNED_SERVER_VERSION);
        expect(existsSync(result.binaryPath)).toBe(true);
      }
    });
  },
);

// ---------------------------------------------------------------------------
// (f) Nothing anywhere + a failing fetch -> failure with manual-install guidance.
// ---------------------------------------------------------------------------

describe.skipIf(currentTarget() === null)(
  "acquireServer — download failure",
  () => {
    it("(f) returns a failure carrying the DownloadError's manual-install guidance", async () => {
      const target = currentTarget()!; // non-null: guarded by skipIf
      const manifest: ServerManifest = {
        version: PINNED_SERVER_VERSION,
        artifacts: {
          [target.rustTriple]: {
            name: `compact-analyzer-${target.rustTriple}.tar.gz`,
            sha256: "0".repeat(64),
          },
        },
      };

      const result = await acquireServer({
        configuredPath: undefined,
        storageDir: tempDir("ca-acq-f-store-"),
        manifest,
        env: {},
        fetchImpl: serve404,
      });

      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.userGuidance).toContain("compact-analyzer.serverPath");
        expect(result.userGuidance).toContain(
          "https://github.com/devrelaicom/compact-analyzer/releases",
        );
        // The reason surfaces the HTTP status for diagnosis.
        expect(result.reason).toContain("404");
      }
    });
  },
);

// ---------------------------------------------------------------------------
// (g) A hanging/timed-out probe makes a candidate invalid without wedging.
// ---------------------------------------------------------------------------

describe("acquireServer — timed-out probe", () => {
  // Simulates the default probe's timeout branch: it resolves not-ok.
  const timedOut: ExecProbe = async () => ({ ok: false, stdout: "" });

  it("(g) a configured path whose probe times out fails cleanly, no download", async () => {
    const noFetch = makeNoFetch();

    const result = await acquireServer({
      configuredPath: "/nonexistent/hanging/compact-analyzer",
      storageDir: tempDir("ca-acq-g1-store-"),
      manifest: emptyManifest,
      env: {},
      exec: timedOut,
      fetchImpl: noFetch.fetchImpl,
    });

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("compact-analyzer.serverPath");
    }
    expect(noFetch.calls()).toBe(0);
  });

  it("(g) with no configured path, a timed-out probe falls through and still completes", async () => {
    const result = await acquireServer({
      configuredPath: undefined,
      storageDir: tempDir("ca-acq-g2-store-"),
      manifest: emptyManifest, // no artefact -> download fails cleanly
      env: {},
      exec: timedOut,
      fetchImpl: serve404,
    });

    // The key property: acquisition resolves (never wedges) to a clean failure.
    expect(result.ok).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// The default `ExecProbe`: real child process, hard timeout + kill, no throws.
// ---------------------------------------------------------------------------

describe.skipIf(isWindows)("defaultExecProbe — real child process", () => {
  it("runs a binary and returns its stdout with ok=true", async () => {
    const bin = makeFakeServer(tempDir("ca-acq-probe-"), "fake-server", "0.1.0");
    const result = await defaultExecProbe(bin, ["--version"], 2000);
    expect(result.ok).toBe(true);
    expect(result.stdout).toContain("compact-analyzer 0.1.0");
  });

  it("kills a hanging child at the timeout and resolves ok=false promptly", async () => {
    const dir = tempDir("ca-acq-probe-timeout-");
    const hang = path.join(dir, "hang");
    writeFileSync(hang, "#!/bin/sh\nsleep 5\necho done\n", { mode: 0o755 });
    chmodSync(hang, 0o755);

    const start = Date.now();
    const result = await defaultExecProbe(hang, ["--version"], 200);
    const elapsed = Date.now() - start;

    expect(result.ok).toBe(false);
    expect(result.stdout).toBe("");
    // Proves the child was killed rather than awaited for its full 5s sleep.
    expect(elapsed).toBeLessThan(3000);
  });
});

describe("defaultExecProbe — missing binary", () => {
  it("resolves ok=false for a non-existent binary and never throws", async () => {
    const missing = path.join(tempDir("ca-acq-missing-"), "does-not-exist");
    const result = await defaultExecProbe(missing, ["--version"], 2000);
    expect(result.ok).toBe(false);
    expect(result.stdout).toBe("");
  });
});
