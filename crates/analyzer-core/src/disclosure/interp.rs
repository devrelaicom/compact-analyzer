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

use std::collections::{HashMap, HashSet};

use compactp_ast::AstNode;
use compactp_ast::expr::{CallExpr, Expr, FoldExpr, LambdaExpr, MapExpr};
use compactp_syntax::{SyntaxKind, SyntaxNode};
use text_size::TextRange;

use crate::Definition;
use crate::db::{
    Db, FileDeps, SourceText, Workspace, item_symbol, item_tree, resolve_in_file, resolve_query,
};
use crate::item_tree::{ItemTree, SymbolKind};
use crate::ty::TyKind;

use super::abs::{
    Abs, PathPoint, Witness, WitnessInfo, abs_equal, combine_abs, disclose, merge_witnesses,
    witnesses_of,
};
use super::leaks::DisclosureSink;
use super::tables;

/// A root's seeded parameter environment: variable name -> its `Abs`.
/// A3's interpreter looks up identifiers here when walking a root's body.
pub type Env = HashMap<String, Abs>;

/// The interprocedural memo cache key + value (B0 Q2, walk-local `call-ht` per
/// the memoization ADR). The key is the 3 axes the compiler's `key-equal?`
/// compares — `(callee_file, callee_symbol, arg_shapes, control)` — and the
/// value is the callee body's returned `Abs` (B3 `interp_circuit_call`). `Abs`
/// and `Witness` derive `Hash`/`Eq` structurally; that is STRICTER than
/// `abs-equal?`/`same-witnesses?`, which only ever costs extra (idempotent)
/// re-walks, never a wrong reuse (see `Witness` in `abs.rs`).
pub type CallCache = HashMap<(crate::FileId, u32, Vec<Abs>, Vec<Witness>), Abs>;

/// Hard bound on cross-circuit recursion depth (spec §0, fail-closed). Recursion
/// is forbidden upstream of the WPP (B0 Q4, `reject-recursive-circuits`), so the
/// `active`-set guard is the real cycle catch; this depth cap is
/// belt-and-suspenders against pathological non-cyclic nesting so the walk always
/// terminates rather than blowing the stack.
const MAX_CALL_DEPTH: u32 = 256;

/// Walk-local state threaded through the interprocedural interpreter (B2/B3).
/// Scoped to a single `disclosure_diagnostics_query` run — built fresh per root
/// walk in `interp_root`, discarded when the walk returns (memoization ADR,
/// Option A). Mirrors the compiler's `call-ht` + `Call-inprocess` machinery
/// (`analysis-passes.ss` ~5013-5116, B0 Q1/Q2/Q4).
#[derive(Default)]
pub struct WalkState {
    /// The walk-local memo cache (`call-ht`, B0 Q2): `interp_circuit_call`
    /// looks up on a HIT (return cached `Abs`, no re-walk) and inserts on a MISS
    /// (after walking the callee body). Keyed on the 3 axes `key-equal?` uses
    /// `(callee_file, callee_symbol, arg_shapes, control)`; scoped to one root
    /// walk (memoization ADR, Option A).
    pub cache: CallCache,
    /// The `Call-inprocess` recursion guard (B0 Q4): the `(file, symbol)` of
    /// every circuit currently on the active call stack. A callee already in
    /// this set is a cycle ⇒ fail closed (amber advisory + clean result), never
    /// an infinite loop.
    pub active: HashSet<(crate::FileId, u32)>,
    /// Current cross-circuit call depth (guards `MAX_CALL_DEPTH`).
    pub depth: u32,
    /// Walk-local return-value witness accumulator: `interp_stmt`'s `return`
    /// arm merges each returned value's witnesses here so a callee's
    /// flow-back value (`interp_body_returning`) picks up returns nested inside
    /// control flow (`if`/`for`) too, not just a tail `return`. Fail-closed:
    /// a return whose value the analyzer can only reach through a branch still
    /// taints the caller-side result (never a silent under-taint). Saved/restored
    /// around each `interp_circuit_call` so a callee's returns don't leak into
    /// the caller's accumulator.
    pub ret_ws: Vec<Witness>,
}

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

/// Discovers the analysis roots (spec §3): every TOP-LEVEL exported circuit
/// plus the file's constructor (R0 S2/S3). Non-exported circuits are excluded;
/// they are analyzed only through call sites (v3b).
///
/// A circuit defined inside a `module { }` (`is_module_nested`) is a root ONLY
/// when it is re-exported to the top level (B4). The compiler treats a module
/// `export circuit` as a disclosing `id-exported?` root iff a top-level
/// `export { name }` list names it (through the import/export chain); a module
/// `export circuit` that is merely module-exported, or imported-without-export,
/// is NOT a root (compactc ACCEPTs such a file — verified against
/// `compact 0.31.1`, task-B4-findings.md probes a/c/d). `reexported_module_circuits`
/// computes exactly that set (same-file resolution); a member of it becomes a
/// real `Root::Circuit` and is walked like any other exported circuit.
///
/// A module-nested exported circuit that is NOT in that set stays EXCLUDED and
/// records an amber advisory via `sink` (spec §0, fail-closed). Seeding one as a
/// root would flag its arg→ledger writes as false-positive `E3100` leaks on
/// compiler-accepted module programs (corpus FP #2). Excluding it fails CLOSED
/// to amber — never a false-positive red, and never silent green (the advisory
/// records the exclusion).
pub fn discover_roots(ctx: &InterpCtx, tree: &ItemTree, sink: &mut DisclosureSink) -> Vec<Root> {
    let reexported = reexported_module_circuits(ctx, tree, sink);
    tree.symbols
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match s.kind {
            SymbolKind::Circuit if s.exported => {
                if is_module_nested(tree, i as u32) && !reexported.contains(&(i as u32)) {
                    // Module-exported but not re-exported to the top level: not a
                    // root (compactc ACCEPTs it there). Fail closed to amber.
                    sink.emit_advisory(
                        s.name_range,
                        format!(
                            "module-nested circuit `{}` is not re-exported to the top level, so \
                             it is not analyzed as a disclosing root",
                            s.name
                        ),
                    );
                    None
                } else {
                    // A top-level exported circuit, OR a module circuit the top
                    // level re-exports (B4): a real `id-exported?` disclosing root.
                    Some(Root::Circuit {
                        name: s.name.clone(),
                        symbol: i as u32,
                    })
                }
            }
            SymbolKind::Constructor => Some(Root::Constructor { symbol: i as u32 }),
            _ => None,
        })
        .collect()
}

/// The set of symbol indices of module-nested circuits that a TOP-LEVEL
/// `export { ... }` list re-exports — the module circuits the compiler roots as
/// `id-exported?` disclosing roots (B4). For each name in each top-level export
/// list, resolve it through the file's own scope/imports (`resolve_in_file`,
/// the same resolution `classify_callee` uses for calls); a name that resolves
/// to a SAME-FILE module-nested `Circuit` symbol is a re-exported root.
///
/// Fail-closed (§0): a top-level export name that resolves to a circuit in a
/// DIFFERENT file (a cross-file re-export we do not seed/walk here) records an
/// amber advisory at the export name rather than silently dropping a potential
/// root. Names that resolve to non-circuits (types, ledgers, witnesses) or to a
/// top-level (non-module) circuit are ignored here — the latter is already a
/// root via its own `export` keyword.
fn reexported_module_circuits(
    ctx: &InterpCtx,
    tree: &ItemTree,
    sink: &mut DisclosureSink,
) -> HashSet<u32> {
    let mut out = HashSet::new();
    let root = SyntaxNode::new_root(crate::db::parsed(ctx.db, ctx.src).green);
    let Some(sf) = compactp_ast::SourceFile::cast(root) else {
        return out;
    };
    for item in sf.items() {
        let compactp_ast::Item::ExportList(exports) = item else {
            continue;
        };
        for name_tok in exports.names() {
            let name = name_tok.text().to_string();
            let Some(Definition::Item { file: df, index }) = resolve_in_file(
                ctx.db,
                ctx.file,
                ctx.src,
                ctx.fd,
                ctx.ws,
                name_tok.text_range().start(),
                name.clone(),
            ) else {
                continue;
            };
            if df == ctx.file {
                if let Some(sym) = tree.symbols.get(index as usize)
                    && sym.kind == SymbolKind::Circuit
                    && is_module_nested(tree, index)
                {
                    out.insert(index);
                }
            } else if item_symbol(ctx.db, ctx.file, ctx.src, ctx.fd, df, index)
                .is_some_and(|s| s.kind == SymbolKind::Circuit)
            {
                // Cross-file re-exported circuit: a root we cannot seed/walk here
                // (needs the target file's SourceText + params). §0 fail-closed —
                // never silently skip a potential disclosing root.
                sink.emit_advisory(
                    name_tok.text_range(),
                    format!(
                        "re-exported circuit `{name}` is defined in another module/file; not \
                         analyzed as a disclosing root here",
                    ),
                );
            }
        }
    }
    out
}

