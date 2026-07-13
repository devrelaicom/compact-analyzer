//! Shared subprocess control for every place in this crate that shells out
//! to the `compact` CLI: spawn-with-piped-stdio, a wall-clock timeout
//! enforced by hand (`std::process` has no built-in wait-with-timeout), and
//! `unix` whole-process-group isolation/kill.
//!
//! Extracted from [`crate::compile::compile_file`] (Task 4) once
//! [`crate::format::format_source`] (Task 5) needed the exact same
//! guarantees: `compact format` is the same `compact` wrapper binary as
//! `compact compile` and can fork the same `compactc.bin`-style grandchild,
//! so both callers need identical never-hang behavior rather than one
//! hardened implementation and one weaker copy.
//!
//! stdout/stderr are piped (never inherited: our own stdout may be an LSP
//! JSON-RPC channel elsewhere in this codebase). Reading a piped child's
//! output only *after* it exits is a classic deadlock risk if the child
//! ever writes more than the OS pipe buffer holds (it blocks on `write`, we're
//! only polling `try_wait`, neither side proceeds) — so, exactly like the
//! LSP test harness reads the server's stdout on a background thread while
//! it polls for exit, this module drains both pipes on background threads
//! that run concurrently with the poll loop.
//!
//! **A single `kill()` on the direct child is not enough on the timeout
//! path**, and the `#[cfg(unix)]` shim test in `compile.rs` proved it
//! empirically: the `compact` binary is not necessarily the leaf process
//! doing the work (nor is a `sh` script whose last statement isn't
//! tail-call-optimized into an `exec`) — it can have already forked a
//! grandchild (e.g. `compactc.bin`) that inherits copies of the same
//! stdout/stderr pipe descriptors. Killing only the direct child leaves that
//! grandchild running and *still holding the pipes open*, so the drain
//! threads block on `read_to_end` waiting for an EOF that only arrives once
//! the orphan eventually exits on its own — silently turning a bounded
//! timeout into an unbounded hang. The fix (`unix` only) is to put the child
//! in its own process group at spawn time (`Command::process_group(0)`,
//! stable in `std` — no new dependency) and, on timeout, signal the whole
//! group (`kill(-pgid, SIGKILL)`, via a raw `extern "C"` binding rather than
//! the `libc` crate: every Unix Rust binary already links libc for `std`'s
//! own runtime, so no new Cargo dependency is needed either). As a second
//! line of defense against any process that still escapes this (a platform
//! without process-group semantics, a double-forked daemon, etc.), the drain
//! side additionally never blocks longer than [`DRAIN_GRACE`] past the
//! child's own exit/kill — so [`run_with_timeout`]'s caller can never hang
//! regardless of what a descendant process does with the pipes afterward.

use std::io::Read;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Interval between `try_wait` polls while waiting for the child to exit.
///
/// Matches the integration-test harness's `wait_with_timeout` poll interval
/// (`crates/compact-analyzer/tests/support/mod.rs`) — fine enough that a
/// fast invocation isn't held up waiting for the next poll, coarse enough
/// not to busy-loop.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Extra time, past the child's own exit or kill, that [`run_with_timeout`]
/// will wait for the stderr-drain thread to report back before giving up on
/// capturing any more of it.
///
/// This is deliberately fixed and small rather than scaled to the caller's
/// `timeout`: it exists purely as a hard upper bound on how much longer
/// `run_with_timeout` can possibly take *after* the child is already dead or
/// killed, so a descendant process that somehow still escapes the
/// process-group kill (see the module doc comment) can delay a result by at
/// most this much, never indefinitely.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// Maximum number of spawn attempts when the OS momentarily reports the target
/// executable as busy for writing (`ETXTBSY`) — see [`spawn_retrying_etxtbsy`]
/// for why that transient can occur at all.
const ETXTBSY_MAX_ATTEMPTS: u32 = 40;

/// Delay between those attempts. The whole budget
/// (`ETXTBSY_MAX_ATTEMPTS * ETXTBSY_RETRY_DELAY`, ~80ms) is a deliberately
/// tight upper bound: it cannot wedge, and it stays far below any caller
/// timeout and the shim tests' own promptness assertions.
const ETXTBSY_RETRY_DELAY: Duration = Duration::from_millis(2);

