//! Syntactic name resolution. No type inference: a token is classified by
//! its parent CST kind (there is no reference node in the Compact CST —
//! verified against compactp), then resolved through lexical scopes
//! (this task) and file scope / imports (Task 4).

use std::sync::Arc;

use compactp_ast::{AstNode, Pat};
use compactp_diagnostics::Diagnostic;
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

/// The token to anchor the scope walk on: the IDENT at the cursor when present,
/// otherwise the token immediately left of the cursor (so completion works at
/// non-IDENT positions such as `const x = ⎸`).
///
/// This differs from `ident_at_offset`: `ident_at_offset` returns `None`
/// unless an IDENT is at/adjacent to `offset`, whereas `anchor_token` always
/// returns *some* token, falling back to the left neighbor at a non-IDENT
/// boundary. The two coincide only on the resolver's own calls
/// (`resolve_local_name`, reached from `resolve()` via `resolve_name_at`):
/// `resolve()` has already anchored on an IDENT token via `ident_at_offset`
/// before `offset` reaches here, so whichever token `ident_at_offset` picked
/// — the boundary's `r` if it was an IDENT, else `l` — is exactly the token
/// `anchor_token`'s own `r.kind() == IDENT` check finds too. Getting this
/// right matters: for a reference immediately followed by a delimiter with
/// no intervening whitespace (e.g. the `x` in `(x: Field) => x, xs`, where
/// `x` abuts `,`), a bare right-biased-then-left-biased pick that ignored
/// IDENT-ness would choose the delimiter token, whose parent is the *outer*
/// node (e.g. `MAP_EXPR`), skipping straight past the enclosing `LAMBDA_EXPR`
/// in the ancestor chain and reporting the lambda param unresolved. Anchoring
/// on the IDENT's own parent instead always starts the walk inside the
/// innermost expression, so the enclosing scope (lambda body, block, etc.)
/// is never skipped.
fn anchor_token(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    // rowan's `token_at_offset` asserts `offset <= root.text_range().end()`
    // and PANICS otherwise (B1-core): a stale document version or a
    // position past EOF can hand this a way-out-of-range offset. Clamping to
    // `end()` is safe (that boundary is already handled by `TokenAtOffset`)
    // and preserves the never-die contract for every caller reached through
    // `scope_bindings_at` (`resolve`, `resolve_local_name`, completion).
    let offset = offset.min(root.text_range().end());
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

impl crate::AnalysisHost {
    /// Resolves the identifier at `pos` to its definition. Thin bridge over the
    /// tracked dispatcher `crate::db::resolve_query`: ensure the file and its
    /// transitive includes are indexed (so the query's cross-file inputs are
    /// materialized), provision the salsa inputs, and hand off. All resolution
    /// logic lives in `db.rs` now.
    pub fn resolve(&mut self, pos: FilePosition) -> Option<Definition> {
        self.ensure_indexed(pos.file);
        let src = self.src_of(pos.file)?;
        let fd = self.file_deps(pos.file);
        let ws = self.workspace();
        crate::db::resolve_query(self.db_ref(), pos.file, src, fd, ws, pos.offset)
    }

    /// Resolves `name` as if referenced at `pos`: locals first (nearest scope
    /// wins), then file scope + imports. Public bridge over the tracked
    /// `crate::db::resolve_name_query`, used by rename's per-use-site conflict
    /// check (which asks whether some *other* name already resolves at a use
    /// site).
    pub fn resolve_name_at(&mut self, pos: FilePosition, name: &str) -> Option<Definition> {
        self.ensure_indexed(pos.file);
        let src = self.src_of(pos.file)?;
        let fd = self.file_deps(pos.file);
        let ws = self.workspace();
        crate::db::resolve_name_query(
            self.db_ref(),
            pos.file,
            src,
            fd,
            ws,
            pos.offset,
            name.to_string(),
        )
    }

    /// The definition's name (for reference search and rename).
    pub fn def_name(&mut self, def: &Definition) -> Option<String> {
        match def {
            Definition::Item { file, index } => {
                let src = self.src_of(*file)?;
                Some(
                    crate::db::item_tree(self.db_ref(), src)
                        .symbols
                        .get(*index as usize)?
                        .name
                        .clone(),
                )
            }
            Definition::Local { name, .. } => Some(name.clone()),
        }
    }

    /// (file, name_range, full_range) for navigation.
    pub fn nav_info(&mut self, def: &Definition) -> Option<(FileId, TextRange, TextRange)> {
        match def {
            Definition::Item { file, index } => {
                let src = self.src_of(*file)?;
                let sym = crate::db::item_tree(self.db_ref(), src)
                    .symbols
                    .get(*index as usize)?
                    .clone();
                Some((*file, sym.name_range, sym.full_range))
            }
            Definition::Local {
                file, name_range, ..
            } => Some((*file, *name_range, *name_range)),
        }
    }

    /// Error diagnostics for imports/includes in `file` that fail resolution.
    /// A cross-file derived fact — recomputed on demand, not cached with the
    /// per-file parse. Thin bridge over the tracked
    /// `crate::db::resolution_diagnostics_query`, mirroring `resolve()`'s
    /// bridge (Task 7): index this file on demand so its dependency edges are
    /// materialized, read the persisted `FileDeps` input (memoized — no
    /// per-call salsa input), precompute the impure "searched:" display
    /// strings, and hand off. All diagnostic logic lives in `db.rs` now.
    pub fn resolution_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        self.ensure_indexed(file);
        let fd = self.file_deps(file);
        let from_dir = self
            .vfs()
            .path(file)
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let search = self
            .import_search_path()
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        crate::db::resolution_diagnostics_query(
            self.db_ref(),
            src,
            fd,
            Arc::from(from_dir.as_str()),
            Arc::from(search.as_str()),
        )
        .to_vec()
    }
}