/// Whether symbol `i`'s enclosing-scope chain runs through a `module { }`. A
/// `Symbol::parent` points at the containing module/struct/enum/contract; a
/// circuit written inside a module (possibly nested several modules deep) has a
/// `SymbolKind::Module` somewhere on that chain. Top-level circuits have no
/// module ancestor (`parent` is `None`, or a non-module container). Used by
/// `discover_roots` to fail closed on module-nested circuits (v3b).
fn is_module_nested(tree: &ItemTree, i: u32) -> bool {
    let mut cur = tree.symbols.get(i as usize).and_then(|s| s.parent);
    while let Some(p) = cur {
        let Some(sym) = tree.symbols.get(p as usize) else {
            break;
        };
        if sym.kind == SymbolKind::Module {
            return true;
        }
        cur = sym.parent;
    }
    false
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
    // One walk-local `WalkState` per root walk (memoization ADR, Option A): the
    // interprocedural cache + recursion guard are scoped to this walk. Sharing a
    // single cache across roots in one file would also be sound, but a per-root
    // state keeps roots independent and matches the compiler's single-driver
    // `handle-call` entry (B0 Q3: roots are never cache-looked-up). The root's
    // own return value is discarded (only non-root callees flow theirs back).
    let mut state = WalkState::default();
    interp_stmt(
        &ctx,
        sink,
        &mut state,
        &mut env,
        &[],
        &compactp_ast::Stmt::Block(body),
    );
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
    /// A user `circuit` resolved to a definition we can interpret (B2): carries
    /// the def's `(file, symbol)` so `interp_circuit_call` can locate its
    /// params + body. Same-file for B2; cross-module is deferred inside
    /// `interp_circuit_call` (B4 lifts it).
    Circuit { file: crate::FileId, symbol: u32 },
    /// A circuit-shaped callee we can NOT interpret here and defer fail-closed
    /// (amber advisory + clean result, §0): an unresolved name (a module-imported
    /// circuit v3a's resolution can't follow), a bodyless `CircuitSig`, or a
    /// cross-contract `ContractCircuit`. Distinct from `Native` — it must NOT
    /// conduit-taint its args (that would false-positive on compiler-accepted
    /// module programs; see `unresolved_callee_class`).
    DeferredCircuit,
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
    state: &mut WalkState,
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
            Some(inner) => interp_expr(ctx, sink, state, env, control, inner),
            None => Abs::Atomic(Vec::new()),
        },
        // Cast / `as` (R0 P2): identity passthrough — casts carry taint
        // unchanged, they do NOT sanitize. The `Type` child does not cast to
        // `Expr`, so the sole `Expr` child is the operand.
        Expr::Cast(_) => match child_exprs(expr.syntax()).first() {
            Some(operand) => interp_expr(ctx, sink, state, env, control, operand),
            None => Abs::Atomic(Vec::new()),
        },
        // `disclose(e)` (R0 D1): the ONLY sanitizer. Interpret the argument,
        // then clear witnesses at every level.
        Expr::Disclose(_) => {
            let arg = match child_exprs(expr.syntax()).first() {
                Some(e) => interp_expr(ctx, sink, state, env, control, e),
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
        Expr::Binary(b) => interp_binary(ctx, sink, state, env, control, b, src),
        Expr::Ternary(_) => interp_ternary(ctx, sink, state, env, control, expr, src),
        Expr::Call(call) => interp_call(ctx, sink, state, env, control, call, src),
        Expr::Member(m) => interp_member(ctx, sink, state, env, control, m, src),
        Expr::Index(_) => interp_index(ctx, sink, state, env, control, expr, src),
        // Struct literal (R0 P5 `new`): `Multiple`, one child per field, slotted
        // into DECLARED field order so it agrees with the projection path
        // (`field_index`/`abs_of_type`, both declared-order). A struct-update
        // (spread) can't be merged positionally here -> fail closed.
        Expr::Struct(s) => {
            if s.update().is_some() {
                return fail_closed_expr(
                    ctx,
                    sink,
                    state,
                    env,
                    control,
                    expr,
                    "struct-update (spread) construction not modeled",
                );
            }
            interp_struct_literal(ctx, sink, state, env, control, s, src)
        }
        // Array literal (R0 AB1/P5 `vector`): arrays are tracked in the
        // aggregate -> `Single` over the join of all element abses.
        Expr::Array(a) => {
            let elems = child_exprs(a.syntax());
            let mut acc: Option<Abs> = None;
            for e in &elems {
                let ea = interp_expr(ctx, sink, state, env, control, e);
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
                let a = interp_expr(ctx, sink, state, env, control, &e);
                ws = merge_witnesses(&ws, &witnesses_of(&a));
            }
            Abs::Atomic(ws)
        }
        // `map`/`fold` (R0 FX1/FX2/FX3, A7): interpret the lambda body over the
        // sequences' element abstracts — an inner `disclose` clears, an inner
        // sink records — with the fold iterated to a bounded `abs_equal` fixed
        // point. Routed out of the fail-closed catch-all.
        Expr::Map(m) => interp_map(ctx, sink, state, env, control, m),
        Expr::Fold(f) => interp_fold(ctx, sink, state, env, control, f),
        // Not in the R0 index (or deferred): fail closed.
        // `slice`/`pad`/`spread`/`lambda`/unary -> unmodeled. Advisory +
        // conservative union of subexpr witnesses.
        Expr::Slice(_) => fail_closed_expr(
            ctx,
            sink,
            state,
            env,
            control,
            expr,
            "slice projection not modeled",
        ),
        Expr::Pad(_) => fail_closed_expr(ctx, sink, state, env, control, expr, "pad not modeled"),
        Expr::Spread(_) => {
            fail_closed_expr(ctx, sink, state, env, control, expr, "spread not modeled")
        }
        Expr::Unary(_) => fail_closed_expr(
            ctx,
            sink,
            state,
            env,
            control,
            expr,
            "unary operator not modeled",
        ),
        Expr::Lambda(_) => {
            fail_closed_expr(ctx, sink, state, env, control, expr, "lambda not modeled")
        }
    }
}

/// Binary operators: `+ - *` (R0 F5, taint kept), the six comparisons (R0 F4,
/// collapse to a scalar `Atomic`), `&& ||` (R0 F3, desugared to `if` in the
/// compiler — keep all operand taint as a scalar). Any other operator (`/`,
/// bit-ops, …) is not in R0 -> fail closed.
fn interp_binary(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    state: &mut WalkState,
    env: &Env,
    control: &[Witness],
    b: &compactp_ast::expr::BinaryExpr,
    src: TextRange,
) -> Abs {
    let kids = child_exprs(b.syntax());
    let op = b.op().map(|t| t.kind());
    let lhs = kids
        .first()
        .map(|e| interp_expr(ctx, sink, state, env, control, e))
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
        .map(|e| interp_expr(ctx, sink, state, env, &rhs_control, e))
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
    state: &mut WalkState,
    env: &Env,
    control: &[Witness],
    expr: &Expr,
    src: TextRange,
) -> Abs {
    let kids = child_exprs(expr.syntax());
    let cond = kids
        .first()
        .map(|e| interp_expr(ctx, sink, state, env, control, e))
        .unwrap_or_else(|| Abs::Atomic(Vec::new()));
    // Both branches are gated on the condition (R0 F1/§3.3): thread its
    // witnesses into `control` so a sink inside either branch records the
    // control-flow leak. DISTINCT from the F1(d) DATA fold below, which folds
    // the condition into the branch VALUE.
    let branch_control = thread_control(witnesses_of(&cond), control, src);
    let then_abs = kids
        .get(1)
        .map(|e| interp_expr(ctx, sink, state, env, &branch_control, e))
        .unwrap_or_else(|| Abs::Atomic(Vec::new()));
    let else_abs = kids
        .get(2)
        .map(|e| interp_expr(ctx, sink, state, env, &branch_control, e))
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
    state: &mut WalkState,
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
        // A cross-circuit call to a resolved user circuit (v3b B2): interpret
        // the callee body under the interpreted arg shapes so witness taint flows
        // through the helper to its ledger/return sinks and back to the caller.
        CalleeClass::Circuit { file: cf, symbol } => {
            interp_circuit_call(ctx, sink, state, env, control, call, cf, symbol, src)
        }
        // A circuit-shaped callee we can't interpret here (unresolved / bodyless
        // sig / cross-contract): the taint semantics of the RESULT are not
        // modeled, so fail closed to an AMBER advisory (spec §0) and return a
        // CLEAN result — NOT a conservatively-tainted one. A tainted result would
        // flow to a sink as a confirmed RED leak, a false positive on
        // compiler-accepted module programs (the `top_func` → `mf(x)` pattern);
        // §0 says fail-closed is *amber*, not red. Arguments are still
        // interpreted (for their own nested sinks/advisories).
        CalleeClass::DeferredCircuit => {
            interp_args_for_effect(ctx, sink, state, env, control, call);
            sink.emit_advisory(src, "cross-circuit call not interpreted (v3b)".to_string());
            Abs::Atomic(Vec::new())
        }
        CalleeClass::Native => {
            // Interpret args KEEPING their taint (the conduit folds flagged
            // args' witnesses into the result; the ledger op leaks flagged
            // args) — unlike the Circuit arm, which discards it.
            let args: Vec<Abs> = arg_exprs(call)
                .iter()
                .map(|a| interp_expr(ctx, sink, state, env, control, a))
                .collect();
            let name = call
                .name()
                .map(|t| t.text().to_string())
                .unwrap_or_default();

            if is_method_call(call) {
                interp_ledger_op(ctx, sink, call, control, &name, &args, src)
            } else {
                // A plain call classified Native is a stdlib/builtin primitive
                // (natives don't resolve to a user symbol) — the N2 conduit
                // table, or the fail-closed unknown-native default.
                match tables::native_conduit(&name) {
                    Some(flags) => native_conduit_result(&name, flags, &args, src),
                    None => unknown_native_result(sink, &name, &args, src),
                }
            }
        }
    }
}

/// Interprets a cross-circuit call (B2 — the core of v3b). Locates the callee's
/// params + body, interprets each argument to an `Abs` (keeping taint), binds a
/// FRESH `empty-env` of `param -> arg_shape` (the callee does NOT inherit the
/// caller's env — B0 Q1, `extend-env empty-env var-name* abs*`), threads the
/// CALLER's `control` in unchanged (B0 Q1), and walks the callee body with
/// `disclosing_fn = None` (a non-root callee's `return` must NOT leak — the K7
/// filter, B0 Q3). The callee's returned `Abs` flows back so caller-side taint
/// propagates. Callee-body ledger/return sinks record into the shared `sink`.
///
/// Fail-closed (§0) at every unanalyzable point — a recursion cycle / excessive
/// depth (B0 Q4), an unresolvable or cross-file callee (cross-module deferred to
/// B4), or an arity mismatch — records an amber advisory and returns a CLEAN
/// `Atomic([])`: never an infinite loop, never a silent green, never a
/// conduit-tainted red.
#[allow(clippy::too_many_arguments)]
fn interp_circuit_call(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    state: &mut WalkState,
    env: &Env,
    control: &[Witness],
    call: &CallExpr,
    callee_file: crate::FileId,
    callee_symbol: u32,
    src: TextRange,
) -> Abs {
    // Recursion / depth guard (B0 Q4, §0 fail-closed, terminating). Recursion is
    // rejected by `reject-recursive-circuits` upstream of the WPP, so `active` is
    // the real cycle catch; the depth cap is belt-and-suspenders against
    // pathological non-cyclic nesting. Either ⇒ amber + clean, never a loop.
    let key = (callee_file, callee_symbol);
    if state.depth >= MAX_CALL_DEPTH || state.active.contains(&key) {
        // Fail-closed WITHOUT dropping a leak nested in an argument (§0): a
        // sub-call inside an arg (e.g. `f(helper(secret))` where `helper` writes
        // `secret` to a ledger) must still record its own sink even though THIS
        // call is un-walkable. Interpret args for effect before the advisory,
        // exactly as the sibling `DeferredCircuit` arm does.
        interp_args_for_effect(ctx, sink, state, env, control, call);
        sink.emit_advisory(
            src,
            "recursive or deeply-nested circuit call not analyzed".to_string(),
        );
        return Abs::Atomic(Vec::new());
    }
    // Locate the callee's params + body (same-file for B2; cross-module is B4).
    let Some(callee) = resolve_circuit_def(ctx, callee_file, callee_symbol) else {
        // Same §0 guard as the recursion branch: realize any sink nested in an
        // argument before failing closed (this cross-file path activates in B4).
        interp_args_for_effect(ctx, sink, state, env, control, call);
        sink.emit_advisory(
            src,
            "cross-circuit call target could not be resolved (cross-module deferred to v3b)"
                .to_string(),
        );
        return Abs::Atomic(Vec::new());
    };
    // Interpret the arguments in the CALLER's env/control (keeping taint). Their
    // own nested sinks/advisories fire here regardless of the callee body.
    let arg_shapes: Vec<Abs> = arg_exprs(call)
        .iter()
        .map(|a| interp_expr(ctx, sink, state, env, control, a))
        .collect();
    if arg_shapes.len() != callee.params.len() {
        sink.emit_advisory(src, "circuit call arity mismatch not analyzed".to_string());
        return Abs::Atomic(Vec::new());
    }
    // B3 walk-local `call-ht` memo (B0 Q2, memoization ADR Option A). Key on the
    // three axes `key-equal?` compares: callee identity `(file, symbol)`, the
    // evaluated arg shapes `abs*`, and the threaded `control` witness set. The
    // key uses the RAW `arg_shapes` — BEFORE this call's argument path points —
    // exactly as the source keys on raw `abs*` and adds path points only in the
    // miss/walk branch (`analysis-passes.ss` ~5066, B0 Q2/Q5).
    let memo_key = (
        callee_file,
        callee_symbol,
        arg_shapes.clone(),
        control.to_vec(),
    );
    if let Some(cached) = state.cache.get(&memo_key) {
        // HIT (B0 Q2): this exact `(uid, abs*, control)` was already walked; its
        // sinks are already recorded, and because `record_leak` dedups by
        // `(src, nature)` a re-walk could only re-derive the same rows (ADR case
        // (c)) — so returning the cached `Abs` WITHOUT re-walking is
        // observationally identical on the leak table (fail-closed, §0). A
        // non-root callee always runs with `disclosing_fn = None` (B0 Q3), so
        // there is no disclosing-context re-entry a hit could wrongly skip.
        return cached.clone();
    }
    // MISS: bind a FRESH env `param -> arg_shape` — NO caller-env inheritance
    // (B0 Q1, `extend-env empty-env var-name* abs*`). Attach the cross-circuit
    // argument path point to each bound arg (B0 Q5: the SAME `add_path_point` +
    // "the [Nth] argument to <callee>" template the native conduit uses, exposure
    // ""), so a witness passed to the helper renders a secondary span at the call
    // site on its witness->sink trail. Single-arg ⇒ no ordinal; multi-arg ⇒
    // 1-based ordinals — `arg_description` handles both.
    let mut callee_env: Env = HashMap::new();
    let arg_count = arg_shapes.len();
    for (i, (p, a)) in callee.params.iter().zip(arg_shapes.iter()).enumerate() {
        let pp = PathPoint {
            src,
            description: arg_description(&callee.name, i, arg_count),
            exposure: String::new(),
        };
        callee_env.insert(p.clone(), add_path_point(a.clone(), &pp));
    }
    // Non-root callee: `disclosing_fn = None` (K7 disarmed, B0 Q3). `callee_file`
    // is `ctx.file` for B2 (guaranteed by `resolve_circuit_def`'s same-file gate);
    // threading it explicitly keeps B4's cross-module lift a one-line change.
    let callee_ctx = InterpCtx {
        file: callee_file,
        disclosing_fn: None,
        ..*ctx
    };
    state.active.insert(key);
    state.depth += 1;
    let ret = interp_body_returning(
        &callee_ctx,
        sink,
        state,
        &mut callee_env,
        control,
        &callee.body,
    );
    state.depth -= 1;
    state.active.remove(&key);
    // Cache the callee's return `Abs` under the 3-axis key so a later call with
    // identical `(uid, abs*, control)` HITs (above) instead of re-walking — the
    // performance bound (ADR Option A), leak-set-invariant by ADR case (c).
    state.cache.insert(memo_key, ret.clone());
    ret
}

