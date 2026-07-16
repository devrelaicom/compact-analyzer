import { describe, it, expect } from "vitest";
import { buildInitOptions, readServerPath, type ConfigReader } from "../../config-core";

/**
 * Builds a fake `ConfigReader` from a plain settings record. Any key not in the
 * record reads back as `undefined`, mirroring an unset `compact-analyzer.*`
 * setting. This keeps the mapping testable under plain vitest — no extension
 * host, no `vscode` module.
 */
function fakeReader(settings: Record<string, unknown>): ConfigReader {
  return <T,>(key: string): T | undefined => (key in settings ? (settings[key] as T) : undefined);
}

describe("buildInitOptions", () => {
  it("(a) at defaults sends EXACTLY compileOnSave + formatting + typeDiagnostics", () => {
    const result = buildInitOptions(fakeReader({}));
    // importSearchPath/toolchainPath must be ABSENT, not present-with-empty.
    expect(Object.keys(result).sort()).toEqual(["compileOnSave", "formatting", "typeDiagnostics"]);
    expect(result).toEqual({ compileOnSave: true, formatting: true, typeDiagnostics: true });
  });

  it("(b) passes a populated importSearchPath and toolchainPath through", () => {
    const result = buildInitOptions(
      fakeReader({ importSearchPath: ["a", "b"], toolchainPath: "/x" }),
    );
    expect(result).toEqual({
      importSearchPath: ["a", "b"],
      toolchainPath: "/x",
      compileOnSave: true,
      formatting: true,
      typeDiagnostics: true,
    });
  });

  it("(c) omits an empty importSearchPath (G2) and a blank toolchainPath", () => {
    const result = buildInitOptions(fakeReader({ importSearchPath: [], toolchainPath: "" }));
    // G2: an explicit `importSearchPath: []` would suppress the server's
    // COMPACT_PATH fallback, so an empty array must NOT be sent at all.
    expect(Object.keys(result).sort()).toEqual(["compileOnSave", "formatting", "typeDiagnostics"]);
    expect(result.importSearchPath).toBeUndefined();
    expect(result.toolchainPath).toBeUndefined();
  });

  it("(d) sends non-default booleans through as false", () => {
    const result = buildInitOptions(fakeReader({ compileOnSave: false, formatting: false }));
    expect(result.compileOnSave).toBe(false);
    expect(result.formatting).toBe(false);
  });

  // Shape-guards: mistyped settings must never throw at the activation path and
  // must never forward a wrong-typed value to the server.
  it("omits importSearchPath when it reads back as null (no throw)", () => {
    const result = buildInitOptions(fakeReader({ importSearchPath: null }));
    expect(Object.keys(result).sort()).toEqual(["compileOnSave", "formatting", "typeDiagnostics"]);
    expect(result.importSearchPath).toBeUndefined();
  });

  it("omits importSearchPath when it is a bare string, not an array", () => {
    const result = buildInitOptions(fakeReader({ importSearchPath: "not-an-array" }));
    expect(result.importSearchPath).toBeUndefined();
  });

  it("drops non-string members of importSearchPath", () => {
    const result = buildInitOptions(fakeReader({ importSearchPath: ["a", 1, null, "b"] }));
    expect(result.importSearchPath).toEqual(["a", "b"]);
  });

  it("omits a non-string toolchainPath", () => {
    const result = buildInitOptions(fakeReader({ toolchainPath: 123 }));
    expect(result.toolchainPath).toBeUndefined();
  });

  it("defaults a non-boolean compileOnSave to true", () => {
    const result = buildInitOptions(fakeReader({ compileOnSave: "yes" }));
    expect(result.compileOnSave).toBe(true);
  });

  it("preserves a configured typeDiagnostics=false", () => {
    const result = buildInitOptions(fakeReader({ typeDiagnostics: false }));
    expect(result.typeDiagnostics).toBe(false);
  });

  it("defaults a non-boolean typeDiagnostics to true", () => {
    const result = buildInitOptions(fakeReader({ typeDiagnostics: "yes" }));
    expect(result.typeDiagnostics).toBe(true);
  });
});

describe("readServerPath", () => {
  it("(e) returns the configured path, or undefined when blank/unset", () => {
    expect(readServerPath(fakeReader({ serverPath: "/opt/bin/compact-analyzer" }))).toBe(
      "/opt/bin/compact-analyzer",
    );
    expect(readServerPath(fakeReader({ serverPath: "" }))).toBeUndefined();
    expect(readServerPath(fakeReader({}))).toBeUndefined();
  });

  it("returns undefined for a mistyped (non-string) serverPath", () => {
    expect(readServerPath(fakeReader({ serverPath: 42 }))).toBeUndefined();
    expect(readServerPath(fakeReader({ serverPath: null }))).toBeUndefined();
  });
});
