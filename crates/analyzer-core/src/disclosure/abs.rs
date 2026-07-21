//! The `Abs` abstract-value lattice (spec §3.4), translated faithfully from
//! the verified compiler model (`analysis-passes.ss`, `compactc-v0.31.1`,
//! commit `0da5b045` — see R0 index rows AB1/AB2, D1, F2, FX3). Struct/tuple
//! fields are tracked individually (`Multiple`); arrays/vectors are tracked
//! in the aggregate (`Single`); `Boolean` exists only for compile-time
//! constants so `if` can constant-fold; everything else is `Atomic`.
//!
//! Pure lattice logic only — no salsa, no interpretation. A2/A3 build the
//! interpreter that produces and consumes these values.
//!
//! `Abs`/`Witness`/`WitnessInfo`/`PathPoint` get their first production
//! caller in `interp.rs` (A2: `abs_of_type` builds `Abs` shapes, `seed_sources`
//! constructs `Witness`es) — the file-level `#[expect(dead_code)]` A1 left
//! here as a scaffold marker is gone.
//!
//! A4 wired `disclosure_diagnostics_query` (`mod.rs`) to run the interpreter
//! from every root and drain its leak table, giving the lattice operations
//! (`disclose`/`combine_abs`/`merge_witnesses`/`witnesses_of`) and the
//! `Abs::Boolean`/`WitnessInfo::WitnessReturn` constructors a live non-test
//! caller — their scaffold dead-code markers are gone. A7 (`interp_fold`) is
//! the live non-test caller of `abs_equal` / `witness_sets_equal` (FX3
//! fixpoint-convergence).

/// A witness value's origin (spec §3.1). Deduped by `uid` in a set.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WitnessInfo {
    /// S1 (spec §3.1): a witness function's return value. Constructed at
    /// witness *call sites* during the interpreter walk (A3), now reached from
    /// the live `disclosure_diagnostics_query` (A4).
    WitnessReturn {
        function: String,
    },
    ConstructorArg {
        arg: String,
    },
    CircuitArg {
        function: String,
        arg: String,
    },
}

/// One tracked witness value: where it entered + the path it has taken.
///
/// `Hash` (with the already-derived `Eq`) makes `Witness` usable inside the
/// B3 walk-local `call-ht` memo key (`(FileId, symbol, Vec<Abs>, Vec<Witness>)`,
/// interp.rs). No field blocks it: `TextRange`/`u32` are `Hash`, `WitnessInfo`
/// derives it, `Vec<PathPoint>` is `Hash` once `PathPoint` is. The derived
/// `Eq`/`Hash` are structural and order-dependent — STRICTER than the compiler's
/// order-independent `abs-equal?`/`same-witnesses?` (B0 Q2), so a derived-equal
/// key implies an `abs-equal?`-equal key: the memo can only ever MISS where the
/// compiler would HIT, never the reverse. A surplus miss re-walks, which the
/// `(src,nature)` leak dedup makes idempotent (memoization ADR case (c)), so the
/// stricter key is leak-set-invariant (fail-closed, §0).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Witness {
    pub uid: u32,
    pub src: text_size::TextRange,
    pub info: WitnessInfo,
    pub path: Vec<PathPoint>, // spec §3.7
}

/// A point on the witness->sink trail (spec §3.7). `Hash` so `Witness` (which
/// carries `Vec<PathPoint>`) is hashable for the B3 memo key; all fields
/// (`TextRange`, `String`) are `Hash`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathPoint {
    pub src: text_size::TextRange,
    pub description: String, // e.g. "the argument to the ledger write"
    pub exposure: String,    // e.g. "a hash of"; "" if unmodified
}

/// The abstract value lattice (spec §3.4). Struct/tuple fields individual;
/// arrays aggregate; booleans only for compile-time constants. `Hash` (over the
/// derived, structural `Eq`) makes `Vec<Abs>` usable as an arg-shape axis of the
/// B3 memo key; every variant's payload (`Vec<Witness>`, `bool`, `Box<Abs>`) is
/// `Hash`. NOTE: this derived `Eq`/`Hash` is order-dependent and distinct from
/// the order-independent `abs_equal` used by the fold fixpoint — see `Witness`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Abs {
    Atomic(Vec<Witness>),
    /// AB2 (spec §3.4): only a compile-time `true`/`false` literal produces
    /// this. Constructed by the interpreter's literal-evaluation arm (A3),
    /// now reached from the live `disclosure_diagnostics_query` (A4).
    Boolean {
        value: bool,
        witnesses: Vec<Witness>,
    },
    Multiple(Vec<Abs>),
    Single(Box<Abs>),
}

