//! Syntactic name resolution. No type inference: a token is classified by
//! its parent CST kind (there is no reference node in the Compact CST —
//! verified against compactp), then resolved through lexical scopes
//! (this task) and file scope / imports (Task 4).

use std::path::{Path, PathBuf};

use compactp_ast::{AstNode, Pat};
use compactp_diagnostics::{Diagnostic, DiagnosticCode};
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

    /// File scope: enclosing-module members, then top-level items, then
    /// imports in declaration order (stdlib and in-file modules only —
    /// filesystem imports are M2b).
    fn resolve_in_file_scope(&mut self, pos: FilePosition, name: &str) -> Option<Definition> {
        let analysis = self.analyze(pos.file)?;
        let tree = analysis.item_tree.clone();
        let root = SyntaxNode::new_root(analysis.green.clone());

        // Inside a module, siblings are visible first.
        if let Some(module_idx) = enclosing_module(&tree, pos.offset)
            && let Some((idx, _)) = tree.children_of(module_idx).find(|(_, s)| s.name == name)
        {
            return Some(Definition::Item {
                file: pos.file,
                index: idx,
            });
        }

        // Top-level items.
        if let Some((idx, _)) = tree.top_level().find(|(_, s)| s.name == name) {
            return Some(Definition::Item {
                file: pos.file,
                index: idx,
            });
        }

        // Included files' top-level declarations are spliced into this scope.
        if let Some(def) = self.resolve_through_includes(pos.file, &root, name, &mut vec![pos.file])
        {
            return Some(def);
        }

        // Imports, in order.
        let source_file = compactp_ast::SourceFile::cast(root)?;
        for import in source_file.imports() {
            if let Some(def) = self.resolve_through_import(pos.file, &tree, &import, name) {
                return Some(def);
            }
        }
        None
    }

    /// Resolves `name` against the top-level declarations of files reachable by
    /// `include` from `root` (the syntax tree of `file`), depth-first. `active`
    /// is the include path currently being expanded, for cycle detection.
    fn resolve_through_includes(
        &mut self,
        file: FileId,
        root: &SyntaxNode,
        name: &str,
        active: &mut Vec<FileId>,
    ) -> Option<Definition> {
        let source_file = compactp_ast::SourceFile::cast(root.clone())?;
        let from_dir = self.vfs().path(file).parent()?.to_path_buf();
        let search = self.import_search_path();
        // Collect target FileIds first (avoids a borrow across the loop body).
        let mut targets = Vec::new();
        for inc in source_file.includes() {
            if let Some(tok) = inc.path() {
                let raw = crate::string_lit_text(tok.text());
                if let Some(path) = crate::find_source_pathname(&from_dir, &search, &raw) {
                    targets.push(self.vfs_mut().file_id(&path));
                }
            }
        }
        for target in targets {
            if active.contains(&target) {
                continue; // cycle along the active include path
            }
            let Some(analysis) = self.analyze(target) else {
                continue; // unreadable include: skip, never die
            };
            if let Some((idx, _)) = analysis.item_tree.top_level().find(|(_, s)| s.name == name) {
                return Some(Definition::Item {
                    file: target,
                    index: idx,
                });
            }
            let target_root = SyntaxNode::new_root(analysis.green.clone());
            active.push(target);
            let found = self.resolve_through_includes(target, &target_root, name, active);
            active.pop();
            if found.is_some() {
                return found;
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
                Some(Definition::Item {
                    file: std_file,
                    index: idx,
                })
            }
            Some(module_token) => {
                let module_name = module_token.text().to_string();
                // In-scope module first (verified: in-scope FIRST, filesystem
                // second). An in-scope module wins even if the member is absent.
                if let Some((module_idx, _)) = tree
                    .top_level()
                    .find(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == module_name)
                {
                    return tree
                        .children_of(module_idx)
                        .find(|(_, s)| s.exported && s.name == target_name)
                        .map(|(idx, _)| Definition::Item { file, index: idx });
                }
                // No in-scope module → filesystem: `<module_name>.compact`
                // containing exactly `module <module_name>`.
                let from_dir = self.vfs().path(file).parent()?.to_path_buf();
                self.resolve_external_module_member(
                    &from_dir,
                    &module_name,
                    &module_name,
                    &target_name,
                )
            }
            // String-path import: never consults in-scope modules; expected
            // module name is the last path component.
            None => {
                let raw = crate::string_lit_text(import.path()?.text());
                let expected = crate::path_module_name(&raw)?;
                let from_dir = self.vfs().path(file).parent()?.to_path_buf();
                self.resolve_external_module_member(&from_dir, &raw, &expected, &target_name)
            }
        }
    }

    /// Resolves `target_name` as an exported member of the single module in the
    /// file that `raw` resolves to, requiring that module's name to equal
    /// `expected_module` (compiler rule). Used by both the identifier and
    /// string-path import arms.
    fn resolve_external_module_member(
        &mut self,
        from_dir: &std::path::Path,
        raw: &str,
        expected_module: &str,
        target_name: &str,
    ) -> Option<Definition> {
        let search = self.import_search_path();
        let path = crate::find_source_pathname(from_dir, &search, raw)?;
        let file = self.vfs_mut().file_id(&path);
        let tree = self.analyze(file)?.item_tree.clone();
        let (module_idx, module_sym) = single_top_level_module(&tree)?;
        if module_sym.name != expected_module {
            return None;
        }
        let (idx, _) = tree
            .children_of(module_idx)
            .find(|(_, s)| s.exported && s.name == target_name)?;
        Some(Definition::Item { file, index: idx })
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
            FilePosition {
                file: pos.file,
                offset: struct_expr.syntax().text_range().start(),
            },
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
            FilePosition {
                file: pos.file,
                offset: receiver.syntax().text_range().start(),
            },
            &receiver_name,
        )?;
        let Definition::Item { file, index } = &receiver_def else {
            return None;
        };
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
                let (idx, _) = std_tree
                    .top_level()
                    .find(|(_, s)| s.exported && s.name == name)?;
                Some(Definition::Item {
                    file: std_file,
                    index: idx,
                })
            }
            Some(t) => {
                let (module_idx, _) = tree
                    .top_level()
                    .find(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == t.text())?;
                let (idx, _) = tree
                    .children_of(module_idx)
                    .find(|(_, s)| s.exported && s.name == name)?;
                Some(Definition::Item {
                    file: pos.file,
                    index: idx,
                })
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
        let Definition::Item { file, index } = parent_def else {
            return None;
        };
        let tree = self.analyze(*file)?.item_tree.clone();
        let (idx, _) = tree
            .children_of(*index)
            .find(|(_, s)| s.kind == kind && s.name == name)?;
        Some(Definition::Item {
            file: *file,
            index: idx,
        })
    }

    /// The definition's name (for reference search and rename).
    pub fn def_name(&mut self, def: &Definition) -> Option<String> {
        match def {
            Definition::Item { file, index } => Some(
                self.analyze(*file)?
                    .item_tree
                    .symbols
                    .get(*index as usize)?
                    .name
                    .clone(),
            ),
            Definition::Local { name, .. } => Some(name.clone()),
        }
    }

    /// (file, name_range, full_range) for navigation.
    pub fn nav_info(&mut self, def: &Definition) -> Option<(FileId, TextRange, TextRange)> {
        match def {
            Definition::Item { file, index } => {
                let sym = self
                    .analyze(*file)?
                    .item_tree
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

    /// Error diagnostics for imports/includes in `file` that fail resolution.
    /// A cross-file derived fact — recomputed on demand, not cached with the
    /// per-file parse.
    pub fn resolution_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic> {
        let Some(analysis) = self.analyze(file) else {
            return Vec::new();
        };
        let tree = analysis.item_tree.clone();
        let root = SyntaxNode::new_root(analysis.green.clone());
        let Some(sf) = compactp_ast::SourceFile::cast(root) else {
            return Vec::new();
        };
        let Some(from_dir) = self.vfs().path(file).parent().map(Path::to_path_buf) else {
            return Vec::new();
        };
        let search = self.import_search_path();
        let mut diags = Vec::new();
        for import in sf.imports() {
            if let Some(name) = import.name() {
                let nm = name.text().to_string();
                if nm == "CompactStandardLibrary" {
                    continue;
                }
                if tree
                    .top_level()
                    .any(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == nm)
                {
                    continue; // satisfied by an in-scope module
                }
                match crate::find_source_pathname(&from_dir, &search, &nm) {
                    None => diags.push(unresolved_import_diag(
                        &nm,
                        &from_dir,
                        &search,
                        name.text_range(),
                    )),
                    Some(path) => {
                        let target = self.vfs_mut().file_id(&path);
                        if let Some(d) = self.module_mismatch_diag(target, &nm, name.text_range()) {
                            diags.push(d);
                        }
                    }
                }
            } else if let Some(ptok) = import.path() {
                let raw = crate::string_lit_text(ptok.text());
                let span = ptok.text_range();
                let Some(expected) = crate::path_module_name(&raw) else {
                    continue;
                };
                match crate::find_source_pathname(&from_dir, &search, &raw) {
                    None => diags.push(unresolved_import_diag(&raw, &from_dir, &search, span)),
                    Some(path) => {
                        let target = self.vfs_mut().file_id(&path);
                        if let Some(d) = self.module_mismatch_diag(target, &expected, span) {
                            diags.push(d);
                        }
                    }
                }
            }
        }
        for inc in sf.includes() {
            if let Some(tok) = inc.path() {
                let raw = crate::string_lit_text(tok.text());
                if crate::find_source_pathname(&from_dir, &search, &raw).is_none() {
                    diags.push(unresolved_include_diag(
                        &raw,
                        &from_dir,
                        &search,
                        tok.text_range(),
                    ));
                }
            }
        }
        diags
    }

    /// Error iff `target`'s single module does not match `expected`.
    fn module_mismatch_diag(
        &mut self,
        target: FileId,
        expected: &str,
        span: TextRange,
    ) -> Option<Diagnostic> {
        let tree = self.analyze(target)?.item_tree.clone();
        match single_top_level_module(&tree) {
            None => Some(Diagnostic::error(
                DiagnosticCode::new("E", 9003),
                format!("imported file does not contain a single `module {expected}`"),
                span,
            )),
            Some((_, sym)) if sym.name != expected => Some(Diagnostic::error(
                DiagnosticCode::new("E", 9002),
                format!(
                    "imported file defines module `{}` rather than expected `{expected}`",
                    sym.name
                ),
                span,
            )),
            Some(_) => None,
        }
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

fn searched_list(from_dir: &Path, search: &[PathBuf]) -> String {
    let mut parts = vec![from_dir.display().to_string()];
    parts.extend(search.iter().map(|p| p.display().to_string()));
    parts.join(", ")
}

fn unresolved_import_diag(
    raw: &str,
    from_dir: &Path,
    search: &[PathBuf],
    span: TextRange,
) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 9001),
        format!(
            "cannot resolve import `{raw}` (searched: {})",
            searched_list(from_dir, search)
        ),
        span,
    )
}

fn unresolved_include_diag(
    raw: &str,
    from_dir: &Path,
    search: &[PathBuf],
    span: TextRange,
) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 9004),
        format!(
            "cannot resolve include `{raw}` (searched: {})",
            searched_list(from_dir, search)
        ),
        span,
    )
}