/// Outcome of a [`run_with_timeout`] call. Every variant carries whatever
/// stderr was captured (lossily decoded as UTF-8) before the outcome was
/// known, except [`ProcessResult::SpawnFailed`], where the process never ran
/// at all.
#[derive(Clone, Debug)]
pub(crate) enum ProcessResult {
    /// The child exited on its own, within the timeout.
    Exited { status: ExitStatus, stderr: String },
    /// The child did not exit within the requested timeout; it was killed
    /// and reaped. `stderr` holds whatever partial output it had produced
    /// before being killed.
    TimedOut { stderr: String },
    /// The caller's cancellation flag (the LSP shutdown token) was observed
    /// set before the child exited; the *whole process group* was killed and
    /// reaped, exactly as on the timeout path. `stderr` holds whatever partial
    /// output was captured before the kill. Distinct from [`ProcessResult::TimedOut`]
    /// only so the outcome is reported honestly — both kill identically.
    Cancelled { stderr: String },
    /// `Command::spawn` itself failed (e.g. the binary doesn't exist or
    /// isn't executable). `message` describes the failure.
    SpawnFailed { message: String },
    /// The child exited (not via our timeout kill), but reaping it
    /// (`Child::wait`) then failed — vanishingly rare in practice. `message`
    /// describes the failure; `stderr` holds whatever was captured up to
    /// that point.
    WaitFailed { message: String, stderr: String },
}

