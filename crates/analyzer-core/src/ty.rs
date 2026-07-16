//! The native checker's type universe. `Ty` is a salsa-interned type id:
//! equality is id comparison and types are shared workspace-wide. Under the
//! `#[salsa::interned]` macro `Ty` carries an *elided* `'db` lifetime (it is
//! `Ty<'db>`, tied to the database borrow it was interned under); ordinary
//! signature elision means no code in this crate spells the lifetime out, but
//! `Ty` is **not** `'static`. In particular a `#[salsa::tracked]` query cannot
//! return a `Ty<'db>` inside a collection (e.g. `Arc<[(_, Ty)]>`) — carry the
//! `'static` `TyKind` from queries and intern `Ty` at the display boundary. The
//! foundation ships only the constructors the primitive-literal slice needs;
//! the universe grows one rule at a time (v2b.3…N). `Ty` never escapes
//! `analyzer-core` — [`ty_display`] is the projection the IDE layer consumes.

use crate::db::Db;
use std::sync::Arc;

/// A Compact type, as far as the foundation models it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TyKind {
    /// `Boolean`.
    Boolean,
    /// `Field`.
    Field,
    /// A type the foundation does not yet model (any non-primitive, or an
    /// absent annotation). Never the *source* of a type error: an `Unknown`
    /// operand or expectation suppresses a rule rather than firing it.
    Unknown,
    /// A bounded unsigned integer `Uint<0..upper>` (half-open: values `[0, upper)`).
    /// The payload is the **exclusive upper bound**, or `None` when it exceeds
    /// `u128` and is not exactly representable — in which case the lattice
    /// compares it conservatively (never a false positive). `Uint<k>` and
    /// `Uint<lo..hi>` both lower to this single canonical form.
    Uint(Option<u128>),
    /// A `Bytes<n>` byte array of length `n`. Implicitly assignable only to
    /// `Bytes<n>` of the *same* length (see [`is_subtype`]); castable per
    /// [`can_cast`]. Byte counts are small, so the length is a `u32`; a
    /// non-literal or overflowing size lowers to `Unknown` upstream.
    Bytes(u32),
    /// `[T0, …, Tk]`. An empty tuple is the unit type `[]`. A tuple and a
    /// `Vector` of equal length compare element-wise (they are the same
    /// "sequence" type to the compiler).
    Tuple(Arc<[TyKind]>),
    /// `Vector<n, T>` — presents as `n` elements each of type `T`; equivalent
    /// to the homogeneous n-element tuple. A non-literal or overflowing size
    /// lowers to `Unknown` upstream, so `n` is always a real `u32` here.
    Vector(u32, Arc<TyKind>),
    /// A nominal `struct` type: its declared name plus its ordered
    /// `(field-name, field-type)` list. Assignability is nominal (same name),
    /// verified against compactc — a differently-named struct of identical
    /// shape is NOT assignable. Fields carry types so member access can type
    /// `s.field` (v2c). An unresolvable struct reference lowers to `Unknown`.
    Struct {
        name: Arc<str>,
        fields: Arc<[(Arc<str>, TyKind)]>,
    },
    /// A nominal `enum` type: its declared name plus its ordered variant names.
    /// Displays as `Enum<Name, V0, V1, …>` (verified). Nominal assignability.
    Enum {
        name: Arc<str>,
        variants: Arc<[Arc<str>]>,
    },
}

/// A salsa-interned type id. Under `#[salsa::interned]` this is `Ty<'db>` with
/// an elidable lifetime (elided at every call site here), tied to the database
/// borrow it was produced under — it is **not** `'static`. Equality is id
/// comparison: two independently constructed `Boolean`s are the same `Ty`.
#[salsa::interned]
#[derive(Debug)]
pub struct Ty {
    #[returns(clone)]
    pub kind: TyKind,
}

/// Display projection for the IDE layer (hover / completion detail).
pub fn ty_display(db: &dyn Db, ty: Ty) -> String {
    display_kind(&ty.kind(db))
}

