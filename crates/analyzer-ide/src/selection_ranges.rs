//! CST ancestor-chain selection ranges.

use analyzer_core::{AnalysisHost, FileId, SyntaxNode, TextRange, TextSize};
use rowan::TokenAtOffset;

/// For each offset, the chain of CST ranges from the token outward to the
/// file, innermost first, with consecutive duplicates removed.
pub fn selection_ranges(
    host: &mut AnalysisHost,
    file: FileId,
    offsets: &[TextSize],
) -> Vec<Vec<TextRange>> {
    let root = match host.analyze(file) {
        Some(a) => SyntaxNode::new_root(a.green.clone()),
        None => return offsets.iter().map(|_| Vec::new()).collect(),
    };
    offsets.iter().map(|&off| chain_at(&root, off)).collect()
}

fn chain_at(root: &SyntaxNode, offset: TextSize) -> Vec<TextRange> {
    let token = match root.token_at_offset(offset) {
        TokenAtOffset::None => return Vec::new(),
        TokenAtOffset::Single(t) => t,
        TokenAtOffset::Between(l, r) => {
            if !r.kind().is_trivia() {
                r
            } else {
                l
            }
        }
    };
    let mut ranges = vec![token.text_range()];
    for node in token.parent().into_iter().flat_map(|p| p.ancestors()) {
        let r = node.text_range();
        if ranges.last() != Some(&r) {
            ranges.push(r);
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, TextSize};

    #[test]
    fn chain_grows_outward_and_is_nested() {
        let source = "circuit f(): Field { return xyz; }";
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/m.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let off = TextSize::new(source.find("xyz").unwrap() as u32 + 1);
        let chains = selection_ranges(&mut host, file, &[off]);
        let chain = &chains[0];
        // Innermost first, each strictly contained in the next, last == file.
        assert_eq!(&source[chain[0]], "xyz");
        for w in chain.windows(2) {
            assert!(w[1].contains_range(w[0]) && w[1] != w[0]);
        }
        assert_eq!(
            chain.last().unwrap().end(),
            TextSize::new(source.len() as u32)
        );
    }
}
