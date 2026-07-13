import { defineConfig } from "vitest/config";

// The `npm test` gate runs ONLY the host-free unit suite. The activation E2E
// under `src/test/e2e/` uses Mocha inside a real VS Code host (via
// `@vscode/test-cli`) and would fail under Vitest — it imports `vscode` and
// relies on Mocha globals — so it is scoped out here. Restricting `include`
// (rather than adding to `exclude`) also keeps the compiled `out/e2e/*.test.js`
// out of Vitest's default `**/*.test.js` discovery.
export default defineConfig({
  test: {
    include: ["src/test/unit/**/*.test.ts"],
  },
});
