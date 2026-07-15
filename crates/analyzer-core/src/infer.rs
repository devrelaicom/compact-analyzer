//! Type inference (foundation slice) + type diagnostics.
//!
//! The inference entry point is `infer_circuit_returns`: one tracked query per
//! circuit body, typing each `return` operand the checker models (a primitive
//! literal — `Boolean`/`Uint`/`Field` — or a cast, typed as its target). It
//! reads `item_tree`/`parsed` directly (single-file typing) — no resolution
//! queries are needed for the slice. The universe of expressions it types
//! grows one rule at a time. Incrementality is file-granular at the foundation
//! (every body reads the file's `parsed` green tree); finer input splitting is
//! a later concern, as noted in the foundation spec.

use std::sync::Arc;

use compactp_ast::AstNode;

use compactp_diagnostics::{Diagnostic, DiagnosticCode};

use crate::db::{Db, SourceText, item_tree, parsed};
use crate::ty::{Ty, TyKind, can_cast, is_subtype, ty_display};

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
        Type::Uint(u) => uint_bound(u),
        Type::Bytes(b) => bytes_size(b),
        Type::Tuple(t) => {
            let elems: Vec<TyKind> = t.element_types().map(|e| type_node_kind(&e)).collect();
            TyKind::Tuple(std::sync::Arc::from(elems))
        }
        Type::Vector(v) => {
            let elem = v
                .element_type()
                .map(|e| type_node_kind(&e))
                .unwrap_or(TyKind::Unknown);
            match v
                .size()
                .and_then(|s| parse_int_literal(&s.syntax().text().to_string()))
            {
                Some(n) if n <= u32::MAX as u128 => {
                    TyKind::Vector(n as u32, std::sync::Arc::new(elem))
                }
                _ => TyKind::Unknown, // non-literal / overflowing size -> suppress
            }
        }
        _ => TyKind::Unknown,
    }
}

/// Parse a `Uint` size or an integer-literal token's text to a `u128`.
/// Accepts decimal, `0x`/`0X` hex, `0o`/`0O` octal, and `0b`/`0B` binary.
/// `None` if the text is not a plain integer literal (e.g. a generic numeric
/// parameter identifier) or overflows `u128`.
fn parse_int_literal(text: &str) -> Option<u128> {
    let t = text.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u128::from_str_radix(hex, 16).ok()
    } else if let Some(oct) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        u128::from_str_radix(oct, 8).ok()
    } else if let Some(bin) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        u128::from_str_radix(bin, 2).ok()
    } else {
        t.parse::<u128>().ok()
    }
}

/// Lower a `Uint<...>` annotation to its canonical `TyKind::Uint(exclusive
/// upper bound)`, per the extracted rule. A malformed or non-literal annotation
/// lowers to `Unknown` (conservatively suppresses the lattice — corpus-safe,
/// since accepted files have no malformed Uints). `Uint<k>` denotes `[0, 2^k)`
/// for `1 <= k <= 248`; `Uint<lo..hi>` denotes `[lo, hi)` with `lo == 0`,
/// `hi >= 1`.
fn uint_bound(u: &compactp_ast::UintType) -> TyKind {
    let sizes: Vec<compactp_ast::TypeSize> = u.sizes().collect();
    match sizes.as_slice() {
        // Bit-width form: Uint<k>.
        [k] => match parse_int_literal(&k.syntax().text().to_string()) {
            Some(k) if (1..=248).contains(&k) => {
                let bound = if k < 128 { Some(1u128 << k) } else { None };
                TyKind::Uint(bound)
            }
            _ => TyKind::Unknown, // out of range or non-literal
        },
        // Range form: Uint<lo..hi>.
        [lo, hi] => {
            if parse_int_literal(&lo.syntax().text().to_string()) != Some(0) {
                return TyKind::Unknown; // range start must be 0 (else malformed)
            }
            match parse_int_literal(&hi.syntax().text().to_string()) {
                Some(0) => TyKind::Unknown, // range end must be >= 1
                Some(hi) => TyKind::Uint(Some(hi)),
                None => TyKind::Uint(None), // hi is huge (>= 2^128) or non-literal-huge
            }
        }
        _ => TyKind::Unknown,
    }
}

