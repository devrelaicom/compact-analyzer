# compact-analyzer M3 — Completion + CST-derived Features — Design

**Date:** 2026-07-08
**Status:** Approved
**Author:** Aaron Bassett (with Claude)

Milestone-level design for **M3**. It extends the project design
(`docs/superpowers/specs/2026-07-06-compact-analyzer-design.md`) and the milestone
context (`.superpowers/milestones/m3-completion-and-cst-features.md`). It builds
directly on M2b (cross-file resolution + workspace index), merged to `main` at
commit `cbb5ba6` with 129 tests green.

## 1. Context

After M1/M2, the analyzer parses, diagnoses, indexes a workspace, and navigates
(goto / references / rename / hover / document-symbols / workspace-symbols) — but
gives **no typing assistance**. Completion is the highest per-keystroke-value
feature and the one the sunsetted official extension never had. M3 adds the
editor-while-typing surface — context-aware completion, semantic highlighting, and
structure features (folding, selection ranges) — everything derivable from the CST
plus M2's resolver, with **no type checker** (that is v2).

The defining constraint is that all of this must behave inside **error-recovered,
mid-keystroke trees**: a user requesting completion has, by definition, a
half-typed and often unparseable buffer. M3's design is shaped around what
compactp's real error-recovery output makes reliably available.

Two empirical findings (verified against `../compactp`, working tree at
`compactp-v0.1.0-beta.1-13-g0ebc1ae`; **re-confirm at the pinned tag during
planning** — see §9) anchor the approach:

- **Trailing-dot recovers cleanly** (`crates/compactp_parser/src/grammar/expressions.rs:176`):
  the postfix-`DOT` arm does `p.bump(DOT); p.expect(IDENT); …`, and `expect`
  emits a diagnostic *without* bumping when the following token is not an `IDENT`.
  So `counter.⎸` (nothing after the dot) still completes a `MEMBER_EXPR` holding
  `NAME_EXPR("counter") + DOT`, and `counter.incr⎸` keeps the partial `IDENT`
  child. The make-or-break case for ledger-method completion survives recovery.
- **The soft spot is the empty expression position.** `expr_bp` (expressions.rs:121)
  begins `let mut lhs = lhs(p)?;` — when no expression can start, `lhs` returns
  `None`, `expr()` consumes nothing, and no node is produced (no error either).
  This is the "silent progress-failure in `expr_bp`" the milestone context warns
  about: it bites empty/placeholder positions (`const x = ⎸`, an empty statement
  slot), not the dot cases. Those positions are classified from the *enclosing*
  node kind + the token to the left, and are exactly where a narrow
  synthetic-identifier fallback would land if the corpus proves it necessary.

## 2. Decisions (settled during brainstorming)

| Question | Decision |
|---|---|
| Context detection under error recovery | **Raw-tree classification** — classify the cursor from the token to its left plus enclosing CST node kinds, on the real recovered tree, reusing the resolver's "classify by parent kind" grain. A **narrow synthetic-identifier fallback** is reserved only for specific mid-keystroke shapes that empirical corpus testing proves the raw tree cannot classify. |
| In-scope symbol enumeration | **Shared enumeration core** — refactor `resolve.rs` so one ancestor walk with one visibility/shadowing ruleset drives both `resolve` (find one match) and completion (collect all bindings). Single source of truth; lives in `analyzer-core`. |
| Ledger-ADT method data representation | **Versioned JSON asset** (`assets/ledger_adts_0_23.json`), embedded via `include_str!` and parsed into a table — parallels the stdlib stub's language-version-in-filename, provenance header, and pin/refresh procedure. M6 automation regenerates a data file, not Rust source. |
| What the ledger-ADT table powers | **Completion + hover** — completion offers ADT methods with signature detail + doc; hover on a ledger-method call renders the same, handled as a contained special case in the hover feature (no new `Definition` variant → goto/references/rename untouched). Stays clear of signature-help (excluded). |
| Semantic tokens breadth | **Fully comprehensive** — every non-trivia token classified (identifiers by structural role, keywords, literals, operators, punctuation), full-document only. |
| Semantic-token modifiers | Minimal: `declaration` at definition-site name tokens, `defaultLibrary` for stdlib / ledger-builtin references. |
| Folding regions | Brace-delimited item/type bodies + the leading consecutive import group (`kind=imports`) + multi-line block comments (`kind=comment`). |
| Selection ranges | The CST ancestor chain at the position. |
| Completion trigger characters | **`.` only**; identifier-position completion fires on normal word-character typing and manual invoke. |
| Completion delivery | Full items in one shot; **no `completionItem/resolve`** round-trip; **no auto-import** (completion ⊆ what actually resolves). |
| Cancellation | **None** for M3 features — completion / tokens / folding / selection are fast per-file operations that never scan the workspace, unlike M2b's find-references / rename. |

