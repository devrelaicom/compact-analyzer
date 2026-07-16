//! Signature help: at a call site `add(1, |)`, the callee's signature label
//! plus which parameter is active.
//!
//! Pure consumer of the resolver + item-tree `Symbol.signature` string (no
//! v2b.9 `type_at` needed): the callee is resolved by name, and its rendered
//! signature is re-parsed into parameter substrings.

use analyzer_core::{
    AnalysisHost, Definition, FilePosition, SymbolKind, SyntaxKind, SyntaxNode, SyntaxToken,
};
use compactp_ast::AstNode;
use compactp_ast::expr::CallExpr;
use rowan::TokenAtOffset;

/// A parameter's UTF-8 byte range **into [`SignatureHelpData::label`]**
/// (not into the source file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamLabel {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelpData {
    /// The callee's full rendered signature, e.g.
    /// `"circuit add(x: Field, y: Field): Field"`.
    pub label: String,
    /// One entry per parameter, offsets into `label`.
    pub params: Vec<ParamLabel>,
    /// Index into `params` of the parameter the cursor is currently in.
    /// `None` when the signature has zero parameters.
    pub active_param: Option<u32>,
}

/// Signature help at `pos`: finds the enclosing call whose argument-list
/// parens contain the cursor, resolves the callee to a circuit/witness item,
/// and renders its signature plus the active-parameter index.
///
/// Returns `None` (never a wrong signature) when: there is no enclosing call
/// at `pos`; the call is a method call (`recv.method(...)`, out of scope for
/// v2c); the callee name doesn't resolve; or it resolves to something other
/// than a circuit/witness item. Never panics on a stale/out-of-range offset.
pub fn signature_help(host: &mut AnalysisHost, pos: FilePosition) -> Option<SignatureHelpData> {
    let root = {
        let analysis = host.analyze(pos.file)?;
        SyntaxNode::new_root(analysis.green.clone())
    };
    // B1-style clamp: `pos.offset` may be stale and land past EOF.
    let offset = pos.offset.min(root.text_range().end());

    let call = enclosing_call(&root, offset)?;

    // Method calls (`recv.method(...)`, a DOT direct-child token) are out of
    // scope for v2c signature help — mirrors `callee_sig`'s posture
    // (analyzer-core's E3005/E3006 arg checks) of only handling plain calls.
    let has_dot = call
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == SyntaxKind::DOT);
    if has_dot {
        return None;
    }

    let name_tok = call.name()?;
    let def = host.resolve(FilePosition {
        file: pos.file,
        offset: name_tok.text_range().start(),
    })?;
    let Definition::Item { file, index } = def else {
        return None;
    };
    let sym = host
        .analyze(file)?
        .item_tree
        .symbols
        .get(index as usize)?
        .clone();
    if !matches!(sym.kind, SymbolKind::Circuit | SymbolKind::Witness) {
        return None;
    }

    let label = sym.signature;
    let params = parse_params(&label);

    // Every counted comma is a DIRECT token child of this `CALL_EXPR` — a
    // nested call's or array literal's commas belong to THAT node instead,
    // so this count is already top-level by construction (no string
    // depth-tracking needed, unlike `parse_params` below which re-parses a
    // flat signature string and must depth-track `<>`/`[]`/`()` itself).
    let comma_count = call
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::COMMA && t.text_range().start() < offset)
        .count() as u32;
    let active_param = if params.is_empty() {
        None
    } else {
        Some(comma_count.min(params.len() as u32 - 1))
    };

    Some(SignatureHelpData {
        label,
        params,
        active_param,
    })
}