/// The sole top-level symbol iff it is exactly one and a module. Mirrors the
/// compiler's "must contain a (single) module definition" rule.
pub(crate) fn single_top_level_module(tree: &crate::ItemTree) -> Option<(u32, &crate::Symbol)> {
    let mut tops = tree.top_level();
    let first = tops.next()?;
    if tops.next().is_some() {
        return None; // more than one top-level declaration
    }
    (first.1.kind == crate::SymbolKind::Module).then_some(first)
}

/// Index of the module symbol whose range contains `offset`, if any.
pub(crate) fn enclosing_module(tree: &crate::ItemTree, offset: TextSize) -> Option<u32> {
    tree.symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == crate::SymbolKind::Module && s.full_range.contains(offset))
        .map(|(i, _)| i as u32)
        .next_back() // innermost (modules nest in document order)
}

/// The `as` alias of an import specifier (no typed accessor: read the IDENT
/// following AS_KW — verified compactp pattern).
pub(crate) fn import_specifier_alias(spec: &compactp_ast::ImportSpecifier) -> Option<String> {
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

/// Finds the IDENT token at/beside `offset`. Between two tokens, prefers
/// the identifier.
pub(crate) fn ident_at_offset(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    // Same B1-core guard as `anchor_token`: this is `resolve()`'s own
    // `token_at_offset` call, reached directly from `hover`/`goto_definition`
    // /`rename`/`references` with a caller-supplied offset that may be past
    // EOF (stale document version, etc). Clamp rather than panic.
    let offset = offset.min(root.text_range().end());
    let pick = |t: SyntaxToken| (t.kind() == SyntaxKind::IDENT).then_some(t);
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => pick(t),
        TokenAtOffset::Between(l, r) => pick(r).or_else(|| pick(l)),
    }
}

