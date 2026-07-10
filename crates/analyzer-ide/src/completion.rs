//! Context-aware completion over the (possibly error-recovered) CST.

use analyzer_core::{AnalysisHost, FilePosition, SyntaxKind, SyntaxNode, SyntaxToken, TextSize};
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
    "export",
    "sealed",
    "pure",
    "circuit",
    "witness",
    "ledger",
    "struct",
    "enum",
    "module",
    "contract",
    "constructor",
    "type",
    "new",
    "import",
    "include",
    "pragma",
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
        .chain(
            start
                .prev_token()
                .into_iter()
                .flat_map(|t| std::iter::successors(Some(t), |p| p.prev_token())),
        )
        .find(|t| !t.kind().is_trivia() && t.text_range().start() < offset)
}

/// The receiver expr node if the cursor is a member-access field position.
fn member_receiver(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    // Partial member: `c.incr⎸` — the IDENT's parent is MEMBER_EXPR/CALL_EXPR,
    // preceded by a DOT.
    if let Some(id) = completion_ident(root, offset)
        && let Some(parent) = id.parent()
        && matches!(
            parent.kind(),
            SyntaxKind::MEMBER_EXPR | SyntaxKind::CALL_EXPR
        )
        && has_dot_before(&parent, id.text_range().start())
    {
        return first_expr_child(&parent);
    }
    // Empty member: `c.⎸` — the token to the left is a DOT under MEMBER/CALL.
    if let Some(dot) = left_token(root, offset).filter(|t| t.kind() == SyntaxKind::DOT)
        && let Some(parent) = dot.parent()
        && matches!(
            parent.kind(),
            SyntaxKind::MEMBER_EXPR | SyntaxKind::CALL_EXPR
        )
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
    node.children()
        .find(|c| compactp_ast::Expr::can_cast(c.kind()))
}

/// The start offset of the first non-trivia token under `node`. Some nodes
/// (e.g. `NAME_EXPR`) carry their leading trivia as their own first child
/// token, so `node.text_range().start()` can point at whitespace rather than
/// the identifier — callers that need a position resolvable via
/// `ident_at_offset` must use this instead.
///
/// PRECONDITION: only valid for a **bare-identifier** node (`NAME_EXPR`). This
/// descends the *entire* subtree (`descendants_with_tokens`), so for a compound
/// node (`CALL_EXPR`/`INDEX_EXPR`/nested `MEMBER_EXPR`) it returns the offset of
/// the *innermost-leftmost* identifier, not the receiver as a whole — callers
/// must gate on `node.kind() == NAME_EXPR` before calling (see `push_member`).
fn non_trivia_start(node: &SyntaxNode) -> TextSize {
    node.descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|t| !t.kind().is_trivia())
        .map(|t| t.text_range().start())
        .unwrap_or_else(|| node.text_range().start())
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
            if r.kind() == SyntaxKind::IDENT {
                r.parent()
            } else {
                l.parent()
            }
        }
        TokenAtOffset::None => None,
    };
    if let Some(n) = &covering
        && n.ancestors()
            .any(|a| is_type_node(a.kind()) || a.kind() == SyntaxKind::TYPE_REF)
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
            if r.kind() == SyntaxKind::IDENT {
                r.parent()
            } else {
                l.parent()
            }
        }
        TokenAtOffset::None => None,
    };
    start.is_some_and(|n| n.ancestors().any(|a| a.kind() == SyntaxKind::BLOCK))
}

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
    push_file_items(host, pos.file, pos.offset, items, ItemFilter::Value);
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
    push_file_items(host, pos.file, pos.offset, items, ItemFilter::Type);
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
    } else if detail.contains(": ") && !detail.starts_with("const ") && !detail.starts_with("for ")
    {
        CompletionKind::Param
    } else {
        CompletionKind::Local
    }
}

