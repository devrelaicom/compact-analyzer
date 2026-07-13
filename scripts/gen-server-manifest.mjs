#!/usr/bin/env node
/**
 * Generate the extension's `server-manifest.json` from a release's checksum
 * assets.
 *
 * The VSIX must embed the FINAL server-artifact checksums, which only exist
 * once `dist` has built and hashed the archives. This script closes that
 * ordering gap: at release time it reads the checksum files dist attaches to
 * the GitHub Release, then bakes a manifest in the schema the extension's
 * download/verify code (`editors/vscode/src/download.ts`) consumes:
 *
 *   { "version": "<x.y.z>",
 *     "artifacts": { "<rustTriple>": { "name": "<archive>", "sha256": "<hex>" } } }
 *
 * The four target triples are fixed (they mirror `dist`'s configured build
 * matrix). For each triple the archive is `compact-analyzer-<triple>.tar.gz`;
 * its sha256 is looked up in the checksum input by the archive's bare
 * filename. Checksums are NEVER hardcoded — they come entirely from the input.
 *
 * Usage:
 *   node scripts/gen-server-manifest.mjs \
 *     --checksums <dir-or-file> \
 *     (--tag v0.1.0 | --version 0.1.0) \
 *     [--out editors/vscode/server-manifest.json]
 *
 *   --checksums  A directory of downloaded release assets (the aggregate
 *                `sha256.sum` and/or per-artifact `<archive>.sha256` files),
 *                or a single such checksum file. Both use the same line shape.
 *   --tag        The release tag (e.g. `v0.1.0`); a single leading `v` is
 *                stripped to derive the version.
 *   --version    The version directly (e.g. `0.1.0`). Mutually exclusive with
 *                `--tag`; exactly one of the two is required.
 *   --out        Output path. Defaults to `editors/vscode/server-manifest.json`
 *                resolved relative to the repository root.
 *
 * This is a dependency-free Node ESM module: its pure functions
 * (`parseChecksums`, `buildManifest`) are exported so they can be unit-tested,
 * and the file only performs I/O when run directly as a CLI.
 */

import {
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
  mkdirSync,
} from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

/**
 * The Rust target triples dist builds, in the canonical order used throughout
 * the extension (and in the committed dev-placeholder manifest). The generated
 * manifest lists artifacts in exactly this order for a stable, reviewable diff.
 */
export const TRIPLES = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
];

/** The versionless archive filename dist produces for a given target triple. */
export function archiveName(triple) {
  return `compact-analyzer-${triple}.tar.gz`;
}

/**
 * Parse checksum text into a map of bare filename to lowercased sha256 hex.
 *
 * Accepts the shape dist writes for both the aggregate `sha256.sum` and the
 * per-artifact `<archive>.sha256` files: one entry per line, a 64-hex digest
 * then whitespace then the filename. The filename may carry a leading `*`
 * (the coreutils "binary mode" marker) and, defensively, a leading path — both
 * are normalised away so lookups are by bare basename. Blank lines and lines
 * that do not match the digest-then-name shape are ignored, so stray comments
 * or trailing newlines never derail parsing.
 */
export function parseChecksums(text) {
  const map = new Map();
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (line === "") continue;
    // <64 hex> <whitespace> [*]<name>. The `*` binary marker is optional; the
    // name is captured greedily so filenames containing spaces still parse.
    const match = /^([0-9a-fA-F]{64})\s+\*?(.+)$/.exec(line);
    if (!match) continue;
    const hex = match[1].toLowerCase();
    const name = path.posix.basename(match[2].trim());
    // First writer wins: the aggregate and per-artifact files agree, but if a
    // name somehow recurred we keep the earliest and never silently overwrite.
    if (!map.has(name)) {
      map.set(name, hex);
    }
  }
  return map;
}

/**
 * Build the server manifest object for `version` from a checksum map.
 *
 * Iterates the fixed triples, resolves each archive's sha256 from the map, and
 * throws a clear error naming the missing artifact (and what WAS found) rather
 * than emitting a manifest with a gap. Never fabricates a checksum.
 */
export function buildManifest({ checksums, version }) {
  if (typeof version !== "string" || version.trim() === "") {
    throw new Error("A non-empty version is required.");
  }
  const artifacts = {};
  for (const triple of TRIPLES) {
    const name = archiveName(triple);
    const sha256 = checksums.get(name);
    if (!sha256) {
      const found = [...checksums.keys()].sort().join(", ") || "(none)";
      throw new Error(
        `No sha256 found for "${name}" (target ${triple}). ` +
          `Checksum entries seen: ${found}.`,
      );
    }
    artifacts[triple] = { name, sha256 };
  }
  return { version: version.trim(), artifacts };
}

/**
 * Read checksum text from a path that is either a single checksum file or a
 * directory of release assets. For a directory, every `sha256.sum` and
 * `*.sha256` file is concatenated (their line shapes are identical), so the
 * caller can simply point this at a folder of downloaded assets.
 */
function readChecksumInput(inputPath) {
  const stats = statSync(inputPath);
  if (stats.isDirectory()) {
    const entries = readdirSync(inputPath)
      .filter((name) => name === "sha256.sum" || name.endsWith(".sha256"))
      .sort();
    if (entries.length === 0) {
      throw new Error(
        `No checksum files (sha256.sum or *.sha256) found in directory ${inputPath}.`,
      );
    }
    return entries
      .map((name) => readFileSync(path.join(inputPath, name), "utf8"))
      .join("\n");
  }
  return readFileSync(inputPath, "utf8");
}

/** Strip a single leading `v` from a tag to derive the version. */
export function versionFromTag(tag) {
  return tag.startsWith("v") ? tag.slice(1) : tag;
}

/** Minimal `--flag value` parser; unknown flags are a hard error. */
function parseArgs(argv) {
  const opts = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    switch (arg) {
      case "--checksums":
      case "--tag":
      case "--version":
      case "--out": {
        const value = argv[i + 1];
        if (value === undefined) {
          throw new Error(`Missing value for ${arg}.`);
        }
        opts[arg.slice(2)] = value;
        i += 1;
        break;
      }
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return opts;
}

function main(argv) {
  const opts = parseArgs(argv);

  if (!opts.checksums) {
    throw new Error("--checksums <dir-or-file> is required.");
  }
  if (opts.tag && opts.version) {
    throw new Error("Pass only one of --tag or --version, not both.");
  }
  const version = opts.version ?? (opts.tag ? versionFromTag(opts.tag) : undefined);
  if (!version) {
    throw new Error("One of --tag or --version is required.");
  }

  // The repository root is this script's parent directory's parent
  // (`<root>/scripts/gen-server-manifest.mjs`).
  const here = path.dirname(fileURLToPath(import.meta.url));
  const repoRoot = path.resolve(here, "..");
  const outPath = opts.out
    ? path.resolve(opts.out)
    : path.join(repoRoot, "editors", "vscode", "server-manifest.json");

  const checksumText = readChecksumInput(path.resolve(opts.checksums));
  const checksums = parseChecksums(checksumText);
  const manifest = buildManifest({ checksums, version });

  mkdirSync(path.dirname(outPath), { recursive: true });
  writeFileSync(outPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  // eslint-disable-next-line no-console
  console.log(`Wrote ${outPath} (version ${version}, ${TRIPLES.length} artifacts).`);
}

// Run as a CLI only when invoked directly (not when imported by a test).
if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    // eslint-disable-next-line no-console
    console.error(`gen-server-manifest: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}