pub(crate) fn generic_definition(
    file: FileId,
    param_node: &SyntaxNode,
    token: &SyntaxToken,
) -> Definition {
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

pub(crate) fn local_from_pattern_site(file: FileId, token: &SyntaxToken) -> Option<Definition> {
    Some(Definition::Local {
        file,
        name: token.text().to_string(),
        name_range: token.text_range(),
        detail: detail_for_binding(token),
    })
}

/// Walks outward from `offset` collecting the nearest binding of `name`, via
/// `scope_bindings_at` (anchored on `anchor_token` — see its doc comment for
/// why that token-pick is safe for the resolver's own calls even though it
/// differs from `ident_at_offset` in general).
pub(crate) fn resolve_local_name(
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
            SyntaxKind::MULTI_CONST_STMT => return format!("const {name}"),
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
    fn typed_lambda_param_resolves() {
        // Corpus-proven form: `map((x: Field) => <body>, xs)`. Cursor on the
        // body use of the typed param. Unlike the untyped case above, a
        // typed lambda param parses as `PARAM(IDENT, TYPE)` with no
        // `IDENT_PAT` wrapper (verified against compactp's
        // `pattern_or_parg`), exercising `lambda_param_binding`'s bare-IDENT
        // fallback branch — previously untested.
        let (name, _) =
            resolve_local("circuit c(): Field { return map((x: Field) => x$0, xs); }").unwrap();
        assert_eq!(name, "x");
    }

    #[test]
    fn struct_pattern_binding_resolves_from_binding_site() {
        // Corpus-proven form: `const { a, b } = <expr>;`. Cursor on the `a`
        // binding in the pattern itself: the token's parent is
        // `STRUCT_PAT_FIELD` directly (shorthand field, no `: pattern`),
        // hitting `resolve()`'s definition-site early return.
        let (name, _) =
            resolve_local("circuit c(pt: Field): Field { const { a$0, b } = pt; return a + b; }")
                .unwrap();
        assert_eq!(name, "a");
    }

    #[test]
    fn struct_pattern_binding_resolves_from_use_site() {
        // Same binding, resolved from a use site: cursor on `a` in the
        // `return a + b;` expression, exercising `resolve_local_name`'s
        // `Pat::Struct` traversal in `collect_pattern_bindings`.
        let (name, _) =
            resolve_local("circuit c(pt: Field): Field { const { a, b } = pt; return a$0 + b; }")
                .unwrap();
        assert_eq!(name, "a");
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

    #[test]
    fn multi_const_in_for_body_detail_is_const_not_for() {
        // Regression: a MULTI_CONST_STMT binding whose block is a `for`-loop
        // body has ancestor chain IDENT_PAT -> MULTI_CONST_STMT -> BLOCK ->
        // FOR_STMT. `detail_for_binding` walks that chain; without an explicit
        // MULTI_CONST_STMT arm it falls through CONST_STMT and lands on the
        // FOR_STMT arm, yielding "for b" instead of "const b". This path is
        // reachable only because MULTI_CONST_STMT bindings now resolve.
        let (name, detail) = resolve_local(
            "circuit f(): [] {\n  for (const i of 0..10) {\n    const a = 1, b = 2;\n    return b$0;\n  }\n}",
        )
        .unwrap();
        assert_eq!(name, "b");
        assert_eq!(detail, "const b");
    }

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

    #[test]
    fn host_resolve_bridges_to_tracked_query() {
        // Smoke test: the host `resolve()` bridge provisions inputs and reaches
        // the tracked `crate::db::resolve_query`, resolving a top-level call to
        // its circuit item (no stdlib needed).
        let (mut host, pos) =
            position_of("circuit helper(): [] {}\ncircuit main(): [] { hel$0per(); }");
        let def = host.resolve(pos).unwrap();
        assert!(matches!(def, Definition::Item { .. }));
    }

    // ---- Task 4: file scope + imports ----

    /// Fixture: verified to parse with zero errors (see plan Global
    /// Constraints). Registers the stdlib stub in a tempdir.
    fn full_host(source: &str) -> (crate::AnalysisHost, FilePosition, tempfile::TempDir) {
        let (clean, offset) = fixture::extract(source);
        let dir = tempfile::tempdir().unwrap();
        let std_path = crate::stdlib::materialize(dir.path()).unwrap();
        let mut host = crate::AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/test/main.compact"));
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
        let (_, name, kind) =
            resolve_item("struct Point { x: Field; }\ncircuit f(p: Po$0int): Field { return 0; }")
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
    fn bare_member_access_on_non_enum_item_resolves_to_none() {
        // Unlike `ledger_adt_methods_resolve_to_none` above (whose trailing
        // call parens make the parser produce a CALL_EXPR, which routes
        // through `resolve_name_at` rather than `resolve_member`), this
        // fixture has NO call parens: `count.foo` parses to a MEMBER_EXPR
        // whose NAME_EXPR receiver `count` resolves to the `Ledger` item.
        // This exercises `resolve_member`'s enum-kind guard directly
        // (verified against compactp's CST: `compactp cst` on this fixture
        // shows `MEMBER_EXPR` wrapping `NAME_EXPR("count")`, `DOT`,
        // `IDENT("foo")`, with zero parse diagnostics).
        let (mut host, pos, _dir) = full_host(
            "export ledger count: Counter;\n\
             circuit f(): [] {\n  const c = count.fo$0o;\n}",
        );
        assert_eq!(host.resolve(pos), None);
    }

    #[test]
    fn string_path_import_with_missing_file_is_unresolved() {
        // `full_host` never writes `./utils` to disk, so this resolves to
        // None because the import target file doesn't exist — NOT because
        // string-path imports are unsupported (M2b resolves them like any
        // other cross-file import; see the M2b tests below).
        let (mut host, pos, _dir) = full_host(
            "import \"./utils\" prefix U_;\n\
             circuit main(): Field { return U_help$0er(); }",
        );
        assert_eq!(host.resolve(pos), None);
    }

    // ---- M2b: cross-file filesystem resolution ----

    /// Writes `others` (relative path, contents) to a tempdir, writes the
    /// `$0`-marked `open` file (extension implied by `open_rel`) there too as
    /// an overlay, and returns (host, tempdir-guard, cursor position).
    fn cross_file(
        marked_open: &str,
        open_rel: &str,
        others: &[(&str, &str)],
    ) -> (crate::AnalysisHost, tempfile::TempDir, FilePosition) {
        let (clean, offset) = crate::fixture::extract(marked_open);
        let dir = tempfile::tempdir().unwrap();
        for (rel, contents) in others {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, contents).unwrap();
        }
        let open_path = dir.path().join(open_rel);
        std::fs::create_dir_all(open_path.parent().unwrap()).unwrap();
        std::fs::write(&open_path, &clean).unwrap();
        let mut host = crate::AnalysisHost::new();
        let file = host.vfs_mut().file_id(&open_path);
        host.vfs_mut().set_overlay(file, clean, 1);
        (host, dir, FilePosition { file, offset })
    }

    #[test]
    fn identifier_import_resolves_across_files() {
        let (mut host, _dir, pos) = cross_file(
            "import Foo;\ncircuit m(): Field { return f$0(); }",
            "main.compact",
            &[(
                "Foo.compact",
                "module Foo { export circuit f(): Field { return 0; } }",
            )],
        );
        let pos_file = pos.file;
        let def = host
            .resolve(pos)
            .expect("resolves through identifier import");
        assert_eq!(host.def_name(&def).unwrap(), "f");
        assert_ne!(host.nav_info(&def).unwrap().0, pos_file); // lands in Foo.compact
    }

    #[test]
    fn string_path_import_resolves_across_files() {
        let (mut host, _dir, pos) = cross_file(
            "import \"sub/Bar\";\ncircuit m(): Field { return g$0(); }",
            "main.compact",
            &[(
                "sub/Bar.compact",
                "module Bar { export circuit g(): Field { return 0; } }",
            )],
        );
        let pos_file = pos.file;
        let def = host
            .resolve(pos)
            .expect("resolves through string-path import");
        assert_eq!(host.def_name(&def).unwrap(), "g");
        assert_ne!(host.nav_info(&def).unwrap().0, pos_file);
    }

    #[test]
    fn import_with_wrong_module_name_is_unresolved() {
        let (mut host, _dir, pos) = cross_file(
            "import Foo;\ncircuit m(): Field { return f$0(); }",
            "main.compact",
            &[(
                "Foo.compact",
                "module NotFoo { export circuit f(): Field { return 0; } }",
            )],
        );
        assert!(host.resolve(pos).is_none());
    }

    #[test]
    fn missing_import_file_is_unresolved() {
        let (mut host, _dir, pos) = cross_file(
            "import Missing;\ncircuit m(): Field { return f$0(); }",
            "main.compact",
            &[],
        );
        assert!(host.resolve(pos).is_none());
    }

    #[test]
    fn include_makes_top_level_decls_visible() {
        // `helper` is NOT exported in the included file — splice is raw.
        let (mut host, _dir, pos) = cross_file(
            "include \"inc\";\ncircuit m(): Field { return help$0er(); }",
            "main.compact",
            &[("inc.compact", "circuit helper(): Field { return 0; }")],
        );
        let pos_file = pos.file;
        let def = host.resolve(pos).expect("resolves into included file");
        assert_eq!(host.def_name(&def).unwrap(), "helper");
        assert_ne!(host.nav_info(&def).unwrap().0, pos_file);
    }

    #[test]
    fn transitive_includes_resolve() {
        let (mut host, _dir, pos) = cross_file(
            "include \"a\";\ncircuit m(): Field { return de$0ep(); }",
            "main.compact",
            &[
                ("a.compact", "include \"b\";"),
                ("b.compact", "circuit deep(): Field { return 0; }"),
            ],
        );
        let pos_file = pos.file;
        let def = host
            .resolve(pos)
            .expect("resolves through transitive include");
        assert_eq!(host.def_name(&def).unwrap(), "deep");
        assert_ne!(host.nav_info(&def).unwrap().0, pos_file);
    }

    #[test]
    fn include_cycle_terminates() {
        let (mut host, _dir, pos) = cross_file(
            "include \"a\";\ncircuit m(): Field { return miss$0ing(); }",
            "main.compact",
            &[
                ("a.compact", "include \"b\";"),
                ("b.compact", "include \"a\";"), // a <-> b cycle
            ],
        );
        // Must terminate (no hang, no panic) and resolve to None.
        assert!(host.resolve(pos).is_none());
    }

    // ---- Task 6: unresolvable-import/include diagnostics ----

    fn diag_host(
        files: &[(&str, &str)],
        open_rel: &str,
    ) -> (crate::AnalysisHost, tempfile::TempDir, FileId) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, contents) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, contents).unwrap();
        }
        let mut host = crate::AnalysisHost::new();
        let file = host.vfs_mut().file_id(&dir.path().join(open_rel));
        (host, dir, file)
    }

    #[test]
    fn unresolved_import_is_an_error() {
        let (mut host, _dir, file) = diag_host(
            &[(
                "main.compact",
                "import Missing;\ncircuit m(): Field { return 0; }",
            )],
            "main.compact",
        );
        let diags = host.resolution_diagnostics(file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, crate::Severity::Error);
        assert!(diags[0].message.contains("cannot resolve import"));
        assert!(diags[0].message.contains("Missing"));
    }

    #[test]
    fn resolvable_import_has_no_diagnostic() {
        let (mut host, _dir, file) = diag_host(
            &[
                (
                    "Foo.compact",
                    "module Foo { export circuit f(): Field { return 0; } }",
                ),
                (
                    "main.compact",
                    "import Foo;\ncircuit m(): Field { return 0; }",
                ),
            ],
            "main.compact",
        );
        assert!(host.resolution_diagnostics(file).is_empty());
    }

    #[test]
    fn module_name_mismatch_is_an_error() {
        let (mut host, _dir, file) = diag_host(
            &[
                (
                    "Foo.compact",
                    "module NotFoo { export circuit f(): Field { return 0; } }",
                ),
                (
                    "main.compact",
                    "import Foo;\ncircuit m(): Field { return 0; }",
                ),
            ],
            "main.compact",
        );
        let diags = host.resolution_diagnostics(file);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("rather than expected"));
    }

    #[test]
    fn unresolved_include_is_an_error() {
        let (mut host, _dir, file) = diag_host(
            &[(
                "main.compact",
                "include \"nope\";\ncircuit m(): Field { return 0; }",
            )],
            "main.compact",
        );
        let diags = host.resolution_diagnostics(file);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("cannot resolve include"));
    }

    // ---- Task 9: tracked resolution_diagnostics ----

    #[test]
    fn resolution_diagnostics_report_unresolved_import() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.compact");
        std::fs::write(&main, "import \"DoesNotExist\";").unwrap();
        let mut host = crate::AnalysisHost::new();
        let id = host.vfs_mut().file_id(&main);
        host.index_file(id);
        let diags = host.resolution_diagnostics(id);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.prefix, "E");
        assert_eq!(diags[0].code.number, 9001);
    }

    /// Byte-identical "searched: {list}" wording: the original imperative
    /// formatting rendered `from_dir` followed by every extra search-path
    /// entry, joined with `", "`. The tracked query
    /// only ever sees the host-precomputed `from_dir_display`/`search_display`
    /// strings, so this locks in that the two combine back into exactly that
    /// format (not, say, a trailing separator when the search path is
    /// non-empty, or a stray one when it's empty).
    #[test]
    fn unresolved_import_searched_list_matches_original_format() {
        let (mut host, dir, file) = diag_host(
            &[(
                "main.compact",
                "import Missing;\ncircuit m(): Field { return 0; }",
            )],
            "main.compact",
        );
        let extra = dir.path().join("libs");
        host.set_import_search_path(vec![extra.clone()]);
        let diags = host.resolution_diagnostics(file);
        assert_eq!(diags.len(), 1);
        let expected = format!(
            "cannot resolve import `Missing` (searched: {}, {})",
            dir.path().display(),
            extra.display()
        );
        assert_eq!(diags[0].message, expected);
    }
}
