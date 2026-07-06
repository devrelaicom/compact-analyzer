# compact-analyzer M2a: Single-File Navigation + Stdlib Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Real language intelligence within a file: goto-definition, find-references, rename, hover, and document symbols — covering locals, top-level items, in-file modules, and the bundled `CompactStandardLibrary` — so a typical single-file Compact contract gets full navigation.

**Architecture:** analyzer-core gains an `ItemTree` (per-file symbol extraction with rendered signatures and doc comments), a bundled stdlib stub registered as a real materialized file, and a syntactic resolver (token classification by parent CST kind → lexical scopes → file scope → imports). A new thin `analyzer-ide` crate exposes editor-agnostic feature functions over core. The binary wires five new LSP capabilities, converting positions at the boundary as in M1.

**Tech Stack:** unchanged from M1 (compactp 0.1.0-beta.1, lsp-server 0.8, lsp-types 0.95.1, rowan 0.16). New crate `analyzer-ide`; analyzer-core adds `compactp_ast` + `compactp_syntax` as runtime deps.

## Milestone map note

The spec's M2 is split into two plans: **M2a (this plan)** — single-file + stdlib resolution and all navigation features; **M2b (next)** — filesystem `import`/`include` resolution, workspace discovery/index, cross-file references/rename, workspace symbols. The split ships usable navigation sooner; the compiler's exact file-resolution semantics (verified from Scheme source) are recorded in the M2b planning notes below and NOT implemented here.

## Global Constraints

- Everything from the M1 plan's Global Constraints still binds (never-die, stdout protocol-only, byte offsets in core/ide, UTF-16 only in the binary, fmt/clippy `-D warnings`/tests green per task, conventional commits).
- New crate `analyzer-ide`: editor-agnostic, speaks `TextSize`/`TextRange`/`FileId`, zero `lsp-types`.
- **Verified CST facts (from compactp 0.1.0-beta.1 source + live `compactp cst` runs — the resolver's foundation; do not "discover" otherwise):**
  - There is NO name-reference node kind. References are bare `IDENT` tokens classified by PARENT kind: `NAME_EXPR` (value ref), `CALL_EXPR` (direct callee: the IDENT is a direct child of CALL_EXPR, NOT wrapped in NAME_EXPR; method receiver IS a child NAME_EXPR), `MEMBER_EXPR` (field after `.`), `TYPE_REF` (type ref), `STRUCT_EXPR` (struct literal name), `STRUCT_FIELD_INIT` (literal field label), `TYPE_SIZE` (nat-generic size reference, e.g. `n` in `Vector<n, Field>`).
  - `NameExpr::ident()` can return `None` (a `NAME_EXPR` may wrap `LEDGER_KW`).
  - Expression structs live at `compactp_ast::expr::*`; only the `Expr` enum is re-exported at crate root. `Item`, `Stmt`, `Pat`, `Type` enums and all item/statement/pattern structs are at crate root.
  - `ModuleDef` has NO members accessor — iterate `module.syntax().children().filter_map(Item::cast)`.
  - `Import` accessors: `name()` (IDENT source), `path()` (STRING_LIT source), `prefix() -> Option<PrefixDecl>` (`PrefixDecl::name()`), `generic_args()`. There is NO `specifiers()` accessor — enumerate via `import.syntax().descendants().filter_map(ImportSpecifier::cast)`. `ImportSpecifier::name()` returns the ORIGINAL name; the `as` alias has no accessor — read it by walking `specifier.syntax().children_with_tokens()` for the IDENT after `AS_KW`.
  - Pattern bindings: `Pat::Ident(p)` → `p.name()`; `Pat::Tuple(t)` → recurse `t.elements()` → `TuplePatElt::pattern()`; `Pat::Struct(s)` → per `StructPatField`: if `pattern()` is Some recurse, else `name()` IS the binding.
  - `ConstStmt`: `pattern()`, `ty()`, `value()`. `ForStmt`: `var_name()`, `body()`. `LambdaExpr`: `param_list()`, `body_block()` (expr-bodied lambdas have no body accessor — treat "offset past param_list end" as in-body). `Param`: `pattern()`, `ty()`.
  - Doc comments: no `///` convention exists; there are only `LINE_COMMENT`/`BLOCK_COMMENT` kinds, attached as leading trivia INSIDE the following item's node. Extract docs from leading `LINE_COMMENT` tokens of the item node.
  - Trivia check is `SyntaxKind::is_trivia()`.
- **Verified stdlib facts (from compiler source @ tag compactc-v0.31.0):** struct fields are snake_case (`is_some`, `goes_left`, `mt_index`) — the docs' camelCase is post-0.31; `ecMul`/`ecMulGenerator` take `Field` scalars; `ecNeg`, `keccak256`, and secp256k1 items do NOT exist at 0.31 — exclude them. `import CompactStandardLibrary;` is compiler-internal (never a file lookup) — the analyzer mirrors this by resolving that exact import name to its bundled stub.
- **Verified parse fixtures (ran through compactp 0.1.0-beta.1, zero errors — tests may rely on them parsing cleanly):** the full stdlib stub in Task 2, and the resolver fixture in Task 3.
- Rename keyword refusal list (active keywords, from the language docs at 0.23): `as assert Boolean Bytes circuit const constructor contract default disclose else enum export false Field fold for from if import include ledger map module new of Opaque pad pragma prefix pure return sealed slice struct true type Uint Vector witness`.
- M2a resolution scope: locals, generics, top-level items, in-file module members via `import M;`, selective imports with aliases, prefix imports, and the bundled stdlib. Imports that would require filesystem search (string paths, or identifier imports with no matching in-file module) resolve to `None` — silently, no diagnostic (M2b's job). Builtin ledger ADT type names (`Counter`, `Cell`, `Map`, `Set`, `List`, `MerkleTree`, `HistoricMerkleTree`, `Kernel`) and their methods have no definition location — resolve to `None`.
- **M2b planning notes (verified compiler semantics, recorded here so they aren't lost; NOT implemented in M2a):** `find-source-pathname` ALWAYS appends `.compact` (extension must be omitted in source); search order is absolute-exact, else importing file's directory first then COMPACT_PATH entries left-to-right; `import Foo;` consults in-scope modules FIRST, filesystem second (`Foo.compact` must contain exactly `module Foo {...}` and nothing else); string imports never consult in-scope modules and derive the expected module name from the last path component; includes splice textually, cycle-detected along the active path only, duplicates NOT deduplicated; imports are memoized per (name, pathname).

---

### Task 1: analyzer-core — ItemTree (symbols, signatures, docs)

**Files:**
- Create: `crates/analyzer-core/src/item_tree.rs`
- Modify: `crates/analyzer-core/src/lib.rs`
- Modify: `crates/analyzer-core/src/analysis.rs`
- Modify: `crates/analyzer-core/Cargo.toml` (add `compactp_ast.workspace = true`, `compactp_syntax.workspace = true` to `[dependencies]`; remove `compactp_syntax` from `[dev-dependencies]`)

**Interfaces:**
- Consumes: `FileAnalysis` (M1), `compactp_ast`, `compactp_syntax`
- Produces (used by all later tasks):
  - `analyzer_core::SymbolKind` — `Circuit, CircuitSig, Witness, Struct, StructField, Enum, EnumVariant, Module, TypeAlias, Ledger, Contract, ContractCircuit, Constructor` (`Clone, Copy, Debug, PartialEq, Eq`)
  - `analyzer_core::Symbol { pub name: String, pub kind: SymbolKind, pub name_range: TextRange, pub full_range: TextRange, pub signature: String, pub doc: Option<String>, pub exported: bool, pub parent: Option<u32> }` (`Clone, Debug`)
  - `analyzer_core::ItemTree { pub symbols: Vec<Symbol> }` with `extract(root: &SyntaxNode) -> ItemTree`, `top_level(&self) -> impl Iterator<Item = (u32, &Symbol)>`, `children_of(&self, parent: u32) -> impl Iterator<Item = (u32, &Symbol)>`
  - `FileAnalysis` gains `pub item_tree: Arc<ItemTree>`
  - lib.rs re-exports: `pub use item_tree::{ItemTree, Symbol, SymbolKind};` and `pub use compactp_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};`

- [ ] **Step 1: Write the failing tests**

Create `crates/analyzer-core/src/item_tree.rs` with the tests module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn tree(source: &str) -> ItemTree {
        let result = compactp_parser::parse(source);
        assert!(result.errors.is_empty(), "fixture must parse cleanly");
        ItemTree::extract(&compactp_syntax::SyntaxNode::new_root(result.green))
    }

    fn find<'t>(t: &'t ItemTree, name: &str) -> &'t Symbol {
        t.symbols.iter().find(|s| s.name == name).expect(name)
    }

    #[test]
    fn extracts_top_level_items_with_kinds() {
        let t = tree(
            "export ledger count: Counter;\n\
             struct Point { x: Field; y: Field; }\n\
             enum Color { red, blue }\n\
             export circuit main(a: Field): Field { return a; }\n\
             witness secret(): Field;\n\
             type Pair = [Field, Field];\n",
        );
        assert_eq!(find(&t, "count").kind, SymbolKind::Ledger);
        assert!(find(&t, "count").exported);
        assert_eq!(find(&t, "Point").kind, SymbolKind::Struct);
        assert!(!find(&t, "Point").exported);
        assert_eq!(find(&t, "Color").kind, SymbolKind::Enum);
        assert_eq!(find(&t, "main").kind, SymbolKind::Circuit);
        assert_eq!(find(&t, "secret").kind, SymbolKind::Witness);
        assert_eq!(find(&t, "Pair").kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn fields_variants_and_module_members_have_parents() {
        let t = tree(
            "struct Point { x: Field; y: Field; }\n\
             enum Color { red, blue }\n\
             module M { export circuit f(): Field { return 0; } }\n",
        );
        let (point_idx, _) = t.top_level().find(|(_, s)| s.name == "Point").unwrap();
        let fields: Vec<_> = t.children_of(point_idx).map(|(_, s)| s.name.clone()).collect();
        assert_eq!(fields, vec!["x", "y"]);
        assert_eq!(find(&t, "x").kind, SymbolKind::StructField);
        assert_eq!(find(&t, "red").kind, SymbolKind::EnumVariant);
        let (m_idx, _) = t.top_level().find(|(_, s)| s.name == "M").unwrap();
        let members: Vec<_> = t.children_of(m_idx).map(|(_, s)| s.name.clone()).collect();
        assert_eq!(members, vec!["f"]);
        assert!(find(&t, "f").exported);
    }

    #[test]
    fn name_range_covers_exactly_the_name() {
        let source = "circuit main(): Field { return 0; }";
        let t = tree(source);
        let sym = find(&t, "main");
        assert_eq!(&source[sym.name_range], "main");
        assert_eq!(u32::from(sym.full_range.start()), 0);
    }

    #[test]
    fn signature_is_rendered_without_body() {
        let t = tree("export circuit main(a: Field,\n    b: Field): Field { return a; }");
        assert_eq!(
            find(&t, "main").signature,
            "export circuit main(a: Field, b: Field): Field"
        );
        let t = tree("struct Point { x: Field; }");
        assert_eq!(find(&t, "Point").signature, "struct Point");
        let t = tree("export ledger count: Counter;");
        assert_eq!(find(&t, "count").signature, "export ledger count: Counter");
    }

    #[test]
    fn leading_line_comments_become_docs() {
        let t = tree(
            "// Adds one.\n// Wraps at the field modulus.\ncircuit inc(x: Field): Field { return x + 1; }",
        );
        assert_eq!(
            find(&t, "inc").doc.as_deref(),
            Some("Adds one.\nWraps at the field modulus.")
        );
        let t = tree("circuit bare(): [] { }");
        assert_eq!(find(&t, "bare").doc, None);
    }

    #[test]
    fn error_recovered_files_still_extract() {
        // Broken item between two good ones: extraction must not panic and
        // must still find the good symbols.
        let result = compactp_parser::parse("circuit a(): [] { }\n@@@\ncircuit b(): [] { }");
        let t = ItemTree::extract(&compactp_syntax::SyntaxNode::new_root(result.green));
        assert!(t.symbols.iter().any(|s| s.name == "a"));
        assert!(t.symbols.iter().any(|s| s.name == "b"));
    }
}
```

Register in `crates/analyzer-core/src/lib.rs` (append):

```rust
mod item_tree;

pub use compactp_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
pub use item_tree::{ItemTree, Symbol, SymbolKind};
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core`
Expected: FAIL — compile errors (`cannot find type ItemTree`; also `compactp_ast`/`compactp_syntax` unresolved until Cargo.toml is updated in Step 3)

- [ ] **Step 3: Update Cargo.toml and write the implementation**

In `crates/analyzer-core/Cargo.toml`, add to `[dependencies]`:

```toml
compactp_ast.workspace = true
compactp_syntax.workspace = true
```

and delete the `compactp_syntax.workspace = true` line from `[dev-dependencies]` (it is now a full dependency).

Prepend to `crates/analyzer-core/src/item_tree.rs`:

```rust
//! Per-file symbol extraction: every declaration with its name span,
//! rendered signature, doc comment, and container. This is the flat,
//! cheap-to-rebuild structure everything in M2 navigates over.

use compactp_ast::{AstNode, Item};
use compactp_syntax::{SyntaxKind, SyntaxNode};
use text_size::TextRange;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Circuit,
    CircuitSig,
    Witness,
    Struct,
    StructField,
    Enum,
    EnumVariant,
    Module,
    TypeAlias,
    Ledger,
    Contract,
    ContractCircuit,
    Constructor,
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// Range of the name identifier (== `full_range` for anonymous items).
    pub name_range: TextRange,
    /// Range of the whole declaration.
    pub full_range: TextRange,
    /// One-line signature for hover, rendered from source.
    pub signature: String,
    /// Leading `//` comment lines, if any.
    pub doc: Option<String>,
    pub exported: bool,
    /// Index of the containing symbol (module, struct, enum, contract).
    pub parent: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct ItemTree {
    pub symbols: Vec<Symbol>,
}

impl ItemTree {
    pub fn extract(root: &SyntaxNode) -> ItemTree {
        let mut tree = ItemTree::default();
        let Some(file) = compactp_ast::SourceFile::cast(root.clone()) else {
            return tree;
        };
        for item in file.items() {
            tree.push_item(&item, None);
        }
        tree
    }

    pub fn top_level(&self) -> impl Iterator<Item = (u32, &Symbol)> {
        self.iter().filter(|(_, s)| s.parent.is_none())
    }

    pub fn children_of(&self, parent: u32) -> impl Iterator<Item = (u32, &Symbol)> {
        self.iter().filter(move |(_, s)| s.parent == Some(parent))
    }

    fn iter(&self) -> impl Iterator<Item = (u32, &Symbol)> {
        self.symbols.iter().enumerate().map(|(i, s)| (i as u32, s))
    }

    fn push(&mut self, symbol: Symbol) -> u32 {
        self.symbols.push(symbol);
        (self.symbols.len() - 1) as u32
    }

    fn push_item(&mut self, item: &Item, parent: Option<u32>) {
        match item {
            Item::CircuitDef(it) => {
                self.push_named(it.syntax(), it.name(), SymbolKind::Circuit, it.is_exported(), parent);
            }
            Item::CircuitDecl(it) => {
                self.push_named(it.syntax(), it.name(), SymbolKind::CircuitSig, it.is_exported(), parent);
            }
            Item::WitnessDecl(it) => {
                self.push_named(it.syntax(), it.name(), SymbolKind::Witness, it.is_exported(), parent);
            }
            Item::LedgerDecl(it) => {
                self.push_named(it.syntax(), it.name(), SymbolKind::Ledger, it.is_exported(), parent);
            }
            Item::TypeDecl(it) => {
                self.push_named(it.syntax(), it.name(), SymbolKind::TypeAlias, it.is_exported(), parent);
            }
            Item::ConstructorDef(it) => {
                let node = it.syntax();
                self.push(Symbol {
                    name: "constructor".to_string(),
                    kind: SymbolKind::Constructor,
                    name_range: node.text_range(),
                    full_range: node.text_range(),
                    signature: render_signature(node),
                    doc: leading_docs(node),
                    exported: false,
                    parent,
                });
            }
            Item::StructDef(it) => {
                let idx =
                    self.push_named(it.syntax(), it.name(), SymbolKind::Struct, it.is_exported(), parent);
                for field in it.fields() {
                    if let Some(name) = field.name() {
                        let ty = field.ty().map(|t| collapse_ws(&t.syntax().text().to_string()));
                        let signature = match ty {
                            Some(ty) => format!("{}: {}", name.text(), ty),
                            None => name.text().to_string(),
                        };
                        self.push(Symbol {
                            name: name.text().to_string(),
                            kind: SymbolKind::StructField,
                            name_range: name.text_range(),
                            full_range: field.syntax().text_range(),
                            signature,
                            doc: leading_docs(field.syntax()),
                            exported: it.is_exported(),
                            parent: Some(idx),
                        });
                    }
                }
            }
            Item::EnumDef(it) => {
                let idx =
                    self.push_named(it.syntax(), it.name(), SymbolKind::Enum, it.is_exported(), parent);
                for variant in it.variants() {
                    if let Some(name) = variant.name() {
                        self.push(Symbol {
                            name: name.text().to_string(),
                            kind: SymbolKind::EnumVariant,
                            name_range: name.text_range(),
                            full_range: variant.syntax().text_range(),
                            signature: name.text().to_string(),
                            doc: None,
                            exported: it.is_exported(),
                            parent: Some(idx),
                        });
                    }
                }
            }
            Item::ContractDecl(it) => {
                let idx = self.push_named(
                    it.syntax(),
                    it.name(),
                    SymbolKind::Contract,
                    it.is_exported(),
                    parent,
                );
                for circuit in it.circuits() {
                    if let Some(name) = circuit.name() {
                        self.push(Symbol {
                            name: name.text().to_string(),
                            kind: SymbolKind::ContractCircuit,
                            name_range: name.text_range(),
                            full_range: circuit.syntax().text_range(),
                            signature: render_signature(circuit.syntax()),
                            doc: leading_docs(circuit.syntax()),
                            exported: it.is_exported(),
                            parent: Some(idx),
                        });
                    }
                }
            }
            Item::ModuleDef(it) => {
                let idx = self.push_named(
                    it.syntax(),
                    it.name(),
                    SymbolKind::Module,
                    it.is_exported(),
                    parent,
                );
                // ModuleDef exposes no members accessor: iterate direct
                // children castable to Item (verified compactp pattern).
                for child in it.syntax().children().filter_map(Item::cast) {
                    self.push_item(&child, Some(idx));
                }
            }
            // Not symbols: handled by the resolver directly.
            Item::Pragma(_) | Item::Include(_) | Item::Import(_) | Item::ExportList(_) => {}
        }
    }

    fn push_named(
        &mut self,
        node: &SyntaxNode,
        name: Option<compactp_syntax::SyntaxToken>,
        kind: SymbolKind,
        exported: bool,
        parent: Option<u32>,
    ) -> u32 {
        let (name_text, name_range) = match &name {
            Some(token) => (token.text().to_string(), token.text_range()),
            // Error-recovered item without a name: skip silently but still
            // return a valid parent index by pushing a placeholder-free path.
            None => (String::new(), node.text_range()),
        };
        self.push(Symbol {
            name: name_text,
            kind,
            name_range,
            full_range: node.text_range(),
            signature: render_signature(node),
            doc: leading_docs(node),
            exported,
            parent,
        })
    }
}

/// Renders a one-line signature: the item's tokens up to (not including)
/// its body block or brace list, whitespace collapsed, trailing `;`/`{`
/// trimmed.
fn render_signature(node: &SyntaxNode) -> String {
    let mut out = String::new();
    let body_start: Option<text_size::TextSize> = node
        .children()
        .find(|c| c.kind() == SyntaxKind::BLOCK)
        .map(|c| c.text_range().start())
        .or_else(|| {
            // struct/enum/contract/module bodies are brace lists directly in
            // the item node: stop at the first `{` token.
            node.children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == SyntaxKind::L_BRACE)
                .map(|t| t.text_range().start())
        });
    for element in node.descendants_with_tokens() {
        let Some(token) = element.into_token() else { continue };
        if let Some(stop) = body_start {
            if token.text_range().start() >= stop {
                break;
            }
        }
        if token.kind().is_trivia() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push_str(token.text());
        }
    }
    let trimmed = out.trim().trim_end_matches(';').trim_end().to_string();
    collapse_ws(&trimmed)
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Leading `//` comment lines attached inside the node (compactp attaches
/// preceding trivia as the node's leading tokens), stripped of slashes.
fn leading_docs(node: &SyntaxNode) -> Option<String> {
    let mut lines = Vec::new();
    for element in node.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::WHITESPACE => continue,
            rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::LINE_COMMENT => {
                lines.push(t.text().trim_start_matches('/').trim().to_string());
            }
            _ => break,
        }
    }
    if lines.is_empty() { None } else { Some(lines.join("\n")) }
}
```

NOTE for the implementer: `SyntaxKind::L_BRACE` and `SyntaxKind::BLOCK` are the expected variant names; if the compactp `SyntaxKind` enum names differ (e.g. `LBRACE`), check `compactp_syntax`'s docs.rs or `syntax_kind.rs` and use the actual names — report the substitution in your report. Indexing `&source[sym.name_range]` in tests works because `text_size::TextRange` implements the required slice-index traits.

In `crates/analyzer-core/src/analysis.rs`: add `pub item_tree: Arc<ItemTree>` to `FileAnalysis`, and in `analyze()` construct it after parsing:

```rust
        let root = compactp_syntax::SyntaxNode::new_root(result.green.clone());
        let analysis = FileAnalysis {
            green: result.green,
            diagnostics: Arc::new(result.errors),
            line_index: Arc::new(LineIndex::new(text)),
            item_tree: Arc::new(crate::item_tree::ItemTree::extract(&root)),
        };
