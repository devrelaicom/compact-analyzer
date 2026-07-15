//! Salsa incremental-computation layer (v2a). Inputs are file text; tracked
//! queries derive parse/line-index/item-tree/symbols/imports. The impure
//! outer loop (VFS reads, disk probes) lives in `AnalysisHost`, which sets
//! these inputs.

use std::sync::Arc;

use compactp_ast::AstNode;
use compactp_diagnostics::{Diagnostic, DiagnosticCode};

use crate::FileAnalysis;
use crate::item_tree::ItemTree;
use crate::line_index::LineIndex;

/// The database trait tracked functions are written against. A blanket impl
/// makes every salsa database (i.e. `CompactDatabase`) a `Db`.
#[salsa::db]
pub trait Db: salsa::Database {}

#[salsa::db]
impl<T: salsa::Database> Db for T {}

/// The concrete salsa database owned by `AnalysisHost`. Single-threaded use;
/// never cloned into another thread in v2a.
#[salsa::db]
#[derive(Default, Clone)]
pub struct CompactDatabase {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for CompactDatabase {}

/// File text as a salsa input. Identity is the salsa-assigned id; the host
/// maps `FileId -> SourceText` so a file keeps one stable input across edits.
#[salsa::input]
#[derive(Debug)]
pub struct SourceText {
    #[returns(clone)]
    pub text: Arc<str>,
}

/// Parse + derive everything M1 knows about one file. Memoized on the
/// `SourceText` input; recomputed only when that file's text changes.
///
/// `no_eq`: one of `FileAnalysis`'s fields
/// (`diagnostics: Arc<Vec<compactp_diagnostics::Diagnostic>>`) can't derive
/// `PartialEq` — `Diagnostic` is defined in an external crate and doesn't
/// implement it. Without `no_eq`, salsa requires the return type to be
/// comparable so it can "backdate" unchanged results to downstream queries;
/// `no_eq` opts this query out of that comparison, so `parsed`'s direct
/// tracked dependents (`file_symbols`, `raw_imports`, both below) always
/// re-execute whenever a file's text changes — `parsed`'s result is treated
/// as changed every time. Those dependents immediately project out
/// `PartialEq` types (`Arc<[(String, u32)]>`, `Arc<[RawDep]>`), though, so
/// *their* outputs backdate normally; that projection layer, not `parsed`
/// itself, is the real backdating firewall insulating further consumers.
/// `no_eq` costs nothing in principle here regardless: `FileAnalysis`
/// embeds the full `green` tree, which reflects every source byte including
/// trivia, so `parsed` could never backdate even if it were `PartialEq`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn parsed(db: &dyn Db, src: SourceText) -> FileAnalysis {
    let text = src.text(db);
    let result = compactp_parser::parse(&text);
    let root = compactp_syntax::SyntaxNode::new_root(result.green.clone());
    FileAnalysis {
        green: result.green,
        diagnostics: Arc::new(result.errors),
        line_index: Arc::new(LineIndex::new(text)),
        item_tree: Arc::new(ItemTree::extract(&root)),
    }
}

/// An import/include target as it appears in source, before filesystem
/// resolution. `CompactStandardLibrary` and imports satisfied by an in-scope
/// module are already filtered out.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RawDep {
    pub raw: String,
    pub is_include: bool,
}

/// Non-empty-named symbols in `src`'s item tree, paired with their index in
/// `ItemTree::symbols`. Derived from `item_tree` (not `parsed` directly) so
/// this query — and everything built on it — benefits from `item_tree`'s
/// backdating firewall: a trivia-only edit leaves `item_tree`'s `Arc`
/// unchanged, so `file_symbols` skips re-execution too.
#[salsa::tracked(returns(clone))]
pub fn file_symbols(db: &dyn Db, src: SourceText) -> Arc<[(String, u32)]> {
    let tree = crate::db::item_tree(db, src);
    tree.symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.name.is_empty())
        .map(|(idx, s)| (s.name.clone(), idx as u32))
        .collect()
}

/// Import/include targets in `src`, before filesystem resolution. The pure
/// half of `AnalysisHost::index_file`'s dependency-extraction loop: this
/// filters out `CompactStandardLibrary` and imports satisfied by an in-scope
/// module, but does not resolve remaining targets to files (that requires
/// the VFS and search path, which live outside salsa).
#[salsa::tracked(returns(clone))]
pub fn raw_imports(db: &dyn Db, src: SourceText) -> Arc<[RawDep]> {
    let analysis = crate::db::parsed(db, src);
    let tree = analysis.item_tree.clone();
    let root = compactp_syntax::SyntaxNode::new_root(analysis.green.clone());
    let Some(sf) = compactp_ast::SourceFile::cast(root) else {
        return Arc::from(Vec::new());
    };
    let mut out = Vec::new();
    for import in sf.imports() {
        let raw = if let Some(n) = import.name() {
            let nm = n.text().to_string();
            if nm == "CompactStandardLibrary" {
                continue;
            }
            if tree
                .top_level()
                .any(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == nm)
            {
                continue;
            }
            nm
        } else if let Some(p) = import.path() {
            crate::string_lit_text(p.text())
        } else {
            continue;
        };
        out.push(RawDep {
            raw,
            is_include: false,
        });
    }
    for inc in sf.includes() {
        if let Some(tok) = inc.path() {
            out.push(RawDep {
                raw: crate::string_lit_text(tok.text()),
                is_include: true,
            });
        }
    }
    Arc::from(out)
}

/// A cross-file resolved edge: one import/include target after host-side
/// disk resolution. `target: None` means the raw string did not resolve to a
/// file.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ResolvedDep {
    pub raw: String,
    pub is_include: bool,
    pub target: Option<(crate::FileId, SourceText)>,
}

/// Per-file cross-file environment, set by the host from `index_file`'s
/// resolution pass. One handle per `FileId`, stored host-side like `sources`.
#[salsa::input]
pub struct FileDeps {
    #[returns(clone)]
    pub deps: Arc<[ResolvedDep]>,
}

