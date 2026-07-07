//! Hover: render the resolved definition as Markdown.

use analyzer_core::{AnalysisHost, Definition, FilePosition};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverResult {
    pub markdown: String,
}

/// Renders the definition at `pos` as Markdown: items get a fenced
/// ```compact signature block plus their doc paragraph; locals get their
/// detail in the fenced block.
pub fn hover(host: &mut AnalysisHost, pos: FilePosition) -> Option<HoverResult> {
    let def = host.resolve(pos)?;
    let markdown = match &def {
        Definition::Item { file, index } => {
            let sym = host
                .analyze(*file)?
                .item_tree
                .symbols
                .get(*index as usize)?
                .clone();
            match &sym.doc {
                Some(doc) => format!("```compact\n{}\n```\n\n{}", sym.signature, doc),
                None => format!("```compact\n{}\n```", sym.signature),
            }
        }
        Definition::Local { detail, .. } => format!("```compact\n{detail}\n```"),
    };
    Some(HoverResult { markdown })
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, FilePosition, fixture};

    fn hover_md(source: &str) -> Option<String> {
        let (clean, offset) = fixture::extract(source);
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        Some(hover(&mut host, FilePosition { file, offset })?.markdown)
    }

    #[test]
    fn hover_on_circuit_shows_signature_and_doc() {
        let md = hover_md(
            "// Doubles a value.\ncircuit double(x: Field): Field { return x + x; }\n\
             circuit m(): Field { return dou$0ble(1); }",
        )
        .unwrap();
        assert_eq!(
            md,
            "```compact\ncircuit double(x: Field): Field\n```\n\nDoubles a value."
        );
    }

    #[test]
    fn hover_on_param_shows_detail() {
        let md = hover_md("circuit f(base: Field): Field { return ba$0se; }").unwrap();
        assert_eq!(md, "```compact\nbase: Field\n```");
    }

    #[test]
    fn hover_on_stdlib_shows_bundled_doc() {
        let md = hover_md(
            "import CompactStandardLibrary;\ncircuit m(x: Field): Bytes<32> { return persistentHa$0sh<Field>(x); }",
        )
        .unwrap();
        assert!(md.contains("persistentHash"), "{md}");
        assert!(md.contains("SHA-256"), "{md}");
    }

    #[test]
    fn hover_on_nothing_is_none() {
        assert!(hover_md("circuit f(): Field { return myst$0ery; }").is_none());
    }
}
