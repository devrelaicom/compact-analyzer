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

/// A Compact type, as far as the foundation models it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
}

/// A salsa-interned type id. Under `#[salsa::interned]` this is `Ty<'db>` with
/// an elidable lifetime (elided at every call site here), tied to the database
/// borrow it was produced under — it is **not** `'static`. Equality is id
/// comparison: two independently constructed `Boolean`s are the same `Ty`.
#[salsa::interned]
#[derive(Debug)]
pub struct Ty {
    #[returns(copy)]
    pub kind: TyKind,
}

/// Display projection for the IDE layer (hover / completion detail).
pub fn ty_display(db: &dyn Db, ty: Ty) -> String {
    match ty.kind(db) {
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
}