## 3. Architecture

### 3.1 Module placement (established layering, applied)

- **`analyzer-core`** gains the *semantic queries*:
  - a **shared scope-enumeration** primitive (working name `scope_bindings_at`) —
    the single ancestor walk both `resolve_local_name` and completion drive;
  - a **completion query** producing plain `CompletionCandidate` values (label,
    a `SymbolKind`-derived kind, detail, doc) for a `FilePosition`;
  - the **ledger-ADT table** + a **receiver-typing** query (ledger field →
    declared ADT head → method surface);
  - a **token-role classification** primitive for semantic tokens.
- **`analyzer-ide`** gains thin *feature composers* returning plain Rust types
  (byte offsets + `FileId`, **zero `lsp-types`**): `completion`, `semantic_tokens`,
  `folding_ranges`, `selection_ranges`. Completion composes core's in-scope
  enumeration with position-valid keywords, struct-literal fields, ledger-ADT
  methods, and stdlib exports. Folding and selection are **pure CST walks that live
  in `analyzer-ide`** (like the existing `document_symbols`) — they need no core
  query and no cross-file state. This matches the existing goto/hover/references
  pattern (thin ide function over core queries).
- **`compact-analyzer` (binary)** gains handlers for `textDocument/completion`,
  `textDocument/semanticTokens/full`, `textDocument/foldingRange`, and
  `textDocument/selectionRange`; advertises the new capabilities (semantic-tokens
  **legend**, completion **trigger characters**); and performs all UTF-16 ↔ byte
  and LSP-type mapping. This is the ONLY crate that names `lsp-types`.

### 3.2 The shared enumeration core (the structural crux)

The resolver currently answers a point query — "does *this one name* resolve
here?" `resolve_local_name` (resolve.rs:720) walks ancestors and, at each scope
node (`BLOCK`, `FOR_STMT`, `LAMBDA_EXPR`, `CIRCUIT_DEF`, `CONSTRUCTOR_DEF`,
`STRUCT_DEF`, `TYPE_DECL`, `MODULE_DEF`), checks for a *match*, honoring subtle
position-visibility rules: a `const` is visible only after its declaration and the
last binding wins (shadowing); a `for` var only inside the loop body; a lambda
param only past the parameter list.

Completion needs the **inverse of the identical walk**: enumerate *all* bindings
visible at the cursor under the *same* rules (never offer a `const` declared after
the cursor). Duplicating those rules is the primary drift risk, so the walk is
refactored into one shared primitive:

```
fn scope_bindings_at(file, root, offset) -> Vec<Binding>
    // walk ancestors from the cursor's token; at each scope node collect
    // the bindings it introduces, applying the SAME position-visibility rules;
    // inner scopes precede outer ones in the returned order.
```

- `resolve_local_name(name)` becomes `scope_bindings_at(...).find(|b| b.name == name)`
  (first hit wins — inner scope, latest binding).
- Completion's local set is `scope_bindings_at(...)` de-duplicated by name with the
  first (innermost / latest) binding winning.

The file-scope half mirrors `resolve_in_file_scope`: completion collects
enclosing-module members, top-level items, included files' top-level declarations,
and imported names (stdlib exports when `CompactStandardLibrary` is imported;
identifier/prefix/selective import members). This deliberately yields exactly the
set the resolver *would* resolve — completion never offers a name that would fail
to resolve (no auto-import; see §7).

**Expected side effect, treated as a feature:** exercising the scope walk from many
new cursor positions is likely to surface latent resolver edges (M2b's Task-16
found one this way). This refactor keeps all 129 existing tests green; any
characterization test that "should pass" but fails is root-caused and fixed as a
real bug, never weakened.

### 3.3 Completion context classification

From a `FilePosition`, classify into a `CompletionContext`. The anchor is the
token to the left of the cursor plus the IDENT-or-partial at the cursor (reusing
`ident_at_offset`'s boundary rule so the classification agrees with the resolver's
anchoring — cf. the Errata-fixed `resolve_local_name` token-pick), then the
enclosing node kind:

- **Member / ledger-method** — cursor after `.` inside a `MEMBER_EXPR` / `CALL_EXPR`.
  Resolve the receiver: a `SymbolKind::Ledger` field → its ADT method surface (§4);
  an `Enum` item → its variants; anything else (struct-typed local, chained call)
  → **empty** (needs types → v2). This is where trailing-dot recovery matters.
- **Struct-literal field** — inside a `STRUCT_EXPR` brace list. Resolve the struct
  type (reusing `resolve_struct_literal_field`'s machinery), offer the struct's
  fields minus those already written.
- **Type position** — inside `TYPE_REF` / `TYPE_SIZE` / param & return type slots →
  in-scope types (structs, enums, type aliases, in-scope generics, builtins) plus
  type keywords (`Field`, `Boolean`, `Bytes`, `Uint`, `Vector`, `Opaque`, …).
- **Expression / statement position** — in-scope locals / params / generics +
  top-level & imported items + stdlib exports (when imported) + position-valid
  statement/expression keywords (`const`, `return`, `if`, `for`, `assert`,
  `disclose`, `map`, `fold`, `default`, `pad`, …).
- **Declaration / top-level position** — declaration keywords (`export`, `sealed`,
  `pure`, `circuit`, `witness`, `ledger`, `struct`, `enum`, `module`, `contract`,
  `constructor`, `type`, `import`, `include`, `pragma`).

Each candidate carries: `label`; a `CompletionItemKind` mapped from `SymbolKind`;
`detail` = the rendered signature (from `ItemTree::signature` or the ledger-ADT
table); `documentation` = the doc comment or table doc. The keyword→position map is
curated at plan time from compactp's grammar (`declarations.rs`, `statements.rs`,
`types.rs`, `expressions.rs`) and the `*_KW` token set — grounded, not recalled.

### 3.4 Error-recovery robustness

Completion classification runs on the real recovered tree. Robustness is proven by
**characterization tests over mid-keystroke fixtures** (trailing `.`, partial
idents, empty expression slots, half-written declarations). The empty-position soft
spot (§1) is classified from the enclosing node + left token. The
**synthetic-identifier fallback** — splice a sentinel `IDENT` at the cursor, reparse
that text, classify the well-formed tree, map ranges back — is implemented **only if**
corpus characterization shows specific shapes the raw tree cannot classify; it is a
scoped escape hatch, not the default path. Any compactp recovery gap that blocks a
needed context (rather than being worked around locally) is logged for an upstream
compactp fix, per the cross-cutting policy.

## 4. Ledger-ADT method table + receiver typing

The ledger ADTs — `Counter`, `Cell`, `Map`, `Set`, `List`, `MerkleTree`,
`HistoricMerkleTree`, `Kernel` — are **compiler builtins invoked with method syntax**
(`counter.increment(1)`), not Compact circuits. There is no Compact surface syntax
that declares "type `Counter` has method `increment`", so — unlike the stdlib stub —
they **cannot** be represented as a `.compact` file the parser reads. Hence a curated
data table.

- **Asset:** `crates/analyzer-core/assets/ledger_adts_0_23.json`, shape
  `{ ADT_name: [ { "name", "sig", "doc" }, … ] }`, embedded via `include_str!` and
  parsed once. Language version in the filename; a provenance header comment is not
  possible in JSON, so provenance + refresh procedure live in a Rust doc-comment on
  the loader module and in the milestone/M6 refresh notes. A test asserts it parses
  and covers all 8 ADTs.
- **Sourcing:** method names / signatures / docs are curated from the compiler
  source at the pinned tag (candidates on disk: `LFDT-Minokawa/compact`,
  `midnight-ledger`), **not** training data — same discipline as the M2a stdlib
  stub. This is a §9 plan-time verification item.
- **Receiver typing:** resolve the receiver `NAME_EXPR` → a `SymbolKind::Ledger`
  item → parse the declared ADT head from its `LedgerDecl` type node (`… : Map<K,V>`
  → `Map`; `… : Counter` → `Counter`) → table lookup by head name (type arguments do
  not change the method surface). The `kernel` stdlib ledger (`export ledger kernel:
  Kernel;`) resolves into the stub and maps to the `Kernel` surface.
- **One level deep, by design.** `map.lookup(k).⎸` has a `CALL_EXPR` receiver, not a
  ledger field, so it yields nothing — completing it would require `lookup`'s return
  type (type inference → v2). This boundary is asserted by a test.