/// Index of the module symbol whose range contains `offset`, if any.
fn enclosing_module(tree: &crate::ItemTree, offset: TextSize) -> Option<u32> {
    tree.symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == crate::SymbolKind::Module && s.full_range.contains(offset))
        .map(|(i, _)| i as u32)
        .next_back() // innermost (modules nest in document order)
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
///
/// Uses the same token-pick as `ident_at_offset` (prefer the IDENT at a
/// token boundary) rather than a bare right-biased-then-left-biased pick.
/// The two must agree: `resolve()` already anchored `name`/`offset` on the
/// IDENT token via `ident_at_offset`, so re-deriving a *different* token
/// here can land the ancestor walk in the wrong subtree. Concretely: for a
/// reference immediately followed by a delimiter with no intervening
/// whitespace (e.g. the `x` in `(x: Field) => x, xs`, where `x` abuts `,`),
/// the old right-biased pick chose the delimiter token, whose parent is the
/// *outer* node (e.g. `MAP_EXPR`), skipping straight past the enclosing
/// `LAMBDA_EXPR` in the ancestor chain and causing the lambda param to be
/// reported unresolved. Anchoring on the IDENT's own parent instead always
/// starts the walk inside the innermost expression, so the enclosing scope
/// (lambda body, block, etc.) is never skipped.
fn resolve_local_name(
    file: FileId,
    root: &SyntaxNode,
    offset: TextSize,
    name: &str,
) -> Option<Definition> {
    let token = ident_at_offset(root, offset)?;
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
}
