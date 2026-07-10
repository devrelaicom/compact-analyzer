# M3 — Completion + CST-derived Features Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add context-aware completion, fully-comprehensive semantic tokens, folding ranges, and selection ranges to compact-analyzer — all derived from the CST plus M2's resolver, with no type checker.

**Architecture:** A shared scope-enumeration primitive in `analyzer-core` powers both the resolver (find one name) and completion (enumerate all in-scope names). A curated, version-pinned JSON table supplies ledger-ADT method surfaces for completion + hover. Four new thin feature modules in `analyzer-ide` (`completion`, `semantic_tokens`, `folding_ranges`, `selection_ranges`) return plain byte-offset types; the `compact-analyzer` binary maps them to LSP (legend, trigger characters, UTF-16). The design spec is `docs/superpowers/specs/2026-07-08-m3-completion-and-cst-features-design.md`.

**Tech Stack:** Rust (edition 2024, rust-version 1.90); `compactp_{syntax,parser,ast,diagnostics}` 0.1.0-beta.1; `rowan`/`text_size`; `lsp-types` 0.95.1 and `lsp-server` (binary only); `serde`/`serde_json` (new in `analyzer-core`, for the ledger-ADT asset).

## Global Constraints

Every task's requirements implicitly include this section. Exact values, copied from the spec — **do not re-derive**.

- **Layering:** `lsp-types` appears ONLY in the `compact-analyzer` binary. `analyzer-core` and `analyzer-ide` speak byte offsets (`TextSize`/`TextRange`) + `FileId` + plain Rust types. Completions, semantic tokens, folding ranges, and selection ranges are all expressed in plain types; UTF-16↔byte conversion, the semantic-tokens legend, and completion trigger characters live only at the binary boundary.
- **Never-die:** the server must not panic the session on malformed/adversarial/mid-keystroke input. Per-request `catch_unwind` (already in `dispatch`). Completion / tokens / folding / selection answer null-or-empty on unresolvable or unparseable positions. They are fast per-file operations — **no workspace scan, no cancellation machinery** (unlike M2b find-references/rename). Any new bulk op (e.g. the extended corpus pass) is guarded like M2b's crawl.
- **Pinned versions:** `lsp-types` = `0.95.1` (0.96+ drops `url::Url`). `compactp_*` = `0.1.0-beta.1`. **Never commit a `[patch.crates-io]`** (an uncommitted patch at `../compactp` is fine for iteration).
- **Tooling gate, per task:** `cargo fmt`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and tests, all green. Conventional commits (`fix:` for correctness fixes, not `test:`/`feat:`).
- **Bin-only binary:** test it with `cargo test -p compact-analyzer` (NOT `--lib` — it runs nothing). When adding a new dependency, the **first** build must NOT use `--locked` (it needs to update `Cargo.lock`); subsequent gates restore `--locked`. Forward-declared struct fields use `#[allow(dead_code)]`, never `#[expect(dead_code)]`.
- **Stdout is protocol-only** (logs → stderr).

---

## Empirically-Verified Facts (do not re-derive)

All verified against `../compactp` (working tree `compactp-v0.1.0-beta.1-13-g0ebc1ae`; the 13 commits since the tag touch only `/site` and CI — the `crates/compactp_{ast,syntax,parser,lexer}` diff against the tag is empty, so the checkout is byte-identical to the pinned `0.1.0-beta.1` the analyzer builds against) and `lsp-types` 0.95.1 (`~/.cargo/registry/src/*/lsp-types-0.95.1/`), on 2026-07-08.

### F1 — `SyntaxKind` surface (`compactp_syntax/src/syntax_kind.rs`)

- **Keyword tokens** (all `*_KW`): `PRAGMA_KW INCLUDE_KW IMPORT_KW FROM_KW PREFIX_KW EXPORT_KW MODULE_KW LEDGER_KW CONSTRUCTOR_KW CIRCUIT_KW WITNESS_KW CONTRACT_KW STRUCT_KW ENUM_KW TYPE_KW CONST_KW RETURN_KW IF_KW ELSE_KW FOR_KW OF_KW ASSERT_KW AS_KW PURE_KW SEALED_KW NEW_KW MAP_KW FOLD_KW DEFAULT_KW DISCLOSE_KW PAD_KW SLICE_KW`. **Builtin-type keywords:** `BOOLEAN_KW FIELD_KW UINT_KW BYTES_KW OPAQUE_KW VECTOR_KW UNSIGNED_KW INTEGER_KW`. **Boolean literals** are keywords too: `TRUE_KW FALSE_KW`.
- **Literal tokens:** `INT_LIT HEX_LIT OCT_LIT BIN_LIT STRING_LIT VERSION_LIT`.
- **Operator tokens:** `EQ PLUS_EQ MINUS_EQ EQ_EQ BANG_EQ LT LT_EQ GT GT_EQ AMP_AMP PIPE_PIPE PLUS MINUS STAR SLASH BANG QUESTION FAT_ARROW DOT DOT_DOT DOT_DOT_DOT`.
- **Delimiter/punctuation tokens:** `L_PAREN R_PAREN L_BRACE R_BRACE L_BRACKET R_BRACKET COMMA SEMICOLON COLON HASH`.
- **Special:** `IDENT ERROR EOF`; trivia `WHITESPACE LINE_COMMENT BLOCK_COMMENT` (`SyntaxKind::is_trivia()` covers exactly these three).
- **Type nodes:** `TYPE_REF BOOLEAN_TYPE FIELD_TYPE UINT_TYPE UNSIGNED_INTEGER_TYPE BYTES_TYPE OPAQUE_TYPE VECTOR_TYPE TUPLE_TYPE RECORD_TYPE GENERIC_ARG_LIST GENERIC_ARG GENERIC_PARAM_LIST GENERIC_PARAM TYPE_SIZE`.
- **Statement nodes:** `BLOCK ASSIGN_STMT EXPR_STMT RETURN_STMT IF_STMT FOR_STMT ASSERT_STMT CONST_STMT MULTI_CONST_STMT` (+ expr-context `ASSIGN_EXPR COMPOUND_ASSIGN_EXPR`).
- **Pattern nodes:** `IDENT_PAT TUPLE_PAT TUPLE_PAT_ELT STRUCT_PAT STRUCT_PAT_FIELD TYPED_PAT`.
- **Expr nodes:** `LITERAL_EXPR NAME_EXPR TERNARY_EXPR BINARY_EXPR UNARY_EXPR CAST_EXPR CALL_EXPR MEMBER_EXPR INDEX_EXPR PAREN_EXPR EXPR_SEQ ARRAY_EXPR BYTES_EXPR SPREAD_EXPR STRUCT_EXPR STRUCT_FIELD_INIT STRUCT_UPDATE DEFAULT_EXPR MAP_EXPR FOLD_EXPR DISCLOSE_EXPR PAD_EXPR SLICE_EXPR LAMBDA_EXPR PARAM_LIST PARAM RANGE_EXPR PREFIX_DECL NAMED_ARG`.

### F2 — AST accessors (`compactp_ast`)

- `LedgerDecl::ty() -> Option<Type>`, `::name()`, `::is_exported()`, `::sealed_kw()` (nodes.rs:230). The declared ADT type is a `Type`.
- `Type` enum (nodes.rs:749): `Ref(TypeRef) | Boolean | Field | Uint | Bytes | Opaque | Vector | Tuple | UnsignedInteger | Record`. All ledger ADT types (`Counter`, `Cell`, `Map`, `Set`, `List`, `MerkleTree`, `HistoricMerkleTree`, `Kernel`) are `Type::Ref(TypeRef)`.
- `TypeRef::name() -> Option<SyntaxToken>` (the head IDENT), `TypeRef::generic_args() -> Option<GenericArgList>` (nodes.rs:829). **The ADT head is `TypeRef::name()`.**
- `NameExpr::ident() -> Option<SyntaxToken>` (expr.rs:197). `MemberExpr::field() -> Option<SyntaxToken>` (the field IDENT; expr.rs:294) — **the receiver has no accessor: it is the child expr node** (walk `member.children()` for the receiver, as `resolve_member` already does). `CallExpr::name() -> Option<SyntaxToken>` (method/callee IDENT), `CallExpr::generic_args()` (expr.rs:277).
- `StructExpr::name() -> Option<SyntaxToken>`, `::field_inits() -> impl Iterator<Item = StructFieldInit>`, `::update() -> Option<StructUpdate>` (expr.rs:324). `StructFieldInit::name() -> Option<SyntaxToken>` (expr.rs:346).
- `ConstStmt::pattern() -> Option<Pat>`, `::ty()`, `::value()` (nodes.rs:1117). `Pat` enum: `Ident(IdentPat)|Tuple(TuplePat)|Struct(StructPat)`; `IdentPat::name()`; `TuplePat::elements() -> TuplePatElt`; `TuplePatElt::pattern()`; `StructPat` fields (as `pattern_bindings` in resolve.rs already handles).
- `ForStmt::var_name()`, `::body() -> Option<Block>` (nodes.rs:1178). `Block::stmts() -> impl Iterator<Item = Stmt>`. `Stmt` enum (nodes.rs:1013): `Const(ConstStmt) | MultiConst(MultiConstStmt) | Expr | Return | If | For | Assert | Assign | Block`.
- `LambdaExpr::param_list()`, `::return_type()`, `::body_block()` (expr.rs:407). Circuit/constructor `params()`, `generic_params()` accessors are already used by resolve.rs.
- `SourceFile::items()`, `::imports()`, `::includes()`; `Import::{name,path,prefix}`, `ImportSpecifier::name`, `PrefixDecl::name` — all already used by resolve.rs/item_tree.rs.

### F3 — `MULTI_CONST_STMT` is a real, currently-mishandled construct (latent resolver bug)

`const_stmt` (parser statements.rs:156-174) emits **`MULTI_CONST_STMT`** for a comma-separated multi-binding `const a = 1, b = 2;` and `CONST_STMT` otherwise. The `MultiConstStmt` AST node (nodes.rs:1134) has NO accessors ("reserved"). The resolver's `resolve_local_name` BLOCK arm matches only `Stmt::Const`, so **bindings introduced by `MULTI_CONST_STMT` are invisible to resolution today** (goto/hover/references miss them). A single `const [a, b] = pair;` is a `CONST_STMT` holding a `TUPLE_PAT` (NOT multi-const), and already resolves. The shared scope-enumeration (Task 1) fixes multi-const for resolver + completion together, and Task 1 adds a characterization test proving it.

### F4 — Error-recovered CST shapes at completion positions (from `compactp cst`, verified 2026-07-08)

The parser recovers these mid-keystroke shapes (cursor `⎸` marked; offsets are illustrative):

- **Trailing dot** `c.⎸` → `MEMBER_EXPR(NAME_EXPR("c"), DOT)`; a following `;` is a sibling under `EXPR_STMT`. The receiver `NAME_EXPR` and `DOT` are present; there is no IDENT after the dot.
- **Partial member** `c.incr⎸` → `MEMBER_EXPR(NAME_EXPR("c"), DOT, IDENT("incr"))`.
- **Method-call member** `c.foo(…)` → `CALL_EXPR` (the `.id(args)` postfix arm completes a `CALL_EXPR`, not `MEMBER_EXPR`).
- **Call shapes** (verified 2026-07-08): a **direct call** `foo(1)` → `CALL_EXPR(IDENT "foo", <args>)` — the callee is a **direct `IDENT` child**, NOT wrapped in `NAME_EXPR`. A **method call** `bar.baz(2)` → `CALL_EXPR(NAME_EXPR "bar", DOT, IDENT "baz", <args>)`. Therefore, for an `IDENT` whose parent is `CALL_EXPR`: a `DOT` **before** it ⇒ method name (`Method`); no `DOT` ⇒ direct callee (`Function`). The receiver in a method call/member is the first `Expr`-castable child node (a `NAME_EXPR` for a simple receiver).
- **Struct-literal open** `Point {⎸ }` → `STRUCT_EXPR(IDENT("Point"), L_BRACE, R_BRACE)` (no field inits yet).
- **Empty expression position** `const x = ⎸;` → `CONST_STMT(CONST_KW, IDENT_PAT, EQ, SEMICOLON)` — **no expression node exists** (the `expr_bp` silent-no-progress soft spot). Classify from the enclosing node (`CONST_STMT`) + the token to the left (`EQ`).
- **Partial expression** `return pe⎸;` → `RETURN_STMT(RETURN_KW, NAME_EXPR(IDENT("pe")), SEMICOLON)`.

**Classifier rule:** anchor on the IDENT at the cursor via the existing `ident_at_offset` boundary pick when present (partial-ident cases); otherwise anchor on the first non-trivia token to the left of the cursor. Then walk that token's parent/ancestors, classifying by `(node kind, left-token kind)`.

### F5 — Keyword → position map (from parser dispatchers)

- **Declaration position** (top-level or module body; `declarations.rs::declaration`): `pragma include import export module ledger sealed constructor circuit witness contract struct enum type new pure`. (`export`/`sealed`/`pure`/`new` are prefixes: `export {…|module|circuit|witness|contract|struct|enum|ledger|type|new type|pure|sealed ledger}`; `sealed ledger`; `pure circuit`; `new type`.)
- **Statement position** (block body; `statements.rs::stmt_inner`): `const return if for contract` + every expression-start keyword (a statement may be an expression statement). `else` only follows an `if` block.
- **Expression position** (`expressions.rs::lhs`): `map fold default disclose pad slice assert true false` + identifiers/literals. (`!` unary, `(`, `[` start exprs too but are not keywords.)
- **Type position** (`types.rs`): `Boolean Field Uint Bytes Opaque Vector Unsigned Integer` + in-scope type names + in-scope generics.

Note: `assert` parses as an expression producing `CALL_EXPR` (via `lhs`), not a dedicated statement, in this grammar; `ASSERT_STMT` is legacy/unused by `stmt_inner`. Offer `assert` in statement/expression position.

### F6 — `lsp-types` 0.95.1 surface (all present; `~/.cargo/registry/src/*/lsp-types-0.95.1/`)

- **Semantic tokens** (`src/semantic_tokens.rs`): `SemanticToken { delta_line, delta_start, length, token_type: u32, token_modifiers_bitset: u32 }`; `SemanticTokens { result_id: Option<String>, data: Vec<SemanticToken> }`; `SemanticTokensLegend { token_types: Vec<SemanticTokenType>, token_modifiers: Vec<SemanticTokenModifier> }`. `SemanticTokenType` and `SemanticTokenModifier` are **newtypes** with `pub const` values and `::new(&'static str)` for **custom types** (used for `punctuation`). Consts include: types `NAMESPACE TYPE STRUCT ENUM ENUM_MEMBER TYPE_PARAMETER PARAMETER VARIABLE PROPERTY FUNCTION METHOD KEYWORD COMMENT STRING NUMBER OPERATOR` (+ CLASS INTERFACE MACRO MODIFIER EVENT REGEXP DECORATOR); modifiers `DECLARATION DEFINITION READONLY STATIC DEPRECATED ABSTRACT ASYNC MODIFICATION DOCUMENTATION DEFAULT_LIBRARY`. Capability: `ServerCapabilities.semantic_tokens_provider: Option<SemanticTokensServerCapabilities>`; build `SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions { work_done_progress_options: Default, legend, range: Some(false), full: Some(SemanticTokensFullOptions::Bool(true)) })`. Result: `SemanticTokensResult::Tokens(SemanticTokens)` (variant is **`Tokens`**, not `Full`). Request: `request::SemanticTokensFullRequest`, METHOD `"textDocument/semanticTokens/full"`, `Params = SemanticTokensParams { text_document, .. }`, `Result = Option<SemanticTokensResult>`.
- **Folding** (`src/folding_range.rs`): `FoldingRange { start_line: u32, start_character: Option<u32>, end_line: u32, end_character: Option<u32>, kind: Option<FoldingRangeKind>, collapsed_text: Option<String> }`; `FoldingRangeKind::{Comment, Imports, Region}` (a real enum). `FoldingRangeParams { text_document, .. }`. Request `request::FoldingRangeRequest`, METHOD `"textDocument/foldingRange"`, `Result = Option<Vec<FoldingRange>>`. Capability `ServerCapabilities.folding_range_provider: Option<FoldingRangeProviderCapability>`; `FoldingRangeProviderCapability::Simple(true)` (or `true.into()`).
- **Selection** (`src/selection_range.rs`): `SelectionRange { range: Range, parent: Option<Box<SelectionRange>> }`. `SelectionRangeParams { text_document, positions: Vec<Position>, .. }`. Request `request::SelectionRangeRequest`, METHOD `"textDocument/selectionRange"`, `Result = Option<Vec<SelectionRange>>`. Capability `ServerCapabilities.selection_range_provider: Option<SelectionRangeProviderCapability>`; `SelectionRangeProviderCapability::Simple(true)`.
- **Completion** (`src/completion.rs`): `CompletionItem { label: String, kind: Option<CompletionItemKind>, detail: Option<String>, documentation: Option<Documentation>, insert_text: Option<String>, sort_text: Option<String>, filter_text: Option<String>, .. }` (derives `Default`). `CompletionItemKind` newtype consts: `TEXT METHOD FUNCTION CONSTRUCTOR FIELD VARIABLE CLASS INTERFACE MODULE PROPERTY UNIT VALUE ENUM KEYWORD SNIPPET COLOR FILE REFERENCE FOLDER ENUM_MEMBER CONSTANT STRUCT EVENT OPERATOR TYPE_PARAMETER`. `Documentation::MarkupContent(MarkupContent { kind: MarkupKind::Markdown, value })`. `CompletionResponse::{Array(Vec<CompletionItem>), List(CompletionList)}`. `CompletionParams { text_document_position, context: Option<CompletionContext>, .. }`. Capability `ServerCapabilities.completion_provider: Option<CompletionOptions>`; `CompletionOptions { trigger_characters: Some(vec!["."]), resolve_provider: Some(false), .. }`. Request `request::Completion`, METHOD `"textDocument/completion"`.

