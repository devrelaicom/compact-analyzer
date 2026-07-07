//! Rename: validate the new name, then reuse `find_references` to build
//! the edit set. The conflict check is a single scope-resolution probe
//! anchored at the definition's *last* reference (not its declaration
//! site), because an earlier anchor can miss a sibling binding declared
//! later in the same scope. It is still one check, not a per-use-site
//! scan; full per-use-site conflict detection is deferred to M2b.

use analyzer_core::{AnalysisHost, FilePosition, TextRange};

use crate::find_references;

/// Active Compact keywords at language 0.23 (from the language reference).
const KEYWORDS: &[&str] = &[
    "as",
    "assert",
    "Boolean",
    "Bytes",
    "circuit",
    "const",
    "constructor",
    "contract",
    "default",
    "disclose",
    "else",
    "enum",
    "export",
    "false",
    "Field",
    "fold",
    "for",
    "from",
    "if",
    "import",
    "include",
    "ledger",
    "map",
    "module",
    "new",
    "of",
    "Opaque",
    "pad",
    "pragma",
    "prefix",
    "pure",
    "return",
    "sealed",
    "slice",
    "struct",
    "true",
    "type",
    "Uint",
    "Vector",
    "witness",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdit {
    pub file: analyzer_core::FileId,
    pub range: TextRange,
    pub new_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameError {
    NotFound,
    InvalidName(String),
    Keyword(String),
    BuiltinTarget,
    Conflict(String),
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameError::NotFound => write!(f, "nothing renameable at the cursor"),
            RenameError::InvalidName(n) => write!(f, "`{n}` is not a valid identifier"),
            RenameError::Keyword(n) => write!(f, "`{n}` is a Compact keyword"),
            RenameError::BuiltinTarget => write!(f, "cannot rename a standard-library item"),
            RenameError::Conflict(n) => {
                write!(f, "`{n}` already resolves in the definition's scope")
            }
        }
    }
}

pub fn rename(
    host: &mut AnalysisHost,
    pos: FilePosition,
    new_name: &str,
) -> Result<Vec<SourceEdit>, RenameError> {
    if !is_valid_identifier(new_name) {
        return Err(RenameError::InvalidName(new_name.to_string()));
    }
    if KEYWORDS.contains(&new_name) {
        return Err(RenameError::Keyword(new_name.to_string()));
    }
    let def = host.resolve(pos).ok_or(RenameError::NotFound)?;
    let (def_file, def_name_range, _) = host.nav_info(&def).ok_or(RenameError::NotFound)?;
    if Some(def_file) == host.stdlib_file() {
        return Err(RenameError::BuiltinTarget);
    }
    let refs = find_references(host, pos, true).ok_or(RenameError::NotFound)?;
    // Conservative conflict check: the new name must not already resolve
    // within the definition's own scope. Anchored at the *last* reference
    // rather than the definition's own name-range start: `resolve_name_at`'s
    // local-scope scan is sequential/shadowing-aware (a later `const` in the
    // same block shadows an earlier one only from its own declaration
    // onward), so a check anchored at the very start of a binding's
    // declaration can never see a sibling declared later in the same block
    // — even though that sibling is genuinely in scope by the time the
    // rename's uses are reached. This is still a single check (not a
    // per-use-site scan); per-use-site conflicts remain a known M2b
    // refinement.
    let check_pos = refs
        .iter()
        .map(|t| t.name_range.start())
        .max()
        .unwrap_or_else(|| def_name_range.start());
    if host
        .resolve_name_at(
            FilePosition {
                file: def_file,
                offset: check_pos,
            },
            new_name,
        )
        .is_some()
    {
        return Err(RenameError::Conflict(new_name.to_string()));
    }
    Ok(refs
        .into_iter()
        .map(|t| SourceEdit {
            file: t.file,
            range: t.name_range,
            new_text: new_name.to_string(),
        })
        .collect())
}

fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, FilePosition, fixture};

    fn try_rename(source: &str, new_name: &str) -> Result<String, RenameError> {
        let (clean, offset) = fixture::extract(source);
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean.clone(), 1);
        let edits = rename(&mut host, FilePosition { file, offset }, new_name)?;
        // Apply edits (back to front) and return the new text.
        let mut text = clean;
        let mut sorted = edits;
        sorted.sort_by_key(|e| std::cmp::Reverse(u32::from(e.range.start())));
        for edit in sorted {
            let start = u32::from(edit.range.start()) as usize;
            let end = u32::from(edit.range.end()) as usize;
            text.replace_range(start..end, &edit.new_text);
        }
        Ok(text)
    }

    #[test]
    fn renames_definition_and_all_uses() {
        let result = try_rename(
            "circuit f(ba$0se: Field): Field { return base + base; }",
            "input",
        )
        .unwrap();
        assert_eq!(
            result,
            "circuit f(input: Field): Field { return input + input; }"
        );
    }

    #[test]
    fn refuses_keywords_and_invalid_names() {
        let src = "circuit f(ba$0se: Field): Field { return base; }";
        assert!(matches!(
            try_rename(src, "circuit"),
            Err(RenameError::Keyword(_))
        ));
        assert!(matches!(
            try_rename(src, "9lives"),
            Err(RenameError::InvalidName(_))
        ));
        assert!(matches!(
            try_rename(src, "has space"),
            Err(RenameError::InvalidName(_))
        ));
    }

    #[test]
    fn refuses_conflicting_target_name() {
        let src = "circuit f(): Field {\n  const a$0 = 1;\n  const b = 2;\n  return a + b;\n}";
        assert!(matches!(
            try_rename(src, "b"),
            Err(RenameError::Conflict(_))
        ));
    }

    #[test]
    fn refuses_stdlib_targets() {
        let (clean, offset) = fixture::extract(
            "import CompactStandardLibrary;\ncircuit m(x: Field): Bytes<32> { return persistentHa$0sh<Field>(x); }",
        );
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        assert!(matches!(
            rename(&mut host, FilePosition { file, offset }, "myHash"),
            Err(RenameError::BuiltinTarget)
        ));
    }
}