/// Lower a `Bytes<n>` annotation to `TyKind::Bytes(n)`. Byte counts are small,
/// so the length is a `u32`; a non-literal size (a generic numeric parameter)
/// or one overflowing `u32` lowers to `Unknown` (conservatively suppresses —
/// corpus-safe, since accepted files with a concrete `Bytes<n>` always have a
/// literal, small `n`). Reuses `parse_int_literal` (decimal/hex/octal/binary).
fn bytes_size(b: &compactp_ast::BytesType) -> TyKind {
    match b
        .size()
        .and_then(|s| parse_int_literal(&s.syntax().text().to_string()))
    {
        Some(n) if n <= u32::MAX as u128 => TyKind::Bytes(n as u32),
        _ => TyKind::Unknown,
    }
}

/// The `TyKind` of a literal expression. `true`/`false` -> `Boolean`; a decimal,
/// hex, octal, or binary integer literal `n` -> `TyKind::Uint(Some(n+1))` (the
/// minimal range `[0, n]`), or `Uint(None)` when `n+1` overflows `u128`; other
/// literals (string, etc.) -> `Unknown`.
fn literal_ty_kind(lit: &compactp_ast::expr::LiteralExpr) -> TyKind {
    use compactp_syntax::SyntaxKind;
    let tokens: Vec<_> = lit
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .collect();
    if tokens
        .iter()
        .any(|t| matches!(t.kind(), SyntaxKind::TRUE_KW | SyntaxKind::FALSE_KW))
    {
        return TyKind::Boolean;
    }
    if let Some(tok) = tokens.iter().find(|t| {
        matches!(
            t.kind(),
            SyntaxKind::INT_LIT | SyntaxKind::HEX_LIT | SyntaxKind::OCT_LIT | SyntaxKind::BIN_LIT
        )
    }) {
        // n -> minimal range [0, n] = exclusive upper n+1. Overflow -> None
        // (still a Uint; e.g. a 2^200 literal is Uint(None) <: Field).
        let bound = parse_int_literal(tok.text()).and_then(|n| n.checked_add(1));
        return TyKind::Uint(bound);
    }
    TyKind::Unknown
}

/// The result `TyKind` of an expression the checker models: a primitive
/// literal (its own type), or a cast `e as T` (its *target* type `T`, per the
/// extracted rule — a cast's result is exactly its annotation, independent of
/// the operand). Any other expression -> `Unknown` (suppresses). Cast operands
/// nest naturally (an `(e as A) as B` types to `B`).
fn expr_ty_kind(expr: &compactp_ast::expr::Expr) -> TyKind {
    use compactp_ast::expr::Expr;
    match expr {
        Expr::Literal(lit) => literal_ty_kind(lit),
        Expr::Cast(cast) => cast
            .ty()
            .map(|t| type_node_kind(&t))
            .unwrap_or(TyKind::Unknown),
        Expr::Array(arr) => {
            // A spread element makes arity/types indeterminate -> Unknown.
            if arr
                .syntax()
                .children()
                .any(|c| c.kind() == compactp_syntax::SyntaxKind::SPREAD_EXPR)
            {
                return TyKind::Unknown;
            }
            let elems: Vec<TyKind> = arr
                .syntax()
                .children()
                .filter_map(compactp_ast::expr::Expr::cast)
                .map(|e| expr_ty_kind(&e))
                .collect();
            TyKind::Tuple(std::sync::Arc::from(elems))
        }
        _ => TyKind::Unknown,
    }
}

