//! Memoized recompute engine (spec Approach A).
//!
//! Parse results are cached per file, keyed by a content hash. An edit that
//! produces identical text is a cache hit; anything else recomputes that
//! file only. The syntax tree is stored as a `rowan::GreenNode` because
//! `SyntaxNode` is `!Send` — consumers rebuild a cursor with
//! `SyntaxNode::new_root(green)` (cheap).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use compactp_diagnostics::Diagnostic;
use rowan::GreenNode;

use crate::item_tree::ItemTree;
use crate::line_index::LineIndex;
use crate::vfs::{FileId, Vfs};

/// Everything M1 knows about one file. All fields are cheap to clone.
#[derive(Clone)]
pub struct FileAnalysis {
    pub green: GreenNode,
    pub diagnostics: Arc<Vec<Diagnostic>>,
    pub line_index: Arc<LineIndex>,
    pub item_tree: Arc<ItemTree>,
}

pub struct AnalysisHost {
    vfs: Vfs,
    cache: HashMap<FileId, (u64, FileAnalysis)>,
    stdlib: Option<FileId>,
}

impl AnalysisHost {
    pub fn new() -> Self {
        Self {
            vfs: Vfs::new(),
            cache: HashMap::new(),
            stdlib: None,
        }
    }

    pub fn vfs(&self) -> &Vfs {
        &self.vfs
    }

    pub fn vfs_mut(&mut self) -> &mut Vfs {
        &mut self.vfs
    }

    /// Registers the materialized stdlib stub so the resolver can target it.
    pub fn register_stdlib(&mut self, path: &std::path::Path) -> FileId {
        let file = self.vfs.file_id(path);
        self.stdlib = Some(file);
        file
    }

    pub fn stdlib_file(&self) -> Option<FileId> {
        self.stdlib
    }

    /// Parses `file`, memoized on content. `None` if the file is unreadable.
    pub fn analyze(&mut self, file: FileId) -> Option<FileAnalysis> {
        let text = self.vfs.read(file)?;
        let hash = content_hash(&text);
        if let Some((cached_hash, analysis)) = self.cache.get(&file)
            && *cached_hash == hash
        {
            return Some(analysis.clone());
        }
        let result = compactp_parser::parse(&text);
        let root = compactp_syntax::SyntaxNode::new_root(result.green.clone());
        let analysis = FileAnalysis {
            green: result.green,
            diagnostics: Arc::new(result.errors),
            line_index: Arc::new(LineIndex::new(text)),
            item_tree: Arc::new(ItemTree::extract(&root)),
        };
        self.cache.insert(file, (hash, analysis.clone()));
        Some(analysis)
    }
}

impl Default for AnalysisHost {
    fn default() -> Self {
        Self::new()
    }
}

fn content_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Opens `text` as an overlay in a fresh host and returns (host, file).
    fn host_with(text: &str) -> (AnalysisHost, crate::FileId) {
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(Path::new("/test/doc.compact"));
        host.vfs_mut().set_overlay(file, text.to_string(), 1);
        (host, file)
    }

    #[test]
    fn valid_source_has_no_diagnostics() {
        let (mut host, file) = host_with("ledger count: Field;");
        let analysis = host.analyze(file).unwrap();
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn broken_source_reports_expected_colon() {
        // Verified fixture: zero-width span at offset 12
        let (mut host, file) = host_with("ledger count Field;");
        let analysis = host.analyze(file).unwrap();
        assert_eq!(analysis.diagnostics.len(), 1);
        let diag = &analysis.diagnostics[0];
        assert_eq!(diag.message, "expected COLON");
        assert_eq!(diag.primary_span.start(), crate::TextSize::new(12));
        assert_eq!(diag.primary_span.end(), crate::TextSize::new(12));
    }

    #[test]
    fn diagnostics_spans_stay_in_bounds() {
        let source = "@@@";
        let (mut host, file) = host_with(source);
        let analysis = host.analyze(file).unwrap();
        assert!(!analysis.diagnostics.is_empty());
        for diag in analysis.diagnostics.iter() {
            assert!(u32::from(diag.primary_span.end()) <= source.len() as u32);
        }
    }

    #[test]
    fn unchanged_content_hits_the_cache() {
        let (mut host, file) = host_with("ledger count: Field;");
        let first = host.analyze(file).unwrap();
        let second = host.analyze(file).unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &first.diagnostics,
            &second.diagnostics
        ));
    }

    #[test]
    fn edit_invalidates_the_cache() {
        let (mut host, file) = host_with("ledger count: Field;");
        let before = host.analyze(file).unwrap();
        assert!(before.diagnostics.is_empty());

        host.vfs_mut()
            .set_overlay(file, "ledger count Field;".to_string(), 2);
        let after = host.analyze(file).unwrap();
        assert_eq!(after.diagnostics.len(), 1);
        assert!(!std::sync::Arc::ptr_eq(
            &before.diagnostics,
            &after.diagnostics
        ));
    }

    #[test]
    fn tree_is_lossless() {
        let source = "/* comment */ ledger count: Field; // trailing";
        let (mut host, file) = host_with(source);
        let analysis = host.analyze(file).unwrap();
        let root = compactp_syntax::SyntaxNode::new_root(analysis.green);
        assert_eq!(root.text().to_string(), source);
    }

    #[test]
    fn unreadable_file_returns_none() {
        let mut host = AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(Path::new("/nonexistent/nope.compact"));
        assert!(host.analyze(file).is_none());
    }
}
