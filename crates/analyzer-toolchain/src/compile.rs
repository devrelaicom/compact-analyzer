//! Runs `compact compile --skip-zk --vscode` on a single source file with a
//! wall-clock timeout, capturing stderr and classifying the outcome.
//!
//! Empirically verified against real `compact 0.5.1` (2026-07-10, see
//! `task-4-report.md`): exit `0` on success (empty stdout/stderr, scratch
//! dir populated), exit `255` on a compile error (stderr holds the
//! `Exception: ...` line — see [`crate::parse`]), exit `1` with a `Usage:`
//! block on stderr when a required argument (e.g. the scratch dir) is
//! omitted. `--compact-path <joined>` is accepted and does resolve
//! `include`d files against it (confirmed with a real multi-file compile).
//!
//! The actual subprocess mechanics (spawn with piped stdio, wall-clock
//! timeout, `unix` process-group kill, pipe-draining to avoid a blocked-pipe
//! deadlock) live in [`crate::process::run_with_timeout`] — shared with
//! [`crate::format::format_source`], since `compact format` is the same
//! `compact` wrapper binary and can fork the same kind of grandchild. See
//! that module's doc comment for the full rationale.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::discovery::Toolchain;
use crate::process::{ProcessResult, run_with_timeout};

/// Classification of a [`compile_file`] invocation's outcome.
///
/// The exit-code mapping was captured empirically against real `compact
/// 0.5.1` (2026-07-10): `0` (success) and `255` (compile error) are the only
/// codes the compiler itself emits for these two cases; a `1` (CLI
/// usage/invocation error, e.g. a missing required argument) and any other
/// exit code this module doesn't specifically recognize both fall to
/// `InvocationError`, alongside spawn/wait/kill failures — this function
/// never panics, so every failure mode has to land in a `CompileStatus`
/// rather than propagating an error type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileStatus {
    /// Exit `0`: compilation succeeded; `scratch` was populated.
    Ok,
    /// Exit `255`: a genuine compile error (parse or semantic). `stderr`
    /// holds the compiler's `Exception: ...` diagnostic — see
    /// [`crate::parse::parse_compiler_stderr`].
    CompileError,
    /// Anything else: a CLI usage error (exit `1`, e.g. a missing
    /// argument), an unrecognized exit code, or a spawn/wait/kill failure
    /// (`stderr` then holds this module's own error text instead of the
    /// child's).
    InvocationError,
    /// The child did not exit within the requested timeout; it was killed
    /// and reaped. `stderr` holds whatever partial output it had produced
    /// before being killed.
    TimedOut,
}

/// The result of one [`compile_file`] call.
#[derive(Clone, Debug)]
pub struct CompileOutcome {
    /// The classified outcome — see [`CompileStatus`].
    pub status: CompileStatus,
    /// Captured stderr (lossily decoded as UTF-8), or — only when `status`
    /// is `InvocationError` and the process never ran at all (a spawn
    /// failure) — this module's own description of what went wrong.
    pub stderr: String,
}

