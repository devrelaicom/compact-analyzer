//! Inlay hints for un-annotated `const` binding types, via v2b.9's `type_at`.
//!
//! Pure consumer: never guesses. `type_at` returns `None` for anything
//! unmodeled (Unknown), so an initializer expression the checker can't type
//! (e.g. a call to an unresolved function) gets no hint at all, rather than a
//! wrong or placeholder one.

use analyzer_core::{AnalysisHost, FileId, SyntaxNode};
use compactp_ast::AstNode;

/// A single type hint: `label` (e.g. `": Field"`) is meant to be inserted at
/// the byte `offset` (just after the binding-name identifier).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlayHint {
    pub offset: u32,
    pub label: String,
}

/// Type hints for every un-annotated `const` binding in `file`, in document
/// order. Skips: consts that already carry an explicit type annotation
/// (redundant); destructuring patterns (`const [a, b] = ...` / `const {a} =
/// ...`) — there's no single binding-name position to anchor the hint on;
/// and any initializer whose `type_at` comes back `None` (unmodeled —
/// never a guessed hint).
pub fn inlay_hints(host: &mut AnalysisHost, file: FileId) -> Vec<InlayHint> {
    let Some(analysis) = host.analyze(file) else {
        return Vec::new();
    };
    let root = SyntaxNode::new_root(analysis.green.clone());

    // Collect the un-annotated const bindings' (name-end-offset, init-offset)
    // pairs up front: `type_at` takes `&mut AnalysisHost`, so it can't be
    // called while a borrow of `analysis`/`root` derived from `host.analyze`
    // is still live across the loop below (a second `host.analyze` call per
    // hint is the established pattern elsewhere in this crate, e.g.
    // `completion.rs`'s `push_member`).
    let candidates: Vec<(u32, u32)> = root
        .descendants()
        .filter_map(compactp_ast::ConstStmt::cast)
        .filter(|c| c.ty().is_none())
        .filter_map(|c| {
            let compactp_ast::Pat::Ident(ident_pat) = c.pattern()? else {
                // Tuple/struct destructuring: no single binding-name offset
                // to anchor a hint on (out of scope).
                return None;
            };
            let name_end: u32 = ident_pat.name()?.text_range().end().into();
            let init_offset: u32 = crate::completion::non_trivia_start(c.value()?.syntax()).into();
            Some((name_end, init_offset))
        })
        .collect();

    let mut out = Vec::new();
    for (name_end, init_offset) in candidates {
        if let Some(kind) = host.type_at(file, init_offset.into()) {
            out.push(InlayHint {
                offset: name_end,
                label: format!(": {}", analyzer_core::display_kind(&kind)),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, fixture};

    fn hints_for(source: &str) -> Vec<InlayHint> {
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        inlay_hints(&mut host, file)
    }

    #[test]
    fn const_binding_gets_a_type_hint() {
        // A bare literal `1` types to `TyKind::Uint(Some(2))` (minimal
        // half-open range [0, 2)) — verified ground truth in
        // analyzer-core's `infer.rs::integer_literal_return_is_typed_uint`
        // and `ty.rs`'s own display test: `display_kind` special-cases an
        // exact-power-of-two upper bound (`2 == 1 << 1`) to the compact
        // `Uint<1>` spelling rather than `Uint<0..2>`.
        let hints = hints_for("circuit c(): Field { const x = 1; return x; }");
        assert!(hints.iter().any(|h| h.label == ": Uint<1>"), "{hints:?}");
    }

    #[test]
    fn annotated_const_gets_no_hint() {
        // Already annotated -> redundant, emit nothing.
        let hints = hints_for("circuit c(): Field { const x: Field = 1; return x; }");
        assert!(
            hints.iter().all(|h| !h.label.starts_with(": ")) || hints.is_empty(),
            "{hints:?}"
        );
    }

    #[test]
    fn unmodeled_initializer_gets_no_hint() {
        // A call whose type isn't modeled -> type_at None -> no hint (never a guess).
        let hints = hints_for("circuit c(): Field { const x = mystery(); return 1; }");
        assert!(hints.is_empty(), "{hints:?}");
    }

    #[test]
    fn hint_offset_lands_right_after_the_binding_name() {
        let (clean, _) = fixture::extract("circuit c(): Field { const x = 1; return x; }$0");
        let hints = hints_for(&clean);
        let h = hints.iter().find(|h| h.label == ": Uint<1>").unwrap();
        // "circuit c(): Field { const x" -> offset right after `x`.
        let expected = clean.find("const x").unwrap() as u32 + "const x".len() as u32;
        assert_eq!(h.offset, expected, "{hints:?} in {clean:?}");
    }

    #[test]
    fn destructuring_pattern_gets_no_hint() {
        // No single binding-name position to anchor on -> skip entirely.
        let hints = hints_for("circuit c(): Field { const [a, b] = [1, 2]; return a; }");
        assert!(hints.is_empty(), "{hints:?}");
    }
}
