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
