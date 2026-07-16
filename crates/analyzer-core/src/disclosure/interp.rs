//! Root discovery + type-driven source seeding (spec §3.1/§3.4, R0 rows
//! AB1, S2, S3). Turns a circuit/constructor declaration into a seeded `Env`
//! (param name -> tainted `Abs`) the interpreter (A3) starts walking from.
//!
//! This module carries the seeding layer (`discover_roots`, `seed_sources`,
//! `abs_of_type`) and the intraprocedural interpreter walk (`interp_expr`/
//! `interp_stmt`) plus the A4 sinks (the ledger Cell-write sink and the
//! `return` sink with its `filter_witnesses` asymmetry). A4 wired
//! `disclosure_diagnostics_query` (`mod.rs`) to run the walk from every root
//! and drain the leak table, so every item here has a live non-test caller —
//! the A1/A2-era file-level `#![cfg_attr(not(test), expect(dead_code))]`
//! scaffold marker is gone.

use std::collections::HashMap;

use compactp_ast::AstNode;
use compactp_ast::expr::{CallExpr, Expr};
use compactp_syntax::{SyntaxKind, SyntaxNode};
use text_size::TextRange;

use crate::Definition;
use crate::db::{Db, FileDeps, SourceText, Workspace, item_symbol, item_tree, resolve_query};
use crate::item_tree::{ItemTree, SymbolKind};
use crate::ty::TyKind;

use super::abs::{
    Abs, PathPoint, Witness, WitnessInfo, combine_abs, disclose, merge_witnesses, witnesses_of,
};
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

// ===========================================================================
// A3: the intraprocedural interpreter walk
// ===========================================================================

/// The salsa/resolution context the walk needs beyond the pure `Abs` values:
/// callee classification (`resolve_query`/`item_symbol`) and declared-type
/// lowering (`type_kind_resolved`) require the database and file handles. The
/// brief's core args (`sink`, `env`, `control`, node) are threaded alongside a
/// borrow of this. A4/A5/A6 consume `interp_expr`/`interp_stmt` with the same
/// shape.
#[derive(Clone, Copy)]
pub struct InterpCtx<'a> {
    pub db: &'a dyn Db,
    pub file: crate::FileId,
    pub src: SourceText,
    pub fd: FileDeps,
    pub ws: Workspace,
    /// The exported circuit currently being walked (R0 `disclosing-function-name?`,
    /// spec §3.2). `Some(name)` inside an exported-circuit root arms the `return`
    /// sink (K7) and names it; `None` (a constructor root, whose body has no
    /// value-returning `return`) disarms it.
    pub disclosing_fn: Option<&'a str>,
}

/// Seeds and walks one analysis root (spec §3.1): `seed_sources` builds the
/// parameter environment, then `interp_stmt` walks the root's body block with
/// an empty control set (control-witness threading is A5). The root's identity
/// arms the `return` sink — an exported circuit sets `disclosing_fn` to its
/// name (K7 fires and is named after it); a constructor leaves it `None` (K7
/// disarmed). Sinks record into `sink`'s leak table; the caller drains it.
pub fn interp_root(ctx: &InterpCtx, sink: &mut DisclosureSink, root: &Root) {
    let disclosing_fn = match root {
        Root::Circuit { name, .. } => Some(name.as_str()),
        Root::Constructor { .. } => None,
    };
    let ctx = InterpCtx {
        disclosing_fn,
        ..*ctx
    };
    let mut env = seed_sources(ctx.db, ctx.file, ctx.src, ctx.fd, ctx.ws, root, sink);
    let Some(body) = root_body(&ctx, root) else {
        return;
    };
    interp_stmt(&ctx, sink, &mut env, &[], &compactp_ast::Stmt::Block(body));
}

/// The body `Block` of a root's circuit/constructor declaration, located the
/// same way `seed_sources` finds its parameters. `None` when the symbol index
/// doesn't resolve or the declaration has no body (both defensive — a `Root`
/// `discover_roots` produced from the same `src` always resolves).
fn root_body(ctx: &InterpCtx, root: &Root) -> Option<compactp_ast::Block> {
    let tree = item_tree(ctx.db, ctx.src);
    let ast_root = SyntaxNode::new_root(crate::db::parsed(ctx.db, ctx.src).green);
    match root {
        Root::Circuit { symbol, .. } => {
            let sym = tree.symbols.get(*symbol as usize)?;
            ast_root
                .descendants()
                .filter_map(compactp_ast::CircuitDef::cast)
                .find(|c| c.name().map(|n| n.text_range()) == Some(sym.name_range))?
                .body()
        }
        Root::Constructor { symbol } => {
            let sym = tree.symbols.get(*symbol as usize)?;
            ast_root
                .descendants()
                .filter_map(compactp_ast::ConstructorDef::cast)
                .find(|c| c.syntax().text_range() == sym.full_range)?
                .body()
        }
    }
}

/// How a call's callee resolves (spec §3.1/§3.6). Drives the `Call` arm.
enum CalleeClass {
    /// S1 source: a user `witness` function. Carries the declared return type
    /// (`Unknown` when it can't be read precisely — fail-closed via
    /// `abs_of_type`).
    Witness { name: String, ret: TyKind },
    /// A user `circuit` (defer cross-circuit interpretation to v3b).
    Circuit,
    /// A native/stdlib primitive, a ledger/method call, or an unresolved
    /// callee — all deferred to A6, fail-closed here.
    Native,
}

