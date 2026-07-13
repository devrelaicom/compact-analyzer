// @vscode/test-cli configuration for the activation smoke E2E (Task 10).
//
// The harness runs Mocha over the COMPILED test JavaScript (emitted to `out/e2e`
// by `src/test/e2e/tsconfig.json`) inside a real VS Code extension host. Before
// the host launches we inject the locally built server binary into the fixture
// workspace's `.vscode/settings.json` as `compact-analyzer.serverPath`, so the
// extension's acquire uses the settings source and the run stays hermetic — it
// never reaches the network download leg. The generated settings file is
// git-ignored (it holds a machine-specific absolute path).

import { mkdirSync, existsSync, writeFileSync } from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

import { defineConfig } from "@vscode/test-cli";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..", "..");
const fixtureDir = path.join(here, "src", "test", "e2e", "fixture");

const serverBinaryName = process.platform === "win32" ? "compact-analyzer.exe" : "compact-analyzer";
const serverBinaryPath = path.join(repoRoot, "target", "debug", serverBinaryName);

if (!existsSync(serverBinaryPath)) {
  throw new Error(
    `Built server binary not found at ${serverBinaryPath}. ` +
      "Run `cargo build -p compact-analyzer` before `npm run test:e2e` (the E2E is hermetic and drives this binary).",
  );
}

// Inject the valid serverPath into the fixture workspace settings BEFORE launch.
// Scenario 2 rewrites this same setting in-place at runtime.
const settingsDir = path.join(fixtureDir, ".vscode");
mkdirSync(settingsDir, { recursive: true });
writeFileSync(
  path.join(settingsDir, "settings.json"),
  `${JSON.stringify({ "compact-analyzer.serverPath": serverBinaryPath }, null, 2)}\n`,
  "utf8",
);

// VS Code opens a Unix-domain control socket inside `--user-data-dir`, and
// `sun_path` caps the socket path near 103 chars. The default
// (`<extension>/.vscode-test/user-data/…-main.sock`) blows past that limit on a
// deeply nested checkout — including the CI workspace path — so we pin a short
// user-data dir. Windows uses named pipes (no length limit), so leave its
// default in place there.
const launchArgs = [];
if (process.platform !== "win32") {
  const userDataDir = "/tmp/compact-analyzer-e2e-ud";
  mkdirSync(userDataDir, { recursive: true });
  launchArgs.push(`--user-data-dir=${userDataDir}`);
}

export default defineConfig({
  label: "e2e",
  files: "out/e2e/**/*.test.js",
  extensionDevelopmentPath: here,
  workspaceFolder: fixtureDir,
  launchArgs,
  mocha: {
    // Room for the VS Code download, the LSP handshake, and the 30s diagnostic
    // poll; individual tests tighten this with their own `this.timeout(...)`.
    timeout: 120_000,
  },
});
