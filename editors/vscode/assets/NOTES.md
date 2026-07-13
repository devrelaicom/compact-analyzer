# Language-asset regeneration & audit notes

This directory holds development-only inputs and evidence for the runtime
language assets (`../language-configuration.json`,
`../syntaxes/compact.tmLanguage.json`, `../snippets/compact.code-snippets`).
Nothing here is packaged into the published VSIX (see `../.vscodeignore`).

## Asset provenance (OQ13 — two upstream repositories, distinct roles)

- **Asset source (this task):** `LFDT-Minokawa/compact` — the source monorepo
  that contains `editor-support/{vsc,vim}`. The grammar, language configuration
  and snippets are adapted from `editor-support/vsc/compact/` at the pinned
  commit `fdfc61cf8b1311ca9fc5f8d155e1017d483a8acd`. Attribution is recorded in
  `../THIRD-PARTY-NOTICES.md`.
- **Distribution repository (not used here):** `midnightntwrk/compact`
  ("Compact Releases") — the toolchain-distribution repo. It has **no**
  `editor-support/` directory and is the origin of toolchain install docs / CI
  used elsewhere in the milestone. It is recorded only to disambiguate it from
  the asset-source monorepo; it is **not** the source of any file in this task.

## `keywords-0.23.txt` generation

The keyword ground truth is the Compact lexer in the local `compactp` checkout
(commit `0ebc1ae9`). `keywords-0.23.txt` is generated deterministically from
the `keyword_or_ident` match arms in
`crates/compactp_lexer/src/lib.rs` (the single source that maps each keyword
string to its `_KW` `SyntaxKind`). Run from the repository root, with the
`compactp` checkout as a sibling directory:

```sh
LC_ALL=C grep -oE '"[A-Za-z]+" => SyntaxKind::[A-Z_]+' \
  ../compactp/crates/compactp_lexer/src/lib.rs \
  | grep -oE '"[A-Za-z]+"' | tr -d '"' | LC_ALL=C sort -u \
  > editors/vscode/assets/keywords-0.23.txt
```

This yields 42 keywords (34 control/structural + boolean literals, plus the 8
builtin-type keywords `Boolean Bytes Field Integer Opaque Uint Unsigned
Vector`). The command is line-number independent, so lexer line drift does not
break it. `src/test/unit/assets.test.ts` asserts every line of this file
appears (as a whole word) in a `match`/`begin`/`end` pattern of the grammar.

## Grammar audit (deltas vs upstream, for language 0.23)

The grammar keeps upstream's `scopeName` (`source.compact`) and all scope names
so existing Compact colour themes keep working. Alternations were updated:

- **Type list (`support.class.compact`)** now matches exactly the 8 builtin
  type keywords from `compactp_syntax::SyntaxKind`:
  `Boolean|Bytes|Field|Integer|Opaque|Uint|Unsigned|Vector`.
  - **Added:** `Unsigned`, `Integer` (0.23 builtin type keywords, absent
    upstream — required for keyword coverage).
  - **Removed:** `JubjubScalar`, `Secp256k1Base`, `Secp256k1Scalar` — these are
    not language builtins and are not in scope under a plain
    `import CompactStandardLibrary;`. The compiler (0.31.1, language 0.23)
    reports `unbound identifier JubjubScalar` for them, so they were phantom
    highlights and were dropped.
- **Control keywords (`keyword.control.compact`):** removed `emit` and
  `implements` — neither is a keyword in the 0.23 lexer.
- **Reserved words (`keyword.reserved.compact`):** the entire pattern was
  dropped. It listed JavaScript reserved words (`this await break case catch
  class continue debugger delete do extends finally function in instanceof null
  super switch throw try typeof var void while with yield implements interface
  package private protected public let static`), none of which are Compact 0.23
  keywords; highlighting them as reserved would mis-colour valid identifiers.
- Import keywords, boolean literals, strings, numbers and comments are carried
  verbatim.

