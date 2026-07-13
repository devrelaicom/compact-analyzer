import { describe, it, expect } from "vitest";
import { PINNED_SERVER_VERSION, parseServerVersion, isCompatible } from "../../version";
import pkg from "../../../package.json";

describe("parseServerVersion", () => {
  it("parses the real --version line", () =>
    expect(parseServerVersion("compact-analyzer 0.1.0\n")).toBe("0.1.0"));
  it("tolerates suffixes", () =>
    expect(parseServerVersion("compact-analyzer 0.2.0-rc.1")).toBe("0.2.0-rc.1"));
  it("rejects other binaries", () =>
    expect(parseServerVersion("compactc 0.31.1")).toBeNull());
  it("rejects garbage/empty", () => {
    expect(parseServerVersion("")).toBeNull();
    expect(parseServerVersion("segfault")).toBeNull();
  });
});

describe("isCompatible (0.x minor-match, >=1.0 major-match, patch skew ok)", () => {
  it.each([
    ["0.1.5", "0.1.0", true],
    ["0.2.0", "0.1.0", false],
    ["1.2.3", "1.0.0", true],
    ["2.0.0", "1.9.9", false],
    ["0.1.0", "0.1.0", true],
  ])("%s vs pin %s -> %s", (server, pin, want) =>
    expect(isCompatible(server, pin)).toBe(want));
  it("rejects non-semver", () => expect(isCompatible("not-a-version", "0.1.0")).toBe(false));
});

it("PINNED_SERVER_VERSION mirrors package.json", () =>
  expect(PINNED_SERVER_VERSION).toBe(pkg.version));