### F7 — Ledger-ADT method surfaces

Curated from `LFDT-Minokawa/compact/compiler/midnight-ledger.ss` (the compiler's `(declare-ledger-adt …)` table) and `standard-library.compact` (the `kernel` field), each method **both source-line-cited and confirmed with a clean `compact compile --skip-zk` probe** on compiler `compactc 0.31.0` / language `0.23.0` / ledger `8.0.2`, on 2026-07-08. The authoritative machine-readable list is the `assets/ledger_adts_0_23.json` asset built in **Task 2** (full method tables inline there). Non-obvious facts to honor (do not re-derive):

- **`Counter.increment`/`decrement` take `Uint<16>`**, not `Uint<64>` (probe: `increment(65536)` → `expected first argument of increment to have type Uint<16>`). `Counter.read()` is `Uint<64>`.
- **`Cell` is implicit.** `Cell` cannot be written as a field type (`export ledger x: Cell<…>` → `unbound identifier Cell`); instead **any ledger field of an ordinary type** (`ledger x: Uint<64>;`, a struct, `Bytes<32>`, etc.) is implicitly a `Cell<T>` exposing `read(): T`, `write(value: T): []`, `resetToDefault(): []`. Therefore receiver-typing maps: declared head ∈ {`Counter`,`Map`,`Set`,`List`,`MerkleTree`,`HistoricMerkleTree`,`Kernel`} → that ADT; **otherwise → `Cell`**. `kernel` (stdlib `export ledger kernel: Kernel;`) → `Kernel`.
- **Excluded from the completion surface** (deliberate M3 scoping, documented limitation): (a) **js-only / runtime-only** methods (`root`, `first_free`, `iter`, `history`, `path_for_leaf`, `find_path_for_leaf`) — the compiler rejects them in-circuit (`runtime-only method, but was invoked in-circuit`); (b) **coin-conditional** methods (`writeCoin`, `insertCoin`, `pushFrontCoin`) — valid only when the element type is `QualifiedShieldedCoinInfo`, undetectable without type inference, so offering them broadly would suggest invalid code.
- **MerkleTree vs HistoricMerkleTree:** HistoricMerkleTree adds `resetHistory(): []` (in-circuit); `checkRoot` is present on both but checks the *current* root (MerkleTree) vs *any past* root (HistoricMerkleTree) — reflected in the docstrings.

---

## Shared Interfaces (single source of truth for cross-task signatures)

Implementers see only their own task; every cross-task name/type lives here.

### analyzer-core (new/changed public surface)

```rust
// resolve.rs — the shared scope-enumeration primitive (Task 1)
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    pub name_range: text_size::TextRange,
    pub detail: String, // "const x" / "x: Field" / "for i" / "generic T" / "generic #n"
}
// Bindings visible at `offset`, nearest-and-latest first (inner scope + later
// shadow win). Drives both resolver find-first and completion collect-all.
// No `file`: `Binding` carries none; consumers pair it with the file they hold.
pub fn scope_bindings_at(root: &SyntaxNode, offset: TextSize) -> Vec<Binding>;

// ledger_adts.rs (Task 2)
#[derive(Clone, Debug, serde::Deserialize)]
pub struct LedgerMethod { pub name: String, pub sig: String, pub doc: String }
// The ADT method surface, or &[] if `adt` is not a known ledger ADT head.
impl AnalysisHost { pub fn ledger_adt_methods(&self, adt: &str) -> &[LedgerMethod]; }
// If `def` is a ledger field, its declared ADT head name (e.g. "Counter", "Map").
impl AnalysisHost { pub fn ledger_field_adt(&mut self, def: &Definition) -> Option<String>; }
```

### analyzer-ide (new modules; all plain types, zero lsp-types)

```rust
// completion.rs (Tasks 3-6)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword, Circuit, Witness, Struct, StructField, Enum, EnumVariant,
    Module, TypeAlias, LedgerField, LedgerMethod, Param, Local, Generic,
    StdlibItem, BuiltinType,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,      // rendered signature / type
    pub documentation: Option<String>,
}
pub fn completion(host: &mut AnalysisHost, pos: FilePosition) -> Vec<CompletionItem>;

// semantic_tokens.rs (Task 9)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    Keyword, Type, Struct, Enum, EnumMember, TypeParameter, Parameter,
    Variable, Property, Function, Method, Namespace, Comment, StringLit,
    Number, Operator, Punctuation,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenMods { pub declaration: bool, pub default_library: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemToken { pub range: TextRange, pub ty: TokenType, pub mods: TokenMods }
// Absolute-range tokens in document order; the binary deltas + UTF-16-encodes them.
pub fn semantic_tokens(host: &mut AnalysisHost, file: FileId) -> Vec<SemToken>;

// folding_ranges.rs (Task 10)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind { Region, Imports, Comment }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRange { pub range: TextRange, pub kind: FoldKind }
pub fn folding_ranges(host: &mut AnalysisHost, file: FileId) -> Vec<FoldRange>;

// selection_ranges.rs (Task 11)
// For each input offset, the CST ancestor chain of ranges, innermost first.
pub fn selection_ranges(host: &mut AnalysisHost, file: FileId, offsets: &[TextSize]) -> Vec<Vec<TextRange>>;
```

### compact-analyzer binary (Task 12-14)

- `server.rs`: handlers `textDocument/completion`, `textDocument/semanticTokens/full`, `textDocument/foldingRange`, `textDocument/selectionRange`; capabilities per F6.
- `semantic_tokens_legend.rs` (new): `LEGEND_TYPES: &[SemanticTokenType]`, `LEGEND_MODIFIERS: &[SemanticTokenModifier]`, `token_type_index(TokenType) -> u32`, `token_mods_bitset(TokenMods) -> u32`, `legend() -> SemanticTokensLegend`. The `Punctuation` type maps to a custom `SemanticTokenType::new("punctuation")`.
- `lsp_utils.rs`: `completion_kind_to_lsp(CompletionKind) -> CompletionItemKind`, delta+UTF-16 encoding of `SemToken`s.

---

## File Structure

**Create:**
- `crates/analyzer-core/src/ledger_adts.rs` — ledger-ADT table type, embedded-JSON loader, `ledger_adt_methods`, `ledger_field_adt`.
- `crates/analyzer-core/assets/ledger_adts_0_23.json` — the curated method table.
- `crates/analyzer-ide/src/completion.rs` — context classification + candidate assembly.
- `crates/analyzer-ide/src/semantic_tokens.rs` — comprehensive token classifier.
- `crates/analyzer-ide/src/folding_ranges.rs` — folding derivation.
- `crates/analyzer-ide/src/selection_ranges.rs` — selection-chain derivation.
- `crates/compact-analyzer/src/semantic_tokens_legend.rs` — legend + index/bitset mapping.
- `crates/compact-analyzer/tests/lsp_completion.rs`, `lsp_semantic_tokens.rs`, `lsp_structure.rs` — LSP integration tests.

**Modify:**
- `crates/analyzer-core/src/resolve.rs` — extract `scope_bindings_at`; reimplement `resolve_local_name` on it; handle `MULTI_CONST_STMT`.
- `crates/analyzer-core/src/analysis.rs` — `ledger_adts` field on `AnalysisHost`, loaded in `new()`.
- `crates/analyzer-core/src/lib.rs` — module + re-exports (`Binding`, `scope_bindings_at`, `LedgerMethod`).
- `crates/analyzer-core/Cargo.toml` — add `serde` (derive) + `serde_json`.
- `crates/analyzer-ide/src/lib.rs` — module decls + re-exports.
- `crates/analyzer-ide/src/hover.rs` — ledger-method hover case.
- `crates/compact-analyzer/src/server.rs` — capabilities + four handlers.
- `crates/compact-analyzer/src/main.rs` — `mod semantic_tokens_legend;`.
- `crates/compact-analyzer/src/lsp_utils.rs` — completion-kind + token encoding helpers.
- `crates/compact-analyzer/tests/corpus_smoke.rs` — exercise the four new features at sampled positions.

---

## Task 1: Shared scope-enumeration core (`scope_bindings_at`) + `MULTI_CONST_STMT` fix

**Files:**
- Modify: `crates/analyzer-core/src/resolve.rs` (rewrite `resolve_local_name`; add `Binding`, `scope_bindings_at`, `anchor_token`, `collect_scope_bindings`, `collect_param_bindings`, `collect_lambda_param_bindings`, `collect_generic_bindings`; delete now-unused `param_binding`, `lambda_param_binding`, `generic_binding`)
- Modify: `crates/analyzer-core/src/lib.rs` (re-export `Binding`, `scope_bindings_at`)
- Test: `crates/analyzer-core/src/resolve.rs` (`#[cfg(test)]` module — existing block)

**Interfaces:**
- Produces: `pub struct Binding { pub name: String, pub name_range: TextRange, pub detail: String }`; `pub fn scope_bindings_at(root: &SyntaxNode, offset: TextSize) -> Vec<Binding>`.
- Consumes: existing `pattern_bindings`, `render_ty`, `detail_for_binding`, `generic_definition`, `ident_at_offset` (all already in resolve.rs). F2 (AST accessors), F3 (`MULTI_CONST_STMT`).

**Context:** `resolve_local_name` (resolve.rs:720) currently walks ancestors matching one name. Refactor it so a single walk enumerates *all* visible bindings (`scope_bindings_at`), and `resolve_local_name` becomes a `find` over it. The walk must (a) preserve every existing resolver behavior (all 129 tests green) and (b) newly handle `MULTI_CONST_STMT` bindings (F3 — currently a latent bug). Ordering: nearest-and-latest first (inner scope before outer; within a block, later `const`s before earlier), so `find` gives correct shadowing and completion can dedup keeping the first (nearest) occurrence.

- [ ] **Step 1: Write the failing characterization test for `MULTI_CONST_STMT`**

Add to the `mod tests` block in `resolve.rs` (the `resolve_local` helper already exists):

```rust
#[test]
fn resolves_multi_const_binding() {
    // `const a = 1, b = 2;` parses to MULTI_CONST_STMT (verified: compactp
    // statements.rs const_stmt emits MULTI_CONST_STMT on a comma). Before the
    // shared-enumeration refactor the resolver's BLOCK arm only handled
    // Stmt::Const, so `b` here resolved to None — a latent bug. This test
    // locks in the fix.
    let (name, detail) =
        resolve_local("circuit f(): Field {\n  const a = 1, b = 2;\n  return b$0;\n}").unwrap();
    assert_eq!(name, "b");
    assert_eq!(detail, "const b");
    // And the first binding of the pair resolves too.
    let (name_a, _) =
        resolve_local("circuit f(): Field {\n  const a = 1, b = 2;\n  return a$0;\n}").unwrap();
    assert_eq!(name_a, "a");
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p analyzer-core resolves_multi_const_binding`
Expected: FAIL — `called Option::unwrap() on a None value` (the `MULTI_CONST_STMT` bindings are invisible today).

- [ ] **Step 3: Add `Binding` + `scope_bindings_at` + collectors**

In `resolve.rs`, add near the top (after the `Definition` enum):

```rust
/// A local binding visible in some lexical scope (param, const, loop var,
/// lambda param, generic). Produced by [`scope_bindings_at`] and consumed by
/// the resolver (find one by name) and completion (enumerate all).
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    pub name_range: TextRange,
    pub detail: String,
}

/// Every local binding visible at `offset`, **nearest-and-latest first**:
/// inner scopes precede outer ones, and within a block later `const`s precede
/// earlier ones. `resolve_local_name` returns the first match (correct
/// shadowing); completion collects all (dedup by name, first wins).
pub fn scope_bindings_at(root: &SyntaxNode, offset: TextSize) -> Vec<Binding> {
    let mut out = Vec::new();
    let Some(token) = anchor_token(root, offset) else {
        return out;
    };
    let Some(start) = token.parent() else {
        return out;
    };
    for node in start.ancestors() {
        collect_scope_bindings(&node, offset, &mut out);
    }
    out
}

/// The token to anchor the scope walk on: the IDENT at the cursor when present
/// (so the resolver path is byte-identical to the old `ident_at_offset`-based
/// walk), otherwise the token immediately left of the cursor (so completion
/// works at non-IDENT positions such as `const x = ⎸`).
fn anchor_token(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => Some(t),
        TokenAtOffset::Between(l, r) => {
            if r.kind() == SyntaxKind::IDENT {
                Some(r)
            } else {
                Some(l)
            }
        }
    }
}

/// Push the bindings that `node` introduces (if it is a scope), applying the
/// same position-visibility rules the resolver has always used.
fn collect_scope_bindings(node: &SyntaxNode, offset: TextSize, out: &mut Vec<Binding>) {
    match node.kind() {
        SyntaxKind::BLOCK => {
            let Some(block) = compactp_ast::Block::cast(node.clone()) else {
                return;
            };
            // Collect const/multi-const bindings that end before the cursor,
            // then reverse so later bindings shadow earlier ones under
            // find-first.
            let mut block_bindings = Vec::new();
            for stmt in block.stmts() {
                let const_node = match stmt {
                    compactp_ast::Stmt::Const(c) => c.syntax().clone(),
                    // MULTI_CONST_STMT (`const a = 1, b = 2;`) has no AST
                    // accessor; its patterns are direct children (F3).
                    compactp_ast::Stmt::MultiConst(m) => m.syntax().clone(),
                    _ => continue,
                };
                if const_node.text_range().end() > offset {
                    break;
                }
                for child in const_node.children() {
                    if let Some(pat) = compactp_ast::Pat::cast(child) {
                        for tok in pattern_bindings(&pat) {
                            block_bindings.push(Binding {
                                name: tok.text().to_string(),
                                name_range: tok.text_range(),
                                detail: detail_for_binding(&tok),
                            });
                        }
                    }
                }
            }
            block_bindings.reverse();
            out.extend(block_bindings);
        }
        SyntaxKind::FOR_STMT => {
            if let Some(for_stmt) = compactp_ast::ForStmt::cast(node.clone())
                && for_stmt
                    .body()
                    .is_some_and(|b| b.syntax().text_range().contains(offset))
                && let Some(var) = for_stmt.var_name()
            {
                out.push(Binding {
                    name: var.text().to_string(),
                    name_range: var.text_range(),
                    detail: format!("for {}", var.text()),
                });
            }
        }
        SyntaxKind::LAMBDA_EXPR => {
            if let Some(lambda) = compactp_ast::expr::LambdaExpr::cast(node.clone())
                && let Some(params) = lambda.param_list()
                && offset >= params.syntax().text_range().end()
            {
                for child in params.syntax().children() {
                    collect_lambda_param_bindings(&child, out);
                }
            }
        }
        SyntaxKind::CIRCUIT_DEF => {
            if let Some(circuit) = compactp_ast::CircuitDef::cast(node.clone()) {
                for param in circuit.params() {
                    collect_param_bindings(&param, out);
                }
                collect_generic_bindings(circuit.generic_params(), out);
            }
        }
        SyntaxKind::CONSTRUCTOR_DEF => {
            if let Some(ctor) = compactp_ast::ConstructorDef::cast(node.clone()) {
                for param in ctor.params() {
                    collect_param_bindings(&param, out);
                }
            }
        }
        SyntaxKind::STRUCT_DEF => {
            if let Some(s) = compactp_ast::StructDef::cast(node.clone()) {
                collect_generic_bindings(s.generic_params(), out);
            }
        }
        SyntaxKind::TYPE_DECL => {
            if let Some(t) = compactp_ast::TypeDecl::cast(node.clone()) {
                collect_generic_bindings(t.generic_params(), out);
            }
        }
        SyntaxKind::MODULE_DEF => {
            if let Some(m) = compactp_ast::ModuleDef::cast(node.clone()) {
                collect_generic_bindings(m.generic_params(), out);
            }
        }
        _ => {}
    }
}

/// Bindings of a circuit/constructor `PARAM(pattern, type)`, detail `name: ty`.
fn collect_param_bindings(param: &compactp_ast::Param, out: &mut Vec<Binding>) {
    if let Some(pat) = param.pattern() {
        let ty = render_ty(param.ty());
        for tok in pattern_bindings(&pat) {
            out.push(Binding {
                name: tok.text().to_string(),
                name_range: tok.text_range(),
                detail: format!("{}: {ty}", tok.text()),
            });
        }
    }
}

/// Bindings of one raw child of a lambda's PARAM_LIST — bare `Pat`,
/// `PARAM(pattern, type)`, or a bare `IDENT` token (verified lambda shapes,
/// mirrors the old `lambda_param_binding`).
fn collect_lambda_param_bindings(node: &SyntaxNode, out: &mut Vec<Binding>) {
    if let Some(pat) = Pat::cast(node.clone()) {
        for tok in pattern_bindings(&pat) {
            out.push(Binding {
                name: tok.text().to_string(),
                name_range: tok.text_range(),
                detail: format!("{}: _", tok.text()),
            });
        }
        return;
    }
    if let Some(param) = compactp_ast::Param::cast(node.clone()) {
        if let Some(pat) = param.pattern() {
            let ty = render_ty(param.ty());
            for tok in pattern_bindings(&pat) {
                out.push(Binding {
                    name: tok.text().to_string(),
                    name_range: tok.text_range(),
                    detail: format!("{}: {ty}", tok.text()),
                });
            }
            return;
        }
        if let Some(tok) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == SyntaxKind::IDENT)
        {
            let ty = render_ty(param.ty());
            out.push(Binding {
                name: tok.text().to_string(),
                name_range: tok.text_range(),
                detail: format!("{}: {ty}", tok.text()),
            });
        }
    }
}

/// Bindings introduced by a generic parameter list, detail `generic T` /
/// `generic #n` (mirrors `generic_definition`).
fn collect_generic_bindings(
    params: Option<compactp_ast::GenericParamList>,
    out: &mut Vec<Binding>,
) {
    let Some(params) = params else {
        return;
    };
    for param in params.params() {
        if let Some(token) = param.name() {
            let numeric = param
                .syntax()
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .any(|t| t.kind() == SyntaxKind::HASH);
            out.push(Binding {
                name: token.text().to_string(),
                name_range: token.text_range(),
                detail: if numeric {
                    format!("generic #{}", token.text())
                } else {
                    format!("generic {}", token.text())
                },
            });
        }
    }
}
```

- [ ] **Step 4: Rewrite `resolve_local_name` as a thin `find`, delete the old per-scope helpers**

Replace the entire body of `resolve_local_name` (resolve.rs:720-844) — keeping its doc comment about the token-pick — with:

```rust
fn resolve_local_name(
    file: FileId,
    root: &SyntaxNode,
    offset: TextSize,
    name: &str,
) -> Option<Definition> {
    scope_bindings_at(root, offset)
        .into_iter()
        .find(|b| b.name == name)
        .map(|b| Definition::Local {
            file,
            name: b.name,
            name_range: b.name_range,
            detail: b.detail,
        })
}
```

Then **delete** the now-unused free functions `param_binding`, `lambda_param_binding`, and `generic_binding` (their logic moved into the `collect_*` collectors). Keep `generic_definition` (still used by `resolve()`'s `GENERIC_PARAM` arm), `pattern_bindings`, `collect_pattern_bindings`, `render_ty`, `collapse_ws`, `detail_for_binding`, `ident_at_offset`.

- [ ] **Step 5: Re-export from `lib.rs`**

In `crates/analyzer-core/src/lib.rs`, change the resolve re-export line:

```rust
pub use resolve::{Binding, Definition, FilePosition, scope_bindings_at};
```

- [ ] **Step 6: Run the full resolver + core suite (regression + new test)**

Run: `cargo test -p analyzer-core`
Expected: PASS — all previously-green resolve tests still pass (behavior preserved) **and** `resolves_multi_const_binding` now passes. If any previously-green test fails, it is a real regression in the refactor — root-cause and fix it; do not weaken the test.

- [ ] **Step 7: Add a direct `scope_bindings_at` enumeration test**

```rust
#[test]
fn scope_bindings_enumerates_locals_nearest_first() {
    // At the cursor, `x` (inner const) shadows the param `x`; both a param and
    // an inner const named differently are all visible; nearest/ latest first.
    let source = "circuit f(x: Field, y: Field): Field {\n  const z = 1;\n  return x$0;\n}";
    let (clean, offset) = fixture::extract(source);
    let result = compactp_parser::parse(&clean);
    let root = SyntaxNode::new_root(result.green);
    let names: Vec<String> = scope_bindings_at(&root, offset)
        .into_iter()
        .map(|b| b.name)
        .collect();
    // Inner-block const `z` first, then params `x`, `y`.
    assert_eq!(names, vec!["z", "x", "y"]);
}
```

- [ ] **Step 8: Run it**

Run: `cargo test -p analyzer-core scope_bindings_enumerates_locals_nearest_first`
Expected: PASS.

- [ ] **Step 9: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: all green, no clippy warnings (confirm the deleted helpers leave no `dead_code`).

```bash
git add crates/analyzer-core/src/resolve.rs crates/analyzer-core/src/lib.rs
git commit -m "fix(core): shared scope enumeration; resolve MULTI_CONST_STMT bindings

Extracts the resolver's lexical-scope walk into scope_bindings_at (used by
both resolve and the upcoming completion feature) and, in doing so, fixes a
latent bug: bindings from a multi-binding const (const a = 1, b = 2;, parsed
as MULTI_CONST_STMT) were invisible to name resolution."
```

---

## Task 2: Ledger-ADT method table (asset + loader + receiver typing)

**Files:**
- Create: `crates/analyzer-core/assets/ledger_adts_0_23.json`
- Create: `crates/analyzer-core/src/ledger_adts.rs`
- Modify: `crates/analyzer-core/src/analysis.rs` (host field + accessors)
- Modify: `crates/analyzer-core/src/lib.rs` (`mod ledger_adts;`, re-export `LedgerMethod`)
- Modify: `crates/analyzer-core/Cargo.toml` (add `serde`, `serde_json`)
- Test: `crates/analyzer-core/src/ledger_adts.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `pub struct LedgerMethod { pub name: String, pub sig: String, pub doc: String }`; `AnalysisHost::ledger_adt_methods(&self, adt: &str) -> &[LedgerMethod]`; `AnalysisHost::ledger_field_adt(&mut self, def: &Definition) -> Option<String>`.
- Consumes: `Definition` (Task 0/M2), `SymbolKind::Ledger`, `LedgerDecl::ty()` + `TypeRef::name()` (F2), the F7 method surface.

