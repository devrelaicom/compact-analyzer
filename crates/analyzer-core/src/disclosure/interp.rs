//! Root discovery + type-driven source seeding (spec §3.1/§3.4, R0 rows
//! AB1, S2, S3). Turns a circuit/constructor declaration into a seeded `Env`
//! (param name -> tainted `Abs`) the interpreter (A3) starts walking from.
//!
//! NO interpreter walk here (`interp_expr`/`interp_stmt` are A3) — this
//! module is the seeding layer A3 consumes: `discover_roots` finds the
//! entry points, `seed_sources` seeds each one's parameter environment via
//! `abs_of_type`.
//!
//! Nothing in this module is called from `disclosure_diagnostics_query` yet
//! (scope: the query stays empty until A3 wires the interpreter walk in), so
//! every item here is unreachable from a live root outside its own unit
//! tests — hence the file-level `#![cfg_attr(not(test), expect(dead_code))]`
//! below, mirroring A1's reasoning for `abs.rs`. It goes unfulfilled the
//! moment A3 wires this module into a live caller, forcing its removal.
#![cfg_attr(not(test), expect(dead_code))]

use std::collections::HashMap;

use compactp_ast::AstNode;
use text_size::TextRange;

use crate::db::{Db, FileDeps, SourceText, Workspace, item_tree};
use crate::item_tree::{ItemTree, SymbolKind};
use crate::ty::TyKind;

use super::abs::{Abs, Witness, WitnessInfo};
use super::leaks::DisclosureSink;

/// A root's seeded parameter environment: variable name -> its `Abs`.
/// A3's interpreter looks up identifiers here when walking a root's body.
pub type Env = HashMap<String, Abs>;

/// One analysis root (spec §3, R0 S2/S3): an exported circuit or the file's
/// constructor. A3 walks each root's body, seeded by `seed_sources`.
/// Non-exported circuits are never roots — they're reached only through
/// call sites (v3b, out of scope here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Root {
    /// An exported circuit. `name` is threaded into each seeded param
    /// witness as `CircuitArg { function: name, .. }`; `symbol` is the
    /// index into `ItemTree::symbols` used to locate its `CircuitDef` CST
    /// node.
    Circuit { name: String, symbol: u32 },
    /// The file's constructor (S2). At most one per file. `symbol` locates
    /// its `ConstructorDef` CST node. A constructor argument never leaks via
    /// `return` (the constructor body can't return a value observable
    /// outside ledger writes) — an A3 interpreter detail, not modeled here.
    Constructor { symbol: u32 },
}

/// Discovers the analysis roots (spec §3): every exported circuit plus the
/// file's constructor (R0 S2/S3). Non-exported circuits are excluded; they
/// are analyzed only through call sites (v3b).
pub fn discover_roots(tree: &ItemTree) -> Vec<Root> {
    tree.symbols
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match s.kind {
            SymbolKind::Circuit if s.exported => Some(Root::Circuit {
                name: s.name.clone(),
                symbol: i as u32,
            }),
            SymbolKind::Constructor => Some(Root::Constructor { symbol: i as u32 }),
            _ => None,
        })
        .collect()
}