- **Hover reuse:** hover on `counter.increment` detects the member-on-ledger-field
  shape, consults the table, and renders `` ```compact\n<sig>\n``` `` + doc — the same
  rendering shape as item hover. Implemented inside the hover feature; `resolve` and
  the `Definition` enum are unchanged (the current `ledger_adt_methods_resolve_to_none`
  test stays valid — resolve still returns `None`; hover no longer depends on resolve
  for this case).

## 5. Semantic tokens (comprehensive, full-document)

Every non-trivia token maps to `(token_type, modifier_set)`; trivia (whitespace) is
skipped. Classification is **structural first** (cheap, no resolution), with a
resolution refinement only for use-site identifiers:

- Keywords (`*_KW`, incl. `TRUE_KW` / `FALSE_KW`) → `keyword`.
- Literals: string → `string`; numeric → `number`.
- Operators (`+ - * == != < <= > >= && || ! => ? :` etc.) → `operator`.
- Punctuation / delimiters (`{ } ( ) [ ] , ; :` and the `.`) → a **custom
  `punctuation`** legend type (no standard `SemanticTokenType` exists for these; the
  M5 extension themes it, stock themes ignore unknown types and fall back to
  TextMate). `<` / `>` are classified by structural context (type-argument list vs
  relational operator).
- Definition-site name tokens → `function` / `method` / `struct` / `enum` /
  `namespace` / `typeParameter` / `property` (per the item kind) **+ `declaration`**.
- Params → `parameter`; generic params → `typeParameter`; local const / for / lambda
  bindings → `variable`; type-reference idents → `type`; struct fields & ledger
  fields → `property`; enum variants → `enumMember`; module names → `namespace`.
- Use-site `NAME_EXPR` / callee idents → resolve to refine role (circuit →
  `function`, local/param → `variable`/`parameter`, ledger → `property`, enum →
  `enum`) and add **`defaultLibrary`** when they resolve into the stdlib stub or name
  a ledger builtin. Resolution per identifier is acceptable at Compact file sizes;
  classify structurally wherever the parent kind already determines the role.

The **legend** (ordered token-type list + modifier list) and the token→index mapping
are declared once at the binary boundary. Full-document (`semanticTokens/full`) only;
**no range or delta** support in M3 (revisit only if a real file proves slow). Exact
`SemanticTokenType` / `SemanticTokenModifier` availability and the custom-type
mechanism are verified against `lsp-types 0.95.1` at spec/plan time (§9).

## 6. Folding + selection ranges

- **Folding ranges** — CST-derived `startLine`→`endLine` for: brace-delimited bodies
  (circuit / witness / constructor / contract / module / struct / enum); the leading
  consecutive `import` run (LSP `kind = imports`); and multi-line block comments
  (`kind = comment`). One node → at most one range; single-line constructs produce
  none.
- **Selection ranges** — for each requested position, walk the CST ancestor chain
  from the token upward (token → enclosing expression → statement → block → item →
  file), emitting each node's range as a nested `parent` link. Pure structure, no
  resolution.

Both are per-file, fast, and never scan the workspace.

## 7. Scope: what completion does *not* do

- **No auto-import.** Completion offers only names that already resolve at the cursor
  (in-scope locals/items/imports/stdlib). Surfacing an unimported workspace symbol
  and inserting its `import` is a code action — excluded from M3 (and code actions as
  a whole are v2).
- **No member completion beyond ledger ADTs and enum variants.** Fields/methods of a
  struct-typed local, or of a chained expression, need type inference → v2.
- **No signature help, inlay hints, type-aware ranking, postfix/snippets,
  formatting.** Per the milestone exclusions; not half-built.

## 8. Testing

- **Fixture unit tests** (rust-analyzer `$0`-marker convention) per completion
  context (member/ledger-method, struct-literal field, type, expression, declaration),
  per semantic-token classification, per folding region, and per selection chain.
  Each test is written to **actually fail if the feature regresses** — proven by
  neutering the feature where a test's guarding is in doubt (M2b Errata E5 lesson).
  Mid-keystroke / error-recovered fixtures are first-class, not afterthoughts.
- **Characterization tests for the scope-enumeration refactor** — all 129 existing
  tests stay green; new tests lock in enumeration at positions the resolver already
  handles. Any "should pass" failure is a real resolver bug, root-caused and fixed
  (M2b Task-16 precedent), and recorded in the plan's Errata.