/// Pure display projection over a `TyKind`; `ty_display` delegates to this.
/// Public so IDE-layer consumers with a bare `TyKind` in hand (no `Ty`/`db`
/// pair to feed `ty_display`) — e.g. v2c struct-field completion, which reads
/// `TyKind::Struct.fields` directly off `AnalysisHost::type_at` — can render
/// it without fabricating a `Ty`.
pub fn display_kind(kind: &TyKind) -> String {
    match kind {
        TyKind::Boolean => "Boolean".to_string(),
        TyKind::Field => "Field".to_string(),
        TyKind::Unknown => "?".to_string(),
        TyKind::Uint(Some(hi)) => {
            // hi == 2^k for 1 <= k <= 248 renders as the bit-width form.
            if hi.is_power_of_two() {
                let k = hi.trailing_zeros();
                if (1..=248).contains(&k) {
                    return format!("Uint<{k}>");
                }
            }
            format!("Uint<0..{hi}>")
        }
        TyKind::Uint(None) => "Uint<0..?>".to_string(),
        TyKind::Bytes(n) => format!("Bytes<{n}>"),
        TyKind::Tuple(elems) => {
            let inner = elems
                .iter()
                .map(display_kind)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        TyKind::Vector(n, elem) => format!("Vector<{n}, {}>", display_kind(elem)),
        TyKind::Struct { name, .. } => name.to_string(),
        TyKind::Enum { name, variants } => {
            let mut out = format!("Enum<{name}");
            for v in variants.iter() {
                out.push_str(", ");
                out.push_str(v);
            }
            out.push('>');
            out
        }
    }
}

/// Assignability: `true` iff a value of `sub` may be used where `sup` is
/// expected, per the compiler's type checker (verified against compact 0.5.1).
/// `Unknown` on either side yields `true` — the checker only *suppresses* on an
/// unmodeled type, it never manufactures a mismatch from one.
pub(crate) fn is_subtype(sub: &TyKind, sup: &TyKind) -> bool {
    match (sub, sup) {
        (TyKind::Unknown, _) | (_, TyKind::Unknown) => true,
        (TyKind::Boolean, TyKind::Boolean) => true,
        (TyKind::Field, TyKind::Field) => true,
        // Every Uint is a subtype of Field (widening to Field always allowed).
        (TyKind::Uint(_), TyKind::Field) => true,
        // Uint<0..a> <: Uint<0..b> iff a <= b (range containment).
        (TyKind::Uint(a), TyKind::Uint(b)) => uint_upper_le(*a, *b),
        // Bytes<n> <: Bytes<m> iff n == m; Bytes is unrelated to all else.
        (TyKind::Bytes(a), TyKind::Bytes(b)) => a == b,
        // Tuple & Vector are one sequence relation (all four combinations).
        (TyKind::Tuple(_) | TyKind::Vector(_, _), TyKind::Tuple(_) | TyKind::Vector(_, _)) => {
            seq_subtype(sub, sup)
        }
        // Nominal: structs assignable iff same name (compactc is nominal — a
        // differently-named struct of identical shape is not assignable, Step 1).
        (TyKind::Struct { name: a, .. }, TyKind::Struct { name: b, .. }) => a == b,
        (TyKind::Enum { name: a, .. }, TyKind::Enum { name: b, .. }) => a == b,
        // Everything else (Boolean/Field/Uint cross, Field->Uint, Bytes
        // cross-kind) is unrelated.
        _ => false,
    }
}

/// Number of elements a sequence type presents, or `None` if not a sequence.
fn seq_len(t: &TyKind) -> Option<u32> {
    match t {
        TyKind::Tuple(v) => Some(v.len() as u32),
        TyKind::Vector(n, _) => Some(*n),
        _ => None,
    }
}

/// The element type at index `i` of a sequence (a Vector yields its single
/// element type for every index). Callers only pass `i < seq_len`.
fn seq_elem(t: &TyKind, i: u32) -> &TyKind {
    match t {
        TyKind::Tuple(v) => &v[i as usize],
        TyKind::Vector(_, e) => e,
        _ => unreachable!("seq_elem on non-sequence"),
    }
}

/// Equal length + element-wise covariance, recursing through `is_subtype`.
fn seq_subtype(sub: &TyKind, sup: &TyKind) -> bool {
    match (seq_len(sub), seq_len(sup)) {
        (Some(a), Some(b)) if a == b => {
            (0..a).all(|i| is_subtype(seq_elem(sub, i), seq_elem(sup, i)))
        }
        _ => false,
    }
}

/// `a <= b` on exclusive upper bounds, conservative when a bound is not exactly
/// representable (`None` = "≥ 2^128"). Exact whenever both fit `u128`. See the
/// plan's *Representation design* table: the only non-exact case is
/// `(None, None)`, resolved to `true` (a possible false negative, never a false
/// positive).
fn uint_upper_le(a: Option<u128>, b: Option<u128>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a <= b,
        (Some(_), None) => true,  // concrete < 2^128 <= huge
        (None, Some(_)) => false, // huge >= 2^128 > concrete
        (None, None) => true,     // undecidable -> conservative accept
    }
}