/// Runs `command` to completion, enforcing `timeout` as a wall-clock
/// deadline and applying this crate's never-hang subprocess contract: stdio
/// is always piped (overwriting whatever `command` had configured), the
/// child is spawned in its own process group on `unix`, both pipes are
/// drained on background threads for the whole lifetime of the wait, and a
/// deadline overrun kills the *whole process group* (not just the direct
/// child) before reaping it. See the module doc comment for the full
/// rationale.
///
/// `cancel`, when `Some`, is polled every [`POLL_INTERVAL`] alongside the
/// exit/deadline checks: the moment it reads `true`, the child's whole process
/// group is killed and reaped immediately — the same handling as a deadline
/// overrun, but reported as [`ProcessResult::Cancelled`]. This is how the LSP
/// binary makes shutdown prompt (its `GlobalState.shutdown` flag becomes the
/// token) instead of waiting out `timeout` on an in-flight compile. `None`
/// disables cancellation (the formatting path, a synchronous main-thread
/// request that never participates in shutdown, passes `None`).
///
/// Never panics. Every failure mode — spawn failure, a `try_wait` polling
/// failure, a deadline overrun, a cancellation, or a reap failure — is
/// reported through [`ProcessResult`] rather than propagating an error type or
/// unwrapping.
pub(crate) fn run_with_timeout(
    mut command: Command,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> ProcessResult {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(&mut command);

    let mut child = match spawn_retrying_etxtbsy(|| command.spawn()) {
        Ok(child) => child,
        Err(err) => {
            let program = command.get_program().to_string_lossy().into_owned();
            return ProcessResult::SpawnFailed {
                message: format!("failed to spawn `{program}`: {err}"),
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

    let outcome = poll_until_exit_or_deadline(&mut child, timeout, cancel);
    if !matches!(outcome, WaitOutcome::Exited) {
        kill_child_tree(&mut child);
    }
    // Reaps the child (a no-op re-read of the already-known status if it
    // exited on its own and `poll_until_exit_or_deadline` already observed
    // that via `try_wait`).
    let wait_result = child.wait();

    let stderr = stderr_rx.recv_timeout(DRAIN_GRACE).unwrap_or_default();

    match outcome {
        WaitOutcome::TimedOut => ProcessResult::TimedOut { stderr },
        WaitOutcome::Cancelled => ProcessResult::Cancelled { stderr },
        WaitOutcome::Exited => match wait_result {
            Ok(status) => ProcessResult::Exited { status, stderr },
            Err(err) => ProcessResult::WaitFailed {
                message: format!("failed to reap child process: {err}"),
                stderr,
            },
        },
    }
}

/// Runs `spawn` — a `Command::spawn`/`Command::output` call — retrying a
/// bounded number of times when it fails with `ETXTBSY` ("text file busy"),
/// and returning any other outcome (success or a different error) immediately.
///
/// On Linux, `execve` fails with `ETXTBSY` if the target file is open for
/// writing by *any* process at that instant. In production this never fires:
/// the real `compact` binary resolved on `PATH` is not being written, so the
/// first attempt always succeeds and this wrapper costs nothing. It earns its
/// keep only under this crate's OWN tests, which fabricate a throwaway
/// `compact` shim on disk and then exec it. Those tests run as threads in a
/// single process, and `Command::spawn` forks: at the fork instant the whole
/// file-descriptor table is copied into the transient child, so a *different*
/// test thread's momentarily-open write handle to some just-created shim is
/// briefly present — as an inherited, still-open-for-write descriptor — in
/// that child until it `execve`s (the descriptor is `CLOEXEC`, but the
/// fork→exec window is not instantaneous). During that sub-millisecond window
/// the shim counts as busy for writing, so a concurrent exec of it hits
/// `ETXTBSY`. That is a genuine transient, reliably cleared by a short retry.
///
/// Only the spawn call itself is retried — never any subsequent wait — so this
/// is entirely orthogonal to [`run_with_timeout`]'s deadline and cancellation
/// handling: `ETXTBSY` is a failure to *start* the child, observed before it
/// runs at all.
pub(crate) fn spawn_retrying_etxtbsy<T>(
    mut spawn: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    // 39 retries in the loop + 1 final attempt below = ETXTBSY_MAX_ATTEMPTS total.
    for _ in 1..ETXTBSY_MAX_ATTEMPTS {
        match spawn() {
            Err(err) if is_text_file_busy(&err) => std::thread::sleep(ETXTBSY_RETRY_DELAY),
            result => return result,
        }
    }
    // Final attempt: its result is surfaced as-is, so an `ETXTBSY` that somehow
    // never clears degrades to exactly today's behaviour (a reported spawn
    // failure) rather than looping forever.
    spawn()
}

/// Whether `err` is the platform's "text file busy" (`ETXTBSY`) spawn failure.
///
/// `ETXTBSY` is errno `26` on the Unixes this crate targets (Linux and macOS
/// alike). The check is `unix`-gated so the retry is provably a no-op off-unix:
/// on Windows errno `26` is an unrelated error (`ERROR_NOT_DOS_DISK`) that a
/// spawn never raises as a "busy" signal, and gating it avoids ever mistaking
/// that for one and burning the retry budget on it.
fn is_text_file_busy(err: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        err.raw_os_error() == Some(26)
    }
    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

/// Why [`poll_until_exit_or_deadline`] stopped waiting on the child.
enum WaitOutcome {
    /// The child exited on its own, before the deadline and before any
    /// cancellation. The caller reaps it and reports its status.
    Exited,
    /// The wall-clock `timeout` elapsed first (or a `try_wait` failure forced
    /// the same handling). The caller kills the whole process group.
    TimedOut,
    /// The caller's `cancel` flag was observed set first. The caller kills the
    /// whole process group — handled identically to [`WaitOutcome::TimedOut`],
    /// kept distinct only so the outcome is reported honestly.
    Cancelled,
}

/// Polls `child` with `try_wait` until it exits, `timeout` elapses, or (when
/// `cancel` is `Some`) that flag is observed set.
///
/// `cancel` is checked at the top of every iteration, *before* `try_wait`, so
/// a flag that becomes `true` is honored within roughly one [`POLL_INTERVAL`]
/// tick no matter how much of `timeout` remains — that promptness is the whole
/// point of the hook. A `try_wait` failure (vanishingly rare — see its docs)
/// is treated the same as a deadline overrun: the caller's subsequent kill
/// will itself fail harmlessly if the process is already gone, and `wait()`
/// still reaps it.
fn poll_until_exit_or_deadline(
    child: &mut Child,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> WaitOutcome {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(flag) = cancel
            && flag.load(Ordering::Acquire)
        {
            return WaitOutcome::Cancelled;
        }
        match child.try_wait() {
            Ok(Some(_status)) => return WaitOutcome::Exited,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return WaitOutcome::TimedOut;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return WaitOutcome::TimedOut,
        }
    }
}

/// Puts the about-to-be-spawned child in a new process group (its own PGID,
/// equal to its own PID) so that [`kill_child_tree`] can later signal the
/// whole group rather than just the single process. `unix`-only: no-op
/// elsewhere (Windows job objects would be the equivalent tool there, but
/// that's out of scope for this crate today).
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
    /// sets each spawned child up for. The negated pgid is computed via
    /// [`group_signal_target`], which saturates rather than naively negating
    /// a cast `i32`, so this can never overflow-panic in a debug build even
    /// for a (real-world impossible) `pid >= 2^31`.
    ///
    /// The return value is ignored: `ESRCH` (the group has already exited —
    /// an expected, harmless race on the timeout path, since the child may
    /// finish between our timeout check and this call) is the only failure
    /// mode that can occur here, and there is nothing actionable to do
    /// about it.
    pub(super) fn kill_group(pid: u32) {
        // SAFETY: `kill` is a plain two-integer libc call with no pointers
        // and no allocation; passing a negative pid is documented, ordinary
        // POSIX usage (`kill(2)`) that only affects OS-level process
        // signaling, not memory safety.
        unsafe {
            kill(group_signal_target(pid), SIGKILL);
        }
    }

    /// Negated process-group id to pass to `kill(2)`. Saturates a `u32` that
    /// exceeds `i32::MAX` (impossible for a real OS pid) to `-i32::MAX` so
    /// the negation can never overflow-panic in a debug build — the naive
    /// `-(pid as i32)` form panics on overflow for any `pid` whose cast to
    /// `i32` lands on `i32::MIN` (e.g. `pid == 1 << 31`), since negating
    /// `i32::MIN` has no representable positive counterpart.
    fn group_signal_target(pid: u32) -> i32 {
        -i32::try_from(pid).unwrap_or(i32::MAX)
    }

    #[cfg(test)]
    mod tests {
        use super::group_signal_target;

        #[test]
        fn group_signal_target_leaves_a_real_pid_unchanged() {
            assert_eq!(group_signal_target(1234), -1234);
        }

        #[test]
        fn group_signal_target_saturates_u32_max_without_panicking() {
            assert_eq!(group_signal_target(u32::MAX), -i32::MAX);
        }

        #[test]
        fn group_signal_target_saturates_the_i32_min_cast_boundary_without_panicking() {
            // `1 << 31` cast to `i32` reinterprets as `i32::MIN`; negating
            // that directly overflows and panics in a debug build (the bug
            // this fn exists to close) — confirm the saturating form
            // sidesteps it instead.
            assert_eq!(group_signal_target(1 << 31), -i32::MAX);
        }
    }
}

/// Spawns a background thread that reads `pipe` to EOF and discards the
/// bytes, purely to keep the child's stdout pipe from filling up and
/// blocking it while the main thread polls for exit. Fire-and-forget: its
/// content is never needed, so there's nothing to report back or join.
/// `None` (stdout wasn't piped, which never happens given how
/// [`run_with_timeout`] builds the command) spawns nothing.
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
/// [`Receiver::recv_timeout`] (as [`run_with_timeout`] does, via
/// [`DRAIN_GRACE`]) rather than blocking indefinitely is what keeps a still-
/// open pipe (e.g. held by an escaped descendant — see the module doc
/// comment) from hanging this crate's caller: the thread itself may still be
/// blocked in `read_to_end` when the receive times out, but it is simply
/// abandoned (never joined) rather than waited on, and will exit harmlessly
/// whenever its read eventually does complete. `None` yields a channel whose
/// sender was never created, so a receive on it fails immediately rather
/// than blocking.
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
    use super::spawn_retrying_etxtbsy;
    use std::io;

    /// Contract test for the ETXTBSY spawn-retry, driven by an injectable
    /// closure over a call counter rather than a real subprocess — so it runs
    /// on every platform and pins down the exact retry arithmetic (the
    /// off-by-one bound) and the no-second-call-on-success guarantee, which the
    /// indirect shim tests can't observe and which never exercise the retry
    /// branch on macOS at all.
    #[test]
    fn spawn_retrying_etxtbsy_contract() {
        // Success on the first attempt: invoked exactly once, never retried.
        let mut calls = 0u32;
        let result: io::Result<()> = spawn_retrying_etxtbsy(|| {
            calls += 1;
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(calls, 1, "a successful spawn must not be retried");

        // A non-ETXTBSY error returns immediately, unretried, on every platform
        // (errno 2 is `ENOENT`). Off-unix this is the whole story: the errno
        // check is a no-op there, so NOTHING is ever retried.
        let mut calls = 0u32;
        let result: io::Result<()> = spawn_retrying_etxtbsy(|| {
            calls += 1;
            Err(io::Error::from_raw_os_error(2))
        });
        assert_eq!(calls, 1, "a non-busy error must not be retried");
        assert_eq!(result.unwrap_err().raw_os_error(), Some(2));

        // The retry path only fires on unix — that is where the kernel raises
        // ETXTBSY (errno 26) and where `is_text_file_busy` matches it.
        #[cfg(unix)]
        {
            // Every attempt reports ETXTBSY: retried up to the bound, then the
            // final error is surfaced — not swallowed, not looped forever.
            let mut calls = 0u32;
            let result: io::Result<()> = spawn_retrying_etxtbsy(|| {
                calls += 1;
                Err(io::Error::from_raw_os_error(26))
            });
            assert_eq!(
                calls,
                super::ETXTBSY_MAX_ATTEMPTS,
                "a never-clearing ETXTBSY must be attempted exactly the bounded number of times"
            );
            assert_eq!(
                result.unwrap_err().raw_os_error(),
                Some(26),
                "the final ETXTBSY must surface rather than be swallowed"
            );

            // A transient that clears: two ETXTBSYs, then success on the third.
            let mut calls = 0u32;
            let result: io::Result<()> = spawn_retrying_etxtbsy(|| {
                calls += 1;
                if calls < 3 {
                    Err(io::Error::from_raw_os_error(26))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_ok());
            assert_eq!(calls, 3, "must succeed on the third attempt");
        }
    }
}
