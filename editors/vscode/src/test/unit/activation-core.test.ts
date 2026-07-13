import { describe, expect, it } from "vitest";

import {
  checkServerInfoCompatibility,
  formatAcquireFailureMessage,
  parseServerManifest,
  shouldOfferCoexistenceHint,
} from "../../activation-core";

describe("parseServerManifest", () => {
  const valid = JSON.stringify({
    version: "0.1.0",
    artifacts: {
      "aarch64-apple-darwin": { name: "a.tar.gz", sha256: "0".repeat(64) },
    },
  });

  it("parses a well-formed manifest", () => {
    const manifest = parseServerManifest(valid);
    expect(manifest).not.toBeNull();
    expect(manifest?.version).toBe("0.1.0");
    expect(manifest?.artifacts["aarch64-apple-darwin"]?.name).toBe("a.tar.gz");
  });

  it("accepts an empty artifacts object (no download possible, but well-formed)", () => {
    const manifest = parseServerManifest(JSON.stringify({ version: "0.1.0", artifacts: {} }));
    expect(manifest).toEqual({ version: "0.1.0", artifacts: {} });
  });

  it("returns null for non-JSON text", () => {
    expect(parseServerManifest("not json {")).toBeNull();
  });

  it("returns null for a JSON scalar or array", () => {
    expect(parseServerManifest("42")).toBeNull();
    expect(parseServerManifest("null")).toBeNull();
    expect(parseServerManifest("[]")).toBeNull();
  });

  it("returns null when version is missing or blank", () => {
    expect(parseServerManifest(JSON.stringify({ artifacts: {} }))).toBeNull();
    expect(parseServerManifest(JSON.stringify({ version: "", artifacts: {} }))).toBeNull();
    expect(parseServerManifest(JSON.stringify({ version: 1, artifacts: {} }))).toBeNull();
  });

  it("returns null when artifacts is missing or not a plain object", () => {
    expect(parseServerManifest(JSON.stringify({ version: "0.1.0" }))).toBeNull();
    expect(parseServerManifest(JSON.stringify({ version: "0.1.0", artifacts: [] }))).toBeNull();
    expect(parseServerManifest(JSON.stringify({ version: "0.1.0", artifacts: null }))).toBeNull();
    expect(parseServerManifest(JSON.stringify({ version: "0.1.0", artifacts: "x" }))).toBeNull();
  });
});

describe("checkServerInfoCompatibility", () => {
  it("reports a compatible version", () => {
    const check = checkServerInfoCompatibility({ name: "compact-analyzer", version: "0.1.9" }, "0.1.0");
    expect(check).toEqual({ kind: "compatible", version: "0.1.9" });
  });

  it("reports an incompatible version with an actionable message", () => {
    const check = checkServerInfoCompatibility({ name: "compact-analyzer", version: "0.2.0" }, "0.1.0");
    expect(check.kind).toBe("incompatible");
    if (check.kind === "incompatible") {
      expect(check.version).toBe("0.2.0");
      expect(check.message).toContain("0.2.0");
      expect(check.message).toContain("0.1.0");
      expect(check.message).toContain("Restart Server");
    }
  });

  it("treats a missing serverInfo or missing/blank version as unknown (not a failure)", () => {
    expect(checkServerInfoCompatibility(undefined, "0.1.0")).toEqual({ kind: "unknown" });
    expect(checkServerInfoCompatibility({ name: "compact-analyzer" }, "0.1.0")).toEqual({
      kind: "unknown",
    });
    expect(checkServerInfoCompatibility({ version: "" }, "0.1.0")).toEqual({ kind: "unknown" });
  });
});

describe("shouldOfferCoexistenceHint", () => {
  it("offers only when the old extension is present and the hint was never shown", () => {
    expect(shouldOfferCoexistenceHint(true, false)).toBe(true);
    expect(shouldOfferCoexistenceHint(true, true)).toBe(false);
    expect(shouldOfferCoexistenceHint(false, false)).toBe(false);
    expect(shouldOfferCoexistenceHint(false, true)).toBe(false);
  });
});

describe("formatAcquireFailureMessage", () => {
  it("combines the reason and the user guidance into one message", () => {
    const message = formatAcquireFailureMessage({
      reason: "No server was found.",
      userGuidance: "Install it manually.",
    });
    expect(message).toContain("No server was found.");
    expect(message).toContain("Install it manually.");
    expect(message).toContain("Compact Analyzer could not start");
  });
});