/// Cast legality: `true` iff `src as tgt` is accepted by the compiler's type
/// checker (verified against compact 0.5.1 / compiler 0.31.1). Casts are far
/// more permissive than subtyping ([`is_subtype`]) — every `Boolean`/`Field`/
/// `Uint` pair interconverts, and `Field`/`Uint` interconvert with `Bytes` of
/// any size. The *only* rejected casts among modeled primitives:
///   - `Boolean` <-> `Bytes` (either direction), and
///   - `Bytes<a>` <-> `Bytes<b>` with `a != b` (Bytes casts must preserve size).
///
/// `Unknown` on either side yields `true` (the checker only suppresses on an
/// unmodeled type; it never manufactures a cast error from one). Any pair not
/// *confirmed* illegal is treated as legal — conservative, never a false
/// positive.
pub(crate) fn can_cast(src: &TyKind, tgt: &TyKind) -> bool {
    match (src, tgt) {
        (TyKind::Unknown, _) | (_, TyKind::Unknown) => true,
        // Boolean cannot cast to or from Bytes.
        (TyKind::Boolean, TyKind::Bytes(_)) | (TyKind::Bytes(_), TyKind::Boolean) => false,
        // Bytes-to-Bytes must preserve size.
        (TyKind::Bytes(a), TyKind::Bytes(b)) => a == b,
        // Every other pair among modeled primitives is castable. A composite
        // type falls into this catch-all (cast legality of composites is not
        // modeled — suppress, never a false positive).
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CompactDatabase;

    #[test]
    fn interning_gives_id_equality() {
        let db = CompactDatabase::default();
        let a = Ty::new(&db, TyKind::Boolean);
        let b = Ty::new(&db, TyKind::Boolean);
        let c = Ty::new(&db, TyKind::Field);
        assert_eq!(a, b, "equal kinds intern to the same id");
        assert_ne!(a, c);
    }

    #[test]
    fn display_projects_to_strings() {
        let db = CompactDatabase::default();
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Boolean)), "Boolean");
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Field)), "Field");
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Unknown)), "?");
    }

    #[test]
    fn uint_display_normalizes_power_of_two() {
        let db = CompactDatabase::default();
        // hi == 2^k -> Uint<k>
        assert_eq!(
            ty_display(&db, Ty::new(&db, TyKind::Uint(Some(256)))),
            "Uint<8>"
        );
        assert_eq!(
            ty_display(&db, Ty::new(&db, TyKind::Uint(Some(2)))),
            "Uint<1>"
        );
        // non power of two -> Uint<0..hi>
        assert_eq!(
            ty_display(&db, Ty::new(&db, TyKind::Uint(Some(10)))),
            "Uint<0..10>"
        );
        assert_eq!(
            ty_display(&db, Ty::new(&db, TyKind::Uint(Some(1)))),
            "Uint<0..1>"
        );
        assert_eq!(
            ty_display(&db, Ty::new(&db, TyKind::Uint(Some(257)))),
            "Uint<0..257>"
        );
        // not exactly representable
        assert_eq!(
            ty_display(&db, Ty::new(&db, TyKind::Uint(None))),
            "Uint<0..?>"
        );
    }

    #[test]
    fn uint_interns_by_bound() {
        let db = CompactDatabase::default();
        let a = Ty::new(&db, TyKind::Uint(Some(256)));
        let b = Ty::new(&db, TyKind::Uint(Some(256)));
        let c = Ty::new(&db, TyKind::Uint(Some(255)));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn bytes_display_and_interning() {
        let db = CompactDatabase::default();
        assert_eq!(
            ty_display(&db, Ty::new(&db, TyKind::Bytes(32))),
            "Bytes<32>"
        );
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Bytes(1))), "Bytes<1>");
        // interns by size
        let a = Ty::new(&db, TyKind::Bytes(32));
        let b = Ty::new(&db, TyKind::Bytes(32));
        let c = Ty::new(&db, TyKind::Bytes(4));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn subtype_lattice_matches_compiler() {
        use TyKind::*;
        // reflexive within a kind
        assert!(is_subtype(&Boolean, &Boolean));
        assert!(is_subtype(&Field, &Field));
        // Uint containment (a <: b iff a <= b)
        assert!(is_subtype(&Uint(Some(6)), &Uint(Some(10)))); // 0..5 into 0..10 -> literal 5 case
        assert!(!is_subtype(&Uint(Some(12)), &Uint(Some(10)))); // 0..12 into 0..10 -> literal 11 case
        assert!(is_subtype(&Uint(Some(10)), &Uint(Some(10))));
        assert!(is_subtype(&Uint(Some(256)), &Uint(Some(65536)))); // Uint8 into Uint16
        assert!(!is_subtype(&Uint(Some(65536)), &Uint(Some(256)))); // Uint16 into Uint8
        // Uint <: Field, one way only
        assert!(is_subtype(&Uint(Some(256)), &Field));
        assert!(!is_subtype(&Field, &Uint(Some(256))));
        // Boolean isolated
        assert!(!is_subtype(&Boolean, &Field));
        assert!(!is_subtype(&Field, &Boolean));
        assert!(!is_subtype(&Boolean, &Uint(Some(256))));
        assert!(!is_subtype(&Uint(Some(256)), &Boolean));
        // conservative on non-representable bounds (never a false positive)
        assert!(is_subtype(&Uint(Some(5)), &Uint(None))); // concrete fits huge
        assert!(!is_subtype(&Uint(None), &Uint(Some(5)))); // huge cannot fit concrete
        assert!(is_subtype(&Uint(None), &Uint(None))); // undecidable -> conservative accept
        assert!(is_subtype(&Uint(None), &Field)); // any Uint <: Field
        // Unknown suppresses in both directions
        assert!(is_subtype(&Unknown, &Field));
        assert!(is_subtype(&Boolean, &Unknown));
    }

    #[test]
    fn bytes_subtype_is_same_size_only() {
        use TyKind::*;
        // reflexive by size
        assert!(is_subtype(&Bytes(32), &Bytes(32)));
        assert!(is_subtype(&Bytes(1), &Bytes(1)));
        // different size is unrelated (both directions)
        assert!(!is_subtype(&Bytes(4), &Bytes(32)));
        assert!(!is_subtype(&Bytes(32), &Bytes(4)));
        // Bytes is unrelated to every other kind
        assert!(!is_subtype(&Bytes(32), &Field));
        assert!(!is_subtype(&Field, &Bytes(32)));
        assert!(!is_subtype(&Bytes(32), &Uint(Some(256))));
        assert!(!is_subtype(&Uint(Some(256)), &Bytes(32)));
        assert!(!is_subtype(&Bytes(32), &Boolean));
        assert!(!is_subtype(&Boolean, &Bytes(32)));
        // Unknown still suppresses in both directions
        assert!(is_subtype(&Unknown, &Bytes(32)));
        assert!(is_subtype(&Bytes(32), &Unknown));
    }

    #[test]
    fn can_cast_matches_compiler() {
        use TyKind::*;
        // The only illegal casts among modeled primitives:
        assert!(!can_cast(&Boolean, &Bytes(32))); // Boolean -> Bytes
        assert!(!can_cast(&Bytes(32), &Boolean)); // Bytes -> Boolean
        assert!(!can_cast(&Bytes(32), &Bytes(4))); // Bytes size change
        assert!(!can_cast(&Bytes(4), &Bytes(32)));
        // Everything else is castable:
        assert!(can_cast(&Bytes(32), &Bytes(32))); // identity
        assert!(can_cast(&Boolean, &Field));
        assert!(can_cast(&Field, &Boolean));
        assert!(can_cast(&Boolean, &Uint(Some(256))));
        assert!(can_cast(&Uint(Some(6)), &Boolean));
        assert!(can_cast(&Uint(Some(6)), &Bytes(32))); // Uint <-> Bytes any size
        assert!(can_cast(&Bytes(32), &Uint(Some(256))));
        assert!(can_cast(&Field, &Bytes(4)));
        assert!(can_cast(&Bytes(4), &Field));
        assert!(can_cast(&Uint(Some(256)), &Uint(Some(6)))); // Uint->Uint ignores bounds
        // Unknown on either side -> legal (suppress).
        assert!(can_cast(&Unknown, &Bytes(32)));
        assert!(can_cast(&Boolean, &Unknown));
    }

    #[test]
    fn tuple_and_vector_display() {
        use std::sync::Arc;
        let db = CompactDatabase::default();
        let tup = TyKind::Tuple(Arc::from(vec![TyKind::Uint(Some(256)), TyKind::Boolean]));
        assert_eq!(ty_display(&db, Ty::new(&db, tup)), "[Uint<8>, Boolean]");
        let vec = TyKind::Vector(3, Arc::new(TyKind::Uint(Some(256))));
        assert_eq!(ty_display(&db, Ty::new(&db, vec)), "Vector<3, Uint<8>>");
        // Empty tuple is the unit type.
        assert_eq!(
            ty_display(&db, Ty::new(&db, TyKind::Tuple(Arc::from(vec![])))),
            "[]"
        );
    }

    #[test]
    fn sequence_subtyping_matches_compiler() {
        use TyKind::*;
        use std::sync::Arc;
        let u8k = || Uint(Some(256));
        let u16k = || Uint(Some(65536));
        let tup = |v: Vec<TyKind>| Tuple(Arc::from(v));
        let vec = |n, t| Vector(n, Arc::new(t));

        // Element-wise covariance: [Uint8, Uint8] <: [Uint16, Field]
        assert!(is_subtype(
            &tup(vec![u8k(), u8k()]),
            &tup(vec![u16k(), Field])
        ));
        // Narrowing an element rejects: [Uint16, Uint16] </: [Uint8, Uint8]
        assert!(!is_subtype(
            &tup(vec![u16k(), u16k()]),
            &tup(vec![u8k(), u8k()])
        ));
        // Non-covariant element rejects: [Uint8, Boolean] </: [Uint16, Field]
        assert!(!is_subtype(
            &tup(vec![u8k(), Boolean]),
            &tup(vec![u16k(), Field])
        ));
        // Arity mismatch rejects.
        assert!(!is_subtype(&tup(vec![u8k(), u8k()]), &tup(vec![u8k()])));
        // Vector covariance + length: Vector<3,Uint8> <: Vector<3,Uint16>, not Vector<2,_>.
        assert!(is_subtype(&vec(3, u8k()), &vec(3, u16k())));
        assert!(!is_subtype(&vec(3, u16k()), &vec(3, u8k())));
        assert!(!is_subtype(&vec(3, u8k()), &vec(2, u8k())));
        // THE equivalence: Vector<3,Uint8> <-> [Uint8, Uint8, Uint8], both directions,
        // and cross-kind covariance.
        assert!(is_subtype(&vec(3, u8k()), &tup(vec![u8k(), u8k(), u8k()])));
        assert!(is_subtype(&tup(vec![u8k(), u8k(), u8k()]), &vec(3, u8k())));
        assert!(is_subtype(&vec(2, u8k()), &tup(vec![u16k(), Field])));
        assert!(is_subtype(&tup(vec![u8k(), u8k()]), &vec(2, Field)));
        // Heterogeneous tuple into a vector rejects when an element is non-covariant.
        assert!(!is_subtype(&tup(vec![u8k(), Boolean]), &vec(2, Field)));
        // Nested sequences recurse.
        assert!(is_subtype(
            &vec(2, tup(vec![u8k(), u8k()])),
            &vec(2, tup(vec![u16k(), Field]))
        ));
        // Unknown element suppresses that position (never a false positive).
        assert!(is_subtype(
            &tup(vec![Unknown, Boolean]),
            &tup(vec![u8k(), Boolean])
        ));
        // A sequence is unrelated to a scalar.
        assert!(!is_subtype(&tup(vec![u8k()]), &u8k()));
        assert!(!is_subtype(&u8k(), &vec(1, u8k())));
    }

    #[test]
    fn struct_display_is_its_name() {
        let s = TyKind::Struct {
            name: Arc::from("S"),
            fields: Arc::from([(Arc::<str>::from("a"), TyKind::Field)]),
        };
        assert_eq!(display_kind(&s), "S");
    }

    #[test]
    fn enum_display_lists_variants() {
        let e = TyKind::Enum {
            name: Arc::from("E"),
            variants: Arc::from([Arc::<str>::from("A"), Arc::<str>::from("B")]),
        };
        assert_eq!(display_kind(&e), "Enum<E, A, B>");
    }

    #[test]
    fn struct_subtype_is_nominal_same_name() {
        let s1 = TyKind::Struct {
            name: Arc::from("S"),
            fields: Arc::from([(Arc::<str>::from("a"), TyKind::Field)]),
        };
        let s2 = s1.clone();
        let other = TyKind::Struct {
            name: Arc::from("T"),
            fields: Arc::from([(Arc::<str>::from("a"), TyKind::Field)]),
        };
        assert!(is_subtype(&s1, &s2));
        assert!(!is_subtype(&s1, &other)); // nominal: different name -> not assignable
        assert!(is_subtype(&s1, &TyKind::Unknown)); // Unknown always suppresses
    }

    #[test]
    fn enum_subtype_is_nominal_same_name() {
        let e1 = TyKind::Enum {
            name: Arc::from("E"),
            variants: Arc::from([Arc::<str>::from("A")]),
        };
        let e2 = e1.clone();
        let f = TyKind::Enum {
            name: Arc::from("F"),
            variants: Arc::from([Arc::<str>::from("A")]),
        };
        assert!(is_subtype(&e1, &e2));
        assert!(!is_subtype(&e1, &f));
    }
}
