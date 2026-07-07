//! Goto-definition: the definition site of the identifier at a position.

use analyzer_core::{AnalysisHost, FilePosition};

use crate::NavTarget;

/// Definition site of the identifier at `pos`.
pub fn goto_definition(host: &mut AnalysisHost, pos: FilePosition) -> Option<NavTarget> {
    let def = host.resolve(pos)?;
    let (file, name_range, full_range) = host.nav_info(&def)?;
    Some(NavTarget {
        file,
        name_range,
        full_range,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{AnalysisHost, FilePosition, fixture};

    fn goto(source: &str) -> Option<(bool, String)> {
        // Returns (is_stdlib_target, text at the target name range).
        let (clean, offset) = fixture::extract(source);
        let dir = tempfile::tempdir().unwrap();
        let std_path = analyzer_core::stdlib::materialize(dir.path()).unwrap();
        let mut host = AnalysisHost::new();
        host.register_stdlib(&std_path);
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(file, clean, 1);
        let target = goto_definition(&mut host, FilePosition { file, offset })?;
        let text = host.vfs_mut().read(target.file).unwrap();
        let name = text[target.name_range].to_string();
        Some((Some(target.file) == host.stdlib_file(), name))
    }

    #[test]
    fn goto_local_and_item() {
        assert_eq!(
            goto(
                "circuit helper(): Field { return 1; }\ncircuit m(): Field { return help$0er(); }"
            ),
            Some((false, "helper".to_string()))
        );
        assert_eq!(
            goto("circuit f(base: Field): Field { return ba$0se; }"),
            Some((false, "base".to_string()))
        );
    }

    #[test]
    fn goto_stdlib_lands_in_stub_file() {
        assert_eq!(
            goto(
                "import CompactStandardLibrary;\ncircuit m(x: Field): Bytes<32> { return persistentHa$0sh<Field>(x); }"
            ),
            Some((true, "persistentHash".to_string()))
        );
    }

    #[test]
    fn goto_nothing_on_unresolved() {
        assert_eq!(goto("circuit m(): Field { return myst$0ery(); }"), None);
    }
}