/// Interprets an expression, returning its abstract taint value (`Abs`).
/// Every form is pinned to an R0 row; anything not in R0 routes through the
/// fail-closed catch-all (advisory + conservative witness union), never a
/// silent taint drop (spec §0). `control` carries the ambient implicit-flow
/// witnesses (A5): branch forms (`&&`/`||`, the ternary) thread the condition
/// into it for their gated operands, and a ledger-field method call records a
/// control-flow leak keyed on it.
pub fn interp_expr(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &Env,
    control: &[Witness],
    expr: &Expr,
) -> Abs {
    let src = expr.syntax().text_range();
    match expr {
        // Literal (R0 AB2/P4): a compile-time `true`/`false` -> Boolean (so
        // `if` can constant-fold); every other literal is untainted Atomic.
        Expr::Literal(lit) => match bool_literal(lit.syntax()) {
            Some(value) => Abs::Boolean {
                value,
                witnesses: Vec::new(),
            },
            None => Abs::Atomic(Vec::new()),
        },
        // Variable reference (R0 P5 `var-ref`): env lookup. Unbound -> a
        // non-witness local (untainted). No path point here — the binding
        // path point is attached at the `const` site (P5 `let*`).
        Expr::Name(n) => n
            .ident()
            .and_then(|tok| env.get(tok.text()).cloned())
            .unwrap_or_else(|| Abs::Atomic(Vec::new())),
        // Parenthesized (structural): passthrough the inner expression.
        Expr::Paren(_) => match child_exprs(expr.syntax()).first() {
            Some(inner) => interp_expr(ctx, sink, env, control, inner),
            None => Abs::Atomic(Vec::new()),
        },
        // Cast / `as` (R0 P2): identity passthrough — casts carry taint
        // unchanged, they do NOT sanitize. The `Type` child does not cast to
        // `Expr`, so the sole `Expr` child is the operand.
        Expr::Cast(_) => match child_exprs(expr.syntax()).first() {
            Some(operand) => interp_expr(ctx, sink, env, control, operand),
            None => Abs::Atomic(Vec::new()),
        },
        // `disclose(e)` (R0 D1): the ONLY sanitizer. Interpret the argument,
        // then clear witnesses at every level.
        Expr::Disclose(_) => {
            let arg = match child_exprs(expr.syntax()).first() {
                Some(e) => interp_expr(ctx, sink, env, control, e),
                None => Abs::Atomic(Vec::new()),
            };
            disclose(&arg)
        }
        // `default<T>` (R0 P1): untainted `Abs` shaped by `T` (empty seed).
        Expr::Default(d) => match d.ty() {
            Some(t) => {
                let kind =
                    crate::infer::type_kind_resolved(ctx.db, ctx.file, ctx.src, ctx.fd, ctx.ws, &t);
                abs_of_type(sink, &kind, &[])
            }
            None => Abs::Atomic(Vec::new()),
        },
        Expr::Binary(b) => interp_binary(ctx, sink, env, control, b, src),
        Expr::Ternary(_) => interp_ternary(ctx, sink, env, control, expr, src),
        Expr::Call(call) => interp_call(ctx, sink, env, control, call, src),
        Expr::Member(m) => interp_member(ctx, sink, env, control, m, src),
        Expr::Index(_) => interp_index(ctx, sink, env, control, expr, src),
        // Struct literal (R0 P5 `new`): `Multiple`, one child per field, slotted
        // into DECLARED field order so it agrees with the projection path
        // (`field_index`/`abs_of_type`, both declared-order). A struct-update
        // (spread) can't be merged positionally here -> fail closed.
        Expr::Struct(s) => {
            if s.update().is_some() {
                return fail_closed_expr(
                    ctx,
                    sink,
                    env,
                    control,
                    expr,
                    "struct-update (spread) construction not modeled",
                );
            }
            interp_struct_literal(ctx, sink, env, control, s, src)
        }
        // Array literal (R0 AB1/P5 `vector`): arrays are tracked in the
        // aggregate -> `Single` over the join of all element abses.
        Expr::Array(a) => {
            let elems = child_exprs(a.syntax());
            let mut acc: Option<Abs> = None;
            for e in &elems {
                let ea = interp_expr(ctx, sink, env, control, e);
                acc = Some(match acc {
                    Some(prev) => combine_abs(&prev, &ea),
                    None => ea,
                });
            }
            Abs::Single(Box::new(acc.unwrap_or_else(|| Abs::Atomic(Vec::new()))))
        }
        // Bytes literal (R0 AB1): a scalar (`Atomic`). Elements are constants;
        // union any witnesses defensively (normally empty).
        Expr::Bytes(bytes) => {
            let mut ws = Vec::new();
            for e in child_exprs(bytes.syntax()) {
                let a = interp_expr(ctx, sink, env, control, &e);
                ws = merge_witnesses(&ws, &witnesses_of(&a));
            }
            Abs::Atomic(ws)
        }
        // Not in the R0 index (or deferred): fail closed.
        // `map`/`fold` -> A7 fixpoint; `slice`/`pad`/`spread`/`lambda`/unary
        // -> unmodeled. Advisory + conservative union of subexpr witnesses.
        Expr::Map(_) => fail_closed_expr(
            ctx,
            sink,
            env,
            control,
            expr,
            "map not modeled (fixpoint deferred to A7)",
        ),
        Expr::Fold(_) => fail_closed_expr(
            ctx,
            sink,
            env,
            control,
            expr,
            "fold not modeled (fixpoint deferred to A7)",
        ),
        Expr::Slice(_) => fail_closed_expr(
            ctx,
            sink,
            env,
            control,
            expr,
            "slice projection not modeled",
        ),
        Expr::Pad(_) => fail_closed_expr(ctx, sink, env, control, expr, "pad not modeled"),
        Expr::Spread(_) => fail_closed_expr(ctx, sink, env, control, expr, "spread not modeled"),
        Expr::Unary(_) => {
            fail_closed_expr(ctx, sink, env, control, expr, "unary operator not modeled")
        }
        Expr::Lambda(_) => fail_closed_expr(ctx, sink, env, control, expr, "lambda not modeled"),
    }
}

/// Binary operators: `+ - *` (R0 F5, taint kept), the six comparisons (R0 F4,
/// collapse to a scalar `Atomic`), `&& ||` (R0 F3, desugared to `if` in the
/// compiler — keep all operand taint as a scalar). Any other operator (`/`,
/// bit-ops, …) is not in R0 -> fail closed.
fn interp_binary(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &Env,
    control: &[Witness],
    b: &compactp_ast::expr::BinaryExpr,
    src: TextRange,
) -> Abs {
    let kids = child_exprs(b.syntax());
    let op = b.op().map(|t| t.kind());
    let lhs = kids
        .first()
        .map(|e| interp_expr(ctx, sink, env, control, e))
        .unwrap_or_else(|| Abs::Atomic(Vec::new()));
    // `&&`/`||` short-circuit (R0 F3): they desugar to `if`, so the RHS runs only
    // under the LHS — a sink inside the RHS is control-dependent on the LHS's
    // witnesses. Thread the LHS into `control` for the RHS (`thread_control`).
    // The DATA result below still unions both operands' taint — separate
    // mechanism from the control set.
    let rhs_control = match op {
        Some(SyntaxKind::AMP_AMP | SyntaxKind::PIPE_PIPE) => {
            thread_control(witnesses_of(&lhs), control, src)
        }
        _ => control.to_vec(),
    };
    let rhs = kids
        .get(1)
        .map(|e| interp_expr(ctx, sink, env, &rhs_control, e))
        .unwrap_or_else(|| Abs::Atomic(Vec::new()));
    match op {
        Some(SyntaxKind::PLUS | SyntaxKind::MINUS | SyntaxKind::STAR) => {
            let exposure = match op {
                Some(SyntaxKind::PLUS) => "the result of an addition involving",
                Some(SyntaxKind::MINUS) => "the result of a subtraction involving",
                _ => "the result of a multiplication involving",
            };
            add_path_point(
                combine_abs(&lhs, &rhs),
                &PathPoint {
                    src,
                    description: "the computation".into(),
                    exposure: exposure.into(),
                },
            )
        }
        Some(
            SyntaxKind::EQ_EQ
            | SyntaxKind::BANG_EQ
            | SyntaxKind::LT
            | SyntaxKind::LT_EQ
            | SyntaxKind::GT
            | SyntaxKind::GT_EQ,
        ) => add_path_point(
            Abs::Atomic(merge_witnesses(&witnesses_of(&lhs), &witnesses_of(&rhs))),
            &PathPoint {
                src,
                description: "the comparison".into(),
                exposure: "the result of a comparison involving".into(),
            },
        ),
        Some(SyntaxKind::AMP_AMP | SyntaxKind::PIPE_PIPE) => {
            // R0 F3: `&&`/`||` are desugared to `if`; keep all operand taint.
            Abs::Atomic(merge_witnesses(&witnesses_of(&lhs), &witnesses_of(&rhs)))
        }
        _ => {
            sink.emit_advisory(src, "binary operator not modeled".to_string());
            Abs::Atomic(merge_witnesses(&witnesses_of(&lhs), &witnesses_of(&rhs)))
        }
    }
}

/// `cond ? then : else` — the `if` expression (R0 F1, DATA join). Const-boolean
/// condition -> mirror the taken branch; otherwise join both branches
/// (`combine_abs`) and, in expression position, fold the condition's own taint
/// into the result value (F1(d) `add-witnesses`). Both branches are also gated
/// on the condition, so its witnesses thread into `control` for a sink inside
/// either branch (A5) — distinct from the F1(d) value fold.
fn interp_ternary(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &Env,
    control: &[Witness],
    expr: &Expr,
    src: TextRange,
) -> Abs {
    let kids = child_exprs(expr.syntax());
    let cond = kids
        .first()
        .map(|e| interp_expr(ctx, sink, env, control, e))
        .unwrap_or_else(|| Abs::Atomic(Vec::new()));
    // Both branches are gated on the condition (R0 F1/§3.3): thread its
    // witnesses into `control` so a sink inside either branch records the
    // control-flow leak. DISTINCT from the F1(d) DATA fold below, which folds
    // the condition into the branch VALUE.
    let branch_control = thread_control(witnesses_of(&cond), control, src);
    let then_abs = kids
        .get(1)
        .map(|e| interp_expr(ctx, sink, env, &branch_control, e))
        .unwrap_or_else(|| Abs::Atomic(Vec::new()));
    let else_abs = kids
        .get(2)
        .map(|e| interp_expr(ctx, sink, env, &branch_control, e))
        .unwrap_or_else(|| Abs::Atomic(Vec::new()));
    let joined = match &cond {
        Abs::Boolean { value, .. } => {
            if *value {
                then_abs
            } else {
                else_abs
            }
        }
        _ => combine_abs(&then_abs, &else_abs),
    };
    // F1(d): a non-constant conditional's value carries the condition's taint.
    let cond_ws = witnesses_of(&cond);
    if cond_ws.is_empty() {
        joined
    } else {
        let tagged: Vec<Witness> = cond_ws
            .into_iter()
            .map(|mut w| {
                w.path.push(PathPoint {
                    src,
                    description: "the conditional expression".into(),
                    exposure: "the boolean value of".into(),
                });
                w
            })
            .collect();
        add_witnesses(joined, &tagged)
    }
}

