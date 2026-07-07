//! Mirror of the compiler's `find-source-pathname` (verified via
//! `compactc --trace-search`, 2026-07-07). `.compact` is ALWAYS appended;
//! the spec omits the extension. Search order: the importing/including file's
//! own directory first, then each import-search-path entry left to right.
//! Absolute specs are used exactly. Pure path logic — the only I/O is
//! `Path::exists`.

use std::path::{Path, PathBuf};

/// Resolves an import/include spec (`raw`, WITHOUT the `.compact` extension)
/// to an existing file. Absolute specs are used exactly; relative specs are
/// searched in `from_dir` first, then each `search_path` entry in order.
/// Returns the first candidate that exists on disk.
pub fn find_source_pathname(
    from_dir: &Path,
    search_path: &[PathBuf],
    raw: &str,
) -> Option<PathBuf> {
    let rel = format!("{raw}.compact");
    if Path::new(raw).is_absolute() {
        let cand = PathBuf::from(rel);
        return cand.exists().then_some(cand);
    }
    let first = from_dir.join(&rel);
    if first.exists() {
        return Some(first);
    }
    for entry in search_path {
        let cand = entry.join(&rel);
        if cand.exists() {
            return Some(cand);
        }
    }
    None
}

/// The text of a STRING_LIT token with its surrounding double quotes removed.
/// (compactp's `.text()` includes the quotes.) Tolerant if already unquoted.
pub fn string_lit_text(tok_text: &str) -> String {
    tok_text
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(tok_text)
        .to_string()
}

/// The module name a string-path import expects: the last path component,
/// ignoring any directory prefix. `None` for an empty spec.
pub fn path_module_name(raw: &str) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    (!last.is_empty()).then(|| last.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_compact_and_searches_from_dir_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let libs = root.join("libs");
        std::fs::create_dir_all(&libs).unwrap();
        std::fs::write(root.join("Foo.compact"), "module Foo {}").unwrap();
        std::fs::write(libs.join("Foo.compact"), "module Foo {}").unwrap();
        // Present in from_dir → found there; search path never consulted.
        let got = find_source_pathname(root, &[libs], "Foo").unwrap();
        assert_eq!(got, root.join("Foo.compact"));
    }

    #[test]
    fn falls_back_to_search_path_left_to_right() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("Qux.compact"), "module Qux {}").unwrap();
        // Not in from_dir, not in `a`, found in `b` (second search entry).
        let got = find_source_pathname(root, &[a, b.clone()], "Qux").unwrap();
        assert_eq!(got, b.join("Qux.compact"));
    }

    #[test]
    fn resolves_subdir_string_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/Bar.compact"), "module Bar {}").unwrap();
        let got = find_source_pathname(root, &[], "sub/Bar").unwrap();
        assert_eq!(got, root.join("sub/Bar.compact"));
    }

    #[test]
    fn absolute_spec_used_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Abs.compact"), "module Abs {}").unwrap();
        let abs_no_ext = root.join("Abs"); // absolute, no extension
        let got =
            find_source_pathname(Path::new("/nowhere"), &[], abs_no_ext.to_str().unwrap()).unwrap();
        assert_eq!(got, root.join("Abs.compact"));
    }

    #[test]
    fn returns_none_when_unresolvable() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_source_pathname(dir.path(), &[], "Missing").is_none());
    }

    #[test]
    fn string_lit_text_strips_quotes() {
        assert_eq!(string_lit_text("\"sub/Bar\""), "sub/Bar");
        assert_eq!(string_lit_text("sub/Bar"), "sub/Bar"); // tolerant if unquoted
    }

    #[test]
    fn module_name_is_last_component() {
        assert_eq!(path_module_name("sub/Bar").as_deref(), Some("Bar"));
        assert_eq!(path_module_name("Bar").as_deref(), Some("Bar"));
        assert_eq!(path_module_name("a/b/c").as_deref(), Some("c"));
        assert_eq!(path_module_name("").as_deref(), None);
    }
}
