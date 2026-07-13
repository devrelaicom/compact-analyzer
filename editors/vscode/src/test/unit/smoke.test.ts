import { describe, expect, it } from 'vitest';

// Smoke test: proves the vitest runner is wired up and executing. Real unit
// tests arrive in later tasks alongside the language-client implementation.
describe('vitest runner', () => {
  it('executes arithmetic', () => {
    expect(1 + 1).toBe(2);
  });
});
