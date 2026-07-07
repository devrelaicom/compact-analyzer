//! The bundled CompactStandardLibrary stub. `import CompactStandardLibrary;`
//! is compiler-internal (verified: never a file lookup), so the analyzer
//! mirrors it with this embedded, signature-accurate stub. It is
//! materialized to a real file on disk so goto-definition into the stdlib
//! lands somewhere an editor can open.

use std::path::{Path, PathBuf};

pub const STDLIB_SOURCE: &str = include_str!("../assets/std_0_23.compact");

/// Writes the stub as `CompactStandardLibrary.compact` under `dir` if it is
/// missing or differs from the bundled text. Returns the file path.
pub fn materialize(dir: &Path) -> std::io::Result<PathBuf> {
    let path = dir.join("CompactStandardLibrary.compact");
    let stale = match std::fs::read_to_string(&path) {
        Ok(existing) => existing != STDLIB_SOURCE,
        Err(_) => true,
    };
    if stale {
        std::fs::create_dir_all(dir)?;
        std::fs::write(&path, STDLIB_SOURCE)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_parses_with_zero_diagnostics() {
        let result = compactp_parser::parse(STDLIB_SOURCE);
        assert!(
            result.errors.is_empty(),
            "stdlib stub must parse cleanly: {:?}",
            result.errors.first()
        );
    }

    #[test]
    fn stub_declares_the_expected_surface() {
        let result = compactp_parser::parse(STDLIB_SOURCE);
        let root = compactp_syntax::SyntaxNode::new_root(result.green);
        let tree = crate::ItemTree::extract(&root);
        for name in [
            "Maybe",
            "Either",
            "some",
            "none",
            "persistentHash",
            "transientCommit",
            "kernel",
            "ecMul",
            "JubjubPoint",
        ] {
            assert!(
                tree.symbols.iter().any(|s| s.name == name && s.exported),
                "missing stdlib symbol: {name}"
            );
        }
        // Post-0.31 items must NOT be present.
        for name in ["ecNeg", "keccak256"] {
            assert!(
                !tree.symbols.iter().any(|s| s.name == name),
                "unexpected: {name}"
            );
        }
    }

    #[test]
    fn materialize_writes_once_and_refreshes_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = materialize(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), STDLIB_SOURCE);
        // Corrupt it; materialize must restore.
        std::fs::write(&path, "stale").unwrap();
        let path2 = materialize(dir.path()).unwrap();
        assert_eq!(path, path2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), STDLIB_SOURCE);
    }

    #[test]
    fn register_stdlib_makes_it_analyzable() {
        let dir = tempfile::tempdir().unwrap();
        let path = materialize(dir.path()).unwrap();
        let mut host = crate::AnalysisHost::new();
        assert_eq!(host.stdlib_file(), None);
        let file = host.register_stdlib(&path);
        assert_eq!(host.stdlib_file(), Some(file));
        let analysis = host.analyze(file).unwrap();
        assert!(analysis.diagnostics.is_empty());
        assert!(
            analysis
                .item_tree
                .symbols
                .iter()
                .any(|s| s.name == "persistentHash")
        );
    }
}
