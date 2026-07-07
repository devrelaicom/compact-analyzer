//! Eager workspace index: per-file symbol tables plus a forward/reverse
//! dependency graph over `import`/`include` edges. Rebuilt one file at a time
//! by `AnalysisHost::index_file`.

use std::collections::{BTreeMap, BTreeSet};

use crate::FileId;

#[derive(Default)]
pub(crate) struct WorkspaceIndex {
    files: BTreeSet<FileId>,
    /// Every named symbol per file: (name, symbol index into the ItemTree).
    symbols: BTreeMap<FileId, Vec<(String, u32)>>,
    /// file → files it imports/includes.
    forward: BTreeMap<FileId, BTreeSet<FileId>>,
    /// file → files that import/include it (dependents).
    reverse: BTreeMap<FileId, BTreeSet<FileId>>,
}

impl WorkspaceIndex {
    pub(crate) fn set_file(
        &mut self,
        file: FileId,
        symbols: Vec<(String, u32)>,
        deps: BTreeSet<FileId>,
    ) {
        self.files.insert(file);
        self.symbols.insert(file, symbols);
        // Detach stale reverse edges from the previous forward set.
        if let Some(old) = self.forward.get(&file) {
            for &d in old {
                if let Some(r) = self.reverse.get_mut(&d) {
                    r.remove(&file);
                }
            }
        }
        for &d in &deps {
            self.reverse.entry(d).or_default().insert(file);
        }
        self.forward.insert(file, deps);
    }

    pub(crate) fn remove(&mut self, file: FileId) {
        self.files.remove(&file);
        self.symbols.remove(&file);
        if let Some(old) = self.forward.remove(&file) {
            for d in old {
                if let Some(r) = self.reverse.get_mut(&d) {
                    r.remove(&file);
                }
            }
        }
        // Reverse edges *pointing at* `file` are intentionally kept so its
        // dependents get re-checked (their imports of it are now unresolved).
    }

    pub(crate) fn files(&self) -> Vec<FileId> {
        self.files.iter().copied().collect()
    }

    pub(crate) fn dependents(&self, file: FileId) -> Vec<FileId> {
        self.reverse
            .get(&file)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn symbols_matching(&self, query: &str) -> Vec<(FileId, u32)> {
        let mut out = Vec::new();
        for (&file, syms) in &self.symbols {
            for (name, idx) in syms {
                if subsequence_ci(query, name) {
                    out.push((file, *idx));
                }
            }
        }
        out
    }
}

/// Case-insensitive subsequence match: every char of `needle` appears in
/// `haystack` in order. An empty needle matches everything.
fn subsequence_ci(needle: &str, haystack: &str) -> bool {
    let mut hs = haystack.chars().flat_map(char::to_lowercase);
    'outer: for nc in needle.chars().flat_map(char::to_lowercase) {
        for hc in hs.by_ref() {
            if hc == nc {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::AnalysisHost;

    fn write(dir: &std::path::Path, rel: &str, contents: &str) -> std::path::PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn indexes_symbols_and_dependency_edges() {
        let dir = tempfile::tempdir().unwrap();
        let foo_p = write(
            dir.path(),
            "Foo.compact",
            "module Foo { export circuit ffff(): Field { return 0; } }",
        );
        let main_p = write(
            dir.path(),
            "main.compact",
            "import Foo;\ncircuit mmmm(): Field { return ffff(); }",
        );
        let mut host = AnalysisHost::new();
        let foo = host.vfs_mut().file_id(&foo_p);
        let main = host.vfs_mut().file_id(&main_p);
        host.index_file(foo);
        host.index_file(main);

        // Nested + top-level symbols are both indexed.
        assert!(
            host.workspace_symbols("ffff")
                .iter()
                .any(|&(f, _)| f == foo)
        );
        assert!(
            host.workspace_symbols("mmmm")
                .iter()
                .any(|&(f, _)| f == main)
        );
        // main imports Foo → Foo's dependents include main.
        assert!(host.dependents_of(foo).contains(&main));
    }

    #[test]
    fn eviction_removes_symbols_and_edges() {
        let dir = tempfile::tempdir().unwrap();
        let foo_p = write(
            dir.path(),
            "Foo.compact",
            "module Foo { export circuit ffff(): Field { return 0; } }",
        );
        let main_p = write(
            dir.path(),
            "main.compact",
            "import Foo;\ncircuit mmmm(): Field { return ffff(); }",
        );
        let mut host = AnalysisHost::new();
        let foo = host.vfs_mut().file_id(&foo_p);
        let main = host.vfs_mut().file_id(&main_p);
        host.index_file(foo);
        host.index_file(main);

        host.remove_workspace_file(main);
        assert!(host.workspace_files().contains(&foo));
        assert!(!host.workspace_files().contains(&main));
        assert!(!host.dependents_of(foo).contains(&main));
    }
}