```

(with `use crate::item_tree::ItemTree;` — adjust the existing `green` move: clone it for `new_root` first, exactly as shown.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core`
Expected: PASS — 23 tests (17 existing + 6 new)

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: no warnings

```bash
git add crates/analyzer-core
git commit -m "feat(core): add ItemTree symbol extraction with signatures and docs"
```

---

### Task 2: analyzer-core — bundled stdlib stub + registration

**Files:**
- Create: `crates/analyzer-core/assets/std_0_23.compact`
- Create: `crates/analyzer-core/src/stdlib.rs`
- Modify: `crates/analyzer-core/src/lib.rs`
- Modify: `crates/analyzer-core/src/analysis.rs`

**Interfaces:**
- Consumes: `Vfs`, `AnalysisHost` (M1), `ItemTree` (Task 1)
- Produces (used by Tasks 4-9):
  - `analyzer_core::stdlib::STDLIB_SOURCE: &str` (the embedded stub text)
  - `analyzer_core::stdlib::materialize(dir: &Path) -> std::io::Result<PathBuf>` — writes `CompactStandardLibrary.compact` into `dir` if missing or stale, returns its path
  - `AnalysisHost::register_stdlib(&mut self, path: &Path) -> FileId` and `AnalysisHost::stdlib_file(&self) -> Option<FileId>`

- [ ] **Step 1: Create the stub asset**

Create `crates/analyzer-core/assets/std_0_23.compact` with EXACTLY this content (verified: parses with compactp 0.1.0-beta.1 with zero errors; signatures verbatim from compiler source @ compactc-v0.31.0; bodies are dummies):

```compact
// CompactStandardLibrary — signature stub bundled with compact-analyzer.
// Pinned to language 0.23 (compiler tag compactc-v0.31.0).
// Signatures verbatim from compiler/standard-library.compact and
// compiler/midnight-natives.ss at that tag; bodies are dummies (the
// analyzer needs declarations, not behavior).

// The kernel ledger giving access to Zswap/token/time operations.
export ledger kernel: Kernel;

// Encapsulates an optionally present value.
export struct Maybe<T> { is_some: Boolean; value: T; }
// Disjoint union of two alternatives.
export struct Either<A, B> { is_left: Boolean; left: A; right: B; }
// Root hash (digest) of a Merkle tree.
export struct MerkleTreeDigest { field: Field; }
// One authentication-path step: sibling digest plus direction.
export struct MerkleTreePathEntry { sibling: MerkleTreeDigest; goes_left: Boolean; }
// Full authentication path to a leaf in a depth-n tree.
export struct MerkleTreePath<#n, T> { leaf: T; path: Vector<n, MerkleTreePathEntry>; }
// A contract address.
export struct ContractAddress { bytes: Bytes<32>; }
// Description of a newly created shielded coin.
export struct ShieldedCoinInfo { nonce: Bytes<32>; color: Bytes<32>; value: Uint<128>; }
// An existing ledger shielded coin, ready to spend.
export struct QualifiedShieldedCoinInfo { nonce: Bytes<32>; color: Bytes<32>; value: Uint<128>; mt_index: Uint<64>; }
// Public key for receiving shielded coins.
export struct ZswapCoinPublicKey { bytes: Bytes<32>; }
// Result of a shielded send: coin sent plus optional change.
export struct ShieldedSendResult { change: Maybe<ShieldedCoinInfo>; sent: ShieldedCoinInfo; }
// A user's public address.
export struct UserAddress { bytes: Bytes<32>; }

// Opaque embedded-curve (Jubjub) point.
export new type JubjubPoint = Opaque<'JubjubPoint'>;

// Wrap a value as a present Maybe.
export circuit some<T>(value: T): Maybe<T> { return default<Maybe<T>>; }
// The empty Maybe.
export circuit none<T>(): Maybe<T> { return default<Maybe<T>>; }
// Construct the left variant.
export circuit left<A, B>(value: A): Either<A, B> { return default<Either<A, B>>; }
// Construct the right variant.
export circuit right<A, B>(value: B): Either<A, B> { return default<Either<A, B>>; }
// Compute the Merkle root implied by a leaf and its path.
export circuit merkleTreePathRoot<#n, T>(path: MerkleTreePath<n, T>): MerkleTreeDigest { return default<MerkleTreeDigest>; }
// As merkleTreePathRoot, but the leaf is already hashed.
export circuit merkleTreePathRootNoLeafHash<#n>(path: MerkleTreePath<n, Bytes<32>>): MerkleTreeDigest { return default<MerkleTreeDigest>; }
// Token type (color) of the native token.
export circuit nativeToken(): Bytes<32> { return default<Bytes<32>>; }
// Derive a namespaced token type from a domain separator and contract.
export circuit tokenType(domain_sep: Bytes<32>, contractAddress: ContractAddress): Bytes<32> { return default<Bytes<32>>; }
// Mint a shielded coin and send it to recipient.
export circuit mintShieldedToken(domain_sep: Bytes<32>, value: Uint<64>, nonce: Bytes<32>, recipient: Either<ZswapCoinPublicKey, ContractAddress>): ShieldedCoinInfo { return default<ShieldedCoinInfo>; }
// Deterministically derive a new nonce from a counter and prior nonce.
export circuit evolveNonce(index: Uint<128>, nonce: Bytes<32>): Bytes<32> { return default<Bytes<32>>; }
// A recipient that provably destroys a coin.
export circuit shieldedBurnAddress(): Either<ZswapCoinPublicKey, ContractAddress> { return default<Either<ZswapCoinPublicKey, ContractAddress>>; }
// Receive a shielded coin into the current contract.
export circuit receiveShielded(coin: ShieldedCoinInfo): [] { }
// Spend a ledger coin, sending value and returning change.
export circuit sendShielded(input: QualifiedShieldedCoinInfo, recipient: Either<ZswapCoinPublicKey, ContractAddress>, value: Uint<128>): ShieldedSendResult { return default<ShieldedSendResult>; }
// Send from a coin created earlier in the same transaction.
export circuit sendImmediateShielded(input: ShieldedCoinInfo, target: Either<ZswapCoinPublicKey, ContractAddress>, value: Uint<128>): ShieldedSendResult { return default<ShieldedSendResult>; }
// Merge two ledger coins of the same color.
export circuit mergeCoin(a: QualifiedShieldedCoinInfo, b: QualifiedShieldedCoinInfo): ShieldedCoinInfo { return default<ShieldedCoinInfo>; }
// Merge a ledger coin with a transaction-created coin.
export circuit mergeCoinImmediate(a: QualifiedShieldedCoinInfo, b: ShieldedCoinInfo): ShieldedCoinInfo { return default<ShieldedCoinInfo>; }
// True if current block time < time.
export circuit blockTimeLt(time: Uint<64>): Boolean { return default<Boolean>; }
// True if current block time >= time.
export circuit blockTimeGte(time: Uint<64>): Boolean { return default<Boolean>; }
// True if current block time > time.
export circuit blockTimeGt(time: Uint<64>): Boolean { return default<Boolean>; }
// True if current block time <= time.
export circuit blockTimeLte(time: Uint<64>): Boolean { return default<Boolean>; }
// Mint an unshielded token; returns its color.
export circuit mintUnshieldedToken(domainSep: Bytes<32>, amount: Uint<64>, recipient: Either<ContractAddress, UserAddress>): Bytes<32> { return default<Bytes<32>>; }
// Send unshielded tokens to a recipient.
export circuit sendUnshielded(color: Bytes<32>, amount: Uint<128>, recipient: Either<ContractAddress, UserAddress>): [] { }
// Receive a specified unshielded amount.
export circuit receiveUnshielded(color: Bytes<32>, amount: Uint<128>): [] { }
// Contract's unshielded balance for color.
export circuit unshieldedBalance(color: Bytes<32>): Uint<128> { return default<Uint<128>>; }
// Balance < amount.
export circuit unshieldedBalanceLt(color: Bytes<32>, amount: Uint<128>): Boolean { return default<Boolean>; }
// Balance >= amount.
export circuit unshieldedBalanceGte(color: Bytes<32>, amount: Uint<128>): Boolean { return default<Boolean>; }
// Balance > amount.
export circuit unshieldedBalanceGt(color: Bytes<32>, amount: Uint<128>): Boolean { return default<Boolean>; }
// Balance <= amount.
export circuit unshieldedBalanceLte(color: Bytes<32>, amount: Uint<128>): Boolean { return default<Boolean>; }

// Circuit-efficient (algebraic) hash.
export circuit transientHash<T>(value: T): Field { return default<Field>; }
// Circuit-efficient commitment hiding value under randomness rand.
export circuit transientCommit<T>(value: T, rand: Field): Field { return default<Field>; }
// SHA-256-based hash suitable for persistent state.
export circuit persistentHash<T>(value: T): Bytes<32> { return default<Bytes<32>>; }
// SHA-256-based commitment hiding value.
export circuit persistentCommit<T>(value: T, rand: Bytes<32>): Bytes<32> { return default<Bytes<32>>; }
// Convert a persistent hash to a field element.
export circuit degradeToTransient(x: Bytes<32>): Field { return default<Field>; }
// Convert a field element to persistent 32-byte form.
export circuit upgradeFromTransient(x: Field): Bytes<32> { return default<Bytes<32>>; }
// X coordinate of a curve point.
export circuit jubjubPointX(np: JubjubPoint): Field { return default<Field>; }
// Y coordinate of a curve point.
export circuit jubjubPointY(np: JubjubPoint): Field { return default<Field>; }
// Elliptic-curve point addition.
export circuit ecAdd(a: JubjubPoint, b: JubjubPoint): JubjubPoint { return default<JubjubPoint>; }
// Scalar multiplication of a point (scalar is Field at 0.31).
export circuit ecMul(a: JubjubPoint, b: Field): JubjubPoint { return default<JubjubPoint>; }
// Multiply the primary generator by a scalar.
export circuit ecMulGenerator(b: Field): JubjubPoint { return default<JubjubPoint>; }
// Hash an arbitrary value to a curve point.
export circuit hashToCurve<T>(value: T): JubjubPoint { return default<JubjubPoint>; }
// Build a point from coordinates.
export circuit constructJubjubPoint(x: Field, y: Field): JubjubPoint { return default<JubjubPoint>; }
// The caller's own Zswap coin public key (witness).
export circuit ownPublicKey(): ZswapCoinPublicKey { return default<ZswapCoinPublicKey>; }
// Register a Zswap input for the call (witness).
export circuit createZswapInput(coin: QualifiedShieldedCoinInfo): [] { }
// Register a Zswap output for the call (witness).
export circuit createZswapOutput(coin: ShieldedCoinInfo, recipient: Either<ZswapCoinPublicKey, ContractAddress>): [] { }
```