/// `disclose(e)` (R0 D1): clears the witness set at EVERY level (`Atomic`,
/// `Boolean`, and recursively through `Multiple`/`Single`). The shape is
/// preserved; only the taint is removed. This is the analyzer's ONLY
/// sanitizer.
pub fn disclose(abs: &Abs) -> Abs {
    match abs {
        Abs::Atomic(_) => Abs::Atomic(Vec::new()),
        Abs::Boolean { value, .. } => Abs::Boolean {
            value: *value,
            witnesses: Vec::new(),
        },
        Abs::Multiple(items) => Abs::Multiple(items.iter().map(disclose).collect()),
        Abs::Single(inner) => Abs::Single(Box::new(disclose(inner))),
    }
}

/// `combine-abs` (R0 F2): the join / lattice union. Witnesses are unioned
/// (`merge_witnesses`, not intersected); `Multiple`/`Single` join elementwise
/// over matching shapes, and `Single` distributes over `Multiple` (each
/// `Multiple` child is joined against the `Single`'s element). Two constant
/// `Boolean`s keep `Boolean` only if their `value`s agree; otherwise (or
/// against a non-`Boolean` operand) the result decays to `Atomic`, per the
/// source's `Abs-boolean` arm.
///
/// The remaining combinations (`Atomic`/`Boolean` vs `Multiple`/`Single`) are
/// shapes the source's own invariant forbids (`abs1`/`abs2` always have the
/// same shape, up to the `Single`-distributes-over-`Multiple` case above) —
/// defensive-only, fail-closed by unioning every reachable witness into a
/// single `Atomic` rather than silently dropping taint.
pub fn combine_abs(a: &Abs, b: &Abs) -> Abs {
    match (a, b) {
        (Abs::Atomic(w1), Abs::Atomic(w2)) => Abs::Atomic(merge_witnesses(w1, w2)),
        (
            Abs::Boolean {
                value: t1,
                witnesses: w1,
            },
            Abs::Boolean {
                value: t2,
                witnesses: w2,
            },
        ) => {
            if t1 == t2 {
                Abs::Boolean {
                    value: *t1,
                    witnesses: merge_witnesses(w1, w2),
                }
            } else {
                Abs::Atomic(merge_witnesses(w1, w2))
            }
        }
        (Abs::Boolean { witnesses: w1, .. }, Abs::Atomic(w2))
        | (Abs::Atomic(w2), Abs::Boolean { witnesses: w1, .. }) => {
            Abs::Atomic(merge_witnesses(w1, w2))
        }
        (Abs::Multiple(a1), Abs::Multiple(a2)) => {
            // Typing guarantees equal arity (same struct/tuple shape); zip
            // would otherwise silently truncate to the shorter side.
            debug_assert_eq!(a1.len(), a2.len());
            Abs::Multiple(
                a1.iter()
                    .zip(a2.iter())
                    .map(|(x, y)| combine_abs(x, y))
                    .collect(),
            )
        }
        (Abs::Single(a1), Abs::Single(a2)) => Abs::Single(Box::new(combine_abs(a1, a2))),
        (Abs::Multiple(a1), Abs::Single(a2)) => {
            Abs::Multiple(a1.iter().map(|x| combine_abs(x, a2)).collect())
        }
        (Abs::Single(a1), Abs::Multiple(a2)) => {
            Abs::Multiple(a2.iter().map(|y| combine_abs(a1, y)).collect())
        }
        (x, y) => Abs::Atomic(merge_witnesses(&witnesses_of(x), &witnesses_of(y))),
    }
}

