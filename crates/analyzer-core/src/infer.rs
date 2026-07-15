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

use crate::db::{Db, SourceText, item_tree, parsed};
use crate::ty::TyKind;

/// Lower a CST `Type` node to a `TyKind`. Only the primitives the foundation
/// models map to a concrete kind; everything else is `Unknown`.
///
/// Not yet called outside this module's tests: Task 5 (the return-mismatch
/// diagnostic) is the first non-test caller, typing a circuit's declared
/// return-type annotation for comparison against `infer_circuit_returns`.
#[allow(dead_code)]
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
}