- [ ] **Step 2: Write the failing tests**

Create `crates/analyzer-core/src/stdlib.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_parses_with_zero_diagnostics() {
        let result = compactp_parser::parse(STDLIB_SOURCE);
        assert!(
            result.errors.is_empty(),
            "stdlib stub must parse cleanly: {:?}",
            result.errors.first()
        );
    }

    #[test]
    fn stub_declares_the_expected_surface() {
        let result = compactp_parser::parse(STDLIB_SOURCE);
        let root = compactp_syntax::SyntaxNode::new_root(result.green);
        let tree = crate::ItemTree::extract(&root);
        for name in ["Maybe", "Either", "some", "none", "persistentHash", "transientCommit", "kernel", "ecMul", "JubjubPoint"] {
            assert!(
                tree.symbols.iter().any(|s| s.name == name && s.exported),
                "missing stdlib symbol: {name}"
            );
        }
        // Post-0.31 items must NOT be present.
        for name in ["ecNeg", "keccak256"] {
            assert!(!tree.symbols.iter().any(|s| s.name == name), "unexpected: {name}");
        }
    }

    #[test]
    fn materialize_writes_once_and_refreshes_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = materialize(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), STDLIB_SOURCE);
        // Corrupt it; materialize must restore.
        std::fs::write(&path, "stale").unwrap();
        let path2 = materialize(dir.path()).unwrap();
        assert_eq!(path, path2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), STDLIB_SOURCE);
    }

    #[test]
    fn register_stdlib_makes_it_analyzable() {
        let dir = tempfile::tempdir().unwrap();
        let path = materialize(dir.path()).unwrap();
        let mut host = crate::AnalysisHost::new();
        assert_eq!(host.stdlib_file(), None);
        let file = host.register_stdlib(&path);
        assert_eq!(host.stdlib_file(), Some(file));
        let analysis = host.analyze(file).unwrap();
        assert!(analysis.diagnostics.is_empty());
        assert!(analysis.item_tree.symbols.iter().any(|s| s.name == "persistentHash"));
    }
}
```

Register in `crates/analyzer-core/src/lib.rs` (append):

```rust
pub mod stdlib;
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p analyzer-core`
Expected: FAIL — compile errors (`STDLIB_SOURCE`, `materialize`, `register_stdlib` not found)

- [ ] **Step 4: Write the implementation**

Prepend to `crates/analyzer-core/src/stdlib.rs`:

```rust
//! The bundled CompactStandardLibrary stub. `import CompactStandardLibrary;`
//! is compiler-internal (verified: never a file lookup), so the analyzer
//! mirrors it with this embedded, signature-accurate stub. It is
//! materialized to a real file on disk so goto-definition into the stdlib
//! lands somewhere an editor can open.

use std::path::{Path, PathBuf};

pub const STDLIB_SOURCE: &str = include_str!("../assets/std_0_23.compact");

/// Writes the stub as `CompactStandardLibrary.compact` under `dir` if it is
/// missing or differs from the bundled text. Returns the file path.
pub fn materialize(dir: &Path) -> std::io::Result<PathBuf> {
    let path = dir.join("CompactStandardLibrary.compact");
    let stale = match std::fs::read_to_string(&path) {
        Ok(existing) => existing != STDLIB_SOURCE,
        Err(_) => true,
    };
    if stale {
        std::fs::create_dir_all(dir)?;
        std::fs::write(&path, STDLIB_SOURCE)?;
    }
    Ok(path)
}
```

In `crates/analyzer-core/src/analysis.rs`, add a `stdlib: Option<FileId>` field to `AnalysisHost` (initialize `None` in `new()`) and these methods:

```rust
    /// Registers the materialized stdlib stub so the resolver can target it.
    pub fn register_stdlib(&mut self, path: &std::path::Path) -> FileId {
        let file = self.vfs.file_id(path);
        self.stdlib = Some(file);
        file
    }

    pub fn stdlib_file(&self) -> Option<FileId> {
        self.stdlib
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p analyzer-core`
Expected: PASS — 27 tests (23 + 4 new)

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: no warnings

```bash
git add crates/analyzer-core
git commit -m "feat(core): bundle CompactStandardLibrary stub with materialization"
```

---

### Task 3: analyzer-core — resolver part 1: classification + local scopes

**Files:**
- Create: `crates/analyzer-core/src/resolve.rs`
- Create: `crates/analyzer-core/src/fixture.rs`
- Modify: `crates/analyzer-core/src/lib.rs`

**Interfaces:**
- Consumes: `AnalysisHost`, `FileAnalysis`, `ItemTree` (Tasks 1-2), compactp AST/syntax
- Produces (Task 4 extends; Tasks 5-9 consume):
  - `analyzer_core::FilePosition { pub file: FileId, pub offset: TextSize }` (`Clone, Copy, Debug`)
  - `analyzer_core::Definition` — `Item { file: FileId, index: u32 }` | `Local { file: FileId, name: String, name_range: TextRange, detail: String }` (`Clone, Debug, PartialEq, Eq`)
  - `AnalysisHost::resolve(&mut self, pos: FilePosition) -> Option<Definition>`
  - `AnalysisHost::resolve_name_at(&mut self, pos: FilePosition, name: &str) -> Option<Definition>` (Task 4 fills the file-scope arm; in this task it resolves locals only)
  - `analyzer_core::fixture::extract(source: &str) -> (String, TextSize)` — strips the first `$0` marker, returns clean text + its offset (test support, `#[doc(hidden)]`)

- [ ] **Step 1: Write the fixture helper and the failing tests**

Create `crates/analyzer-core/src/fixture.rs`:

```rust
//! Test support: `$0` cursor markers in inline fixtures (rust-analyzer
//! convention). Not for production use.
#![doc(hidden)]

use text_size::TextSize;

/// Removes the first `$0` from `source`, returning the cleaned text and the
/// marker's byte offset. Panics if no marker is present.
pub fn extract(source: &str) -> (String, TextSize) {
    let idx = source.find("$0").expect("fixture must contain a $0 marker");
    let mut clean = String::with_capacity(source.len() - 2);
    clean.push_str(&source[..idx]);
    clean.push_str(&source[idx + 2..]);
    (clean, TextSize::new(idx as u32))
}
```

Create `crates/analyzer-core/src/resolve.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    /// Parses a `$0`-marked fixture into a host and returns the position.
    fn position_of(source: &str) -> (crate::AnalysisHost, FilePosition) {
        let (clean, offset) = fixture::extract(source);
        let mut host = crate::AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/test/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        (host, FilePosition { file, offset })
    }

    fn resolve_local(source: &str) -> Option<(String, String)> {
        let (mut host, pos) = position_of(source);
        match host.resolve(pos)? {
            Definition::Local { name, detail, .. } => Some((name, detail)),
            Definition::Item { .. } => panic!("expected a local"),
        }
    }

    #[test]
    fn resolves_circuit_param() {
        let (name, detail) =
            resolve_local("circuit f(base: Field): Field { return ba$0se; }").unwrap();
        assert_eq!(name, "base");
        assert_eq!(detail, "base: Field");
    }

    #[test]
    fn resolves_const_with_shadowing() {
        // The second `const x` shadows the first; the reference after it
        // must resolve to the SECOND binding.
        let source = "circuit f(): Field {\n  const x = 1;\n  const x = 2;\n  return x$0;\n}";
        let (mut host, pos) = position_of(source);
        let Some(Definition::Local { name_range, .. }) = host.resolve(pos) else {
            panic!("expected local");
        };
        let (clean, _) = fixture::extract(source);
        // The second binding's `x` is after the first one.
        let second_x = clean.rfind("const x").unwrap() + "const ".len();
        assert_eq!(u32::from(name_range.start()) as usize, second_x);
    }

    #[test]
    fn const_is_not_visible_before_its_declaration() {
        assert!(resolve_local("circuit f(): Field {\n  const a = b$0;\n  const b = 1;\n  return 0;\n}").is_none());
    }

    #[test]
    fn resolves_for_loop_binding_only_inside_body() {
        let (name, detail) =
            resolve_local("circuit f(): [] {\n  for (const i of 0..10) {\n    const y = i$0;\n  }\n}").unwrap();
        assert_eq!(name, "i");
        assert_eq!(detail, "for i");
        // Not visible after the loop.
        assert!(resolve_local(
            "circuit f(): Field {\n  for (const i of 0..10) { }\n  return i$0;\n}"
        )
        .is_none());
    }

    #[test]
    fn resolves_tuple_pattern_binding() {
        let (name, _) = resolve_local(
            "circuit f(pair: [Field, Field]): Field {\n  const [a, b] = pair;\n  return b$0;\n}",
        )
        .unwrap();
        assert_eq!(name, "b");
    }

    #[test]
    fn resolves_generic_type_param_in_type_position() {
        let (name, detail) =
            resolve_local("circuit id<T>(value: T): T$0 { return value; }").unwrap();
        assert_eq!(name, "T");
        assert_eq!(detail, "generic T");
    }

    #[test]
    fn resolves_nat_generic_in_type_size_position() {
        let (name, detail) =
            resolve_local("struct Box<#n> { v: Vector<n$0, Field>; }").unwrap();
        assert_eq!(name, "n");
        assert_eq!(detail, "generic #n");
    }

    #[test]
    fn resolves_lambda_param() {
        let (name, _) = resolve_local(
            "circuit f(v: Vector<2, Field>): Vector<2, Field> {\n  return map((x) => x$0 + 1, v);\n}",
        )
        .unwrap();
        assert_eq!(name, "x");
    }

    #[test]
    fn definition_site_resolves_to_itself() {
        let source = "circuit f(): Field {\n  const lo$0ng_name = 1;\n  return long_name;\n}";
        let (mut host, pos) = position_of(source);
        let Some(Definition::Local { name, .. }) = host.resolve(pos) else {
            panic!("expected local");
        };
        assert_eq!(name, "long_name");
    }

    #[test]
    fn unknown_name_resolves_to_none_gracefully() {
        assert!(resolve_local("circuit f(): Field { return mystery$0; }").is_none());
    }
}
```

Register in `crates/analyzer-core/src/lib.rs` (append):

```rust
pub mod fixture;
mod resolve;

pub use resolve::{Definition, FilePosition};
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core`
Expected: FAIL — compile errors (`FilePosition`, `Definition`, `resolve` not found)

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analyzer-core/src/resolve.rs`:

```rust
//! Syntactic name resolution. No type inference: a token is classified by
//! its parent CST kind (there is no reference node in the Compact CST —
//! verified against compactp), then resolved through lexical scopes
//! (this task) and file scope / imports (Task 4).

use compactp_ast::{AstNode, Pat};
use compactp_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use rowan::TokenAtOffset;
use text_size::{TextRange, TextSize};

use crate::vfs::FileId;

