//! The `Abs` abstract-value lattice (spec §3.4), translated faithfully from
//! the verified compiler model (`analysis-passes.ss`, `compactc-v0.31.1`,
//! commit `0da5b045` — see R0 index rows AB1/AB2, D1, F2, FX3). Struct/tuple
//! fields are tracked individually (`Multiple`); arrays/vectors are tracked
//! in the aggregate (`Single`); `Boolean` exists only for compile-time
//! constants so `if` can constant-fold; everything else is `Atomic`.
//!
//! Pure lattice logic only — no salsa, no interpretation. A2/A3 build the
//! interpreter that produces and consumes these values.

// A1 lands the lattice with no production caller yet — it's exercised only
// by the unit tests below until A2 (source seeding) and A3 (the interpreter)
// consume it. `expect`, not `allow`, so the lint stays honest: once a real
// caller lands, the expectation goes unfulfilled and this attribute must be
// deleted.
#![expect(dead_code)]

/// A witness value's origin (spec §3.1). Deduped by `uid` in a set.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WitnessInfo {
    WitnessReturn { function: String },
    ConstructorArg { arg: String },
    CircuitArg { function: String, arg: String },
}

/// One tracked witness value: where it entered + the path it has taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Witness {
    pub uid: u32,
    pub src: text_size::TextRange,
    pub info: WitnessInfo,
    pub path: Vec<PathPoint>, // spec §3.7
}

/// A point on the witness->sink trail (spec §3.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathPoint {
    pub src: text_size::TextRange,
    pub description: String, // e.g. "the argument to the ledger write"
    pub exposure: String,    // e.g. "a hash of"; "" if unmodified
}

/// The abstract value lattice (spec §3.4). Struct/tuple fields individual;
/// arrays aggregate; booleans only for compile-time constants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Abs {
    Atomic(Vec<Witness>),
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
/// over matching shapes. Two constant `Boolean`s keep `Boolean` only if their
/// `value`s agree; otherwise (or against a non-`Boolean` operand) the result
/// decays to `Atomic`, per the source's `Abs-boolean` arm.
///
/// The source's own invariant is that `abs1` and `abs2` have the same shape;
/// any other combination (e.g. `Multiple` vs `Single`) is defensive-only —
/// fail-closed by unioning every reachable witness into a single `Atomic`
/// rather than silently dropping taint.
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
        (Abs::Multiple(a1), Abs::Multiple(a2)) => Abs::Multiple(
            a1.iter()
                .zip(a2.iter())
                .map(|(x, y)| combine_abs(x, y))
                .collect(),
        ),
        (Abs::Single(a1), Abs::Single(a2)) => Abs::Single(Box::new(combine_abs(a1, a2))),
        (x, y) => Abs::Atomic(merge_witnesses(&witnesses_of(x), &witnesses_of(y))),
    }
}

/// `abs-equal?` (R0 FX3): structural equality used by the fold/map fixpoint
/// (FX1) to detect convergence. Same `Abs` shape and the same witness set at
/// every level (uid + path, order-independent).
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
/// `Boolean`, and recursively through `Multiple`/`Single`). Used by sinks
/// (A4) and as a test helper here.
pub fn witnesses_of(abs: &Abs) -> Vec<Witness> {
    match abs {
        Abs::Atomic(witnesses) | Abs::Boolean { witnesses, .. } => witnesses.clone(),
        Abs::Multiple(items) => items.iter().flat_map(witnesses_of).collect(),
        Abs::Single(inner) => witnesses_of(inner),
    }
}

/// Union of two witness lists, deduped by `uid` and kept sorted by `uid`
/// (mirrors the source's own invariant on `witness*`, line 4714: "sorted by
/// uid, no duplicates"). On a duplicate uid the first-seen entry (from `w1`)
/// wins.
pub fn merge_witnesses(w1: &[Witness], w2: &[Witness]) -> Vec<Witness> {
    let mut out: Vec<Witness> = w1.to_vec();
    for w in w2 {
        if !out.iter().any(|existing| existing.uid == w.uid) {
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