/// Declared-type -> seeded `Abs` shape (R0 AB1, spec §3.4 `default-value`).
/// struct/tuple -> `Multiple` (one child per field/element, each tracked
/// individually); vector/array -> `Single` (the element type, aggregated —
/// one child abstracts every element); everything else the foundation's
/// `TyKind` models as a scalar (`Boolean`, `Field`, `Uint`, `Bytes`, `Enum`)
/// -> `Atomic`, with `seed` placed at the leaf. `default-value`'s `talias`
/// arm (recurse on the aliased type) has no distinct case here: a
/// `Type::Ref` is already resolved past any alias by `type_kind_resolved`
/// (`infer.rs`) before it becomes a `TyKind`, so by the time `abs_of_type`
/// sees a `TyKind` there is no alias left to recurse through.
///
/// `Unknown` fails closed (spec §0): the shape can't be known, so this seeds
/// `Atomic` — never a silently untainted result — AND records an advisory
/// via `sink`. The advisory is anchored at the first seed witness's
/// declaration site; `seed` is empty only in a hypothetical direct call
/// (every real caller in this module threads a non-empty seed), so the
/// `TextRange::empty(0.into())` fallback is defensive, not a real path.
pub fn abs_of_type(sink: &mut DisclosureSink, kind: &TyKind, seed: &[Witness]) -> Abs {
    match kind {
        TyKind::Struct { fields, .. } => Abs::Multiple(
            fields
                .iter()
                .map(|(_, field_ty)| abs_of_type(sink, field_ty, seed))
                .collect(),
        ),
        TyKind::Tuple(elems) => Abs::Multiple(
            elems
                .iter()
                .map(|elem_ty| abs_of_type(sink, elem_ty, seed))
                .collect(),
        ),
        TyKind::Vector(_, elem) => Abs::Single(Box::new(abs_of_type(sink, elem, seed))),
        TyKind::Unknown => {
            let src = seed
                .first()
                .map(|w| w.src)
                .unwrap_or_else(|| TextRange::empty(0.into()));
            sink.emit_advisory(
                src,
                "parameter type could not be resolved to a known shape (an Unknown v2b \
                 type); treated as tainted rather than silently clean"
                    .to_string(),
            );
            Abs::Atomic(seed.to_vec())
        }
        TyKind::Boolean
        | TyKind::Field
        | TyKind::Uint(_)
        | TyKind::Bytes(_)
        | TyKind::Enum { .. } => Abs::Atomic(seed.to_vec()),
    }
}

/// Seeds a root's parameter environment (R0 S2/S3, spec §3.1): each
/// parameter gets an `Abs` shaped by its declared type (`abs_of_type`),
/// tainted at every leaf by a fresh witness — `ConstructorArg` for the
/// constructor, `CircuitArg { function, arg }` for an exported circuit.
/// Declared types are resolved via `type_kind_resolved` (`infer.rs`), which
/// lowers a named `Type::Ref` to its nominal `TyKind::Struct`/`Enum` (not
/// just the primitive/sequence surface `type_node_kind` covers) — so a
/// struct-typed parameter seeds `Multiple`, not `Unknown`.
///
/// Witness-*return* sources (S1) are NOT seeded here — those arise at
/// witness *call sites* during the interpreter walk (A3); only the two
/// declaration-time sources are seeded from a root's signature.
///
/// Returns an empty `Env` (defensively, never a panic) if `root`'s `symbol`
/// index doesn't resolve to the expected CST node — this should not happen
/// for a `Root` `discover_roots` produced from the same `src`.
pub fn seed_sources(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
    root: &Root,
    sink: &mut DisclosureSink,
) -> Env {
    let tree = item_tree(db, src);
    let ast_root = compactp_syntax::SyntaxNode::new_root(crate::db::parsed(db, src).green);

    let params: Vec<(String, TextRange, compactp_ast::Type)> = match root {
        Root::Circuit { symbol, .. } => {
            let Some(sym) = tree.symbols.get(*symbol as usize) else {
                return Env::new();
            };
            let Some(cd) = ast_root
                .descendants()
                .filter_map(compactp_ast::CircuitDef::cast)
                .find(|c| c.name().map(|n| n.text_range()) == Some(sym.name_range))
            else {
                return Env::new();
            };
            cd.params().filter_map(param_name_and_type).collect()
        }
        Root::Constructor { symbol } => {
            let Some(sym) = tree.symbols.get(*symbol as usize) else {
                return Env::new();
            };
            let Some(ctor) = ast_root
                .descendants()
                .filter_map(compactp_ast::ConstructorDef::cast)
                .find(|c| c.syntax().text_range() == sym.full_range)
            else {
                return Env::new();
            };
            ctor.params().filter_map(param_name_and_type).collect()
        }
    };

    let mut env = Env::new();
    for (name, name_range, ty_ast) in params {
        let kind = crate::infer::type_kind_resolved(db, file, src, fd, ws, &ty_ast);
        let info = match root {
            Root::Circuit { name: function, .. } => WitnessInfo::CircuitArg {
                function: function.clone(),
                arg: name.clone(),
            },
            Root::Constructor { .. } => WitnessInfo::ConstructorArg { arg: name.clone() },
        };
        let witness = Witness {
            uid: sink.next_witness_uid(),
            src: name_range,
            info,
            path: Vec::new(),
        };
        let abs = abs_of_type(sink, &kind, std::slice::from_ref(&witness));
        env.insert(name, abs);
    }
    env
}

