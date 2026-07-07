//! Rename: validate the new name, gather all references workspace-wide via
//! `find_references_cancellable`, then run a per-use-site conflict check — at
//! every reference site the new name must resolve to nothing or to the same
//! definition. Edits span every file that references the symbol.

use analyzer_core::{AnalysisHost, FilePosition, TextRange};

use crate::find_references_cancellable;

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

pub fn rename_cancellable(
    host: &mut AnalysisHost,
    pos: FilePosition,
    new_name: &str,
    should_continue: &dyn Fn() -> bool,
) -> Result<Vec<SourceEdit>, RenameError> {
    if !is_valid_identifier(new_name) {
        return Err(RenameError::InvalidName(new_name.to_string()));
    }
    if KEYWORDS.contains(&new_name) {
        return Err(RenameError::Keyword(new_name.to_string()));
    }
    let def = host.resolve(pos).ok_or(RenameError::NotFound)?;
    let (def_file, _, _) = host.nav_info(&def).ok_or(RenameError::NotFound)?;
    if Some(def_file) == host.stdlib_file() {
        return Err(RenameError::BuiltinTarget);
    }
    let refs = find_references_cancellable(host, pos, true, should_continue)
        .ok_or(RenameError::NotFound)?;

    // Per-use-site conflict detection: at each reference site, the new name
    // must not already resolve to a *different* definition (which the rename
    // would shadow or collide with). Works across files.
    for r in &refs {
        if let Some(existing) = host.resolve_name_at(
            FilePosition {
                file: r.file,
                offset: r.name_range.start(),
            },
            new_name,
        ) && existing != def
        {
            return Err(RenameError::Conflict(new_name.to_string()));
        }
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

/// Non-cancellable convenience wrapper (used by tests and simple callers).
pub fn rename(
    host: &mut AnalysisHost,
    pos: FilePosition,
    new_name: &str,
) -> Result<Vec<SourceEdit>, RenameError> {
    rename_cancellable(host, pos, new_name, &|| true)
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
    fn renames_across_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Foo.compact"),
            "module Foo { export circuit ff(): Field { return 0; } }",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("main.compact"),
            "import Foo;\ncircuit m(): Field { return ff() + ff(); }",
        )
        .unwrap();
        let mut host = AnalysisHost::new();
        host.discover_and_index(&[dir.path().to_path_buf()], &|| true);
        let foo = host.vfs_mut().file_id(&dir.path().join("Foo.compact"));
        let main = host.vfs_mut().file_id(&dir.path().join("main.compact"));
        let foo_text = host.vfs_mut().read(foo).unwrap();
        let off = foo_text.find("ff(").unwrap() as u32;

        let edits = rename(
            &mut host,
            FilePosition {
                file: foo,
                offset: analyzer_core::TextSize::new(off),
            },
            "gg",
        )
        .unwrap();
        assert_eq!(edits.len(), 3); // declaration + two cross-file uses
        assert_eq!(edits.iter().filter(|e| e.file == main).count(), 2);
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
