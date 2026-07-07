//! Find-references across the workspace. Each candidate IDENT with matching
//! text is re-resolved and kept only if it resolves to the same definition —
//! shadowing-aware and cross-file. Local bindings stay single-file.

use analyzer_core::{AnalysisHost, Definition, FilePosition, SyntaxKind, SyntaxNode};

use crate::NavTarget;

/// All references to the definition at `pos`, workspace-wide. `should_continue`
/// is polled per file; returning `false` abandons the scan (answers `None`).
pub fn find_references_cancellable(
    host: &mut AnalysisHost,
    pos: FilePosition,
    include_declaration: bool,
    should_continue: &dyn Fn() -> bool,
) -> Option<Vec<NavTarget>> {
    let def = host.resolve(pos)?;
    let name = host.def_name(&def)?;
    let (def_file, def_name_range, _) = host.nav_info(&def)?;

    // Local bindings are file-local; items are workspace-wide.
    let files: Vec<_> = if matches!(def, Definition::Local { .. }) {
        vec![pos.file]
    } else {
        let mut v = host.workspace_files();
        v.push(pos.file);
        v.push(def_file);
        v.sort();
        v.dedup();
        v
    };

    let mut out = Vec::new();
    for file in files {
        if !should_continue() {
            return None;
        }
        // Cheap name-presence prune: skip files that don't mention the name.
        let Some(text) = host.vfs_mut().read(file) else {
            continue;
        };
        if !text.contains(name.as_str()) {
            continue;
        }
        let Some(analysis) = host.analyze(file) else {
            continue;
        };
        let root = SyntaxNode::new_root(analysis.green.clone());
        let candidates: Vec<_> = root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == SyntaxKind::IDENT && t.text() == name)
            .map(|t| t.text_range())
            .collect();
        for range in candidates {
            let is_declaration = file == def_file && range == def_name_range;
            if is_declaration && !include_declaration {
                continue;
            }
            let resolved = host.resolve(FilePosition {
                file,
                offset: range.start(),
            });
            if resolved.as_ref() == Some(&def) {
                out.push(NavTarget {
                    file,
                    name_range: range,
                    full_range: range,
                });
            }
        }
    }
    out.sort_by_key(|t| (t.file, t.name_range.start()));
    out.dedup();
    Some(out)
}

/// Non-cancellable convenience wrapper (used by tests and simple callers).
pub fn find_references(
    host: &mut AnalysisHost,
    pos: FilePosition,
    include_declaration: bool,
) -> Option<Vec<NavTarget>> {
    find_references_cancellable(host, pos, include_declaration, &|| true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, FilePosition, fixture};

    fn refs(source: &str, include_decl: bool) -> Vec<String> {
        let (clean, offset) = fixture::extract(source);
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean.clone(), 1);
        let targets = find_references(&mut host, FilePosition { file, offset }, include_decl)
            .unwrap_or_default();
        targets
            .into_iter()
            .map(|t| format!("{:?}", t.name_range))
            .collect()
    }

    #[test]
    fn finds_all_uses_of_a_param() {
        let source =
            "circuit f(ba$0se: Field): Field {\n  const a = base + base;\n  return base;\n}";
        assert_eq!(refs(source, false).len(), 3);
        assert_eq!(refs(source, true).len(), 4); // + declaration
    }

    #[test]
    fn shadowed_bindings_are_distinct() {
        // References of the FIRST x must not include uses of the second.
        let source = "circuit f(): Field {\n  const x$0 = 1;\n  const y = x;\n  const x = 2;\n  return x;\n}";
        assert_eq!(refs(source, false).len(), 1); // only `const y = x;`
    }

    #[test]
    fn item_references_from_use_site() {
        let source = "circuit helper(): Field { return 1; }\n\
                      circuit a(): Field { return helper(); }\n\
                      circuit b(): Field { return hel$0per(); }";
        assert_eq!(refs(source, false).len(), 2);
        assert_eq!(refs(source, true).len(), 3);
    }

    #[test]
    fn no_references_for_unresolvable() {
        assert!(refs("circuit f(): Field { return myst$0ery; }", false).is_empty());
    }

    #[test]
    fn cancellation_abandons_the_scan() {
        // A resolvable symbol, but `should_continue` returns false before the
        // first file is scanned — the scan must abandon and answer `None`,
        // not an (incomplete) `Some(..)`.
        let source =
            "circuit f(ba$0se: Field): Field {\n  const a = base + base;\n  return base;\n}";
        let (clean, offset) = fixture::extract(source);
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        let result =
            find_references_cancellable(&mut host, FilePosition { file, offset }, false, &|| false);
        assert_eq!(result, None);
    }

    use analyzer_core::TextSize;

    #[test]
    fn references_span_the_workspace() {
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
        let off = foo_text.find("ff(").unwrap() as u32; // the `ff` declaration
        let refs = find_references(
            &mut host,
            FilePosition {
                file: foo,
                offset: TextSize::new(off),
            },
            true,
        )
        .unwrap();

        assert_eq!(refs.len(), 3); // declaration in Foo + two uses in main
        assert_eq!(refs.iter().filter(|r| r.file == main).count(), 2);
    }
}