/// A `Param`'s name (+ its declaration span, for the seeded witness's `src`)
/// and declared `Type` node, or `None` if it has no declared type or its
/// pattern isn't a simple identifier. The latter is structurally impossible
/// for a circuit/constructor `Param` today — the grammar's `param` rule
/// (`compactp_parser::grammar::patterns::param`) only ever builds an
/// `IDENT_PAT` there; tuple/struct destructuring patterns are `const`-binding
/// only — so this is a defensive fallback, never an observed path, kept so a
/// future grammar change fails closed (skips the param) instead of
/// panicking.
fn param_name_and_type(
    param: compactp_ast::Param,
) -> Option<(String, TextRange, compactp_ast::Type)> {
    let compactp_ast::Pat::Ident(ident) = param.pattern()? else {
        return None;
    };
    let name_tok = ident.name()?;
    let ty = param.ty()?;
    Some((name_tok.text().to_string(), name_tok.text_range(), ty))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::CompactDatabase;

    fn empty_ws(db: &CompactDatabase) -> Workspace {
        Workspace::new(
            db,
            None,
            Arc::new(std::collections::BTreeMap::new()),
            Arc::new(std::collections::BTreeMap::new()),
        )
    }

    fn witness(uid: u32) -> Witness {
        Witness {
            uid,
            src: TextRange::empty(0.into()),
            info: WitnessInfo::CircuitArg {
                function: "f".into(),
                arg: "x".into(),
            },
            path: Vec::new(),
        }
    }

    #[test]
    fn abs_of_type_builds_struct_multiple_array_single() {
        let mut sink = DisclosureSink::new();
        let w = witness(0);

        // Scalar -> Atomic, seed at the leaf.
        let scalar = abs_of_type(&mut sink, &TyKind::Field, std::slice::from_ref(&w));
        assert_eq!(scalar, Abs::Atomic(vec![w.clone()]));

        // Struct -> Multiple, one child per field.
        let st = TyKind::Struct {
            name: Arc::from("Point"),
            fields: Arc::from([
                (Arc::from("x"), TyKind::Field),
                (Arc::from("y"), TyKind::Boolean),
            ]),
        };
        let struct_abs = abs_of_type(&mut sink, &st, std::slice::from_ref(&w));
        match struct_abs {
            Abs::Multiple(children) => {
                assert_eq!(children.len(), 2);
                assert_eq!(children[0], Abs::Atomic(vec![w.clone()]));
                assert_eq!(children[1], Abs::Atomic(vec![w.clone()]));
            }
            other => panic!("expected Abs::Multiple, got {other:?}"),
        }

        // Tuple -> Multiple too.
        let tup = TyKind::Tuple(Arc::from(vec![TyKind::Field, TyKind::Field]));
        assert!(matches!(
            abs_of_type(&mut sink, &tup, std::slice::from_ref(&w)),
            Abs::Multiple(children) if children.len() == 2
        ));

        // Vector/array -> Single, the ELEMENT type's Abs (aggregated, not
        // one-per-index).
        let vec_ty = TyKind::Vector(3, Arc::new(TyKind::Field));
        let vec_abs = abs_of_type(&mut sink, &vec_ty, std::slice::from_ref(&w));
        match vec_abs {
            Abs::Single(inner) => assert_eq!(*inner, Abs::Atomic(vec![w])),
            other => panic!("expected Abs::Single, got {other:?}"),
        }

        // No advisories from any of the above -- all shapes were known.
        assert!(sink.advisories().is_empty());
    }

    #[test]
    fn abs_of_type_unknown_emits_advisory() {
        let mut sink = DisclosureSink::new();
        let w = witness(0);
        let result = abs_of_type(&mut sink, &TyKind::Unknown, std::slice::from_ref(&w));
        // Fails closed: still seeded (never silently untainted)...
        assert_eq!(result, Abs::Atomic(vec![w]));
        // ...and the uncertainty is recorded exactly once.
        assert_eq!(sink.advisories().len(), 1);
        assert!(sink.advisories()[0].reason.contains("Unknown"));
    }

    #[test]
    fn seed_sources_tags_constructor_and_circuit_args() {
        let db = CompactDatabase::default();
        let text = "export ledger c: Uint<8>;\n\
                     constructor(secret: Uint<8>) { c = secret; }\n\
                     export circuit f(pub_x: Field): [] { }\n";
        let src = SourceText::new(&db, Arc::from(text));
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        let ws = empty_ws(&db);
        let file = crate::FileId::from_raw_for_test(0);
        let tree = item_tree(&db, src);

        let roots = discover_roots(&tree);
        let ctor_root = roots
            .iter()
            .find(|r| matches!(r, Root::Constructor { .. }))
            .expect("constructor root");
        let circuit_root = roots
            .iter()
            .find(|r| matches!(r, Root::Circuit { name, .. } if name == "f"))
            .expect("circuit root f");

        let mut sink = DisclosureSink::new();
        let ctor_env = seed_sources(&db, file, src, fd, ws, ctor_root, &mut sink);
        let secret_abs = ctor_env.get("secret").expect("secret seeded");
        match secret_abs {
            Abs::Atomic(witnesses) => {
                assert_eq!(witnesses.len(), 1);
                assert_eq!(
                    witnesses[0].info,
                    WitnessInfo::ConstructorArg {
                        arg: "secret".into()
                    }
                );
            }
            other => panic!("expected Abs::Atomic, got {other:?}"),
        }

        let circuit_env = seed_sources(&db, file, src, fd, ws, circuit_root, &mut sink);
        let pub_x_abs = circuit_env.get("pub_x").expect("pub_x seeded");
        match pub_x_abs {
            Abs::Atomic(witnesses) => {
                assert_eq!(witnesses.len(), 1);
                assert_eq!(
                    witnesses[0].info,
                    WitnessInfo::CircuitArg {
                        function: "f".into(),
                        arg: "pub_x".into(),
                    }
                );
            }
            other => panic!("expected Abs::Atomic, got {other:?}"),
        }

        assert!(sink.advisories().is_empty());
    }

    #[test]
    fn discover_roots_finds_exported_circuits_and_constructor_only() {
        let db = CompactDatabase::default();
        let text = "constructor(x: Field) { }\n\
                     export circuit pub_c(): [] { }\n\
                     circuit priv_c(): [] { }\n";
        let src = SourceText::new(&db, Arc::from(text));
        let tree = item_tree(&db, src);
        let roots = discover_roots(&tree);

        assert!(roots.iter().any(|r| matches!(r, Root::Constructor { .. })));
        assert!(
            roots
                .iter()
                .any(|r| matches!(r, Root::Circuit { name, .. } if name == "pub_c"))
        );
        assert!(
            !roots
                .iter()
                .any(|r| matches!(r, Root::Circuit { name, .. } if name == "priv_c"))
        );
        assert_eq!(roots.len(), 2);
    }
}