/// Runs `<tc.compact_bin> compile --skip-zk --vscode [--compact-path
/// <joined search_path>] <source> <scratch>`, enforcing `timeout` as a
/// wall-clock deadline.
///
/// `search_path` is joined with the platform path-list separator via
/// [`std::env::join_paths`] (`:` on Unix, `;` on Windows) exactly as `PATH`
/// itself is — matching how `compact compile --compact-path` expects a
/// path list (confirmed empirically: a colon-joined list resolved `include`
/// across two directories in one real compile). An empty `search_path`
/// omits the flag entirely rather than passing `--compact-path ""`.
///
/// Never panics. A spawn failure, a `try_wait` polling failure, or a
/// deadline overrun are all handled without unwrapping: the first two map
/// to `CompileStatus::InvocationError` with a description of the failure as
/// `stderr`; the last maps to `CompileStatus::TimedOut` after the child is
/// killed and reaped so no zombie/orphan process is left behind.
pub fn compile_file(
    tc: &Toolchain,
    source: &Path,
    scratch: &Path,
    search_path: &[PathBuf],
    timeout: Duration,
) -> CompileOutcome {
    let mut command = Command::new(&tc.compact_bin);
    command.args(["compile", "--skip-zk", "--vscode"]);

    if !search_path.is_empty() {
        match env::join_paths(search_path.iter()) {
            Ok(joined) => {
                command.arg("--compact-path").arg(joined);
            }
            Err(err) => {
                // Only possible if a search-path entry contains the
                // platform's own list separator (or, on Windows, a `"`) —
                // an adversarial/corrupted input this crate can't repair.
                // Reported as an invocation error rather than silently
                // dropping the offending entries: a compile that then fails
                // to resolve an `include` for a silently-omitted directory
                // would be far more confusing to debug.
                return CompileOutcome {
                    status: CompileStatus::InvocationError,
                    stderr: format!("invalid --compact-path entry: {err}"),
                };
            }
        }
    }

    command.arg(source).arg(scratch);

    match run_with_timeout(command, timeout) {
        ProcessResult::Exited { status, stderr } => {
            let classified = match status.code() {
                Some(0) => CompileStatus::Ok,
                Some(255) => CompileStatus::CompileError,
                _ => CompileStatus::InvocationError,
            };
            CompileOutcome {
                status: classified,
                stderr,
            }
        }
        ProcessResult::TimedOut { stderr } => CompileOutcome {
            status: CompileStatus::TimedOut,
            stderr,
        },
        ProcessResult::SpawnFailed { message } => CompileOutcome {
            status: CompileStatus::InvocationError,
            stderr: message,
        },
        ProcessResult::WaitFailed { message, stderr } => CompileOutcome {
            status: CompileStatus::InvocationError,
            stderr: format!("{message}\n{stderr}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn write_source(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write source fixture");
        path
    }

    #[test]
    fn compiles_broken_file_and_reports_compile_error() {
        let Some(tc) = Toolchain::discover(None) else {
            eprintln!("compact not present; skipping");
            return;
        };

        let src_dir = tempfile::tempdir().expect("tempdir");
        let scratch_dir = tempfile::tempdir().expect("tempdir");
        let source = write_source(
            src_dir.path(),
            "bad.compact",
            "pragma language_version >= 0.16;\n\n\
             export circuit foo(): Field {\n  return undefined_name;\n}\n",
        );

        let outcome = compile_file(
            &tc,
            &source,
            scratch_dir.path(),
            &[],
            Duration::from_secs(30),
        );

        assert_eq!(outcome.status, CompileStatus::CompileError, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Exception:"),
            "stderr was: {:?}",
            outcome.stderr
        );
    }

    #[test]
    fn compiles_valid_file_and_populates_scratch() {
        let Some(tc) = Toolchain::discover(None) else {
            eprintln!("compact not present; skipping");
            return;
        };

        let src_dir = tempfile::tempdir().expect("tempdir");
        let scratch_dir = tempfile::tempdir().expect("tempdir");
        let source = write_source(
            src_dir.path(),
            "good.compact",
            "pragma language_version >= 0.16;\n\n\
             export circuit foo(): Field {\n  return 1;\n}\n",
        );

        let outcome = compile_file(
            &tc,
            &source,
            scratch_dir.path(),
            &[],
            Duration::from_secs(30),
        );

        assert_eq!(outcome.status, CompileStatus::Ok, "{outcome:?}");
        assert_eq!(outcome.stderr, "");
        let populated = std::fs::read_dir(scratch_dir.path())
            .expect("read scratch dir")
            .next()
            .is_some();
        assert!(populated, "expected scratch dir to be populated");
    }

    #[test]
    fn compact_path_resolves_included_file_in_search_path() {
        // Empirical confirmation (2026-07-10, task-4-report.md) that
        // `--compact-path` is accepted and actually consulted: without it,
        // this same setup fails with `failed to locate file "util.compact"`.
        let Some(tc) = Toolchain::discover(None) else {
            eprintln!("compact not present; skipping");
            return;
        };

        let src_dir = tempfile::tempdir().expect("tempdir");
        let lib_dir = tempfile::tempdir().expect("tempdir");
        let scratch_dir = tempfile::tempdir().expect("tempdir");

        write_source(
            lib_dir.path(),
            "util.compact",
            "pragma language_version >= 0.16;\n\n\
             export circuit helper(): Field {\n  return 1;\n}\n",
        );
        let source = write_source(
            src_dir.path(),
            "main.compact",
            "pragma language_version >= 0.16;\n\n\
             include \"util\";\n\n\
             export circuit foo(): Field {\n  return helper();\n}\n",
        );

        let outcome = compile_file(
            &tc,
            &source,
            scratch_dir.path(),
            &[lib_dir.path().to_path_buf()],
            Duration::from_secs(30),
        );

        assert_eq!(outcome.status, CompileStatus::Ok, "{outcome:?}");
    }

    #[cfg(unix)]
    #[test]
    fn timed_out_child_is_killed_and_call_returns_promptly() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let shim_dir = tempfile::tempdir().expect("tempdir");
        let shim_path = shim_dir.path().join("compact");
        let mut file = std::fs::File::create(&shim_path).expect("create shim");
        // Sleeps far longer than the timeout below, regardless of args, so
        // the ONLY way this test passes is if the child is actually killed
        // rather than awaited to completion.
        file.write_all(b"#!/bin/sh\nsleep 5\n")
            .expect("write shim script");
        let mut perms = std::fs::metadata(&shim_path)
            .expect("stat shim")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim_path, perms).expect("chmod shim");

        let tc = Toolchain {
            compact_bin: shim_path,
            tool_version: "0.0.0-shim".to_string(),
            language_version: "0.0.0-shim".to_string(),
        };

        let src_dir = tempfile::tempdir().expect("tempdir");
        let scratch_dir = tempfile::tempdir().expect("tempdir");
        let source = write_source(src_dir.path(), "irrelevant.compact", "unused");

        let timeout = Duration::from_millis(200);
        let started = Instant::now();
        let outcome = compile_file(&tc, &source, scratch_dir.path(), &[], timeout);
        let elapsed = started.elapsed();

        assert_eq!(outcome.status, CompileStatus::TimedOut, "{outcome:?}");
        assert!(
            elapsed < Duration::from_secs(3),
            "expected compile_file to return within roughly {timeout:?} + a small grace \
             period (well before the shim's 5s sleep), but it took {elapsed:?}"
        );
    }
}
