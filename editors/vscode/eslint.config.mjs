// Flat ESLint config using the typescript-eslint recommended (non
// type-checked) ruleset. Scoped to the TypeScript sources under src/; the
// bundle output and dependencies are ignored.
import tseslint from 'typescript-eslint';

export default tseslint.config(
  {
    ignores: ['dist/', 'node_modules/'],
  },
  {
    files: ['**/*.ts'],
    extends: [tseslint.configs.recommended],
  },
);