/// A call: witness (S1 source), circuit (v3b), or native/ledger/unresolved (A6).
fn interp_call(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &Env,
    control: &[Witness],
    call: &CallExpr,
    src: TextRange,
) -> Abs {
    match classify_callee(ctx, call) {
        CalleeClass::Witness { name, ret } => {
            // S1: a fresh witness-return source shaped by the witness's return
            // type. The witness's own arguments are not propagated (the source
            // returns a fixed tainted abs regardless of args).
            let anchor = call.name().map(|t| t.text_range()).unwrap_or(src);
            let w = Witness {
                uid: sink.next_witness_uid(),
                src: anchor,
                info: WitnessInfo::WitnessReturn { function: name },
                path: Vec::new(),
            };
            abs_of_type(sink, &ret, std::slice::from_ref(&w))
        }
        // A cross-circuit call (v3b) or a native/stdlib/ledger-op/unresolved
        // callee (A6): the taint semantics of the RESULT are not yet modeled, so
        // fail closed to an AMBER advisory (spec §0) and return a CLEAN result —
        // NOT a conservatively-tainted one. A tainted result would flow to a sink
        // as a confirmed RED leak (§0 says fail-closed is *amber*, not red), and
        // for a native that is actually a sanitizer (`persistentCommit`/
        // `transientCommit`, `discloses: nothing`) that RED would be a false
        // positive on compiler-accepted code — breaking the corpus gate. A6's
        // conduit table promotes the real conduits (`persistentHash` → "a hash
        // of") back to tainted; until then the uncertainty lives in the advisory.
        // Arguments are still interpreted (for their own nested sinks/advisories).
        CalleeClass::Circuit => {
            interp_args_for_effect(ctx, sink, env, control, call);
            sink.emit_advisory(src, "cross-circuit call not interpreted (v3b)".to_string());
            Abs::Atomic(Vec::new())
        }
        CalleeClass::Native => {
            interp_args_for_effect(ctx, sink, env, control, call);
            // A5 (K2): a ledger-field METHOD call (`c.method(args)`, receiver a
            // ledger field) is a ledger OPERATION. When gated, emit the
            // control-flow leak on the ambient `control` set (`record_leak`
            // no-ops when `control` is empty ⇒ a top-level container op records
            // no control leak). KEEP the amber advisory below for the still-
            // unmodeled per-arg DATA flags (the `discloses?` table is A6): a
            // top-level container op stays amber, never silent green (§0). No
            // container-op DATA leak is recorded here — A6 owns that table.
            if is_method_call(call)
                && let Some(recv) = method_receiver(call)
                && is_ledger_field(ctx, &recv)
            {
                sink.record_leak(src, "performing this ledger operation", control.to_vec());
            }
            sink.emit_advisory(src, "native call not yet modeled (A6)".to_string());
            Abs::Atomic(Vec::new())
        }
    }
}

/// Struct literal `S { … }` (R0 P5 `new`): a `Multiple` whose children are in
/// the struct's DECLARED field order — canonically agreeing with `abs_of_type`
/// (seeded params) and `field_index` (member projection), both declared-order.
/// Field-init expressions are still interpreted in SOURCE order (their
/// evaluation-order side effects — nested sinks/advisories — are preserved),
/// then slotted into their declared index. Fails closed to source order + an
/// advisory when the struct's declared type can't be resolved (never a
/// confident wrong order).
fn interp_struct_literal(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &Env,
    control: &[Witness],
    s: &compactp_ast::expr::StructExpr,
    src: TextRange,
) -> Abs {
    // Interpret every field-init once, in source order, keyed by field name.
    let inits: Vec<(Option<String>, Abs)> = s
        .field_inits()
        .map(|fi| {
            let name = fi.name().map(|n| n.text().to_string());
            let abs = match child_exprs(fi.syntax()).first() {
                Some(e) => interp_expr(ctx, sink, env, control, e),
                None => Abs::Atomic(Vec::new()),
            };
            (name, abs)
        })
        .collect();

    // Resolve the struct's declared field order (same path `field_index` uses).
    let declared: Option<Vec<String>> = s.name().and_then(|name| {
        match crate::db::named_type_ty(
            ctx.db,
            ctx.file,
            ctx.src,
            ctx.fd,
            ctx.ws,
            name.text_range().start(),
        ) {
            TyKind::Struct { fields, .. } => {
                Some(fields.iter().map(|(n, _)| n.to_string()).collect())
            }
            _ => None,
        }
    });

    match declared {
        Some(order) => Abs::Multiple(
            order
                .iter()
                .map(|fname| {
                    inits
                        .iter()
                        .find(|(n, _)| n.as_deref() == Some(fname.as_str()))
                        .map(|(_, abs)| abs.clone())
                        .unwrap_or_else(|| Abs::Atomic(Vec::new()))
                })
                .collect(),
        ),
        None => {
            sink.emit_advisory(
                src,
                "struct literal type could not be resolved; fields kept in source order"
                    .to_string(),
            );
            Abs::Multiple(inits.into_iter().map(|(_, abs)| abs).collect())
        }
    }
}

/// Member access `expr.field` (R0 P5 projection). Projects the field out of a
/// `Multiple` when the field's positional index is resolvable (the base's
/// static struct type, declared field order). Fail closed (advisory +
/// conservative union) when the shape/index can't be resolved.
fn interp_member(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &Env,
    control: &[Witness],
    m: &compactp_ast::expr::MemberExpr,
    src: TextRange,
) -> Abs {
    let Some(base) = child_exprs(m.syntax()).into_iter().next() else {
        return Abs::Atomic(Vec::new());
    };
    let base_abs = interp_expr(ctx, sink, env, control, &base);
    let field = m.field().map(|t| t.text().to_string());
    if let Abs::Multiple(children) = &base_abs
        && let Some(idx) = field_index(ctx, &base, field.as_deref())
        && let Some(child) = children.get(idx)
    {
        return child.clone();
    }
    sink.emit_advisory(
        src,
        "struct/tuple member projection could not be resolved; conservatively unioned".to_string(),
    );
    Abs::Atomic(witnesses_of(&base_abs))
}

/// Index `expr[i]` (R0 P5 `vector-ref`). Array element out of a `Single`, with
/// the index's own witnesses unioned into the selected element ("the element
/// selected by"). A runtime index into a `Multiple` (heterogeneous aggregate)
/// can't be resolved positionally -> fail closed.
fn interp_index(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &Env,
    control: &[Witness],
    expr: &Expr,
    src: TextRange,
) -> Abs {
    let kids = child_exprs(expr.syntax());
    let base = kids
        .first()
        .map(|e| interp_expr(ctx, sink, env, control, e))
        .unwrap_or_else(|| Abs::Atomic(Vec::new()));
    let idx_ws = kids
        .get(1)
        .map(|e| witnesses_of(&interp_expr(ctx, sink, env, control, e)))
        .unwrap_or_default();
    let pp = PathPoint {
        src,
        description: "the index".into(),
        exposure: "the element selected by".into(),
    };
    match base {
        Abs::Single(inner) => add_path_point(add_witnesses(*inner, &idx_ws), &pp),
        Abs::Multiple(children) => {
            let mut ws = idx_ws;
            for c in &children {
                ws = merge_witnesses(&ws, &witnesses_of(c));
            }
            sink.emit_advisory(
                src,
                "index into heterogeneous aggregate could not be resolved; conservatively unioned"
                    .to_string(),
            );
            Abs::Atomic(ws)
        }
        other => Abs::Atomic(merge_witnesses(&witnesses_of(&other), &idx_ws)),
    }
}

