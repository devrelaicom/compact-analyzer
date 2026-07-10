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
const STMT_KEYWORDS: &[&str] = &["const", "return", "if", "else", "for", "assert", "contract"];
const EXPR_KEYWORDS: &[&str] = &[
    "map", "fold", "default", "disclose", "pad", "slice", "true", "false",
];
const TYPE_KEYWORDS: &[&str] = &[
    "Boolean", "Field", "Uint", "Bytes", "Opaque", "Vector", "Unsigned", "Integer",
];

/// Context-aware completion candidates at `pos`. Never panics; returns empty on
/// an unreadable/unclassifiable position.
pub fn completion(host: &mut AnalysisHost, pos: FilePosition) -> Vec<CompletionItem> {
    let Some(analysis) = host.analyze(pos.file) else {
        return Vec::new();
    };
    let root = SyntaxNode::new_root(analysis.green.clone());
    // B1: `pos.offset` may be stale (e.g. a client raced an edit) and land
    // past the document's true end; rowan's `token_at_offset` asserts
    // `offset <= range.end()` and panics otherwise. Clamp once here so every
    // classifier / push_* helper downstream sees an in-range offset,
    // honoring this function's "Never panics" contract.
    let pos = FilePosition {
        file: pos.file,
        offset: pos.offset.min(root.text_range().end()),
    };
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
        Ctx::StructLiteral(se) => push_struct_fields(host, pos, &root, &se, &mut items), // Task 5
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
pub(crate) fn non_trivia_start(node: &SyntaxNode) -> TextSize {
    node.descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|t| !t.kind().is_trivia())
        .map(|t| t.text_range().start())
        .unwrap_or_else(|| node.text_range().start())
}

/// The enclosing STRUCT_EXPR iff the cursor is at a field-name-or-shorthand
/// position: either nothing typed yet (left token is `{` or `,`), or a
/// partial ident has been typed and is NOT in a field-value position after
/// `:`.
///
/// Verified against compactp's CST (`compactp cst`): an un-colon-followed
/// identifier inside a struct literal body parses as a bare `NAME_EXPR`,
/// directly a child of `STRUCT_EXPR` (e.g. `Point { x: 1, y⎸ }`). Once a `:`
/// follows the name, the value expression's parent becomes
/// `STRUCT_FIELD_INIT` instead (e.g. `Point { x: y⎸ }` → `NAME_EXPR` is a
/// child of `STRUCT_FIELD_INIT`, not `STRUCT_EXPR`) — that IS a value
/// position and must NOT classify as struct-literal completion.
fn enclosing_struct_literal(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    if let Some(id) = completion_ident(root, offset)
        && let Some(name_expr) = id.parent()
        && name_expr.kind() == SyntaxKind::NAME_EXPR
        && let Some(se) = name_expr.parent()
        && se.kind() == SyntaxKind::STRUCT_EXPR
    {
        return Some(se);
    }
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
    // C2: ONE `seen` set threaded through locals -> file items -> imports,
    // in precedence order (nearest/first wins), so a name that is both a
    // local and an item (e.g. a param shadowing a top-level circuit) is
    // offered exactly once, under the binding that actually resolves at the
    // cursor (mirrors resolve.rs's `resolve_name_at`: locals first, then
    // `resolve_in_file_scope`).
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
    push_file_items(
        host,
        pos.file,
        pos.offset,
        items,
        ItemFilter::Value,
        &mut seen,
    );
    push_imported_items(host, pos.file, root, items, ItemFilter::Value, &mut seen);
}