/// Workspace singleton. Holds the stdlib file's `(FileId, SourceText)` (or
/// `None`) plus a `FileId -> FileDeps` map so tracked resolution queries can
/// reach a *target* file's own cross-file edges (needed for the transitive
/// include walk). The host republishes `file_deps` only when its keyset
/// changes (a file added/removed); per-file dep *content* changes flow
/// through the individual `FileDeps` inputs. It also holds `file_srcs`, a
/// `FileId -> SourceText` map for every file reachable from the workspace,
/// republished on that same structural (keyset-change) schedule; see its own
/// doc comment below for detail.
#[salsa::input]
pub struct Workspace {
    #[returns(clone)]
    pub stdlib: Option<(crate::FileId, SourceText)>,
    #[returns(clone)]
    pub file_deps: std::sync::Arc<std::collections::BTreeMap<crate::FileId, FileDeps>>,
    /// `FileId -> SourceText` for every file reachable as a resolution target
    /// (indexed files, resolved import/include targets, stdlib). Lets
    /// `source_text_for` recover a target's text with an O(1) map lookup that
    /// depends only on this map — not on every file's `FileDeps`. Republished
    /// by the host solely on structural change (a file enters/leaves the
    /// workspace), never on an in-body edit.
    #[returns(clone)]
    pub file_srcs: std::sync::Arc<std::collections::BTreeMap<crate::FileId, SourceText>>,
}

/// Item tree for `src`, memoized on the `SourceText` input. Projects
/// `parsed`'s item tree out. `ItemTree` (and `Symbol`) derive `PartialEq,
/// Eq`, so this query backdates normally: a trivia-only edit re-runs
/// `parsed` (which can never backdate — see above) but produces an
/// unchanged `ItemTree`, so `item_tree` returns the *same* `Arc` allocation
/// and every tracked query downstream of `item_tree` skips re-execution.
/// This is the backdating firewall the rest of v2b.1's resolution queries
/// build on.
#[salsa::tracked(returns(clone))]
pub fn item_tree(db: &dyn Db, src: SourceText) -> Arc<ItemTree> {
    crate::db::parsed(db, src).item_tree
}

/// File-scope name resolution as a tracked query: enclosing-module siblings,
/// then top-level items, then included files' top-level decls, then imports.
/// This is the salsa port of the former imperative file-scope resolver; all
/// cross-file targets come from `fd`/`ws` (no I/O).
#[salsa::tracked(returns(clone))]
pub fn resolve_in_file(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    offset: text_size::TextSize,
    name: String,
) -> Option<crate::Definition> {
    let tree = crate::db::item_tree(db, src);

    // Inside a module, siblings are visible first.
    if let Some(m) = crate::resolve::enclosing_module(&tree, offset)
        && let Some((idx, _)) = tree.children_of(m).find(|(_, s)| s.name == name)
    {
        return Some(crate::Definition::Item { file, index: idx });
    }

    // Top-level items.
    if let Some((idx, _)) = tree.top_level().find(|(_, s)| s.name == name) {
        return Some(crate::Definition::Item { file, index: idx });
    }

    // Included files' top-level declarations are spliced into this scope.
    if let Some(d) = resolve_includes(db, file, fd, ws, &name, &mut vec![file]) {
        return Some(d);
    }

    // Imports, in order.
    resolve_imports(db, file, src, fd, ws, &name)
}