/// The innermost `CALL_EXPR` ancestor of the token(s) adjacent to `offset`
/// whose argument-list parens (`(...)`) contain `offset`. Tries both sides of
/// a token boundary (`TokenAtOffset::Between`), preferring the right token
/// (the common "just typed `(` or `,`" case).
fn enclosing_call(root: &SyntaxNode, offset: analyzer_core::TextSize) -> Option<CallExpr> {
    let candidates: Vec<_> = match root.token_at_offset(offset) {
        TokenAtOffset::None => Vec::new(),
        TokenAtOffset::Single(t) => vec![t],
        TokenAtOffset::Between(l, r) => vec![r, l],
    };
    for tok in candidates {
        let Some(mut node) = tok.parent() else {
            continue;
        };
        loop {
            if let Some(call) = CallExpr::cast(node.clone())
                && let Some((l_paren, r_paren)) = paren_tokens(&call)
                && l_paren.text_range().end() <= offset
                && offset <= r_paren.text_range().start()
            {
                return Some(call);
            }
            match node.parent() {
                Some(parent) => node = parent,
                None => break,
            }
        }
    }
    None
}

/// The direct `L_PAREN`/`R_PAREN` token children of a `CALL_EXPR` — the
/// argument-list delimiters (as opposed to a `GENERIC_ARG_LIST` child node's
/// own `<`/`>`, which are a different token kind and live inside a nested
/// node, not as direct tokens of the call).
fn paren_tokens(call: &CallExpr) -> Option<(SyntaxToken, SyntaxToken)> {
    let mut l_paren = None;
    let mut r_paren = None;
    for tok in call
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
    {
        match tok.kind() {
            SyntaxKind::L_PAREN if l_paren.is_none() => l_paren = Some(tok),
            SyntaxKind::R_PAREN => r_paren = Some(tok),
            _ => {}
        }
    }
    Some((l_paren?, r_paren?))
}

/// Parses parameter substrings out of a rendered signature's top-level
/// `(...)`, depth-tracking `<>`/`[]`/`()` so `Uint<0..8>` and `[A, B]` inside
/// a parameter's type don't split it. Empty parens (`()`) yield no params.
fn parse_params(label: &str) -> Vec<ParamLabel> {
    let Some((open, close)) = top_level_parens(label) else {
        return Vec::new();
    };
    let interior = &label[open..close];
    if interior.trim().is_empty() {
        return Vec::new();
    }
    split_top_level_commas(interior)
        .into_iter()
        .map(|(start, end)| {
            let seg = &interior[start..end];
            let trimmed_start = start + (seg.len() - seg.trim_start().len());
            let trimmed_end = end - (seg.len() - seg.trim_end().len());
            ParamLabel {
                start: (open + trimmed_start) as u32,
                end: (open + trimmed_end) as u32,
            }
        })
        .collect()
}

