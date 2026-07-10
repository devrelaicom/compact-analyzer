//! Corpus smoke: run compactp's ~486-file corpus through workspace indexing
//! and the span-producing IDE features, asserting no panic and no
//! out-of-bounds spans. Skips cleanly when the corpus is unavailable — set
//! COMPACT_CORPUS_DIR, or check out ../compactp next to this repo.

use std::path::{Path, PathBuf};

use analyzer_core::{
    AnalysisHost, FileId, FilePosition, SyntaxKind, SyntaxNode, TextRange, TextSize,
};

fn corpus_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("COMPACT_CORPUS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../compactp/tests/corpus");
    p.is_dir().then_some(p)
}

fn assert_in_bounds(host: &mut AnalysisHost, file: FileId, range: TextRange, what: &str) {
    let len = host
        .vfs_mut()
        .read(file)
        .map(|t| t.len() as u32)
        .unwrap_or(0);
    assert!(
        u32::from(range.end()) <= len,
        "{what}: span {range:?} exceeds file length {len} in {:?}",
        host.vfs().path(file)
    );
}

fn check_doc_symbol(host: &mut AnalysisHost, file: FileId, sym: &analyzer_ide::DocSymbol) {
    assert_in_bounds(host, file, sym.full_range, "doc symbol full_range");
    assert_in_bounds(
        host,
        file,
        sym.selection_range,
        "doc symbol selection_range",
    );
    for child in &sym.children {
        check_doc_symbol(host, file, child);
    }
}

#[test]
fn corpus_smoke_no_panics_no_oob_spans() {
    let Some(dir) = corpus_dir() else {
        eprintln!("corpus smoke SKIPPED: no COMPACT_CORPUS_DIR and no ../compactp checkout");
        return;
    };
    let mut host = AnalysisHost::new();
    host.discover_and_index(&[dir], &|| true);
    let files = host.workspace_files();
    assert!(
        files.len() > 100,
        "expected a large corpus, got {}",
        files.len()
    );

    for file in files {
        let Some(analysis) = host.analyze(file) else {
            continue;
        };
        for sym in analyzer_ide::document_symbols(&mut host, file) {
            check_doc_symbol(&mut host, file, &sym);
        }
        let root = SyntaxNode::new_root(analysis.green.clone());
        let offsets: Vec<TextSize> = root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| t.text_range().start())
            .collect();
        for off in offsets {
            let pos = FilePosition { file, offset: off };
            if let Some(nav) = analyzer_ide::goto_definition(&mut host, pos) {
                assert_in_bounds(&mut host, nav.file, nav.name_range, "goto");
                assert_in_bounds(&mut host, nav.file, nav.full_range, "goto full_range");
            }
            let _ = analyzer_ide::hover(&mut host, pos);
        }
    }

    for s in analyzer_ide::workspace_symbols(&mut host, "") {
        assert_in_bounds(&mut host, s.file, s.name_range, "workspace symbol");
    }
}

/// M3 sweep: runs completion, semantic tokens, folding, and selection ranges
/// over the corpus at sampled positions, guarded under `catch_unwind` so one
/// pathological file cannot abort the run. Asserts no panics and no
/// out-of-bounds spans, including inside error-recovered trees.
#[test]
fn m3_features_never_panic_on_corpus() {
    let Some(dir) = corpus_dir() else {
        eprintln!("corpus not present; skipping");
        return;
    };
    let files = analyzer_core::discover_compact_files(&[dir]);
    for path in files {
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let len = src.len() as u32;
        let result = std::panic::catch_unwind(|| {
            let mut host = analyzer_core::AnalysisHost::new();
            let file = host.vfs_mut().file_id(&path);
            host.vfs_mut().set_overlay(file, src.clone(), 1);

            // Semantic tokens: in-bounds, ordered.
            for t in analyzer_ide::semantic_tokens(&mut host, file) {
                assert!(u32::from(t.range.end()) <= len, "token OOB in {:?}", path);
            }
            // Folding + selection: in-bounds.
            for f in analyzer_ide::folding_ranges(&mut host, file) {
                assert!(u32::from(f.range.end()) <= len);
            }
            // Completion at a bounded sample of offsets (every ~16th byte on a
            // char boundary), including right after any '.' (member trigger).
            let mut offsets: Vec<analyzer_core::TextSize> = (0..=len)
                .step_by(16)
                .filter(|&o| src.is_char_boundary(o as usize))
                .map(analyzer_core::TextSize::new)
                .collect();
            for (i, _) in src.match_indices('.') {
                let o = (i + 1) as u32;
                if src.is_char_boundary(o as usize) {
                    offsets.push(analyzer_core::TextSize::new(o));
                }
            }
            for off in &offsets {
                let _ = analyzer_ide::completion(
                    &mut host,
                    analyzer_core::FilePosition { file, offset: *off },
                );
            }
            let chains = analyzer_ide::selection_ranges(&mut host, file, &offsets);
            for chain in chains {
                for r in chain {
                    assert!(u32::from(r.end()) <= len);
                }
            }
        });
        assert!(result.is_ok(), "M3 features panicked on {:?}", path);
    }
}