#[derive(Clone, Copy, Debug)]
pub struct FilePosition {
    pub file: FileId,
    pub offset: TextSize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Definition {
    /// A declaration in some file's ItemTree.
    Item { file: FileId, index: u32 },
    /// A local binding (param, const, loop var, lambda param, generic).
    Local {
        file: FileId,
        name: String,
        name_range: TextRange,
        detail: String,
    },
}

impl crate::AnalysisHost {
    /// Resolves the identifier at `pos` to its definition.
    pub fn resolve(&mut self, pos: FilePosition) -> Option<Definition> {
        let analysis = self.analyze(pos.file)?;
        let root = SyntaxNode::new_root(analysis.green.clone());
        let token = ident_at_offset(&root, pos.offset)?;
        let name = token.text().to_string();
        let parent = token.parent()?;

        match parent.kind() {
            // Definition sites resolve to themselves.
            SyntaxKind::IDENT_PAT => {
                return local_from_pattern_site(pos.file, &token);
            }
            SyntaxKind::STRUCT_PAT_FIELD => {
                // Shorthand `{a}` binds `a`; `name: pat` labels a field.
                return local_from_pattern_site(pos.file, &token);
            }
            SyntaxKind::FOR_STMT => {
                return Some(Definition::Local {
                    file: pos.file,
                    name: name.clone(),
                    name_range: token.text_range(),
                    detail: format!("for {name}"),
                });
            }
            SyntaxKind::GENERIC_PARAM => {
                return Some(generic_definition(pos.file, &parent, &token));
            }
            _ => {}
        }

        // Names of item declarations resolve to the item itself.
        if let Some(def) = self.item_declaration_at(pos.file, &parent, &token) {
            return Some(def);
        }

        match parent.kind() {
            SyntaxKind::NAME_EXPR | SyntaxKind::CALL_EXPR | SyntaxKind::TYPE_REF
            | SyntaxKind::STRUCT_EXPR | SyntaxKind::TYPE_SIZE => {
                self.resolve_name_at(pos, &name)
            }
            SyntaxKind::STRUCT_FIELD_INIT => self.resolve_struct_literal_field(pos, &parent, &name),
            SyntaxKind::MEMBER_EXPR => self.resolve_member(pos, &parent, &name),
            SyntaxKind::IMPORT_SPECIFIER => self.resolve_import_specifier(pos, &parent, &name),
            _ => None,
        }
    }

    /// Resolves `name` as if referenced at `pos`: locals first (nearest
    /// scope wins), then file scope + imports (Task 4).
    pub fn resolve_name_at(&mut self, pos: FilePosition, name: &str) -> Option<Definition> {
        let analysis = self.analyze(pos.file)?;
        let root = SyntaxNode::new_root(analysis.green.clone());
        if let Some(local) = resolve_local_name(pos.file, &root, pos.offset, name) {
            return Some(local);
        }
        self.resolve_in_file_scope(pos, name)
    }

    /// Task 4 replaces this stub with file-scope + import resolution.
    fn resolve_in_file_scope(&mut self, _pos: FilePosition, _name: &str) -> Option<Definition> {
        None
    }

    /// Task 4 implements these; stubs keep this task compiling.
    fn resolve_struct_literal_field(
        &mut self,
        _pos: FilePosition,
        _parent: &SyntaxNode,
        _name: &str,
    ) -> Option<Definition> {
        None
    }

    fn resolve_member(
        &mut self,
        _pos: FilePosition,
        _parent: &SyntaxNode,
        _name: &str,
    ) -> Option<Definition> {
        None
    }

    fn resolve_import_specifier(
        &mut self,
        _pos: FilePosition,
        _parent: &SyntaxNode,
        _name: &str,
    ) -> Option<Definition> {
        None
    }

    /// If `token` is the name of an item declaration, return that item.
    fn item_declaration_at(
        &mut self,
        file: FileId,
        parent: &SyntaxNode,
        token: &SyntaxToken,
    ) -> Option<Definition> {
        let analysis = self.analyze(file)?;
        let range = token.text_range();
        let is_decl_kind = matches!(
            parent.kind(),
            SyntaxKind::CIRCUIT_DEF
                | SyntaxKind::CIRCUIT_DECL
                | SyntaxKind::WITNESS_DECL
                | SyntaxKind::LEDGER_DECL
                | SyntaxKind::STRUCT_DEF
                | SyntaxKind::STRUCT_FIELD
                | SyntaxKind::ENUM_DEF
                | SyntaxKind::ENUM_VARIANT
                | SyntaxKind::MODULE_DEF
                | SyntaxKind::TYPE_DECL
                | SyntaxKind::CONTRACT_DECL
                | SyntaxKind::CONTRACT_CIRCUIT
        );
        if !is_decl_kind {
            return None;
        }
        let index = analysis
            .item_tree
            .symbols
            .iter()
            .position(|s| s.name_range == range)?;
        Some(Definition::Item { file, index: index as u32 })
    }
}

/// Finds the IDENT token at/beside `offset`. Between two tokens, prefers
/// the identifier.
fn ident_at_offset(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    let pick = |t: SyntaxToken| (t.kind() == SyntaxKind::IDENT).then_some(t);
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => pick(t),
        TokenAtOffset::Between(l, r) => pick(r).or_else(|| pick(l)),
    }
}

fn generic_definition(file: FileId, param_node: &SyntaxNode, token: &SyntaxToken) -> Definition {
    let numeric = param_node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == SyntaxKind::HASH);
    Definition::Local {
        file,
        name: token.text().to_string(),
        name_range: token.text_range(),
        detail: if numeric {
            format!("generic #{}", token.text())
        } else {
            format!("generic {}", token.text())
        },
    }
}

fn local_from_pattern_site(file: FileId, token: &SyntaxToken) -> Option<Definition> {
    Some(Definition::Local {
        file,
        name: token.text().to_string(),
        name_range: token.text_range(),
        detail: detail_for_binding(token),
    })
}