**Context:** The ledger ADTs are compiler builtins invoked with method syntax; they are not Compact circuits (F7), so the table is a curated JSON asset embedded and parsed once at host construction. `ledger_field_adt` maps a resolved ledger field to its ADT key (declared head, or `Cell` for a plain-typed field — F7).

- [ ] **Step 1: Create the asset `crates/analyzer-core/assets/ledger_adts_0_23.json`**

Exact contents (curated + probe-confirmed per F7; js-only and coin-conditional methods excluded):

```json
{
  "_meta": {
    "language_version": "0.23",
    "compiler": "compactc-0.31.0",
    "ledger": "8.0.2",
    "source": "LFDT-Minokawa/compact compiler/midnight-ledger.ss (declare-ledger-adt) + standard-library.compact",
    "note": "In-circuit methods only. js-only (root/iter/history/path_for_leaf/first_free/find_path_for_leaf) and coin-conditional (writeCoin/insertCoin/pushFrontCoin) methods are intentionally omitted. Refresh procedure: re-read midnight-ledger.ss at the target compiler tag; confirm each via `compact compile --skip-zk` probes."
  },
  "Counter": [
    { "name": "read", "sig": "read(): Uint<64>", "doc": "Retrieves the current value of the counter." },
    { "name": "lessThan", "sig": "lessThan(threshold: Uint<64>): Boolean", "doc": "Returns whether the counter is less than the given threshold." },
    { "name": "increment", "sig": "increment(amount: Uint<16>): []", "doc": "Increments the counter by the given amount." },
    { "name": "decrement", "sig": "decrement(amount: Uint<16>): []", "doc": "Decrements the counter; going below zero is a run-time error." },
    { "name": "resetToDefault", "sig": "resetToDefault(): []", "doc": "Resets this Counter to its default value of 0." }
  ],
  "Cell": [
    { "name": "read", "sig": "read(): T", "doc": "Returns the current contents of this Cell." },
    { "name": "write", "sig": "write(value: T): []", "doc": "Overwrites the content of this Cell with the given value." },
    { "name": "resetToDefault", "sig": "resetToDefault(): []", "doc": "Resets this Cell to the default value of its type." }
  ],
  "Map": [
    { "name": "isEmpty", "sig": "isEmpty(): Boolean", "doc": "Returns if this Map is the empty map." },
    { "name": "size", "sig": "size(): Uint<64>", "doc": "Returns the number of entries in this Map." },
    { "name": "member", "sig": "member(key: K): Boolean", "doc": "Returns if a key is contained within this Map." },
    { "name": "lookup", "sig": "lookup(key: K): V", "doc": "Looks up the value of a key; the value may itself be an ADT." },
    { "name": "insert", "sig": "insert(key: K, value: V): []", "doc": "Updates this Map to include a new value at a given key." },
    { "name": "insertDefault", "sig": "insertDefault(key: K): []", "doc": "Inserts the value type's default value at a given key." },
    { "name": "remove", "sig": "remove(key: K): []", "doc": "Updates this Map to not include a given key." },
    { "name": "resetToDefault", "sig": "resetToDefault(): []", "doc": "Resets this Map to the empty map." }
  ],
  "Set": [
    { "name": "isEmpty", "sig": "isEmpty(): Boolean", "doc": "Returns whether this Set is the empty set." },
    { "name": "size", "sig": "size(): Uint<64>", "doc": "Returns the number of unique entries." },
    { "name": "member", "sig": "member(elem: T): Boolean", "doc": "Returns if an element is in this Set." },
    { "name": "insert", "sig": "insert(elem: T): []", "doc": "Updates this Set to include a given element." },
    { "name": "remove", "sig": "remove(elem: T): []", "doc": "Updates this Set to not include a given element." },
    { "name": "resetToDefault", "sig": "resetToDefault(): []", "doc": "Resets this Set to the empty set." }
  ],
  "List": [
    { "name": "isEmpty", "sig": "isEmpty(): Boolean", "doc": "Returns if this List is the empty list." },
    { "name": "length", "sig": "length(): Uint<64>", "doc": "Returns the number of elements." },
    { "name": "head", "sig": "head(): Maybe<T>", "doc": "Retrieves the head as a Maybe (safe on an empty list)." },
    { "name": "pushFront", "sig": "pushFront(value: T): []", "doc": "Pushes a new element onto the front." },
    { "name": "popFront", "sig": "popFront(): []", "doc": "Removes the first element from the front." },
    { "name": "resetToDefault", "sig": "resetToDefault(): []", "doc": "Resets this List to the empty list." }
  ],
  "MerkleTree": [
    { "name": "isFull", "sig": "isFull(): Boolean", "doc": "Returns if the tree is full and no further items can be directly inserted." },
    { "name": "checkRoot", "sig": "checkRoot(rt: MerkleTreeDigest): Boolean", "doc": "Tests if rt is the current root of this tree." },
    { "name": "insert", "sig": "insert(item: T): []", "doc": "Inserts a new leaf at the first free index." },
    { "name": "insertIndex", "sig": "insertIndex(item: T, index: Uint<64>): []", "doc": "Inserts a new leaf at a specific index." },
    { "name": "insertHash", "sig": "insertHash(hash: Bytes<32>): []", "doc": "Inserts a leaf with a given hash at the first free index." },
    { "name": "insertHashIndex", "sig": "insertHashIndex(hash: Bytes<32>, index: Uint<64>): []", "doc": "Inserts a leaf with a given hash at a specific index." },
    { "name": "insertIndexDefault", "sig": "insertIndexDefault(index: Uint<64>): []", "doc": "Inserts a default-value leaf at an index (emulates removal)." },
    { "name": "resetToDefault", "sig": "resetToDefault(): []", "doc": "Resets to the empty Merkle tree." }
  ],
  "HistoricMerkleTree": [
    { "name": "isFull", "sig": "isFull(): Boolean", "doc": "Returns if the tree is full." },
    { "name": "checkRoot", "sig": "checkRoot(rt: MerkleTreeDigest): Boolean", "doc": "Tests if rt is one of the past roots of this tree." },
    { "name": "insert", "sig": "insert(item: T): []", "doc": "Inserts a leaf at the first free index and records the new root in history." },
    { "name": "insertIndex", "sig": "insertIndex(item: T, index: Uint<64>): []", "doc": "Inserts a leaf at a specific index and records the new root." },
    { "name": "insertHash", "sig": "insertHash(hash: Bytes<32>): []", "doc": "Inserts a leaf with a given hash at the first free index." },
    { "name": "insertHashIndex", "sig": "insertHashIndex(hash: Bytes<32>, index: Uint<64>): []", "doc": "Inserts a leaf with a given hash at a specific index." },
    { "name": "insertIndexDefault", "sig": "insertIndexDefault(index: Uint<64>): []", "doc": "Inserts a default-value leaf at an index." },
    { "name": "resetHistory", "sig": "resetHistory(): []", "doc": "Resets the history, leaving only the current root valid." },
    { "name": "resetToDefault", "sig": "resetToDefault(): []", "doc": "Resets to the empty Merkle tree." }
  ],
  "Kernel": [
    { "name": "self", "sig": "self(): ContractAddress", "doc": "Returns the current contract's address." },
    { "name": "checkpoint", "sig": "checkpoint(): []", "doc": "Marks execution up to this point as one atomic unit." },
    { "name": "claimZswapNullifier", "sig": "claimZswapNullifier(nul: Bytes<32>): []", "doc": "Requires the nullifier's presence in the tx, claimed by no other call." },
    { "name": "claimZswapCoinSpend", "sig": "claimZswapCoinSpend(note: Bytes<32>): []", "doc": "Requires a commitment in the tx claimed as a spend by no other call." },
    { "name": "claimZswapCoinReceive", "sig": "claimZswapCoinReceive(note: Bytes<32>): []", "doc": "Requires a commitment in the tx claimed as a receive by no other call." },
    { "name": "claimContractCall", "sig": "claimContractCall(addr: Bytes<32>, entry_point: Bytes<32>, comm: Field): []", "doc": "Requires a matching contract-to-contract call in the tx, unclaimed by others." },
    { "name": "mintShielded", "sig": "mintShielded(domain_sep: Bytes<32>, amount: Uint<64>): []", "doc": "Mints shielded coins of a token type derived from the contract address + domain sep." },
    { "name": "mintUnshielded", "sig": "mintUnshielded(domain_sep: Bytes<32>, amount: Uint<64>): []", "doc": "Mints unshielded coins of a derived token type." },
    { "name": "claimUnshieldedCoinSpend", "sig": "claimUnshieldedCoinSpend(token_type: Either<Bytes<32>, Bytes<32>>, address: Either<ContractAddress, UserAddress>, amount: Uint<128>): []", "doc": "Authorizes an unshielded coin of the given type to be transferred to an address." },
    { "name": "incUnshieldedOutputs", "sig": "incUnshieldedOutputs(token_type: Either<Bytes<32>, Bytes<32>>, amount: Uint<128>): []", "doc": "Increments unshielded output for a token type (used when sending)." },
    { "name": "incUnshieldedInputs", "sig": "incUnshieldedInputs(token_type: Either<Bytes<32>, Bytes<32>>, amount: Uint<128>): []", "doc": "Increments unshielded input for a token type (used when receiving)." },
    { "name": "balance", "sig": "balance(token_type: Either<Bytes<32>, Bytes<32>>): Uint<128>", "doc": "Returns the contract's unshielded balance of a token type." },
    { "name": "balanceLessThan", "sig": "balanceLessThan(token_type: Either<Bytes<32>, Bytes<32>>, amount: Uint<128>): Boolean", "doc": "Whether the balance of a token type is less than the given amount." },
    { "name": "balanceGreaterThan", "sig": "balanceGreaterThan(token_type: Either<Bytes<32>, Bytes<32>>, amount: Uint<128>): Boolean", "doc": "Whether the balance of a token type is greater than the given amount." },
    { "name": "blockTimeLessThan", "sig": "blockTimeLessThan(time: Uint<64>): Boolean", "doc": "Whether current block time is less than time (seconds since the Unix epoch)." },
    { "name": "blockTimeGreaterThan", "sig": "blockTimeGreaterThan(time: Uint<64>): Boolean", "doc": "Whether current block time is greater than time." }
  ]
}
```

- [ ] **Step 2: Add `serde`/`serde_json` to `crates/analyzer-core/Cargo.toml`**

Under `[dependencies]`:

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 3: Write the failing loader test**

Create `crates/analyzer-core/src/ledger_adts.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_parses_and_covers_all_adts() {
        let table = LedgerAdtTable::load();
        for adt in [
            "Counter", "Cell", "Map", "Set", "List", "MerkleTree",
            "HistoricMerkleTree", "Kernel",
        ] {
            assert!(!table.methods(adt).is_empty(), "missing ADT: {adt}");
        }
        // Probe-confirmed non-obvious facts (F7).
        let inc = table
            .methods("Counter")
            .iter()
            .find(|m| m.name == "increment")
            .unwrap();
        assert_eq!(inc.sig, "increment(amount: Uint<16>): []");
        assert!(table.methods("nonsense").is_empty());
        // js-only / coin methods are intentionally absent.
        assert!(!table.methods("MerkleTree").iter().any(|m| m.name == "root"));
        assert!(!table.methods("Map").iter().any(|m| m.name == "insertCoin"));
    }
}
```

- [ ] **Step 4: Implement the loader (make it compile & the test fail meaningfully first)**

Above the test module in `ledger_adts.rs`:

```rust
//! Curated ledger-ADT method surfaces for completion + hover.
//!
//! The ledger ADTs (Counter, Cell, Map, Set, List, MerkleTree,
//! HistoricMerkleTree, Kernel) are compiler builtins invoked with method
//! syntax (`counter.increment(1)`), not Compact circuits — so they cannot be
//! expressed as a `.compact` stub. This table is curated from the compiler
//! source at the pinned tag and embedded as JSON.
//!
//! Provenance: `LFDT-Minokawa/compact compiler/midnight-ledger.ss`
//! (`declare-ledger-adt` forms) + `standard-library.compact` (the `kernel`
//! field), language 0.23 / compiler 0.31.0 / ledger 8.0.2. In-circuit methods
//! only; js-only and coin-conditional methods are omitted (see the asset's
//! `_meta.note`). Refresh: re-read `midnight-ledger.ss` at the target compiler
//! tag and re-confirm each method with `compact compile --skip-zk` probes;
//! bump the asset filename's language version and this doc-comment together.

use std::collections::BTreeMap;

/// One in-circuit method of a ledger ADT.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct LedgerMethod {
    pub name: String,
    pub sig: String,
    pub doc: String,
}

const LEDGER_ADTS_JSON: &str = include_str!("../assets/ledger_adts_0_23.json");

/// The parsed ADT → methods table. The `_meta` object in the JSON is ignored.
pub(crate) struct LedgerAdtTable {
    by_adt: BTreeMap<String, Vec<LedgerMethod>>,
}

impl LedgerAdtTable {
    pub(crate) fn load() -> Self {
        // The asset is embedded and covered by a parse test; a malformed asset
        // is a build-time authoring error, but never panic the server — fall
        // back to an empty table so completion/hover simply offer nothing.
        let raw: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(LEDGER_ADTS_JSON).unwrap_or_default();
        let mut by_adt = BTreeMap::new();
        for (adt, value) in raw {
            if adt == "_meta" {
                continue;
            }
            if let Ok(methods) = serde_json::from_value::<Vec<LedgerMethod>>(value) {
                by_adt.insert(adt, methods);
            }
        }
        Self { by_adt }
    }

    pub(crate) fn methods(&self, adt: &str) -> &[LedgerMethod] {
        self.by_adt.get(adt).map(Vec::as_slice).unwrap_or(&[])
    }
}
```

- [ ] **Step 5: Run the loader test (first non-`--locked` build — new deps)**

Run: `cargo test -p analyzer-core --lib ledger_adts` (NOTE: omit `--locked` on this first build so `Cargo.lock` picks up serde/serde_json)
Expected: PASS.

- [ ] **Step 6: Wire the table onto `AnalysisHost` + add receiver typing**

In `analysis.rs`, add the field to `AnalysisHost`:

```rust
    ledger_adts: crate::ledger_adts::LedgerAdtTable,
```

In `AnalysisHost::new()`, initialize it: `ledger_adts: crate::ledger_adts::LedgerAdtTable::load(),`.

Add the two accessors in an `impl AnalysisHost` block (in `ledger_adts.rs`, so the module owns its host surface):

```rust
impl crate::AnalysisHost {
    /// In-circuit method surface for a ledger ADT head, or `&[]` if unknown.
    pub fn ledger_adt_methods(&self, adt: &str) -> &[LedgerMethod] {
        self.ledger_adts_table().methods(adt)
    }

    /// If `def` is a ledger field, the ADT key to look its methods up under:
    /// the declared type head when it is a known ADT
    /// (Counter/Map/Set/List/MerkleTree/HistoricMerkleTree/Kernel), otherwise
    /// `"Cell"` — a plain-typed ledger field is implicitly a Cell (F7).
    pub fn ledger_field_adt(&mut self, def: &crate::Definition) -> Option<String> {
        let crate::Definition::Item { file, index } = def else {
            return None;
        };
        let analysis = self.analyze(*file)?;
        let sym = analysis.item_tree.symbols.get(*index as usize)?;
        if sym.kind != crate::SymbolKind::Ledger {
            return None;
        }
        let name_range = sym.name_range;
        let root = crate::SyntaxNode::new_root(analysis.green.clone());
        // Find the LEDGER_DECL whose name token matches, read its type head.
        let ledger = root
            .descendants()
            .filter(|n| n.kind() == crate::SyntaxKind::LEDGER_DECL)
            .find_map(compactp_ast::LedgerDecl::cast)
            .filter(|d| d.name().is_some_and(|t| t.text_range() == name_range))?;
        let head = match ledger.ty()? {
            compactp_ast::Type::Ref(r) => r.name()?.text().to_string(),
            _ => return Some("Cell".to_string()), // builtin scalar type → Cell
        };
        const KNOWN: &[&str] = &[
            "Counter", "Cell", "Map", "Set", "List", "MerkleTree",
            "HistoricMerkleTree", "Kernel",
        ];
        Some(if KNOWN.contains(&head.as_str()) {
            head
        } else {
            "Cell".to_string() // user/struct-typed field → implicit Cell
        })
    }
}
```

Add a private accessor next to the field so the `impl` above can reach it (the field is private to `analysis.rs`):

```rust
// in analysis.rs, impl AnalysisHost
    pub(crate) fn ledger_adts_table(&self) -> &crate::ledger_adts::LedgerAdtTable {
        &self.ledger_adts
    }
```

Add `use compactp_ast::AstNode;` to `ledger_adts.rs` if needed for `cast`.

- [ ] **Step 7: Register the module + re-export in `lib.rs`**

```rust
mod ledger_adts;
// ...
pub use ledger_adts::LedgerMethod;
```

(Keep `LedgerAdtTable` crate-private — only `LedgerMethod` is part of the public surface.)

- [ ] **Step 8: Write + run the receiver-typing test**

Add to `ledger_adts.rs` tests (reuse the `full_host`-style helper pattern; a compact inline version):

```rust
    #[test]
    fn ledger_field_adt_maps_head_and_implicit_cell() {
        let mut host = crate::AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(
            file,
            "export ledger cnt: Counter;\nexport ledger bal: Uint<64>;\n".to_string(),
            1,
        );
        let tree = host.analyze(file).unwrap().item_tree.clone();
        let cnt = tree.symbols.iter().position(|s| s.name == "cnt").unwrap();
        let bal = tree.symbols.iter().position(|s| s.name == "bal").unwrap();
        assert_eq!(
            host.ledger_field_adt(&crate::Definition::Item { file, index: cnt as u32 }),
            Some("Counter".to_string())
        );
        // Plain-typed ledger field is an implicit Cell.
        assert_eq!(
            host.ledger_field_adt(&crate::Definition::Item { file, index: bal as u32 }),
            Some("Cell".to_string())
        );
    }
```

Run: `cargo test -p analyzer-core --lib ledger_adts`
Expected: PASS (both tests).

- [ ] **Step 9: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-core`
Expected: green (now `--locked` again — `Cargo.lock` already updated in Step 5).

```bash
git add crates/analyzer-core/assets/ledger_adts_0_23.json crates/analyzer-core/src/ledger_adts.rs crates/analyzer-core/src/analysis.rs crates/analyzer-core/src/lib.rs crates/analyzer-core/Cargo.toml Cargo.lock
git commit -m "feat(core): curated ledger-ADT method table + receiver typing"
```

---

## Task 3: Completion context classifier + keyword completion

**Files:**
- Modify: `crates/analyzer-ide/Cargo.toml` (add `compactp_ast`, `rowan` deps + `tempfile` dev-dep)
- Create: `crates/analyzer-ide/src/completion.rs`
- Modify: `crates/analyzer-ide/src/lib.rs` (`mod completion;` + re-exports)
- Test: `crates/analyzer-ide/src/completion.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `CompletionKind`, `CompletionItem`, `pub fn completion(host, pos) -> Vec<CompletionItem>` (per Shared Interfaces). Internal: `enum Ctx`, `fn classify(root, offset) -> Ctx`.
- Consumes: F1 (kinds), F4 (CST shapes + classifier rule), F5 (keyword→position map).

**Context:** Classify the cursor on the real (error-recovered) tree per F4, then emit the position-valid keywords. Symbol/member/field candidates are added in Tasks 4-5; this task delivers the classifier and keyword completion, so its match already routes every context. **Layering note:** these CST-derived features need typed AST access, so `analyzer-ide` gains direct `compactp_ast` + `rowan` dependencies here (the syntax-tree types `SyntaxNode`/`SyntaxKind` are already re-exported to ide by core; `lsp-types` remains absent — the invariant that matters). Recorded in Self-review notes as a reviewed deviation from the spec's "core owns the completion query" framing.

- [ ] **Step 0: Add `analyzer-ide` dependencies**

In `crates/analyzer-ide/Cargo.toml`:

```toml
[dependencies]
analyzer-core.workspace = true
compactp_ast.workspace = true
rowan.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

(`tempfile` is needed by the stdlib-based test in Task 4. `compactp_ast`/`rowan` are already in the workspace + `Cargo.lock` — but adding these dependency edges changes `analyzer-ide`'s entry in `Cargo.lock`, so **the first build in Step 4 must omit `--locked`**, then subsequent gates restore it.)

- [ ] **Step 1: Write failing keyword-completion tests**

Create `crates/analyzer-ide/src/completion.rs` with a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, FilePosition, fixture};

    fn labels(source: &str) -> Vec<String> {
        let (clean, offset) = fixture::extract(source);
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        completion(&mut host, FilePosition { file, offset })
            .into_iter()
            .map(|c| c.label)
            .collect()
    }

    #[test]
    fn declaration_position_offers_declaration_keywords() {
        // Top level, nothing typed yet.
        let ls = labels("$0");
        assert!(ls.contains(&"circuit".to_string()));
        assert!(ls.contains(&"ledger".to_string()));
        assert!(ls.contains(&"import".to_string()));
        // Not an expression keyword.
        assert!(!ls.contains(&"return".to_string()));
    }

    #[test]
    fn statement_position_offers_statement_and_expr_keywords() {
        let ls = labels("circuit f(): Field {\n  $0\n}");
        assert!(ls.contains(&"return".to_string()));
        assert!(ls.contains(&"const".to_string()));
        assert!(ls.contains(&"disclose".to_string()));
        // Not a declaration keyword (we're inside a body).
        assert!(!ls.contains(&"circuit".to_string()));
    }

    #[test]
    fn type_position_offers_type_keywords() {
        let ls = labels("circuit f(x: $0): Field { return 0; }");
        assert!(ls.contains(&"Field".to_string()));
        assert!(ls.contains(&"Bytes".to_string()));
        assert!(!ls.contains(&"return".to_string()));
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p analyzer-ide completion`
Expected: FAIL to compile (`completion` undefined).

- [ ] **Step 3: Implement types, classifier, and keyword completion**

Top of `crates/analyzer-ide/src/completion.rs`:

```rust
//! Context-aware completion over the (possibly error-recovered) CST.

use analyzer_core::{
    AnalysisHost, FilePosition, SyntaxKind, SyntaxNode, SyntaxToken, TextSize,
};
use compactp_ast::AstNode;
use rowan::TokenAtOffset;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    Circuit,
    Witness,
    Struct,
    StructField,
    Enum,
    EnumVariant,
    Module,
    TypeAlias,
    LedgerField,
    LedgerMethod,
    Param,
    Local,
    Generic,
    StdlibItem,
    BuiltinType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
}

/// The cursor's completion context, classified from the raw tree (F4).
enum Ctx {
    /// After `.` on a receiver expression node.
    Member(SyntaxNode),
    /// Inside a struct-literal brace body (field-name position).
    StructLiteral(SyntaxNode),
    /// Type-annotation / type-reference position.
    Type,
    /// Statement / expression position inside a body block.
    Expr,
    /// Top-level or module-body declaration position.
    Declaration,
}

// F5 keyword sets.
const DECL_KEYWORDS: &[&str] = &[
    "export", "sealed", "pure", "circuit", "witness", "ledger", "struct", "enum",
    "module", "contract", "constructor", "type", "new", "import", "include", "pragma",
];
const STMT_KEYWORDS: &[&str] = &["const", "return", "if", "else", "for", "assert"];
const EXPR_KEYWORDS: &[&str] = &[
    "map", "fold", "default", "disclose", "pad", "slice", "true", "false",
];
const TYPE_KEYWORDS: &[&str] = &["Boolean", "Field", "Uint", "Bytes", "Opaque", "Vector"];

/// Context-aware completion candidates at `pos`. Never panics; returns empty on
/// an unreadable/unclassifiable position.
pub fn completion(host: &mut AnalysisHost, pos: FilePosition) -> Vec<CompletionItem> {
    let Some(analysis) = host.analyze(pos.file) else {
        return Vec::new();
    };
    let root = SyntaxNode::new_root(analysis.green.clone());
    let mut items = Vec::new();
    match classify(&root, pos.offset) {
        Ctx::Declaration => push_keywords(&mut items, DECL_KEYWORDS, CompletionKind::Keyword),
        Ctx::Expr => {
            push_keywords(&mut items, STMT_KEYWORDS, CompletionKind::Keyword);
            push_keywords(&mut items, EXPR_KEYWORDS, CompletionKind::Keyword);
            // Task 4 adds in-scope symbols here.
            push_scope_and_items(host, pos, &root, &mut items);
        }
        Ctx::Type => {
            push_keywords(&mut items, TYPE_KEYWORDS, CompletionKind::BuiltinType);
            // Task 4 adds in-scope type items here.
            push_type_items(host, pos, &root, &mut items);
        }
        Ctx::Member(receiver) => push_member(host, pos, &receiver, &mut items), // Task 5
        Ctx::StructLiteral(se) => push_struct_fields(host, pos, &se, &mut items), // Task 5
    }
    items
}

fn push_keywords(items: &mut Vec<CompletionItem>, kws: &[&str], kind: CompletionKind) {
    for kw in kws {
        items.push(CompletionItem {
            label: (*kw).to_string(),
            kind: kind.clone(),
            detail: None,
            documentation: None,
        });
    }
}

// ---- context classification (F4) ----

fn classify(root: &SyntaxNode, offset: TextSize) -> Ctx {
    if let Some(receiver) = member_receiver(root, offset) {
        return Ctx::Member(receiver);
    }
    if let Some(se) = enclosing_struct_literal(root, offset) {
        return Ctx::StructLiteral(se);
    }
    if in_type_position(root, offset) {
        return Ctx::Type;
    }
    if in_block_body(root, offset) {
        Ctx::Expr
    } else {
        Ctx::Declaration
    }
}

/// The IDENT the user is typing at the cursor, if any (partial-ident cases).
fn completion_ident(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => (t.kind() == SyntaxKind::IDENT).then_some(t),
        TokenAtOffset::Between(l, r) => {
            if l.kind() == SyntaxKind::IDENT && l.text_range().end() == offset {
                Some(l)
            } else if r.kind() == SyntaxKind::IDENT {
                Some(r)
            } else {
                None
            }
        }
    }
}

/// The first non-trivia token strictly to the left of the cursor.
fn left_token(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    let start = match root.token_at_offset(offset) {
        TokenAtOffset::None => return None,
        TokenAtOffset::Single(t) => t,
        TokenAtOffset::Between(l, _) => l,
    };
    std::iter::once(start.clone())
        .chain(start.prev_token().into_iter().flat_map(|t| {
            std::iter::successors(Some(t), |p| p.prev_token())
        }))
        .find(|t| !t.kind().is_trivia() && t.text_range().start() < offset)
}

/// The receiver expr node if the cursor is a member-access field position.
fn member_receiver(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    // Partial member: `c.incr⎸` — the IDENT's parent is MEMBER_EXPR/CALL_EXPR,
    // preceded by a DOT.
    if let Some(id) = completion_ident(root, offset)
        && let Some(parent) = id.parent()
        && matches!(parent.kind(), SyntaxKind::MEMBER_EXPR | SyntaxKind::CALL_EXPR)
        && has_dot_before(&parent, id.text_range().start())
    {
        return first_expr_child(&parent);
    }
    // Empty member: `c.⎸` — the token to the left is a DOT under MEMBER/CALL.
    if let Some(dot) = left_token(root, offset).filter(|t| t.kind() == SyntaxKind::DOT)
        && let Some(parent) = dot.parent()
        && matches!(parent.kind(), SyntaxKind::MEMBER_EXPR | SyntaxKind::CALL_EXPR)
    {
        return first_expr_child(&parent);
    }
    None
}

fn has_dot_before(node: &SyntaxNode, before: TextSize) -> bool {
    node.children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .any(|t| t.kind() == SyntaxKind::DOT && t.text_range().end() <= before)
}

fn first_expr_child(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.children().find(|c| compactp_ast::Expr::can_cast(c.kind()))
}

/// The enclosing STRUCT_EXPR iff the cursor is at a field-name position (left
/// token is `{` or `,`, i.e. not inside a field value after `:`).
fn enclosing_struct_literal(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    let start = match root.token_at_offset(offset) {
        TokenAtOffset::Single(t) => t.parent()?,
        TokenAtOffset::Between(l, _) => l.parent()?,
        TokenAtOffset::None => return None,
    };
    let se = start
        .ancestors()
        .find(|n| n.kind() == SyntaxKind::STRUCT_EXPR)?;
    let left = left_token(root, offset)?;
    matches!(left.kind(), SyntaxKind::L_BRACE | SyntaxKind::COMMA).then_some(se)
}

/// True at a type-annotation / type-reference position.
fn in_type_position(root: &SyntaxNode, offset: TextSize) -> bool {
    // Partial ident directly in a TYPE_REF, or covered by a type node.
    if let Some(id) = completion_ident(root, offset)
        && let Some(parent) = id.parent()
        && (parent.kind() == SyntaxKind::TYPE_REF || is_type_node(parent.kind()))
    {
        return true;
    }
    let covering = match root.token_at_offset(offset) {
        TokenAtOffset::Single(t) => t.parent(),
        TokenAtOffset::Between(l, r) => {
            if r.kind() == SyntaxKind::IDENT { r.parent() } else { l.parent() }
        }
        TokenAtOffset::None => None,
    };
    if let Some(n) = &covering
        && n.ancestors().any(|a| is_type_node(a.kind()) || a.kind() == SyntaxKind::TYPE_REF)
    {
        return true;
    }
    // Empty position right after a `:` that introduces a type.
    if let Some(colon) = left_token(root, offset).filter(|t| t.kind() == SyntaxKind::COLON)
        && let Some(p) = colon.parent()
    {
        return matches!(
            p.kind(),
            SyntaxKind::PARAM
                | SyntaxKind::LEDGER_DECL
                | SyntaxKind::STRUCT_FIELD
                | SyntaxKind::CONST_STMT
                | SyntaxKind::TYPE_DECL
                | SyntaxKind::CONTRACT_CIRCUIT
        );
    }
    false
}

fn is_type_node(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::BOOLEAN_TYPE
            | SyntaxKind::FIELD_TYPE
            | SyntaxKind::UINT_TYPE
            | SyntaxKind::UNSIGNED_INTEGER_TYPE
            | SyntaxKind::BYTES_TYPE
            | SyntaxKind::OPAQUE_TYPE
            | SyntaxKind::VECTOR_TYPE
            | SyntaxKind::TUPLE_TYPE
            | SyntaxKind::RECORD_TYPE
            | SyntaxKind::GENERIC_ARG
            | SyntaxKind::GENERIC_ARG_LIST
    )
}

/// True inside a circuit/constructor/lambda body BLOCK (statement/expr
/// position); false at top-level or module-body (declaration position).
fn in_block_body(root: &SyntaxNode, offset: TextSize) -> bool {
    let start = match root.token_at_offset(offset) {
        TokenAtOffset::Single(t) => t.parent(),
        TokenAtOffset::Between(l, r) => {
            if r.kind() == SyntaxKind::IDENT { r.parent() } else { l.parent() }
        }
        TokenAtOffset::None => None,
    };
    start.is_some_and(|n| n.ancestors().any(|a| a.kind() == SyntaxKind::BLOCK))
}

// Task 4/5 fill these; declared here so `completion` compiles now.
fn push_scope_and_items(
    _host: &mut AnalysisHost,
    _pos: FilePosition,
    _root: &SyntaxNode,
    _items: &mut Vec<CompletionItem>,
) {
}
fn push_type_items(
    _host: &mut AnalysisHost,
    _pos: FilePosition,
    _root: &SyntaxNode,
    _items: &mut Vec<CompletionItem>,
) {
}
fn push_member(
    _host: &mut AnalysisHost,
    _pos: FilePosition,
    _receiver: &SyntaxNode,
    _items: &mut Vec<CompletionItem>,
) {
}
fn push_struct_fields(
    _host: &mut AnalysisHost,
    _pos: FilePosition,
    _struct_expr: &SyntaxNode,
    _items: &mut Vec<CompletionItem>,
) {
}
```

Register in `crates/analyzer-ide/src/lib.rs`:

```rust
mod completion;
pub use completion::{completion, CompletionItem, CompletionKind};
```

- [ ] **Step 4: Run the keyword tests (first build omits `--locked` — new dep edges)**

Run: `cargo test -p analyzer-ide completion` (no `--locked`, so `Cargo.lock` records the new `analyzer-ide → compactp_ast/rowan` edges)
Expected: PASS (the three keyword tests). The four `push_*` stubs are intentionally empty (filled in Tasks 4-5); leaving the calls in `completion` keeps them from tripping `dead_code`.

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`
Expected: green (now `--locked` again — `Cargo.lock` already updated in Step 4).

```bash
git add crates/analyzer-ide/Cargo.toml crates/analyzer-ide/src/completion.rs crates/analyzer-ide/src/lib.rs Cargo.lock
git commit -m "feat(ide): completion context classifier + keyword completion"
```

---

## Task 4: Completion — in-scope symbols (locals, items, imports, stdlib)

**Files:**
- Modify: `crates/analyzer-ide/src/completion.rs` (implement `push_scope_and_items`, `push_type_items`)
- Test: `crates/analyzer-ide/src/completion.rs`

**Interfaces:**
- Consumes: `analyzer_core::scope_bindings_at` (Task 1); `host.analyze(file).item_tree`; `host.stdlib_file()`; `Symbol`/`SymbolKind`; `SourceFile::imports()`. F5.

**Context:** Expression position offers locals (via `scope_bindings_at`) + in-scope top-level/module items + imported names + stdlib exports (when `CompactStandardLibrary` is imported). Type position offers the type-like items (structs, enums, type aliases, in-scope generics). Completion ⊆ what resolves — no auto-import.

- [ ] **Step 1: Write failing tests**

```rust
    #[test]
    fn expr_position_offers_locals_and_items() {
        let ls = labels(
            "circuit helper(): Field { return 1; }\n\
             circuit f(base: Field): Field {\n  const local = 1;\n  return $0\n}",
        );
        assert!(ls.contains(&"base".to_string()));   // param
        assert!(ls.contains(&"local".to_string()));  // const local
        assert!(ls.contains(&"helper".to_string()));  // sibling circuit
        assert!(ls.contains(&"f".to_string()));       // self
    }

    #[test]
    fn expr_position_offers_stdlib_when_imported() {
        // full_host-style: register stdlib in a tempdir.
        let (clean, offset) = fixture::extract(
            "import CompactStandardLibrary;\ncircuit f(): Field { return $0 }",
        );
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        let ls: Vec<String> = completion(&mut host, FilePosition { file, offset })
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert!(ls.contains(&"persistentHash".to_string()));
    }

    #[test]
    fn type_position_offers_user_types() {
        let ls = labels(
            "struct Point { x: Field; }\nenum Color { red }\n\
             circuit f(p: $0): Field { return 0; }",
        );
        assert!(ls.contains(&"Point".to_string()));
        assert!(ls.contains(&"Color".to_string()));
        assert!(ls.contains(&"Field".to_string())); // builtin still there
        // Not a value-only circuit.
        assert!(!ls.contains(&"f".to_string()));
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p analyzer-ide completion`
Expected: FAIL (locals/items/stdlib/user-types missing).

- [ ] **Step 3: Implement `push_scope_and_items` and `push_type_items`**

Replace the two stubs:

```rust
fn push_scope_and_items(
    host: &mut AnalysisHost,
    pos: FilePosition,
    root: &SyntaxNode,
    items: &mut Vec<CompletionItem>,
) {
    // Locals (params, consts, loop vars, lambda params, generics) — nearest
    // wins; dedup by name.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for b in analyzer_core::scope_bindings_at(root, pos.offset) {
        if seen.insert(b.name.clone()) {
            items.push(CompletionItem {
                label: b.name,
                kind: local_kind(&b.detail),
                detail: Some(b.detail),
                documentation: None,
            });
        }
    }
    push_file_items(host, pos.file, items, ItemFilter::Value);
    push_imported_items(host, pos.file, root, items, ItemFilter::Value);
}

fn push_type_items(
    host: &mut AnalysisHost,
    pos: FilePosition,
    root: &SyntaxNode,
    items: &mut Vec<CompletionItem>,
) {
    // In-scope generic type params.
    for b in analyzer_core::scope_bindings_at(root, pos.offset) {
        if b.detail.starts_with("generic ") {
            items.push(CompletionItem {
                label: b.name,
                kind: CompletionKind::Generic,
                detail: Some(b.detail),
                documentation: None,
            });
        }
    }
    push_file_items(host, pos.file, items, ItemFilter::Type);
    push_imported_items(host, pos.file, root, items, ItemFilter::Type);
}

#[derive(Clone, Copy)]
enum ItemFilter {
    Value,
    Type,
}

fn item_matches(kind: analyzer_core::SymbolKind, filter: ItemFilter) -> bool {
    use analyzer_core::SymbolKind as K;
    match filter {
        ItemFilter::Value => matches!(
            kind,
            K::Circuit | K::CircuitSig | K::Witness | K::Ledger | K::Module
        ),
        ItemFilter::Type => matches!(kind, K::Struct | K::Enum | K::TypeAlias),
    }
}

fn kind_of(kind: analyzer_core::SymbolKind) -> CompletionKind {
    use analyzer_core::SymbolKind as K;
    match kind {
        K::Circuit | K::CircuitSig => CompletionKind::Circuit,
        K::Witness => CompletionKind::Witness,
        K::Struct => CompletionKind::Struct,
        K::Enum => CompletionKind::Enum,
        K::Module => CompletionKind::Module,
        K::TypeAlias => CompletionKind::TypeAlias,
        K::Ledger => CompletionKind::LedgerField,
        _ => CompletionKind::Local,
    }
}

fn local_kind(detail: &str) -> CompletionKind {
    if detail.starts_with("generic ") {
        CompletionKind::Generic
    } else if detail.contains(": ") && !detail.starts_with("const ") && !detail.starts_with("for ") {
        CompletionKind::Param
    } else {
        CompletionKind::Local
    }
}

/// Top-level (and enclosing-module) items of `file` matching `filter`.
fn push_file_items(
    host: &mut AnalysisHost,
    file: analyzer_core::FileId,
    items: &mut Vec<CompletionItem>,
    filter: ItemFilter,
) {
    let Some(analysis) = host.analyze(file) else {
        return;
    };
    let tree = analysis.item_tree.clone();
    for (_, sym) in tree.top_level() {
        if sym.name.is_empty() || !item_matches(sym.kind, filter) {
            continue;
        }
        items.push(CompletionItem {
            label: sym.name.clone(),
            kind: kind_of(sym.kind),
            detail: Some(sym.signature.clone()),
            documentation: sym.doc.clone(),
        });
    }
}

/// Names brought in by imports (stdlib exports; in-scope-module members).
fn push_imported_items(
    host: &mut AnalysisHost,
    file: analyzer_core::FileId,
    root: &SyntaxNode,
    items: &mut Vec<CompletionItem>,
    filter: ItemFilter,
) {
    let Some(sf) = compactp_ast::SourceFile::cast(root.clone()) else {
        return;
    };
    for import in sf.imports() {
        match import.name() {
            Some(t) if t.text() == "CompactStandardLibrary" => {
                let Some(std_file) = host.stdlib_file() else { continue };
                let Some(analysis) = host.analyze(std_file) else { continue };
                let tree = analysis.item_tree.clone();
                for (_, sym) in tree.top_level() {
                    if !sym.exported || sym.name.is_empty() || !item_matches(sym.kind, filter) {
                        continue;
                    }
                    items.push(CompletionItem {
                        label: sym.name.clone(),
                        kind: CompletionKind::StdlibItem,
                        detail: Some(sym.signature.clone()),
                        documentation: sym.doc.clone(),
                    });
                }
            }
            Some(t) => {
                // In-scope module: offer its exported members (no prefix/alias
                // rewriting in M3 — labels are the plain member names; a
                // prefixed import still resolves the plain member via M2).
                let module_name = t.text().to_string();
                let Some(analysis) = host.analyze(file) else { continue };
                let tree = analysis.item_tree.clone();
                if let Some((midx, _)) = tree
                    .top_level()
                    .find(|(_, s)| s.kind == analyzer_core::SymbolKind::Module && s.name == module_name)
                {
                    for (_, sym) in tree.children_of(midx) {
                        if !sym.exported || sym.name.is_empty() || !item_matches(sym.kind, filter) {
                            continue;
                        }
                        items.push(CompletionItem {
                            label: sym.name.clone(),
                            kind: kind_of(sym.kind),
                            detail: Some(sym.signature.clone()),
                            documentation: sym.doc.clone(),
                        });
                    }
                }
            }
            None => {} // string-path imports: cross-file member enumeration deferred (see Self-review)
        }
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p analyzer-ide completion`
Expected: PASS (locals, items, stdlib, user-types).

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`

```bash
git add crates/analyzer-ide/src/completion.rs
git commit -m "feat(ide): completion offers in-scope symbols, items, and stdlib"
```

---

## Task 5: Completion — struct-literal fields, ledger methods, enum variants

**Files:**
- Modify: `crates/analyzer-ide/src/completion.rs` (implement `push_member`, `push_struct_fields`)
- Test: `crates/analyzer-ide/src/completion.rs`

**Interfaces:**
- Consumes: `host.resolve`, `host.ledger_field_adt`, `host.ledger_adt_methods` (Task 2), `host.field_of`-equivalent via item_tree `children_of`, `SymbolKind`. F4 (receiver node), F7 (ledger surface).

**Context:** Member context resolves the receiver: a ledger field → ADT methods (Cell for plain-typed fields); an enum → variants. Struct-literal context resolves the struct type → its fields (minus those already written).

- [ ] **Step 1: Write failing tests**

```rust
    #[test]
    fn member_on_ledger_field_offers_adt_methods() {
        let ls = labels(
            "export ledger cnt: Counter;\ncircuit f(): [] { cnt.$0 }",
        );
        assert!(ls.contains(&"increment".to_string()));
        assert!(ls.contains(&"resetToDefault".to_string()));
        // increment's detail carries the probe-confirmed Uint<16> signature.
        // (label-only assert here; detail checked below)
    }

    #[test]
    fn member_on_plain_ledger_field_offers_cell_methods() {
        let ls = labels(
            "export ledger bal: Uint<64>;\ncircuit f(): [] { bal.$0 }",
        );
        assert!(ls.contains(&"read".to_string()));
        assert!(ls.contains(&"write".to_string()));
    }

    #[test]
    fn member_on_enum_offers_variants() {
        let ls = labels(
            "enum Color { red, blue }\ncircuit f(): [] { const c = Color.$0 }",
        );
        assert!(ls.contains(&"red".to_string()));
        assert!(ls.contains(&"blue".to_string()));
    }

    #[test]
    fn struct_literal_offers_remaining_fields() {
        let ls = labels(
            "struct Point { x: Field; y: Field; }\n\
             circuit f(): [] { const p = Point { x: 1, $0 }; }",
        );
        assert!(ls.contains(&"y".to_string()));
        // x already provided — not re-offered.
        assert!(!ls.contains(&"x".to_string()));
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p analyzer-ide completion`
Expected: FAIL (member/struct candidates missing).

- [ ] **Step 3: Implement `push_member` and `push_struct_fields`**

Replace the two stubs:

```rust
fn push_member(
    host: &mut AnalysisHost,
    pos: FilePosition,
    receiver: &SyntaxNode,
    items: &mut Vec<CompletionItem>,
) {
    // Resolve the receiver at its own start offset (mirrors resolve_member).
    let recv_pos = FilePosition {
        file: pos.file,
        offset: receiver.text_range().start(),
    };
    let Some(def) = host.resolve(recv_pos) else {
        return;
    };
    // Ledger field → ADT (or implicit Cell) methods.
    if let Some(adt) = host.ledger_field_adt(&def) {
        for m in host.ledger_adt_methods(&adt) {
            items.push(CompletionItem {
                label: m.name.clone(),
                kind: CompletionKind::LedgerMethod,
                detail: Some(m.sig.clone()),
                documentation: Some(m.doc.clone()),
            });
        }
        return;
    }
    // Enum receiver → variants.
    if let analyzer_core::Definition::Item { file, index } = &def
        && let Some(analysis) = host.analyze(*file)
    {
        let tree = analysis.item_tree.clone();
        if tree
            .symbols
            .get(*index as usize)
            .is_some_and(|s| s.kind == analyzer_core::SymbolKind::Enum)
        {
            for (_, sym) in tree.children_of(*index) {
                if sym.kind == analyzer_core::SymbolKind::EnumVariant && !sym.name.is_empty() {
                    items.push(CompletionItem {
                        label: sym.name.clone(),
                        kind: CompletionKind::EnumVariant,
                        detail: Some(sym.signature.clone()),
                        documentation: sym.doc.clone(),
                    });
                }
            }
        }
    }
}

fn push_struct_fields(
    host: &mut AnalysisHost,
    pos: FilePosition,
    struct_expr: &SyntaxNode,
    items: &mut Vec<CompletionItem>,
) {
    let Some(se) = compactp_ast::expr::StructExpr::cast(struct_expr.clone()) else {
        return;
    };
    let Some(name_tok) = se.name() else { return };
    // Fields already written in this literal.
    let provided: std::collections::HashSet<String> = se
        .field_inits()
        .filter_map(|fi| fi.name().map(|t| t.text().to_string()))
        .collect();
    // Resolve the struct type from its name token position.
    let def = host.resolve(FilePosition {
        file: pos.file,
        offset: name_tok.text_range().start(),
    });
    let Some(analyzer_core::Definition::Item { file, index }) = def else {
        return;
    };
    let Some(analysis) = host.analyze(file) else { return };
    let tree = analysis.item_tree.clone();
    if tree
        .symbols
        .get(index as usize)
        .is_none_or(|s| s.kind != analyzer_core::SymbolKind::Struct)
    {
        return;
    }
    for (_, sym) in tree.children_of(index) {
        if sym.kind == analyzer_core::SymbolKind::StructField
            && !sym.name.is_empty()
            && !provided.contains(&sym.name)
        {
            items.push(CompletionItem {
                label: sym.name.clone(),
                kind: CompletionKind::StructField,
                detail: Some(sym.signature.clone()),
                documentation: sym.doc.clone(),
            });
        }
    }
}
```

- [ ] **Step 4: Add a detail-parity assertion for the ledger method**

```rust
    #[test]
    fn ledger_method_detail_uses_probe_confirmed_signature() {
        let (clean, offset) = fixture::extract("export ledger cnt: Counter;\ncircuit f(): [] { cnt.$0 }");
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        let inc = completion(&mut host, FilePosition { file, offset })
            .into_iter()
            .find(|c| c.label == "increment")
            .unwrap();
        assert_eq!(inc.detail.as_deref(), Some("increment(amount: Uint<16>): []"));
        assert_eq!(inc.kind, CompletionKind::LedgerMethod);
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p analyzer-ide completion`
Expected: PASS (member ADT/Cell/enum, struct fields, detail parity).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`

```bash
git add crates/analyzer-ide/src/completion.rs
git commit -m "feat(ide): completion for ledger methods, enum variants, struct fields"
```

---

## Task 6: Hover on ledger-ADT methods

**Files:**
- Modify: `crates/analyzer-ide/src/hover.rs`
- Test: `crates/analyzer-ide/src/hover.rs`

**Interfaces:**
- Consumes: `host.resolve`, `host.ledger_field_adt`, `host.ledger_adt_methods`; F4 call/member shapes; F7.

**Context:** `resolve` returns `None` for a ledger-method call (the `ledger_adt_methods_resolve_to_none` test still holds). Hover falls back to the ledger table when the cursor is on a method IDENT after a `.` on a ledger-field receiver. No new `Definition` variant — goto/references/rename are untouched.

- [ ] **Step 1: Write failing tests**

Add to `hover.rs` tests (the `hover_md` helper already registers the stdlib):

```rust
    #[test]
    fn hover_on_ledger_method_shows_table_signature() {
        let md = hover_md(
            "export ledger cnt: Counter;\ncircuit f(): [] { cnt.incre$0ment(1); }",
        )
        .unwrap();
        assert!(md.contains("increment(amount: Uint<16>): []"), "{md}");
        assert!(md.contains("Increments the counter"), "{md}");
    }

    #[test]
    fn hover_on_plain_ledger_field_method_shows_cell_signature() {
        let md = hover_md(
            "export ledger bal: Uint<64>;\ncircuit f(): [] { bal.wr$0ite(7); }",
        )
        .unwrap();
        assert!(md.contains("write(value: T): []"), "{md}");
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p analyzer-ide hover`
Expected: FAIL — `hover_md(...).unwrap()` panics (hover returns `None` for these today).

- [ ] **Step 3: Implement the fallback**

In `hover.rs`, change imports and `hover`:

```rust
use analyzer_core::{
    AnalysisHost, Definition, FilePosition, SyntaxKind, SyntaxNode, SyntaxToken,
};
use compactp_ast::AstNode;
use rowan::TokenAtOffset;

pub fn hover(host: &mut AnalysisHost, pos: FilePosition) -> Option<HoverResult> {
    if let Some(def) = host.resolve(pos) {
        let markdown = match &def {
            Definition::Item { file, index } => {
                let sym = host
                    .analyze(*file)?
                    .item_tree
                    .symbols
                    .get(*index as usize)?
                    .clone();
                match &sym.doc {
                    Some(doc) => format!("```compact\n{}\n```\n\n{}", sym.signature, doc),
                    None => format!("```compact\n{}\n```", sym.signature),
                }
            }
            Definition::Local { detail, .. } => format!("```compact\n{detail}\n```"),
        };
        return Some(HoverResult { markdown });
    }
    ledger_method_hover(host, pos)
}

/// Hover for `receiver.method` where `receiver` is a ledger field: render the
/// method's signature + doc from the ledger-ADT table.
fn ledger_method_hover(host: &mut AnalysisHost, pos: FilePosition) -> Option<HoverResult> {
    let root = {
        let analysis = host.analyze(pos.file)?;
        SyntaxNode::new_root(analysis.green.clone())
    };
    let token = ident_at(&root, pos.offset)?;
    let parent = token.parent()?;
    if !matches!(parent.kind(), SyntaxKind::MEMBER_EXPR | SyntaxKind::CALL_EXPR) {
        return None;
    }
    // A DOT must precede the method IDENT (excludes a direct-call callee).
    let dot_before = parent
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .any(|t| t.kind() == SyntaxKind::DOT && t.text_range().end() <= token.text_range().start());
    if !dot_before {
        return None;
    }
    let receiver = parent
        .children()
        .find(|c| compactp_ast::Expr::can_cast(c.kind()))?;
    let def = host.resolve(FilePosition {
        file: pos.file,
        offset: receiver.text_range().start(),
    })?;
    let adt = host.ledger_field_adt(&def)?;
    let name = token.text().to_string();
    let m = host
        .ledger_adt_methods(&adt)
        .iter()
        .find(|m| m.name == name)?;
    Some(HoverResult {
        markdown: format!("```compact\n{}\n```\n\n{}", m.sig, m.doc),
    })
}

/// The IDENT at/adjacent to `offset` (prefer the right token at a boundary).
fn ident_at(root: &SyntaxNode, offset: analyzer_core::TextSize) -> Option<SyntaxToken> {
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => (t.kind() == SyntaxKind::IDENT).then_some(t),
        TokenAtOffset::Between(l, r) => (r.kind() == SyntaxKind::IDENT)
            .then_some(r)
            .or_else(|| (l.kind() == SyntaxKind::IDENT).then_some(l)),
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p analyzer-ide hover`
Expected: PASS (new ledger-method hovers + all existing hover tests).

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`

```bash
git add crates/analyzer-ide/src/hover.rs
git commit -m "feat(ide): hover renders ledger-ADT method signatures"
```

---

## Task 7: Semantic tokens (comprehensive classifier)

**Files:**
- Create: `crates/analyzer-ide/src/semantic_tokens.rs`
- Modify: `crates/analyzer-ide/src/lib.rs`
- Test: `crates/analyzer-ide/src/semantic_tokens.rs`

**Interfaces:**
- Produces: `TokenType`, `TokenMods`, `SemToken`, `pub fn semantic_tokens(host, file) -> Vec<SemToken>` (Shared Interfaces).
- Consumes: F1 (token kinds), F4 (call shapes), `host.resolve`, `host.stdlib_file`, `SymbolKind`.

**Context:** Every non-whitespace token → `(TokenType, TokenMods)`, in document order (the binary deltas + UTF-16-encodes them). Structural classification first; use-site `NAME_EXPR`/direct-callee idents resolve to refine role and add `default_library`.

- [ ] **Step 1: Write failing tests**

Create `crates/analyzer-ide/src/semantic_tokens.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::AnalysisHost;

    fn toks(source: &str) -> Vec<(String, TokenType)> {
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let text = source.to_string();
        semantic_tokens(&mut host, file)
            .into_iter()
            .map(|t| (text[t.range].to_string(), t.ty))
            .collect()
    }

    fn ty_of<'a>(v: &'a [(String, TokenType)], text: &str) -> Option<&'a TokenType> {
        v.iter().find(|(t, _)| t == text).map(|(_, k)| k)
    }

    #[test]
    fn classifies_declaration_and_types() {
        let v = toks("export circuit inc(x: Field): Field { return x + 1; }");
        assert_eq!(ty_of(&v, "export"), Some(&TokenType::Keyword));
        assert_eq!(ty_of(&v, "circuit"), Some(&TokenType::Keyword));
        assert_eq!(ty_of(&v, "inc"), Some(&TokenType::Function));
        assert_eq!(ty_of(&v, "x"), Some(&TokenType::Parameter));
        assert_eq!(ty_of(&v, "Field"), Some(&TokenType::Type));
        assert_eq!(ty_of(&v, "+"), Some(&TokenType::Operator));
        assert_eq!(ty_of(&v, "1"), Some(&TokenType::Number));
        assert_eq!(ty_of(&v, "("), Some(&TokenType::Punctuation));
        assert_eq!(ty_of(&v, "return"), Some(&TokenType::Keyword));
    }

    #[test]
    fn classifies_ledger_and_calls() {
        let v = toks("export ledger cnt: Counter;\ncircuit f(): [] { helper(); cnt.increment(1); }");
        assert_eq!(ty_of(&v, "cnt"), Some(&TokenType::Property)); // ledger field decl + use
        assert_eq!(ty_of(&v, "helper"), Some(&TokenType::Function)); // direct callee (unresolved → Function)
        assert_eq!(ty_of(&v, "increment"), Some(&TokenType::Method)); // method after dot
    }

    #[test]
    fn declaration_modifier_and_comment() {
        let mut host = AnalysisHost::new();
        let src = "// hi\ncircuit f(): [] { }";
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/m.compact"));
        host.vfs_mut().set_overlay(file, src.to_string(), 1);
        let ts = semantic_tokens(&mut host, file);
        let f = ts.iter().find(|t| &src[t.range] == "f").unwrap();
        assert_eq!(f.ty, TokenType::Function);
        assert!(f.mods.declaration);
        assert!(ts.iter().any(|t| t.ty == TokenType::Comment));
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p analyzer-ide semantic_tokens`
Expected: FAIL to compile (`semantic_tokens` undefined).

- [ ] **Step 3: Implement the classifier**

Above the tests:

```rust
//! Comprehensive, full-document semantic-token classification from the CST.

use analyzer_core::{
    AnalysisHost, Definition, FileId, FilePosition, SymbolKind, SyntaxKind, SyntaxNode,
    SyntaxToken, TextRange,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    Keyword,
    Type,
    Struct,
    Enum,
    EnumMember,
    TypeParameter,
    Parameter,
    Variable,
    Property,
    Function,
    Method,
    Namespace,
    Comment,
    StringLit,
    Number,
    Operator,
    Punctuation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenMods {
    pub declaration: bool,
    pub default_library: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemToken {
    pub range: TextRange,
    pub ty: TokenType,
    pub mods: TokenMods,
}

/// All non-whitespace tokens classified, in document order.
pub fn semantic_tokens(host: &mut AnalysisHost, file: FileId) -> Vec<SemToken> {
    let root = match host.analyze(file) {
        Some(a) => SyntaxNode::new_root(a.green.clone()),
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for tok in root
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
    {
        let Some((ty, mods)) = classify_token(host, file, &tok) else {
            continue;
        };
        out.push(SemToken {
            range: tok.text_range(),
            ty,
            mods,
        });
    }
    out
}

fn classify_token(
    host: &mut AnalysisHost,
    file: FileId,
    tok: &SyntaxToken,
) -> Option<(TokenType, TokenMods)> {
    use SyntaxKind::*;
    let mods = TokenMods::default();
    let ty = match tok.kind() {
        WHITESPACE | ERROR | EOF => return None,
        LINE_COMMENT | BLOCK_COMMENT => TokenType::Comment,
        STRING_LIT => TokenType::StringLit,
        INT_LIT | HEX_LIT | OCT_LIT | BIN_LIT | VERSION_LIT => TokenType::Number,
        BOOLEAN_KW | FIELD_KW | UINT_KW | BYTES_KW | OPAQUE_KW | VECTOR_KW | UNSIGNED_KW
        | INTEGER_KW => TokenType::Type,
        k if is_keyword(k) => TokenType::Keyword,
        k if is_operator(k) => TokenType::Operator,
        k if is_punct(k) => TokenType::Punctuation,
        IDENT => return Some(classify_ident(host, file, tok)),
        _ => return None,
    };
    Some((ty, mods))
}

fn is_keyword(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        PRAGMA_KW | INCLUDE_KW | IMPORT_KW | FROM_KW | PREFIX_KW | EXPORT_KW | MODULE_KW
            | LEDGER_KW | CONSTRUCTOR_KW | CIRCUIT_KW | WITNESS_KW | CONTRACT_KW | STRUCT_KW
            | ENUM_KW | TYPE_KW | CONST_KW | RETURN_KW | IF_KW | ELSE_KW | FOR_KW | OF_KW
            | ASSERT_KW | AS_KW | PURE_KW | SEALED_KW | NEW_KW | MAP_KW | FOLD_KW | DEFAULT_KW
            | DISCLOSE_KW | PAD_KW | SLICE_KW | TRUE_KW | FALSE_KW
    )
}

fn is_operator(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        EQ | PLUS_EQ | MINUS_EQ | EQ_EQ | BANG_EQ | LT | LT_EQ | GT | GT_EQ | AMP_AMP
            | PIPE_PIPE | PLUS | MINUS | STAR | SLASH | BANG | QUESTION | FAT_ARROW | DOT
            | DOT_DOT | DOT_DOT_DOT
    )
}

fn is_punct(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        L_PAREN | R_PAREN | L_BRACE | R_BRACE | L_BRACKET | R_BRACKET | COMMA | SEMICOLON
            | COLON | HASH
    )
}

/// Classify an IDENT by its parent kind, resolving use-site names for refinement.
fn classify_ident(host: &mut AnalysisHost, file: FileId, tok: &SyntaxToken) -> (TokenType, TokenMods) {
    use SyntaxKind::*;
    let mut mods = TokenMods::default();
    let Some(parent) = tok.parent() else {
        return (TokenType::Variable, mods);
    };
    match parent.kind() {
        CIRCUIT_DEF | CIRCUIT_DECL | WITNESS_DECL => {
            mods.declaration = true;
            (TokenType::Function, mods)
        }
        CONTRACT_CIRCUIT => {
            mods.declaration = true;
            (TokenType::Method, mods)
        }
        STRUCT_DEF | CONTRACT_DECL => {
            mods.declaration = true;
            (TokenType::Struct, mods)
        }
        ENUM_DEF => {
            mods.declaration = true;
            (TokenType::Enum, mods)
        }
        MODULE_DEF => {
            mods.declaration = true;
            (TokenType::Namespace, mods)
        }
        TYPE_DECL => {
            mods.declaration = true;
            (TokenType::Type, mods)
        }
        LEDGER_DECL | STRUCT_FIELD => {
            mods.declaration = true;
            (TokenType::Property, mods)
        }
        ENUM_VARIANT => {
            mods.declaration = true;
            (TokenType::EnumMember, mods)
        }
        GENERIC_PARAM => {
            mods.declaration = true;
            (TokenType::TypeParameter, mods)
        }
        FOR_STMT => {
            mods.declaration = true;
            (TokenType::Variable, mods)
        }
        TYPE_REF => (TokenType::Type, mods),
        STRUCT_EXPR => (TokenType::Struct, mods),
        STRUCT_FIELD_INIT | MEMBER_EXPR => (TokenType::Property, mods),
        PARAM => (TokenType::Parameter, mods),
        IDENT_PAT => {
            let is_param = parent.ancestors().any(|a| a.kind() == PARAM);
            (
                if is_param {
                    TokenType::Parameter
                } else {
                    TokenType::Variable
                },
                mods,
            )
        }
        IMPORT | IMPORT_SPECIFIER | PREFIX_DECL | PRAGMA => (TokenType::Namespace, mods),
        NAME_EXPR => classify_use_site(host, file, tok, TokenType::Variable),
        CALL_EXPR => {
            // A DOT before the IDENT ⇒ method name; else a direct callee (F4).
            let dot_before = parent
                .children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
                .any(|t| t.kind() == DOT && t.text_range().end() <= tok.text_range().start());
            if dot_before {
                (TokenType::Method, mods)
            } else {
                classify_use_site(host, file, tok, TokenType::Function)
            }
        }
        _ => (TokenType::Variable, mods),
    }
}

/// Resolve a use-site identifier to refine its token type + `default_library`.
fn classify_use_site(
    host: &mut AnalysisHost,
    file: FileId,
    tok: &SyntaxToken,
    fallback: TokenType,
) -> (TokenType, TokenMods) {
    let mut mods = TokenMods::default();
    let pos = FilePosition {
        file,
        offset: tok.text_range().start(),
    };
    let Some(def) = host.resolve(pos) else {
        return (fallback, mods);
    };
    if let Definition::Item { file: def_file, .. } = &def
        && host.stdlib_file() == Some(*def_file)
    {
        mods.default_library = true;
    }
    let ty = match &def {
        Definition::Local { detail, .. } => {
            if detail.starts_with("generic ") {
                TokenType::TypeParameter
            } else if detail.contains(": ")
                && !detail.starts_with("const ")
                && !detail.starts_with("for ")
            {
                TokenType::Parameter
            } else {
                TokenType::Variable
            }
        }
        Definition::Item { file: def_file, index } => {
            match host
                .analyze(*def_file)
                .and_then(|a| a.item_tree.symbols.get(*index as usize).map(|s| s.kind))
            {
                Some(SymbolKind::Circuit) | Some(SymbolKind::CircuitSig)
                | Some(SymbolKind::Witness) => TokenType::Function,
                Some(SymbolKind::Ledger) => TokenType::Property,
                Some(SymbolKind::Struct) => TokenType::Struct,
                Some(SymbolKind::Enum) => TokenType::Enum,
                Some(SymbolKind::Module) => TokenType::Namespace,
                Some(SymbolKind::TypeAlias) => TokenType::Type,
                _ => fallback,
            }
        }
    };
    (ty, mods)
}
```

Register in `lib.rs`:

```rust
mod semantic_tokens;
pub use semantic_tokens::{semantic_tokens, SemToken, TokenMods, TokenType};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p analyzer-ide semantic_tokens`
Expected: PASS.

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`

```bash
git add crates/analyzer-ide/src/semantic_tokens.rs crates/analyzer-ide/src/lib.rs
git commit -m "feat(ide): comprehensive CST semantic-token classification"
```

---

## Task 8: Folding ranges

**Files:**
- Create: `crates/analyzer-ide/src/folding_ranges.rs`
- Modify: `crates/analyzer-ide/src/lib.rs`
- Test: `crates/analyzer-ide/src/folding_ranges.rs`

**Interfaces:**
- Produces: `FoldKind`, `FoldRange`, `pub fn folding_ranges(host, file) -> Vec<FoldRange>` (Shared Interfaces).
- Consumes: F1 (node/token kinds).

**Context:** CST-derived byte ranges for brace-delimited bodies, the leading import/include run (`kind=Imports`), and multi-line block comments (`kind=Comment`). Byte ranges only; the binary derives start/end lines and drops single-line ranges.

- [ ] **Step 1: Write failing tests**

Create `crates/analyzer-ide/src/folding_ranges.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::AnalysisHost;

    fn folds(source: &str) -> Vec<(String, FoldKind)> {
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/m.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let text = source.to_string();
        folding_ranges(&mut host, file)
            .into_iter()
            .map(|f| (text[f.range].to_string(), f.kind))
            .collect()
    }

    #[test]
    fn folds_circuit_body_and_struct() {
        let v = folds("circuit f(): Field {\n  return 0;\n}\nstruct P {\n  x: Field;\n}");
        assert!(v.iter().any(|(t, k)| t.starts_with('{') && *k == FoldKind::Region));
        assert_eq!(v.iter().filter(|(_, k)| *k == FoldKind::Region).count(), 2);
    }

    #[test]
    fn folds_import_group_and_block_comment() {
        let v = folds("import CompactStandardLibrary;\nimport Foo;\n/* a\n   b */\ncircuit f(): [] { }");
        assert!(v.iter().any(|(_, k)| *k == FoldKind::Imports));
        assert!(v.iter().any(|(_, k)| *k == FoldKind::Comment));
    }

    #[test]
    fn single_line_body_is_not_folded_here() {
        // Byte range still spans one line; the binary drops it, but the ide
        // layer emits a range only when start != end line is possible — so we
        // still emit; assert the range is present but the binary will filter.
        let v = folds("circuit f(): [] { }");
        // Region range for `{ }` is emitted (single-line filtering is the
        // binary's job); nothing else.
        assert!(v.iter().all(|(_, k)| *k == FoldKind::Region));
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p analyzer-ide folding_ranges`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
//! CST-derived folding ranges (byte ranges; the binary maps to lines).

use analyzer_core::{AnalysisHost, FileId, SyntaxKind, SyntaxNode, TextRange, TextSize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Region,
    Imports,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub range: TextRange,
    pub kind: FoldKind,
}

pub fn folding_ranges(host: &mut AnalysisHost, file: FileId) -> Vec<FoldRange> {
    let root = match host.analyze(file) {
        Some(a) => SyntaxNode::new_root(a.green.clone()),
        None => return Vec::new(),
    };
    let mut out = Vec::new();

    // 1. Brace-delimited bodies: from the opening `{` to the closing `}`.
    for node in root.descendants() {
        if matches!(
            node.kind(),
            SyntaxKind::CIRCUIT_DEF
                | SyntaxKind::CONSTRUCTOR_DEF
                | SyntaxKind::CONTRACT_DECL
                | SyntaxKind::MODULE_DEF
                | SyntaxKind::STRUCT_DEF
                | SyntaxKind::ENUM_DEF
        ) && let Some(range) = brace_span(&node)
        {
            out.push(FoldRange {
                range,
                kind: FoldKind::Region,
            });
        }
    }

    // 2. Leading consecutive import/include run (the root is always
    // SOURCE_FILE; children are the top-level items in document order).
    let import_run: Vec<SyntaxNode> = root
        .children()
        .take_while(|n| {
            matches!(n.kind(), SyntaxKind::IMPORT | SyntaxKind::INCLUDE)
                || n.kind() == SyntaxKind::PRAGMA
        })
        .filter(|n| matches!(n.kind(), SyntaxKind::IMPORT | SyntaxKind::INCLUDE))
        .collect();
    if let (Some(first), Some(last)) = (import_run.first(), import_run.last())
        && import_run.len() >= 2
    {
        out.push(FoldRange {
            range: TextRange::new(first.text_range().start(), last.text_range().end()),
            kind: FoldKind::Imports,
        });
    }

    // 3. Multi-line block comments.
    for tok in root
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
    {
        if tok.kind() == SyntaxKind::BLOCK_COMMENT && tok.text().contains('\n') {
            out.push(FoldRange {
                range: tok.text_range(),
                kind: FoldKind::Comment,
            });
        }
    }

    out
}

/// Byte range from a node's first `{` to its last `}` (start of `{` to start
/// of `}`), or `None` if either brace is missing.
fn brace_span(node: &SyntaxNode) -> Option<TextRange> {
    let mut open: Option<TextSize> = None;
    let mut close: Option<TextSize> = None;
    for t in node
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
    {
        match t.kind() {
            SyntaxKind::L_BRACE if open.is_none() => open = Some(t.text_range().start()),
            SyntaxKind::R_BRACE => close = Some(t.text_range().start()),
            _ => {}
        }
    }
    // Circuit bodies are a BLOCK child, so also look one level in.
    if open.is_none() || close.is_none() {
        if let Some(block) = node.children().find(|c| c.kind() == SyntaxKind::BLOCK) {
            for t in block
                .children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
            {
                match t.kind() {
                    SyntaxKind::L_BRACE if open.is_none() => open = Some(t.text_range().start()),
                    SyntaxKind::R_BRACE => close = Some(t.text_range().start()),
                    _ => {}
                }
            }
        }
    }
    Some(TextRange::new(open?, close?))
}
```

Register in `lib.rs`:

```rust
mod folding_ranges;
pub use folding_ranges::{folding_ranges, FoldKind, FoldRange};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p analyzer-ide folding_ranges`
Expected: PASS.

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`

```bash
git add crates/analyzer-ide/src/folding_ranges.rs crates/analyzer-ide/src/lib.rs
git commit -m "feat(ide): CST-derived folding ranges"
```

---

## Task 9: Selection ranges

**Files:**
- Create: `crates/analyzer-ide/src/selection_ranges.rs`
- Modify: `crates/analyzer-ide/src/lib.rs`
- Test: `crates/analyzer-ide/src/selection_ranges.rs`

**Interfaces:**
- Produces: `pub fn selection_ranges(host, file, offsets: &[TextSize]) -> Vec<Vec<TextRange>>` (Shared Interfaces) — one innermost-first chain per input offset.
- Consumes: F1.

- [ ] **Step 1: Write failing test**

Create `crates/analyzer-ide/src/selection_ranges.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, TextSize};

    #[test]
    fn chain_grows_outward_and_is_nested() {
        let source = "circuit f(): Field { return xyz; }";
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/m.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let off = TextSize::new(source.find("xyz").unwrap() as u32 + 1);
        let chains = selection_ranges(&mut host, file, &[off]);
        let chain = &chains[0];
        // Innermost first, each strictly contained in the next, last == file.
        assert_eq!(&source[chain[0]], "xyz");
        for w in chain.windows(2) {
            assert!(w[1].contains_range(w[0]) && w[1] != w[0]);
        }
        assert_eq!(chain.last().unwrap().end(), TextSize::new(source.len() as u32));
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p analyzer-ide selection_ranges`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
//! CST ancestor-chain selection ranges.

use analyzer_core::{AnalysisHost, FileId, SyntaxNode, TextRange, TextSize};
use rowan::TokenAtOffset;

/// For each offset, the chain of CST ranges from the token outward to the
/// file, innermost first, with consecutive duplicates removed.
pub fn selection_ranges(
    host: &mut AnalysisHost,
    file: FileId,
    offsets: &[TextSize],
) -> Vec<Vec<TextRange>> {
    let root = match host.analyze(file) {
        Some(a) => SyntaxNode::new_root(a.green.clone()),
        None => return offsets.iter().map(|_| Vec::new()).collect(),
    };
    offsets.iter().map(|&off| chain_at(&root, off)).collect()
}

fn chain_at(root: &SyntaxNode, offset: TextSize) -> Vec<TextRange> {
    let token = match root.token_at_offset(offset) {
        TokenAtOffset::None => return Vec::new(),
        TokenAtOffset::Single(t) => t,
        TokenAtOffset::Between(l, r) => {
            if !r.kind().is_trivia() {
                r
            } else {
                l
            }
        }
    };
    let mut ranges = vec![token.text_range()];
    for node in token.parent().into_iter().flat_map(|p| p.ancestors()) {
        let r = node.text_range();
        if ranges.last() != Some(&r) {
            ranges.push(r);
        }
    }
    ranges
}
```

Register in `lib.rs`:

```rust
mod selection_ranges;
pub use selection_ranges::selection_ranges;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p analyzer-ide selection_ranges`
Expected: PASS.

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-ide`

```bash
git add crates/analyzer-ide/src/selection_ranges.rs crates/analyzer-ide/src/lib.rs
git commit -m "feat(ide): CST ancestor-chain selection ranges"
```

---

## Task 10: Binary — completion handler + capability + trigger `.`

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs` (capability + handler)
- Modify: `crates/compact-analyzer/src/lsp_utils.rs` (`completion_kind_to_lsp`, `completion_item_to_lsp`)
- Create: `crates/compact-analyzer/tests/lsp_completion.rs`

**Interfaces:**
- Consumes: `analyzer_ide::{completion, CompletionItem, CompletionKind}`; F6 (completion types, `CompletionOptions.trigger_characters`, `Completion` METHOD); the harness in `tests/support/mod.rs`.

- [ ] **Step 1: Write the failing integration test**

Create `crates/compact-analyzer/tests/lsp_completion.rs`:

```rust
mod support;

use serde_json::json;
use support::{Client, did_open, lsp_position, temp_doc};

#[test]
fn completion_offers_ledger_methods_after_dot() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    let src = "export ledger cnt: Counter;\ncircuit f(): [] { cnt. }";
    did_open(&mut client, &uri, 1, src);
    let (line, col) = lsp_position(src, ". }"); // position OF the '.'
    let resp = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": col + 1}, // right after the '.'
        }),
    );
    let items = resp["result"].as_array().cloned().unwrap_or_default();
    let labels: Vec<String> = items
        .iter()
        .filter_map(|i| i["label"].as_str().map(str::to_string))
        .collect();
    assert!(labels.contains(&"increment".to_string()), "{labels:?}");
    assert!(labels.contains(&"resetToDefault".to_string()), "{labels:?}");
    client.shutdown();
}

#[test]
fn completion_offers_keywords_at_statement_start() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    let src = "circuit f(): Field {\n  \n}";
    did_open(&mut client, &uri, 1, src);
    // Blank line inside the body.
    let resp = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": 1, "character": 2},
        }),
    );
    let items = resp["result"].as_array().cloned().unwrap_or_default();
    let labels: Vec<String> = items
        .iter()
        .filter_map(|i| i["label"].as_str().map(str::to_string))
        .collect();
    assert!(labels.contains(&"return".to_string()), "{labels:?}");
    client.shutdown();
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p compact-analyzer --test lsp_completion`
Expected: FAIL — the server answers `method not supported` / null (no completion handler yet).

- [ ] **Step 3: Advertise the capability**

In `server.rs`, add to the `ServerCapabilities { … }` literal:

```rust
        completion_provider: Some(lsp_types::CompletionOptions {
            trigger_characters: Some(vec![".".to_string()]),
            resolve_provider: Some(false),
            ..Default::default()
        }),
```

- [ ] **Step 4: Add the mapping helpers to `lsp_utils.rs`**

```rust
pub(crate) fn completion_kind_to_lsp(
    kind: analyzer_ide::CompletionKind,
) -> lsp_types::CompletionItemKind {
    use analyzer_ide::CompletionKind as K;
    use lsp_types::CompletionItemKind as L;
    match kind {
        K::Keyword => L::KEYWORD,
        K::Circuit | K::Witness | K::StdlibItem => L::FUNCTION,
        K::Struct => L::STRUCT,
        K::StructField => L::FIELD,
        K::Enum => L::ENUM,
        K::EnumVariant => L::ENUM_MEMBER,
        K::Module => L::MODULE,
        K::TypeAlias => L::INTERFACE,
        K::LedgerField => L::PROPERTY,
        K::LedgerMethod => L::METHOD,
        K::Param | K::Local => L::VARIABLE,
        K::Generic => L::TYPE_PARAMETER,
        K::BuiltinType => L::KEYWORD,
    }
}

pub(crate) fn completion_item_to_lsp(
    c: analyzer_ide::CompletionItem,
) -> lsp_types::CompletionItem {
    lsp_types::CompletionItem {
        label: c.label,
        kind: Some(completion_kind_to_lsp(c.kind)),
        detail: c.detail,
        documentation: c.documentation.map(|value| {
            lsp_types::Documentation::MarkupContent(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value,
            })
        }),
        ..Default::default()
    }
}
```

- [ ] **Step 5: Add the request handler**

In `handle_request`'s `match req.method.as_str()`, add:

```rust
            "textDocument/completion" => {
                let result = serde_json::from_value::<lsp_types::CompletionParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let doc = params.text_document_position;
                        self.position_to_file_position(&doc.text_document.uri, doc.position)
                    })
                    .map(|pos| {
                        analyzer_ide::completion(&mut self.host, pos)
                            .into_iter()
                            .map(lsp_utils::completion_item_to_lsp)
                            .collect::<Vec<_>>()
                    })
                    .map(lsp_types::CompletionResponse::Array);
                self.respond_ok(req.id, result);
            }
```

- [ ] **Step 6: Run the integration test**

Run: `cargo test -p compact-analyzer --test lsp_completion`
Expected: PASS.

- [ ] **Step 7: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p compact-analyzer`

```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/src/lsp_utils.rs crates/compact-analyzer/tests/lsp_completion.rs
git commit -m "feat(server): textDocument/completion with dot trigger"
```

---

## Task 11: Binary — semantic tokens handler + legend + capability

**Files:**
- Create: `crates/compact-analyzer/src/semantic_tokens_legend.rs`
- Modify: `crates/compact-analyzer/src/main.rs` (`mod semantic_tokens_legend;`)
- Modify: `crates/compact-analyzer/src/server.rs` (capability + handler)
- Create: `crates/compact-analyzer/tests/lsp_semantic_tokens.rs`

**Interfaces:**
- Produces: `legend() -> SemanticTokensLegend`, `encode_semantic_tokens(&[SemToken], &LineIndex) -> Vec<SemanticToken>`.
- Consumes: `analyzer_ide::{semantic_tokens, SemToken, TokenType, TokenMods}`; F6.

**Context:** LSP semantic tokens are single-line, delta-encoded, UTF-16. Multi-line tokens (block comments) are skipped by the encoder — the token legend's `punctuation` type is a custom `SemanticTokenType::new(...)`.

- [ ] **Step 1: Write the failing integration test**

Create `crates/compact-analyzer/tests/lsp_semantic_tokens.rs`:

```rust
mod support;

use serde_json::json;
use support::{Client, did_open, temp_doc};

#[test]
fn semantic_tokens_full_returns_encoded_data() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    did_open(
        &mut client,
        &uri,
        1,
        "export circuit inc(x: Field): Field { return x + 1; }",
    );
    let resp = client.request(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": {"uri": uri} }),
    );
    let data = resp["result"]["data"].as_array().cloned().expect("data array");
    assert!(!data.is_empty());
    assert_eq!(data.len() % 5, 0, "5 ints per token");
    client.shutdown();
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p compact-analyzer --test lsp_semantic_tokens`
Expected: FAIL (no handler).

- [ ] **Step 3: Implement the legend + encoder**

Create `crates/compact-analyzer/src/semantic_tokens_legend.rs`:

```rust
//! Semantic-tokens legend and byte-range → LSP delta/UTF-16 encoding.

use analyzer_core::LineIndex;
use analyzer_ide::{SemToken, TokenMods, TokenType};
use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

/// Token-type legend (index order MUST match `token_type_index`).
fn legend_types() -> Vec<SemanticTokenType> {
    vec![
        SemanticTokenType::KEYWORD,        // 0
        SemanticTokenType::TYPE,           // 1
        SemanticTokenType::STRUCT,         // 2
        SemanticTokenType::ENUM,           // 3
        SemanticTokenType::ENUM_MEMBER,    // 4
        SemanticTokenType::TYPE_PARAMETER, // 5
        SemanticTokenType::PARAMETER,      // 6
        SemanticTokenType::VARIABLE,       // 7
        SemanticTokenType::PROPERTY,       // 8
        SemanticTokenType::FUNCTION,       // 9
        SemanticTokenType::METHOD,         // 10
        SemanticTokenType::NAMESPACE,      // 11
        SemanticTokenType::COMMENT,        // 12
        SemanticTokenType::STRING,         // 13
        SemanticTokenType::NUMBER,         // 14
        SemanticTokenType::OPERATOR,       // 15
        SemanticTokenType::new("punctuation"), // 16 (custom)
    ]
}

/// Modifier legend (bit order MUST match `token_mods_bitset`).
fn legend_modifiers() -> Vec<SemanticTokenModifier> {
    vec![
        SemanticTokenModifier::DECLARATION,     // bit 0
        SemanticTokenModifier::DEFAULT_LIBRARY, // bit 1
    ]
}

pub(crate) fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: legend_types(),
        token_modifiers: legend_modifiers(),
    }
}

fn token_type_index(ty: TokenType) -> u32 {
    match ty {
        TokenType::Keyword => 0,
        TokenType::Type => 1,
        TokenType::Struct => 2,
        TokenType::Enum => 3,
        TokenType::EnumMember => 4,
        TokenType::TypeParameter => 5,
        TokenType::Parameter => 6,
        TokenType::Variable => 7,
        TokenType::Property => 8,
        TokenType::Function => 9,
        TokenType::Method => 10,
        TokenType::Namespace => 11,
        TokenType::Comment => 12,
        TokenType::StringLit => 13,
        TokenType::Number => 14,
        TokenType::Operator => 15,
        TokenType::Punctuation => 16,
    }
}

fn token_mods_bitset(mods: TokenMods) -> u32 {
    let mut bits = 0;
    if mods.declaration {
        bits |= 1 << 0;
    }
    if mods.default_library {
        bits |= 1 << 1;
    }
    bits
}

/// Delta-encode absolute-range tokens into LSP `SemanticToken`s. Tokens that
/// span more than one line (e.g. block comments) are skipped — LSP semantic
/// tokens are single-line.
pub(crate) fn encode_semantic_tokens(tokens: &[SemToken], li: &LineIndex) -> Vec<SemanticToken> {
    let mut out = Vec::new();
    let mut prev_line = 0u32;
    let mut prev_col = 0u32;
    for t in tokens {
        let start = li.line_col(t.range.start());
        let end = li.line_col(t.range.end());
        if end.line != start.line {
            continue;
        }
        let length = end.col.saturating_sub(start.col);
        if length == 0 {
            continue;
        }
        let delta_line = start.line - prev_line;
        let delta_start = if delta_line == 0 {
            start.col - prev_col
        } else {
            start.col
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: token_type_index(t.ty),
            token_modifiers_bitset: token_mods_bitset(t.mods),
        });
        prev_line = start.line;
        prev_col = start.col;
    }
    out
}
```

Add `mod semantic_tokens_legend;` to `main.rs`.

- [ ] **Step 4: Advertise the capability + handler**

In `server.rs` capabilities literal:

```rust
        semantic_tokens_provider: Some(
            lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                lsp_types::SemanticTokensOptions {
                    work_done_progress_options: Default::default(),
                    legend: crate::semantic_tokens_legend::legend(),
                    range: Some(false),
                    full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                },
            ),
        ),
```

Handler:

```rust
            "textDocument/semanticTokens/full" => {
                let result = serde_json::from_value::<lsp_types::SemanticTokensParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let file = *self.open_files.get(&params.text_document.uri)?;
                        let li = self.host.analyze(file)?.line_index.clone();
                        let tokens = analyzer_ide::semantic_tokens(&mut self.host, file);
                        Some(lsp_types::SemanticTokens {
                            result_id: None,
                            data: crate::semantic_tokens_legend::encode_semantic_tokens(
                                &tokens, &li,
                            ),
                        })
                    })
                    .map(lsp_types::SemanticTokensResult::Tokens);
                self.respond_ok(req.id, result);
            }