/// `abs-equal?` (R0 FX3): structural equality used by the fold fixpoint (FX1,
/// `interp_fold`) to detect convergence. Same `Abs` shape and the same witness
/// set at every level (uid + path, order-independent).
pub fn abs_equal(a: &Abs, b: &Abs) -> bool {
    match (a, b) {
        (Abs::Atomic(w1), Abs::Atomic(w2)) => witness_sets_equal(w1, w2),
        (
            Abs::Boolean {
                value: t1,
                witnesses: w1,
            },
            Abs::Boolean {
                value: t2,
                witnesses: w2,
            },
        ) => t1 == t2 && witness_sets_equal(w1, w2),
        (Abs::Multiple(a1), Abs::Multiple(a2)) => {
            a1.len() == a2.len() && a1.iter().zip(a2.iter()).all(|(x, y)| abs_equal(x, y))
        }
        (Abs::Single(a1), Abs::Single(a2)) => abs_equal(a1, a2),
        _ => false,
    }
}

/// Order-independent witness-set equality (uid + path), used by `abs_equal`.
fn witness_sets_equal(w1: &[Witness], w2: &[Witness]) -> bool {
    let mut s1 = w1.to_vec();
    let mut s2 = w2.to_vec();
    s1.sort_by_key(|w| w.uid);
    s2.sort_by_key(|w| w.uid);
    s1 == s2
}

/// Collects every witness reachable from `abs`, at every level (`Atomic`,
/// `Boolean`, and recursively through `Multiple`/`Single`), folded through
/// `merge_witnesses` (R0 `abs->witnesses`: dedup by uid, union paths) so the
/// same witness reachable from two `Multiple` children is reported once, not
/// once per occurrence. Used by sinks (A4) and as a test helper here.
pub fn witnesses_of(abs: &Abs) -> Vec<Witness> {
    match abs {
        Abs::Atomic(witnesses) | Abs::Boolean { witnesses, .. } => witnesses.clone(),
        Abs::Multiple(items) => items
            .iter()
            .map(witnesses_of)
            .fold(Vec::new(), |acc, w| merge_witnesses(&acc, &w)),
        Abs::Single(inner) => witnesses_of(inner),
    }
}

/// Whether any witness value is present in an abstract value. Used to gate the
/// §0 fail-closed advisories: at an unanalyzable point with demonstrably nothing
/// private in play, the advisory is pure noise (it can never front a real leak),
/// so it is suppressed. This does NOT change the interpretation — children are
/// still walked, taint unions still built, leaks still recorded.
pub fn taint_present(abs: &Abs) -> bool {
    !witnesses_of(abs).is_empty()
}