/// If `cast` is an `src as tgt` the checker can attribute as **illegal**,
/// returns `(src, tgt)` for the diagnostic. The operand's source type must be
/// known — a primitive-literal or (nested) cast operand is typed here (a
/// parameter / local / arbitrary expression types to `Unknown`, and
/// `can_cast(Unknown, _)` is
/// `true`, so it suppresses). The operand is the `CAST_EXPR`'s `Expr` child
/// (the target `Type` child does not cast to `Expr`).
fn cast_error(cast: &compactp_ast::expr::CastExpr) -> Option<(TyKind, TyKind)> {
    let tgt = type_node_kind(&cast.ty()?);
    let operand = cast
        .syntax()
        .children()
        .find_map(compactp_ast::expr::Expr::cast)?;
    let src = expr_ty_kind(&operand);
    (!can_cast(&src, &tgt)).then_some((src, tgt))
}

/// The illegal-cast diagnostic (`E3002`). Wording tracks compactc's
/// ("cannot cast from type X to type Y"); wording is not gated by the harness.
fn cast_error_diag(
    db: &dyn Db,
    src: TyKind,
    tgt: TyKind,
    span: text_size::TextRange,
) -> Diagnostic {
    let src = ty_display(db, Ty::new(db, src));
    let tgt = ty_display(db, Ty::new(db, tgt));
    Diagnostic::error(
        DiagnosticCode::new("E", 3002),
        format!("cannot cast from type {src} to type {tgt}"),
        span,
    )
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

/// `(item-tree index, CircuitDef)` for every circuit in `src`, built with a
/// single walk of the file's green tree. Replaces per-circuit
/// `circuit_node_by_index` calls (each of which re-walked the tree) in the hot
/// diagnostics path. Association is by the symbol's `name_range` matching the
/// `CircuitDef`'s name-token range — identical to `circuit_node_by_index`, so
/// nested circuits are covered the same way.
fn circuit_node_map(db: &dyn Db, src: SourceText) -> Vec<(u32, compactp_ast::CircuitDef)> {
    let tree = item_tree(db, src);
    let index_of: std::collections::HashMap<text_size::TextRange, u32> = tree
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == crate::SymbolKind::Circuit)
        .map(|(i, s)| (s.name_range, i as u32))
        .collect();
    let root = compactp_syntax::SyntaxNode::new_root(parsed(db, src).green);
    root.descendants()
        .filter_map(compactp_ast::CircuitDef::cast)
        .filter_map(|c| {
            let range = c.name().map(|n| n.text_range())?;
            index_of.get(&range).map(|&idx| (idx, c))
        })
        .collect()
}

/// One `return` operand the checker models: its span, its result type, and —
/// when the operand is an illegal cast the checker can attribute — the
/// `(src, tgt)` of that cast. Both the inference entry point and the
/// diagnostics query read this single per-body walk (preserving the v2b.3
/// one-walk property).
struct ReturnTy {
    span: text_size::TextRange,
    /// Result type of the operand (a cast's result is its target type).
    kind: TyKind,
    /// `Some((src, tgt))` iff the operand is an illegal `src as tgt` cast with
    /// a known (literal) source. The return-lattice check is skipped for such
    /// operands — compactc reports only the cast error.
    cast_error: Option<(TyKind, TyKind)>,
}

/// Types each `return` operand the checker models (primitive literals and
/// casts) in a circuit body. One green-tree walk per consumer.
fn circuit_returns(circuit: &compactp_ast::CircuitDef) -> Vec<ReturnTy> {
    let Some(body) = circuit.body() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for stmt in body.stmts() {
        let compactp_ast::Stmt::Return(ret) = stmt else {
            continue;
        };
        let Some(value) = ret.value() else {
            continue;
        };
        let cast_error = match &value {
            compactp_ast::expr::Expr::Cast(cast) => cast_error(cast),
            _ => None,
        };
        // Only model literals and casts; skip operands the checker can't type.
        if matches!(
            value,
            compactp_ast::expr::Expr::Literal(_)
                | compactp_ast::expr::Expr::Cast(_)
                | compactp_ast::expr::Expr::Array(_)
        ) {
            out.push(ReturnTy {
                span: ret.syntax().text_range(),
                kind: expr_ty_kind(&value),
                cast_error,
            });
        }
    }
    out
}