/// Resolves `name` against the top-level declarations of files reachable by
/// `include` from `file`, depth-first. A plain `db`-taking recursive helper
/// (NOT a tracked query) carrying the explicit `active` cycle guard, so the
/// cyclic include graph never becomes a salsa cycle; it still benefits from
/// `item_tree` memoization. A target's own `FileDeps` (needed to recurse into
/// *its* includes) is looked up in `ws.file_deps`; a target that was never
/// indexed (`None`) is skipped — matching the resolver's "unreadable include:
/// skip, never die" ethos.
pub(crate) fn resolve_includes(
    db: &dyn Db,
    // Unused here: retained for call-site symmetry with the sibling
    // resolution helpers (e.g. `resolve_imports`), which do need `file`.
    _file: crate::FileId,
    fd: FileDeps,
    ws: Workspace,
    name: &str,
    active: &mut Vec<crate::FileId>,
) -> Option<crate::Definition> {
    let deps = fd.deps(db);
    for dep in deps.iter().filter(|d| d.is_include) {
        let Some((tfile, tsrc)) = dep.target else {
            continue;
        };
        if active.contains(&tfile) {
            continue; // cycle along the active include path
        }
        let ttree = crate::db::item_tree(db, tsrc);
        if let Some((idx, _)) = ttree.top_level().find(|(_, s)| s.name == name) {
            return Some(crate::Definition::Item {
                file: tfile,
                index: idx,
            });
        }
        // Recurse into the include's own includes, via the workspace-wide map.
        let Some(tfd) = ws.file_deps(db).get(&tfile).copied() else {
            continue; // target not indexed: skip
        };
        active.push(tfile);
        let found = resolve_includes(db, tfile, tfd, ws, name, active);
        active.pop();
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Resolves `name` through the file's imports, in declaration order. Salsa
/// port of the former imperative import-resolution driver loop: rebuild the
/// AST from the memoized `parsed` green tree and try each `import` in turn.
/// Pure — every cross-file target arrives via `fd`/`ws`.
pub(crate) fn resolve_imports(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    name: &str,
) -> Option<crate::Definition> {
    let tree = crate::db::item_tree(db, src);
    let green = crate::db::parsed(db, src).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let sf = compactp_ast::SourceFile::cast(root)?;
    for import in sf.imports() {
        if let Some(d) = resolve_one_import(db, file, &tree, fd, ws, &import, name) {
            return Some(d);
        }
    }
    None
}

/// One import's contribution to the namespace, honoring prefix and selective
/// specifier lists (with aliases). Faithful transcription of the former
/// imperative import-resolution logic: the two filesystem-probe call sites
/// become reads of the matching `ResolvedDep` in `fd` (via
/// `resolve_external_module_member`), and the `CompactStandardLibrary` arm
/// reads `ws.stdlib`.
fn resolve_one_import(
    db: &dyn Db,
    file: crate::FileId,
    tree: &crate::ItemTree,
    fd: FileDeps,
    ws: Workspace,
    import: &compactp_ast::Import,
    name: &str,
) -> Option<crate::Definition> {
    // Determine the effective name to look up in the import's exports.
    let mut lookup: &str = name;
    let stripped;
    if let Some(prefix) = import.prefix().and_then(|p| p.name()) {
        // Prefix applies to every name from this import: unprefixed names never
        // resolve through it.
        stripped = name.strip_prefix(prefix.text())?.to_string();
        lookup = &stripped;
    }

    // Selective list: only listed names are visible; `as` aliases rename them.
    let specifiers: Vec<(String, String)> = import
        .syntax()
        .descendants()
        .filter_map(compactp_ast::ImportSpecifier::cast)
        .filter_map(|spec| {
            let original = spec.name()?.text().to_string();
            let alias =
                crate::resolve::import_specifier_alias(&spec).unwrap_or_else(|| original.clone());
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
            let (std_file, std_src) = ws.stdlib(db)?;
            let std_tree = crate::db::item_tree(db, std_src);
            let (idx, _) = std_tree
                .top_level()
                .find(|(_, s)| s.exported && s.name == target_name)?;
            Some(crate::Definition::Item {
                file: std_file,
                index: idx,
            })
        }
        Some(module_token) => {
            let module_name = module_token.text().to_string();
            // In-scope module first (in-scope FIRST, filesystem second). An
            // in-scope module wins even if the member is absent.
            if let Some((module_idx, _)) = tree
                .top_level()
                .find(|(_, s)| s.kind == crate::SymbolKind::Module && s.name == module_name)
            {
                return tree
                    .children_of(module_idx)
                    .find(|(_, s)| s.exported && s.name == target_name)
                    .map(|(idx, _)| crate::Definition::Item { file, index: idx });
            }
            // No in-scope module → external module member via the file's
            // resolved deps: `<module_name>` containing exactly `module
            // <module_name>`.
            resolve_external_module_member(db, fd, &module_name, &module_name, &target_name)
        }
        // String-path import: never consults in-scope modules; expected module
        // name is the last path component.
        None => {
            let raw = crate::string_lit_text(import.path()?.text());
            let expected = crate::path_module_name(&raw)?;
            resolve_external_module_member(db, fd, &raw, &expected, &target_name)
        }
    }
}

/// Resolves `target_name` as an exported member of the single module in the
/// file that `raw` resolves to, requiring that module's name to equal
/// `expected_module` (compiler rule). Used by both the identifier and
/// string-path import arms. The host's disk probe + `analyze` becomes: find the
/// matching non-include `ResolvedDep` in `fd` and read the target's
/// `item_tree`.
pub(crate) fn resolve_external_module_member(
    db: &dyn Db,
    fd: FileDeps,
    raw: &str,
    expected_module: &str,
    target_name: &str,
) -> Option<crate::Definition> {
    let dep = fd
        .deps(db)
        .iter()
        .find(|d| d.raw == raw && !d.is_include)?
        .clone();
    let (tfile, tsrc) = dep.target?;
    let ttree = crate::db::item_tree(db, tsrc);
    let (module_idx, module_sym) = crate::resolve::single_top_level_module(&ttree)?;
    if module_sym.name != expected_module {
        return None;
    }
    let (idx, _) = ttree
        .children_of(module_idx)
        .find(|(_, s)| s.exported && s.name == target_name)?;
    Some(crate::Definition::Item {
        file: tfile,
        index: idx,
    })
}

/// Tracked top-level resolution dispatcher: the salsa port of
/// `AnalysisHost::resolve`. Classifies the token at `offset` by its parent CST
/// kind (there is no reference node in the Compact CST) and routes to the
/// matching arm. Pure — every cross-file target arrives via `fd`/`ws`; the host
/// bridge (`AnalysisHost::resolve`) provisions those inputs (indexing the file
/// and its transitive includes) before calling in.
#[salsa::tracked(returns(clone))]
pub fn resolve_query(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    offset: text_size::TextSize,
) -> Option<crate::Definition> {
    use compactp_syntax::SyntaxKind;
    let green = crate::db::parsed(db, src).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let token = crate::resolve::ident_at_offset(&root, offset)?;
    let name = token.text().to_string();
    let parent = token.parent()?;

    match parent.kind() {
        // Definition sites resolve to themselves. Shorthand `{a}` binds `a`;
        // `name: pat` labels a field — both hit the pattern-site early return.
        SyntaxKind::IDENT_PAT | SyntaxKind::STRUCT_PAT_FIELD => {
            return crate::resolve::local_from_pattern_site(file, &token);
        }
        SyntaxKind::FOR_STMT => {
            return Some(crate::Definition::Local {
                file,
                name: name.clone(),
                name_range: token.text_range(),
                detail: format!("for {name}"),
            });
        }
        SyntaxKind::GENERIC_PARAM => {
            return Some(crate::resolve::generic_definition(file, &parent, &token));
        }
        _ => {}
    }

    // Names of item declarations resolve to the item itself.
    if let Some(def) = item_declaration_at(db, file, src, &parent, &token) {
        return Some(def);
    }

    match parent.kind() {
        SyntaxKind::NAME_EXPR
        | SyntaxKind::CALL_EXPR
        | SyntaxKind::TYPE_REF
        | SyntaxKind::STRUCT_EXPR
        | SyntaxKind::TYPE_SIZE => resolve_name_at(db, file, src, fd, ws, &root, offset, &name),
        SyntaxKind::STRUCT_FIELD_INIT => {
            resolve_struct_literal_field(db, file, src, fd, ws, &root, &parent, &name)
        }
        SyntaxKind::MEMBER_EXPR => resolve_member(db, file, src, fd, ws, &root, &parent, &name),
        SyntaxKind::IMPORT_SPECIFIER => resolve_import_specifier(db, file, src, ws, &parent, &name),
        _ => None,
    }
}

/// Tracked entry point for "resolve `name` as if referenced at `offset`",
/// independent of whatever token actually sits at `offset`. Used by rename's
/// per-use-site conflict check (via the `AnalysisHost::resolve_name_at`
/// bridge); the top-level dispatcher calls the inner `resolve_name_at` helper
/// directly.
#[salsa::tracked(returns(clone))]
pub(crate) fn resolve_name_query(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    offset: text_size::TextSize,
    name: String,
) -> Option<crate::Definition> {
    let green = crate::db::parsed(db, src).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    resolve_name_at(db, file, src, fd, ws, &root, offset, &name)
}

/// Resolves `name` as if referenced at `offset`: locals first (nearest scope
/// wins), then file scope + imports (the tracked `resolve_in_file`). Port of
/// `AnalysisHost::resolve_name_at`.
#[allow(clippy::too_many_arguments)]
fn resolve_name_at(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    root: &compactp_syntax::SyntaxNode,
    offset: text_size::TextSize,
    name: &str,
) -> Option<crate::Definition> {
    if let Some(local) = crate::resolve::resolve_local_name(file, root, offset, name) {
        return Some(local);
    }
    resolve_in_file(db, file, src, fd, ws, offset, name.to_string())
}

/// Struct-literal field label: resolve the struct name (from its own position),
/// then find the matching `StructField` child. Port of
/// `AnalysisHost::resolve_struct_literal_field`.
#[allow(clippy::too_many_arguments)]
fn resolve_struct_literal_field(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    root: &compactp_syntax::SyntaxNode,
    field_init: &compactp_syntax::SyntaxNode,
    name: &str,
) -> Option<crate::Definition> {
    let struct_expr = field_init
        .ancestors()
        .find(|n| n.kind() == compactp_syntax::SyntaxKind::STRUCT_EXPR)
        .and_then(compactp_ast::expr::StructExpr::cast)?;
    let struct_name = struct_expr.name()?.text().to_string();
    let struct_off = struct_expr.syntax().text_range().start();
    let struct_def = resolve_name_at(db, file, src, fd, ws, root, struct_off, &struct_name)?;
    field_of(
        db,
        &struct_def,
        crate::SymbolKind::StructField,
        name,
        file,
        src,
        ws,
    )
}

/// Member access: the only syntactically decidable case is a `NAME_EXPR`
/// receiver resolving to an enum → the member is a variant. Everything else
/// (ledger ADT methods, struct field access) needs types → None. Port of
/// `AnalysisHost::resolve_member`.
#[allow(clippy::too_many_arguments)]
fn resolve_member(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    root: &compactp_syntax::SyntaxNode,
    member: &compactp_syntax::SyntaxNode,
    name: &str,
) -> Option<crate::Definition> {
    let receiver = member
        .children()
        .find(|n| n.kind() == compactp_syntax::SyntaxKind::NAME_EXPR)
        .and_then(compactp_ast::expr::NameExpr::cast)?;
    let receiver_name = receiver.ident()?.text().to_string();
    let receiver_off = receiver.syntax().text_range().start();
    let receiver_def = resolve_name_at(db, file, src, fd, ws, root, receiver_off, &receiver_name)?;
    let crate::Definition::Item { file: rfile, index } = &receiver_def else {
        return None;
    };
    let rsrc = source_text_for(db, *rfile, file, src, ws)?;
    let tree = item_tree(db, rsrc);
    if tree.symbols[*index as usize].kind != crate::SymbolKind::Enum {
        return None;
    }
    field_of(
        db,
        &receiver_def,
        crate::SymbolKind::EnumVariant,
        name,
        file,
        src,
        ws,
    )
}

/// Import specifier: only the ORIGINAL name in `import { orig as alias }` is a
/// reference into the imported module (the alias is a binding). Resolve it
/// through this import, bypassing the specifier's own alias filtering. Port of
/// `AnalysisHost::resolve_import_specifier`.
fn resolve_import_specifier(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    ws: Workspace,
    specifier: &compactp_syntax::SyntaxNode,
    name: &str,
) -> Option<crate::Definition> {
    let spec = compactp_ast::ImportSpecifier::cast(specifier.clone())?;
    let original = spec.name()?;
    if original.text() != name {
        return None;
    }
    let import = specifier
        .ancestors()
        .find(|n| n.kind() == compactp_syntax::SyntaxKind::IMPORT)
        .and_then(compactp_ast::Import::cast)?;
    let tree = item_tree(db, src);
    match import.name() {
        Some(t) if t.text() == "CompactStandardLibrary" => {
            let (std_file, std_src) = ws.stdlib(db)?;
            let std_tree = item_tree(db, std_src);
            let (idx, _) = std_tree
                .top_level()
                .find(|(_, s)| s.exported && s.name == name)?;
            Some(crate::Definition::Item {
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
            Some(crate::Definition::Item { file, index: idx })
        }
        None => None,
    }
}

/// If `token` names an item declaration in `src`, return that item. Port of
/// `AnalysisHost::item_declaration_at`.
fn item_declaration_at(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    parent: &compactp_syntax::SyntaxNode,
    token: &compactp_syntax::SyntaxToken,
) -> Option<crate::Definition> {
    use compactp_syntax::SyntaxKind;
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
    let tree = item_tree(db, src);
    let range = token.text_range();
    let index = tree.symbols.iter().position(|s| s.name_range == range)?;
    Some(crate::Definition::Item {
        file,
        index: index as u32,
    })
}

/// Child symbol of `parent_def` with the given kind and name. Port of
/// `AnalysisHost::field_of`; the target file's `item_tree` is read via
/// [`source_text_for`] rather than the host's `analyze`.
fn field_of(
    db: &dyn Db,
    parent_def: &crate::Definition,
    kind: crate::SymbolKind,
    name: &str,
    this_file: crate::FileId,
    this_src: SourceText,
    ws: Workspace,
) -> Option<crate::Definition> {
    let crate::Definition::Item { file, index } = parent_def else {
        return None;
    };
    let psrc = source_text_for(db, *file, this_file, this_src, ws)?;
    let tree = item_tree(db, psrc);
    let (idx, _) = tree
        .children_of(*index)
        .find(|(_, s)| s.kind == kind && s.name == name)?;
    Some(crate::Definition::Item {
        file: *file,
        index: idx,
    })
}

/// Maps a `FileId` reachable from the resolution context back to its
/// `SourceText`, via `Workspace.file_srcs` (O(1), and dependency-narrow: it
/// depends only on `file_srcs`, not on every file's `FileDeps`). The current
/// file and stdlib are checked first so a resolution that never leaves the
/// current file takes no `Workspace` dependency at all.
fn source_text_for(
    db: &dyn Db,
    target: crate::FileId,
    this_file: crate::FileId,
    this_src: SourceText,
    ws: Workspace,
) -> Option<SourceText> {
    if target == this_file {
        return Some(this_src);
    }
    if let Some((sfile, ssrc)) = ws.stdlib(db)
        && sfile == target
    {
        return Some(ssrc);
    }
    ws.file_srcs(db).get(&target).copied()
}

/// Error diagnostics for imports/includes in `src`, given `fd`'s resolved
/// targets. Tracked port of `AnalysisHost::resolution_diagnostics` (+ the
/// module-mismatch check it used to run inline): for each import/include
/// recovered from `parsed(db, src)`'s green tree, look up the matching `ResolvedDep` in `fd`
/// — `target: None` reports the unresolved-import/include diagnostic
/// (E9001/E9004); `target: Some((_, tsrc))` runs the module-mismatch check
/// against `tsrc`'s item tree for imports (E9002/E9003); includes have no
/// mismatch check, exactly as the original. The in-scope-module and
/// `CompactStandardLibrary` skips are preserved verbatim (both make an import
/// emit no diagnostic at all, matching the original's `continue`s). `fd` is the
/// persisted `FileDeps` input published by `index_file` (no I/O happens here).
///
/// The "searched: {list}" text embeds `from_dir` plus the import search path
/// — a display detail that depends on host `Path::display()` formatting, an
/// impure concern the query itself must not touch. The host precomputes
/// `from_dir_display` (`from_dir.display().to_string()`) and `search_display`
/// (the search path list joined with `", "`, the same way the original
/// imperative "searched:" formatting joined it) and hands them in as
/// `Arc<str>`; [`searched_list_display`] recombines them into the exact
/// original string.
///
/// `no_eq`: `Diagnostic` (external crate `compactp_diagnostics`) has no
/// `PartialEq`, so `Arc<[Diagnostic]>` can't be compared for backdating —
/// same rationale as `parsed` above.
#[salsa::tracked(returns(clone), no_eq)]
pub fn resolution_diagnostics_query(
    db: &dyn Db,
    src: SourceText,
    fd: FileDeps,
    from_dir_display: Arc<str>,
    search_display: Arc<str>,
) -> Arc<[Diagnostic]> {
    let tree = crate::db::item_tree(db, src);
    let green = crate::db::parsed(db, src).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let Some(sf) = compactp_ast::SourceFile::cast(root) else {
        return Arc::from(Vec::new());
    };
    let deps = fd.deps(db);
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
            push_import_diag(
                db,
                &deps,
                &nm,
                &nm,
                name.text_range(),
                &from_dir_display,
                &search_display,
                &mut diags,
            );
        } else if let Some(ptok) = import.path() {
            let raw = crate::string_lit_text(ptok.text());
            let span = ptok.text_range();
            let Some(expected) = crate::path_module_name(&raw) else {
                continue;
            };
            push_import_diag(
                db,
                &deps,
                &raw,
                &expected,
                span,
                &from_dir_display,
                &search_display,
                &mut diags,
            );
        }
    }

    for inc in sf.includes() {
        if let Some(tok) = inc.path() {
            let raw = crate::string_lit_text(tok.text());
            let target = deps
                .iter()
                .find(|d| d.is_include && d.raw == raw)
                .and_then(|d| d.target);
            if target.is_none() {
                diags.push(unresolved_include_diag_display(
                    &raw,
                    &from_dir_display,
                    &search_display,
                    tok.text_range(),
                ));
            }
        }
    }

    Arc::from(diags)
}

/// Resolve a `Definition::Item` to its `Symbol`. The current file's symbols
/// come from `item_tree(src)`; a cross-file target's `SourceText` is reached by
/// matching its `FileId` against a materialized `fd` dep's target (includes and
/// imports alike). A target neither in the current file nor in `fd` yields
/// `None` (suppresses).
fn item_symbol(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    def_file: crate::FileId,
    index: u32,
) -> Option<crate::Symbol> {
    if def_file == file {
        return item_tree(db, src).symbols.get(index as usize).cloned();
    }
    let (_, tsrc) = fd
        .deps(db)
        .iter()
        .filter_map(|d| d.target)
        .find(|(f, _)| *f == def_file)?;
    item_tree(db, tsrc).symbols.get(index as usize).cloned()
}

/// The generic-arity mismatch diagnostic (`E3003`). Wording tracks compactc's
/// ("mismatch between actual number A and declared number D of generic
/// parameters for Name"); wording is not gated by the harness.
fn generic_arity_diag(
    actual: u32,
    declared: u32,
    name: &str,
    span: text_size::TextRange,
) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 3003),
        format!(
            "mismatch between actual number {actual} and declared number {declared} of generic parameters for {name}"
        ),
        span,
    )
}

