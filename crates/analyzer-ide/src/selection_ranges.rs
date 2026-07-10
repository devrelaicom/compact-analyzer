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
    // rowan's `token_at_offset` asserts the offset lies within the root range
    // and PANICS otherwise (it only returns `TokenAtOffset::None` for an empty
    // range, never for an out-of-bounds offset). Callers pass arbitrary
    // offsets — a stale document version or a position past EOF — so guard the
    // upper bound here to preserve the never-die contract.
    if offset > root.text_range().end() {
        return Vec::new();
    }
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
    use analyzer_core::{AnalysisHost, TextRange, TextSize};

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
        // Outermost range is the whole file exactly (start AND end), not just
        // the end offset.
        assert_eq!(
            *chain.last().unwrap(),
            TextRange::new(0.into(), TextSize::new(source.len() as u32))
        );
    }

    #[test]
    fn out_of_range_offset_yields_empty_chain_without_panic() {
        // A stale document version or a position past EOF can hand us an
        // offset beyond the buffer. rowan's `token_at_offset` asserts the
        // offset is within the root range and PANICS otherwise, so the
        // never-die guarantee requires an explicit bounds guard: the chain
        // for such an offset must be empty, and the call must not panic.
        let source = "circuit f(): Field { return xyz; }";
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/m.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let off = TextSize::new(source.len() as u32 + 50);
        let chains = selection_ranges(&mut host, file, &[off]);
        assert_eq!(chains.len(), 1);
        assert!(chains[0].is_empty());
    }

    #[test]
    fn multiple_offsets_each_get_their_own_chain() {
        // Per-offset independence: two valid offsets inside two different
        // tokens, walked over the same shared root, each yield their own
        // innermost-first chain climbing out to the whole file.
        let source = "circuit f(): Field { return xyz; }";
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(std::path::Path::new("/t/m.compact"));
        host.vfs_mut().set_overlay(file, source.to_string(), 1);
        let off_field = TextSize::new(source.find("Field").unwrap() as u32 + 1);
        let off_xyz = TextSize::new(source.find("xyz").unwrap() as u32 + 1);
        let chains = selection_ranges(&mut host, file, &[off_field, off_xyz]);
        assert_eq!(chains.len(), 2);
        assert_eq!(&source[chains[0][0]], "Field");
        assert_eq!(&source[chains[1][0]], "xyz");
        let whole = TextRange::new(0.into(), TextSize::new(source.len() as u32));
        assert_eq!(*chains[0].last().unwrap(), whole);
        assert_eq!(*chains[1].last().unwrap(), whole);
    }
}
