import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

// Drive the REAL generator as a subprocess against a synthetic `sha256.sum`
// fixture. Exercising the shipped CLI (argv parsing, file reading, JSON output)
// end-to-end is both a truer test and keeps the `tsc` gate clean — the test
// imports no `.mjs` from outside the extension's own `src` tree, so there is no
// declaration-file coupling to the repo-root script.
//
// The generator lives at the repository root. Vitest always runs from the
// extension package directory (`editors/vscode`), locally via `npm test` and in
// CI via `working-directory: editors/vscode`, so the root is two levels up from
// the working directory. (`import.meta`/`__dirname` are both off-limits here:
// this is a CommonJS package, so `.ts` files may use neither reliably.)
const repoRoot = path.resolve(process.cwd(), "..", "..");
const script = path.join(repoRoot, "scripts", "gen-server-manifest.mjs");

// The four target triples dist builds, and the archive name each maps to. This
// mirrors the committed dev-placeholder manifest and the `ServerManifest`
// schema consumed by `src/download.ts`.
const EXPECTED = [
  {
    triple: "aarch64-apple-darwin",
    sha: "a".repeat(64),
  },
  {
    triple: "x86_64-apple-darwin",
    sha: "b".repeat(64),
  },
  {
    triple: "x86_64-unknown-linux-gnu",
    sha: "c".repeat(64),
  },
  {
    triple: "x86_64-pc-windows-msvc",
    sha: "d".repeat(64),
  },
] as const;

interface ServerManifest {
  version: string;
  artifacts: Record<string, { name: string; sha256: string }>;
}

let workDir: string;

beforeAll(() => {
  workDir = mkdtempSync(path.join(tmpdir(), "gen-manifest-"));
});

afterAll(() => {
  rmSync(workDir, { recursive: true, force: true });
});

function runGenerator(checksumsPath: string, tag: string): ServerManifest {
  const outPath = path.join(workDir, `manifest-${Buffer.from(tag).toString("hex")}.json`);
  execFileSync(process.execPath, [script, "--checksums", checksumsPath, "--tag", tag, "--out", outPath], {
    encoding: "utf8",
  });
  return JSON.parse(readFileSync(outPath, "utf8")) as ServerManifest;
}

describe("gen-server-manifest", () => {
  it("bakes version + all four triples from an aggregate sha256.sum (with the `*` binary marker)", () => {
    // Synthetic fixture in dist's V6 `sha256.sum` shape: `<64hex> *<bare-name>`.
    const sumPath = path.join(workDir, "sha256.sum");
    const lines = EXPECTED.map(
      ({ triple, sha }) => `${sha} *compact-analyzer-${triple}.tar.gz`,
    );
    writeFileSync(sumPath, `${lines.join("\n")}\n`, "utf8");

    const manifest = runGenerator(sumPath, "v9.9.9");

    // Version comes from the tag with its leading `v` stripped.
    expect(manifest.version).toBe("9.9.9");

    // All four triples present with the correct archive name + sha256, and no
    // extras — byte-compatible with the ServerManifest schema.
    expect(Object.keys(manifest.artifacts).sort()).toEqual(
      EXPECTED.map((e) => e.triple).sort(),
    );
    for (const { triple, sha } of EXPECTED) {
      const entry = manifest.artifacts[triple];
      expect(entry).toBeDefined();
      // Optional chaining keeps `noUncheckedIndexedAccess` satisfied; if the
      // entry were missing the comparison against a string still fails.
      expect(entry?.name).toBe(`compact-analyzer-${triple}.tar.gz`);
      expect(entry?.sha256).toBe(sha);
    }
  });

  it("is structurally compatible with the ServerManifest schema", () => {
    const sumPath = path.join(workDir, "schema.sum");
    const lines = EXPECTED.map(
      ({ triple, sha }) => `${sha} *compact-analyzer-${triple}.tar.gz`,
    );
    writeFileSync(sumPath, `${lines.join("\n")}\n`, "utf8");

    const manifest = runGenerator(sumPath, "v0.1.0");

    // Shape the extension's download/verify code relies on: a version string
    // and an artifacts record whose every value is { name, sha256 } strings,
    // each sha256 a full 64-char lowercase hex digest.
    expect(typeof manifest.version).toBe("string");
    expect(typeof manifest.artifacts).toBe("object");
    for (const value of Object.values(manifest.artifacts)) {
      expect(typeof value.name).toBe("string");
      expect(value.name.length).toBeGreaterThan(0);
      expect(value.sha256).toMatch(/^[0-9a-f]{64}$/);
    }
    // Exactly the two top-level keys the schema defines — nothing leaks in.
    expect(Object.keys(manifest).sort()).toEqual(["artifacts", "version"]);
  });

  it("does not fabricate checksums: a missing artifact fails loudly", () => {
    const sumPath = path.join(workDir, "incomplete.sum");
    // Only one of the four artifacts is present.
    writeFileSync(
      sumPath,
      `${"a".repeat(64)} *compact-analyzer-aarch64-apple-darwin.tar.gz\n`,
      "utf8",
    );

    expect(() => runGenerator(sumPath, "v0.1.0")).toThrow();
  });
});