/// Generic-specialization diagnostics for `src`: every `TypeRef` whose name
/// resolves to a named-type definition (struct / enum / type alias) must carry
/// exactly the definition's declared number of generic arguments. Resolution
/// is required (arity lives on the *definition*), so this query is
/// resolution-fed like `resolution_diagnostics_query`. Suppresses whenever the
/// name does not resolve to a countable named-type definition — never a false
/// positive.
#[salsa::tracked(returns(clone), no_eq)]
pub fn generic_diagnostics_query(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
) -> Arc<[Diagnostic]> {
    let green = crate::db::parsed(db, src).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let mut diags = Vec::new();

    for tref in root.descendants().filter_map(compactp_ast::TypeRef::cast) {
        let Some(name_tok) = tref.name() else {
            continue;
        };
        // Resolve the type name to its definition.
        let def = resolve_query(db, file, src, fd, ws, name_tok.text_range().start());
        let Some(crate::Definition::Item {
            file: def_file,
            index,
        }) = def
        else {
            continue; // unresolved, or a local/generic-param binding -> suppress
        };
        let Some(sym) = item_symbol(db, file, src, fd, def_file, index) else {
            continue; // target unreachable -> suppress
        };
        // Only named *type* declarations carry a checkable generic arity.
        if !matches!(
            sym.kind,
            crate::SymbolKind::Struct | crate::SymbolKind::Enum | crate::SymbolKind::TypeAlias
        ) {
            continue;
        }
        let actual = tref
            .generic_args()
            .map(|a| a.args().count() as u32)
            .unwrap_or(0);
        if actual != sym.generic_param_count {
            diags.push(generic_arity_diag(
                actual,
                sym.generic_param_count,
                name_tok.text(),
                tref.syntax().text_range(),
            ));
        }
    }
    Arc::from(diags)
}

