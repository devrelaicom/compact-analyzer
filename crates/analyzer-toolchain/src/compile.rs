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
//! `std::process` has no built-in wait-with-timeout, so the deadline is
//! enforced by hand: spawn, then poll [`std::process::Child::try_wait`]
//! against a deadline — mirroring the integration-test harness's
//! `wait_with_timeout` poll loop (`crates/compact-analyzer/tests/support/mod.rs`)
//! — and kill the child if the deadline passes first.
//!
//! stdout/stderr are piped (never inherited: our own stdout may be an LSP
//! JSON-RPC channel elsewhere in this codebase, and this crate follows the
//! same convention regardless). Reading a piped child's output only *after*
//! it exits is a classic deadlock risk if the child ever writes more than
//! the OS pipe buffer holds (it blocks on `write`, we're only polling
//! `try_wait`, neither side proceeds) — so, exactly like the LSP test
//! harness reads the server's stdout on a background thread while it polls
//! for exit, this module drains both pipes on background threads that run
//! concurrently with the poll loop.
//!
//! **A single `kill()` on the direct child is not enough on the timeout
//! path**, and the `#[cfg(unix)]` shim test below proved it empirically: the
//! `compact` binary is not necessarily the leaf process doing the work (nor
//! is a `sh` script whose last statement isn't tail-call-optimized into an
//! `exec`) — it can have already forked a grandchild (e.g. `compactc.bin`)
//! that inherits copies of the same stdout/stderr pipe descriptors. Killing
//! only the direct child leaves that grandchild running and *still holding
//! the pipes open*, so the drain threads block on `read_to_end` waiting for
//! an EOF that only arrives once the orphan eventually exits on its own —
//! silently turning a bounded timeout into an unbounded hang. The fix
//! (`unix` only) is to put the child in its own process group at spawn time
//! (`Command::process_group(0)`, stable in `std` — no new dependency) and,
//! on timeout, signal the whole group (`kill(-pgid, SIGKILL)`, via a raw
//! `extern "C"` binding rather than the `libc` crate: every Unix Rust binary
//! already links libc for `std`'s own runtime, so no new Cargo dependency is
//! needed either). As a second line of defense against any process that
//! still escapes this (a platform without process-group semantics, a
//! double-forked daemon, etc.), the drain side additionally never blocks
//! longer than [`DRAIN_GRACE`] past the child's own exit/kill — so
//! `compile_file` itself can never hang regardless of what a descendant
//! process does with the pipes afterward.

use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::discovery::Toolchain;

/// Interval between `try_wait` polls while waiting for the child to exit.
///
/// Matches the integration-test harness's `wait_with_timeout` poll interval
/// (`crates/compact-analyzer/tests/support/mod.rs`) — fine enough that a
/// fast compile isn't held up waiting for the next poll, coarse enough not
/// to busy-loop.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Extra time, past the child's own exit or kill, that [`compile_file`]
/// will wait for the stderr-drain thread to report back before giving up on
/// capturing any more of it.
///
/// This is deliberately fixed and small rather than scaled to the caller's
/// `timeout`: it exists purely as a hard upper bound on how much longer
/// `compile_file` can possibly take *after* the child is already dead or
/// killed, so a descendant process that somehow still escapes the
/// process-group kill (see the module doc comment) can delay a result by at
/// most this much, never indefinitely.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

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

    command
        .arg(source)
        .arg(scratch)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return CompileOutcome {
                status: CompileStatus::InvocationError,
                stderr: format!("failed to spawn `{}`: {err}", tc.compact_bin.display()),
            };
        }
    };

    // Drain both pipes on background threads for the whole lifetime of the
    // wait below — see the module doc comment for why reading only after
    // exit is unsafe in general. stdout is fire-and-forget (its content is
    // discarded, so there's nothing to wait for); stderr reports back over
    // `stderr_rx`, received with a bounded grace period below rather than
    // joined unconditionally, so an escaped descendant still holding the
    // pipe open (see the module doc comment) can never make this function
    // hang.
    spawn_stdout_drain(child.stdout.take());
    let stderr_rx = spawn_stderr_capture(child.stderr.take());

    let timed_out = poll_until_exit_or_deadline(&mut child, timeout);
    if timed_out {
        kill_child_tree(&mut child);
    }
    // Reaps the child (a no-op re-read of the already-known status if it
    // exited on its own and `poll_until_exit_or_deadline` already observed
    // that via `try_wait`).
    let wait_result = child.wait();

    let stderr = stderr_rx.recv_timeout(DRAIN_GRACE).unwrap_or_default();

    if timed_out {
        return CompileOutcome {
            status: CompileStatus::TimedOut,
            stderr,
        };
    }

    match wait_result {
        Ok(status) => {
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
        Err(err) => CompileOutcome {
            status: CompileStatus::InvocationError,
            stderr: format!("failed to reap child process: {err}\n{stderr}"),
        },
    }
}

