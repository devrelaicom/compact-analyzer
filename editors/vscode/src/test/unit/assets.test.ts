import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

// vitest is launched from the extension package root (`editors/vscode`) by the
// `test` npm script, so asset paths resolve relative to the current working
// directory. `import.meta` is avoided deliberately: the project type-checks the
// tests under a CommonJS module target where it is not permitted.
const extRoot = process.cwd();

const read = (relative: string): string => {
  const path = join(extRoot, relative);
  if (!existsSync(path)) {
    throw new Error(
      `asset not found: ${relative} (resolved from cwd ${extRoot}); run the tests from editors/vscode`,
    );
  }
  return readFileSync(path, 'utf8');
};

/**
 * Strip JSONC (line/block comments and trailing commas) to strict JSON.
 *
 * The scanner is string-aware: comment markers and commas that live inside a
 * JSON string (e.g. the `"//"` value in `language-configuration.json`) are
 * preserved. This lets the tests parse the comment-bearing configuration files
 * that VS Code itself reads as JSONC, without pulling in a parser dependency.
 */
function stripJsonc(input: string): string {
  let out = '';
  let inString = false;
  let i = 0;

  while (i < input.length) {
    const char = input[i];

    if (inString) {
      out += char;
      if (char === '\\') {
        // Preserve the escaped character verbatim.
        out += input[i + 1] ?? '';
        i += 2;
        continue;
      }
      if (char === '"') {
        inString = false;
      }
      i += 1;
      continue;
    }

    if (char === '"') {
      inString = true;
      out += char;
      i += 1;
      continue;
    }

    if (char === '/' && input[i + 1] === '/') {
      i += 2;
      while (i < input.length && input[i] !== '\n') {
        i += 1;
      }
      continue;
    }

    if (char === '/' && input[i + 1] === '*') {
      i += 2;
      while (i < input.length && !(input[i] === '*' && input[i + 1] === '/')) {
        i += 1;
      }
      i += 2;
      continue;
    }

    out += char;
    i += 1;
  }

  // Drop trailing commas that precede a closing brace/bracket. NB: this pass
  // runs over the whole assembled output, not only structural regions — the
  // string-aware scan above does NOT shield it, so a literal ",}" or ",]"
  // inside a string value would be rewritten too. That is safe here only
  // because no string value in the shipped asset files contains that byte
  // sequence. Fold this into the scan loop if that assumption ever breaks.
  return out.replace(/,(\s*[}\]])/g, '$1');
}

const parseJsonc = <T,>(text: string): T => JSON.parse(stripJsonc(text)) as T;

const escapeForRegExp = (value: string): string =>
  value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

type Grammar = {
  scopeName: string;
};

type Snippet = {
  body: string | string[];
};

const grammarPath = 'syntaxes/compact.tmLanguage.json';
const snippetsPath = 'snippets/compact.code-snippets';
const languageConfigPath = 'language-configuration.json';
const keywordsPath = 'assets/keywords-0.23.txt';

describe('language assets parse', () => {
  it('language-configuration.json parses as JSONC', () => {
    expect(() => parseJsonc(read(languageConfigPath))).not.toThrow();
  });

  it('compact.tmLanguage.json parses as JSON', () => {
    expect(() => JSON.parse(read(grammarPath))).not.toThrow();
  });

  it('compact.code-snippets parses as JSONC', () => {
    expect(() => parseJsonc(read(snippetsPath))).not.toThrow();
  });
});

describe('grammar', () => {
  it('declares scopeName "source.compact"', () => {
    const grammar = JSON.parse(read(grammarPath)) as Grammar;
    expect(grammar.scopeName).toBe('source.compact');
  });
});

describe('keyword coverage', () => {
  const keywords = read(keywordsPath)
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);

  // Collect every `match`/`begin`/`end` regexp string from the grammar so the
  // coverage check is a crude, drift-catching textual scan over the patterns.
  const patternStrings: string[] = [];
  const collectPatterns = (node: unknown): void => {
    if (Array.isArray(node)) {
      for (const child of node) {
        collectPatterns(child);
      }
      return;
    }
    if (node !== null && typeof node === 'object') {
      for (const [key, value] of Object.entries(node)) {
        if (
          (key === 'match' || key === 'begin' || key === 'end') &&
          typeof value === 'string'
        ) {
          patternStrings.push(value);
        } else {
          collectPatterns(value);
        }
      }
    }
  };
  collectPatterns(JSON.parse(read(grammarPath)));
  const haystack = patternStrings.join('\n');

  it('reads a non-empty keyword list', () => {
    expect(keywords.length).toBeGreaterThan(0);
  });

  it.each(keywords)('grammar match patterns cover keyword "%s"', (keyword) => {
    const wordPattern = new RegExp(`\\b${escapeForRegExp(keyword)}\\b`);
    expect(wordPattern.test(haystack)).toBe(true);
  });
});

describe('snippets', () => {
  const snippets = parseJsonc<Record<string, Snippet>>(read(snippetsPath));
  const entries = Object.entries(snippets);

  /**
   * Returns a description of the first malformed placeholder in `body`, or
   * `null` when every `$n` / `${n:…}` / `${n|…|}` placeholder is well-formed.
   */
  const placeholderError = (body: string): string | null => {
    let i = 0;
    while (i < body.length) {
      const char = body[i];
      if (char === '\\') {
        // Skip an escaped character (e.g. `\$` or `\}`).
        i += 2;
        continue;
      }
      if (char !== '$') {
        i += 1;
        continue;
      }

      const next = body[i + 1];
      if (next === '{') {
        // Find the brace that closes this placeholder.
        let depth = 0;
        let close = -1;
        for (let j = i + 1; j < body.length; j += 1) {
          const inner = body[j];
          if (inner === '\\') {
            j += 1;
            continue;
          }
          if (inner === '{') {
            depth += 1;
          } else if (inner === '}') {
            depth -= 1;
            if (depth === 0) {
              close = j;
              break;
            }
          }
        }
        if (close === -1) {
          return `unbalanced \${…} in ${JSON.stringify(body)}`;
        }
        const spec = body.slice(i + 2, close);
        // A tabstop index, optionally followed by `:default` or `|choices|`.
        if (!/^\d+(?::[\s\S]*|\|[^|]*\|)?$/.test(spec)) {
          return `malformed placeholder \${${spec}} in ${JSON.stringify(body)}`;
        }
        i = close + 1;
        continue;
      }

      if (next === undefined || !/\d/.test(next)) {
        return `stray '$' (unescaped, not a placeholder) in ${JSON.stringify(body)}`;
      }
      i += 1;
    }
    return null;
  };

  it('defines at least one snippet', () => {
    expect(entries.length).toBeGreaterThan(0);
  });

  it.each(entries)('snippet "%s" has a body', (_name, snippet) => {
    expect(snippet.body).toBeDefined();
  });

  it.each(entries)(
    'snippet "%s" has well-formed placeholders',
    (_name, snippet) => {
      const body = Array.isArray(snippet.body)
        ? snippet.body.join('\n')
        : snippet.body;
      expect(placeholderError(body)).toBeNull();
    },
  );
});

describe('manifest', () => {
  it('contributes no problemMatchers (upstream matchers were dropped)', () => {
    const pkg = JSON.parse(read('package.json')) as {
      contributes?: Record<string, unknown>;
    };
    expect(pkg.contributes).toBeDefined();
    expect(Object.keys(pkg.contributes ?? {})).not.toContain('problemMatchers');
  });
});