/// A callee circuit's parameter names + body block (B2). Located by the symbol's
/// `name_range` the same way `seed_sources`/`root_body` find a root's def.
struct CircuitDef {
    /// The callee circuit's name (`id-sym function-name`), used to render the
    /// B3 cross-circuit argument path point ("the [Nth] argument to <name>").
    name: String,
    /// Parameter names, in declaration order — bound positionally to the call's
    /// interpreted `arg_shapes`.
    params: Vec<String>,
    /// The circuit body block, walked by `interp_body_returning`.
    body: compactp_ast::Block,
}

/// Resolves a callee circuit's params + body from its `(file, symbol)`. `None`
/// (caller fails closed) when the callee is cross-file (B2 is same-file only;
/// B4 adds cross-module by reading the target file's `SourceText`/item tree),
/// its symbol index doesn't resolve, or the declaration has no body.
fn resolve_circuit_def(
    ctx: &InterpCtx,
    callee_file: crate::FileId,
    callee_symbol: u32,
) -> Option<CircuitDef> {
    // B2: same-file only. A cross-file circuit needs the target file's source
    // text + item tree threaded in — deferred to B4.
    if callee_file != ctx.file {
        return None;
    }
    let tree = item_tree(ctx.db, ctx.src);
    let sym = tree.symbols.get(callee_symbol as usize)?;
    let ast_root = SyntaxNode::new_root(crate::db::parsed(ctx.db, ctx.src).green);
    let cd = ast_root
        .descendants()
        .filter_map(compactp_ast::CircuitDef::cast)
        .find(|c| c.name().map(|n| n.text_range()) == Some(sym.name_range))?;
    let params = cd
        .params()
        .filter_map(|p| param_name_and_type(p).map(|(name, _, _)| name))
        .collect();
    let body = cd.body()?;
    Some(CircuitDef {
        name: sym.name.clone(),
        params,
        body,
    })
}

/// Walks a callee circuit body (B2) and returns the `Abs` that flows back to the
/// caller — the value of its `return` (the body block's value). Factored out of
/// `interp_stmt`'s block handling so a callee's returned value is capturable; the
/// root path keeps discarding it (`interp_root` ignores the body value).
///
/// A tail `return e` is captured precisely (its `Abs`, so a struct/tuple return
/// still projects field-accurately at the caller). Returns nested inside control
/// flow (`if`/`for`) are captured conservatively via the walk-local `ret_ws`
/// accumulator (`interp_stmt`'s `return` arm) and folded into the result as a
/// witness union — fail-closed (never a silent under-taint of the flow-back
/// value). A body with no `return` (a `[]`-returning helper like `stash`) flows
/// back a clean unit `Atomic([])`.
fn interp_body_returning(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    state: &mut WalkState,
    env: &mut Env,
    control: &[Witness],
    body: &compactp_ast::Block,
) -> Abs {
    // Save/restore the caller's return accumulator so this callee's returns don't
    // bleed into it (and the caller's don't bleed into the callee's flow-back).
    let saved_ret = std::mem::take(&mut state.ret_ws);
    let mut tail: Option<Abs> = None;
    for s in body.stmts() {
        if let compactp_ast::Stmt::Return(r) = &s {
            // A top-level `return`: capture its value precisely for the flow-back
            // SHAPE. The K7 return sink is disarmed for a non-root callee
            // (`disclosing_fn = None`, B0 Q3), so interpreting the value here
            // records no return leak — it only realizes any nested arg/ledger
            // sinks in the returned expression.
            let val = r
                .value()
                .map(|e| interp_expr(ctx, sink, state, env, control, &e))
                .unwrap_or_else(|| Abs::Atomic(Vec::new()));
            tail = Some(match tail {
                Some(prev) => combine_abs(&prev, &val),
                None => val,
            });
        } else {
            interp_stmt(ctx, sink, state, env, control, &s);
        }
    }
    // `ret_ws` now holds witnesses from returns nested inside control flow only
    // (the top-level returns above bypass `interp_stmt`). Restore the caller's.
    let nested = std::mem::replace(&mut state.ret_ws, saved_ret);
    let mut result = tail.unwrap_or_else(|| Abs::Atomic(Vec::new()));
    if !nested.is_empty() {
        // Fail-closed: a value returned from a branch still taints the flow-back.
        result = add_witnesses(result, &nested);
    }
    result
}

/// A method call reached as `CalleeClass::Native`: a ledger ADT operation
/// (L2/L3). It's a ledger op when its receiver is a ledger field (A5) OR its op
/// name is a known ledger op (so a Kernel op whose receiver doesn't resolve as a
/// ledger symbol is still modeled). Records the K1 per-arg DATA leaks (each arg
/// whose `discloses?` flag is truthy AND carries witnesses) and the K2
/// control-flow leak; returns the op's `default-value` (clean). A method call
/// that is NOT a recognizable ledger op fails closed to an amber advisory +
/// clean result (defensive — Compact has no user-defined methods).
fn interp_ledger_op(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    call: &CallExpr,
    control: &[Witness],
    name: &str,
    args: &[Abs],
    src: TextRange,
) -> Abs {
    let recv_is_ledger = method_receiver(call)
        .map(|r| is_ledger_field(ctx, &r))
        .unwrap_or(false);
    let known = tables::ledger_op(name);

    if !recv_is_ledger && known.is_none() {
        // Not a recognizable ledger op: fail closed (never silent green, §0),
        // record no leak (no ledger field / op to key on).
        sink.emit_advisory(
            src,
            "method call on a non-ledger receiver not modeled".to_string(),
        );
        return Abs::Atomic(Vec::new());
    }

    // K2 (A5): performing a ledger op under witness control leaks the gating
    // witnesses (no-op when `control` is empty ⇒ a top-level op records none).
    sink.record_leak(src, "performing this ledger operation", control.to_vec());

    match known {
        // L2 container op: every argument leaks with exposure "" (L1 default).
        Some(tables::LedgerOp::AllArgsLeak) => {
            leak_all_ledger_args(sink, name, args, src);
        }
        // L3 Kernel op: per-arg `discloses?` flags (Some ⇒ leak; None ⇒ hidden).
        Some(tables::LedgerOp::PerArg(flags)) => {
            for (i, (flag, arg_abs)) in flags.iter().zip(args).enumerate() {
                if let Some(exposure) = flag {
                    leak_ledger_arg(sink, name, i, flags.len(), exposure, arg_abs, src);
                }
            }
        }
        // Unknown ledger op on a ledger field: fail-closed leak of every
        // witness arg (matches the L1 default: no clause ⇒ leaks) + advisory.
        None => {
            leak_all_ledger_args(sink, name, args, src);
            sink.emit_advisory(
                src,
                format!("unknown ledger op `{name}`; leaking all witness args (fail-closed, may be stale)"),
            );
        }
    }
    Abs::Atomic(Vec::new())
}

/// Records a K1 DATA leak for every argument of a container op (L2): each arg
/// carries the default `discloses? = ""` (exposure ""), so every witness-bearing
/// arg leaks. `record_leak` no-ops on a clean (witness-free) arg.
fn leak_all_ledger_args(sink: &mut DisclosureSink, op: &str, args: &[Abs], src: TextRange) {
    for (i, arg_abs) in args.iter().enumerate() {
        leak_ledger_arg(sink, op, i, args.len(), "", arg_abs, src);
    }
}

/// Records one K1 ledger-op DATA leak: tags the arg's witnesses with the
/// `discloses?` exposure path point (`the [Nth] argument to <op>`) and records
/// them under nature `"ledger operation"` (no-op when the arg is witness-free).
fn leak_ledger_arg(
    sink: &mut DisclosureSink,
    op: &str,
    i: usize,
    arg_count: usize,
    exposure: &str,
    arg_abs: &Abs,
    src: TextRange,
) {
    let pp = PathPoint {
        src,
        description: arg_description(op, i, arg_count),
        exposure: exposure.to_string(),
    };
    let witnesses = witnesses_of(&add_path_point(arg_abs.clone(), &pp));
    sink.record_leak(src, "ledger operation", witnesses);
}

/// N2 native conduit result: a native records NO leak — it folds each flagged
/// arg's witnesses into its result with that arg's exposure. Args flagged
/// `nothing` (`None`) are hidden. The result is modeled as a flat `Atomic` of
/// the folded witnesses (all native result types are scalars or
/// structs-of-scalars derived from these same args, so a flat carrier never
/// under-taints — see `tables::native_conduit`).
fn native_conduit_result(name: &str, flags: tables::ArgFlags, args: &[Abs], src: TextRange) -> Abs {
    let mut folded: Vec<Witness> = Vec::new();
    for (i, (flag, arg_abs)) in flags.iter().zip(args).enumerate() {
        if let Some(exposure) = flag {
            let pp = PathPoint {
                src,
                description: arg_description(name, i, flags.len()),
                exposure: (*exposure).to_string(),
            };
            let tagged = witnesses_of(&add_path_point(arg_abs.clone(), &pp));
            folded = merge_witnesses(&folded, &tagged);
        }
    }
    Abs::Atomic(folded)
}

/// Fail-closed unknown-native default (R0 N1, spec §0): a plain call that is
/// neither a known native nor a resolvable witness/circuit is conduit-tainted
/// (ALL args fold into the result, exposure "") AND advised. The N2 table is
/// complete for compactc-v0.31.1, so this fires only on version drift / an
/// unresolvable callee — never silent green.
fn unknown_native_result(
    sink: &mut DisclosureSink,
    name: &str,
    args: &[Abs],
    src: TextRange,
) -> Abs {
    let mut folded: Vec<Witness> = Vec::new();
    for (i, arg_abs) in args.iter().enumerate() {
        let pp = PathPoint {
            src,
            description: arg_description(name, i, args.len()),
            exposure: String::new(),
        };
        folded = merge_witnesses(
            &folded,
            &witnesses_of(&add_path_point(arg_abs.clone(), &pp)),
        );
    }
    sink.emit_advisory(
        src,
        format!("unknown native `{name}` not in the conduit table; conduit-tainting all args (fail-closed, may be stale)"),
    );
    Abs::Atomic(folded)
}