/// The ledger-call argument-mismatch diagnostic (`E3004`). Wording tracks
/// compactc's ("expected first argument of M to have type T but received U");
/// wording is not gated by the harness.
fn ledger_arg_diag(
    pos: usize,
    method: &str,
    expected: &str,
    actual: &str,
    span: text_size::TextRange,
) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 3004),
        format!(
            "expected argument {} of {method} to have type {expected} but received {actual}",
            pos + 1
        ),
        span,
    )
}

/// If the identifier at `offset` resolves to a ledger field, its builtin ADT
/// key (Counter/Map/…), or `None` when it is not a ledger field or its declared
/// type resolves to a user-defined type (implicit Cell / not a builtin). Pure
/// resolution-context twin of `AnalysisHost::ledger_field_adt`.
fn ledger_adt_at(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    offset: text_size::TextSize,
) -> Option<String> {
    let def = resolve_query(db, file, src, fd, ws, offset)?;
    let crate::Definition::Item { file: df, index } = def else {
        return None;
    };
    let sym = item_symbol(db, file, src, fd, df, index)?;
    if sym.kind != crate::SymbolKind::Ledger {
        return None;
    }
    // Read the ledger field's declared type head from its own file.
    let (_, dsrc) = if df == file {
        (df, src)
    } else {
        fd.deps(db)
            .iter()
            .filter_map(|d| d.target)
            .find(|(f, _)| *f == df)?
    };
    let green = crate::db::parsed(db, dsrc).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let ledger = root
        .descendants()
        .filter_map(compactp_ast::LedgerDecl::cast)
        .find(|d| d.name().is_some_and(|t| t.text_range() == sym.name_range))?;
    let head_tok = match ledger.ty()? {
        compactp_ast::Type::Ref(r) => r.name()?,
        _ => return None, // scalar-typed -> implicit Cell (no builtin ADT methods checked)
    };
    // If the head resolves to a user-defined named type, it is not a builtin
    // ADT. Only run this check when the ledger field is in the SAME file as the
    // call (`df == file`), where `src`/`fd` are the correct resolution inputs
    // for `df`; a cross-file field's head would need `df`'s own `FileDeps`, and
    // using the caller's `fd` could mis-resolve. Skipping the check cross-file
    // is safe: emitting `E3004` requires a *builtin* ADT, which requires
    // `import CompactStandardLibrary;`, and with that import a user type named
    // like a builtin is a duplicate-binding compile error — so on any
    // `compactc`-accepted file a builtin head is genuinely the builtin.
    if df == file {
        let head_off = head_tok.text_range().start();
        if let Some(crate::Definition::Item {
            file: hf,
            index: hi,
        }) = resolve_query(db, file, src, fd, ws, head_off)
            && let Some(hs) = item_symbol(db, file, src, fd, hf, hi)
            && matches!(
                hs.kind,
                crate::SymbolKind::Struct | crate::SymbolKind::Enum | crate::SymbolKind::TypeAlias
            )
        {
            return None;
        }
    }
    const KNOWN: &[&str] = &[
        "Counter",
        "Cell",
        "Map",
        "Set",
        "List",
        "MerkleTree",
        "HistoricMerkleTree",
        "Kernel",
    ];
    let head = head_tok.text().to_string();
    KNOWN.contains(&head.as_str()).then_some(head)
}