/// Interprets a statement, threading `env` (mutated by bindings). Blocks
/// sequence; `const` binds; `return`/assign interpret their operands and record
/// both the A4 DATA sink and the A5 control-flow sink; `if` threads the
/// condition's witnesses into `control` for both branches (A5); `for` is
/// deferred to A7.
pub fn interp_stmt(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &mut Env,
    control: &[Witness],
    stmt: &compactp_ast::Stmt,
) {
    use compactp_ast::Stmt;
    match stmt {
        Stmt::Block(b) => {
            for s in b.stmts() {
                interp_stmt(ctx, sink, env, control, &s);
            }
        }
        // `const pat = expr` (R0 P5 `let*`): interpret RHS, bind the pattern.
        Stmt::Const(c) => {
            let val = c
                .value()
                .map(|e| interp_expr(ctx, sink, env, control, &e))
                .unwrap_or_else(|| Abs::Atomic(Vec::new()));
            bind_pattern(sink, env, c.pattern().as_ref(), val);
        }
        // Expression statement / assert: interpret operands so their `Abs`
        // (and any advisory) is realized. Neither is a sink.
        Stmt::Expr(_) | Stmt::Assert(_) => {
            for e in child_exprs(stmt.syntax()) {
                interp_expr(ctx, sink, env, control, &e);
            }
        }
        // Assignment (R0 K1 ledger sink, spec §3.2). A Cell-style write
        // `c = e` (also `c += e` / `c -= e`) to a ledger field leaks the
        // written value's witnesses (L1 default: an op arg with no `discloses`
        // clause leaks). Interpret ONLY the RHS as a value — NOT the LHS
        // write-target (Step 0 minor: reading the l-value emits a spurious
        // projection advisory). The precise per-op `discloses?` flag table +
        // container-op (`c.method(arg)`) args are A6.
        Stmt::Assign(a) => match child_exprs(a.syntax()).as_slice() {
            [target, value] => {
                let value_abs = interp_expr(ctx, sink, env, control, value);
                if is_ledger_field(ctx, target) {
                    let range = a.syntax().text_range();
                    sink.record_leak(range, "ledger operation", witnesses_of(&value_abs));
                    // A5 (K2): conditioning a ledger op on a witness discloses
                    // that witness as a SEPARATE control-flow leak — a distinct
                    // nature ⇒ a distinct table entry (never merged into the data
                    // leak, never tainting the written value). `record_leak`
                    // no-ops when `control` is empty (fires only when gated).
                    sink.record_leak(range, "performing this ledger operation", control.to_vec());
                }
            }
            // Defensive (unexpected shape): realize taint, record no sink.
            kids => {
                for e in kids {
                    interp_expr(ctx, sink, env, control, e);
                }
            }
        },
        // Return (R0 K7 return sink + `filter_witnesses` asymmetry, spec §3.2).
        // Only fires inside an exported-circuit root (`disclosing_fn`), and only
        // for `WitnessReturn` taint — returning the circuit's own argument (or a
        // constructor argument) is NOT a disclosure. A4 does the DATA leak; the
        // control-witness leak (K6) is A5.
        Stmt::Return(r) => {
            let val = r.value().map(|e| interp_expr(ctx, sink, env, control, &e));
            if let (Some(fname), Some(val)) = (ctx.disclosing_fn, &val) {
                let range = stmt.syntax().text_range();
                let filtered = filter_witnesses(&witnesses_of(val));
                sink.record_leak(
                    range,
                    format!("the value returned from exported circuit {fname}"),
                    filtered,
                );
                // A5 (K6): a return gated on a witness discloses the gating
                // witness as a distinct control-flow leak. The SAME K7 asymmetry
                // applies (`filter_witnesses`) — being gated on the circuit's own
                // argument is not a disclosure, only a `WitnessReturn` control
                // witness leaks. `record_leak` no-ops on an empty (filtered) set.
                sink.record_leak(
                    range,
                    format!("returning this value from exported circuit {fname}"),
                    filter_witnesses(control),
                );
            }
        }
        // `if` in statement position (R0 F1/§3.3): interpret the condition under
        // the ambient `control`, then thread its witnesses into `control` for
        // every branch (`thread_control`) — a sink inside a branch records the
        // separate control-flow leak. The condition is the sole `Expr` child and
        // precedes the branch blocks in source order, so `branch_control` is set
        // before any branch runs. Branches get a child scope (bindings do not
        // escape). An `else if` is a nested `If` Stmt: it re-threads its own
        // condition on top of this branch_control, so gating accumulates.
        Stmt::If(_) => {
            let mut branch_control = control.to_vec();
            for child in stmt.syntax().children() {
                if let Some(e) = Expr::cast(child.clone()) {
                    let cond = interp_expr(ctx, sink, env, control, &e);
                    branch_control =
                        thread_control(witnesses_of(&cond), control, stmt.syntax().text_range());
                } else if let Some(blk) = compactp_ast::Block::cast(child.clone()) {
                    let mut scope = env.clone();
                    interp_stmt(
                        ctx,
                        sink,
                        &mut scope,
                        &branch_control,
                        &compactp_ast::Stmt::Block(blk),
                    );
                } else if let Some(nested) = compactp_ast::Stmt::cast(child.clone()) {
                    let mut scope = env.clone();
                    interp_stmt(ctx, sink, &mut scope, &branch_control, &nested);
                }
            }
        }
        // `for` (R0 fixpoint — deferred to A7): fail closed. Interpret the body
        // once in a child scope with the loop var bound conservatively.
        Stmt::For(f) => {
            sink.emit_advisory(
                stmt.syntax().text_range(),
                "for loop not modeled (fixpoint deferred to A7)".to_string(),
            );
            let mut range_ws = Vec::new();
            for e in child_exprs(stmt.syntax()) {
                let a = interp_expr(ctx, sink, env, control, &e);
                range_ws = merge_witnesses(&range_ws, &witnesses_of(&a));
            }
            let mut scope = env.clone();
            if let Some(v) = f.var_name() {
                scope.insert(v.text().to_string(), Abs::Atomic(range_ws));
            }
            if let Some(body) = f.body() {
                interp_stmt(
                    ctx,
                    sink,
                    &mut scope,
                    control,
                    &compactp_ast::Stmt::Block(body),
                );
            }
        }
        // Reserved/unused grammar node — fail closed.
        Stmt::MultiConst(_) => {
            sink.emit_advisory(
                stmt.syntax().text_range(),
                "multi-const statement not modeled".to_string(),
            );
            for e in child_exprs(stmt.syntax()) {
                interp_expr(ctx, sink, env, control, &e);
            }
        }
    }
}