/// Union of two witness lists, deduped by `uid` and kept sorted by `uid`
/// (R0 F2 `merge-witnesses`: "dedup by uid, union"). On a duplicate uid the
/// two entries are merged into one by unioning their `path`s (mirrors the
/// source's `(append path1* path2*)`) rather than discarding either side —
/// dropping a path would lose part of the witness->sink trail (spec §3.7).
pub fn merge_witnesses(w1: &[Witness], w2: &[Witness]) -> Vec<Witness> {
    let mut out: Vec<Witness> = w1.to_vec();
    for w in w2 {
        if let Some(existing) = out.iter_mut().find(|existing| existing.uid == w.uid) {
            existing.path.extend(w.path.iter().cloned());
        } else {
            out.push(w.clone());
        }
    }
    out.sort_by_key(|w| w.uid);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disclose_clears_all_levels_and_join_unions() {
        let w = Witness {
            uid: 1,
            src: text_size::TextRange::empty(0.into()),
            info: WitnessInfo::CircuitArg {
                function: "f".into(),
                arg: "x".into(),
            },
            path: vec![],
        };
        let tainted = Abs::Multiple(vec![
            Abs::Atomic(vec![w.clone()]),
            Abs::Single(Box::new(Abs::Atomic(vec![w.clone()]))),
        ]);
        // disclose clears witnesses at every level:
        assert!(witnesses_of(&disclose(&tainted)).is_empty());
        // combine_abs unions (not intersects):
        let joined = combine_abs(&Abs::Atomic(vec![w.clone()]), &Abs::Atomic(vec![]));
        assert_eq!(witnesses_of(&joined).len(), 1);
    }

    #[test]
    fn combine_abs_distributes_single_over_multiple() {
        let w = Witness {
            uid: 1,
            src: text_size::TextRange::empty(0.into()),
            info: WitnessInfo::CircuitArg {
                function: "f".into(),
                arg: "x".into(),
            },
            path: vec![],
        };
        let multiple = Abs::Multiple(vec![Abs::Atomic(vec![w.clone()])]);
        let single = Abs::Single(Box::new(Abs::Atomic(vec![])));
        // R0 F2: Single distributes over Multiple -- the result stays
        // Multiple (never flattens to Atomic), and each child is joined
        // against the Single's element.
        let joined = combine_abs(&multiple, &single);
        match joined {
            Abs::Multiple(children) => {
                assert_eq!(children.len(), 1);
                assert_eq!(witnesses_of(&children[0]).len(), 1);
            }
            other => panic!("expected Abs::Multiple, got {other:?}"),
        }
    }

    #[test]
    fn merge_witnesses_unions_paths_on_uid_collision() {
        let path_a = PathPoint {
            src: text_size::TextRange::empty(0.into()),
            description: "A".into(),
            exposure: "".into(),
        };
        let path_b = PathPoint {
            src: text_size::TextRange::empty(0.into()),
            description: "B".into(),
            exposure: "".into(),
        };
        let info = WitnessInfo::CircuitArg {
            function: "f".into(),
            arg: "x".into(),
        };
        let w1 = Witness {
            uid: 1,
            src: text_size::TextRange::empty(0.into()),
            info: info.clone(),
            path: vec![path_a.clone()],
        };
        let w2 = Witness {
            uid: 1,
            src: text_size::TextRange::empty(0.into()),
            info,
            path: vec![path_b.clone()],
        };
        // R0 F2 merge-witnesses: dedup by uid, UNION (not first-wins) --
        // both witnesses' paths survive in one merged entry.
        let merged = merge_witnesses(&[w1], &[w2]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].path, vec![path_a, path_b]);
    }

    #[test]
    fn witnesses_of_dedups_across_multiple_children() {
        let w = Witness {
            uid: 1,
            src: text_size::TextRange::empty(0.into()),
            info: WitnessInfo::CircuitArg {
                function: "f".into(),
                arg: "x".into(),
            },
            path: vec![],
        };
        // The same witness (uid) reachable from two Multiple children --
        // abs->witnesses folds via merge-witnesses, so it's reported once.
        let abs = Abs::Multiple(vec![Abs::Atomic(vec![w.clone()]), Abs::Atomic(vec![w])]);
        assert_eq!(witnesses_of(&abs).len(), 1);
    }

    #[test]
    fn abs_equal_is_structural_and_order_independent() {
        let w1 = Witness {
            uid: 1,
            src: text_size::TextRange::empty(0.into()),
            info: WitnessInfo::CircuitArg {
                function: "f".into(),
                arg: "x".into(),
            },
            path: vec![],
        };
        let w2 = Witness {
            uid: 2,
            src: text_size::TextRange::empty(0.into()),
            info: WitnessInfo::ConstructorArg { arg: "y".into() },
            path: vec![],
        };
        // Same witness set, different insertion order -> still equal (FX3 is
        // a set comparison, not list-positional).
        let a = Abs::Atomic(vec![w1.clone(), w2.clone()]);
        let b = Abs::Atomic(vec![w2.clone(), w1.clone()]);
        assert!(abs_equal(&a, &b));

        // Fewer witnesses -> not equal.
        assert!(!abs_equal(&a, &Abs::Atomic(vec![w1.clone()])));

        // A fixpoint's driving case: the reached fixpoint of a converging
        // sequence compares equal to itself once no new witness is added.
        assert!(abs_equal(
            &Abs::Multiple(vec![
                Abs::Atomic(vec![w1.clone()]),
                Abs::Single(Box::new(Abs::Atomic(vec![w2.clone()])))
            ]),
            &Abs::Multiple(vec![
                Abs::Atomic(vec![w1]),
                Abs::Single(Box::new(Abs::Atomic(vec![w2])))
            ]),
        ));
    }
}