/// Ledger method-call argument type checks (`E3004`) for `src`. For each method
/// call `recv.method(args)` whose receiver resolves to a builtin-ADT ledger
/// field and whose `method` is in that ADT's typed surface, any positional
/// argument that is a typeable literal/cast is checked against a concretely
/// modeled parameter type. Suppresses on every uncertainty (never a false
/// positive). Resolution-fed like `generic_diagnostics_query`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn ledger_call_diagnostics_query(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
) -> Arc<[Diagnostic]> {
    use compactp_ast::AstNode;
    let green = crate::db::parsed(db, src).green;
    let root = compactp_syntax::SyntaxNode::new_root(green);
    let mut diags = Vec::new();

    for call in root
        .descendants()
        .filter_map(compactp_ast::expr::CallExpr::cast)
    {
        // A method call has a DOT token child; a plain call does not.
        let Some(dot) = call
            .syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == compactp_syntax::SyntaxKind::DOT)
        else {
            continue;
        };
        let dot_start = dot.text_range().start();
        // Receiver: the Expr child ending at/before the DOT. Method name: CallExpr::name().
        let Some(recv) = call
            .syntax()
            .children()
            .filter_map(compactp_ast::expr::Expr::cast)
            .find(|e| e.syntax().text_range().end() <= dot_start)
        else {
            continue;
        };
        // Only a bare NAME_EXPR receiver is identified; anything else -> suppress.
        if recv.syntax().kind() != compactp_syntax::SyntaxKind::NAME_EXPR {
            continue;
        }
        // Resolve at the receiver's IDENT token, NOT the node start: the parser
        // attaches the statement's leading whitespace inside the NAME_EXPR, so
        // the node start points at whitespace and `resolve_query`
        // (`token_at_offset`) would fail. Mirrors `generic_diagnostics_query`,
        // which resolves at `name_tok.text_range().start()`.
        let Some(recv_off) = recv
            .syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == compactp_syntax::SyntaxKind::IDENT)
            .map(|t| t.text_range().start())
        else {
            continue;
        };
        let Some(method_tok) = call.name() else {
            continue;
        };
        let method = method_tok.text().to_string();

        let Some(adt) = ledger_adt_at(db, file, src, fd, ws, recv_off) else {
            continue;
        };
        let Some(sig) = crate::ledger_adts::ledger_method_sig(&adt, &method) else {
            continue;
        };

        // Positional argument Exprs after the L_PAREN (skip NAMED_ARG etc.).
        let lparen_start = call
            .syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == compactp_syntax::SyntaxKind::L_PAREN)
            .map(|t| t.text_range().start());
        let Some(lparen_start) = lparen_start else {
            continue;
        };
        let args: Vec<compactp_ast::expr::Expr> = call
            .syntax()
            .children()
            .filter_map(compactp_ast::expr::Expr::cast)
            .filter(|e| e.syntax().text_range().start() >= lparen_start)
            .collect();

        for (i, arg) in args.iter().enumerate() {
            let Some(param) = sig.params.get(i) else {
                break;
            };
            if *param == crate::ty::TyKind::Unknown {
                continue; // generic / unmodeled param -> suppress
            }
            let actual = crate::infer::expr_ty_kind(arg);
            if actual == crate::ty::TyKind::Unknown {
                continue; // non-literal/cast arg -> suppress
            }
            if !crate::ty::is_subtype(&actual, param) {
                diags.push(ledger_arg_diag(
                    i,
                    &method,
                    &crate::ty::display_kind(param),
                    &crate::ty::display_kind(&actual),
                    arg.syntax().text_range(),
                ));
            }
        }
    }
    Arc::from(diags)
}

