//! Document symbols: a nested outline built from the `ItemTree` hierarchy.

use analyzer_core::{AnalysisHost, FileId, ItemTree, SymbolKind, TextRange};

#[derive(Clone, Debug)]
pub struct DocSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: Option<String>,
    pub full_range: TextRange,
    pub selection_range: TextRange,
    pub children: Vec<DocSymbol>,
}

pub fn document_symbols(host: &mut AnalysisHost, file: FileId) -> Vec<DocSymbol> {
    let Some(analysis) = host.analyze(file) else {
        return Vec::new();
    };
    let tree = analysis.item_tree;
    tree.top_level()
        .filter_map(|(idx, _)| build(&tree, idx))
        .collect()
}

fn build(tree: &ItemTree, index: u32) -> Option<DocSymbol> {
    let symbol = &tree.symbols[index as usize];
    if symbol.name.is_empty() {
        return None; // error-recovered nameless item
    }
    Some(DocSymbol {
        name: symbol.name.clone(),
        kind: symbol.kind,
        detail: Some(symbol.signature.clone()).filter(|s| s != &symbol.name),
        full_range: symbol.full_range,
        selection_range: symbol.name_range,
        children: tree
            .children_of(index)
            .filter_map(|(idx, _)| build(tree, idx))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::AnalysisHost;

    #[test]
    fn nested_symbols_mirror_the_item_tree() {
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(
            file,
            "module M {\n  export circuit f(): Field { return 0; }\n}\n\
             struct Point { x: Field; }\n\
             constructor(v: Field) { }\n"
                .to_string(),
            1,
        );
        let symbols = document_symbols(&mut host, file);
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["M", "Point", "constructor"]);
        assert_eq!(symbols[0].children[0].name, "f");
        assert_eq!(symbols[1].children[0].name, "x");
        assert_eq!(symbols[0].kind, analyzer_core::SymbolKind::Module);
    }
}