/// Polls `child` with `try_wait` until it exits or `timeout` elapses.
///
/// Returns `true` if the deadline passed first (the caller kills the
/// child), `false` if the child exited on its own. A `try_wait` failure
/// (vanishingly rare — see its docs) is treated the same as a deadline
/// overrun: the caller's subsequent kill will itself fail harmlessly if the
/// process is already gone, and `wait()` still reaps it.
fn poll_until_exit_or_deadline(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return false,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return true;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return true,
        }
    }
}

/// Puts the about-to-be-spawned child in a new process group (its own PGID,
/// equal to its own PID) so that [`kill_child_tree`] can later signal the
/// whole group rather than just the single process. `unix`-only: no-op
/// elsewhere (Windows job objects would be the equivalent tool there, but
/// that's out of scope for this crate today — see the module doc comment
/// and the task report's concerns section).
#[cfg(unix)]
fn isolate_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn isolate_process_group(_command: &mut Command) {}

/// Kills `child`'s entire process group on `unix` (falling back to a plain
/// `Child::kill()` elsewhere), so a grandchild that inherited the piped
/// stdout/stderr (see the module doc comment) is reaped too instead of
/// being orphaned and left holding the pipes open.
#[cfg(unix)]
fn kill_child_tree(child: &mut Child) {
    group_kill::kill_group(child.id());
    // Belt-and-braces: also signal the direct child by PID through `std`,
    // in case the raw group signal below didn't reach it for some
    // unforeseen reason. Harmless to send twice — a process that's already
    // dead just yields `ESRCH`, silently ignored by `kill_group` and by
    // `Child::kill` alike.
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_child_tree(child: &mut Child) {
    let _ = child.kill();
}

/// A minimal, dependency-free binding to POSIX `kill(2)` for signalling a
/// whole process group at once — something `std::process::Child` has no API
/// for (it only ever signals the single PID it holds).
#[cfg(unix)]
mod group_kill {
    // Edition 2024 requires `extern` blocks to be marked `unsafe`; calling
    // any item they declare remains `unsafe` regardless of edition. No
    // `libc` crate dependency is introduced by this: every Unix Rust binary
    // already links against the platform's libc for `std`'s own runtime, so
    // this binds a symbol that's already present in the final binary.
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    /// The POSIX `SIGKILL` signal number (9 on every Unix this crate
    /// targets — Linux and macOS both define it identically; there is no
    /// portable `std` constant for it).
    const SIGKILL: i32 = 9;

    /// Sends `SIGKILL` to the process group led by `pid`. Per `kill(2)`,
    /// passing a *negative* pid targets the whole group rather than the
    /// single process — this is the mechanism [`super::isolate_process_group`]
    /// sets each spawned child up for.
    ///
    /// The return value is ignored: `ESRCH` (the group has already exited —
    /// an expected, harmless race on the timeout path, since the child may
    /// finish between our timeout check and this call) is the only failure
    /// mode that can occur here, and there is nothing actionable to do
    /// about it.
    pub fn kill_group(pid: u32) {
        // SAFETY: `kill` is a plain two-integer libc call with no pointers
        // and no allocation; passing a negative pid is documented, ordinary
        // POSIX usage (`kill(2)`) that only affects OS-level process
        // signaling, not memory safety.
        unsafe {
            kill(-(pid as i32), SIGKILL);
        }
    }
}

/// Spawns a background thread that reads `pipe` to EOF and discards the
/// bytes, purely to keep the child's stdout pipe from filling up and
/// blocking it while the main thread polls for exit. Fire-and-forget: its
/// content is never needed, so there's nothing to report back or join.
/// `None` (stdout wasn't piped, which never happens given how
/// `compile_file` builds the command, but this stays total) spawns nothing.
fn spawn_stdout_drain(pipe: Option<ChildStdout>) {
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut sink = Vec::new();
            let _ = pipe.read_to_end(&mut sink);
        });
    }
}

/// Spawns a background thread that reads `pipe` to EOF, lossily decodes it
/// as UTF-8, and sends it on the returned channel. Receiving with
/// [`Receiver::recv_timeout`] (as [`compile_file`] does, via [`DRAIN_GRACE`])
/// rather than blocking indefinitely is what keeps a still-open pipe (e.g.
/// held by an escaped descendant — see the module doc comment) from hanging
/// this crate's caller: the thread itself may still be blocked in
/// `read_to_end` when the receive times out, but it is simply abandoned
/// (never joined) rather than waited on, and will exit harmlessly whenever
/// its read eventually does complete. `None` yields a channel whose sender
/// was never created, so a receive on it fails immediately rather than
/// blocking.
fn spawn_stderr_capture(pipe: Option<ChildStderr>) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
        });
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

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