/// One import's diagnostic contribution: find the matching non-include
/// `ResolvedDep` for `raw`; push the unresolved-import diagnostic if it
/// didn't resolve, else run the module-mismatch check on the resolved
/// target's item tree.
#[allow(clippy::too_many_arguments)]
fn push_import_diag(
    db: &dyn Db,
    deps: &[ResolvedDep],
    raw: &str,
    expected_module: &str,
    span: text_size::TextRange,
    from_dir_display: &str,
    search_display: &str,
    diags: &mut Vec<Diagnostic>,
) {
    let target = deps
        .iter()
        .find(|d| !d.is_include && d.raw == raw)
        .and_then(|d| d.target);
    match target {
        None => diags.push(unresolved_import_diag_display(
            raw,
            from_dir_display,
            search_display,
            span,
        )),
        Some((_, tsrc)) => {
            if let Some(d) = module_mismatch_diag_query(db, tsrc, expected_module, span) {
                diags.push(d);
            }
        }
    }
}

/// Error iff `tsrc`'s single top-level module does not match `expected`. Port
/// of the former imperative module-mismatch check; the host's
/// `analyze(target)` disk read becomes a plain `item_tree(db, tsrc)` —
/// `tsrc` already arrived via a resolved `ResolvedDep`, materialized once,
/// host-side, so no I/O happens here.
fn module_mismatch_diag_query(
    db: &dyn Db,
    tsrc: SourceText,
    expected: &str,
    span: text_size::TextRange,
) -> Option<Diagnostic> {
    let tree = crate::db::item_tree(db, tsrc);
    match crate::resolve::single_top_level_module(&tree) {
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

/// Recombines the host-precomputed `from_dir_display`/`search_display`
/// strings into the exact "searched: ..." list the original imperative
/// formatting produced: `from_dir` alone when the search path is empty, else
/// `from_dir, <search paths joined by ", ">` — `search_display` is already
/// that same join, computed host-side the same way the original did it.
fn searched_list_display(from_dir_display: &str, search_display: &str) -> String {
    if search_display.is_empty() {
        from_dir_display.to_string()
    } else {
        format!("{from_dir_display}, {search_display}")
    }
}

fn unresolved_import_diag_display(
    raw: &str,
    from_dir_display: &str,
    search_display: &str,
    span: text_size::TextRange,
) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 9001),
        format!(
            "cannot resolve import `{raw}` (searched: {})",
            searched_list_display(from_dir_display, search_display)
        ),
        span,
    )
}

