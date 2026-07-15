//! Type inference (foundation slice) + type diagnostics.
//!
//! The inference entry point is `infer_circuit_returns`: one tracked query per
//! circuit body, typing the primitive-literal operand of each `return`. It
//! reads `item_tree`/`parsed` directly (single-file typing) — no resolution
//! queries are needed for the slice. The universe of expressions it types
//! grows one rule at a time. Incrementality is file-granular at the foundation
//! (every body reads the file's `parsed` green tree); finer input splitting is
//! a later concern, as noted in the foundation spec.

use std::sync::Arc;

use compactp_ast::AstNode;

use compactp_diagnostics::{Diagnostic, DiagnosticCode};

use crate::db::{Db, SourceText, item_tree, parsed};
use crate::ty::{Ty, TyKind, ty_display};

/// Lower a CST `Type` node to a `TyKind`. Only the primitives the foundation
/// models map to a concrete kind; everything else is `Unknown`.
///
/// Called by `type_diagnostics_query` to type a circuit's declared
/// return-type annotation for comparison against `infer_circuit_returns`.
pub(crate) fn type_node_kind(ty: &compactp_ast::Type) -> TyKind {
    use compactp_ast::Type;
    match ty {
        Type::Boolean(_) => TyKind::Boolean,
        Type::Field(_) => TyKind::Field,
        _ => TyKind::Unknown,
    }
}

/// The `TyKind` of a literal expression. A `true`/`false` token is `Boolean`;
/// numeric/string literals are `Unknown` at the foundation (the `Uint` lattice
/// and byte/string typing are later rules).
fn literal_ty_kind(lit: &compactp_ast::expr::LiteralExpr) -> TyKind {
    use compactp_syntax::SyntaxKind;
    let has = |k: SyntaxKind| {
        lit.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == k)
    };
    if has(SyntaxKind::TRUE_KW) || has(SyntaxKind::FALSE_KW) {
        TyKind::Boolean
    } else {
        TyKind::Unknown
    }
}

/// The `CircuitDef` CST node for the circuit at item-tree index `index`, found
/// by matching the symbol's `name_range` against each `CircuitDef`'s name
/// token range (covers nested circuits too). `None` if `index` is not a
/// circuit.
pub(crate) fn circuit_node_by_index(
    db: &dyn Db,
    src: SourceText,
    index: u32,
) -> Option<compactp_ast::CircuitDef> {
    let tree = item_tree(db, src);
    let sym = tree.symbols.get(index as usize)?;
    if sym.kind != crate::SymbolKind::Circuit {
        return None;
    }
    let name_range = sym.name_range;
    let root = compactp_syntax::SyntaxNode::new_root(parsed(db, src).green);
    root.descendants()
        .filter_map(compactp_ast::CircuitDef::cast)
        .find(|c| c.name().map(|n| n.text_range()) == Some(name_range))
}

/// Inference entry point for one circuit body: `(return-statement range,
/// TyKind)` for each `return` whose operand is a primitive literal the
/// foundation can type. Carries the `'static` `TyKind` (not `Ty<'db>`): salsa
/// forbids a tracked return of `Arc<[(_, Ty<'db>)]>` — see the Salsa lifetime
/// constraint note in this task. `TyKind` is `PartialEq`, so this query
/// backdates normally; `Ty` is interned downstream, at the display boundary.
#[salsa::tracked(returns(clone))]
pub fn infer_circuit_returns(
    db: &dyn Db,
    src: SourceText,
    circuit_index: u32,
) -> Arc<[(text_size::TextRange, TyKind)]> {
    let Some(circuit) = circuit_node_by_index(db, src, circuit_index) else {
        return Arc::from(Vec::new());
    };
    let Some(body) = circuit.body() else {
        return Arc::from(Vec::new());
    };
    let mut out = Vec::new();
    for stmt in body.stmts() {
        let compactp_ast::Stmt::Return(ret) = stmt else {
            continue;
        };
        let Some(compactp_ast::expr::Expr::Literal(lit)) = ret.value() else {
            continue;
        };
        let kind = literal_ty_kind(&lit);
        out.push((ret.syntax().text_range(), kind));
    }
    Arc::from(out)
}

/// Type diagnostics for `src` (foundation rule: primitive-literal return
/// mismatch). For each circuit with a declared return type the foundation
/// models (`Boolean`/`Field`), every primitive-literal `return` operand whose
/// `Ty` differs from the declared type yields one diagnostic. An `Unknown`
/// declared type or operand suppresses the rule (never a false positive).
///
/// `no_eq`: `Diagnostic` (external crate) has no `PartialEq`, so
/// `Arc<[Diagnostic]>` can't be compared for backdating — same rationale and
/// shape as `resolution_diagnostics_query` in `db.rs`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn type_diagnostics_query(db: &dyn Db, src: SourceText) -> Arc<[Diagnostic]> {
    let tree = item_tree(db, src);
    let mut diags = Vec::new();
    for (idx, sym) in tree
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == crate::SymbolKind::Circuit)
    {
        let idx = idx as u32;
        let Some(circuit) = circuit_node_by_index(db, src, idx) else {
            continue;
        };
        let declared = circuit
            .return_type()
            .map(|t| type_node_kind(&t))
            .unwrap_or(TyKind::Unknown);
        if declared == TyKind::Unknown {
            continue; // only fire on return types the foundation models
        }
        for (range, kind) in infer_circuit_returns(db, src, idx).iter() {
            let actual = *kind;
            if actual != TyKind::Unknown && actual != declared {
                diags.push(return_mismatch_diag(
                    db, actual, declared, &sym.name, *range,
                ));
            }
        }
    }
    Arc::from(diags)
}