/// Binds a `const` pattern to its value's `Abs` (R0 P5 `let*`). A simple
/// identifier binds precisely (with a "the binding of <name>" path point);
/// tuple destructuring binds elementwise when the value is a matching
/// `Multiple`, else — and for struct destructuring — every bound identifier
/// gets the conservative whole-value witness union (fail closed).
fn bind_pattern(
    sink: &mut DisclosureSink,
    env: &mut Env,
    pat: Option<&compactp_ast::Pat>,
    val: Abs,
) {
    use compactp_ast::Pat;
    match pat {
        Some(Pat::Ident(id)) => {
            if let Some(tok) = id.name() {
                let name = tok.text().to_string();
                let bound = add_path_point(
                    val,
                    &PathPoint {
                        src: tok.text_range(),
                        description: format!("the binding of {name}"),
                        exposure: String::new(),
                    },
                );
                env.insert(name, bound);
            }
        }
        Some(Pat::Tuple(tp)) => {
            let elems: Vec<compactp_ast::Pat> = tp.elements().filter_map(|e| e.pattern()).collect();
            if let Abs::Multiple(children) = &val
                && children.len() == elems.len()
            {
                for (p, child) in elems.iter().zip(children.iter()) {
                    bind_pattern(sink, env, Some(p), child.clone());
                }
                return;
            }
            let ws = witnesses_of(&val);
            for p in &elems {
                bind_conservative(env, p, &ws);
            }
            sink.emit_advisory(
                tp.syntax().text_range(),
                "tuple destructuring shape did not match value; conservatively unioned".to_string(),
            );
        }
        Some(Pat::Struct(sp)) => {
            let ws = witnesses_of(&val);
            for f in sp.fields() {
                if let Some(tok) = f.name() {
                    env.insert(tok.text().to_string(), Abs::Atomic(ws.clone()));
                }
            }
            sink.emit_advisory(
                sp.syntax().text_range(),
                "struct destructuring not modeled positionally; conservatively unioned".to_string(),
            );
        }
        None => {}
    }
}

/// Binds every identifier reachable in `pat` to the conservative witness union
/// `ws` (fail-closed destructuring fallback).
fn bind_conservative(env: &mut Env, pat: &compactp_ast::Pat, ws: &[Witness]) {
    use compactp_ast::Pat;
    match pat {
        Pat::Ident(id) => {
            if let Some(tok) = id.name() {
                env.insert(tok.text().to_string(), Abs::Atomic(ws.to_vec()));
            }
        }
        Pat::Tuple(tp) => {
            for e in tp.elements() {
                if let Some(p) = e.pattern() {
                    bind_conservative(env, &p, ws);
                }
            }
        }
        Pat::Struct(sp) => {
            for f in sp.fields() {
                if let Some(tok) = f.name() {
                    env.insert(tok.text().to_string(), Abs::Atomic(ws.to_vec()));
                }
            }
        }
    }
}

/// Fail-closed catch-all (spec §0): record an advisory and return a
/// conservative `Atomic` unioning every subexpression's witnesses (interpreting
/// each direct `Expr` child recursively, so nested taint is never dropped).
fn fail_closed_expr(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &Env,
    control: &[Witness],
    expr: &Expr,
    reason: &str,
) -> Abs {
    let mut ws = Vec::new();
    for child in child_exprs(expr.syntax()) {
        let a = interp_expr(ctx, sink, env, control, &child);
        ws = merge_witnesses(&ws, &witnesses_of(&a));
    }
    sink.emit_advisory(expr.syntax().text_range(), reason.to_string());
    Abs::Atomic(ws)
}

/// Classifies a call's callee (witness / circuit / native). A method call
/// (`recv.method(..)`), an unresolved name, or a non-callable resolution all
/// fail closed to `Native` (A6). Cross-file circuits/witnesses classify by kind
/// too, but a cross-file witness's return type is read only same-file.
fn classify_callee(ctx: &InterpCtx, call: &CallExpr) -> CalleeClass {
    if is_method_call(call) {
        return CalleeClass::Native;
    }
    let Some(name_tok) = call.name() else {
        return CalleeClass::Native;
    };
    let Some(Definition::Item { file: df, index }) = resolve_query(
        ctx.db,
        ctx.file,
        ctx.src,
        ctx.fd,
        ctx.ws,
        name_tok.text_range().start(),
    ) else {
        return CalleeClass::Native;
    };
    let Some(sym) = item_symbol(ctx.db, ctx.file, ctx.src, ctx.fd, df, index) else {
        return CalleeClass::Native;
    };
    match sym.kind {
        SymbolKind::Witness => CalleeClass::Witness {
            name: sym.name.clone(),
            ret: witness_return_ty(ctx, df, &sym),
        },
        SymbolKind::Circuit | SymbolKind::CircuitSig | SymbolKind::ContractCircuit => {
            CalleeClass::Circuit
        }
        _ => CalleeClass::Native,
    }
}

/// The declared return `TyKind` of a same-file witness, else `Unknown`
/// (fail-closed; `abs_of_type` then advisory-seeds an `Atomic`).
fn witness_return_ty(ctx: &InterpCtx, def_file: crate::FileId, sym: &crate::Symbol) -> TyKind {
    if def_file != ctx.file {
        return TyKind::Unknown;
    }
    let root = SyntaxNode::new_root(crate::db::parsed(ctx.db, ctx.src).green);
    let ret = root
        .descendants()
        .filter_map(compactp_ast::WitnessDecl::cast)
        .find(|w| w.name().map(|n| n.text_range()) == Some(sym.name_range))
        .and_then(|w| w.return_type());
    match ret {
        Some(t) => crate::infer::type_kind_resolved(ctx.db, ctx.file, ctx.src, ctx.fd, ctx.ws, &t),
        None => TyKind::Unknown,
    }
}

/// Interprets a call's argument expressions for their side effects (nested
/// sinks + advisories), discarding the taint — the fail-closed circuit/native
/// arms return a CLEAN result (the arg taint does not propagate through the
/// unmodeled callee until A6's conduit table decides which args are conduits).
fn interp_args_for_effect(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    env: &Env,
    control: &[Witness],
    call: &CallExpr,
) {
    for arg in arg_exprs(call) {
        interp_expr(ctx, sink, env, control, &arg);
    }
}

/// The direct `Expr` children of a syntax node, in source order.
fn child_exprs(node: &SyntaxNode) -> Vec<Expr> {
    node.children().filter_map(Expr::cast).collect()
}

/// A call's argument expressions: the `Expr` children after the `(` (excludes
/// a method-call receiver, which appears before the `(`).
fn arg_exprs(call: &CallExpr) -> Vec<Expr> {
    let Some(lparen) = call
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == SyntaxKind::L_PAREN)
        .map(|t| t.text_range().start())
    else {
        return Vec::new();
    };
    call.syntax()
        .children()
        .filter_map(Expr::cast)
        .filter(|e| e.syntax().text_range().start() >= lparen)
        .collect()
}

/// A method call's receiver expression (`recv` in `recv.method(args)`): the sole
/// `Expr` child before the `(`. Used by A5 to recognize a ledger-field method
/// call (`c.increment(1)`) as a ledger operation for the control-flow leak.
/// `None` when there is no `(` or no receiver expression (a plain call).
fn method_receiver(call: &CallExpr) -> Option<Expr> {
    let lparen = call
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == SyntaxKind::L_PAREN)
        .map(|t| t.text_range().start())?;
    call.syntax()
        .children()
        .filter_map(Expr::cast)
        .find(|e| e.syntax().text_range().start() < lparen)
}

/// A method call carries a `.` token child (`recv.method(..)`); a plain call
/// does not.
fn is_method_call(call: &CallExpr) -> bool {
    call.syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == SyntaxKind::DOT)
}

/// K7 asymmetry (spec §3.2, `analysis-passes.ss:5561`, `filter-witnesses`): at
/// the `return` sink, keep ONLY `WitnessReturn` witnesses and drop
/// `ConstructorArg`/`CircuitArg`. Returning an exported circuit's own argument
/// (or a constructor argument) is NOT a disclosure — the load-bearing
/// asymmetry. A5 reuses this for the K6 control-witness return leak.
pub fn filter_witnesses(witnesses: &[Witness]) -> Vec<Witness> {
    witnesses
        .iter()
        .filter(|w| matches!(w.info, WitnessInfo::WitnessReturn { .. }))
        .cloned()
        .collect()
}