```

- [ ] **Step 5: Run the integration test**

Run: `cargo test -p compact-analyzer --test lsp_semantic_tokens`
Expected: PASS.

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p compact-analyzer`

```bash
git add crates/compact-analyzer/src/semantic_tokens_legend.rs crates/compact-analyzer/src/main.rs crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests/lsp_semantic_tokens.rs
git commit -m "feat(server): textDocument/semanticTokens/full with legend"
```

---

## Task 12: Binary — folding + selection range handlers + capabilities

**Files:**
- Modify: `crates/compact-analyzer/src/server.rs` (2 capabilities + 2 handlers + 1 helper)
- Create: `crates/compact-analyzer/tests/lsp_structure.rs`

**Interfaces:**
- Consumes: `analyzer_ide::{folding_ranges, FoldKind, selection_ranges}`; `lsp_utils::range_to_lsp`; F6 (folding/selection types + METHODs).

- [ ] **Step 1: Write the failing integration test**

Create `crates/compact-analyzer/tests/lsp_structure.rs`:

```rust
mod support;

use serde_json::json;
use support::{Client, did_open, temp_doc};

#[test]
fn folding_ranges_cover_body_and_imports() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    did_open(
        &mut client,
        &uri,
        1,
        "import CompactStandardLibrary;\nimport Foo;\ncircuit f(): Field {\n  return 0;\n}",
    );
    let resp = client.request(
        "textDocument/foldingRange",
        json!({ "textDocument": {"uri": uri} }),
    );
    let ranges = resp["result"].as_array().cloned().expect("ranges");
    assert!(ranges.iter().any(|r| r["kind"] == "imports"));
    // The circuit body spans multiple lines → a fold with no/region kind.
    assert!(ranges.iter().any(|r| r["startLine"] != r["endLine"]));
    client.shutdown();
}

#[test]
fn selection_range_is_nested() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    let src = "circuit f(): Field { return xyz; }";
    did_open(&mut client, &uri, 1, src);
    // Position on `xyz`.
    let col = src.find("xyz").unwrap() as u32 + 1;
    let resp = client.request(
        "textDocument/selectionRange",
        json!({
            "textDocument": {"uri": uri},
            "positions": [{"line": 0, "character": col}],
        }),
    );
    let sel = &resp["result"][0];
    // Innermost range is the identifier; it has a parent chain.
    assert!(sel["parent"].is_object());
    client.shutdown();
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p compact-analyzer --test lsp_structure`
Expected: FAIL (no handlers).