/// The return-type-mismatch diagnostic. Wording tracks `compactc`'s
/// ("mismatch between actual return type X and declared return type Y of
/// circuit Z"); wording is not gated by the harness.
fn return_mismatch_diag(
    db: &dyn Db,
    actual: TyKind,
    declared: TyKind,
    circuit_name: &str,
    span: text_size::TextRange,
) -> Diagnostic {
    let actual = ty_display(db, Ty::new(db, actual));
    let declared = ty_display(db, Ty::new(db, declared));
    Diagnostic::error(
        DiagnosticCode::new("E", 3001),
        format!(
            "mismatch between actual return type {actual} and declared return type {declared} of circuit {circuit_name}"
        ),
        span,
    )
}

impl crate::AnalysisHost {
    /// Type diagnostics for `file` (foundation: primitive-literal return
    /// mismatch). Thin bridge over the tracked `type_diagnostics_query`,
    /// mirroring `resolution_diagnostics`. Single-file typing: no `FileDeps`/
    /// `Workspace` inputs are needed for the slice. Editor surfacing (Problems
    /// panel + toggle) is wired in v2b.final, which consumes this method.
    pub fn type_diagnostics(&mut self, file: crate::FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        type_diagnostics_query(self.db_ref(), src).to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{CompactDatabase, SourceText};
    use crate::ty::TyKind;
    use std::sync::Arc;

    fn circuit_index(db: &dyn crate::db::Db, src: SourceText, name: &str) -> u32 {
        let tree = crate::db::item_tree(db, src);
        tree.symbols
            .iter()
            .position(|s| s.name == name && s.kind == crate::SymbolKind::Circuit)
            .expect("circuit present") as u32
    }

    #[test]
    fn boolean_literal_return_is_typed_boolean() {
        let db = CompactDatabase::default();
        let src = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Field { return true; }"),
        );
        let idx = circuit_index(&db, src, "foo");
        let returns = infer_circuit_returns(&db, src, idx);
        assert_eq!(returns.len(), 1);
        assert_eq!(returns[0].1, TyKind::Boolean);
    }

    #[test]
    fn non_primitive_literal_return_is_unknown() {
        let db = CompactDatabase::default();
        // A numeric literal: the Uint lattice is a later rule, so it is Unknown
        // at the foundation.
        let src = SourceText::new(&db, Arc::from("export circuit foo(): Field { return 1; }"));
        let idx = circuit_index(&db, src, "foo");
        let returns = infer_circuit_returns(&db, src, idx);
        assert_eq!(returns.len(), 1);
        assert_eq!(returns[0].1, TyKind::Unknown);
    }

    #[test]
    fn type_node_kind_maps_primitives() {
        // Exercised indirectly, but assert the mapping directly via a parse.
        let db = CompactDatabase::default();
        let src = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Boolean { return true; }"),
        );
        let idx = circuit_index(&db, src, "foo");
        let circuit = circuit_node_by_index(&db, src, idx).expect("circuit node");
        assert_eq!(
            type_node_kind(&circuit.return_type().unwrap()),
            TyKind::Boolean
        );
    }

    #[test]
    fn return_mismatch_emits_one_type_diagnostic() {
        let db = CompactDatabase::default();
        let src = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Field { return true; }"),
        );
        let diags = type_diagnostics_query(&db, src);
        assert_eq!(diags.len(), 1, "one mismatch");
        assert_eq!(diags[0].code.prefix, "E");
        assert_eq!(diags[0].code.number, 3001);
        assert!(
            diags[0].message.contains("Boolean") && diags[0].message.contains("Field"),
            "message names both types: {:?}",
            diags[0].message
        );
    }

    #[test]
    fn matching_return_emits_no_diagnostic() {
        let db = CompactDatabase::default();
        let src = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Boolean { return true; }"),
        );
        assert!(type_diagnostics_query(&db, src).is_empty());
    }

    #[test]
    fn unknown_operand_or_expectation_suppresses() {
        let db = CompactDatabase::default();
        // Unknown declared type (a Uint): no rule fires yet.
        let a = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Uint<0..7> { return true; }"),
        );
        assert!(type_diagnostics_query(&db, a).is_empty());
        // Unknown operand (numeric literal) into a modeled type: suppressed.
        let b = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Boolean { return 1; }"),
        );
        assert!(type_diagnostics_query(&db, b).is_empty());
    }

    #[test]
    fn host_bridge_reports_return_mismatch() {
        use crate::AnalysisHost;
        use std::path::Path;
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(Path::new("/t/foo.compact"));
        host.vfs_mut().set_overlay(
            file,
            "export circuit foo(): Field { return true; }".to_string(),
            1,
        );
        let diags = host.type_diagnostics(file);
        assert_eq!(diags.len(), 1);
    }
}
