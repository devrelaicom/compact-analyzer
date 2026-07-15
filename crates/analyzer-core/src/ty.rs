//! The native checker's type universe. `Ty` is a salsa-interned, lifetime-free
//! (`'static`) type id: equality is id comparison and types are shared
//! workspace-wide. The foundation ships only the constructors the
//! primitive-literal slice needs; the universe grows one rule at a time
//! (v2b.3…N). `Ty` never escapes `analyzer-core` — [`ty_display`] is the
//! projection the IDE layer consumes.

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

/// A salsa-interned type. Lifetime-free (all fields are `'static`), so a `Ty`
/// is a `Copy`, workspace-shared id — two independently constructed
/// `Boolean`s are the same `Ty`.
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
