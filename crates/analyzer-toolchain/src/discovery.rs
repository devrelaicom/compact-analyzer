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
///
/// An override candidate is kept only if it resolves to a concrete file that
/// already exists on disk. This is load-bearing, not just a fast path: a bare
/// relative override name with no path separator (e.g. `"compact"`) handed
/// straight to `Command::new` would make the OS launcher (`execvp` on Unix,
/// `CreateProcess` on Windows) run its *own* `PATH` search — silently
/// reintroducing the very `PATH` fallback the override contract forbids. The
/// `is_file` gate forces such an unresolvable override to `None` instead.
fn candidates(override_path: Option<&Path>) -> Vec<PathBuf> {
    match override_path {
        Some(path) if path.is_dir() => keep_if_file(path.join(exe_name())),
        Some(path) => keep_if_file(path.to_path_buf()),
        None => match env::var_os("PATH") {
            Some(path_var) => env::split_paths(&path_var)
                .map(|dir| dir.join(exe_name()))
                .collect(),
            None => Vec::new(),
        },
    }
}

/// One-element candidate list if `path` is an existing file, else empty — keeps
/// an unresolvable override from reaching `Command::new` as a bare name.
fn keep_if_file(path: PathBuf) -> Vec<PathBuf> {
    if path.is_file() {
        vec![path]
    } else {
        Vec::new()
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
/// Any spawn error, non-zero exit, non-UTF-8 output, or output that does not
/// [look like a version][looks_like_version] yields `None` — a
/// discovered-but-unrunnable binary, or one that answers with something other
/// than a version, is treated as absent.
///
/// The version-shape check is load-bearing on Windows: `C:\Windows\System32`
/// is always on `PATH` and holds `compact.exe` — the NTFS file-compression
/// utility, entirely unrelated to the Compact compiler. It exits `0` and
/// prints prose for `compile --version`, so accepting any non-empty stdout as
/// a version would mistake it for the toolchain and make every toolchain-gated
/// test run against the wrong binary instead of self-skipping.
fn query_version(candidate: &Path, flag: &str) -> Option<String> {
    let output = run(candidate, &["compile", flag])?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if looks_like_version(trimmed) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// True if `s` looks like a `compact` version answer — a bare dotted-numeric
/// version such as `0.31.1` or `0.23.0` (at least `MAJOR.MINOR`), optionally
/// with a `-`/`+` pre-release or build suffix (`0.31.1-beta.2`).
///
/// Deliberately strict: it exists solely to tell the real Compact compiler
/// apart from an unrelated binary that merely happens to be named `compact` on
/// `PATH` and to exit `0` with some non-version stdout — most concretely
/// Windows' own `C:\Windows\System32\compact.exe` (the NTFS compression tool),
/// whose prose answers must be rejected so discovery reports absence there.
fn looks_like_version(s: &str) -> bool {
    if s.is_empty() || s.chars().any(char::is_whitespace) {
        return false;
    }
    // Drop any pre-release/build tail; the numeric core carries the shape we
    // validate. `split` always yields at least one element, so `next().unwrap()`
    // is infallible.
    let core = s.split(['-', '+']).next().unwrap();
    let components: Vec<&str> = core.split('.').collect();
    components.len() >= 2
        && components
            .iter()
            .all(|comp| !comp.is_empty() && comp.bytes().all(|b| b.is_ascii_digit()))
}

/// Spawns `candidate` with `args`, capturing stdout/stderr and never
/// inheriting our own stdio (critical: our stdout may be an LSP JSON-RPC
/// channel). Returns `None` on any spawn failure instead of propagating.
fn run(candidate: &Path, args: &[&str]) -> Option<Output> {
    let mut command = Command::new(candidate);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Retry a transient `ETXTBSY` around the spawn: see
    // [`crate::process::spawn_retrying_etxtbsy`] for why a just-created shim
    // can be momentarily busy for writing under this crate's parallel tests.
    // A no-op in production, where the resolved `compact` is never mid-write.
    crate::process::spawn_retrying_etxtbsy(|| command.output()).ok()
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

    /// Mimics an unrelated binary that merely happens to be named `compact` on
    /// PATH: it exits `0` for the version queries but answers with multi-line
    /// prose instead of a version. This is the cross-platform stand-in for
    /// Windows' own `C:\Windows\System32\compact.exe` (the NTFS file-compression
    /// utility), which is what actually poisoned discovery on the Windows CI
    /// runner — there `C:\Windows\System32` is always on PATH.
    #[cfg(unix)]
    fn write_impostor_shim(dir: &Path) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("compact");
        let script = "#!/bin/sh\n\
             echo 'Compresses and uncompresses files on NTFS partitions.'\n\
             echo ''\n\
             echo '0 files within 1 directories were compressed.'\n\
             exit 0\n";
        let mut file = std::fs::File::create(&path).expect("create impostor shim");
        file.write_all(script.as_bytes())
            .expect("write impostor shim");
        let mut perms = std::fs::metadata(&path).expect("stat shim").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod shim");
        path
    }

    #[cfg(unix)]
    #[test]
    fn discover_rejects_a_compact_named_binary_that_answers_with_non_version_prose() {
        // Regression (Windows CI): PATH there contains
        // `C:\Windows\System32\compact.exe`, the NTFS file-compression utility,
        // which is unrelated to the Compact compiler. It exits `0` and prints
        // prose for `compile --version`, so discovery used to accept it and the
        // toolchain-gated tests ran against it (and failed) instead of
        // self-skipping. A non-version answer must be treated as "not the
        // compiler" and reported as absence.
        let dir = tempfile::tempdir().expect("tempdir");
        write_impostor_shim(dir.path());

        assert!(Toolchain::discover(Some(dir.path())).is_none());
    }

    #[test]
    fn looks_like_version_accepts_real_answers_and_rejects_prose() {
        // Real `compact compile --version` / `--language-version` answers.
        assert!(looks_like_version("0.31.1"));
        assert!(looks_like_version("0.23.0"));
        // A pre-release/build suffix is tolerated.
        assert!(looks_like_version("0.31.1-beta.2"));
        // Impostor / prose answers.
        assert!(!looks_like_version(""));
        assert!(!looks_like_version("42")); // no dotted component
        assert!(!looks_like_version(
            "0 files within 1 directories were compressed."
        ));
        assert!(!looks_like_version(
            "Compresses and uncompresses files on NTFS partitions."
        ));
        assert!(!looks_like_version("v0.31.1")); // leading non-digit
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

    #[test]
    fn discover_does_not_path_search_a_bare_name_override() {
        // A bare relative name (no path separator) must NOT be handed to
        // `Command::new` unresolved: the OS launcher (`execvp`/`CreateProcess`)
        // would run its own PATH search and silently find the real `compact`
        // installed on this machine, contradicting the "an override is never
        // combined with a PATH fallback" invariant. An override that doesn't
        // resolve to a concrete on-disk file must yield `None`.
        assert!(Toolchain::discover(Some(Path::new("compact"))).is_none());
    }

    #[test]
    fn discover_returns_none_for_nonexistent_absolute_override() {
        assert!(Toolchain::discover(Some(Path::new("/nonexistent/compact"))).is_none());
    }
}
