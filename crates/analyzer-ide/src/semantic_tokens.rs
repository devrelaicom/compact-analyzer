//! Comprehensive, full-document semantic-token classification from the CST.

use analyzer_core::{
    AnalysisHost, Definition, FileId, FilePosition, SymbolKind, SyntaxKind, SyntaxNode,
    SyntaxToken, TextRange,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    Keyword,
    Type,
    Struct,
    Enum,
    EnumMember,
    TypeParameter,
    Parameter,
    Variable,
    Property,
    Function,
    Method,
    Namespace,
    Comment,
    StringLit,
    Number,
    Operator,
    Punctuation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenMods {
    pub declaration: bool,
    pub default_library: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemToken {
    pub range: TextRange,
    pub ty: TokenType,
    pub mods: TokenMods,
}

/// All non-whitespace tokens classified, in document order.
pub fn semantic_tokens(host: &mut AnalysisHost, file: FileId) -> Vec<SemToken> {
    let root = match host.analyze(file) {
        Some(a) => SyntaxNode::new_root(a.green.clone()),
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for tok in root
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
    {
        let Some((ty, mods)) = classify_token(host, file, &tok) else {
            continue;
        };
        out.push(SemToken {
            range: tok.text_range(),
            ty,
            mods,
        });
    }
    out
}

fn classify_token(
    host: &mut AnalysisHost,
    file: FileId,
    tok: &SyntaxToken,
) -> Option<(TokenType, TokenMods)> {
    use SyntaxKind::*;
    let mods = TokenMods::default();
    let ty = match tok.kind() {
        WHITESPACE | ERROR | EOF => return None,
        LINE_COMMENT | BLOCK_COMMENT => TokenType::Comment,
        STRING_LIT => TokenType::StringLit,
        INT_LIT | HEX_LIT | OCT_LIT | BIN_LIT | VERSION_LIT => TokenType::Number,
        BOOLEAN_KW | FIELD_KW | UINT_KW | BYTES_KW | OPAQUE_KW | VECTOR_KW | UNSIGNED_KW
        | INTEGER_KW => TokenType::Type,
        // `<` / `>` are structurally ambiguous (type-argument brackets vs
        // relational operators); classify by parent kind. Must precede the
        // generic `is_operator` check, which no longer matches LT/GT.
        LT | GT => classify_angle_bracket(tok),
        k if is_keyword(k) => TokenType::Keyword,
        k if is_operator(k) => TokenType::Operator,
        k if is_punct(k) => TokenType::Punctuation,
        IDENT => return Some(classify_ident(host, file, tok)),
        _ => return None,
    };
    Some((ty, mods))
}

/// Classify a `<` / `>` token by structural context (spec §5): the bracket of a
/// type-argument list, sized-builtin type, generic-parameter list, or
/// `default<...>` expression is punctuation; anything else is a relational
/// operator. Verified against the CST at `compactp` 0.1.0-beta.1: the LT/GT
/// tokens are *direct children* of the delimiter node in every type context
/// (`BYTES_TYPE`/`UINT_TYPE`/`VECTOR_TYPE`/`OPAQUE_TYPE` for sized builtins,
/// `GENERIC_ARG_LIST` for use-site type args and generic calls,
/// `GENERIC_PARAM_LIST` for declaration-site params, `DEFAULT_EXPR` for
/// `default<T>`) and of `BINARY_EXPR` when relational. Unknown parents fall
/// back to `Operator` (the pre-fix behaviour) to avoid over-broad
/// reclassification.
fn classify_angle_bracket(tok: &SyntaxToken) -> TokenType {
    use SyntaxKind::*;
    match tok.parent().map(|p| p.kind()) {
        Some(
            BYTES_TYPE | UINT_TYPE | VECTOR_TYPE | OPAQUE_TYPE | GENERIC_ARG_LIST
            | GENERIC_PARAM_LIST | DEFAULT_EXPR,
        ) => TokenType::Punctuation,
        _ => TokenType::Operator,
    }
}

fn is_keyword(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        PRAGMA_KW
            | INCLUDE_KW
            | IMPORT_KW
            | FROM_KW
            | PREFIX_KW
            | EXPORT_KW
            | MODULE_KW
            | LEDGER_KW
            | CONSTRUCTOR_KW
            | CIRCUIT_KW
            | WITNESS_KW
            | CONTRACT_KW
            | STRUCT_KW
            | ENUM_KW
            | TYPE_KW
            | CONST_KW
            | RETURN_KW
            | IF_KW
            | ELSE_KW
            | FOR_KW
            | OF_KW
            | ASSERT_KW
            | AS_KW
            | PURE_KW
            | SEALED_KW
            | NEW_KW
            | MAP_KW
            | FOLD_KW
            | DEFAULT_KW
            | DISCLOSE_KW
            | PAD_KW
            | SLICE_KW
            | TRUE_KW
            | FALSE_KW
    )
}

fn is_operator(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        // NB: LT/GT are handled by `classify_angle_bracket` (structural), not
        // here. LT_EQ/GT_EQ (`<=`/`>=`) are always relational, never brackets.
        // DOT is punctuation (member access); DOT_DOT/DOT_DOT_DOT (range/spread)
        // remain operators.
        EQ | PLUS_EQ
            | MINUS_EQ
            | EQ_EQ
            | BANG_EQ
            | LT_EQ
            | GT_EQ
            | AMP_AMP
            | PIPE_PIPE
            | PLUS
            | MINUS
            | STAR
            | SLASH
            | BANG
            | QUESTION
            | FAT_ARROW
            | DOT_DOT
            | DOT_DOT_DOT
    )
}

fn is_punct(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        L_PAREN
            | R_PAREN
            | L_BRACE
            | R_BRACE
            | L_BRACKET
            | R_BRACKET
            | COMMA
            | SEMICOLON
            | COLON
            | HASH
            | DOT
    )
}

