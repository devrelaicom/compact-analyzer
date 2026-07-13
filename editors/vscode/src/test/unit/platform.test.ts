import { describe, it, expect } from "vitest";
import { currentTarget, artifactName, type PlatformTarget } from "../../platform";

describe("currentTarget (supported v1 matrix)", () => {
  // platform, arch, expected rustTriple, expected binaryName
  it.each<[NodeJS.Platform, string, string, string]>([
    ["darwin", "arm64", "aarch64-apple-darwin", "compact-analyzer"],
    ["darwin", "x64", "x86_64-apple-darwin", "compact-analyzer"],
    ["linux", "x64", "x86_64-unknown-linux-gnu", "compact-analyzer"],
    ["win32", "x64", "x86_64-pc-windows-msvc", "compact-analyzer.exe"],
  ])("%s/%s maps to %s", (platform, arch, rustTriple, binaryName) => {
    const target = currentTarget(platform, arch);
    expect(target).not.toBeNull();
    expect(target?.rustTriple).toBe(rustTriple);
    expect(target?.binaryName).toBe(binaryName);
    // archiveExt is fixed to ".tar.gz" for every target (Resolved OQ4).
    expect(target?.archiveExt).toBe(".tar.gz");
  });
});

describe("currentTarget (unsupported combos return null)", () => {
  it.each<[NodeJS.Platform, string]>([
    // The plausible future add — deliberately unsupported for v1.
    ["linux", "arm64"],
    ["darwin", "ia32"],
    ["freebsd", "x64"],
    ["win32", "arm64"],
    ["linux", "ia32"],
  ])("%s/%s is unsupported", (platform, arch) => {
    expect(currentTarget(platform, arch)).toBeNull();
  });
});

describe("currentTarget (default parameters)", () => {
  it("defaults to process.platform and process.arch when omitted", () => {
    expect(currentTarget()).toEqual(
      currentTarget(process.platform, process.arch),
    );
  });
});

describe("artifactName (versionless, matches dist's real naming)", () => {
  // rustTriple, binaryName, expected artifact filename (verbatim from dist).
  it.each<[string, string, string]>([
    [
      "aarch64-apple-darwin",
      "compact-analyzer",
      "compact-analyzer-aarch64-apple-darwin.tar.gz",
    ],
    [
      "x86_64-apple-darwin",
      "compact-analyzer",
      "compact-analyzer-x86_64-apple-darwin.tar.gz",
    ],
    [
      "x86_64-unknown-linux-gnu",
      "compact-analyzer",
      "compact-analyzer-x86_64-unknown-linux-gnu.tar.gz",
    ],
    [
      "x86_64-pc-windows-msvc",
      "compact-analyzer.exe",
      "compact-analyzer-x86_64-pc-windows-msvc.tar.gz",
    ],
  ])("%s produces the versionless archive name", (rustTriple, binaryName, expected) => {
    const target: PlatformTarget = {
      rustTriple,
      archiveExt: ".tar.gz",
      binaryName,
    };
    expect(artifactName(target)).toBe(expected);
    // Guard against a version segment sneaking back in — the tag carries it.
    expect(artifactName(target)).not.toMatch(/\d+\.\d+\.\d+/);
  });
});
