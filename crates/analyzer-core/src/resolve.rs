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
            SyntaxKind::NAME_EXPR
            | SyntaxKind::CALL_EXPR
            | SyntaxKind::TYPE_REF
            | SyntaxKind::STRUCT_EXPR
            | SyntaxKind::TYPE_SIZE => self.resolve_name_at(pos, &name),
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
        Some(Definition::Item {
            file,
            index: index as u32,
        })
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
                    if in_body
                        && let Some(var) = for_stmt.var_name()
                        && var.text() == name
                    {
                        return Some(Definition::Local {
                            file,
                            name: name.to_string(),
                            name_range: var.text_range(),
                            detail: format!("for {name}"),
                        });
                    }
                }
            }
            SyntaxKind::LAMBDA_EXPR => {
                if let Some(lambda) = compactp_ast::expr::LambdaExpr::cast(node.clone())
                    && let Some(params) = lambda.param_list()
                {
                    // In-body means past the parameter list (covers
                    // both block- and expression-bodied lambdas).
                    if offset >= params.syntax().text_range().end() {
                        // Verified against compactp: unlike circuit/
                        // constructor PARAM_LISTs, a lambda's
                        // PARAM_LIST does not always wrap children in
                        // PARAM nodes (an untyped param like `(x) =>`
                        // is a bare IDENT_PAT), so walk raw children
                        // instead of `ParamList::params()`.
                        for child in params.syntax().children() {
                            if let Some(def) = lambda_param_binding(file, &child, name) {
                                return Some(def);
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
                if let Some(s) = compactp_ast::StructDef::cast(node.clone())
                    && let Some(def) = generic_binding(file, s.generic_params(), name)
                {
                    return Some(def);
                }
            }
            SyntaxKind::TYPE_DECL => {
                if let Some(t) = compactp_ast::TypeDecl::cast(node.clone())
                    && let Some(def) = generic_binding(file, t.generic_params(), name)
                {
                    return Some(def);
                }
            }
            SyntaxKind::MODULE_DEF => {
                if let Some(m) = compactp_ast::ModuleDef::cast(node.clone())
                    && let Some(def) = generic_binding(file, m.generic_params(), name)
                {
                    return Some(def);
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
            let ty = render_ty(param.ty());
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

/// Resolves one raw child of a lambda's `PARAM_LIST` to a binding.
///
/// Verified against compactp: unlike `CircuitDef`/`ConstructorDef` params
/// (always `PARAM(IDENT_PAT, TYPE)`), a lambda parameter list stores an
/// untyped param as a bare `Pat` (no `PARAM` wrapper: `(x) => ...`) and a
/// typed param as `PARAM` wrapping a bare `IDENT` token rather than an
/// `IDENT_PAT` (`(x: Field) => ...`). Both shapes are handled directly
/// since `ParamList::params()` and `Param::pattern()` only cover the
/// circuit/constructor shape.
fn lambda_param_binding(file: FileId, node: &SyntaxNode, name: &str) -> Option<Definition> {
    if let Some(pat) = Pat::cast(node.clone()) {
        for binding in pattern_bindings(&pat) {
            if binding.text() == name {
                return Some(Definition::Local {
                    file,
                    name: name.to_string(),
                    name_range: binding.text_range(),
                    detail: format!("{name}: _"),
                });
            }
        }
        return None;
    }
    let param = compactp_ast::Param::cast(node.clone())?;
    if let Some(pat) = param.pattern() {
        for binding in pattern_bindings(&pat) {
            if binding.text() == name {
                let ty = render_ty(param.ty());
                return Some(Definition::Local {
                    file,
                    name: name.to_string(),
                    name_range: binding.text_range(),
                    detail: format!("{name}: {ty}"),
                });
            }
        }
        return None;
    }
    // No Pat child: the pattern is a bare IDENT token.
    let token = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == SyntaxKind::IDENT)?;
    if token.text() != name {
        return None;
    }
    let ty = render_ty(param.ty());
    Some(Definition::Local {
        file,
        name: name.to_string(),
        name_range: token.text_range(),
        detail: format!("{name}: {ty}"),
    })
}

/// Renders a type node's text, collapsing the leading/interior whitespace
/// trivia that compactp attaches inside type nodes (verified: e.g.
/// `FIELD_TYPE` includes its leading space as a child token).
fn render_ty(ty: Option<compactp_ast::Type>) -> String {
    ty.map(|t| collapse_ws(&t.syntax().text().to_string()))
        .unwrap_or_else(|| "_".to_string())
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
                    let ty = render_ty(param.ty());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    /// Parses a `$0`-marked fixture into a host and returns the position.
    fn position_of(source: &str) -> (crate::AnalysisHost, FilePosition) {
        let (clean, offset) = fixture::extract(source);
        let mut host = crate::AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/test/main.compact"));
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
        assert!(
            resolve_local("circuit f(): Field {\n  const a = b$0;\n  const b = 1;\n  return 0;\n}")
                .is_none()
        );
    }

    #[test]
    fn resolves_for_loop_binding_only_inside_body() {
        let (name, detail) = resolve_local(
            "circuit f(): [] {\n  for (const i of 0..10) {\n    const y = i$0;\n  }\n}",
        )
        .unwrap();
        assert_eq!(name, "i");
        assert_eq!(detail, "for i");
        // Not visible after the loop.
        assert!(
            resolve_local("circuit f(): Field {\n  for (const i of 0..10) { }\n  return i$0;\n}")
                .is_none()
        );
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
        let (name, detail) = resolve_local("struct Box<#n> { v: Vector<n$0, Field>; }").unwrap();
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