/// Classify an IDENT by its parent kind, resolving use-site names for refinement.
fn classify_ident(
    host: &mut AnalysisHost,
    file: FileId,
    tok: &SyntaxToken,
) -> (TokenType, TokenMods) {
    use SyntaxKind::*;
    let mut mods = TokenMods::default();
    let Some(parent) = tok.parent() else {
        return (TokenType::Variable, mods);
    };
    match parent.kind() {
        CIRCUIT_DEF | CIRCUIT_DECL | WITNESS_DECL => {
            mods.declaration = true;
            (TokenType::Function, mods)
        }
        CONTRACT_CIRCUIT => {
            mods.declaration = true;
            (TokenType::Method, mods)
        }
        STRUCT_DEF | CONTRACT_DECL => {
            mods.declaration = true;
            (TokenType::Struct, mods)
        }
        ENUM_DEF => {
            mods.declaration = true;
            (TokenType::Enum, mods)
        }
        MODULE_DEF => {
            mods.declaration = true;
            (TokenType::Namespace, mods)
        }
        TYPE_DECL => {
            mods.declaration = true;
            (TokenType::Type, mods)
        }
        LEDGER_DECL | STRUCT_FIELD => {
            mods.declaration = true;
            (TokenType::Property, mods)
        }
        ENUM_VARIANT => {
            mods.declaration = true;
            (TokenType::EnumMember, mods)
        }
        GENERIC_PARAM => {
            mods.declaration = true;
            (TokenType::TypeParameter, mods)
        }
        FOR_STMT => {
            mods.declaration = true;
            (TokenType::Variable, mods)
        }
        TYPE_REF => (TokenType::Type, mods),
        STRUCT_EXPR => (TokenType::Struct, mods),
        STRUCT_FIELD_INIT | MEMBER_EXPR => (TokenType::Property, mods),
        PARAM => {
            mods.declaration = true;
            (TokenType::Parameter, mods)
        }
        IDENT_PAT => {
            mods.declaration = true;
            let is_param = parent.ancestors().any(|a| a.kind() == PARAM);
            (
                if is_param {
                    TokenType::Parameter
                } else {
                    TokenType::Variable
                },
                mods,
            )
        }
        IMPORT | IMPORT_SPECIFIER | PREFIX_DECL | PRAGMA => (TokenType::Namespace, mods),
        NAME_EXPR => classify_use_site(host, file, tok, TokenType::Variable),
        CALL_EXPR => {
            // A DOT before the IDENT ⇒ method name; else a direct callee (F4).
            let dot_before = parent
                .children_with_tokens()
                .filter_map(rowan::NodeOrToken::into_token)
                .any(|t| t.kind() == DOT && t.text_range().end() <= tok.text_range().start());
            if dot_before {
                (TokenType::Method, mods)
            } else {
                classify_use_site(host, file, tok, TokenType::Function)
            }
        }
        _ => (TokenType::Variable, mods),
    }
}

