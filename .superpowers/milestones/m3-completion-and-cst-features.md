# M3 — Completion + CST-derived features

## Goal

Make the analyzer feel like a modern IDE while typing: context-aware completion,
semantic highlighting, and structure features (folding, selection ranges) — all
derivable from the CST plus M2's resolver, with **no type checker**.

## Why

After M2, navigation works but typing assistance is absent. Completion is the highest
per-keystroke-value feature and the one the sunsetted official extension never had.
Everything in M3 is deliberately scoped to what syntax + name resolution can answer, so
it doesn't block on v2 type checking.

## Includes

- **Completion**, context-aware by cursor position:
  - keywords valid at the position (top level vs statement vs type position, etc.);
  - in-scope symbols via the M2 resolver (locals, params, generics, items, imports);
  - stdlib items when `CompactStandardLibrary` is imported;
  - struct fields inside struct literals (struct type is named in the literal — syntactic);
  - **ledger ADT methods** after `.` on a ledger field (`counter.` → `increment`, …).
    This is possible without types because ledger fields are explicitly typed at their
    declaration — read the declared ADT (`Counter`, `Cell`, `Map`, `Set`, `List`,
    `MerkleTree`, `HistoricMerkleTree`, `Kernel`) and offer its method surface. Requires
    curating a builtin ledger-ADT method table (names, signatures, docs) — verify
    against the compiler source at the pinned tag, not training data.
- **Semantic tokens:** syntax-level classification from the CST (keywords, types,
  circuits, witnesses, ledger fields, modules, params). Supersedes the TextMate grammar
  progressively once the extension exists (M5).
- **Folding ranges and selection ranges** derived from the CST.

## Excludes

- Signature help, code actions, inlay hints (v2 — they want real types).
- Type-aware completion ranking/filtering (v2).
- Postfix/snippet-style smart completions (revisit post-v1).
- Formatting (M4 — it shells out to the toolchain).

## Facts / constraints to honor

- Member completion beyond ledger ADTs (e.g. fields of a struct-typed local) needs type
  inference — explicitly out of scope; don't half-build it.
- The ledger ADT method surfaces changed across compiler releases upstream; the method
  table must be versioned alongside the stdlib stub (same refresh procedure, same
  language-version pinning).
- Completion must behave inside error-recovered (mid-keystroke) trees — this is where
  compactp's expression-recovery rough edge (silent progress-failure in `expr_bp`) may
  surface; fixes land upstream in compactp.
- `analyzer-ide` stays LSP-free: completions/tokens expressed in plain Rust types +
  byte offsets; only the binary maps to LSP (UTF-16, token legends, trigger characters).

## Dependencies / notes

- Needs M2a's resolver + ItemTree; benefits from M2b's workspace index (cross-file
  symbols in completion) but keyword/local/stdlib completion could proceed on M2a alone
  if reordering ever makes sense.
- Semantic tokens: decide full-document vs range vs delta support at planning time;
  start with full-document (simplest, fine for Compact-sized files).