- [ ] **Step 3: Advertise both capabilities**

In `server.rs` capabilities literal:

```rust
        folding_range_provider: Some(lsp_types::FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(lsp_types::SelectionRangeProviderCapability::Simple(true)),
```

- [ ] **Step 4: Add the folding handler**

```rust
            "textDocument/foldingRange" => {
                let result = serde_json::from_value::<lsp_types::FoldingRangeParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let file = *self.open_files.get(&params.text_document.uri)?;
                        let li = self.host.analyze(file)?.line_index.clone();
                        let folds = analyzer_ide::folding_ranges(&mut self.host, file);
                        Some(
                            folds
                                .into_iter()
                                .filter_map(|f| fold_to_lsp(&li, f))
                                .collect::<Vec<_>>(),
                        )
                    });
                self.respond_ok(req.id, result);
            }
```

- [ ] **Step 5: Add the selection handler**

```rust
            "textDocument/selectionRange" => {
                let result = serde_json::from_value::<lsp_types::SelectionRangeParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let file = *self.open_files.get(&params.text_document.uri)?;
                        let analysis = self.host.analyze(file)?;
                        let li = analysis.line_index.clone();
                        let offsets: Vec<analyzer_core::TextSize> = params
                            .positions
                            .iter()
                            .filter_map(|p| lsp_utils::offset_from_position(&li, *p))
                            .collect();
                        let chains = analyzer_ide::selection_ranges(&mut self.host, file, &offsets);
                        Some(
                            chains
                                .into_iter()
                                .filter_map(|chain| build_selection(&li, &chain))
                                .collect::<Vec<_>>(),
                        )
                    });
                self.respond_ok(req.id, result);
            }
```