/// Inference entry point for one circuit body: `(return-statement range,
/// TyKind)` for each `return` whose operand is a primitive literal or cast the
/// checker can type. Carries the `'static` `TyKind` (not `Ty<'db>`): salsa
/// forbids a tracked return of `Arc<[(_, Ty<'db>)]>`. `Ty` is interned
/// downstream, at the display boundary.
#[salsa::tracked(returns(clone))]
pub fn infer_circuit_returns(
    db: &dyn Db,
    src: SourceText,
    circuit_index: u32,
) -> Arc<[(text_size::TextRange, TyKind)]> {
    let Some(circuit) = circuit_node_by_index(db, src, circuit_index) else {
        return Arc::from(Vec::new());
    };
    Arc::from(
        circuit_returns(&circuit)
            .into_iter()
            .map(|r| (r.span, r.kind))
            .collect::<Vec<_>>(),
    )
}

/// Type diagnostics for `src`: an illegal-cast return operand (`E3002`), and
/// a return-mismatch (`E3001`) against a declared return type the checker
/// models (`Boolean`/`Field`/`Uint`/`Bytes`). For each circuit, every `return`
/// operand the checker can type (a primitive literal or a cast) is checked in
/// turn: an illegal cast is reported on its own; otherwise a mismatch between
/// the operand's type and the declared return type fires. An `Unknown`
/// declared type or operand suppresses the corresponding rule (never a false
/// positive).
///
/// `no_eq`: `Diagnostic` (external crate) has no `PartialEq`, so
/// `Arc<[Diagnostic]>` can't be compared for backdating — same rationale and
/// shape as `resolution_diagnostics_query` in `db.rs`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn type_diagnostics_query(db: &dyn Db, src: SourceText) -> Arc<[Diagnostic]> {
    let tree = item_tree(db, src);
    let mut diags = Vec::new();
    for (idx, circuit) in circuit_node_map(db, src) {
        let declared = circuit
            .return_type()
            .map(|t| type_node_kind(&t))
            .unwrap_or(TyKind::Unknown);
        let name = &tree.symbols[idx as usize].name;
        for ret in circuit_returns(&circuit) {
            // An illegal cast is reported on its own (compactc reports only the
            // cast error) and suppresses the return-mismatch for that operand —
            // regardless of whether the declared return type is modeled.
            if let Some((s, t)) = ret.cast_error {
                diags.push(cast_error_diag(db, s, t, ret.span));
                continue;
            }
            // Return-mismatch only fires on a return type the checker models.
            if declared != TyKind::Unknown && !is_subtype(&ret.kind, &declared) {
                diags.push(return_mismatch_diag(db, ret.kind, declared.clone(), name, ret.span));
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
    /// mismatch) plus generic-specialization (arity) diagnostics. Thin
    /// bridge over the tracked `type_diagnostics_query`, mirroring
    /// `resolution_diagnostics`, merged with `generic_diagnostics` so this
    /// is the single type-diagnostics surface the differential harness (and,
    /// later, the editor) consumes. Single-file typing: no `FileDeps`/
    /// `Workspace` inputs are needed for the `type_diagnostics_query` slice.
    /// Editor surfacing (Problems panel + toggle) is wired in v2b.final,
    /// which consumes this method.
    pub fn type_diagnostics(&mut self, file: crate::FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        let mut diags = type_diagnostics_query(self.db_ref(), src).to_vec();
        diags.extend(self.generic_diagnostics(file));
        diags
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
    fn integer_literal_return_is_typed_uint() {
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("export circuit foo(): Field { return 1; }"));
        let idx = circuit_index(&db, src, "foo");
        let returns = infer_circuit_returns(&db, src, idx);
        assert_eq!(returns.len(), 1);
        // 1 -> minimal range [0, 1] -> exclusive upper 2.
        assert_eq!(returns[0].1, TyKind::Uint(Some(2)));
    }

    #[test]
    fn integer_literal_types_to_minimal_uint() {
        let db = CompactDatabase::default();
        // 5 -> Uint<0..6>
        let a = SourceText::new(&db, Arc::from("export circuit f(): Field { return 5; }"));
        let ia = circuit_index(&db, a, "f");
        assert_eq!(
            infer_circuit_returns(&db, a, ia)[0].1,
            TyKind::Uint(Some(6))
        );
        // 0 -> Uint<0..1>
        let b = SourceText::new(&db, Arc::from("export circuit f(): Field { return 0; }"));
        let ib = circuit_index(&db, b, "f");
        assert_eq!(
            infer_circuit_returns(&db, b, ib)[0].1,
            TyKind::Uint(Some(1))
        );
        // hex 0xFF -> Uint<0..256>
        let c = SourceText::new(&db, Arc::from("export circuit f(): Field { return 0xFF; }"));
        let ic = circuit_index(&db, c, "f");
        assert_eq!(
            infer_circuit_returns(&db, c, ic)[0].1,
            TyKind::Uint(Some(256))
        );
        // octal 0o10 (8) -> Uint<0..9>
        let d = SourceText::new(&db, Arc::from("export circuit f(): Field { return 0o10; }"));
        let id = circuit_index(&db, d, "f");
        assert_eq!(
            infer_circuit_returns(&db, d, id)[0].1,
            TyKind::Uint(Some(9))
        );
        // binary 0b1010 (10) -> Uint<0..11>
        let e = SourceText::new(
            &db,
            Arc::from("export circuit f(): Field { return 0b1010; }"),
        );
        let ie = circuit_index(&db, e, "f");
        assert_eq!(
            infer_circuit_returns(&db, e, ie)[0].1,
            TyKind::Uint(Some(11))
        );
    }

    #[test]
    fn tuple_and_vector_annotations_lower() {
        use std::sync::Arc;
        let db = CompactDatabase::default();
        // [Uint<8>, Boolean] -> Tuple([Uint(256), Boolean])
        let a = SourceText::new(&db, Arc::from("export circuit f(): [Uint<8>, Boolean] { }"));
        let ia = circuit_index(&db, a, "f");
        let ca = circuit_node_by_index(&db, a, ia).unwrap();
        assert_eq!(
            type_node_kind(&ca.return_type().unwrap()),
            TyKind::Tuple(Arc::from(vec![TyKind::Uint(Some(256)), TyKind::Boolean]))
        );
        // Vector<3, Uint<8>> -> Vector(3, Uint(256))
        let b = SourceText::new(&db, Arc::from("export circuit f(): Vector<3, Uint<8>> { }"));
        let ib = circuit_index(&db, b, "f");
        let cb = circuit_node_by_index(&db, b, ib).unwrap();
        assert_eq!(
            type_node_kind(&cb.return_type().unwrap()),
            TyKind::Vector(3, Arc::new(TyKind::Uint(Some(256))))
        );
        // Empty tuple -> unit.
        let c = SourceText::new(&db, Arc::from("export circuit f(): [] { }"));
        let ic = circuit_index(&db, c, "f");
        let cc = circuit_node_by_index(&db, c, ic).unwrap();
        assert_eq!(type_node_kind(&cc.return_type().unwrap()), TyKind::Tuple(Arc::from(vec![])));
        // Non-literal vector size -> Unknown (suppresses).
        let d = SourceText::new(&db, Arc::from("export circuit f(): Vector<N, Uint<8>> { }"));
        let id = circuit_index(&db, d, "f");
        let cd = circuit_node_by_index(&db, d, id).unwrap();
        assert_eq!(type_node_kind(&cd.return_type().unwrap()), TyKind::Unknown);
    }

    #[test]
    fn tuple_literal_return_typed_and_checked() {
        use std::sync::Arc;
        let db = CompactDatabase::default();
        // return [1, true] types to Tuple([Uint(2), Boolean]).
        let a = SourceText::new(&db, Arc::from("export circuit f(): [Uint<8>, Boolean] { return [1, true]; }"));
        let ia = circuit_index(&db, a, "f");
        assert_eq!(
            infer_circuit_returns(&db, a, ia)[0].1,
            TyKind::Tuple(Arc::from(vec![TyKind::Uint(Some(2)), TyKind::Boolean]))
        );
        // In-range literal tuple into declared tuple -> accept.
        assert!(type_diagnostics_query(&db, a).is_empty());
        // Tuple literal into a Vector of equal length -> accept (the equivalence).
        let b = SourceText::new(&db, Arc::from("export circuit f(): Vector<3, Uint<8>> { return [1, 2, 3]; }"));
        assert!(type_diagnostics_query(&db, b).is_empty());
        // Element out of range -> reject (300 not <: Uint<8>).
        let c = SourceText::new(&db, Arc::from("export circuit f(): Vector<3, Uint<8>> { return [1, 2, 300]; }"));
        assert_eq!(type_diagnostics_query(&db, c).len(), 1);
        // Arity mismatch -> reject.
        let d = SourceText::new(&db, Arc::from("export circuit f(): [Uint<8>] { return [1, true]; }"));
        assert_eq!(type_diagnostics_query(&db, d).len(), 1);
        // A spread element makes the literal indeterminate -> Unknown -> suppress.
        let e = SourceText::new(&db, Arc::from("export circuit f(x: [Uint<8>, Uint<8>]): [Uint<8>, Uint<8>] { return [...x]; }"));
        assert!(type_diagnostics_query(&db, e).is_empty());
    }

    #[test]
    fn literal_into_uint_range_accept_and_reject() {
        let db = CompactDatabase::default();
        // In range: 5 (Uint<0..6>) <: Uint<0..10> -> no diagnostic
        let ok = SourceText::new(
            &db,
            Arc::from("export circuit f(): Uint<0..10> { return 5; }"),
        );
        assert!(type_diagnostics_query(&db, ok).is_empty());
        // Out of range: 11 (Uint<0..12>) not <: Uint<0..10> -> one diagnostic
        let bad = SourceText::new(
            &db,
            Arc::from("export circuit f(): Uint<0..10> { return 11; }"),
        );
        let diags = type_diagnostics_query(&db, bad);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("Uint<0..12>") && diags[0].message.contains("Uint<0..10>"),
            "message uses normalized Uint display: {:?}",
            diags[0].message
        );
        // Uint literal into Field is always fine.
        let field = SourceText::new(&db, Arc::from("export circuit f(): Field { return 5; }"));
        assert!(type_diagnostics_query(&db, field).is_empty());
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
    fn uint_annotation_lowers_to_bound() {
        let db = CompactDatabase::default();
        // Uint<8> -> exclusive upper 2^8 = 256
        let a = SourceText::new(&db, Arc::from("export circuit f(): Uint<8> { }"));
        let ia = circuit_index(&db, a, "f");
        let ca = circuit_node_by_index(&db, a, ia).unwrap();
        assert_eq!(
            type_node_kind(&ca.return_type().unwrap()),
            TyKind::Uint(Some(256))
        );
        // Uint<0..10> -> exclusive upper 10
        let b = SourceText::new(&db, Arc::from("export circuit f(): Uint<0..10> { }"));
        let ib = circuit_index(&db, b, "f");
        let cb = circuit_node_by_index(&db, b, ib).unwrap();
        assert_eq!(
            type_node_kind(&cb.return_type().unwrap()),
            TyKind::Uint(Some(10))
        );
        // Malformed (non-zero start) -> Unknown (suppresses; not our diagnostic)
        let c = SourceText::new(&db, Arc::from("export circuit f(): Uint<3..10> { }"));
        let ic = circuit_index(&db, c, "f");
        let cc = circuit_node_by_index(&db, c, ic).unwrap();
        assert_eq!(type_node_kind(&cc.return_type().unwrap()), TyKind::Unknown);
        // Width beyond 127 -> not exactly representable
        let d = SourceText::new(&db, Arc::from("export circuit f(): Uint<200> { }"));
        let id = circuit_index(&db, d, "f");
        let cd = circuit_node_by_index(&db, d, id).unwrap();
        assert_eq!(
            type_node_kind(&cd.return_type().unwrap()),
            TyKind::Uint(None)
        );
    }

    #[test]
    fn bytes_annotation_lowers_to_size() {
        let db = CompactDatabase::default();
        // Bytes<32> -> Bytes(32)
        let a = SourceText::new(&db, Arc::from("export circuit f(): Bytes<32> { }"));
        let ia = circuit_index(&db, a, "f");
        let ca = circuit_node_by_index(&db, a, ia).unwrap();
        assert_eq!(
            type_node_kind(&ca.return_type().unwrap()),
            TyKind::Bytes(32)
        );
        // Bytes<1>
        let b = SourceText::new(&db, Arc::from("export circuit f(): Bytes<1> { }"));
        let ib = circuit_index(&db, b, "f");
        let cb = circuit_node_by_index(&db, b, ib).unwrap();
        assert_eq!(type_node_kind(&cb.return_type().unwrap()), TyKind::Bytes(1));
        // Generic/non-literal size -> Unknown (suppresses; not our diagnostic)
        let c = SourceText::new(&db, Arc::from("export circuit f(): Bytes<N> { }"));
        let ic = circuit_index(&db, c, "f");
        let cc = circuit_node_by_index(&db, c, ic).unwrap();
        assert_eq!(type_node_kind(&cc.return_type().unwrap()), TyKind::Unknown);
    }

    #[test]
    fn uint_literal_into_bytes_return_rejects() {
        let db = CompactDatabase::default();
        // return 5 (Uint<0..6>) into Bytes<32>: Uint not <: Bytes -> one diagnostic
        // (matches compactc: "mismatch ... actual Uint<0..6> ... declared Bytes<32>").
        let src = SourceText::new(
            &db,
            Arc::from("export circuit f(): Bytes<32> { return 5; }"),
        );
        let diags = type_diagnostics_query(&db, src);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("Uint<0..6>") && diags[0].message.contains("Bytes<32>"),
            "message uses normalized display: {:?}",
            diags[0].message
        );
    }

    #[test]
    fn unknown_operand_or_expectation_suppresses() {
        let db = CompactDatabase::default();
        // Boolean literal into a Uint return: rejects (Boolean </: Uint).
        let a = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Uint<0..7> { return true; }"),
        );
        assert_eq!(type_diagnostics_query(&db, a).len(), 1);
        // Integer literal into a Boolean return: rejects (Uint </: Boolean),
        // matching compactc (`Boolean return 5` is a type error).
        let b = SourceText::new(
            &db,
            Arc::from("export circuit foo(): Boolean { return 1; }"),
        );
        assert_eq!(type_diagnostics_query(&db, b).len(), 1);
        // A return type the checker does not model (a named type reference)
        // still suppresses. (Vector/Tuple are now modeled — see
        // `tuple_and_vector_annotations_lower` / `tuple_literal_return_typed_and_checked`.)
        let c = SourceText::new(
            &db,
            Arc::from("export circuit foo(): MyType { return true; }"),
        );
        assert!(type_diagnostics_query(&db, c).is_empty());
    }

    #[test]
    fn multi_circuit_reports_only_the_mismatched_one() {
        let db = CompactDatabase::default();
        let src = SourceText::new(
            &db,
            Arc::from(
                "export circuit good(): Boolean { return true; }\n\
                 export circuit bad(): Field { return true; }",
            ),
        );
        let diags = type_diagnostics_query(&db, src);
        assert_eq!(diags.len(), 1, "only `bad` mismatches");
        assert!(
            diags[0].message.contains("bad"),
            "diagnostic names the right circuit: {:?}",
            diags[0].message
        );
    }

    #[test]
    fn cast_return_types_to_target() {
        let db = CompactDatabase::default();
        // x as Uint<8> -> result type Uint<8> (exclusive upper 256), regardless of operand.
        let a = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Field): Field { return x as Uint<8>; }"),
        );
        let ia = circuit_index(&db, a, "f");
        assert_eq!(
            infer_circuit_returns(&db, a, ia)[0].1,
            TyKind::Uint(Some(256))
        );
        // x as Bytes<32> -> result type Bytes(32).
        let b = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Field): Bytes<32> { return x as Bytes<32>; }"),
        );
        let ib = circuit_index(&db, b, "f");
        assert_eq!(infer_circuit_returns(&db, b, ib)[0].1, TyKind::Bytes(32));
    }

    #[test]
    fn cast_flows_through_return_lattice() {
        let db = CompactDatabase::default();
        // Cast into a WIDER Uint: Uint<8> <: Uint<16> -> accept.
        let ok = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Field): Uint<16> { return x as Uint<8>; }"),
        );
        assert!(type_diagnostics_query(&db, ok).is_empty());
        // Cast into a NARROWER Uint: Uint<8> not <: Uint<0..10> -> reject.
        let bad = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Field): Uint<0..10> { return x as Uint<8>; }"),
        );
        let diags = type_diagnostics_query(&db, bad);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("Uint<8>") && diags[0].message.contains("Uint<0..10>"),
            "message: {:?}",
            diags[0].message
        );
        // Legal same-size Bytes cast into a DIFFERENT-size Bytes return: reject
        // (cast is legal identity -> Bytes<4>; Bytes<4> not <: Bytes<32>).
        let bytes = SourceText::new(
            &db,
            Arc::from("export circuit f(x: Bytes<4>): Bytes<32> { return x as Bytes<4>; }"),
        );
        assert_eq!(type_diagnostics_query(&db, bytes).len(), 1);
    }

    #[test]
    fn illegal_cast_from_literal_emits_diagnostic() {
        let db = CompactDatabase::default();
        // true as Bytes<32>: Boolean -> Bytes is illegal -> E3002.
        let src = SourceText::new(
            &db,
            Arc::from("export circuit f(): Bytes<32> { return true as Bytes<32>; }"),
        );
        let diags = type_diagnostics_query(&db, src);
        assert_eq!(diags.len(), 1, "one cast error");
        assert_eq!(diags[0].code.prefix, "E");
        assert_eq!(diags[0].code.number, 3002);
        assert!(
            diags[0].message.contains("cannot cast")
                && diags[0].message.contains("Boolean")
                && diags[0].message.contains("Bytes<32>"),
            "message: {:?}",
            diags[0].message
        );
    }

    #[test]
    fn legal_literal_casts_emit_nothing() {
        let db = CompactDatabase::default();
        // Uint literal casts are all legal; Boolean -> non-Bytes is legal.
        for code in [
            "export circuit f(): Bytes<32> { return 5 as Bytes<32>; }", // Uint -> Bytes ok
            "export circuit f(): Boolean { return 5 as Boolean; }",     // Uint -> Boolean ok
            "export circuit f(): Uint<8> { return true as Uint<8>; }",  // Boolean -> Uint ok
        ] {
            let src = SourceText::new(&db, Arc::from(code));
            assert!(
                type_diagnostics_query(&db, src).is_empty(),
                "expected no diagnostic for: {code}"
            );
        }
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

    #[test]
    fn type_diagnostics_includes_generic_arity() {
        use crate::AnalysisHost;
        use std::path::Path;
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(Path::new("/t/g.compact"));
        // Box<T> used with no args -> generic-arity diagnostic surfaces via
        // the merged type_diagnostics.
        host.vfs_mut().set_overlay(
            file,
            "struct Box<T> { v: T; }\nexport circuit m(): Box { return default<Box>; }".to_string(),
            1,
        );
        let diags = host.type_diagnostics(file);
        assert!(
            diags.iter().any(|d| d.code.number == 3003),
            "expected an E3003 generic-arity diagnostic, got: {:?}",
            diags.iter().map(|d| d.code.number).collect::<Vec<_>>()
        );
    }
}