/// Top-level (and enclosing-module) items of `file` matching `filter`.
///
/// Mirrors the resolver's `resolve_in_file_scope` (crates/analyzer-core/src/
/// resolve.rs): inside a module the innermost enclosing module's siblings are
/// in scope (and — like the resolver's same-file lookup — need *not* be
/// exported), in addition to the file's top-level items.
fn push_file_items(
    host: &mut AnalysisHost,
    file: analyzer_core::FileId,
    offset: analyzer_core::TextSize,
    items: &mut Vec<CompletionItem>,
    filter: ItemFilter,
) {
    let Some(analysis) = host.analyze(file) else {
        return;
    };
    let tree = analysis.item_tree.clone();
    // Innermost enclosing module's siblings (same-file: no `exported` gate).
    if let Some(module_idx) = enclosing_module_index(&tree, offset) {
        for (_, sym) in tree.children_of(module_idx) {
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

/// Index of the innermost `Module` symbol whose full range contains `offset`,
/// or `None` at top level. Replicates the resolver's private `enclosing_module`
/// (modules nest in document order, so the last match is the innermost).
fn enclosing_module_index(
    tree: &analyzer_core::ItemTree,
    offset: analyzer_core::TextSize,
) -> Option<u32> {
    tree.symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.kind == analyzer_core::SymbolKind::Module && s.full_range.contains(offset)
        })
        .map(|(i, _)| i as u32)
        .next_back()
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
                let Some(std_file) = host.stdlib_file() else {
                    continue;
                };
                let Some(analysis) = host.analyze(std_file) else {
                    continue;
                };
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
                let Some(analysis) = host.analyze(file) else {
                    continue;
                };
                let tree = analysis.item_tree.clone();
                if let Some((midx, _)) = tree.top_level().find(|(_, s)| {
                    s.kind == analyzer_core::SymbolKind::Module && s.name == module_name
                }) {
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

fn push_member(
    host: &mut AnalysisHost,
    pos: FilePosition,
    receiver: &SyntaxNode,
    items: &mut Vec<CompletionItem>,
) {
    // One-level-deep receiver typing only (F7 / spec §4): the receiver must be
    // a bare identifier expression (`NAME_EXPR`, e.g. `cnt.⎸` / `Color.⎸`).
    // Anything else — a `CALL_EXPR` (`tbl.lookup(k).⎸`), `INDEX_EXPR`
    // (`arr[i].⎸`), a nested `MEMBER_EXPR`, etc. — needs type inference (v2),
    // so we offer nothing. This gate is load-bearing: without it,
    // `non_trivia_start` would descend into a compound receiver and resolve its
    // *innermost* identifier (`tbl`, a ledger `Map` field), then dump the whole
    // ADT surface after `.lookup(k).` — violating the invariant.
    if receiver.kind() != SyntaxKind::NAME_EXPR {
        return;
    }
    // Resolve the receiver at its first non-trivia token's start offset.
    // `receiver.text_range().start()` is NOT safe here: leading trivia (e.g.
    // whitespace before `cnt` in `NAME_EXPR@45..49 { WHITESPACE " " IDENT
    // "cnt" }`) is a child of the receiver node itself, so the node's raw
    // range start can land on whitespace rather than the identifier —
    // `host.resolve` (via `ident_at_offset`) requires the offset to be at/
    // adjacent to an IDENT token and returns `None` otherwise.
    let recv_pos = FilePosition {
        file: pos.file,
        offset: non_trivia_start(receiver),
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
    let Some(analysis) = host.analyze(file) else {
        return;
    };
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

    #[test]
    fn expr_position_offers_locals_and_items() {
        let ls = labels(
            "circuit helper(): Field { return 1; }\n\
             circuit f(base: Field): Field {\n  const local = 1;\n  return $0\n}",
        );
        assert!(ls.contains(&"base".to_string())); // param
        assert!(ls.contains(&"local".to_string())); // const local
        assert!(ls.contains(&"helper".to_string())); // sibling circuit
        assert!(ls.contains(&"f".to_string())); // self
    }

    #[test]
    fn expr_position_offers_stdlib_when_imported() {
        // full_host-style: register stdlib in a tempdir.
        let (clean, offset) =
            fixture::extract("import CompactStandardLibrary;\ncircuit f(): Field { return $0 }");
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

    #[test]
    fn expr_position_offers_enclosing_module_siblings() {
        // Inside module `M`, `helper` (a non-exported sibling) is in scope and
        // must be offered — mirroring the resolver's `resolve_in_file_scope`.
        // `hidden`, a child of a *non-enclosing* module `N`, must NOT be.
        let ls = labels(
            "module N { circuit hidden(): Field { return 1; } }\n\
             module M {\n\
               circuit helper(): Field { return 1; }\n\
               circuit f(): Field { return $0 }\n\
             }",
        );
        assert!(ls.contains(&"helper".to_string())); // enclosing-module sibling
        assert!(!ls.contains(&"hidden".to_string())); // non-enclosing module's private child
    }

    #[test]
    fn expr_position_dedups_shadowed_locals() {
        // `const x` shadows the `x` param; the label must appear exactly once
        // (nearest/const wins over the param via the `seen` dedup set).
        let ls = labels("circuit f(x: Field): Field { const x = 1; return $0 }");
        assert_eq!(ls.iter().filter(|l| *l == "x").count(), 1);
    }

    #[test]
    fn member_on_ledger_field_offers_adt_methods() {
        let ls = labels("export ledger cnt: Counter;\ncircuit f(): [] { cnt.$0 }");
        assert!(ls.contains(&"increment".to_string()));
        assert!(ls.contains(&"resetToDefault".to_string()));
        // increment's detail carries the probe-confirmed Uint<16> signature.
        // (label-only assert here; detail checked below)
    }

    #[test]
    fn member_on_plain_ledger_field_offers_cell_methods() {
        let ls = labels("export ledger bal: Uint<64>;\ncircuit f(): [] { bal.$0 }");
        assert!(ls.contains(&"read".to_string()));
        assert!(ls.contains(&"write".to_string()));
    }

    #[test]
    fn member_on_enum_offers_variants() {
        let ls = labels("enum Color { red, blue }\ncircuit f(): [] { const c = Color.$0 }");
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

    // CARRY-OVER (Task 3 review): `enclosing_struct_literal` takes the LEFT
    // token on `TokenAtOffset::Between`, unlike the sibling classifiers which
    // prefer a RIGHT IDENT. This exercises the `Between` path directly (comma
    // immediately followed by `}`, no whitespace between them, so the cursor
    // sits exactly on the token boundary rather than inside trivia) to
    // confirm it still classifies correctly and offers the right field.
    // `STRUCT_EXPR`'s children are flat (comma and `}` are both direct
    // children of `STRUCT_EXPR`), so either token's parent resolves to the
    // same `STRUCT_EXPR` — the left-vs-right choice is genuinely inert here.
    #[test]
    fn struct_literal_between_token_boundary_still_classifies() {
        let ls = labels(
            "struct Point { x: Field; y: Field; }\n\
             circuit f(): [] { const p = Point { x: 1,$0} }",
        );
        assert!(ls.contains(&"y".to_string()));
        assert!(!ls.contains(&"x".to_string()));
    }

    #[test]
    fn member_on_chained_receiver_offers_nothing() {
        // F7 / spec §4: one-level-deep receiver typing only. A chained /
        // CALL_EXPR receiver (`tbl.lookup(k).⎸`, where `tbl` is a ledger `Map`
        // field) is NOT a bare identifier — completing it would require type
        // inference (v2). `push_member` must offer nothing here; in particular
        // it must NOT resolve the inner `tbl` and dump the full `Map` surface.
        let ls = labels(
            "export ledger tbl: Map<Field, Field>;\n\
             circuit f(): [] { const k = 1; tbl.lookup(k).$0 }",
        );
        for m in ["lookup", "insert", "member", "isEmpty", "size", "remove"] {
            assert!(
                !ls.contains(&m.to_string()),
                "chained receiver leaked Map method `{m}`: {ls:?}"
            );
        }
    }

    #[test]
    fn ledger_method_detail_uses_probe_confirmed_signature() {
        let (clean, offset) =
            fixture::extract("export ledger cnt: Counter;\ncircuit f(): [] { cnt.$0 }");
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        let inc = completion(&mut host, FilePosition { file, offset })
            .into_iter()
            .find(|c| c.label == "increment")
            .unwrap();
        assert_eq!(
            inc.detail.as_deref(),
            Some("increment(amount: Uint<16>): []")
        );
        assert_eq!(inc.kind, CompletionKind::LedgerMethod);
    }
}