fn unresolved_include_diag_display(
    raw: &str,
    from_dir_display: &str,
    search_display: &str,
    span: text_size::TextRange,
) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::new("E", 9004),
        format!(
            "cannot resolve include `{raw}` (searched: {})",
            searched_list_display(from_dir_display, search_display)
        ),
        span,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_reuses_memo_across_unchanged_calls() {
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("ledger count: Field;"));
        let a = crate::db::parsed(&db, src);
        let b = crate::db::parsed(&db, src);
        // returns(clone) hands back clones of the memoized value; the inner Arcs
        // are the same allocation on a memo hit.
        assert!(Arc::ptr_eq(&a.diagnostics, &b.diagnostics));
    }

    #[test]
    fn editing_one_file_does_not_recompute_another() {
        use salsa::Setter;

        let mut db = CompactDatabase::default();
        let a = SourceText::new(&db, Arc::from("ledger a: Field;"));
        let b = SourceText::new(&db, Arc::from("ledger b: Field;"));

        let _ = crate::db::parsed(&db, a);
        let _ = crate::db::parsed(&db, b);
        let a_diag = crate::db::parsed(&db, a).diagnostics;

        // Edit b; a's memo must survive (same Arc allocation).
        b.set_text(&mut db).to(Arc::from("ledger b2: Field;"));
        let a_diag_again = crate::db::parsed(&db, a).diagnostics;
        assert!(Arc::ptr_eq(&a_diag, &a_diag_again));
    }

    #[test]
    fn trivia_only_edit_backdates_item_tree() {
        use salsa::Setter as _;
        let mut db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("export circuit c(): [] {}"));
        let t1 = crate::db::item_tree(&db, src);
        let s1 = crate::db::file_symbols(&db, src);
        // Add only a trailing comment: items are byte-identical.
        src.set_text(&mut db)
            .to(Arc::from("export circuit c(): [] {} // note"));
        let t2 = crate::db::item_tree(&db, src);
        let s2 = crate::db::file_symbols(&db, src);
        // `item_tree` is a *direct* dependent of `parsed`, which is `no_eq`
        // and therefore always reports "changed" once its input changed;
        // `item_tree` must always re-execute and reallocate its `Arc` in
        // that case (salsa backdates a memo's `changed_at`, but "[r]e-
        // execution may update or replace the value" -- salsa never hands
        // back the old allocation from the query that itself re-executed).
        // So the trivia-only edit still produces a *different* Arc here --
        // but an equal one, confirming `ItemTree`'s new `PartialEq` derive
        // makes the values compare equal despite the byte-different source.
        assert_eq!(
            *t1, *t2,
            "trivia-only edit must leave the ItemTree value unchanged"
        );
        // The backdating firewall payoff appears one hop further downstream:
        // `file_symbols` depends on `item_tree` (not `parsed`), so when
        // `item_tree`'s `changed_at` backdates, `file_symbols` sees no
        // change in its dependency and skips re-execution entirely --
        // returning the exact same `Arc` allocation as before the edit.
        assert!(
            Arc::ptr_eq(&s1, &s2),
            "file_symbols must backdate (same Arc) across a trivia-only edit"
        );
    }

    #[test]
    fn resolve_in_file_finds_top_level_and_enclosing_module() {
        let db = CompactDatabase::default();
        let src = SourceText::new(
            &db,
            Arc::from("circuit helper(): [] {}\nmodule M { circuit inner(): [] {} }"),
        );
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        let ws = Workspace::new(
            &db,
            None,
            Arc::new(std::collections::BTreeMap::new()),
            Arc::new(std::collections::BTreeMap::new()),
        );
        // Top-level `helper` resolves from anywhere.
        let d = resolve_in_file(
            &db,
            crate::FileId::from_raw_for_test(0),
            src,
            fd,
            ws,
            0u32.into(),
            "helper".to_string(),
        );
        assert!(matches!(d, Some(crate::Definition::Item { .. })));
        // A name that does not exist resolves to None.
        assert!(
            resolve_in_file(
                &db,
                crate::FileId::from_raw_for_test(0),
                src,
                fd,
                ws,
                0u32.into(),
                "nope".to_string(),
            )
            .is_none()
        );
    }

    #[test]
    fn resolve_imports_in_scope_module_member() {
        let db = CompactDatabase::default();
        let src = SourceText::new(
            &db,
            Arc::from("module M { export circuit ex(): [] {} }\nimport M;"),
        );
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        // No file_deps map entries needed here: the import resolves through the
        // in-scope module `M`, never touching the filesystem/deps arm.
        let ws = Workspace::new(
            &db,
            None,
            Arc::new(std::collections::BTreeMap::new()),
            Arc::new(std::collections::BTreeMap::new()),
        );
        let d = resolve_imports(&db, crate::FileId::from_raw_for_test(0), src, fd, ws, "ex");
        assert!(matches!(d, Some(crate::Definition::Item { index, .. }) if index != 0));
    }

    #[test]
    fn raw_imports_filters_stdlib_and_local_modules() {
        let db = CompactDatabase::default();
        let text =
            "import CompactStandardLibrary;\nimport Foo;\nmodule Foo {}\nimport \"bar/baz\";\n";
        let src = SourceText::new(&db, Arc::from(text));
        let deps = crate::db::raw_imports(&db, src);
        // CompactStandardLibrary filtered; `Foo` satisfied by local module → filtered;
        // only the string-path import survives.
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].raw, "bar/baz");
    }

    #[test]
    fn source_text_for_reads_file_srcs_map() {
        let db = CompactDatabase::default();
        let this_src = SourceText::new(&db, Arc::from("circuit here(): [] {}"));
        let other_src = SourceText::new(&db, Arc::from("circuit there(): [] {}"));
        let this = crate::FileId::from_raw_for_test(0);
        let other = crate::FileId::from_raw_for_test(1);
        let mut srcs = std::collections::BTreeMap::new();
        srcs.insert(other, other_src);
        let ws = Workspace::new(
            &db,
            None,
            Arc::new(std::collections::BTreeMap::new()),
            Arc::new(srcs),
        );
        // `other` is neither `this_file` nor stdlib nor in this file's deps:
        // it must be recovered from `file_srcs`.
        let got = source_text_for(&db, other, this, this_src, ws);
        assert_eq!(got, Some(other_src));
        // A file present nowhere resolves to None.
        assert_eq!(
            source_text_for(&db, crate::FileId::from_raw_for_test(9), this, this_src, ws),
            None
        );
    }

    #[test]
    fn generic_typeref_arity_is_checked() {
        use crate::AnalysisHost;
        use std::path::Path;

        fn diags(src: &str) -> Vec<compactp_diagnostics::Diagnostic> {
            let mut host = AnalysisHost::new();
            let file = host.vfs_mut().file_id(Path::new("/t/a.compact"));
            host.vfs_mut().set_overlay(file, src.to_string(), 1);
            host.generic_diagnostics(file)
        }

        // Correct arity: no diagnostic.
        assert!(
            diags("struct Box<T> { v: T; }\nexport circuit m(): Box<Field> { return default<Box<Field>>; }")
                .is_empty()
        );
        // Missing args (0 vs 1): the struct name appears in two independent
        // TypeRef positions here -- the circuit's return-type annotation and
        // the argument to `default<...>` -- each a genuine, separately
        // spanned arity violation, so two diagnostics are expected (one per
        // TypeRef, per the query's contract), both naming Box.
        let d = diags("struct Box<T> { v: T; }\nexport circuit m(): Box { return default<Box>; }");
        assert_eq!(d.len(), 2);
        for diag in &d {
            assert!(
                diag.message.contains("actual number 0")
                    && diag.message.contains("declared number 1")
                    && diag.message.contains("Box")
            );
        }
        // Wrong count (1 vs 2); again two TypeRef positions (return type +
        // `default<...>` argument).
        assert_eq!(
            diags("struct Pair<A, B> { a: A; b: B; }\nexport circuit m(): Pair<Field> { return default<Pair<Field>>; }").len(),
            2
        );
        // Args on a non-generic struct (1 vs 0); again two TypeRef positions.
        assert_eq!(
            diags("struct Point { x: Field; }\nexport circuit m(): Point<Field> { return default<Point<Field>>; }").len(),
            2
        );
        // Unresolved name suppresses (never a false positive).
        assert!(
            diags("export circuit m(): Nope<Field> { return default<Nope<Field>>; }").is_empty()
        );
    }
}