/// Resolve a use-site identifier to refine its token type + `default_library`.
fn classify_use_site(
    host: &mut AnalysisHost,
    file: FileId,
    tok: &SyntaxToken,
    fallback: TokenType,
) -> (TokenType, TokenMods) {
    let mut mods = TokenMods::default();
    let pos = FilePosition {
        file,
        offset: tok.text_range().start(),
    };
    let Some(def) = host.resolve(pos) else {
        return (fallback, mods);
    };
    if let Definition::Item { file: def_file, .. } = &def
        && host.stdlib_file() == Some(*def_file)
    {
        mods.default_library = true;
    }
    let ty = match &def {
        Definition::Local { detail, .. } => {
            if detail.starts_with("generic ") {
                TokenType::TypeParameter
            } else if detail.contains(": ")
                && !detail.starts_with("const ")
                && !detail.starts_with("for ")
            {
                TokenType::Parameter
            } else {
                TokenType::Variable
            }
        }
        Definition::Item {
            file: def_file,
            index,
        } => {
            match host
                .analyze(*def_file)
                .and_then(|a| a.item_tree.symbols.get(*index as usize).map(|s| s.kind))
            {
                Some(SymbolKind::Circuit)
                | Some(SymbolKind::CircuitSig)
                | Some(SymbolKind::Witness) => TokenType::Function,
                Some(SymbolKind::Ledger) => TokenType::Property,
                Some(SymbolKind::Struct) => TokenType::Struct,
                Some(SymbolKind::Enum) => TokenType::Enum,
                Some(SymbolKind::Module) => TokenType::Namespace,
                Some(SymbolKind::TypeAlias) => TokenType::Type,
                _ => fallback,
            }
        }
    };
    (ty, mods)
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::AnalysisHost;

    fn toks(source: &str) -> Vec<(String, TokenType)> {
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let text = source.to_string();
        semantic_tokens(&mut host, file)
            .into_iter()
            .map(|t| (text[t.range].to_string(), t.ty))
            .collect()
    }

    fn ty_of<'a>(v: &'a [(String, TokenType)], text: &str) -> Option<&'a TokenType> {
        v.iter().find(|(t, _)| t == text).map(|(_, k)| k)
    }

    #[test]
    fn classifies_declaration_and_types() {
        let v = toks("export circuit inc(x: Field): Field { return x + 1; }");
        assert_eq!(ty_of(&v, "export"), Some(&TokenType::Keyword));
        assert_eq!(ty_of(&v, "circuit"), Some(&TokenType::Keyword));
        assert_eq!(ty_of(&v, "inc"), Some(&TokenType::Function));
        assert_eq!(ty_of(&v, "x"), Some(&TokenType::Parameter));
        assert_eq!(ty_of(&v, "Field"), Some(&TokenType::Type));
        assert_eq!(ty_of(&v, "+"), Some(&TokenType::Operator));
        assert_eq!(ty_of(&v, "1"), Some(&TokenType::Number));
        assert_eq!(ty_of(&v, "("), Some(&TokenType::Punctuation));
        assert_eq!(ty_of(&v, "return"), Some(&TokenType::Keyword));
    }

    #[test]
    fn classifies_ledger_and_calls() {
        let v =
            toks("export ledger cnt: Counter;\ncircuit f(): [] { helper(); cnt.increment(1); }");
        assert_eq!(ty_of(&v, "cnt"), Some(&TokenType::Property)); // ledger field decl + use
        assert_eq!(ty_of(&v, "helper"), Some(&TokenType::Function)); // direct callee (unresolved → Function)
        assert_eq!(ty_of(&v, "increment"), Some(&TokenType::Method)); // method after dot
    }

    #[test]
    fn declaration_modifier_and_comment() {
        let mut host = AnalysisHost::new();
        let src = "// hi\ncircuit f(x: Field): [] { const y = 1; }";
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/m.compact"));
        host.vfs_mut().set_overlay(file, src.to_string(), 1);
        let ts = semantic_tokens(&mut host, file);
        let f = ts.iter().find(|t| &src[t.range] == "f").unwrap();
        assert_eq!(f.ty, TokenType::Function);
        assert!(f.mods.declaration);
        assert!(ts.iter().any(|t| t.ty == TokenType::Comment));
        // CR3-M1: a param's and a `const` local's definition-site name token
        // also carry `declaration == true` — every other def-site arm
        // (FOR_STMT, GENERIC_PARAM, item defs, LEDGER_DECL/STRUCT_FIELD,
        // ENUM_VARIANT) already did; PARAM/IDENT_PAT were the odd ones out.
        let x = ts.iter().find(|t| &src[t.range] == "x").unwrap();
        assert_eq!(x.ty, TokenType::Parameter);
        assert!(x.mods.declaration);
        let y = ts.iter().find(|t| &src[t.range] == "y").unwrap();
        assert_eq!(y.ty, TokenType::Variable);
        assert!(y.mods.declaration);
    }

    // ---- Fix #2: member-access `.` is punctuation (spec §5) ----

    #[test]
    fn member_access_dot_is_punctuation() {
        // The `.` in `cnt.increment` delimits a member access — it is
        // punctuation, not an operator.
        let v = toks("circuit f(): [] { cnt.increment(1); }");
        assert_eq!(ty_of(&v, "."), Some(&TokenType::Punctuation));
    }

    // ---- Fix #1: `<` / `>` classified by structural context (spec §5) ----

    #[test]
    fn sized_type_angle_brackets_are_punctuation() {
        // `Bytes<32>`: both brackets are children of BYTES_TYPE → punctuation.
        // (Verified against the CST: the LT/GT tokens are direct children of
        // the type node, not of TYPE_SIZE, which only wraps the size expr.)
        let v = toks("circuit f(x: Bytes<32>): [] { }");
        assert_eq!(ty_of(&v, "<"), Some(&TokenType::Punctuation));
        assert_eq!(ty_of(&v, ">"), Some(&TokenType::Punctuation));
    }

    #[test]
    fn generic_type_arg_angle_brackets_are_punctuation() {
        // `Vector<3, Field>`: brackets are children of VECTOR_TYPE.
        let v = toks("circuit f(): Vector<3, Field> { return default<Vector<3, Field>>; }");
        assert_eq!(ty_of(&v, "<"), Some(&TokenType::Punctuation));
        assert_eq!(ty_of(&v, ">"), Some(&TokenType::Punctuation));
    }

    #[test]
    fn generic_param_list_angle_brackets_are_punctuation() {
        // `struct S<T>`: brackets are children of GENERIC_PARAM_LIST.
        let v = toks("struct S<T> { v: T; }");
        assert_eq!(ty_of(&v, "<"), Some(&TokenType::Punctuation));
        assert_eq!(ty_of(&v, ">"), Some(&TokenType::Punctuation));
    }

    #[test]
    fn relational_angle_brackets_are_operators() {
        // A relational `<` / `>` in an expression stays an operator. This guards
        // against over-broad reclassification of every angle bracket.
        let v = toks("circuit f(a: Field, b: Field): Boolean { return a < b || a > b; }");
        assert_eq!(ty_of(&v, "<"), Some(&TokenType::Operator));
        assert_eq!(ty_of(&v, ">"), Some(&TokenType::Operator));
    }

    // ---- Fix #3: the resolution-refinement path (`classify_use_site`) ----

    #[test]
    fn use_site_ledger_reference_resolves_to_property() {
        // Guards the RESOLVED branch of `classify_use_site`: the *use-site*
        // `cnt` (a bare NAME_EXPR receiver) is not classified structurally —
        // it must resolve to the ledger declaration and be refined to Property
        // with no `declaration` modifier. If `classify_use_site` were replaced
        // with `return fallback` (Variable), this would fail. `ty_of` only
        // returns the first (decl-site) match, so we inspect both occurrences.
        let mut host = AnalysisHost::new();
        let src = "export ledger cnt: Counter;\ncircuit f(): [] { cnt.increment(1); }";
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/u.compact"));
        host.vfs_mut().set_overlay(file, src.to_string(), 1);
        let ts = semantic_tokens(&mut host, file);
        let cnts: Vec<_> = ts.iter().filter(|t| &src[t.range] == "cnt").collect();
        assert_eq!(cnts.len(), 2, "expected decl + use occurrences of cnt");
        // Declaration site (structural).
        assert_eq!(cnts[0].ty, TokenType::Property);
        assert!(cnts[0].mods.declaration);
        // Use site (resolved via classify_use_site → SymbolKind::Ledger).
        assert_eq!(cnts[1].ty, TokenType::Property);
        assert!(!cnts[1].mods.declaration);
    }

    #[test]
    fn use_site_param_reference_resolves_to_parameter() {
        // CR3-M4: E6 (above) only covered the resolved-Item branches of
        // `classify_use_site`; this covers the resolved-`Definition::Local`
        // heuristic — `resolve.rs`'s detail string `"x: Field"` (contains
        // `": "`, doesn't start with `"const "`/`"for "`) must be read back
        // as `Parameter`. A drift in `resolve.rs`'s detail-string format
        // (e.g. dropping the `": "` separator, or a `"const "` prefix
        // collision) would silently regress this to `Variable` without a
        // guard here.
        let mut host = AnalysisHost::new();
        let src = "circuit f(x: Field): Field { return x + x; }";
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/p.compact"));
        host.vfs_mut().set_overlay(file, src.to_string(), 1);
        let ts = semantic_tokens(&mut host, file);
        let xs: Vec<_> = ts.iter().filter(|t| &src[t.range] == "x").collect();
        assert_eq!(xs.len(), 3, "expected decl + two use occurrences of x");
        // Declaration site (structural, IDENT_PAT arm under PARAM).
        assert_eq!(xs[0].ty, TokenType::Parameter);
        assert!(xs[0].mods.declaration);
        // Use sites (resolved via classify_use_site → Definition::Local).
        assert_eq!(xs[1].ty, TokenType::Parameter);
        assert!(!xs[1].mods.declaration);
        assert_eq!(xs[2].ty, TokenType::Parameter);
        assert!(!xs[2].mods.declaration);
    }

    #[test]
    fn stdlib_use_site_sets_default_library() {
        // Guards the `default_library` modifier + the Item resolution branch:
        // a use-site call to a stdlib circuit must resolve into the registered
        // stub file → Function + default_library. Mirrors Task 4's stdlib
        // fixture setup (tempdir + materialize + register_stdlib).
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let src = "import CompactStandardLibrary;\n\
                   circuit f(x: Field): Bytes<32> { return persistentHash<Field>(x); }";
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, src.to_string(), 1);
        let ts = semantic_tokens(&mut host, file);
        let ph = ts
            .iter()
            .find(|t| &src[t.range] == "persistentHash")
            .unwrap();
        assert_eq!(ph.ty, TokenType::Function);
        assert!(ph.mods.default_library);
    }
}
