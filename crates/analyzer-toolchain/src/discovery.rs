//! Discovery of the `compact` CLI toolchain and the versions it reports.
//!
//! Resolution order: an explicit override (a path to the `compact` executable, or
//! a directory containing it) is tried in isolation; `PATH` is only searched when
//! no override is given. A binary that is found but fails to answer the version
//! queries is treated as absent — discovery never fails loudly, it returns `None`.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A discovered `compact` toolchain and the versions it reports.
#[derive(Clone, Debug)]
pub struct Toolchain {
    /// Absolute path to the `compact` executable to invoke.
    pub compact_bin: PathBuf,
    /// Compiler/toolchain version, e.g. `"0.31.1"` (from `compact compile --version`).
    pub tool_version: String,
    /// Language version, e.g. `"0.23.0"` (from `compact compile --language-version`).
    pub language_version: String,
}

impl Toolchain {
    /// Discovers a working `compact` toolchain.
    ///
    /// `override_path` may point directly at the `compact` executable, or at a
    /// directory containing it; when it is `None`, `PATH` is searched instead.
    /// An override is never combined with a `PATH` fallback: if it doesn't
    /// resolve to a usable binary, discovery reports absence rather than
    /// silently substituting whatever else is on `PATH`.
    ///
    /// Returns `None` if no candidate resolves to a binary that answers both
    /// version queries successfully. Never panics; never propagates a spawn
    /// error to the caller.
    pub fn discover(override_path: Option<&Path>) -> Option<Toolchain> {
        candidates(override_path)
            .into_iter()
            .find_map(|candidate| probe(&candidate))
    }
}

/// Platform-appropriate executable name for the `compact` CLI (`compact` on
/// Unix, `compact.exe` on Windows).
fn exe_name() -> String {
    format!("compact{}", env::consts::EXE_SUFFIX)
}

/// Enumerates candidate `compact` executable paths in resolution order.
///
/// When `override_path` is given, it is the *only* source of candidates (a
/// direct file path, or `<dir>/compact` if it names a directory) — no `PATH`
/// fallback. When it is `None`, every `PATH` entry is tried.
fn candidates(override_path: Option<&Path>) -> Vec<PathBuf> {
    match override_path {
        Some(path) if path.is_dir() => vec![path.join(exe_name())],
        Some(path) => vec![path.to_path_buf()],
        None => match env::var_os("PATH") {
            Some(path_var) => env::split_paths(&path_var)
                .map(|dir| dir.join(exe_name()))
                .collect(),
            None => Vec::new(),
        },
    }
}

/// Runs the version queries against `candidate`; returns `None` on any spawn
/// failure, non-zero exit, or unparseable output.
fn probe(candidate: &Path) -> Option<Toolchain> {
    let tool_version = query_version(candidate, "--version")?;
    let language_version = query_version(candidate, "--language-version")?;
    Some(Toolchain {
        compact_bin: candidate.to_path_buf(),
        tool_version,
        language_version,
    })
}

/// Runs `<candidate> compile <flag>`, returning trimmed stdout on success.
///
/// Any spawn error, non-zero exit, non-UTF-8 output, or empty output yields
/// `None` — a discovered-but-unrunnable binary is treated as absent.
fn query_version(candidate: &Path, flag: &str) -> Option<String> {
    let output = run(candidate, &["compile", flag])?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Spawns `candidate` with `args`, capturing stdout/stderr and never
/// inheriting our own stdio (critical: our stdout may be an LSP JSON-RPC
/// channel). Returns `None` on any spawn failure instead of propagating.
fn run(candidate: &Path, args: &[&str]) -> Option<Output> {
    Command::new(candidate)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_shim(dir: &Path, tool_version: &str, language_version: &str) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("compact");
        let script = format!(
            "#!/bin/sh\n\
if [ \"$1\" = \"compile\" ] && [ \"$2\" = \"--version\" ]; then\n\
echo \"{tool_version}\"\n\
exit 0\n\
fi\n\
if [ \"$1\" = \"compile\" ] && [ \"$2\" = \"--language-version\" ]; then\n\
echo \"{language_version}\"\n\
exit 0\n\
fi\n\
exit 1\n"
        );
        let mut file = std::fs::File::create(&path).expect("create shim script");
        file.write_all(script.as_bytes())
            .expect("write shim script");
        let mut perms = std::fs::metadata(&path).expect("stat shim").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod shim");
        path
    }

    #[cfg(unix)]
    #[test]
    fn discover_finds_toolchain_via_directory_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_shim(dir.path(), "9.9.9", "1.2.3");

        let toolchain = Toolchain::discover(Some(dir.path())).expect("toolchain discovered");

        assert_eq!(toolchain.tool_version, "9.9.9");
        assert_eq!(toolchain.language_version, "1.2.3");
        assert_eq!(toolchain.compact_bin, dir.path().join("compact"));
    }

    #[cfg(unix)]
    #[test]
    fn discover_finds_toolchain_via_direct_file_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), "4.5.6", "7.8.9");

        let toolchain = Toolchain::discover(Some(&shim)).expect("toolchain discovered");

        assert_eq!(toolchain.tool_version, "4.5.6");
        assert_eq!(toolchain.language_version, "7.8.9");
        assert_eq!(toolchain.compact_bin, shim);
    }

    #[cfg(unix)]
    #[test]
    fn discover_treats_non_executable_binary_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("compact");
        std::fs::write(&path, b"#!/bin/sh\necho nope\n").expect("write file");
        // Deliberately left non-executable (default create mode carries no +x bit).

        assert!(Toolchain::discover(Some(dir.path())).is_none());
    }

    #[test]
    fn discover_returns_none_for_empty_override_dir() {
        let dir = tempfile::tempdir().expect("tempdir");

        assert!(Toolchain::discover(Some(dir.path())).is_none());
    }

    #[test]
    fn discover_returns_none_for_nonexistent_override_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");

        assert!(Toolchain::discover(Some(&missing)).is_none());
    }
}