fn push_type_items(
    host: &mut AnalysisHost,
    pos: FilePosition,
    root: &SyntaxNode,
    items: &mut Vec<CompletionItem>,
) {
    // Same C2 dedup discipline as `push_scope_and_items`, for type position.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // In-scope generic type params.
    for b in analyzer_core::scope_bindings_at(root, pos.offset) {
        if b.detail.starts_with("generic ") && seen.insert(b.name.clone()) {
            items.push(CompletionItem {
                label: b.name,
                kind: CompletionKind::Generic,
                detail: Some(b.detail),
                documentation: None,
            });
        }
    }
    push_file_items(
        host,
        pos.file,
        pos.offset,
        items,
        ItemFilter::Type,
        &mut seen,
    );
    push_imported_items(host, pos.file, root, items, ItemFilter::Type, &mut seen);
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
    seen: &mut std::collections::HashSet<String>,
) {
    let Some(analysis) = host.analyze(file) else {
        return;
    };
    let tree = analysis.item_tree.clone();
    // Innermost enclosing module's siblings (same-file: no `exported` gate).
    if let Some(module_idx) = enclosing_module_index(&tree, offset) {
        for (_, sym) in tree.children_of(module_idx) {
            if sym.name.is_empty()
                || !item_matches(sym.kind, filter)
                || !seen.insert(sym.name.clone())
            {
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
        if sym.name.is_empty() || !item_matches(sym.kind, filter) || !seen.insert(sym.name.clone())
        {
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

/// Names brought in by imports (stdlib exports; in-scope-module members),
/// honoring each import's prefix / selective specifier list / `as` aliases
/// (C1): completion must offer exactly the labels the resolver would resolve
/// at the cursor through this import (`resolve_through_import` in
/// crates/analyzer-core/src/resolve.rs) — no more (a plain member name under
/// a `prefix` import, an unlisted member under a selective list) and no
/// less (the prefixed / aliased label itself).
fn push_imported_items(
    host: &mut AnalysisHost,
    file: analyzer_core::FileId,
    root: &SyntaxNode,
    items: &mut Vec<CompletionItem>,
    filter: ItemFilter,
    seen: &mut std::collections::HashSet<String>,
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
                let originals: Vec<String> = tree
                    .top_level()
                    .filter(|(_, s)| {
                        s.exported && !s.name.is_empty() && item_matches(s.kind, filter)
                    })
                    .map(|(_, s)| s.name.clone())
                    .collect();
                for (label, original) in import_visible_labels(&import, &originals) {
                    let Some((_, sym)) = tree.top_level().find(|(_, s)| s.name == original) else {
                        continue;
                    };
                    if !seen.insert(label.clone()) {
                        continue;
                    }
                    items.push(CompletionItem {
                        label,
                        kind: CompletionKind::StdlibItem,
                        detail: Some(sym.signature.clone()),
                        documentation: sym.doc.clone(),
                    });
                }
            }
            Some(t) => {
                let module_name = t.text().to_string();
                let Some(analysis) = host.analyze(file) else {
                    continue;
                };
                let tree = analysis.item_tree.clone();
                let Some((midx, _)) = tree.top_level().find(|(_, s)| {
                    s.kind == analyzer_core::SymbolKind::Module && s.name == module_name
                }) else {
                    continue;
                };
                let originals: Vec<String> = tree
                    .children_of(midx)
                    .filter(|(_, s)| {
                        s.exported && !s.name.is_empty() && item_matches(s.kind, filter)
                    })
                    .map(|(_, s)| s.name.clone())
                    .collect();
                for (label, original) in import_visible_labels(&import, &originals) {
                    let Some((_, sym)) = tree.children_of(midx).find(|(_, s)| s.name == original)
                    else {
                        continue;
                    };
                    if !seen.insert(label.clone()) {
                        continue;
                    }
                    items.push(CompletionItem {
                        label,
                        kind: kind_of(sym.kind),
                        detail: Some(sym.signature.clone()),
                        documentation: sym.doc.clone(),
                    });
                }
            }
            None => {} // string-path imports: cross-file member enumeration deferred (see Self-review)
        }
    }
}

/// This import's `(visible_label, original_member_name)` pairs: for each
/// name in `originals` (the exported members available at this import's
/// source), the label a user would have to type to resolve it through this
/// import.
///
/// Mirrors `resolve_through_import`'s effective-name logic (crates/
/// analyzer-core/src/resolve.rs) in reverse — that function goes label ->
/// original (strip the prefix, then look up the selective list); this goes
/// original -> label:
/// - A `prefix` import offers ONLY the prefixed label: the resolver's
///   `name.strip_prefix(prefix)?` means an unprefixed name never resolves
///   through a prefixed import, so completion must not offer the bare name.
/// - A selective specifier list (`import { a, b as c } from M;`) offers
///   ONLY the listed originals, labeled by their `as` alias when present;
///   an unlisted exported member is invisible through this import.
/// - Otherwise, every original member is offered under its own name.
fn import_visible_labels(
    import: &compactp_ast::Import,
    originals: &[String],
) -> Vec<(String, String)> {
    let prefix = import
        .prefix()
        .and_then(|p| p.name())
        .map(|t| t.text().to_string());
    // Selective list + aliases (no typed accessor for the alias: walk
    // descendants — verified compactp pattern, same as the resolver's
    // private `import_specifier_alias`).
    let specifiers: Vec<(String, String)> = import
        .syntax()
        .descendants()
        .filter_map(compactp_ast::ImportSpecifier::cast)
        .filter_map(|spec| {
            let original = spec.name()?.text().to_string();
            let alias = specifier_alias(&spec).unwrap_or_else(|| original.clone());
            Some((alias, original))
        })
        .collect();

    originals
        .iter()
        .filter_map(|original| {
            let post_alias_label = if specifiers.is_empty() {
                original.clone()
            } else {
                specifiers
                    .iter()
                    .find(|(_, orig)| orig == original)?
                    .0
                    .clone()
            };
            let label = match &prefix {
                Some(p) => format!("{p}{post_alias_label}"),
                None => post_alias_label,
            };
            Some((label, original.clone()))
        })
        .collect()
}

/// The `as` alias of an import specifier (no typed accessor: read the IDENT
/// following AS_KW — verified compactp pattern; mirrors the resolver's
/// private `import_specifier_alias` in crates/analyzer-core/src/resolve.rs).
fn specifier_alias(spec: &compactp_ast::ImportSpecifier) -> Option<String> {
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
    root: &SyntaxNode,
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
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Resolve the struct type from its name token position.
    let def = host.resolve(FilePosition {
        file: pos.file,
        offset: name_tok.text_range().start(),
    });
    if let Some(analyzer_core::Definition::Item { file, index }) = def
        && let Some(analysis) = host.analyze(file)
    {
        let tree = analysis.item_tree.clone();
        if tree
            .symbols
            .get(index as usize)
            .is_some_and(|s| s.kind == analyzer_core::SymbolKind::Struct)
        {
            for (_, sym) in tree.children_of(index) {
                if sym.kind == analyzer_core::SymbolKind::StructField
                    && !sym.name.is_empty()
                    && !provided.contains(&sym.name)
                    && seen.insert(sym.name.clone())
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
    }
    // C3: the field-name-or-shorthand position is genuinely ambiguous
    // between a struct-field label and a shorthand-init local reference
    // (`Point { localVar }`) — offer in-scope locals too. Fields win on a
    // name collision (pushed first above).
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

    // ---- C1: prefix / selective-list / alias import semantics ----

    #[test]
    fn prefix_import_offers_prefixed_label_not_bare_name() {
        // Resolver ground truth: `resolve.rs::prefix_import_strips_prefix`
        // resolves `In_double` and explicitly does NOT resolve bare `double`
        // through a `prefix In_` import. Completion must match.
        let ls = labels(
            "module Inner {\n  export circuit double(x: Field): Field { return x + x; }\n}\n\
             import Inner prefix In_;\n\
             circuit main(): Field { return $0 }",
        );
        assert!(ls.contains(&"In_double".to_string()));
        assert!(!ls.contains(&"double".to_string()));
    }

    #[test]
    fn selective_import_offers_only_listed_member() {
        // Resolver ground truth: `resolve.rs::selective_import_with_alias`
        // shows a non-listed member (`triple`) does NOT resolve through
        // `import { double } from Inner;`.
        let ls = labels(
            "module Inner {\n  export circuit double(x: Field): Field { return x + x; }\n  export circuit triple(x: Field): Field { return x + x + x; }\n}\n\
             import { double } from Inner;\n\
             circuit main(): Field { return $0 }",
        );
        assert!(ls.contains(&"double".to_string()));
        assert!(!ls.contains(&"triple".to_string()));
    }

    #[test]
    fn alias_import_offers_alias_not_original() {
        // Resolver ground truth: `resolve.rs::selective_import_with_alias`
        // resolves `twice` (the alias) through `import { double as twice }
        // from Inner;`; the original name `double` is not visible under a
        // selective (aliased) list.
        let ls = labels(
            "module Inner {\n  export circuit double(x: Field): Field { return x + x; }\n}\n\
             import { double as twice } from Inner;\n\
             circuit main(): Field { return $0 }",
        );
        assert!(ls.contains(&"twice".to_string()));
        assert!(!ls.contains(&"double".to_string()));
    }

    #[test]
    fn stdlib_prefix_import_offers_prefixed_label_not_bare_name() {
        // Same C1 fix, exercised through the CompactStandardLibrary arm
        // (distinct code path from the identifier-module arm above).
        let (clean, offset) = fixture::extract(
            "import CompactStandardLibrary prefix Std_;\n\
             circuit main(x: Field): Bytes<32> { return $0 }",
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
        assert!(ls.contains(&"Std_persistentHash".to_string()));
        assert!(!ls.contains(&"persistentHash".to_string()));
    }

    // ---- C2: dedup across locals / file-items / imported-items ----

    #[test]
    fn dedup_shadowed_item_prefers_local_kind() {
        // `helper` is both a top-level circuit AND (inside `main`) a
        // shadowing param. Resolver ground truth:
        // `resolve.rs::locals_shadow_top_level_items` — only the param
        // resolves at the cursor. Before the fix, `push_file_items` appended
        // the circuit unconditionally after `push_scope_and_items`'s
        // locals-only dedup, so `helper` appeared TWICE.
        let (clean, offset) = fixture::extract(
            "circuit helper(): Field { return 1; }\n\
             circuit main(helper: Field): Field { return $0 }",
        );
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        let items = completion(&mut host, FilePosition { file, offset });
        let helpers: Vec<_> = items.iter().filter(|c| c.label == "helper").collect();
        assert_eq!(
            helpers.len(),
            1,
            "expected exactly one `helper`, got {helpers:?}"
        );
        assert_eq!(helpers[0].kind, CompletionKind::Param);
    }

    // ---- C3: struct-field completion after a typed prefix char ----

    #[test]
    fn struct_literal_field_position_after_partial_ident_offers_field_and_locals() {
        // `Point { x: 1, y⎸ }` — the left token at the cursor is the IDENT
        // `y`, not `{`/`,`, so the OLD `enclosing_struct_literal` fell
        // through to Ctx::Expr and offered nothing struct-shaped. Verified
        // against compactp's CST (`compactp cst`): an un-colon-followed
        // ident in a struct literal body parses as a bare `NAME_EXPR`,
        // directly a child of `STRUCT_EXPR` — genuinely ambiguous between a
        // struct-field label and a shorthand-init local reference, so both
        // are offered.
        let ls = labels(
            "struct Point { x: Field; y: Field; }\n\
             circuit f(): [] { const local = 1; const p = Point { x: 1, y$0 }; }",
        );
        assert!(ls.contains(&"y".to_string())); // remaining field
        assert!(ls.contains(&"local".to_string())); // in-scope local (shorthand-ambiguous)
        assert!(!ls.contains(&"x".to_string())); // already provided
    }

    #[test]
    fn struct_literal_value_position_after_colon_behaves_as_expr_context() {
        // `Point { x: ⎸ }` — this IS a value position (`STRUCT_FIELD_INIT`'s
        // value expr), not a field-name-or-shorthand position: the value
        // expr's parent is `STRUCT_FIELD_INIT`, not `STRUCT_EXPR` directly
        // (verified against compactp's CST). Must still behave as an
        // expression context: locals/keywords, not field names.
        let ls = labels(
            "struct Point { x: Field; y: Field; }\n\
             circuit f(): [] { const local = 1; const p = Point { x: $0 }; }",
        );
        assert!(ls.contains(&"local".to_string()));
        assert!(ls.contains(&"return".to_string())); // Ctx::Expr statement keyword
        assert!(!ls.contains(&"y".to_string())); // NOT struct-field completion
    }

    // ---- C4 / C5: keyword-list fidelity ----

    #[test]
    fn statement_position_offers_contract_keyword() {
        // F5 + compactp `grammar/statements.rs`: `CONTRACT_KW =>
        // super::declarations::declaration(p)` — a nested `contract Name {
        // ... }` is dispatched from statement position (composable
        // contracts), so `contract` is a statement-position keyword.
        let ls = labels("circuit f(): Field {\n  $0\n}");
        assert!(ls.contains(&"contract".to_string()));
    }

    #[test]
    fn type_position_offers_unsigned_integer_keywords() {
        // F1 grammar/types.rs: `Unsigned Integer<...>` is spelled with two
        // keyword tokens, UNSIGNED_KW and INTEGER_KW.
        let ls = labels("circuit f(x: $0): Field { return 0; }");
        assert!(ls.contains(&"Unsigned".to_string()));
        assert!(ls.contains(&"Integer".to_string()));
    }

    // ---- B1: never-die offset clamp ----

    #[test]
    fn completion_at_offset_past_eof_returns_empty_and_does_not_panic() {
        // `pos.offset` can be stale (e.g. a client raced an edit) and land
        // past the document's true end. rowan's `token_at_offset`
        // `assert!`s `offset <= range.end()` and panics otherwise —
        // contradicting `completion`'s own "Never panics" doc comment.
        //
        // This fixture ends right after an unresolvable receiver's `.`
        // (`mystery.`) — verified against compactp's CST: this is
        // `MEMBER_EXPR { NAME_EXPR("mystery"), DOT }`, a position that
        // legitimately resolves to `Ctx::Member` with zero completions
        // (the receiver `mystery` doesn't resolve to anything). Once the
        // way-past-EOF offset is clamped to the document's true end, it
        // lands exactly here and returns an empty Vec instead of panicking.
        let source = "circuit f(): [] { mystery.";
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let past_eof = FilePosition {
            file,
            offset: TextSize::from(9_999),
        };
        let items = completion(&mut host, past_eof);
        assert!(
            items.is_empty(),
            "expected empty completions, got {items:?}"
        );
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
