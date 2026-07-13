/**
 * Pure platform → release-artifact mapping.
 *
 * This module has no `vscode` import and performs no I/O: it is a literal
 * lookup table plus two total functions over it. Task 6 (the extractor)
 * consumes `binaryName` and the archive name produced here; the archive's
 * internal nesting and the Windows `.exe` suffix inside the tarball are its
 * concern, not this module's.
 */

export interface PlatformTarget {
  /** Rust target triple, e.g. "aarch64-apple-darwin". */
  rustTriple: string;
  /** Archive extension — fixed to ".tar.gz" for every target (Resolved OQ4). */
  archiveExt: ".tar.gz";
  /** Server binary name on this platform. */
  binaryName: string;
}

/**
 * The supported v1 release matrix, keyed by `${platform}/${arch}`.
 *
 * Anything absent from this table is unsupported and resolves to `null`.
 * `linux/arm64` is the plausible future add: it is deliberately omitted for
 * now so it returns `null`. When the release matrix gains that target, add a
 * row here with the triple "aarch64-unknown-linux-gnu".
 */
const TARGETS: Readonly<Record<string, PlatformTarget>> = {
  "darwin/arm64": {
    rustTriple: "aarch64-apple-darwin",
    archiveExt: ".tar.gz",
    binaryName: "compact-analyzer",
  },
  "darwin/x64": {
    rustTriple: "x86_64-apple-darwin",
    archiveExt: ".tar.gz",
    binaryName: "compact-analyzer",
  },
  "linux/x64": {
    rustTriple: "x86_64-unknown-linux-gnu",
    archiveExt: ".tar.gz",
    binaryName: "compact-analyzer",
  },
  "win32/x64": {
    rustTriple: "x86_64-pc-windows-msvc",
    archiveExt: ".tar.gz",
    binaryName: "compact-analyzer.exe",
  },
};

/**
 * Map (`process.platform`, `process.arch`) to a release target, or `null` when
 * the current platform/arch pair is unsupported. Both parameters default to the
 * running process's values when omitted.
 */
export function currentTarget(
  platform: NodeJS.Platform = process.platform,
  arch: string = process.arch,
): PlatformTarget | null {
  // `noUncheckedIndexedAccess` widens the lookup to `… | undefined`; `?? null`
  // collapses the miss case to the documented `null`.
  return TARGETS[`${platform}/${arch}`] ?? null;
}

/**
 * The release artifact filename for a target, matching dist's real naming.
 *
 * Versionless by design: the git tag carries the version, so the archive name
 * does not. Reproduces the four hardcoded dist outputs exactly, e.g.
 * "compact-analyzer-aarch64-apple-darwin.tar.gz".
 */
export function artifactName(target: PlatformTarget): string {
  return `compact-analyzer-${target.rustTriple}${target.archiveExt}`;
}