/// A call-argument path-point description, mirroring the compiler's
/// `"the ~@[~:r ~]argument to ~a"` (`analysis-passes.ss`): the ordinal is
/// included only when the callee takes more than one argument (a single-arg
/// native/op reads "the argument to persistentHash", matching compactc).
fn arg_description(callee: &str, i: usize, arg_count: usize) -> String {
    if arg_count > 1 {
        format!("the {} argument to {}", ordinal(i + 1), callee)
    } else {
        format!("the argument to {callee}")
    }
}

/// The English ordinal word for `n` (1 -> "first"), mirroring the compiler's
/// `~:r` directive. Falls back to a numeric ordinal beyond the small set that
/// any native/ledger-op arity actually needs.
fn ordinal(n: usize) -> String {
    match n {
        1 => "first".into(),
        2 => "second".into(),
        3 => "third".into(),
        4 => "fourth".into(),
        5 => "fifth".into(),
        6 => "sixth".into(),
        7 => "seventh".into(),
        8 => "eighth".into(),
        _ => format!("{n}th"),
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
    state: &mut WalkState,
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
                Some(e) => interp_expr(ctx, sink, state, env, control, e),
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
    state: &mut WalkState,
    env: &Env,
    control: &[Witness],
    m: &compactp_ast::expr::MemberExpr,
    src: TextRange,
) -> Abs {
    let Some(base) = child_exprs(m.syntax()).into_iter().next() else {
        return Abs::Atomic(Vec::new());
    };
    let base_abs = interp_expr(ctx, sink, state, env, control, &base);
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
    state: &mut WalkState,
    env: &Env,
    control: &[Witness],
    expr: &Expr,
    src: TextRange,
) -> Abs {
    let kids = child_exprs(expr.syntax());
    let base = kids
        .first()
        .map(|e| interp_expr(ctx, sink, state, env, control, e))
        .unwrap_or_else(|| Abs::Atomic(Vec::new()));
    let idx_ws = kids
        .get(1)
        .map(|e| witnesses_of(&interp_expr(ctx, sink, state, env, control, e)))
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

// ===========================================================================
// A7: fold/map over aggregates (R0 FX1/FX2/FX3)
// ===========================================================================

/// `map(fun, seq1, …, seqN)` (R0 FX2). Binds the lambda's params to the
/// sequences' element abstracts and interprets the lambda BODY via
/// `interp_expr` (so an inner `disclose` clears taint, an inner sink records a
/// leak) to produce the result element abstract. Two regimes, mirroring the
/// compiler (`analysis-passes.ss:5402`):
/// - any sequence is a heterogeneous `Multiple` (tuple) ⇒ fully unroll,
///   applying the body per index ⇒ `Multiple` of the per-index results;
/// - otherwise (homogeneous aggregated arrays — `Single`/`Atomic`) ⇒ one
///   application over the aggregated element ⇒ `Single(result_elem)`.
///
/// No fixpoint is needed for `map` (each element is independent). A first arg
/// that is not an expression-bodied lambda (a named function reference, or a
/// block body) is unmodeled ⇒ fail closed (§0).
fn interp_map(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    state: &mut WalkState,
    env: &Env,
    control: &[Witness],
    m: &MapExpr,
) -> Abs {
    let kids = child_exprs(m.syntax());
    let Some((fun, seqs)) = kids.split_first() else {
        return fail_closed_expr(
            ctx,
            sink,
            state,
            env,
            control,
            &Expr::Map(m.clone()),
            "malformed map",
        );
    };
    let Some((body, params)) = lambda_parts(fun) else {
        return fail_closed_expr(
            ctx,
            sink,
            state,
            env,
            control,
            &Expr::Map(m.clone()),
            "map with a non-lambda function reference not modeled",
        );
    };
    // A map with no sequences degenerates to an empty aggregate (FX2 `len = 0`).
    if seqs.is_empty() {
        return Abs::Multiple(Vec::new());
    }
    let seq_abses: Vec<Abs> = seqs
        .iter()
        .map(|s| interp_expr(ctx, sink, state, env, control, s))
        .collect();

    if let Some(len) = unroll_len(&seq_abses) {
        // Heterogeneous unroll: apply the body per index.
        let results = (0..len)
            .map(|i| {
                let values = seq_abses.iter().map(|s| seq_element_at(s, i));
                let child = bind_lambda_env(env, &params, values);
                interp_expr(ctx, sink, state, &child, control, &body)
            })
            .collect();
        Abs::Multiple(results)
    } else {
        // Homogeneous aggregate: one application over the element abstracts.
        let values = seq_abses.iter().map(seq_element_abs);
        let child = bind_lambda_env(env, &params, values);
        let elem = interp_expr(ctx, sink, state, &child, control, &body);
        Abs::Single(Box::new(elem))
    }
}

/// `fold(fun, init, seq1, …, seqN)` (R0 FX1/FX3). The accumulator starts at
/// `init`; the lambda body is interpreted with its params bound to
/// `(acc, elem1, …, elemN)`. Two regimes, mirroring the compiler
/// (`analysis-passes.ss:5428`):
/// - any sequence is a heterogeneous `Multiple` (tuple) ⇒ fully unroll
///   element-by-element (exact, bounded by the tuple length);
/// - otherwise (homogeneous aggregated arrays) ⇒ iterate to an `abs_equal`
///   fixed point, HARD-BOUNDED by `fold_fixpoint_bound` so the analysis always
///   terminates (see that function for the bound + termination argument).
///
/// The ambient `control` (implicit flow) is threaded into every body
/// interpretation, so a fold inside a witness-gated branch keeps the K2/K6
/// control-leak semantics. A non-lambda function / block body / a fold missing
/// its init or sequence is unmodeled ⇒ fail closed (§0).
fn interp_fold(
    ctx: &InterpCtx,
    sink: &mut DisclosureSink,
    state: &mut WalkState,
    env: &Env,
    control: &[Witness],
    f: &FoldExpr,
) -> Abs {
    let kids = child_exprs(f.syntax());
    // fun, init, then one-or-more sequences.
    let (Some(fun), Some(init_expr), Some(seqs)) = (kids.first(), kids.get(1), kids.get(2..))
    else {
        return fail_closed_expr(
            ctx,
            sink,
            state,
            env,
            control,
            &Expr::Fold(f.clone()),
            "fold missing its function, init value, or sequence",
        );
    };
    if seqs.is_empty() {
        return fail_closed_expr(
            ctx,
            sink,
            state,
            env,
            control,
            &Expr::Fold(f.clone()),
            "fold missing its sequence",
        );
    }
    let Some((body, params)) = lambda_parts(fun) else {
        return fail_closed_expr(
            ctx,
            sink,
            state,
            env,
            control,
            &Expr::Fold(f.clone()),
            "fold with a non-lambda function reference not modeled",
        );
    };
    let init_abs = interp_expr(ctx, sink, state, env, control, init_expr);
    let seq_abses: Vec<Abs> = seqs
        .iter()
        .map(|s| interp_expr(ctx, sink, state, env, control, s))
        .collect();

    if let Some(len) = unroll_len(&seq_abses) {
        // Heterogeneous unroll: exact, `len` applications.
        let mut acc = init_abs;
        for i in 0..len {
            let values =
                std::iter::once(acc.clone()).chain(seq_abses.iter().map(|s| seq_element_at(s, i)));
            let child = bind_lambda_env(env, &params, values);
            acc = interp_expr(ctx, sink, state, &child, control, &body);
        }
        acc
    } else {
        // Homogeneous aggregate: iterate to an `abs_equal` fixed point,
        // hard-bounded for termination.
        let elems: Vec<Abs> = seq_abses.iter().map(seq_element_abs).collect();
        let bound = fold_fixpoint_bound(&init_abs, &elems);
        let mut acc = init_abs;
        for _ in 0..bound {
            let values = std::iter::once(acc.clone()).chain(elems.iter().cloned());
            let child = bind_lambda_env(env, &params, values);
            let next = interp_expr(ctx, sink, state, &child, control, &body);
            if abs_equal(&next, &acc) {
                return next;
            }
            acc = next;
        }
        acc
    }
}

/// The fold fixpoint iteration bound (R0 FX1/FX3) for the aggregated regime.
///
/// **Termination.** The loop runs at most this many times regardless of
/// convergence, so `interp_fold` always terminates — the load-bearing
/// requirement. The compiler bounds by the array's static length; once
/// `slice`/aggregation has erased that length (the abstract is a `Single`/
/// `Atomic`), we recover a finite bound from the witness UNIVERSE instead: the
/// distinct witnesses reachable from `init` and the sequence elements, plus a
/// small margin.
///
/// **Why the universe bounds convergence.** Interpreting the fixed body over a
/// fixed `env` draws witnesses only from that env (params bound to `acc` +ele-
/// ments); no operation invents a witness UID except a `witness()` call, which
/// a `map`/`fold` lambda body does not perform over these element abstracts,
/// and `disclose` only ever REMOVES. So the accumulator's witness set is
/// monotone within this universe and stabilizes within `|universe|`
/// applications; `abs_equal` early-stops there. The `+ 2` margin covers the
/// clean-init first step and the confirming equal iteration. (If a body did
/// mint fresh witnesses each step, the hard bound still guarantees termination
/// — a finite fail-closed approximation, never a hang.)
fn fold_fixpoint_bound(init: &Abs, elems: &[Abs]) -> usize {
    let mut universe = witnesses_of(init);
    for e in elems {
        universe = merge_witnesses(&universe, &witnesses_of(e));
    }
    universe.len() + 2
}

/// The lambda `(body_expr, param_names)` of a map/fold's first argument, or
/// `None` when it is not an expression-bodied lambda (a named function
/// reference, or a block-bodied lambda `… => { … }` — both unmodeled, caller
/// fails closed). Params are positional; a non-identifier param pattern yields
/// a `None` slot so index alignment with the bound element abstracts holds.
fn lambda_parts(fun: &Expr) -> Option<(Expr, Vec<Option<String>>)> {
    let Expr::Lambda(lam) = unwrap_paren(fun) else {
        return None;
    };
    let body = lambda_body_expr(&lam)?;
    let params = lambda_param_names(&lam);
    Some((body, params))
}

/// The body expression of an expression-bodied lambda (`… => expr`); `None`
/// for a block body (`… => { … }`). The body is the sole `Expr` child of the
/// `LAMBDA_EXPR` — the `PARAM_LIST` and an optional return-`Type` node do not
/// cast to `Expr`.
fn lambda_body_expr(lam: &LambdaExpr) -> Option<Expr> {
    if lam.body_block().is_some() {
        return None;
    }
    lam.syntax().children().filter_map(Expr::cast).last()
}

/// The lambda's parameter names, in order. A lambda param is a bare `IDENT_PAT`
/// (untyped, `(x, y) => …`) or a `PARAM` wrapping a pattern (typed,
/// `(x: T) => …`) — `ParamList::params` only yields the latter, so this walks
/// the `PARAM_LIST`'s child nodes directly to catch both. A non-identifier
/// pattern contributes a `None` slot (kept for positional alignment).
fn lambda_param_names(lam: &LambdaExpr) -> Vec<Option<String>> {
    let Some(pl) = lam.param_list() else {
        return Vec::new();
    };
    pl.syntax()
        .children()
        .map(|child| ident_name_of_param_child(&child))
        .collect()
}

/// The identifier name of a `PARAM_LIST` child node — either a bare `IdentPat`
/// (untyped lambda param) or a `Param` wrapping an `IdentPat` (typed). `None`
/// for any other (tuple/struct) pattern.
fn ident_name_of_param_child(child: &SyntaxNode) -> Option<String> {
    if let Some(param) = compactp_ast::Param::cast(child.clone()) {
        return match param.pattern() {
            Some(compactp_ast::Pat::Ident(id)) => id.name().map(|t| t.text().to_string()),
            _ => None,
        };
    }
    match compactp_ast::Pat::cast(child.clone()) {
        Some(compactp_ast::Pat::Ident(id)) => id.name().map(|t| t.text().to_string()),
        _ => None,
    }
}

/// A child `Env` = `base` with the lambda's params (positionally) bound to
/// `values`. A `None` param slot (non-identifier pattern) is skipped; a surplus
/// value with no matching param is ignored (`zip` stops at the shorter).
fn bind_lambda_env(
    base: &Env,
    params: &[Option<String>],
    values: impl IntoIterator<Item = Abs>,
) -> Env {
    let mut child = base.clone();
    for (name, val) in params.iter().zip(values) {
        if let Some(name) = name {
            child.insert(name.clone(), val);
        }
    }
    child
}

/// The unroll length when a fold/map should be fully unrolled: `Some(len)` iff
/// at least one sequence is a heterogeneous `Multiple` (tuple/struct), taking
/// the minimum `Multiple` arity (defensive against a shape mismatch). `None`
/// ⇒ every sequence is homogeneous (`Single`/`Atomic`), the aggregated regime.
fn unroll_len(seq_abses: &[Abs]) -> Option<usize> {
    seq_abses
        .iter()
        .filter_map(|a| match a {
            Abs::Multiple(children) => Some(children.len()),
            _ => None,
        })
        .min()
}

/// The aggregated element abstract of a sequence (map/fold homogeneous
/// regime): a `Single`'s inner element; an `Atomic`/`Boolean` stands for its
/// own aggregated element. A `Multiple` is unexpected here (the caller routes
/// any-`Multiple` to the unroll regime) — its children are joined defensively.
fn seq_element_abs(seq: &Abs) -> Abs {
    match seq {
        Abs::Single(inner) => (**inner).clone(),
        Abs::Multiple(children) => children
            .iter()
            .fold(Abs::Atomic(Vec::new()), |acc, c| combine_abs(&acc, c)),
        other => other.clone(),
    }
}

/// The element abstract at index `i` of a sequence (unroll regime): a
/// `Multiple`'s i-th child; a `Single`/`Atomic`/`Boolean` yields the same
/// aggregated element at every index.
fn seq_element_at(seq: &Abs, i: usize) -> Abs {
    match seq {
        Abs::Multiple(children) => children
            .get(i)
            .cloned()
            .unwrap_or_else(|| Abs::Atomic(Vec::new())),
        Abs::Single(inner) => (**inner).clone(),
        other => other.clone(),
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
    state: &mut WalkState,
    env: &mut Env,
    control: &[Witness],
    stmt: &compactp_ast::Stmt,
) {
    use compactp_ast::Stmt;
    match stmt {
        Stmt::Block(b) => {
            for s in b.stmts() {
                interp_stmt(ctx, sink, state, env, control, &s);
            }
        }
        // `const pat = expr` (R0 P5 `let*`): interpret RHS, bind the pattern.
        Stmt::Const(c) => {
            let val = c
                .value()
                .map(|e| interp_expr(ctx, sink, state, env, control, &e))
                .unwrap_or_else(|| Abs::Atomic(Vec::new()));
            bind_pattern(sink, env, c.pattern().as_ref(), val);
        }
        // Expression statement / assert: interpret operands so their `Abs`
        // (and any advisory) is realized. Neither is a sink.
        Stmt::Expr(_) | Stmt::Assert(_) => {
            for e in child_exprs(stmt.syntax()) {
                interp_expr(ctx, sink, state, env, control, &e);
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
                let value_abs = interp_expr(ctx, sink, state, env, control, value);
                if is_ledger_field(ctx, target) {
                    // v3c C3 (minimal-scope `disclose()` quick-fix): the DATA
                    // leak's primary span is the RHS EXPRESSION's own range —
                    // NOT `a.syntax().text_range()` (the whole `lhs = rhs;`
                    // statement, LHS and `;` included). Differential-verified
                    // against `compactc` 0.31.1: `c = disclose(getW());`
                    // compiles clean; `disclose(c = getW());` (a wrap of the
                    // whole statement) is REJECTED — the compiler re-locates
                    // the same undisclosed value at the inner RHS. A wider
                    // span here would let a quick-fix built on it silently
                    // sanitize a second, unrelated leak in the same statement.
                    let value_range = trimmed_range(value.syntax());
                    sink.record_leak(value_range, "ledger operation", witnesses_of(&value_abs));
                    // A5 (K2): conditioning a ledger op on a witness discloses
                    // that witness as a SEPARATE control-flow leak — a distinct
                    // nature ⇒ a distinct table entry (never merged into the data
                    // leak, never tainting the written value). `record_leak`
                    // no-ops when `control` is empty (fires only when gated).
                    // Kept at the statement range: this leak has no single
                    // wrappable value expression (the gating witness lives in
                    // the branch condition elsewhere), so it is never offered
                    // the `disclose()` quick-fix.
                    let stmt_range = a.syntax().text_range();
                    sink.record_leak(
                        stmt_range,
                        "performing this ledger operation",
                        control.to_vec(),
                    );
                }
            }
            // Defensive (unexpected shape): realize taint, record no sink.
            kids => {
                for e in kids {
                    interp_expr(ctx, sink, state, env, control, e);
                }
            }
        },
        // Return (R0 K7 return sink + `filter_witnesses` asymmetry, spec §3.2).
        // Only fires inside an exported-circuit root (`disclosing_fn`), and only
        // for `WitnessReturn` taint — returning the circuit's own argument (or a
        // constructor argument) is NOT a disclosure. A4 does the DATA leak; the
        // control-witness leak (K6) is A5.
        Stmt::Return(r) => {
            let val = r
                .value()
                .map(|e| interp_expr(ctx, sink, state, env, control, &e));
            // Interprocedural flow-back (B2): merge this return's value witnesses
            // into the walk-local accumulator so a callee's `interp_body_returning`
            // captures returns nested inside control flow, not just a tail
            // `return`. Unfiltered (unlike the K7 sink below): flowing a witness
            // OUT of a callee to the CALLER is a data flow, not a `return`-sink
            // disclosure — the caller decides if the returned value reaches a
            // sink. Harmless on a root walk (nobody reads the accumulator). Fires
            // regardless of `disclosing_fn` (a non-root callee has it `None`).
            if let Some(val) = &val {
                state.ret_ws = merge_witnesses(&state.ret_ws, &witnesses_of(val));
            }
            if let (Some(fname), Some(val)) = (ctx.disclosing_fn, &val) {
                let stmt_range = stmt.syntax().text_range();
                // v3c C3 (minimal-scope `disclose()` quick-fix): the DATA
                // leak's primary span is the returned EXPRESSION's own range
                // — not the whole `return expr;` statement (`return` keyword
                // and `;` included), which is not a valid `disclose()` target
                // and would over-wrap a second leak in the same statement.
                // Differential-verified against `compactc` 0.31.1:
                // `return disclose(getW());` compiles clean. Falls back to
                // `stmt_range` only if `r.value()` is somehow `None` here,
                // which cannot happen (this arm requires `val: Some`, and
                // `val` is derived from `r.value()`), kept only to avoid an
                // unreachable `unwrap`.
                let value_range = r
                    .value()
                    .map(|e| trimmed_range(e.syntax()))
                    .unwrap_or(stmt_range);
                let filtered = filter_witnesses(&witnesses_of(val));
                sink.record_leak(
                    value_range,
                    format!("the value returned from exported circuit {fname}"),
                    filtered,
                );
                // A5 (K6): a return gated on a witness discloses the gating
                // witness as a distinct control-flow leak. The SAME K7 asymmetry
                // applies (`filter_witnesses`) — being gated on the circuit's own
                // argument is not a disclosure, only a `WitnessReturn` control
                // witness leaks. `record_leak` no-ops on an empty (filtered) set.
                // Kept at the statement range (no single wrappable value
                // expression fixes a control-flow disclosure): never offered
                // the `disclose()` quick-fix.
                sink.record_leak(
                    stmt_range,
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
                    let cond = interp_expr(ctx, sink, state, env, control, &e);
                    branch_control =
                        thread_control(witnesses_of(&cond), control, stmt.syntax().text_range());
                } else if let Some(blk) = compactp_ast::Block::cast(child.clone()) {
                    let mut scope = env.clone();
                    interp_stmt(
                        ctx,
                        sink,
                        state,
                        &mut scope,
                        &branch_control,
                        &compactp_ast::Stmt::Block(blk),
                    );
                } else if let Some(nested) = compactp_ast::Stmt::cast(child.clone()) {
                    let mut scope = env.clone();
                    interp_stmt(ctx, sink, state, &mut scope, &branch_control, &nested);
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
                let a = interp_expr(ctx, sink, state, env, control, &e);
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
                    state,
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
                interp_expr(ctx, sink, state, env, control, &e);
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
    state: &mut WalkState,
    env: &Env,
    control: &[Witness],
    expr: &Expr,
    reason: &str,
) -> Abs {
    let mut ws = Vec::new();
    for child in child_exprs(expr.syntax()) {
        let a = interp_expr(ctx, sink, state, env, control, &child);
        ws = merge_witnesses(&ws, &witnesses_of(&a));
    }
    sink.emit_advisory(expr.syntax().text_range(), reason.to_string());
    Abs::Atomic(ws)
}

/// Classifies a call's callee (witness / circuit / native). A method call
/// (`recv.method(..)`) fails closed to `Native` (the ledger-op path). A name
/// that resolves to a same-file symbol classifies by kind. A name that does
/// NOT resolve (`unresolved_callee_class`) is a KNOWN native (in the N2 conduit
/// table) or — far more likely on a module program — a circuit reached through
/// an import chain v3a cannot follow; the latter is deferred (amber), NOT
/// conduit-tainted. A cross-file witness's return type is read only same-file.
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
        return unresolved_callee_class(name_tok.text());
    };
    let Some(sym) = item_symbol(ctx.db, ctx.file, ctx.src, ctx.fd, df, index) else {
        return unresolved_callee_class(name_tok.text());
    };
    match sym.kind {
        SymbolKind::Witness => CalleeClass::Witness {
            name: sym.name.clone(),
            ret: witness_return_ty(ctx, df, &sym),
        },
        // A user circuit with a body: interpretable (B2). Carries the def's
        // `(file, symbol)`; `interp_circuit_call` fails closed if it's cross-file
        // (B4) or its body can't be read.
        SymbolKind::Circuit => CalleeClass::Circuit {
            file: df,
            symbol: index,
        },
        // A bodyless circuit signature or a cross-contract circuit: nothing to
        // walk here ⇒ deferred fail-closed (amber), NOT native conduit-taint.
        SymbolKind::CircuitSig | SymbolKind::ContractCircuit => CalleeClass::DeferredCircuit,
        _ => CalleeClass::Native,
    }
}

/// Classifies an UNRESOLVED plain-call name (spec §3.6, fail-closed §0):
/// resolution could not bind it to a same-file symbol. Natives never resolve to
/// a user symbol either, so a name IN the N2 conduit table
/// (`tables::native_conduit`) is still a native — keep modeling its conduit
/// taint (`persistentHash` → "a hash of"). Any OTHER unresolved name (a
/// module-imported / cross-module circuit v3a cannot follow, an ambiguous
/// re-export) is treated as a deferred CIRCUIT call — an amber v3b
/// advisory-clean result, NOT the unknown-native conduit-taint. A red
/// conduit-taint on such a name is a false positive on compiler-accepted module
/// programs (corpus FP #2, `top_func` → `mf(x)`); an unresolved user/module
/// name is far more likely a circuit than an unrecognized native, and amber is
/// strictly safer than a false-positive red (§0). The unknown-native
/// conduit-taint default survives only for a name that resolves to a non-
/// circuit/witness symbol (the `_ => Native` arm above) — the genuine
/// version-drift case.
fn unresolved_callee_class(name: &str) -> CalleeClass {
    if tables::native_conduit(name).is_some() {
        CalleeClass::Native
    } else {
        CalleeClass::DeferredCircuit
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
    state: &mut WalkState,
    env: &Env,
    control: &[Witness],
    call: &CallExpr,
) {
    for arg in arg_exprs(call) {
        interp_expr(ctx, sink, state, env, control, &arg);
    }
}

/// The direct `Expr` children of a syntax node, in source order.
fn child_exprs(node: &SyntaxNode) -> Vec<Expr> {
    node.children().filter_map(Expr::cast).collect()
}

/// `node`'s range with leading/trailing trivia (whitespace, comments)
/// stripped. This `rowan` 0.16 tree attaches a token's leading whitespace as
/// the FIRST child of the token/node that follows it (see the parser's
/// snapshot tests: `WHITESPACE@21..22` nested inside the following
/// `NAME_EXPR`, not a preceding sibling) — so a bare `node.text_range()` on
/// an expression can include a stray leading space from the operator before
/// it. That extra byte is harmless for most callers (diagnostics render a
/// snippet either way), but it matters here (v3c C3): the disclosure
/// interpreter's DATA-sink leaks use this to record the MINIMAL tainted
/// expression, which a `disclose()` quick-fix later wraps verbatim — an
/// untrimmed span would insert `disclose(` one byte too early (`c
/// =disclose( getW())`), which is cosmetically wrong even though it isn't a
/// security issue (extra whitespace, not extra scope).
fn trimmed_range(node: &SyntaxNode) -> TextRange {
    let mut tokens = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia());
    let Some(first) = tokens.next() else {
        return node.text_range();
    };
    let last = tokens.last().unwrap_or_else(|| first.clone());
    TextRange::new(first.text_range().start(), last.text_range().end())
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

    use text_size::TextSize;

    use super::super::leaks::DisclosureLeak;
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
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };

        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        let ctor_root = roots
            .iter()
            .find(|r| matches!(r, Root::Constructor { .. }))
            .expect("constructor root");
        let circuit_root = roots
            .iter()
            .find(|r| matches!(r, Root::Circuit { name, .. } if name == "f"))
            .expect("circuit root f");

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
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        let ws = empty_ws(&db);
        let file = crate::FileId::from_raw_for_test(0);
        let tree = item_tree(&db, src);
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);

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

    #[test]
    fn module_nested_circuit_not_a_root() {
        // Fix (a): a module-nested `export circuit` that writes its arg to a
        // ledger is NOT a disclosing root — compactc reaches it only through its
        // import/export chain, so it accepts the file. v3a excludes it and
        // records an amber advisory (§0), emitting NO E-leak. WITHOUT the
        // exclusion `leak` would be seeded as a root and `c = x` would flag a
        // false-positive `E3100` (corpus FP #2, the test3a/base_func pattern).
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             module M {\n\
             export ledger c: Field;\n\
             export circuit leak(x: Field): Field { c = x; return x; }\n\
             }\n",
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
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        assert!(
            !roots
                .iter()
                .any(|r| matches!(r, Root::Circuit { name, .. } if name == "leak")),
            "module-nested circuit must be excluded from roots"
        );
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        assert!(
            sink.drain_leaks().is_empty(),
            "module-nested circuit must not emit a confirmed leak"
        );
        assert!(
            sink.advisories()
                .iter()
                .any(|a| a.reason.contains("module-nested circuit `leak`")),
            "excluded module circuit must record an amber advisory (never silent green)"
        );
    }

    #[test]
    fn module_reexported_circuit_is_a_root_and_leaks() {
        // B4 surface 1: a module `export circuit` that writes its own parameter
        // to a ledger becomes a disclosing ROOT when a top-level `export { s }`
        // re-exports it (`s` -> `store`). compactc REJECTs this exact shape
        // (task-B4-findings.md probe b / module_circuit_leak.compact). The
        // re-exported module circuit must therefore be a root, seeded with its
        // own param, and its arg->ledger write must confirm an E-leak — NOT the
        // fail-closed amber that an un-re-exported module circuit gets.
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             module Inner {\n\
             export ledger stored: Field;\n\
             export circuit store(x: Field): [] { stored = x; }\n\
             }\n\
             import { store as s } from Inner;\n\
             export { s };\n",
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
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        assert!(
            roots
                .iter()
                .any(|r| matches!(r, Root::Circuit { name, .. } if name == "store")),
            "a re-exported module circuit must be a disclosing root"
        );
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        assert!(
            !sink.drain_leaks().is_empty(),
            "the re-exported module circuit's arg->ledger write must confirm a leak"
        );
        // No fail-closed "not re-exported" advisory for `store` (it IS analyzed).
        assert!(
            !sink
                .advisories()
                .iter()
                .any(|a| a.reason.contains("`store`") && a.reason.contains("not re-exported")),
            "an analyzed re-exported root must not also record the excluded-module advisory"
        );
    }

    #[test]
    fn module_reexported_disclosing_circuit_is_a_root_but_no_leak() {
        // §0 / regression guard for corpus FP #2: a re-exported module circuit
        // is rooted, but if it disclose()s before the ledger write it must NOT
        // leak (matches module_reexport_ok.compact, which compactc ACCEPTs).
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             module Inner {\n\
             export ledger stored: Field;\n\
             export circuit store(x: Field): [] { stored = disclose(x); }\n\
             }\n\
             import { store as s } from Inner;\n\
             export { s };\n",
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
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        assert!(
            roots
                .iter()
                .any(|r| matches!(r, Root::Circuit { name, .. } if name == "store")),
            "a re-exported module circuit is still rooted even when it discloses"
        );
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        assert!(
            sink.drain_leaks().is_empty(),
            "a re-exported circuit that discloses before the write must not leak"
        );
    }

    #[test]
    fn module_imported_call_leaks_interprocedurally() {
        // B4 surface 2 (within-file module resolution, already served by B2/B3):
        // a top-level exported circuit `top` passes its witness param to the
        // module-imported `s` (== `Inner.store`), which writes it to a ledger.
        // compactc REJECTs (module_call_leak.compact). The module-imported call
        // must resolve to its body and confirm the interprocedural leak.
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             module Inner {\n\
             export ledger stored: Field;\n\
             export circuit store(x: Field): [] { stored = x; }\n\
             }\n\
             import { store as s } from Inner;\n\
             export circuit top(secret: Field): [] { s(secret); }\n",
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
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        assert!(
            !sink.drain_leaks().is_empty(),
            "a call to a module-imported circuit that leaks must confirm interprocedurally"
        );
    }

    #[test]
    fn unresolved_callee_is_amber_not_conduit() {
        // Fix (b): a call to an UNRESOLVED name (`mf` — not a known native, not
        // a witness, resolves to no same-file symbol) is a deferred circuit call
        // (amber v3b advisory-clean), NOT the unknown-native conduit-taint. A
        // witness value flowed through it must therefore NOT reach the ledger
        // sink as a confirmed leak — that red is the false positive on the
        // module-import programs compactc accepts (the `top_func` → `mf(x)`
        // pattern, corpus FP #2).
        let (db, src, fd, ws, file) = walk_setup(
            "import CompactStandardLibrary;\n\
             export ledger c: Field;\n\
             witness getW(): Field;\n\
             export circuit f(): [] { c = mf(getW()); }\n",
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
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        assert!(
            sink.drain_leaks().is_empty(),
            "an unresolved callee must not conduit-taint a witness into a confirmed leak"
        );
        assert!(
            !sink.advisories().is_empty(),
            "the deferred unresolved call must record an amber advisory (never silent green)"
        );
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
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
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
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
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
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
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
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
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
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
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
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
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
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
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
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
        assert!(
            !sink.advisories().is_empty(),
            "unhandled form emits advisory"
        );
        assert_eq!(witnesses_of(&out).len(), 1, "unhandled form keeps taint");
    }

    #[test]
    fn unresolved_call_is_amber_clean_not_conduit() {
        // Fix (b) (supersedes A6's unresolved-callee stopgap): a plain call to
        // an UNRESOLVED name (`blackbox` — not a known native, not a resolvable
        // witness/circuit) is a deferred CIRCUIT call ⇒ an amber advisory (§0)
        // and a CLEAN result — NOT the unknown-native conduit-taint. A red
        // conduit-taint here is the corpus FP #2 (`top_func` → `mf(x)`). A KNOWN
        // native (in the N2 table) still conduits; that path is unchanged.
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
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
        assert!(
            !sink.advisories().is_empty(),
            "an unresolved call emits a fail-closed advisory (never silent green)"
        );
        assert!(
            witnesses_of(&out).is_empty(),
            "an unresolved call must NOT conduit-taint its arg into the result"
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
            &mut WalkState::default(),
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
            &mut WalkState::default(),
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
            &mut WalkState::default(),
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
        let text = "witness getW(): Uint<8>;\n\
             export circuit leaky(): Uint<8> { return getW(); }\n\
             export circuit ok(x: Uint<8>): Uint<8> { return x; }\n";
        let (db, src, fd, ws, file) = walk_setup(text);
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
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
        // v3c C3 (minimal-scope `disclose()` quick-fix, differential-verified
        // against `compactc` 0.31.1): the leak's primary span must be the
        // MINIMAL returned expression `getW()` — not the whole `return
        // getW();` statement (which includes the `return` keyword and `;`
        // and is not a valid `disclose()` target).
        let call_start = text.find("return getW()").unwrap() + "return ".len();
        let expected = TextRange::new(
            TextSize::new(call_start as u32),
            TextSize::new((call_start + "getW()".len()) as u32),
        );
        assert_eq!(
            leaks[0].src, expected,
            "K7 leak's primary span must be the minimal return expression, not the whole return statement"
        );
    }

    #[test]
    fn ledger_cell_write_records_leak() {
        // K1 ledger sink: `c = getW()` writes a witness value to a ledger Cell.
        let text = "import CompactStandardLibrary;\n\
             export ledger c: Uint<8>;\n\
             witness getW(): Uint<8>;\n\
             export circuit f(): [] { c = getW(); }\n";
        let (db, src, fd, ws, file) = walk_setup(text);
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        let leaks = sink.drain_leaks();
        assert_eq!(leaks.len(), 1, "the ledger write leaks the witness");
        assert_eq!(leaks[0].nature, "ledger operation");
        assert_eq!(leaks[0].witnesses.len(), 1);
        // v3c C3 (minimal-scope `disclose()` quick-fix, differential-verified
        // against `compactc` 0.31.1): the leak's primary span must be the
        // MINIMAL RHS expression `getW()` — not the whole `c = getW();`
        // statement. Confirmed empirically: `c = disclose(getW());` compiles
        // clean under 0.31.1; `disclose(c = getW());` (a broader wrap) is
        // REJECTED (the compiler re-locates the same undisclosed value at
        // the inner RHS) — so a broader wrap is not just imprecise, it is
        // actively rejected by the compiler.
        // (NOT `text.find("getW()")`: that would match the earlier `witness
        // getW(): Uint<8>;` declaration, which also contains `getW()` as a
        // substring — anchor on the assignment itself instead.)
        let call_start = text.find("c = getW()").unwrap() + "c = ".len();
        let expected = TextRange::new(
            TextSize::new(call_start as u32),
            TextSize::new((call_start + "getW()".len()) as u32),
        );
        assert_eq!(
            leaks[0].src, expected,
            "K1 leak's primary span must be the minimal RHS expression, not the whole assignment statement"
        );
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
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
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
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
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
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        assert!(
            sink.drain_leaks().is_empty(),
            "a disclosed condition clears control ⇒ no control leak"
        );
    }

    #[test]
    fn ledger_method_call_control_leaks_no_data_and_no_advisory() {
        // A6: a ledger-field METHOD call (`c.increment(1)`, c: Counter) gated on
        // a witness records the K2 control leak. The `increment` op IS now
        // modeled (L2), so its clean constant arg `1` records NO data leak and
        // the op emits NO fail-closed advisory (it replaced A5's stopgap).
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
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        let had_advisory = !sink.advisories().is_empty();
        let leaks = sink.drain_leaks();
        assert_eq!(
            leaks.len(),
            1,
            "only the control leak (clean constant arg ⇒ no data leak)"
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
            !had_advisory,
            "a modeled (known) ledger op emits no fail-closed advisory"
        );
    }

    // -- A6 native conduit + ledger-op flag tables ----------------------

    /// Drives every root and returns the drained leak table plus whether any
    /// advisory was recorded — the shared shape of the A6 call-handling tests.
    fn walk_leaks(text: &str) -> (Vec<DisclosureLeak>, bool) {
        let (db, src, fd, ws, file) = walk_setup(text);
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let mut sink = DisclosureSink::new();
        let roots = discover_roots(&base, &tree, &mut sink);
        for root in &roots {
            interp_root(&base, &mut sink, root);
        }
        let had_advisory = !sink.advisories().is_empty();
        (sink.drain_leaks(), had_advisory)
    }

    #[test]
    fn native_hash_is_conduit_then_ledger_write_leaks() {
        // N2 conduit + K1 sink (the `hash_then_leak` flip): `persistentHash`
        // records NO leak itself, but folds getW into its Bytes<32> result with
        // exposure "a hash of" — so the enclosing ledger Cell-write leaks it.
        let (leaks, had_advisory) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger c: Bytes<32>;\n\
             witness getW(): Bytes<32>;\n\
             export circuit f(): [] { c = persistentHash<Bytes<32>>(getW()); }\n",
        );
        assert_eq!(leaks.len(), 1, "the ledger write leaks the hashed witness");
        assert_eq!(leaks[0].nature, "ledger operation");
        assert_eq!(leaks[0].witnesses.len(), 1);
        assert_eq!(
            leaks[0].witnesses[0].info,
            WitnessInfo::WitnessReturn {
                function: "getW".into()
            }
        );
        // The conduit exposure rides on the witness's path trail.
        assert!(
            leaks[0].witnesses[0]
                .path
                .iter()
                .any(|pp| pp.exposure == "a hash of"),
            "the conduit's 'a hash of' exposure is on the disclosure path"
        );
        // A known native + a modeled ledger write ⇒ no fail-closed advisory.
        assert!(!had_advisory, "known native conduit emits no advisory");
    }

    #[test]
    fn native_commit_hides_witness_no_leak() {
        // N2: `persistentCommit` flags BOTH args `nothing` — the native
        // sanitizer. Committing witnesses then writing the result leaks nothing.
        let (leaks, had_advisory) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger c: Bytes<32>;\n\
             witness getW(): Bytes<32>;\n\
             witness getR(): Bytes<32>;\n\
             export circuit f(): [] { c = persistentCommit<Bytes<32>>(getW(), getR()); }\n",
        );
        assert!(leaks.is_empty(), "commit hides both args ⇒ no leak");
        assert!(!had_advisory, "known native sanitizer emits no advisory");
    }

    #[test]
    fn container_op_witness_arg_leaks() {
        // L2: `Set.insert` carries the default `discloses? = ""` (truthy) ⇒ its
        // witness argument is an immediate "ledger operation" leak.
        let (leaks, had_advisory) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger s: Set<Uint<8>>;\n\
             witness getW(): Uint<8>;\n\
             export circuit f(): [] { s.insert(getW()); }\n",
        );
        assert_eq!(leaks.len(), 1, "the container op leaks its witness arg");
        assert_eq!(leaks[0].nature, "ledger operation");
        assert_eq!(leaks[0].witnesses.len(), 1);
        assert_eq!(
            leaks[0].witnesses[0].info,
            WitnessInfo::WitnessReturn {
                function: "getW".into()
            }
        );
        assert!(!had_advisory, "known ledger op emits no advisory");
    }

    // -- A7 fold/map fixpoint over aggregates ---------------------------

    /// Interprets the sole map/fold expr of a one-line circuit body against a
    /// hand-built `env` (the sequence bindings), returning its `Abs`.
    fn interp_agg_expr(
        text: &str,
        kind: compactp_syntax::SyntaxKind,
        seeds: &[(&str, Abs)],
    ) -> (Abs, Vec<String>) {
        let (db, src, fd, ws, file) = walk_setup(text);
        let ctx = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let root = root_of(&db, src);
        let expr = first_expr(&root, kind);
        let mut env = Env::new();
        for (name, abs) in seeds {
            env.insert((*name).to_string(), abs.clone());
        }
        let mut sink = DisclosureSink::new();
        let out = interp_expr(&ctx, &mut sink, &mut WalkState::default(), &env, &[], &expr);
        let advisories: Vec<String> = sink.advisories().iter().map(|a| a.reason.clone()).collect();
        (out, advisories)
    }

    #[test]
    fn map_with_disclosing_lambda_is_clean() {
        // FX2 + D1: the lambda `disclose(x + y)` sanitizes INSIDE the map, so
        // the result carries NO witnesses — even though both sequences are
        // tainted. This FAILS against A3's catch-all (which unions the
        // sequences' witnesses without ever looking in the lambda). This is the
        // root-caused corpus FP (`slice_part_one` `test19`).
        let (out, _adv) = interp_agg_expr(
            "export circuit f(): [] { const r = map((x, y) => disclose(x + y), a, b); }\n",
            compactp_syntax::SyntaxKind::MAP_EXPR,
            &[
                ("a", Abs::Single(Box::new(Abs::Atomic(vec![witness(1)])))),
                ("b", Abs::Single(Box::new(Abs::Atomic(vec![witness(2)])))),
            ],
        );
        assert!(
            witnesses_of(&out).is_empty(),
            "a disclosing map lambda clears taint (got {:?})",
            witnesses_of(&out)
        );
    }

    #[test]
    fn map_without_disclose_propagates_taint() {
        // FX2: no disclose ⇒ the map is a conduit — the tainted sequence's
        // witness rides through into the `Single` element result.
        let (out, _adv) = interp_agg_expr(
            "export circuit f(): [] { const r = map((x, y) => x + y, a, b); }\n",
            compactp_syntax::SyntaxKind::MAP_EXPR,
            &[
                ("a", Abs::Single(Box::new(Abs::Atomic(vec![witness(1)])))),
                ("b", Abs::Single(Box::new(Abs::Atomic(vec![])))),
            ],
        );
        assert!(
            matches!(out, Abs::Single(_)),
            "map over aggregated arrays yields a Single"
        );
        assert_eq!(
            witnesses_of(&out).len(),
            1,
            "no disclose ⇒ the map propagates the sequence's taint"
        );
    }

    #[test]
    fn fold_over_witness_array_reaches_fixpoint() {
        // FX1/FX3: `fold((a, b) => a + b, 0, s)` accumulating a tainted array
        // stabilizes at an `abs_equal` fixed point (the test COMPLETING proves
        // termination) and the accumulator carries the array's taint.
        let (out, _adv) = interp_agg_expr(
            "export circuit f(): [] { const r = fold((a, b) => a + b, 0, s); }\n",
            compactp_syntax::SyntaxKind::FOLD_EXPR,
            &[("s", Abs::Single(Box::new(Abs::Atomic(vec![witness(1)]))))],
        );
        assert_eq!(
            witnesses_of(&out).len(),
            1,
            "the accumulator carries the folded array's taint"
        );
    }

    #[test]
    fn fold_with_disclosing_lambda_is_clean() {
        // FX1 + D1: a disclosing fold body clears the accumulator every step ⇒
        // the fixed point is clean.
        let (out, _adv) = interp_agg_expr(
            "export circuit f(): [] { const r = fold((a, b) => disclose(a + b), 0, s); }\n",
            compactp_syntax::SyntaxKind::FOLD_EXPR,
            &[("s", Abs::Single(Box::new(Abs::Atomic(vec![witness(1)]))))],
        );
        assert!(
            witnesses_of(&out).is_empty(),
            "a disclosing fold lambda clears taint (got {:?})",
            witnesses_of(&out)
        );
    }

    #[test]
    fn fold_over_heterogeneous_tuple_unrolls() {
        // FX1 unroll regime: a heterogeneous `Multiple` (tuple) sequence is
        // fully unrolled element-by-element (no fixpoint). `(a, b) => a + b`
        // over a 2-tuple whose second element is tainted accumulates that taint.
        let (out, _adv) = interp_agg_expr(
            "export circuit f(): [] { const r = fold((a, b) => a + b, 0, t); }\n",
            compactp_syntax::SyntaxKind::FOLD_EXPR,
            &[(
                "t",
                Abs::Multiple(vec![Abs::Atomic(vec![]), Abs::Atomic(vec![witness(7)])]),
            )],
        );
        assert_eq!(
            witnesses_of(&out).len(),
            1,
            "unrolled fold accumulates the tuple's tainted element"
        );
    }

    #[test]
    fn unresolved_call_no_leak_but_advises() {
        // Fix (b): a witness value flowed through an UNRESOLVED callee
        // (`mysteryNative` — not a known native, not a resolvable symbol) does
        // NOT reach the ledger sink as a confirmed leak, because the call is a
        // deferred circuit (amber-clean), not the unknown-native conduit-taint.
        // The deferral is still recorded as an amber advisory (§0), never silent
        // green. (This was the corpus FP #2 shape: `top_func` writing `mf(x)`.)
        let (leaks, had_advisory) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger c: Bytes<32>;\n\
             witness getW(): Bytes<32>;\n\
             export circuit f(): [] { c = mysteryNative(getW()); }\n",
        );
        assert!(
            leaks.is_empty(),
            "an unresolved callee must not conduit-taint a witness into a leak"
        );
        assert!(
            had_advisory,
            "the deferred unresolved call records a fail-closed advisory (§0)"
        );
    }

    // -- B2: interprocedural cross-circuit call interpretation --------------

    #[test]
    fn cross_circuit_call_propagates_taint_to_callee_ledger_sink() {
        // leak(secret) -> stash(secret) -> `store = v` is a confirmed E3100 leak.
        // Validated against compactc 0.31.1 (fixture interproc_call_leak.compact
        // REJECTs: "potential witness-value disclosure must be declared but is
        // not", path `secret` -> "the argument to stash" -> RHS of `=`).
        let (leaks, _adv) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger store: Field;\n\
             circuit stash(v: Field): [] { store = v; }\n\
             export circuit leak(secret: Field): [] { stash(secret); }\n",
        );
        assert!(
            !leaks.is_empty(),
            "cross-circuit witness->ledger must confirm a leak"
        );
        assert_eq!(leaks[0].nature, "ledger operation");
        // The leaked witness is leak's own `secret` param, threaded THROUGH
        // stash's `v` param to the callee's ledger write — the interprocedural
        // flow the intraprocedural walk could not see.
        assert!(
            leaks.iter().flat_map(|l| &l.witnesses).any(|w| matches!(
                &w.info,
                WitnessInfo::CircuitArg { function, arg }
                    if function == "leak" && arg == "secret"
            )),
            "the leaked witness must be leak's `secret` param, flowed through stash: {leaks:?}"
        );
    }

    #[test]
    fn cross_circuit_callee_disclose_sanitizes() {
        // The callee `disclose()`s before the write — interproc_call_ok.compact
        // compiles CLEAN under compactc 0.31.1, so the analyzer must confirm no
        // leak. This is the fail-closed corpus-safety direction: interprocedural
        // interpretation must not manufacture a false positive when the callee
        // sanitizes.
        let (leaks, _adv) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger store: Field;\n\
             circuit stash(v: Field): [] { store = disclose(v); }\n\
             export circuit leak(secret: Field): [] { stash(secret); }\n",
        );
        assert!(
            leaks.is_empty(),
            "callee disclose() must sanitize -- no leak (got {leaks:?})"
        );
    }

    #[test]
    fn cross_circuit_return_value_flows_back_to_caller_sink() {
        // The callee RETURNS a witness-return value; the caller writes that
        // returned value to the ledger. The leak only manifests because the
        // callee's returned `Abs` flows back to the caller (B0 Q1). Validated
        // against compactc 0.31.1 (rejects with the disclosure banner).
        let (leaks, _adv) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger store: Field;\n\
             witness getW(): Field;\n\
             circuit fetch(): Field { return getW(); }\n\
             export circuit f(): [] { store = fetch(); }\n",
        );
        assert!(
            !leaks.is_empty(),
            "a witness value RETURNED from a callee and sunk by the caller must leak"
        );
        assert!(
            leaks.iter().flat_map(|l| &l.witnesses).any(|w| matches!(
                &w.info,
                WitnessInfo::WitnessReturn { function } if function == "getW"
            )),
            "the flowed-back leak must carry getW's witness-return: {leaks:?}"
        );
    }

    #[test]
    fn cross_circuit_callee_return_is_not_a_disclosure_sink() {
        // K7 filter (B0 Q3): a NON-root callee runs with `disclosing_fn = None`,
        // so `return v` (v = the callee's own witness-derived arg) is NOT itself
        // a disclosure — only the caller's use of the returned value can leak. If
        // the caller does nothing observable with it, there is no leak. This
        // guards against wrongly arming the return sink inside a callee.
        let (leaks, _adv) = walk_leaks(
            "import CompactStandardLibrary;\n\
             witness getW(): Field;\n\
             circuit fetch(): Field { return getW(); }\n\
             export circuit f(): Field { const x = fetch(); return disclose(x); }\n",
        );
        assert!(
            leaks.is_empty(),
            "a callee `return` is not a sink; the caller disclosed the value (got {leaks:?})"
        );
    }

    #[test]
    fn recursive_circuit_call_fails_closed_and_terminates() {
        // §0 fail-closed + termination (B0 Q4): recursion is rejected upstream of
        // the WPP, so a hand-written cyclic program (which compactc would reject)
        // must NOT hang the analyzer. The `active`-set guard breaks the cycle,
        // records an amber advisory, and returns clean — never an infinite loop,
        // never a stack overflow. (This source does not compile; the point is the
        // analyzer's robustness, not a differential match.)
        let (leaks, had_advisory) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger store: Field;\n\
             circuit ping(v: Field): Field { return pong(v); }\n\
             circuit pong(v: Field): Field { return ping(v); }\n\
             export circuit f(secret: Field): [] { store = disclose(ping(secret)); }\n",
        );
        // Termination is the assertion that matters (the test returning at all).
        // The recursion is broken fail-closed with an amber advisory; the caller
        // `disclose()`s, so no confirmed leak is manufactured.
        assert!(
            had_advisory,
            "the broken recursion cycle must record a fail-closed amber advisory (§0)"
        );
        assert!(
            leaks.is_empty(),
            "the disclosed result must not leak; recursion guard stays fail-closed (got {leaks:?})"
        );
    }

    #[test]
    fn arg_side_leak_survives_recursion_guard_fail_closed() {
        // §0 regression (B2 review): the recursion-guard early-return must STILL
        // interpret the call's ARGUMENTS for effect, so a leak nested inside an
        // argument is not silently dropped when THIS call is un-walkable. Here
        // self-recursive `recurse` trips the guard on its inner call, but that
        // inner call's argument `leaky(getW())` runs a same-file helper that
        // writes a witness to the ledger — that inner E3100 must still be
        // confirmed, not dropped. Deterministic: the guard trips exactly once
        // (on the second `recurse`), and `leaky` is walked via the arg. Before
        // the fix, the guard bailed before interpreting the argument, silently
        // losing this leak (a §0 silent-green).
        let (leaks, had_advisory) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger store: Field;\n\
             witness getW(): Field;\n\
             circuit leaky(x: Field): Field { store = x; return x; }\n\
             circuit recurse(v: Field): [] { recurse(leaky(getW())); }\n\
             export circuit f(): [] { recurse(0); }\n",
        );
        assert!(
            had_advisory,
            "the recursion guard must record a fail-closed amber advisory (§0)"
        );
        assert!(
            leaks.iter().flat_map(|l| &l.witnesses).any(|w| matches!(
                &w.info,
                WitnessInfo::WitnessReturn { function } if function == "getW"
            )),
            "the leak nested in the recursive call's argument must NOT be dropped: {leaks:?}"
        );
    }

    // -- B3: cross-circuit argument path points (B0 Q5) -----------------

    #[test]
    fn cross_circuit_arg_attaches_path_point() {
        // B0 Q5: passing a value as an argument to a helper attaches the SAME
        // path point natives use — `add-path-point` + "the [Nth] argument to
        // <callee>", exposure "". Ground truth: the local compactc 0.31.1
        // REJECTs `interproc_path.compact` with the WPP banner and renders the
        // path "the argument to stash at line 10 char 31" — the single-arg,
        // no-ordinal form this asserts.
        let (leaks, _adv) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger store: Field;\n\
             witness getSecret(): Field;\n\
             circuit stash(v: Field): [] { store = v; }\n\
             export circuit reveal(): [] { stash(getSecret()); }\n",
        );
        assert!(
            !leaks.is_empty(),
            "witness -> helper -> ledger must confirm a leak"
        );
        // The single-arg call renders "the argument to stash" (NO ordinal),
        // exposure "" — the cross-circuit arg path point rides the witness trail.
        assert!(
            leaks.iter().flat_map(|l| &l.witnesses).any(|w| w
                .path
                .iter()
                .any(|pp| pp.description == "the argument to stash" && pp.exposure.is_empty())),
            "the leaked witness must carry the single-arg cross-circuit path point: {leaks:?}"
        );
    }

    #[test]
    fn cross_circuit_multi_arg_path_point_uses_ordinal() {
        // B0 Q5: a MULTI-arg call carries 1-based ordinals ("the second argument
        // to <callee>"), via the same `arg_description` the native conduit uses.
        let (leaks, _adv) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger store: Field;\n\
             witness getSecret(): Field;\n\
             circuit sink2(a: Field, b: Field): [] { store = b; }\n\
             export circuit reveal(): [] { sink2(0, getSecret()); }\n",
        );
        assert!(
            leaks.iter().flat_map(|l| &l.witnesses).any(|w| w
                .path
                .iter()
                .any(|pp| pp.description == "the second argument to sink2")),
            "the second-arg witness must carry a 1-based-ordinal path point: {leaks:?}"
        );
    }

    // -- B3: walk-local call-ht memo (B0 Q2, memoization ADR Option A) ---

    #[test]
    fn memo_helper_called_twice_yields_one_leak_row() {
        // §0 fail-closed invariant / ADR case (c): the memo must NOT change the
        // leak verdict. A helper called twice with IDENTICAL (arg_shapes,
        // control) leaks at one ledger-write (src, nature); whether the second
        // call HITs the memo or re-walks, `(src,nature)` dedup makes the two
        // observationally identical — exactly ONE leak row either way. This is
        // an OBSERVABLE assertion on the leak table, not private-counter peeking.
        let (leaks, _adv) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger store: Field;\n\
             circuit stash(v: Field): [] { store = v; }\n\
             export circuit leak(secret: Field): [] { stash(secret); stash(secret); }\n",
        );
        assert_eq!(
            leaks.len(),
            1,
            "a helper called twice identically must dedup to ONE leak row: {leaks:?}"
        );
        assert_eq!(leaks[0].nature, "ledger operation");
        // And the leaked witness is still `leak`'s `secret` (flowed through the
        // helper both times) — the verdict is unchanged by memoization.
        assert!(
            leaks.iter().flat_map(|l| &l.witnesses).any(|w| matches!(
                &w.info,
                WitnessInfo::CircuitArg { function, arg }
                    if function == "leak" && arg == "secret"
            )),
            "the deduped leak must still carry leak's `secret`: {leaks:?}"
        );
    }

    #[test]
    fn memo_collapses_exponential_fanout() {
        // OBSERVABLE reuse proof (resolves the B2-review exponential-re-walk
        // termination concern). A doubling call chain — each helper calls the
        // next TWICE with the SAME (arg_shapes, control) — is 2^depth walks
        // WITHOUT the memo (this test would never return) and O(depth) walks
        // WITH it (each key walked once, sibling calls HIT). Completion at
        // depth 40 (~1.1e12 un-memoized walks) is itself the assertion that the
        // memo reuses cached results rather than re-walking. The leak verdict is
        // still exactly one row (ADR case (c)), unchanged from a single walk.
        const DEPTH: usize = 40;
        let mut src = String::from(
            "import CompactStandardLibrary;\n\
             export ledger store: Field;\n\
             circuit h0(x: Field): [] { store = x; }\n",
        );
        for i in 1..=DEPTH {
            src.push_str(&format!(
                "circuit h{i}(x: Field): [] {{ h{prev}(x); h{prev}(x); }}\n",
                prev = i - 1
            ));
        }
        src.push_str(&format!(
            "export circuit top(secret: Field): [] {{ h{DEPTH}(secret); }}\n"
        ));

        let (leaks, _adv) = walk_leaks(&src);
        // Reaching here at all proves the memo collapsed the 2^DEPTH fan-out.
        assert_eq!(
            leaks.len(),
            1,
            "the doubling chain must still yield exactly one leak row: {leaks:?}"
        );
        assert_eq!(leaks[0].nature, "ledger operation");
        assert!(
            leaks.iter().flat_map(|l| &l.witnesses).any(|w| matches!(
                &w.info,
                WitnessInfo::CircuitArg { function, arg }
                    if function == "top" && arg == "secret"
            )),
            "the leak must carry top's `secret`, flowed through the whole chain: {leaks:?}"
        );
    }

    /// Ground truth (v3c C4): pins that `merge_witnesses` (R0 F2) genuinely
    /// produces a CONSECUTIVE DUPLICATE path point in the raw leak table when
    /// the exact same already-pathed witness is joined with itself -- here, a
    /// `const y = getW();` used unchanged in BOTH arms of a non-constant
    /// ternary (`1 == 2` interprets as `Abs::Atomic`, not `Abs::Boolean`, so
    /// `interp_ternary` takes the `combine_abs` join, not the constant-fold
    /// shortcut). Both arms are literal clones of the SAME env binding (same
    /// uid, same one-entry path `["the binding of y"]`); `merge_witnesses`
    /// concatenates `w1.path` with `w2.path` on the uid collision, so the
    /// result is the exact PathPoint appearing twice, back to back.
    ///
    /// This is a REAL leak-table artifact of the interpreter's merge
    /// semantics, not a rendering bug -- so the fix belongs in the DISPLAY
    /// layer (`leak_to_diagnostic`'s consecutive-duplicate collapse), never
    /// here: collapsing at the interp level would touch `record_leak`/the
    /// leak table, which spec §0 forbids (display-only).
    #[test]
    fn ternary_same_witness_both_branches_yields_duplicate_path_point() {
        let (leaks, _adv) = walk_leaks(
            "import CompactStandardLibrary;\n\
             export ledger c: Bytes<32>;\n\
             witness getW(): Bytes<32>;\n\
             export circuit f(): [] {\n\
               const y = getW();\n\
               c = 1 == 2 ? y : y;\n\
             }\n",
        );
        assert_eq!(leaks.len(), 1);
        let path = &leaks[0].witnesses[0].path;
        assert_eq!(
            path.len(),
            2,
            "the raw leak table carries the duplicate -- collapsing it is the renderer's job: {path:?}"
        );
        assert_eq!(
            path[0], path[1],
            "the two entries are byte-for-byte identical: {path:?}"
        );
        assert_eq!(path[0].description, "the binding of y");
    }
}
