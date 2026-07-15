//! Per-file symbol extraction: every declaration with its name span,
//! rendered signature, doc comment, and container. This is the flat,
//! cheap-to-rebuild structure everything in M2 navigates over.

use compactp_ast::{AstNode, GenericParamList, Item};
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

#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Number of generic parameters the declaration takes (`0` when it has
    /// none or cannot be generic). Read by the generic-arity check to compare
    /// against a use site's supplied generic-argument count.
    pub generic_param_count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
                self.push_named(
                    it.syntax(),
                    it.name(),
                    SymbolKind::Circuit,
                    it.is_exported(),
                    parent,
                    generic_count(it.generic_params()),
                );
            }
            Item::CircuitDecl(it) => {
                self.push_named(
                    it.syntax(),
                    it.name(),
                    SymbolKind::CircuitSig,
                    it.is_exported(),
                    parent,
                    generic_count(it.generic_params()),
                );
            }
            Item::WitnessDecl(it) => {
                self.push_named(
                    it.syntax(),
                    it.name(),
                    SymbolKind::Witness,
                    it.is_exported(),
                    parent,
                    generic_count(it.generic_params()),
                );
            }
            Item::LedgerDecl(it) => {
                self.push_named(
                    it.syntax(),
                    it.name(),
                    SymbolKind::Ledger,
                    it.is_exported(),
                    parent,
                    0,
                );
            }
            Item::TypeDecl(it) => {
                self.push_named(
                    it.syntax(),
                    it.name(),
                    SymbolKind::TypeAlias,
                    it.is_exported(),
                    parent,
                    generic_count(it.generic_params()),
                );
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
                    generic_param_count: 0,
                });
            }
            Item::StructDef(it) => {
                let idx = self.push_named(
                    it.syntax(),
                    it.name(),
                    SymbolKind::Struct,
                    it.is_exported(),
                    parent,
                    generic_count(it.generic_params()),
                );
                for field in it.fields() {
                    if let Some(name) = field.name() {
                        let ty = field
                            .ty()
                            .map(|t| collapse_ws(&t.syntax().text().to_string()));
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
                            generic_param_count: 0,
                        });
                    }
                }
            }
            Item::EnumDef(it) => {
                let idx = self.push_named(
                    it.syntax(),
                    it.name(),
                    SymbolKind::Enum,
                    it.is_exported(),
                    parent,
                    0,
                );
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
                            generic_param_count: 0,
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
                    0,
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
                            generic_param_count: 0,
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
                    0,
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
        generic_param_count: u32,
    ) -> u32 {
        let (name_text, name_range) = match &name {
            Some(token) => (token.text().to_string(), token.text_range()),
            // Error-recovered item without a name: push an empty-name
            // `Symbol` as a placeholder so the caller still gets a valid
            // parent index. The placeholder is inert downstream —
            // `document_symbols` drops empty-name symbols and the resolver
            // never matches an empty name.
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
            generic_param_count,
        })
    }
}

/// The number of generic parameters a definition node declares, or `0`.
/// `T` and `#n` parameters both count. Generic over any node exposing
/// `generic_params()` (circuits, functions/witnesses, structs, enums, type
/// aliases, contracts).
fn generic_count(params: Option<GenericParamList>) -> u32 {
    params.map(|p| p.params().count() as u32).unwrap_or(0)
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
        let Some(token) = element.into_token() else {
            continue;
        };
        if let Some(stop) = body_start
            && token.text_range().start() >= stop
        {
            break;
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
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

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
        let fields: Vec<_> = t
            .children_of(point_idx)
            .map(|(_, s)| s.name.clone())
            .collect();
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
    fn records_generic_param_count() {
        let t = tree(
            "export struct Box<T> { v: T; }\n\
             export struct Pair<A, B> { a: A; b: B; }\n\
             export struct Point { x: Field; y: Field; }\n\
             export enum Color { Red, Green }\n\
             export circuit id<T>(x: T): T { return x; }\n\
             export circuit plain(x: Field): Field { return x; }",
        );
        assert_eq!(find(&t, "Box").generic_param_count, 1);
        assert_eq!(find(&t, "Pair").generic_param_count, 2);
        assert_eq!(find(&t, "Point").generic_param_count, 0);
        assert_eq!(find(&t, "Color").generic_param_count, 0);
        assert_eq!(find(&t, "id").generic_param_count, 1);
        assert_eq!(find(&t, "plain").generic_param_count, 0);
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