- **Corpus smoke, extended** — run completion / semantic-tokens / folding / selection
  at sampled positions across compactp's ~486-file corpus, asserting no panics and no
  out-of-bounds spans, including inside error-recovered trees. Extends the M2b corpus
  test rather than adding a parallel one.
- **LSP integration** — black-box tests driving the real binary over stdio for
  `textDocument/completion` (including `.`-triggered ledger-method completion),
  `semanticTokens/full` (legend + token stream), `foldingRange`, and `selectionRange`.

## 9. Plan-time verification items

Precedent: the M1 plan carried 5 real bugs, M2a's 3, M2b's 5 — all caught only by
empirical checks; and training data about Compact/compactp is explicitly unreliable.
Every fixture and API fact in the plan is pre-verified against `../compactp` **at the
pinned tag `0.1.0-beta.1`** (the working tree is currently 13 commits ahead — pin
before trusting) and against the compiler source at the corresponding tag. The
following are discharged during planning and their verified forms recorded in the
plan's Global Constraints:

- **Ledger-ADT method surfaces** — the names, signatures, and docs for all 8 ADTs,
  curated from the compiler source at the pinned tag (not memory). Sole source of the
  `ledger_adts_0_23.json` asset.
- **compactp AST accessors** — `LedgerDecl` declared-type accessor (to read the ADT
  head), `StructExpr` name/fields, `MemberExpr` receiver/name, `NameExpr::ident`,
  `Block::stmts`, and the concrete `SyntaxKind`s for each completion position context.
- **Keyword → position map** — grounded in `declarations.rs` / `statements.rs` /
  `types.rs` / `expressions.rs` and the `*_KW` token set, per position context.
- **Error-recovery shapes** — confirm at the pinned tag which mid-keystroke shapes
  the raw tree classifies (trailing dot / partial ident confirmed on the ahead-of-tag
  tree; re-confirm) and which, if any, require the synthetic-identifier fallback.
- **`lsp-types 0.95.1` surface** — `SemanticTokens` / `SemanticTokensLegend` /
  `SemanticTokenType` / `SemanticTokenModifier` and how to express a custom
  punctuation type; `SemanticTokensServerCapabilities` (`full` only);
  `FoldingRange` (+ `FoldingRangeKind`); `SelectionRange`; `CompletionItem` /
  `CompletionItemKind` / `CompletionResponse`; `CompletionOptions.trigger_characters`.

Any further fixture or API fact the plan introduces is still verified at
implementation time per its per-task NOTES, as in M1/M2a/M2b.

## 10. Invariants preserved

`lsp-types` appears only in the `compact-analyzer` binary; `analyzer-core` and
`analyzer-ide` speak byte offsets (`TextSize` / `TextRange`) + `FileId` + plain Rust
types — completions, semantic tokens, folding ranges, and selection ranges are all
expressed this way; UTF-16 ↔ byte conversion, the semantic-tokens legend, and
completion trigger characters live only at the binary boundary; `lsp-types` stays
pinned `0.95.1`. The server never dies on malformed / adversarial / mid-keystroke
input: completion / tokens / folding / selection answer null-or-empty on unresolvable
or unparseable positions, under the existing per-request `catch_unwind`; any new bulk
operation (e.g. an extended corpus pass) is guarded like M2b's crawl. stdout is
protocol-only (logs → stderr). `compactp_*` stays pinned `0.1.0-beta.1`; **no
`[patch.crates-io]` is ever committed** (an uncommitted patch at `../compactp` is fine
for iteration). Rust edition 2024, rust-version 1.90; the tooling gate — `cargo fmt`
+ `cargo clippy --workspace --all-targets --locked -- -D warnings` + tests — is green
per task. The binary is bin-only (`cargo test -p compact-analyzer`, never `--lib`); a
new dependency's first build omits `--locked`, subsequent gates restore it;
forward-declared struct fields use `#[allow(dead_code)]`, never `#[expect(dead_code)]`.
Conventional commits (`fix:` for correctness fixes).

## 11. Out of scope (unchanged from the milestone context)

Signature help, code actions, inlay hints, auto-import, type-aware completion
ranking/filtering, member completion beyond ledger ADTs (needs type inference → v2),
postfix/snippet-style completions (revisit post-v1), and formatting (M4 — it shells
out to the toolchain). Semantic-token range/delta support (full-document is
sufficient for Compact-sized files).