/// Walks outward from `offset` collecting the nearest binding of `name`.
fn resolve_local_name(
    file: FileId,
    root: &SyntaxNode,
    offset: TextSize,
    name: &str,
) -> Option<Definition> {
    let token = root
        .token_at_offset(offset)
        .right_biased()
        .or_else(|| root.token_at_offset(offset).left_biased())?;
    let start = token.parent()?;
    for node in start.ancestors() {
        match node.kind() {
            SyntaxKind::BLOCK => {
                if let Some(block) = compactp_ast::Block::cast(node.clone()) {
                    // Later consts shadow earlier ones: keep the LAST
                    // binding that ends before the reference.
                    let mut found: Option<SyntaxToken> = None;
                    for stmt in block.stmts() {
                        if let compactp_ast::Stmt::Const(c) = stmt {
                            if c.syntax().text_range().end() > offset {
                                break;
                            }
                            if let Some(pat) = c.pattern() {
                                for binding in pattern_bindings(&pat) {
                                    if binding.text() == name {
                                        found = Some(binding);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(binding) = found {
                        return Some(Definition::Local {
                            file,
                            name: name.to_string(),
                            name_range: binding.text_range(),
                            detail: detail_for_binding(&binding),
                        });
                    }
                }
            }
            SyntaxKind::FOR_STMT => {
                if let Some(for_stmt) = compactp_ast::ForStmt::cast(node.clone()) {
                    let in_body = for_stmt
                        .body()
                        .is_some_and(|b| b.syntax().text_range().contains(offset));
                    if in_body {
                        if let Some(var) = for_stmt.var_name() {
                            if var.text() == name {
                                return Some(Definition::Local {
                                    file,
                                    name: name.to_string(),
                                    name_range: var.text_range(),
                                    detail: format!("for {name}"),
                                });
                            }
                        }
                    }
                }
            }
            SyntaxKind::LAMBDA_EXPR => {
                if let Some(lambda) = compactp_ast::expr::LambdaExpr::cast(node.clone()) {
                    if let Some(params) = lambda.param_list() {
                        // In-body means past the parameter list (covers
                        // both block- and expression-bodied lambdas).
                        if offset >= params.syntax().text_range().end() {
                            for param in params.params() {
                                if let Some(def) = param_binding(file, &param, name) {
                                    return Some(def);
                                }
                            }
                        }
                    }
                }
            }
            SyntaxKind::CIRCUIT_DEF => {
                if let Some(circuit) = compactp_ast::CircuitDef::cast(node.clone()) {
                    for param in circuit.params() {
                        if let Some(def) = param_binding(file, &param, name) {
                            return Some(def);
                        }
                    }
                    if let Some(def) = generic_binding(file, circuit.generic_params(), name) {
                        return Some(def);
                    }
                }
            }
            SyntaxKind::CONSTRUCTOR_DEF => {
                if let Some(ctor) = compactp_ast::ConstructorDef::cast(node.clone()) {
                    for param in ctor.params() {
                        if let Some(def) = param_binding(file, &param, name) {
                            return Some(def);
                        }
                    }
                }
            }
            SyntaxKind::STRUCT_DEF => {
                if let Some(s) = compactp_ast::StructDef::cast(node.clone()) {
                    if let Some(def) = generic_binding(file, s.generic_params(), name) {
                        return Some(def);
                    }
                }
            }
            SyntaxKind::TYPE_DECL => {
                if let Some(t) = compactp_ast::TypeDecl::cast(node.clone()) {
                    if let Some(def) = generic_binding(file, t.generic_params(), name) {
                        return Some(def);
                    }
                }
            }
            SyntaxKind::MODULE_DEF => {
                if let Some(m) = compactp_ast::ModuleDef::cast(node.clone()) {
                    if let Some(def) = generic_binding(file, m.generic_params(), name) {
                        return Some(def);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn param_binding(file: FileId, param: &compactp_ast::Param, name: &str) -> Option<Definition> {
    let pat = param.pattern()?;
    for binding in pattern_bindings(&pat) {
        if binding.text() == name {
            let ty = param
                .ty()
                .map(|t| t.syntax().text().to_string())
                .unwrap_or_else(|| "_".to_string());
            return Some(Definition::Local {
                file,
                name: name.to_string(),
                name_range: binding.text_range(),
                detail: format!("{name}: {ty}"),
            });
        }
    }
    None
}

fn generic_binding(
    file: FileId,
    params: Option<compactp_ast::GenericParamList>,
    name: &str,
) -> Option<Definition> {
    for param in params?.params() {
        let token = param.name()?;
        if token.text() == name {
            return Some(generic_definition(file, param.syntax(), &token));
        }
    }
    None
}

/// All name tokens bound by a pattern (verified traversal per compactp AST).
fn pattern_bindings(pat: &Pat) -> Vec<SyntaxToken> {
    let mut out = Vec::new();
    collect_pattern_bindings(pat, &mut out);
    out
}

fn collect_pattern_bindings(pat: &Pat, out: &mut Vec<SyntaxToken>) {
    match pat {
        Pat::Ident(p) => {
            if let Some(name) = p.name() {
                out.push(name);
            }
        }
        Pat::Tuple(t) => {
            for elt in t.elements() {
                if let Some(inner) = elt.pattern() {
                    collect_pattern_bindings(&inner, out);
                }
            }
        }
        Pat::Struct(s) => {
            for field in s.fields() {
                match field.pattern() {
                    Some(inner) => collect_pattern_bindings(&inner, out),
                    None => {
                        if let Some(name) = field.name() {
                            out.push(name);
                        }
                    }
                }
            }
        }
    }
}

/// "const x" / "x: Field" detail for a binding token, derived from context.
fn detail_for_binding(token: &SyntaxToken) -> String {
    let name = token.text();
    for node in token.parent().into_iter().flat_map(|p| p.ancestors()) {
        match node.kind() {
            SyntaxKind::PARAM => {
                if let Some(param) = compactp_ast::Param::cast(node.clone()) {
                    let ty = param
                        .ty()
                        .map(|t| t.syntax().text().to_string())
                        .unwrap_or_else(|| "_".to_string());
                    return format!("{name}: {ty}");
                }
            }
            SyntaxKind::CONST_STMT => return format!("const {name}"),
            SyntaxKind::FOR_STMT => return format!("for {name}"),
            _ => {}
        }
    }
    format!("const {name}")
}
```

NOTES for the implementer:
- Expected `SyntaxKind` variant names used above: `IDENT_PAT, STRUCT_PAT_FIELD, FOR_STMT, GENERIC_PARAM, NAME_EXPR, CALL_EXPR, TYPE_REF, STRUCT_EXPR, TYPE_SIZE, STRUCT_FIELD_INIT, MEMBER_EXPR, IMPORT_SPECIFIER, BLOCK, CIRCUIT_DEF, CIRCUIT_DECL, WITNESS_DECL, LEDGER_DECL, STRUCT_DEF, STRUCT_FIELD, ENUM_DEF, ENUM_VARIANT, MODULE_DEF, TYPE_DECL, CONTRACT_DECL, CONTRACT_CIRCUIT, CONSTRUCTOR_DEF, LAMBDA_EXPR, PARAM, CONST_STMT, HASH`. If any differ in compactp's actual enum, use the actual names and note the substitution in your report.
- `TokenAtOffset` has `right_biased()`/`left_biased()` helpers in rowan 0.16.
- The dead-code stubs (`resolve_struct_literal_field` etc.) are replaced in Task 4 — if clippy flags them as unused, they are called from `resolve`, so they are not dead; if an unused-parameter warning appears, the leading underscores already suppress it.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core`
Expected: PASS — 37 tests (27 + 10 new)

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: no warnings

```bash
git add crates/analyzer-core
git commit -m "feat(core): resolver with token classification and lexical scopes"
```

---

### Task 4: analyzer-core — resolver part 2: file scope + imports + stdlib

**Files:**
- Modify: `crates/analyzer-core/src/resolve.rs`

**Interfaces:**
- Consumes: Task 3's resolver skeleton, Task 2's stdlib registration, ItemTree
- Produces: the four Task-3 stubs become real:
  - `resolve_in_file_scope` — enclosing-module members → top-level items → imports (stdlib / in-file modules, honoring selective lists with aliases and prefixes)
  - `resolve_struct_literal_field` — `Point { x$0: … }` → `Point`'s field `x`
  - `resolve_member` — `Color.red$0` → enum variant (only when the receiver resolves to an enum)
  - `resolve_import_specifier` — `import { some$0 } from CompactStandardLibrary;` → the imported item
  - plus `AnalysisHost::def_name(&mut self, def: &Definition) -> Option<String>` and `AnalysisHost::nav_info(&mut self, def: &Definition) -> Option<(FileId, TextRange, TextRange)>` (name-range + full-range for navigation)

- [ ] **Step 1: Write the failing tests**

Append to the tests module in `crates/analyzer-core/src/resolve.rs`:

```rust
    // ---- Task 4: file scope + imports ----

    /// Fixture: verified to parse with zero errors (see plan Global
    /// Constraints). Registers the stdlib stub in a tempdir.
    fn full_host(source: &str) -> (crate::AnalysisHost, FilePosition, tempfile::TempDir) {
        let (clean, offset) = fixture::extract(source);
        let dir = tempfile::tempdir().unwrap();
        let std_path = crate::stdlib::materialize(dir.path()).unwrap();
        let mut host = crate::AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host.vfs_mut().file_id(std::path::Path::new("/test/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        (host, FilePosition { file, offset }, dir)
    }

    fn resolve_item(source: &str) -> Option<(crate::FileId, String, crate::SymbolKind)> {
        let (mut host, pos, _dir) = full_host(source);
        match host.resolve(pos)? {
            Definition::Item { file, index } => {
                let sym = host.analyze(file).unwrap().item_tree.symbols[index as usize].clone();
                Some((file, sym.name, sym.kind))
            }
            Definition::Local { .. } => panic!("expected an item"),
        }
    }

    #[test]
    fn resolves_top_level_circuit_call() {
        let (_, name, kind) = resolve_item(
            "circuit helper(): Field { return 1; }\ncircuit main(): Field { return hel$0per(); }",
        )
        .unwrap();
        assert_eq!(name, "helper");
        assert_eq!(kind, crate::SymbolKind::Circuit);
    }

    #[test]
    fn resolves_type_reference_to_struct() {
        let (_, name, kind) = resolve_item(
            "struct Point { x: Field; }\ncircuit f(p: Po$0int): Field { return 0; }",
        )
        .unwrap();
        assert_eq!(name, "Point");
        assert_eq!(kind, crate::SymbolKind::Struct);
    }

    #[test]
    fn locals_shadow_top_level_items() {
        // `helper` is both a top-level circuit and a local const: the local wins.
        let source = "circuit helper(): Field { return 1; }\n\
                      circuit main(): Field {\n  const helper = 2;\n  return help$0er;\n}";
        let (mut host, pos, _dir) = full_host(source);
        assert!(matches!(host.resolve(pos), Some(Definition::Local { .. })));
    }

    #[test]
    fn resolves_imported_module_member() {
        let (_, name, _) = resolve_item(
            "module Inner {\n  export circuit double(x: Field): Field { return x + x; }\n}\n\
             import Inner;\n\
             circuit main(): Field { return dou$0ble(1); }",
        )
        .unwrap();
        assert_eq!(name, "double");
    }

    #[test]
    fn unimported_module_members_do_not_resolve() {
        let (mut host, pos, _dir) = full_host(
            "module Inner {\n  export circuit double(x: Field): Field { return x + x; }\n}\n\
             circuit main(): Field { return dou$0ble(1); }",
        );
        assert_eq!(host.resolve(pos), None);
    }

    #[test]
    fn selective_import_with_alias() {
        let (_, name, _) = resolve_item(
            "module Inner {\n  export circuit double(x: Field): Field { return x + x; }\n}\n\
             import { double as twice } from Inner;\n\
             circuit main(): Field { return twi$0ce(1); }",
        )
        .unwrap();
        assert_eq!(name, "double");
        // The original name is NOT visible under a selective import list.
        let (mut host, pos, _dir) = full_host(
            "module Inner {\n  export circuit double(x: Field): Field { return x + x; }\n  export circuit triple(x: Field): Field { return x + x + x; }\n}\n\
             import { double } from Inner;\n\
             circuit main(): Field { return tri$0ple(1); }",
        );
        assert_eq!(host.resolve(pos), None);
    }

    #[test]
    fn prefix_import_strips_prefix() {
        let (_, name, _) = resolve_item(
            "module Inner {\n  export circuit double(x: Field): Field { return x + x; }\n}\n\
             import Inner prefix In_;\n\
             circuit main(): Field { return In_dou$0ble(1); }",
        )
        .unwrap();
        assert_eq!(name, "double");
        // Unprefixed name must NOT resolve from a prefixed import.
        let (mut host, pos, _dir) = full_host(
            "module Inner {\n  export circuit double(x: Field): Field { return x + x; }\n}\n\
             import Inner prefix In_;\n\
             circuit main(): Field { return dou$0ble(1); }",
        );
        assert_eq!(host.resolve(pos), None);
    }

    #[test]
    fn resolves_stdlib_call_into_stub_file() {
        let (mut host, pos, _dir) = full_host(
            "import CompactStandardLibrary;\n\
             circuit main(x: Field): Bytes<32> { return persistent$0Hash<Field>(x); }",
        );
        let Some(Definition::Item { file, index }) = host.resolve(pos) else {
            panic!("expected stdlib item");
        };
        assert_eq!(Some(file), host.stdlib_file());
        let sym = &host.analyze(file).unwrap().item_tree.symbols[index as usize];
        assert_eq!(sym.name, "persistentHash");
    }

    #[test]
    fn stdlib_not_visible_without_import() {
        let (mut host, pos, _dir) =
            full_host("circuit main(x: Field): Bytes<32> { return persistent$0Hash<Field>(x); }");
        assert_eq!(host.resolve(pos), None);
    }

    #[test]
    fn resolves_struct_literal_field_label() {
        let (_, name, kind) = resolve_item(
            "struct Point { x: Field; y: Field; }\n\
             circuit f(): Field {\n  const p = Point { x$0: 1, y: 2 };\n  return 0;\n}",
        )
        .unwrap();
        assert_eq!(name, "x");
        assert_eq!(kind, crate::SymbolKind::StructField);
    }

    #[test]
    fn resolves_enum_variant_member() {
        let (_, name, kind) = resolve_item(
            "enum Color { red, blue }\n\
             circuit f(): [] {\n  const c = Color.re$0d;\n}",
        )
        .unwrap();
        assert_eq!(name, "red");
        assert_eq!(kind, crate::SymbolKind::EnumVariant);
    }

    #[test]
    fn ledger_adt_methods_resolve_to_none() {
        let (mut host, pos, _dir) = full_host(
            "export ledger count: Counter;\n\
             circuit f(): [] {\n  count.incre$0ment(1);\n}",
        );
        assert_eq!(host.resolve(pos), None);
    }

    #[test]
    fn string_path_imports_resolve_to_none_in_m2a() {
        let (mut host, pos, _dir) = full_host(
            "import \"./utils\" prefix U_;\n\
             circuit main(): Field { return U_help$0er(); }",
        );
        assert_eq!(host.resolve(pos), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core`
Expected: FAIL — the new tests fail (stubs return `None`; `resolve_item` panics on `None`). Compile errors for `def_name`/`nav_info` are not expected yet (they're only added in Step 3 and consumed by Task 5).

- [ ] **Step 3: Replace the Task-3 stubs with the real implementation**

In `crates/analyzer-core/src/resolve.rs`, replace the four stub methods inside `impl crate::AnalysisHost` and add the two nav helpers:

```rust
    /// File scope: enclosing-module members, then top-level items, then
    /// imports in declaration order (stdlib and in-file modules only —
    /// filesystem imports are M2b).
    fn resolve_in_file_scope(&mut self, pos: FilePosition, name: &str) -> Option<Definition> {
        let analysis = self.analyze(pos.file)?;
        let tree = analysis.item_tree.clone();
        let root = SyntaxNode::new_root(analysis.green.clone());

        // Inside a module, siblings are visible first.
        if let Some(module_idx) = enclosing_module(&tree, pos.offset) {
            if let Some((idx, _)) = tree
                .children_of(module_idx)
                .find(|(_, s)| s.name == name)
            {
                return Some(Definition::Item { file: pos.file, index: idx });
            }
        }

        // Top-level items.
        if let Some((idx, _)) = tree.top_level().find(|(_, s)| s.name == name) {
            return Some(Definition::Item { file: pos.file, index: idx });
        }

        // Imports, in order.
        let Some(source_file) = compactp_ast::SourceFile::cast(root) else {
            return None;
        };
        for import in source_file.imports() {
            if let Some(def) = self.resolve_through_import(pos.file, &tree, &import, name) {
                return Some(def);
            }
        }
        None
    }

    /// One import's contribution to the namespace, honoring prefix and
    /// selective specifier lists (with aliases).
    fn resolve_through_import(
        &mut self,
        file: FileId,
        tree: &crate::ItemTree,
        import: &compactp_ast::Import,
        name: &str,
    ) -> Option<Definition> {
        // Determine the effective name to look up in the import's exports.
        let mut lookup: &str = name;
        let stripped;
        if let Some(prefix) = import.prefix().and_then(|p| p.name()) {
            // Prefix applies to every name from this import: unprefixed
            // names never resolve through it.
            stripped = name.strip_prefix(prefix.text())?.to_string();
            lookup = &stripped;
        }

        // Selective list: only listed names are visible; `as` aliases
        // rename them. (No typed accessor: walk descendants — verified
        // compactp pattern.)
        let specifiers: Vec<(String, String)> = import
            .syntax()
            .descendants()
            .filter_map(compactp_ast::ImportSpecifier::cast)
            .filter_map(|spec| {
                let original = spec.name()?.text().to_string();
                let alias = import_specifier_alias(&spec).unwrap_or_else(|| original.clone());
                Some((alias, original))
            })
            .collect();
        let target_name: String = if specifiers.is_empty() {
            lookup.to_string()
        } else {
            specifiers
                .iter()
                .find(|(alias, _)| alias == lookup)?
                .1
                .clone()
        };

        // Where do this import's exports live?
        match import.name() {
            Some(module_token) if module_token.text() == "CompactStandardLibrary" => {
                let std_file = self.stdlib_file()?;
                let std_tree = self.analyze(std_file)?.item_tree.clone();
                let (idx, _) = std_tree
                    .top_level()
                    .find(|(_, s)| s.exported && s.name == target_name)?;
                Some(Definition::Item { file: std_file, index: idx })
            }
            Some(module_token) => {
                // In-file module import (filesystem fallback is M2b).
                let (module_idx, _) = tree
                    .top_level()
                    .find(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == module_token.text())?;
                let (idx, _) = tree
                    .children_of(module_idx)
                    .find(|(_, s)| s.exported && s.name == target_name)?;
                Some(Definition::Item { file, index: idx })
            }
            // String-path import: M2b.
            None => None,
        }
    }

    fn resolve_struct_literal_field(
        &mut self,
        pos: FilePosition,
        field_init: &SyntaxNode,
        name: &str,
    ) -> Option<Definition> {
        let struct_expr = field_init
            .ancestors()
            .find(|n| n.kind() == SyntaxKind::STRUCT_EXPR)
            .and_then(compactp_ast::expr::StructExpr::cast)?;
        let struct_name = struct_expr.name()?.text().to_string();
        let struct_def = self.resolve_name_at(
            FilePosition { file: pos.file, offset: struct_expr.syntax().text_range().start() },
            &struct_name,
        )?;
        self.field_of(&struct_def, crate::SymbolKind::StructField, name)
    }

    fn resolve_member(
        &mut self,
        pos: FilePosition,
        member: &SyntaxNode,
        name: &str,
    ) -> Option<Definition> {
        // Only the syntactically decidable case: receiver is a NAME_EXPR
        // resolving to an enum → the member is a variant. Everything else
        // (ledger ADT methods, struct field access) needs types → None.
        let receiver = member
            .children()
            .find(|n| n.kind() == SyntaxKind::NAME_EXPR)
            .and_then(compactp_ast::expr::NameExpr::cast)?;
        let receiver_name = receiver.ident()?.text().to_string();
        let receiver_def = self.resolve_name_at(
            FilePosition { file: pos.file, offset: receiver.syntax().text_range().start() },
            &receiver_name,
        )?;
        let Definition::Item { file, index } = &receiver_def else { return None };
        let tree = self.analyze(*file)?.item_tree.clone();
        if tree.symbols[*index as usize].kind != crate::SymbolKind::Enum {
            return None;
        }
        self.field_of(&receiver_def, crate::SymbolKind::EnumVariant, name)
    }

    fn resolve_import_specifier(
        &mut self,
        pos: FilePosition,
        specifier: &SyntaxNode,
        name: &str,
    ) -> Option<Definition> {
        // Only the ORIGINAL name in `import { orig as alias }` is a
        // reference into the imported module; the alias is a binding.
        let spec = compactp_ast::ImportSpecifier::cast(specifier.clone())?;
        let original = spec.name()?;
        if original.text() != name {
            return None;
        }
        let import = specifier
            .ancestors()
            .find(|n| n.kind() == SyntaxKind::IMPORT)
            .and_then(compactp_ast::Import::cast)?;
        let tree = self.analyze(pos.file)?.item_tree.clone();
        // Resolve the original name through this import, bypassing the
        // specifier's own alias filtering by looking up directly.
        match import.name() {
            Some(t) if t.text() == "CompactStandardLibrary" => {
                let std_file = self.stdlib_file()?;
                let std_tree = self.analyze(std_file)?.item_tree.clone();
                let (idx, _) = std_tree.top_level().find(|(_, s)| s.exported && s.name == name)?;
                Some(Definition::Item { file: std_file, index: idx })
            }
            Some(t) => {
                let (module_idx, _) = tree
                    .top_level()
                    .find(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == t.text())?;
                let (idx, _) = tree.children_of(module_idx).find(|(_, s)| s.exported && s.name == name)?;
                Some(Definition::Item { file: pos.file, index: idx })
            }
            None => None,
        }
    }

    /// Child symbol of `parent_def` with the given kind and name.
    fn field_of(
        &mut self,
        parent_def: &Definition,
        kind: crate::SymbolKind,
        name: &str,
    ) -> Option<Definition> {
        let Definition::Item { file, index } = parent_def else { return None };
        let tree = self.analyze(*file)?.item_tree.clone();
        let (idx, _) = tree
            .children_of(*index)
            .find(|(_, s)| s.kind == kind && s.name == name)?;
        Some(Definition::Item { file: *file, index: idx })
    }

    /// The definition's name (for reference search and rename).
    pub fn def_name(&mut self, def: &Definition) -> Option<String> {
        match def {
            Definition::Item { file, index } => Some(
                self.analyze(*file)?.item_tree.symbols.get(*index as usize)?.name.clone(),
            ),
            Definition::Local { name, .. } => Some(name.clone()),
        }
    }

    /// (file, name_range, full_range) for navigation.
    pub fn nav_info(&mut self, def: &Definition) -> Option<(FileId, TextRange, TextRange)> {
        match def {
            Definition::Item { file, index } => {
                let sym = self.analyze(*file)?.item_tree.symbols.get(*index as usize)?.clone();
                Some((*file, sym.name_range, sym.full_range))
            }
            Definition::Local { file, name_range, .. } => Some((*file, *name_range, *name_range)),
        }
    }
```

And add these free functions at module level:

```rust
/// Index of the module symbol whose range contains `offset`, if any.
fn enclosing_module(tree: &crate::ItemTree, offset: TextSize) -> Option<u32> {
    tree.symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == crate::SymbolKind::Module && s.full_range.contains(offset))
        .map(|(i, _)| i as u32)
        .last() // innermost (modules nest in document order)
}

/// The `as` alias of an import specifier (no typed accessor: read the IDENT
/// following AS_KW — verified compactp pattern).
fn import_specifier_alias(spec: &compactp_ast::ImportSpecifier) -> Option<String> {
    let mut saw_as = false;
    for element in spec.syntax().children_with_tokens() {
        if let Some(token) = element.into_token() {
            if token.kind() == SyntaxKind::AS_KW {
                saw_as = true;
            } else if saw_as && token.kind() == SyntaxKind::IDENT {
                return Some(token.text().to_string());
            }
        }
    }
    None
}
```

NOTE: expected kind names `IMPORT` and `AS_KW` — verify against compactp's enum as before.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core`
Expected: PASS — 50 tests (37 + 13 new)

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: no warnings

```bash
git add crates/analyzer-core
git commit -m "feat(core): file-scope and import resolution with stdlib support"
```

---

### Task 5: analyzer-ide crate + goto-definition end to end

**Files:**
- Create: `crates/analyzer-ide/Cargo.toml`
- Create: `crates/analyzer-ide/src/lib.rs`
- Create: `crates/analyzer-ide/src/goto_definition.rs`
- Create: `crates/compact-analyzer/tests/support/mod.rs`
- Modify: `Cargo.toml` (workspace members + `analyzer-ide` dependency alias)
- Modify: `crates/compact-analyzer/Cargo.toml` (add `analyzer-ide.workspace = true`)
- Modify: `crates/compact-analyzer/src/server.rs` (stdlib startup, request routing, capabilities)
- Modify: `crates/compact-analyzer/src/lsp_utils.rs` (position→offset, NavTarget→Location)
- Modify: `crates/compact-analyzer/tests/lsp_integration.rs` (use the shared harness)
- Create: `crates/compact-analyzer/tests/lsp_navigation.rs`

**Interfaces:**
- Consumes: `AnalysisHost::{resolve, nav_info}` (Task 4), `LineIndex::offset` (M1 — its first production caller)
- Produces:
  - `analyzer_ide::NavTarget { pub file: FileId, pub name_range: TextRange, pub full_range: TextRange }` (`Clone, Debug, PartialEq, Eq`)
  - `analyzer_ide::goto_definition(host: &mut AnalysisHost, pos: FilePosition) -> Option<NavTarget>`
  - Server: `definitionProvider` capability; `textDocument/definition` handler
  - `lsp_utils::offset_from_position(li: &LineIndex, pos: lsp_types::Position) -> Option<TextSize>`
  - `lsp_utils::nav_target_to_location(host: &mut AnalysisHost, target: &analyzer_ide::NavTarget) -> Option<lsp_types::Location>`
  - Shared black-box test harness at `tests/support/mod.rs` (the M1 `Client` + `read_message` + `temp_doc` + `did_open`, made `pub`, plus `pub fn lsp_position(text: &str, needle: &str) -> (u32, u32)` returning the 0-based line/UTF-16 col of the first occurrence of `needle`)

- [ ] **Step 1: Scaffold the crate and refactor the test harness**

`crates/analyzer-ide/Cargo.toml`:

```toml
[package]
name = "analyzer-ide"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
publish = false

[dependencies]
analyzer-core.workspace = true
compactp_syntax.workspace = true
rowan.workspace = true
text-size.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

Workspace root `Cargo.toml`: add `"crates/analyzer-ide"` to `members` and `analyzer-ide = { path = "crates/analyzer-ide" }` to `[workspace.dependencies]`. Add `analyzer-ide.workspace = true` to `crates/compact-analyzer/Cargo.toml` `[dependencies]`.

`crates/analyzer-ide/src/lib.rs`:

```rust
//! Editor-agnostic IDE features over analyzer-core. Speaks byte offsets
//! (`TextSize`/`TextRange`) and `FileId` — zero LSP types.

mod goto_definition;

pub use goto_definition::goto_definition;

use analyzer_core::{FileId, TextRange};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavTarget {
    pub file: FileId,
    pub name_range: TextRange,
    pub full_range: TextRange,
}
```

Move the ENTIRE `Client`, `read_message`, `temp_doc`, and `did_open` definitions from `crates/compact-analyzer/tests/lsp_integration.rs` into a new `crates/compact-analyzer/tests/support/mod.rs`, making each `pub` (`pub struct Client`, `pub fn …`, and all `Client` methods `pub`). Add to it:

```rust
/// 0-based (line, UTF-16 column) of the first occurrence of `needle`.
pub fn lsp_position(text: &str, needle: &str) -> (u32, u32) {
    let idx = text.find(needle).expect("needle not in fixture");
    let line = text[..idx].matches('\n').count() as u32;
    let line_start = text[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = text[line_start..idx].encode_utf16().count() as u32;
    (line, col)
}
```

In `lsp_integration.rs`, replace the moved definitions with:

```rust
mod support;
use support::{Client, did_open, temp_doc};
```

(Adjust imports so the file compiles unchanged otherwise — the five M1 tests must still pass verbatim.)

- [ ] **Step 2: Write the failing tests**

Unit tests in `crates/analyzer-ide/src/goto_definition.rs` (tests module first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{fixture, AnalysisHost, FilePosition};

    fn goto(source: &str) -> Option<(bool, String)> {
        // Returns (is_stdlib_target, text at the target name range).
        let (clean, offset) = fixture::extract(source);
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        let target = goto_definition(&mut host, FilePosition { file, offset })?;
        let text = host.vfs_mut().read(target.file).unwrap();
        let name = text[target.name_range].to_string();
        Some((Some(target.file) == host.stdlib_file(), name))
    }

    #[test]
    fn goto_local_and_item() {
        assert_eq!(
            goto("circuit helper(): Field { return 1; }\ncircuit m(): Field { return help$0er(); }"),
            Some((false, "helper".to_string()))
        );
        assert_eq!(
            goto("circuit f(base: Field): Field { return ba$0se; }"),
            Some((false, "base".to_string()))
        );
    }

    #[test]
    fn goto_stdlib_lands_in_stub_file() {
        assert_eq!(
            goto("import CompactStandardLibrary;\ncircuit m(x: Field): Bytes<32> { return persistentHa$0sh<Field>(x); }"),
            Some((true, "persistentHash".to_string()))
        );
    }

    #[test]
    fn goto_nothing_on_unresolved() {
        assert_eq!(goto("circuit m(): Field { return myst$0ery(); }"), None);
    }
}
```

Create `crates/compact-analyzer/tests/lsp_navigation.rs`:

```rust
//! Black-box tests for the navigation features added in M2a.

mod support;

use serde_json::{Value, json};
use support::{Client, did_open, lsp_position, temp_doc};

const NAV_FIXTURE: &str = "\
import CompactStandardLibrary;
struct Point { x: Field; y: Field; }
circuit helper(base: Field): Field { return base; }
export circuit main(input: Field): Bytes<32> {
  const doubled = helper(input);
  return persistentHash<Field>(doubled);
}
";

fn open_fixture(client: &mut Client) -> (tempfile::TempDir, lsp_types::Url) {
    let (dir, uri) = temp_doc();
    did_open(client, &uri, 1, NAV_FIXTURE);
    (dir, uri)
}

fn definition_at(client: &mut Client, uri: &lsp_types::Url, line: u32, character: u32) -> Value {
    let response = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        }),
    );
    response["result"].clone()
}

#[test]
fn definition_of_local_call_and_stdlib() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = open_fixture(&mut client);

    // On `helper` in `helper(input)` (line 4) → the circuit on line 2.
    let (line, col) = lsp_position(NAV_FIXTURE, "helper(input)");
    let result = definition_at(&mut client, &uri, line, col);
    assert_eq!(result["uri"], json!(uri));
    assert_eq!(result["range"]["start"]["line"], 2);

    // On `persistentHash` → a DIFFERENT file (the materialized stdlib stub).
    let (line, col) = lsp_position(NAV_FIXTURE, "persistentHash");
    let result = definition_at(&mut client, &uri, line, col);
    let target_uri = result["uri"].as_str().expect("stdlib definition location");
    assert_ne!(target_uri, uri.as_str());
    assert!(target_uri.ends_with("CompactStandardLibrary.compact"));

    // On whitespace → null.
    let result = definition_at(&mut client, &uri, 0, 0);
    assert!(result.is_null());

    client.shutdown();
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p analyzer-ide` — FAIL: `goto_definition` not implemented.
Run: `cargo test -p compact-analyzer --test lsp_navigation` — FAIL: server answers `textDocument/definition` with MethodNotFound, so `result` is null where a location is expected.
Run: `cargo test -p compact-analyzer --test lsp_integration` — must still PASS after the harness refactor before proceeding.

- [ ] **Step 4: Implement the feature, the conversions, and the server wiring**

`crates/analyzer-ide/src/goto_definition.rs` (prepend above tests):

```rust
use analyzer_core::{AnalysisHost, FilePosition};

use crate::NavTarget;

/// Definition site of the identifier at `pos`.
pub fn goto_definition(host: &mut AnalysisHost, pos: FilePosition) -> Option<NavTarget> {
    let def = host.resolve(pos)?;
    let (file, name_range, full_range) = host.nav_info(&def)?;
    Some(NavTarget { file, name_range, full_range })
}
```

Add to `crates/compact-analyzer/src/lsp_utils.rs`:

```rust
pub(crate) fn offset_from_position(
    li: &analyzer_core::LineIndex,
    pos: Position,
) -> Option<analyzer_core::TextSize> {
    li.offset(analyzer_core::LineCol { line: pos.line, col: pos.character })
}

pub(crate) fn nav_target_to_location(
    host: &mut analyzer_core::AnalysisHost,
    target: &analyzer_ide::NavTarget,
) -> Option<Location> {
    let uri = Url::from_file_path(host.vfs().path(target.file)).ok()?;
    let analysis = host.analyze(target.file)?;
    Some(Location { uri, range: range_to_lsp(&analysis.line_index, target.name_range) })
}
```

In `crates/compact-analyzer/src/server.rs`:

1. At startup (in `run()`, after `initialize_finish`), materialize and register the stdlib — graceful on failure:

```rust
    let mut state = GlobalState::new(connection.sender.clone());
    let stdlib_dir = std::env::temp_dir()
        .join(format!("compact-analyzer-stdlib-{}", env!("CARGO_PKG_VERSION")));
    match analyzer_core::stdlib::materialize(&stdlib_dir) {
        Ok(path) => {
            state.host.register_stdlib(&path);
            eprintln!("compact-analyzer: stdlib stub at {}", path.display());
        }
        Err(err) => eprintln!("compact-analyzer: stdlib unavailable ({err}); continuing without it"),
    }
```

(`GlobalState` is defined in the same module as `run()`, so `state.host` is directly accessible — no visibility change needed.)

2. Capabilities: add to the `ServerCapabilities` literal:

```rust
        definition_provider: Some(lsp_types::OneOf::Left(true)),
```

3. Request routing: replace `handle_request` with a dispatcher:

```rust
    fn handle_request(&mut self, req: Request) {
        match req.method.as_str() {
            "textDocument/definition" => {
                let result = serde_json::from_value::<lsp_types::GotoDefinitionParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let doc = params.text_document_position_params;
                        self.position_to_file_position(&doc.text_document.uri, doc.position)
                    })
                    .and_then(|pos| analyzer_ide::goto_definition(&mut self.host, pos))
                    .and_then(|target| lsp_utils::nav_target_to_location(&mut self.host, &target));
                self.respond_ok(req.id, result);
            }
            _ => self.respond(Response::new_err(
                req.id,
                METHOD_NOT_FOUND,
                format!("method not supported: {}", req.method),
            )),
        }
    }

    /// (uri, Position) → FilePosition for an OPEN document. Unknown or
    /// unopened documents return None (the request answers null).
    fn position_to_file_position(
        &mut self,
        uri: &Url,
        position: lsp_types::Position,
    ) -> Option<analyzer_core::FilePosition> {
        let file = *self.open_files.get(uri)?;
        let analysis = self.host.analyze(file)?;
        let offset = lsp_utils::offset_from_position(&analysis.line_index, position)?;
        Some(analyzer_core::FilePosition { file, offset })
    }

    fn respond_ok<T: serde::Serialize>(&self, id: lsp_server::RequestId, result: T) {
        match serde_json::to_value(result) {
            Ok(value) => self.respond(Response::new_ok(id, value)),
            Err(err) => self.respond(Response::new_err(
                id,
                INTERNAL_ERROR,
                format!("failed to serialize response: {err}"),
            )),
        }
    }
```

(`serde` is not currently a direct dependency of compact-analyzer: add `serde.workspace = true` to its `[dependencies]`.)

- [ ] **Step 5: Run all tests to verify they pass**

Run: `cargo test --workspace --locked`
Expected: PASS — analyzer-core 50, analyzer-ide 3, compact-analyzer 5 unit + 5 integration + 1 navigation

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: no warnings

```bash
git add Cargo.toml Cargo.lock crates
git commit -m "feat(ide): analyzer-ide crate with end-to-end goto-definition"
```

---

### Task 6: find-references

**Files:**
- Create: `crates/analyzer-ide/src/references.rs`
- Modify: `crates/analyzer-ide/src/lib.rs`
- Modify: `crates/compact-analyzer/src/server.rs`
- Modify: `crates/compact-analyzer/tests/lsp_navigation.rs`

**Interfaces:**
- Consumes: `resolve`, `resolve_name_at` via re-resolution, `def_name`, `nav_info`
- Produces:
  - `analyzer_ide::find_references(host: &mut AnalysisHost, pos: FilePosition, include_declaration: bool) -> Option<Vec<NavTarget>>` — same-file references, sorted by range start, deduplicated; the declaration's name range included only when `include_declaration`
  - Server: `references_provider: Some(OneOf::Left(true))`; `textDocument/references` handler

- [ ] **Step 1: Write the failing tests**

`crates/analyzer-ide/src/references.rs` tests module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{fixture, AnalysisHost, FilePosition};

    fn refs(source: &str, include_decl: bool) -> Vec<String> {
        let (clean, offset) = fixture::extract(source);
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean.clone(), 1);
        let targets =
            find_references(&mut host, FilePosition { file, offset }, include_decl).unwrap_or_default();
        targets
            .into_iter()
            .map(|t| format!("{:?}", t.name_range))
            .collect()
    }

    #[test]
    fn finds_all_uses_of_a_param() {
        let source = "circuit f(ba$0se: Field): Field {\n  const a = base + base;\n  return base;\n}";
        assert_eq!(refs(source, false).len(), 3);
        assert_eq!(refs(source, true).len(), 4); // + declaration
    }

    #[test]
    fn shadowed_bindings_are_distinct() {
        // References of the FIRST x must not include uses of the second.
        let source = "circuit f(): Field {\n  const x$0 = 1;\n  const y = x;\n  const x = 2;\n  return x;\n}";
        assert_eq!(refs(source, false).len(), 1); // only `const y = x;`
    }

    #[test]
    fn item_references_from_use_site() {
        let source = "circuit helper(): Field { return 1; }\n\
                      circuit a(): Field { return helper(); }\n\
                      circuit b(): Field { return hel$0per(); }";
        assert_eq!(refs(source, false).len(), 2);
        assert_eq!(refs(source, true).len(), 3);
    }

    #[test]
    fn no_references_for_unresolvable() {
        assert!(refs("circuit f(): Field { return myst$0ery; }", false).is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-ide`
Expected: FAIL — `find_references` not found

- [ ] **Step 3: Implement**

`crates/analyzer-ide/src/references.rs` (prepend):

```rust
use analyzer_core::{AnalysisHost, FilePosition, SyntaxKind, SyntaxNode};

use crate::NavTarget;

/// All same-file references to the definition at `pos`. Each candidate
/// IDENT with matching text is re-resolved and kept only if it resolves to
/// the same definition — so shadowed bindings are distinguished correctly.
pub fn find_references(
    host: &mut AnalysisHost,
    pos: FilePosition,
    include_declaration: bool,
) -> Option<Vec<NavTarget>> {
    let def = host.resolve(pos)?;
    let name = host.def_name(&def)?;
    let (def_file, def_name_range, _) = host.nav_info(&def)?;

    let analysis = host.analyze(pos.file)?;
    let root = SyntaxNode::new_root(analysis.green.clone());

    let candidates: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::IDENT && t.text() == name)
        .map(|t| t.text_range())
        .collect();

    let mut out = Vec::new();
    for range in candidates {
        let is_declaration = def_file == pos.file && range == def_name_range;
        if is_declaration && !include_declaration {
            continue;
        }
        let resolved = host.resolve(FilePosition { file: pos.file, offset: range.start() });
        if resolved.as_ref() == Some(&def) {
            out.push(NavTarget { file: pos.file, name_range: range, full_range: range });
        }
    }
    out.sort_by_key(|t| t.name_range.start());
    out.dedup();
    Some(out)
}
```

Register in `crates/analyzer-ide/src/lib.rs`: `mod references;` + `pub use references::find_references;`.

Server wiring in `server.rs` — capability `references_provider: Some(lsp_types::OneOf::Left(true))`, and a new match arm:

```rust
            "textDocument/references" => {
                let result = serde_json::from_value::<lsp_types::ReferenceParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let include_decl = params.context.include_declaration;
                        let pos = self.position_to_file_position(
                            &params.text_document_position.text_document.uri,
                            params.text_document_position.position,
                        )?;
                        analyzer_ide::find_references(&mut self.host, pos, include_decl)
                    })
                    .map(|targets| {
                        targets
                            .iter()
                            .filter_map(|t| lsp_utils::nav_target_to_location(&mut self.host, t))
                            .collect::<Vec<_>>()
                    });
                self.respond_ok(req.id, result);
            }
```

Integration test appended to `lsp_navigation.rs`:

```rust
#[test]
fn references_for_helper() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = open_fixture(&mut client);

    let (line, col) = lsp_position(NAV_FIXTURE, "helper(base");
    let response = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": col},
            "context": {"includeDeclaration": true},
        }),
    );
    let locations = response["result"].as_array().expect("array of locations");
    assert_eq!(locations.len(), 2); // declaration + call in main

    client.shutdown();
}
```

- [ ] **Step 4: Run all tests to verify they pass**

Run: `cargo test --workspace --locked`
Expected: PASS — analyzer-ide now 7 unit tests; navigation integration now 2 tests

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates
git commit -m "feat(ide): find-references with shadowing-aware re-resolution"
```

---

### Task 7: rename

**Files:**
- Create: `crates/analyzer-ide/src/rename.rs`
- Modify: `crates/analyzer-ide/src/lib.rs`
- Modify: `crates/compact-analyzer/src/server.rs`
- Modify: `crates/compact-analyzer/tests/lsp_navigation.rs`

**Interfaces:**
- Consumes: `find_references` (Task 6), `resolve_name_at`, `stdlib_file`, `nav_info`
- Produces:
  - `analyzer_ide::SourceEdit { pub file: FileId, pub range: TextRange, pub new_text: String }`
  - `analyzer_ide::RenameError` — `NotFound | InvalidName(String) | Keyword(String) | BuiltinTarget | Conflict(String)` (implements `Display`)
  - `analyzer_ide::rename(host: &mut AnalysisHost, pos: FilePosition, new_name: &str) -> Result<Vec<SourceEdit>, RenameError>`
  - Server: `rename_provider: Some(OneOf::Left(true))`; `textDocument/rename` handler returning a `WorkspaceEdit` or an LSP error (code `-32600` InvalidRequest is NOT used — use `INTERNAL_ERROR`? No: use request-failed `-32803` with the human-readable reason)

- [ ] **Step 1: Write the failing tests**

`crates/analyzer-ide/src/rename.rs` tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{fixture, AnalysisHost, FilePosition};

    fn try_rename(source: &str, new_name: &str) -> Result<String, RenameError> {
        let (clean, offset) = fixture::extract(source);
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean.clone(), 1);
        let edits = rename(&mut host, FilePosition { file, offset }, new_name)?;
        // Apply edits (back to front) and return the new text.
        let mut text = clean;
        let mut sorted = edits;
        sorted.sort_by_key(|e| std::cmp::Reverse(u32::from(e.range.start())));
        for edit in sorted {
            let start = u32::from(edit.range.start()) as usize;
            let end = u32::from(edit.range.end()) as usize;
            text.replace_range(start..end, &edit.new_text);
        }
        Ok(text)
    }

    #[test]
    fn renames_definition_and_all_uses() {
        let result = try_rename(
            "circuit f(ba$0se: Field): Field { return base + base; }",
            "input",
        )
        .unwrap();
        assert_eq!(result, "circuit f(input: Field): Field { return input + input; }");
    }

    #[test]
    fn refuses_keywords_and_invalid_names() {
        let src = "circuit f(ba$0se: Field): Field { return base; }";
        assert!(matches!(try_rename(src, "circuit"), Err(RenameError::Keyword(_))));
        assert!(matches!(try_rename(src, "9lives"), Err(RenameError::InvalidName(_))));
        assert!(matches!(try_rename(src, "has space"), Err(RenameError::InvalidName(_))));
    }

    #[test]
    fn refuses_conflicting_target_name() {
        let src = "circuit f(): Field {\n  const a$0 = 1;\n  const b = 2;\n  return a + b;\n}";
        assert!(matches!(try_rename(src, "b"), Err(RenameError::Conflict(_))));
    }

    #[test]
    fn refuses_stdlib_targets() {
        let (clean, offset) = fixture::extract(
            "import CompactStandardLibrary;\ncircuit m(x: Field): Bytes<32> { return persistentHa$0sh<Field>(x); }",
        );
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        assert!(matches!(
            rename(&mut host, FilePosition { file, offset }, "myHash"),
            Err(RenameError::BuiltinTarget)
        ));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-ide`
Expected: FAIL — `rename`/`RenameError` not found

- [ ] **Step 3: Implement**

`crates/analyzer-ide/src/rename.rs` (prepend):

```rust
use analyzer_core::{AnalysisHost, FilePosition, TextRange};

use crate::find_references;

/// Active Compact keywords at language 0.23 (from the language reference).
const KEYWORDS: &[&str] = &[
    "as", "assert", "Boolean", "Bytes", "circuit", "const", "constructor", "contract",
    "default", "disclose", "else", "enum", "export", "false", "Field", "fold", "for",
    "from", "if", "import", "include", "ledger", "map", "module", "new", "of", "Opaque",
    "pad", "pragma", "prefix", "pure", "return", "sealed", "slice", "struct", "true",
    "type", "Uint", "Vector", "witness",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdit {
    pub file: analyzer_core::FileId,
    pub range: TextRange,
    pub new_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameError {
    NotFound,
    InvalidName(String),
    Keyword(String),
    BuiltinTarget,
    Conflict(String),
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameError::NotFound => write!(f, "nothing renameable at the cursor"),
            RenameError::InvalidName(n) => write!(f, "`{n}` is not a valid identifier"),
            RenameError::Keyword(n) => write!(f, "`{n}` is a Compact keyword"),
            RenameError::BuiltinTarget => write!(f, "cannot rename a standard-library item"),
            RenameError::Conflict(n) => {
                write!(f, "`{n}` already resolves in the definition's scope")
            }
        }
    }
}

pub fn rename(
    host: &mut AnalysisHost,
    pos: FilePosition,
    new_name: &str,
) -> Result<Vec<SourceEdit>, RenameError> {
    if !is_valid_identifier(new_name) {
        return Err(RenameError::InvalidName(new_name.to_string()));
    }
    if KEYWORDS.contains(&new_name) {
        return Err(RenameError::Keyword(new_name.to_string()));
    }
    let def = host.resolve(pos).ok_or(RenameError::NotFound)?;
    let (def_file, def_name_range, _) = host.nav_info(&def).ok_or(RenameError::NotFound)?;
    if Some(def_file) == host.stdlib_file() {
        return Err(RenameError::BuiltinTarget);
    }
    // Conservative conflict check: the new name must not already resolve at
    // the definition site. (Per-use-site conflicts are a known M2b
    // refinement.)
    if host
        .resolve_name_at(
            FilePosition { file: def_file, offset: def_name_range.start() },
            new_name,
        )
        .is_some()
    {
        return Err(RenameError::Conflict(new_name.to_string()));
    }
    let refs = find_references(host, pos, true).ok_or(RenameError::NotFound)?;
    Ok(refs
        .into_iter()
        .map(|t| SourceEdit { file: t.file, range: t.name_range, new_text: new_name.to_string() })
        .collect())
}

fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
```

Register in lib.rs: `mod rename;` + `pub use rename::{rename, RenameError, SourceEdit};`.

Server wiring — capability `rename_provider: Some(lsp_types::OneOf::Left(true))`, constant `const REQUEST_FAILED: i32 = -32803;`, and match arm:

```rust
            "textDocument/rename" => {
                let Ok(params) = serde_json::from_value::<lsp_types::RenameParams>(req.params)
                else {
                    self.respond(Response::new_err(req.id, INTERNAL_ERROR, "bad params".into()));
                    return;
                };
                let uri = params.text_document_position.text_document.uri.clone();
                let Some(pos) = self.position_to_file_position(
                    &uri,
                    params.text_document_position.position,
                ) else {
                    self.respond_ok(req.id, Option::<lsp_types::WorkspaceEdit>::None);
                    return;
                };
                match analyzer_ide::rename(&mut self.host, pos, &params.new_name) {
                    Ok(edits) => {
                        let mut changes: std::collections::HashMap<Url, Vec<lsp_types::TextEdit>> =
                            std::collections::HashMap::new();
                        for edit in edits {
                            let Some(target_uri) =
                                Url::from_file_path(self.host.vfs().path(edit.file)).ok()
                            else {
                                continue;
                            };
                            let Some(analysis) = self.host.analyze(edit.file) else { continue };
                            changes.entry(target_uri).or_default().push(lsp_types::TextEdit {
                                range: lsp_utils::range_to_lsp(&analysis.line_index, edit.range),
                                new_text: edit.new_text,
                            });
                        }
                        self.respond_ok(
                            req.id,
                            Some(lsp_types::WorkspaceEdit {
                                changes: Some(changes),
                                ..Default::default()
                            }),
                        );
                    }
                    Err(err) => self.respond(Response::new_err(req.id, REQUEST_FAILED, err.to_string())),
                }
            }
```

Integration test appended to `lsp_navigation.rs`:

```rust
#[test]
fn rename_rejects_keyword_and_applies_valid() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = open_fixture(&mut client);

    let (line, col) = lsp_position(NAV_FIXTURE, "doubled");
    let response = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": col},
            "newName": "circuit",
        }),
    );
    assert!(response.get("error").is_some(), "keyword rename must fail: {response}");

    let response = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": col},
            "newName": "tripled",
        }),
    );
    let edits = &response["result"]["changes"][uri.as_str()];
    assert_eq!(edits.as_array().unwrap().len(), 2); // decl + use

    client.shutdown();
}
```

- [ ] **Step 4: Run all tests to verify they pass**

Run: `cargo test --workspace --locked`
Expected: PASS — analyzer-ide 11 unit tests; navigation integration 3 tests

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates
git commit -m "feat(ide): rename with keyword, validity, conflict, and builtin guards"
```