- [ ] **Step 6: Add the two free helpers (bottom of `server.rs`)**

```rust
/// Byte-range fold → LSP `FoldingRange`; single-line ranges are dropped.
fn fold_to_lsp(
    li: &analyzer_core::LineIndex,
    fold: analyzer_ide::FoldRange,
) -> Option<lsp_types::FoldingRange> {
    let start = li.line_col(fold.range.start());
    let end = li.line_col(fold.range.end());
    if start.line == end.line {
        return None;
    }
    let kind = match fold.kind {
        analyzer_ide::FoldKind::Imports => Some(lsp_types::FoldingRangeKind::Imports),
        analyzer_ide::FoldKind::Comment => Some(lsp_types::FoldingRangeKind::Comment),
        analyzer_ide::FoldKind::Region => Some(lsp_types::FoldingRangeKind::Region),
    };
    Some(lsp_types::FoldingRange {
        start_line: start.line,
        start_character: None,
        end_line: end.line,
        end_character: None,
        kind,
        collapsed_text: None,
    })
}

/// Innermost-first byte-range chain → nested LSP `SelectionRange`.
fn build_selection(
    li: &analyzer_core::LineIndex,
    chain: &[analyzer_core::TextRange],
) -> Option<lsp_types::SelectionRange> {
    let mut cur: Option<Box<lsp_types::SelectionRange>> = None;
    for &r in chain.iter().rev() {
        cur = Some(Box::new(lsp_types::SelectionRange {
            range: lsp_utils::range_to_lsp(li, r),
            parent: cur,
        }));
    }
    cur.map(|b| *b)
}
```

- [ ] **Step 7: Run the integration test**

Run: `cargo test -p compact-analyzer --test lsp_structure`
Expected: PASS.

- [ ] **Step 8: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p compact-analyzer`

```bash
git add crates/compact-analyzer/src/server.rs crates/compact-analyzer/tests/lsp_structure.rs
git commit -m "feat(server): folding + selection range handlers"
```

---

## Task 13: Corpus smoke — exercise the four features at sampled positions

**Files:**
- Modify: `crates/compact-analyzer/tests/corpus_smoke.rs`

**Interfaces:**
- Consumes: the existing corpus-smoke harness; `analyzer_core::AnalysisHost`, `analyzer_ide::{completion, semantic_tokens, folding_ranges, selection_ranges}`.

**Context:** Run completion (at every token boundary of a sample of files), semantic tokens, folding, and selection over the corpus, asserting no panics and no out-of-bounds spans in error-recovered trees. Guard the sweep so one pathological file cannot abort the run (never-die). This extends, not replaces, the M2b corpus test.

- [ ] **Step 1: Read the existing corpus test to match its file-discovery + gating pattern**

Run: `sed -n '1,91p' crates/compact-analyzer/tests/corpus_smoke.rs`
Expected: note how it locates the corpus (env var / `../compactp/tests/corpus`), how it skips when absent (the CI-gated `corpus_smoke` test), and its per-file loop.

- [ ] **Step 2: Add the M3 feature sweep**

Append a test that reuses the corpus-locating helper (call it `corpus_dir()` — if the existing file names it differently, use that). Each file is analyzed and every new feature is invoked under `catch_unwind`; completion is sampled at each token's start + end offset (bounded), asserting returned ranges stay in bounds:

```rust
#[test]
fn m3_features_never_panic_on_corpus() {
    let Some(dir) = corpus_dir() else {
        eprintln!("corpus not present; skipping");
        return;
    };
    let files = analyzer_core::discover_compact_files(&[dir]);
    for path in files {
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let len = src.len() as u32;
        let result = std::panic::catch_unwind(|| {
            let mut host = analyzer_core::AnalysisHost::new();
            let file = host.vfs_mut().file_id(&path);
            host.vfs_mut().set_overlay(file, src.clone(), 1);

            // Semantic tokens: in-bounds, ordered.
            for t in analyzer_ide::semantic_tokens(&mut host, file) {
                assert!(u32::from(t.range.end()) <= len, "token OOB in {:?}", path);
            }
            // Folding + selection: in-bounds.
            for f in analyzer_ide::folding_ranges(&mut host, file) {
                assert!(u32::from(f.range.end()) <= len);
            }
            // Completion at a bounded sample of offsets (every ~16th byte on a
            // char boundary), including right after any '.' (member trigger).
            let mut offsets: Vec<analyzer_core::TextSize> = (0..=len)
                .step_by(16)
                .filter(|&o| src.is_char_boundary(o as usize))
                .map(analyzer_core::TextSize::new)
                .collect();
            for (i, _) in src.match_indices('.') {
                let o = (i + 1) as u32;
                if src.is_char_boundary(o as usize) {
                    offsets.push(analyzer_core::TextSize::new(o));
                }
            }
            for off in &offsets {
                let _ = analyzer_ide::completion(
                    &mut host,
                    analyzer_core::FilePosition { file, offset: *off },
                );
            }
            let chains = analyzer_ide::selection_ranges(&mut host, file, &offsets);
            for chain in chains {
                for r in chain {
                    assert!(u32::from(r.end()) <= len);
                }
            }
        });
        assert!(result.is_ok(), "M3 features panicked on {:?}", path);
    }
}
```

If the existing file has no `corpus_dir()` helper, factor the corpus-location logic the existing test uses into one, and have both tests call it.

- [ ] **Step 3: Run it (locally, where the corpus is present)**

Run: `cargo test -p compact-analyzer --test corpus_smoke`
Expected: PASS locally (corpus present); the test self-skips in CI where the corpus is absent, matching the existing `corpus_smoke` behavior (baseline: 129 local / 128 CI).

- [ ] **Step 4: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p compact-analyzer`

```bash
git add crates/compact-analyzer/tests/corpus_smoke.rs
git commit -m "test(server): corpus smoke exercises completion, tokens, folding, selection"
```

---

## Self-review notes (deliberate, reviewed departures from the spec)

1. **Feature logic lives in `analyzer-ide`, which gains `compactp_ast` + `rowan` deps** (Task 3). The spec's §3.1 framed core as owning the "completion query" and "token-role classification"; in the plan the classifier/candidate/token logic lives in `analyzer-ide` because splitting the AST walk across the core/ide boundary is more convoluted than letting the syntactic-feature layer touch the syntax tree (as rust-analyzer's ide layer does). Core still owns the shared *scope enumeration* (`scope_bindings_at`), the *ledger table*, and *receiver typing* — the genuinely reusable semantic queries. The invariant that actually matters — **no `lsp-types` in core/ide** — is preserved.
2. **"Cell is implicit" enrichment** (F7, Task 2): a plain-typed ledger field (`ledger x: Uint<64>;`) is treated as a `Cell<T>` and offers `read`/`write`/`resetToDefault`. This goes beyond the spec's explicit-ADT-only framing; it is empirically confirmed (probe: `Cell` is not a writable field type) and makes member completion useful on the common case.
3. **Excluded ledger methods** (F7): js-only (runtime-only) methods and coin-conditional methods are omitted from the completion surface — a documented limitation, because the former are rejected in-circuit and the latter need type info to offer correctly.
4. **String-path import member enumeration in completion is deferred.** Completion covers locals, in-file/module items, identifier-import members, and stdlib exports (the resolvable set). Enumerating members brought in by `import "path";` is a follow-up — consistent with M2b's precedent of not extending every cross-file surface at once. (Goto/hover on those names still work via M2's resolver; only the *completion listing* is deferred.)
5. **Semantic tokens skip multi-line tokens at the binary encoder** (Task 11): LSP semantic tokens are single-line, so a multi-line `BLOCK_COMMENT` is not emitted as a semantic token (its coloring is left to the M5 TextMate grammar). Folding still folds multi-line block comments (Task 8). All single-line tokens — the vast majority — are emitted, honoring the "comprehensive" decision.
6. **`MULTI_CONST_STMT` resolver fix rides along** (Task 1, F3): a real latent bug (multi-binding `const a = 1, b = 2;` bindings were invisible to resolution) is fixed by the shared-enumeration refactor and locked in by a characterization test — the kind of edge the brief predicted completion would surface.

## Errata

_(Empty at authoring time. During execution, when a step's own code/fixture/API turns out wrong, fix it empirically, record the correction here with the task number and the root cause, and commit the Errata edit promptly — uncommitted plan edits get reverted by implementer subagents' git operations, as bit M2b.)_

## Definition of Done

- All 13 tasks complete; every task's gate (`cargo fmt` + `cargo clippy --workspace --all-targets --locked -- -D warnings` + tests) green; per-task commit landed.
- Test count risen from the 129 baseline (new: ~3 resolve, ~2 ledger_adts, ~9 completion, ~2 hover, ~3 semantic_tokens, ~3 folding, ~1 selection unit tests; ~2 lsp_completion, ~1 lsp_semantic_tokens, ~2 lsp_structure integration; ~1 corpus sweep) — record the exact final count in `.superpowers/sdd/progress.md`.
- Final whole-branch review on the most capable model (base = `git merge-base main HEAD`); one fix subagent for any Critical/Important findings; re-verify.
- `superpowers:finishing-a-development-branch` → **fast-forward merge to `main` locally, then push**.
- Update the `milestone-status` memory and flip **M3 → Done** in `.superpowers/milestones/README.md` (commit the flip on the branch so it merges with M3).
