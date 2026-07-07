//! Workspace symbols: query the core symbol index and enrich each hit with its
//! kind, name range, and container (parent) name. User code only — the bundled
//! stdlib is excluded by the core index.

use analyzer_core::{AnalysisHost, FileId, SymbolKind, TextRange};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSymbolItem {
    pub file: FileId,
    pub name: String,
    pub kind: SymbolKind,
    pub name_range: TextRange,
    pub container: Option<String>,
}

/// Symbols whose name subsequence-matches `query` (case-insensitive), across
/// the whole workspace index.
pub fn workspace_symbols(host: &mut AnalysisHost, query: &str) -> Vec<WorkspaceSymbolItem> {
    let hits = host.workspace_symbols(query);
    let mut out = Vec::new();
    for (file, idx) in hits {
        let Some(analysis) = host.analyze(file) else {
            continue;
        };
        let Some(sym) = analysis.item_tree.symbols.get(idx as usize) else {
            continue;
        };
        let container = sym
            .parent
            .and_then(|p| analysis.item_tree.symbols.get(p as usize))
            .map(|s| s.name.clone());
        out.push(WorkspaceSymbolItem {
            file,
            name: sym.name.clone(),
            kind: sym.kind,
            name_range: sym.name_range,
            container,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_symbols_by_subsequence_across_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.compact"),
            "circuit transfer(): Field { return 0; }",
        )
        .unwrap();
        std::fs::write(dir.path().join("b.compact"), "struct Holder { x: Field; }").unwrap();
        let mut host = AnalysisHost::new();
        host.discover_and_index(&[dir.path().to_path_buf()], &|| true);

        let hits = workspace_symbols(&mut host, "trns"); // subsequence of "transfer"
        assert!(hits.iter().any(|s| s.name == "transfer"));
        // A struct field carries its struct as the container.
        let field = workspace_symbols(&mut host, "x")
            .into_iter()
            .find(|s| s.name == "x" && s.kind == SymbolKind::StructField);
        assert_eq!(field.and_then(|s| s.container).as_deref(), Some("Holder"));
    }
}