## Snippet audit (bodies verified against language 0.23)

Every kept snippet was expanded with its default choices and compiled inside a
minimal valid contract using the installed toolchain (`compact` dev tool
0.5.1 → compiler 0.31.1, language version 0.23), via
`compact compile -- --skip-zk --vscode <file> <out>`. The upstream snippets are
v0.2.11-era; outcomes:

| Snippet | Outcome | Detail |
| --- | --- | --- |
| `pragma` | Fixed | `language_version` is now a fixed token (was a placeholder); default version `0.8.6` → `0.23`. |
| `ledger` | Fixed | Removed a stray literal tab in the body; `ledger x: Field;` verified. |
| `constructor` | Fixed | Dropped a trailing `;` that produced an invalid empty statement; empty-body constructor verified. |
| `circuit` | Kept | Kept the `export circuit`/`circuit` choice; gave `return` a default (`${6:x}`) so the default expansion compiles. |
| `witness` | Kept | `witness foo(x: Field): Field;` verified unchanged. |
| `enum` | Fixed | Variant names clarified to `Idle`/`Active`; `enum State { … }` verified. |
| `stdlib` (was `init`) | Fixed | **Syntax change:** `include "std";` → `import CompactStandardLibrary;`. The old `include "std"` form is not valid in 0.23. |
| `if` | Fixed | Simplified to a single `if (condition) { … }` body (dropped the always-`else`); empty-body form verified. |
| `map` | Kept | `map(fn, vector)` structure verified (`map(inc, v)` and lambda forms both compile). Dropped the stale `for` prefix alias. |
| `fold` | Kept | `fold(fn, base, vector)` structure verified. |
| `module` | Kept | `module M { … }` verified (empty body compiles). |
| `struct` | Fixed | Replaced an invalid empty-statement body with a real field (`${2:field}: ${3:Field}`); verified. |
| `assert` | Fixed | **Syntax change:** bare `assert cond "msg";` → call form `assert(condition, "message");`. The bare form is a parse error in 0.23 (`parse error: found "a" looking for "("`). |
| `new smart contract` (file template) | Rewritten | Upstream used `include "std";` and a `ledger { }` block (both invalid in 0.23). Rewritten to `pragma language_version 0.23;` + `import CompactStandardLibrary;` + `export ledger …` + an `export circuit` using `disclose(...)`; the default expansion compiles. |

No snippets were dropped — each was salvageable and now verified. Upstream
branding icons are not referenced by any snippet.

## Problem matchers — dropped (do NOT re-add a `problemMatchers` key)

Upstream contributes three problem matchers (`compactException`,
`compactInternal`, `compactCommandNotFound`). All three are dropped. The
`compactException` matcher's regexp is:

```
^Exception: (.*?) line (\d+), char (\d+): (.*)$
```

with `fileLocation: ["absolute"]`. The real compiler (0.31.1) emits a different
plain-stderr shape. Recompiling a file with the stale bare-`assert` syntax
(`compact compile -- --skip-zk <file>`, i.e. without `--vscode`) produces:

```
Exception: broken.compact line 4 char 10:
  parse error: found "a" looking for "("
```

The upstream regexp fails on all three counts:

1. It expects `line N, char C` **with a comma** after the line number; the real
   output has no comma (`line 4 char 10`).
2. It is single-line (anchored `^…$`); the real output puts the message on a
   **second, indented line**.
3. `fileLocation: ["absolute"]` expects an absolute path; the real output uses a
   **basename** (`broken.compact`).

(The `--vscode` compiler flag collapses the message onto one line but still
omits the comma and still emits a basename, so even that shape would not match.)

Problem matchers are also redundant here: server-side compile-on-save
diagnostics (LSP, delivered in M4) already surface compiler errors natively.
`src/test/unit/assets.test.ts` guards this decision by asserting `contributes`
has no `problemMatchers` key.