---

### Task 8: hover

**Files:**
- Create: `crates/analyzer-ide/src/hover.rs`
- Modify: `crates/analyzer-ide/src/lib.rs`
- Modify: `crates/compact-analyzer/src/server.rs`
- Modify: `crates/compact-analyzer/tests/lsp_navigation.rs`

**Interfaces:**
- Consumes: `resolve`, `nav_info`, `Symbol { signature, doc }`
- Produces:
  - `analyzer_ide::HoverResult { pub markdown: String }` (no range: LSP hover range is optional, and the definition's range is the wrong thing to highlight when hovering a use site)
  - `analyzer_ide::hover(host: &mut AnalysisHost, pos: FilePosition) -> Option<HoverResult>` — items render as a fenced ```compact signature block plus the doc paragraph; locals render their detail in the fenced block
  - Server: `hover_provider: Some(HoverProviderCapability::Simple(true))`; `textDocument/hover` handler

- [ ] **Step 1: Write the failing tests**

`crates/analyzer-ide/src/hover.rs` tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{fixture, AnalysisHost, FilePosition};

    fn hover_md(source: &str) -> Option<String> {
        let (clean, offset) = fixture::extract(source);
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        Some(hover(&mut host, FilePosition { file, offset })?.markdown)
    }

    #[test]
    fn hover_on_circuit_shows_signature_and_doc() {
        let md = hover_md(
            "// Doubles a value.\ncircuit double(x: Field): Field { return x + x; }\n\
             circuit m(): Field { return dou$0ble(1); }",
        )
        .unwrap();
        assert_eq!(
            md,
            "```compact\ncircuit double(x: Field): Field\n```\n\nDoubles a value."
        );
    }

    #[test]
    fn hover_on_param_shows_detail() {
        let md = hover_md("circuit f(base: Field): Field { return ba$0se; }").unwrap();
        assert_eq!(md, "```compact\nbase: Field\n```");
    }

    #[test]
    fn hover_on_stdlib_shows_bundled_doc() {
        let md = hover_md(
            "import CompactStandardLibrary;\ncircuit m(x: Field): Bytes<32> { return persistentHa$0sh<Field>(x); }",
        )
        .unwrap();
        assert!(md.contains("persistentHash"), "{md}");
        assert!(md.contains("SHA-256"), "{md}");
    }

    #[test]
    fn hover_on_nothing_is_none() {
        assert!(hover_md("circuit f(): Field { return myst$0ery; }").is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-ide`
Expected: FAIL — `hover` not found

- [ ] **Step 3: Implement**

`crates/analyzer-ide/src/hover.rs` (prepend):

```rust
use analyzer_core::{AnalysisHost, Definition, FilePosition};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverResult {
    pub markdown: String,
}

pub fn hover(host: &mut AnalysisHost, pos: FilePosition) -> Option<HoverResult> {
    let def = host.resolve(pos)?;
    let markdown = match &def {
        Definition::Item { file, index } => {
            let sym = host.analyze(*file)?.item_tree.symbols.get(*index as usize)?.clone();
            match &sym.doc {
                Some(doc) => format!("```compact\n{}\n```\n\n{}", sym.signature, doc),
                None => format!("```compact\n{}\n```", sym.signature),
            }
        }
        Definition::Local { detail, .. } => format!("```compact\n{detail}\n```"),
    };
    Some(HoverResult { markdown })
}
```

Register: `mod hover;` + `pub use hover::{hover, HoverResult};`.

Server — capability `hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true))`, match arm:

```rust
            "textDocument/hover" => {
                let result = serde_json::from_value::<lsp_types::HoverParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let doc = params.text_document_position_params;
                        let pos = self
                            .position_to_file_position(&doc.text_document.uri, doc.position)?;
                        let hover = analyzer_ide::hover(&mut self.host, pos)?;
                        Some(lsp_types::Hover {
                            contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                                kind: lsp_types::MarkupKind::Markdown,
                                value: hover.markdown,
                            }),
                            range: None,
                        })
                    });
                self.respond_ok(req.id, result);
            }
```

Integration test appended to `lsp_navigation.rs`:

```rust
#[test]
fn hover_shows_stdlib_signature() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = open_fixture(&mut client);

    let (line, col) = lsp_position(NAV_FIXTURE, "persistentHash");
    let response = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": col},
        }),
    );
    let value = response["result"]["contents"]["value"].as_str().unwrap();
    assert!(value.contains("persistentHash"), "{value}");

    client.shutdown();
}
```

- [ ] **Step 4: Run all tests to verify they pass**

Run: `cargo test --workspace --locked`
Expected: PASS — analyzer-ide 15 unit tests; navigation integration 4 tests

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates
git commit -m "feat(ide): hover with rendered signatures and doc comments"
```

---

### Task 9: document symbols + severity-coverage carry-over + README

**Files:**
- Create: `crates/analyzer-ide/src/document_symbols.rs`
- Modify: `crates/analyzer-ide/src/lib.rs`
- Modify: `crates/compact-analyzer/src/server.rs`
- Modify: `crates/compact-analyzer/src/lsp_utils.rs` (severity tests carry-over)
- Modify: `crates/compact-analyzer/tests/lsp_navigation.rs`
- Modify: `README.md`

**Interfaces:**
- Consumes: `ItemTree` hierarchy
- Produces:
  - `analyzer_ide::DocSymbol { pub name: String, pub kind: SymbolKind, pub detail: Option<String>, pub full_range: TextRange, pub selection_range: TextRange, pub children: Vec<DocSymbol> }`
  - `analyzer_ide::document_symbols(host: &mut AnalysisHost, file: FileId) -> Vec<DocSymbol>` (nameless symbols other than constructors are skipped)
  - Server: `document_symbol_provider: Some(OneOf::Left(true))`; handler returning nested `DocumentSymbol`s
  - `lsp_utils`: unit tests asserting `Severity::Warning → WARNING` and `Severity::Note → INFORMATION` (M1 final-review carry-over)

- [ ] **Step 1: Write the failing tests**

`crates/analyzer-ide/src/document_symbols.rs` tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::AnalysisHost;

    #[test]
    fn nested_symbols_mirror_the_item_tree() {
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(
            file,
            "module M {\n  export circuit f(): Field { return 0; }\n}\n\
             struct Point { x: Field; }\n\
             constructor(v: Field) { }\n"
                .to_string(),
            1,
        );
        let symbols = document_symbols(&mut host, file);
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["M", "Point", "constructor"]);
        assert_eq!(symbols[0].children[0].name, "f");
        assert_eq!(symbols[1].children[0].name, "x");
        assert_eq!(symbols[0].kind, analyzer_core::SymbolKind::Module);
    }
}
```

Add to `crates/compact-analyzer/src/lsp_utils.rs` tests module (M1 carry-over):

```rust
    #[test]
    fn warning_and_note_severities_map_correctly() {
        let li = LineIndex::new(Arc::from("x"));
        let uri = lsp_types::Url::parse("file:///tmp/x.compact").unwrap();
        let warn = Diagnostic::warning(
            DiagnosticCode::new("W", 1),
            "w".to_string(),
            TextRange::new(TextSize::new(0), TextSize::new(1)),
        );
        assert_eq!(diagnostic_to_lsp(&warn, &li, &uri).severity, Some(DiagnosticSeverity::WARNING));
        let note = Diagnostic::note(
            DiagnosticCode::new("N", 1),
            "n".to_string(),
            TextRange::new(TextSize::new(0), TextSize::new(1)),
        );
        assert_eq!(
            diagnostic_to_lsp(&note, &li, &uri).severity,
            Some(DiagnosticSeverity::INFORMATION)
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-ide` — FAIL: `document_symbols` not found. (`cargo test -p compact-analyzer` for the severity test compiles and passes immediately — that's fine, it is a coverage carry-over, not TDD of new behavior.)

- [ ] **Step 3: Implement**

`crates/analyzer-ide/src/document_symbols.rs` (prepend):

```rust
use analyzer_core::{AnalysisHost, FileId, ItemTree, SymbolKind, TextRange};

#[derive(Clone, Debug)]
pub struct DocSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: Option<String>,
    pub full_range: TextRange,
    pub selection_range: TextRange,
    pub children: Vec<DocSymbol>,
}

pub fn document_symbols(host: &mut AnalysisHost, file: FileId) -> Vec<DocSymbol> {
    let Some(analysis) = host.analyze(file) else { return Vec::new() };
    let tree = analysis.item_tree;
    tree.top_level().filter_map(|(idx, _)| build(&tree, idx)).collect()
}

fn build(tree: &ItemTree, index: u32) -> Option<DocSymbol> {
    let symbol = &tree.symbols[index as usize];
    if symbol.name.is_empty() {
        return None; // error-recovered nameless item
    }
    Some(DocSymbol {
        name: symbol.name.clone(),
        kind: symbol.kind,
        detail: Some(symbol.signature.clone()).filter(|s| s != &symbol.name),
        full_range: symbol.full_range,
        selection_range: symbol.name_range,
        children: tree.children_of(index).filter_map(|(idx, _)| build(tree, idx)).collect(),
    })
}
```

Register: `mod document_symbols;` + `pub use document_symbols::{document_symbols, DocSymbol};`.

Server — capability `document_symbol_provider: Some(lsp_types::OneOf::Left(true))`, kind mapping + match arm (DocumentSymbol has a deprecated field; the `#[allow(deprecated)]` is required):

```rust
            "textDocument/documentSymbol" => {
                let result = serde_json::from_value::<lsp_types::DocumentSymbolParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let file = *self.open_files.get(&params.text_document.uri)?;
                        let analysis = self.host.analyze(file)?;
                        let line_index = analysis.line_index.clone();
                        let symbols = analyzer_ide::document_symbols(&mut self.host, file);
                        Some(
                            symbols
                                .into_iter()
                                .map(|s| doc_symbol_to_lsp(&line_index, s))
                                .collect::<Vec<_>>(),
                        )
                    });
                self.respond_ok(req.id, result.map(lsp_types::DocumentSymbolResponse::Nested));
            }
```

And in `server.rs` (module level):

```rust
#[allow(deprecated)] // lsp_types::DocumentSymbol::deprecated
fn doc_symbol_to_lsp(
    li: &analyzer_core::LineIndex,
    symbol: analyzer_ide::DocSymbol,
) -> lsp_types::DocumentSymbol {
    lsp_types::DocumentSymbol {
        name: symbol.name,
        detail: symbol.detail,
        kind: symbol_kind_to_lsp(symbol.kind),
        tags: None,
        deprecated: None,
        range: crate::lsp_utils::range_to_lsp(li, symbol.full_range),
        selection_range: crate::lsp_utils::range_to_lsp(li, symbol.selection_range),
        children: Some(
            symbol.children.into_iter().map(|c| doc_symbol_to_lsp(li, c)).collect(),
        ),
    }
}

fn symbol_kind_to_lsp(kind: analyzer_core::SymbolKind) -> lsp_types::SymbolKind {
    use analyzer_core::SymbolKind as K;
    match kind {
        K::Circuit | K::CircuitSig | K::Witness => lsp_types::SymbolKind::FUNCTION,
        K::Struct => lsp_types::SymbolKind::STRUCT,
        K::StructField => lsp_types::SymbolKind::FIELD,
        K::Enum => lsp_types::SymbolKind::ENUM,
        K::EnumVariant => lsp_types::SymbolKind::ENUM_MEMBER,
        K::Module => lsp_types::SymbolKind::MODULE,
        K::TypeAlias => lsp_types::SymbolKind::INTERFACE,
        K::Ledger => lsp_types::SymbolKind::PROPERTY,
        K::Contract => lsp_types::SymbolKind::INTERFACE,
        K::ContractCircuit => lsp_types::SymbolKind::METHOD,
        K::Constructor => lsp_types::SymbolKind::CONSTRUCTOR,
    }
}
```

Integration test appended to `lsp_navigation.rs`:

```rust
#[test]
fn document_symbols_are_nested() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = open_fixture(&mut client);

    let response = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    );
    let symbols = response["result"].as_array().unwrap();
    let names: Vec<_> = symbols.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Point", "helper", "main"]);
    let point_children = symbols[0]["children"].as_array().unwrap();
    assert_eq!(point_children.len(), 2);

    client.shutdown();
}
```

README: replace the `## Status` section body with:

```markdown
## Status: M2a — single-file navigation

- Live syntax diagnostics as you type (no compiler round-trip)
- Goto-definition, find-references, rename, hover, and document symbols
  across locals, top-level items, in-file modules, and the bundled
  `CompactStandardLibrary` (goto lands in a readable stub with docs)
- Correct UTF-16 positions; the server never dies on malformed code

Cross-file imports, workspace symbols, completion, and compiler-integrated
diagnostics are next — see
`docs/superpowers/specs/2026-07-06-compact-analyzer-design.md`.
```

- [ ] **Step 4: Run all tests to verify they pass**

Run: `cargo test --workspace --locked`
Expected: PASS — full suite: analyzer-core 50, analyzer-ide 16, compact-analyzer 6 unit + 5 integration + 5 navigation

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates README.md
git commit -m "feat(ide): document symbols; carry-over severity tests; M2a README"
```

---

## Self-review notes (completed during planning)

- **Fixture discipline:** the stdlib stub and the resolver mega-fixture were run through compactp 0.1.0-beta.1 during planning (zero errors both). The CST shape table in Global Constraints was verified with live `compactp cst` runs (TYPE_SIZE/IDENT, MEMBER_EXPR/NAME_EXPR+IDENT, STRUCT_EXPR/IDENT+STRUCT_FIELD_INIT). SyntaxKind variant NAMES used in match arms are expected from the compactp public-api baseline but individual spellings (e.g. `L_BRACE`, `AS_KW`, `STRUCT_PAT_FIELD`, `CONTRACT_CIRCUIT`) must be confirmed against the enum at implementation time — tasks carry explicit NOTES to substitute actual names and report.
- **Resolution semantics are OUR feature contract** (the tests define them); the compiler-verified facts (import forms, stdlib internality, prefix/selective composition) constrain them where they overlap.
- **Layering:** resolver + ItemTree live in analyzer-core per the spec's crate responsibilities; analyzer-ide stays a thin feature layer; only the binary touches lsp-types. `enclosing_module` uses `last()` on a document-order filter — correct for nested modules since ancestors precede descendants in ItemTree insertion order.
- **Known M2a simplifications (deliberate, recorded):** one flat namespace (no type-vs-value namespace split); rename conflict check is definition-site only; references/rename are single-file; `IfStmt` has no condition accessor in compactp (irrelevant here — no bindings in conditions); export lists (`export { a, b };`) do not affect resolution visibility in M2a (everything in-file is visible; export only gates module-member imports).
- **Type consistency check:** `Definition`/`FilePosition`/`NavTarget`/`SourceEdit` signatures in Interfaces blocks match all use sites in Tasks 5-9 (checked name-by-name). Test-count arithmetic: core 17→23→27→37→50; ide 3→7→11→15→16; binary unit 5→6 (severity), integration 5, navigation 1→2→3→4→5.
- **Spec coverage (M2a slice):** name-resolution core (§2 staged plan, §3.4) → Tasks 3-4; stdlib stubs (§3.5) → Task 2; goto-def/references/rename/hover/document-symbols rows of the v1 feature table (§4) → Tasks 5-9. Deferred to M2b: cross-file resolution + workspace index + workspace symbols (§3.4, §4); deferred to M3: completion, semantic tokens, folding.