/// Whether the assignment target `target` writes a ledger field (R0 K1): its
/// leftmost identifier resolves to a `SymbolKind::Ledger` declaration. A Cell
/// write `c = e`, `c.x = e`, or `c[i] = e` all pivot on the root ledger name.
/// A non-ledger target (impossible for a well-formed circuit body, where the
/// only assignable l-values are ledger fields) resolves to `false` — no leak.
fn is_ledger_field(ctx: &InterpCtx, target: &Expr) -> bool {
    let Some(name_tok) = leftmost_name_token(target) else {
        return false;
    };
    let Some(Definition::Item { file: df, index }) = resolve_query(
        ctx.db,
        ctx.file,
        ctx.src,
        ctx.fd,
        ctx.ws,
        name_tok.text_range().start(),
    ) else {
        return false;
    };
    let Some(sym) = item_symbol(ctx.db, ctx.file, ctx.src, ctx.fd, df, index) else {
        return false;
    };
    sym.kind == SymbolKind::Ledger
}

/// The leftmost identifier token of an l-value expression: the root name of a
/// `Name`/`Member`/`Index` target (`c`, `c.x`, `c[i]` all -> `c`), peeling
/// parens. `None` for any other shape.
fn leftmost_name_token(target: &Expr) -> Option<compactp_syntax::SyntaxToken> {
    match unwrap_paren(target) {
        Expr::Name(n) => n.ident(),
        Expr::Member(m) => child_exprs(m.syntax())
            .into_iter()
            .next()
            .and_then(|base| leftmost_name_token(&base)),
        Expr::Index(i) => child_exprs(i.syntax())
            .into_iter()
            .next()
            .and_then(|base| leftmost_name_token(&base)),
        _ => None,
    }
}

/// The positional index of `field` within the base's struct shape, in the
/// struct's DECLARED field order — the single canonical order every `Multiple`
/// is built in (`abs_of_type` for seeded params, `interp_struct_literal` for
/// literals). `None` when it can't be determined (caller fails closed).
///
/// Resolved uniformly via the tracked `expr_ty` (the query `AnalysisHost::type_at`
/// fronts, `infer.rs:588`), reachable here from `ctx` — for a struct-literal base
/// AND a name-bound base alike, since both now carry a declared-order `Multiple`.
/// Step 0 (A3 review deferral): without this, a NAME-bound struct member fell
/// through to a flat `Atomic(union of ALL fields)`, over-tainting a clean field
/// into a false positive once the sinks exist.
fn field_index(ctx: &InterpCtx, base: &Expr, field: Option<&str>) -> Option<usize> {
    let field = field?;
    let base = unwrap_paren(base);
    // Resolve the base's static struct type (declared field order).
    // Anchor `expr_ty` at the base's last meaningful token, not its node start:
    // a NAME_EXPR node can carry leading whitespace, and `enclosing_expr` biases
    // a whitespace-boundary offset to the preceding token — landing on the wrong
    // expression (Unknown). The last non-trivia token (`s` in `s`, `b` in `a.b`)
    // sits inside the base and resolves to the base's own type.
    let off = base
        .syntax()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.text().trim().is_empty())
        .last()
        .map(|t| t.text_range().start())
        .unwrap_or_else(|| base.syntax().text_range().start());
    let kind = crate::db::expr_ty(ctx.db, ctx.file, ctx.src, ctx.fd, ctx.ws, off);
    if let TyKind::Struct { fields, .. } = kind {
        return fields.iter().position(|(n, _)| n.as_ref() == field);
    }
    None
}

/// Peels parenthesization to reach the inner expression.
fn unwrap_paren(e: &Expr) -> Expr {
    let mut cur = e.clone();
    while let Expr::Paren(p) = &cur {
        match child_exprs(p.syntax()).into_iter().next() {
            Some(inner) => cur = inner,
            None => break,
        }
    }
    cur
}

/// Whether a `LITERAL_EXPR` is a compile-time boolean, and its value (R0 AB2).
fn bool_literal(node: &SyntaxNode) -> Option<bool> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find_map(|t| match t.kind() {
            SyntaxKind::TRUE_KW => Some(true),
            SyntaxKind::FALSE_KW => Some(false),
            _ => None,
        })
}

/// Threads a branch condition's witnesses into the ambient `control` set for a
/// branch body (spec §3.3, R0 F1): each condition witness gains a "the
/// conditional branch" path point (exposure "the boolean value of"), then unions
/// with the incoming `control`. This threaded set is the implicit-flow carrier
/// the control-flow leak (K2/K6) keys on at every sink inside the branch — a
/// SEPARATE mechanism from the F1(d) DATA fold (which taints the branch VALUE);
/// the condition's witnesses are NOT folded into any branch value here. A
/// disclosed condition (`disclose(cond)`) has an empty witness set ⇒ nothing is
/// added ⇒ no control leak (the `implicit_flow_disclosed` invariant).
fn thread_control(
    cond_ws: Vec<Witness>,
    control: &[Witness],
    branch_src: TextRange,
) -> Vec<Witness> {
    let tagged: Vec<Witness> = cond_ws
        .into_iter()
        .map(|mut w| {
            w.path.push(PathPoint {
                src: branch_src,
                description: "the conditional branch".into(),
                exposure: "the boolean value of".into(),
            });
            w
        })
        .collect();
    merge_witnesses(&tagged, control)
}

/// `add-witnesses` (R0 F1(d)/P5): unions `extra` into every atomic/boolean leaf
/// of `abs`, recursing through `Multiple`/`Single`. Preserves shape.
fn add_witnesses(abs: Abs, extra: &[Witness]) -> Abs {
    match abs {
        Abs::Atomic(w) => Abs::Atomic(merge_witnesses(&w, extra)),
        Abs::Boolean { value, witnesses } => Abs::Boolean {
            value,
            witnesses: merge_witnesses(&witnesses, extra),
        },
        Abs::Multiple(items) => {
            Abs::Multiple(items.into_iter().map(|a| add_witnesses(a, extra)).collect())
        }
        Abs::Single(inner) => Abs::Single(Box::new(add_witnesses(*inner, extra))),
    }
}

