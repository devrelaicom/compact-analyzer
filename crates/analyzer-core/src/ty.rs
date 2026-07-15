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
}