/// The byte range strictly INSIDE the first depth-0 `(...)` in `s`
/// (`(` and `)` not included), depth-tracking `<`/`[`/`(` together so a
/// generic parameter list (`<T>`) before the param list doesn't get mistaken
/// for it.
fn top_level_parens(s: &str) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let mut open = None;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' | '[' | '(' => {
                if ch == '(' && depth == 0 && open.is_none() {
                    open = Some(i + 1);
                }
                depth += 1;
            }
            '>' | ']' | ')' => {
                depth -= 1;
                if ch == ')'
                    && depth == 0
                    && let Some(open) = open
                {
                    return Some((open, i));
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits `s` on commas at depth 0 (depth-tracking `<>`/`[]`/`()`), returning
/// raw (untrimmed) byte ranges relative to `s`. Always yields at least one
/// range (the whole string) — callers filter out the empty-parens case
/// before calling this.
fn split_top_level_commas(s: &str) -> Vec<(usize, usize)> {
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut out = Vec::new();
    for (i, ch) in s.char_indices() {
        match ch {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push((start, i));
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push((start, s.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, FilePosition, fixture};

    fn sig_help_at(source: &str) -> Option<SignatureHelpData> {
        let (clean, offset) = fixture::extract(source);
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        signature_help(&mut host, FilePosition { file, offset })
    }

    #[test]
    fn active_param_advances_with_commas() {
        // cursor after the first comma -> active param index 1
        let d = sig_help_at(
            "circuit add(x: Field, y: Field): Field { return x + y; }\n\
             circuit c(): Field { return add(1, $0 2); }",
        )
        .unwrap();
        assert_eq!(d.label, "circuit add(x: Field, y: Field): Field");
        assert_eq!(d.params.len(), 2);
        assert_eq!(d.active_param, Some(1));
        // param labels index into the signature string
        assert_eq!(
            &d.label[d.params[0].start as usize..d.params[0].end as usize],
            "x: Field"
        );
        assert_eq!(
            &d.label[d.params[1].start as usize..d.params[1].end as usize],
            "y: Field"
        );
    }

    #[test]
    fn signature_help_absent_outside_a_call_is_none() {
        assert!(sig_help_at("circuit c(): Field { return 1$0; }").is_none());
    }

    #[test]
    fn active_param_before_first_comma_is_zero() {
        let d = sig_help_at(
            "circuit add(x: Field, y: Field): Field { return x + y; }\n\
             circuit c(): Field { return add($01, 2); }",
        )
        .unwrap();
        assert_eq!(d.active_param, Some(0));
    }

    #[test]
    fn nested_call_and_array_args_only_count_top_level_commas() {
        // `f(1,2)` and `[3,4]` each have their own commas; neither is a
        // top-level comma of the OUTER `add(...)` call. The cursor sits
        // right after the (single) top-level comma, so active param must be
        // 1 — NOT 3 (which a naive non-depth-tracking count would produce).
        let d = sig_help_at(
            "circuit f(a: Field, b: Field): Field { return a; }\n\
             circuit add(x: Field, y: Field): Field { return x; }\n\
             circuit c(): Field { return add(f(1,2), $0[3,4]); }",
        )
        .unwrap();
        assert_eq!(d.label, "circuit add(x: Field, y: Field): Field");
        assert_eq!(d.active_param, Some(1));
    }

    #[test]
    fn method_call_is_out_of_scope() {
        // `cnt.increment(|)`: a method call (DOT child) — v2c signature help
        // is a plain-call-only pure consumer; never guess a wrong signature.
        assert!(
            sig_help_at("export ledger cnt: Counter;\ncircuit f(): [] { cnt.increment($01); }")
                .is_none()
        );
    }

    #[test]
    fn unresolved_callee_is_none() {
        assert!(sig_help_at("circuit c(): Field { return mystery($01); }").is_none());
    }

    #[test]
    fn zero_param_signature_has_no_active_param() {
        let d = sig_help_at(
            "circuit zero(): Field { return 1; }\n\
             circuit c(): Field { return zero($0); }",
        )
        .unwrap();
        assert!(d.params.is_empty());
        assert_eq!(d.active_param, None);
    }

    #[test]
    fn generic_type_params_do_not_split_on_their_inner_comma_like_chars() {
        // `Uint<0..8>` contains no comma, but exercise a param type using
        // brackets (`[Field, Field]`) to prove depth-tracking in the LABEL
        // parse (not just the call-site comma count).
        let d = sig_help_at(
            "circuit pair(p: [Field, Field]): Field { return p[0]; }\n\
             circuit c(): Field { return pair($0[1, 2]); }",
        )
        .unwrap();
        assert_eq!(d.label, "circuit pair(p: [Field, Field]): Field");
        assert_eq!(d.params.len(), 1);
        assert_eq!(
            &d.label[d.params[0].start as usize..d.params[0].end as usize],
            "p: [Field, Field]"
        );
        assert_eq!(d.active_param, Some(0));
    }

    #[test]
    fn offset_past_eof_returns_none_and_does_not_panic() {
        let source = "circuit f(): Field { return xyz; }";
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let past_eof = FilePosition {
            file,
            offset: analyzer_core::TextSize::from(9_999),
        };
        assert!(signature_help(&mut host, past_eof).is_none());
    }
}