/// `add-path-point` (R0 F4/F5/P5): appends `pp` to the trail of every witness
/// at every leaf of `abs`.
fn add_path_point(abs: Abs, pp: &PathPoint) -> Abs {
    let push = |mut w: Witness| {
        w.path.push(pp.clone());
        w
    };
    match abs {
        Abs::Atomic(w) => Abs::Atomic(w.into_iter().map(push).collect()),
        Abs::Boolean { value, witnesses } => Abs::Boolean {
            value,
            witnesses: witnesses.into_iter().map(push).collect(),
        },
        Abs::Multiple(items) => {
            Abs::Multiple(items.into_iter().map(|a| add_path_point(a, pp)).collect())
        }
        Abs::Single(inner) => Abs::Single(Box::new(add_path_point(*inner, pp))),
    }
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

    // -- A3 interpreter-walk tests --------------------------------------

    fn walk_setup(
        text: &str,
    ) -> (
        CompactDatabase,
        SourceText,
        FileDeps,
        Workspace,
        crate::FileId,
    ) {
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from(text));
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        let ws = empty_ws(&db);
        let file = crate::FileId::from_raw_for_test(0);
        (db, src, fd, ws, file)
    }

    fn root_of(db: &CompactDatabase, src: SourceText) -> compactp_syntax::SyntaxNode {
        compactp_syntax::SyntaxNode::new_root(crate::db::parsed(db, src).green)
    }

    fn first_expr(
        root: &compactp_syntax::SyntaxNode,
        kind: compactp_syntax::SyntaxKind,
    ) -> compactp_ast::expr::Expr {
        root.descendants()
            .find(|n| n.kind() == kind)
            .and_then(compactp_ast::expr::Expr::cast)
            .expect("expr of kind not found")
    }

    #[test]
    fn disclose_sanitizes() {
        let (db, src, fd, ws, file) =
            walk_setup("export circuit f(): Field { return disclose(w); }\n");
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, compactp_syntax::SyntaxKind::DISCLOSE_EXPR);
        let mut env = Env::new();
        env.insert("w".into(), Abs::Atomic(vec![witness(1)]));
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &env, &[], &expr);
        assert!(witnesses_of(&out).is_empty(), "disclose must clear taint");
    }

    #[test]
    fn witness_call_seeds_witness_return() {
        let (db, src, fd, ws, file) =
            walk_setup("witness getW(): Field;\nexport circuit f(): Field { return getW(); }\n");
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, compactp_syntax::SyntaxKind::CALL_EXPR);
        let env = Env::new();
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &env, &[], &expr);
        match out {
            Abs::Atomic(ws) => {
                assert_eq!(ws.len(), 1);
                assert_eq!(
                    ws[0].info,
                    WitnessInfo::WitnessReturn {
                        function: "getW".into()
                    }
                );
            }
            other => panic!("expected Atomic witness-return, got {other:?}"),
        }
    }

    #[test]
    fn member_projects_struct_field() {
        let (db, src, fd, ws, file) = walk_setup(
            "witness g1(): Field;\nwitness g2(): Field;\nstruct S { a: Field, b: Field }\n\
             export circuit f(): Field { return (S { a: g1(), b: g2() }).b; }\n",
        );
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, compactp_syntax::SyntaxKind::MEMBER_EXPR);
        let env = Env::new();
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &env, &[], &expr);
        // `.b` must project to g2's witness only (g1 dropped -> field granularity).
        let got = witnesses_of(&out);
        assert_eq!(got.len(), 1, "one field's witness");
        assert_eq!(
            got[0].info,
            WitnessInfo::WitnessReturn {
                function: "g2".into()
            }
        );
    }

    #[test]
    fn cast_passthrough_keeps_taint() {
        let (db, src, fd, ws, file) =
            walk_setup("export circuit f(): Field { return w as Field; }\n");
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, compactp_syntax::SyntaxKind::CAST_EXPR);
        let mut env = Env::new();
        env.insert("w".into(), Abs::Atomic(vec![witness(1)]));
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &env, &[], &expr);
        assert_eq!(witnesses_of(&out).len(), 1, "cast does not sanitize");
    }

    #[test]
    fn comparison_unions_to_atomic() {
        let (db, src, fd, ws, file) =
            walk_setup("export circuit f(): Boolean { return w == 0; }\n");
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, compactp_syntax::SyntaxKind::BINARY_EXPR);
        let mut env = Env::new();
        env.insert("w".into(), Abs::Atomic(vec![witness(1)]));
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &env, &[], &expr);
        assert!(
            matches!(out, Abs::Atomic(_)),
            "comparison result is Atomic, not Boolean"
        );
        assert_eq!(witnesses_of(&out).len(), 1);
    }

    #[test]
    fn if_joins_both_branches() {
        let (db, src, fd, ws, file) =
            walk_setup("export circuit f(): Field { return c ? a : b; }\n");
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, compactp_syntax::SyntaxKind::TERNARY_EXPR);
        let mut env = Env::new();
        env.insert("c".into(), Abs::Atomic(vec![])); // runtime (non-const) condition
        env.insert("a".into(), Abs::Atomic(vec![witness(1)]));
        env.insert("b".into(), Abs::Atomic(vec![witness(2)]));
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &env, &[], &expr);
        assert_eq!(
            witnesses_of(&out).len(),
            2,
            "join unions both branch witnesses"
        );
    }

    #[test]
    fn arithmetic_keeps_taint() {
        let (db, src, fd, ws, file) = walk_setup("export circuit f(): Field { return w + 0; }\n");
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, compactp_syntax::SyntaxKind::BINARY_EXPR);
        let mut env = Env::new();
        env.insert("w".into(), Abs::Atomic(vec![witness(1)]));
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &env, &[], &expr);
        assert_eq!(witnesses_of(&out).len(), 1, "arithmetic does not sanitize");
    }

    #[test]
    fn unhandled_form_emits_advisory_and_keeps_taint() {
        // `!w` (UnaryExpr) is not in the R0 index -> fail-closed catch-all.
        let (db, src, fd, ws, file) = walk_setup("export circuit f(): Boolean { return !w; }\n");
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, compactp_syntax::SyntaxKind::UNARY_EXPR);
        let mut env = Env::new();
        env.insert("w".into(), Abs::Atomic(vec![witness(1)]));
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &env, &[], &expr);
        assert!(
            !sink.advisories().is_empty(),
            "unhandled form emits advisory"
        );
        assert_eq!(witnesses_of(&out).len(), 1, "unhandled form keeps taint");
    }

    #[test]
    fn native_call_fails_closed_to_amber_clean() {
        // Unresolved callee -> native/unknown path (A6 deferral): fail closed to
        // an AMBER advisory with a CLEAN result. The arg taint does NOT propagate
        // through the unmodeled callee (a tainted result would become a
        // false-positive RED leak at a downstream sink for a native that is
        // actually a sanitizer, e.g. `persistentCommit`). A6's conduit table
        // promotes the real conduits (e.g. `persistentHash`) back to tainted.
        let (db, src, fd, ws, file) =
            walk_setup("export circuit f(): Field { return blackbox(w); }\n");
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, compactp_syntax::SyntaxKind::CALL_EXPR);
        let mut env = Env::new();
        env.insert("w".into(), Abs::Atomic(vec![witness(1)]));
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &env, &[], &expr);
        assert!(
            !sink.advisories().is_empty(),
            "native/unknown call emits advisory"
        );
        assert!(
            witnesses_of(&out).is_empty(),
            "native call result is clean (amber, not red) — taint deferred to A6"
        );
    }

    #[test]
    fn const_binding_binds_env() {
        let (db, src, fd, ws, file) = walk_setup(
            "witness getW(): Field;\nexport circuit f(): Field { const s = getW(); return s; }\n",
        );
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let block = root
            .descendants()
            .find(|n| n.kind() == compactp_syntax::SyntaxKind::BLOCK)
            .and_then(compactp_ast::Block::cast)
            .expect("circuit body block");
        let mut env = Env::new();
        let mut sink = DisclosureSink::new();
        interp_stmt(
            &ctx,
            &mut sink,
            &mut env,
            &[],
            &compactp_ast::Stmt::Block(block),
        );
        let bound = env.get("s").expect("const binding present");
        assert_eq!(
            witnesses_of(bound).len(),
            1,
            "const binds the witness-return abs"
        );
    }

    #[test]
    fn name_bound_struct_member_projects_precisely() {
        // Step 0 (A3 review deferral): a NAME-bound struct member must project
        // to the accessed field only. Pre-fix, `field_index` could not resolve a
        // name's static struct type, so `interp_member` fell to a flat
        // `Atomic(union of ALL fields)` -> the clean field over-tainted (a false
        // positive once sinks exist). This test FAILS against that over-taint.
        let (db, src, fd, ws, file) = walk_setup(
            "witness getW(): Field;\nstruct S { secret: Field, clean: Field }\n\
             export circuit f(): [] {\n\
             const s = S { secret: getW(), clean: 0 };\n\
             const a = s.clean;\n\
             const b = s.secret;\n\
             }\n",
        );
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let block = root
            .descendants()
            .find(|n| n.kind() == compactp_syntax::SyntaxKind::BLOCK)
            .and_then(compactp_ast::Block::cast)
            .expect("circuit body block");
        let mut env = Env::new();
        let mut sink = DisclosureSink::new();
        interp_stmt(
            &ctx,
            &mut sink,
            &mut env,
            &[],
            &compactp_ast::Stmt::Block(block),
        );
        // `a = s.clean` -> the clean field only: NO witnesses.
        let a = env.get("a").expect("binding a present");
        assert!(
            witnesses_of(a).is_empty(),
            "clean field must not carry the secret's witness (got {:?})",
            witnesses_of(a)
        );
        // `b = s.secret` -> the witness field: carries getW's witness.
        let b = env.get("b").expect("binding b present");
        assert_eq!(
            witnesses_of(b).len(),
            1,
            "secret field must carry exactly its own witness"
        );
        assert_eq!(
            witnesses_of(b)[0].info,
            WitnessInfo::WitnessReturn {
                function: "getW".into()
            }
        );
    }

    #[test]
    fn out_of_order_struct_literal_member_projects_precisely() {
        // Corpus-safety regression: a struct literal written OUT OF declared
        // field order must still project by field identity. The `Multiple` is
        // built in DECLARED order (`a`, `b`), so `s.b` (declared idx 1) resolves
        // to the clean field even though the literal lists `b` first. Against a
        // source-order `Multiple`, `s.b` would index the FIRST child (`a`'s
        // witness) -> a confident WRONG index -> false-positive red on a clean
        // field (compactc accepts this).
        let (db, src, fd, ws, file) = walk_setup(
            "witness getW(): Uint<8>;\nstruct S { a: Uint<8>, b: Uint<8> }\n\
             export circuit f(): [] {\n\
             const s = S { b: 0, a: getW() };\n\
             const clean = s.b;\n\
             const secret = s.a;\n\
             }\n",
        );
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let block = root
            .descendants()
            .find(|n| n.kind() == compactp_syntax::SyntaxKind::BLOCK)
            .and_then(compactp_ast::Block::cast)
            .expect("circuit body block");
        let mut env = Env::new();
        let mut sink = DisclosureSink::new();
        interp_stmt(
            &ctx,
            &mut sink,
            &mut env,
            &[],
            &compactp_ast::Stmt::Block(block),
        );
        // `s.b` -> the clean field (declared idx 1): NO witnesses.
        let clean = env.get("clean").expect("binding clean present");
        assert!(
            witnesses_of(clean).is_empty(),
            "clean field b must not carry a's witness (got {:?})",
            witnesses_of(clean)
        );
        // `s.a` -> the witness field (declared idx 0): carries getW.
        let secret = env.get("secret").expect("binding secret present");
        assert_eq!(
            witnesses_of(secret).len(),
            1,
            "field a must carry exactly its own witness"
        );
        assert_eq!(
            witnesses_of(secret)[0].info,
            WitnessInfo::WitnessReturn {
                function: "getW".into()
            }
        );
    }

    #[test]
    fn return_sink_records_only_witness_return_taint() {
        // K7 asymmetry: a witness-return leaks on `return`; a circuit argument
        // does not. Drive `interp_root` (which arms the return sink).
        let (db, src, fd, ws, file) = walk_setup(
            "witness getW(): Uint<8>;\n\
             export circuit leaky(): Uint<8> { return getW(); }\n\
             export circuit ok(x: Uint<8>): Uint<8> { return x; }\n",
        );
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let roots = discover_roots(&tree);
        let mut sink = DisclosureSink::new();
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        let leaks = sink.drain_leaks();
        // Exactly one leak: `leaky`'s witness return. `ok` returns its own arg
        // (CircuitArg) -> filtered out.
        assert_eq!(leaks.len(), 1, "only the witness-return circuit leaks");
        assert_eq!(
            leaks[0].nature,
            "the value returned from exported circuit leaky"
        );
        assert_eq!(leaks[0].witnesses.len(), 1);
    }

    #[test]
    fn ledger_cell_write_records_leak() {
        // K1 ledger sink: `c = getW()` writes a witness value to a ledger Cell.
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             export ledger c: Uint<8>;\n\
             witness getW(): Uint<8>;\n\
             export circuit f(): [] { c = getW(); }\n",
        );
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let roots = discover_roots(&tree);
        let mut sink = DisclosureSink::new();
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        let leaks = sink.drain_leaks();
        assert_eq!(leaks.len(), 1, "the ledger write leaks the witness");
        assert_eq!(leaks[0].nature, "ledger operation");
        assert_eq!(leaks[0].witnesses.len(), 1);
    }

    // -- A5 implicit-flow (control-witness) tests -----------------------

    #[test]
    fn control_witness_threads_through_if_and_leaks_at_sink() {
        // A5 (K2): a ledger Cell-write gated on a witness records a CONTROL-flow
        // leak on the gating witness, and NO data leak on the clean written
        // value `5` (record_leak no-ops on an empty witness set).
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             export ledger c: Uint<8>;\n\
             witness getW(): Boolean;\n\
             export circuit f(): [] { if (getW()) { c = 5; } }\n",
        );
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let roots = discover_roots(&tree);
        let mut sink = DisclosureSink::new();
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        let leaks = sink.drain_leaks();
        assert_eq!(
            leaks.len(),
            1,
            "only the control-flow leak, not a data leak on the clean `5`"
        );
        assert_eq!(leaks[0].nature, "performing this ledger operation");
        assert_eq!(leaks[0].witnesses.len(), 1);
        assert_eq!(
            leaks[0].witnesses[0].info,
            WitnessInfo::WitnessReturn {
                function: "getW".into()
            }
        );
    }

    #[test]
    fn control_leak_is_distinct_from_data_leak() {
        // A witness written to a ledger field UNDER a DIFFERENT witness control
        // yields TWO distinct leak-table entries: the DATA leak (the written
        // value's witness) and the CONTROL leak (the gating witness), keyed by
        // distinct nature strings.
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             export ledger c: Uint<8>;\n\
             witness getCtl(): Boolean;\n\
             witness getData(): Uint<8>;\n\
             export circuit f(): [] { if (getCtl()) { c = getData(); } }\n",
        );
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let roots = discover_roots(&tree);
        let mut sink = DisclosureSink::new();
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        let mut leaks = sink.drain_leaks();
        // Sort by nature: "ledger operation" (data) < "performing …" (control).
        leaks.sort_by(|a, b| a.nature.cmp(&b.nature));
        assert_eq!(leaks.len(), 2, "distinct data + control leak entries");
        assert_eq!(leaks[0].nature, "ledger operation");
        assert_eq!(leaks[0].witnesses.len(), 1);
        assert_eq!(
            leaks[0].witnesses[0].info,
            WitnessInfo::WitnessReturn {
                function: "getData".into()
            }
        );
        assert_eq!(leaks[1].nature, "performing this ledger operation");
        assert_eq!(leaks[1].witnesses.len(), 1);
        assert_eq!(
            leaks[1].witnesses[0].info,
            WitnessInfo::WitnessReturn {
                function: "getCtl".into()
            }
        );
    }

    #[test]
    fn disclosed_condition_clears_control_no_leak() {
        // `if (disclose(getW())) { c = 5; }` — the disclosed condition has an
        // empty witness set, so `control` stays empty inside the branch and NO
        // control leak fires (the `implicit_flow_disclosed` invariant).
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             export ledger c: Uint<8>;\n\
             witness getW(): Boolean;\n\
             export circuit f(): [] { if (disclose(getW())) { c = 5; } }\n",
        );
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let roots = discover_roots(&tree);
        let mut sink = DisclosureSink::new();
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        assert!(
            sink.drain_leaks().is_empty(),
            "a disclosed condition clears control ⇒ no control leak"
        );
    }

    #[test]
    fn ledger_method_call_control_leaks_and_keeps_advisory() {
        // A5/A6 boundary: a ledger-field METHOD call (`c.increment(1)`,
        // c: Counter) gated on a witness records the control leak, while KEEPING
        // the amber advisory for the still-unmodeled per-arg DATA flags (A6). No
        // container-op DATA leak is recorded here.
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             export ledger c: Counter;\n\
             witness getW(): Boolean;\n\
             export circuit f(): [] { if (getW()) { c.increment(1); } }\n",
        );
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let roots = discover_roots(&tree);
        let mut sink = DisclosureSink::new();
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        let advisory_kept = !sink.advisories().is_empty();
        let leaks = sink.drain_leaks();
        assert_eq!(
            leaks.len(),
            1,
            "only the control leak (no container-op data)"
        );
        assert_eq!(leaks[0].nature, "performing this ledger operation");
        assert_eq!(leaks[0].witnesses.len(), 1);
        assert_eq!(
            leaks[0].witnesses[0].info,
            WitnessInfo::WitnessReturn {
                function: "getW".into()
            }
        );
        assert!(
            advisory_kept,
            "amber advisory kept for the unmodeled container-op data flags (A6)"
        );
    }
}
