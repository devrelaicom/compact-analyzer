//! Never-die robustness pass for `analyzer_toolchain`'s public parsing and
//! position-mapping surface.
//!
//! Mirrors M3's corpus never-die sweep
//! (`crates/compact-analyzer/tests/corpus_smoke.rs::m3_features_never_panic_on_corpus`)
//! but scoped to the two functions that own this crate's untrusted-input
//! boundary:
//!
//! - [`parse_compiler_stderr`] parses arbitrary `compact` CLI stderr —
//!   adversarial by construction, since it's whatever text a subprocess
//!   happened to write. Step 1 below fuzzes it with a battery of malformed
//!   strings, no corpus checkout required, so it always runs in CI.
//! - [`locate`] maps a compiler-reported `(line, col)` back to a byte
//!   `TextRange` by parsing the source itself — so a real `.compact` corpus
//!   is the most representative adversarial input for it. Step 2 below
//!   sweeps every corpus file at a battery of in-bounds and out-of-bounds
//!   positions, and self-skips cleanly when no corpus checkout is present
//!   (this crate has no subprocess/toolchain dependency, and the corpus
//!   lives in a sibling repo, not a build-time dependency).
//!
//! Both parts assert on the actual public contract documented on the
//! functions themselves ("never panics"; `None` or an in-bounds range) —
//! they do not weaken it by swallowing a real panic as "expected".

use std::panic;
use std::path::{Path, PathBuf};

use analyzer_toolchain::{ParsedStderr, locate, parse_compiler_stderr};

// =====================================================================
// Step 1: `parse_compiler_stderr` adversarial battery (pure, always runs)
// =====================================================================

/// Statically-known malformed/adversarial stderr blobs. A few
/// dynamically-built cases (very long lines, embedded control bytes) are
/// appended in [`all_adversarial_inputs`] since they can't be `const`.
fn static_adversarial_inputs() -> Vec<&'static str> {
    vec![
        // Empty / whitespace-only.
        "",
        " ",
        "\t",
        "\n",
        "\n\n   \n\t\n",
        "\r\n\r\n",
        // Truncated `Exception:` at every stage of the grammar.
        "Exception:",
        "Exception: ",
        "Exception: bad.compact",
        "Exception: bad.compact line",
        "Exception: bad.compact line ",
        "Exception: bad.compact line 8",
        "Exception: bad.compact line 8 char",
        "Exception: bad.compact line 8 char ",
        "Exception: bad.compact line 8 char 8",
        "Exception: bad.compact line 8 char 8:",
        "Exception: bad.compact line 8 char 8: ",
        // Negative numbers (u32 has no sign).
        "Exception: bad.compact line -1 char 8: message",
        "Exception: bad.compact line 8 char -1: message",
        "Exception: bad.compact line -1 char -1: message",
        // Numbers that overflow u32::MAX (4294967295).
        "Exception: bad.compact line 99999999999999999999 char 8: message",
        "Exception: bad.compact line 8 char 99999999999999999999: message",
        "Exception: bad.compact line 99999999999999999999 char 99999999999999999999: message",
        "Exception: bad.compact line 4294967296 char 8: message",
        // Present but non-numeric line/char.
        "Exception: bad.compact line four char 8: message",
        "Exception: bad.compact line 8 char eight: message",
        "Exception: bad.compact line four char eight: message",
        "Exception: bad.compact line 8.5 char 8: message",
        "Exception: bad.compact line 0x8 char 8: message",
        "Exception: bad.compact line NaN char 8: message",
        "Exception: bad.compact line  char 8: message",
        "Exception: bad.compact line 8 char : message",
        // Usage block only (CLI invocation error, not a compile error).
        "Usage: compactc.bin <flag> ... <source-pathname> <target-directory-pathname>",
        "Usage: compactc.bin <flag> ... <source-pathname> <target-directory-pathname>\n       --help displays detailed usage information\n",
        // Degenerate basenames / marker placement.
        " line 1 char 1: message",
        "line 1 char 1: message",
        "Exception: line 1 char 1: message",
        "Exception:  line 1 char 1: message",
        " line  char : ",
        "Exception: my file.compact line 4 char 13: unbound identifier undefined_in_util",
        // Shape matches but nested markers repeat.
        "Exception: a.compact line 1 char 1: b.compact line 2 char 2: nested",
        "Exception: bad.compact line 1 line 2 char 8: message",
        "Exception: bad.compact line 8 char 8 char 9: message",
        // Multi-line mixes of valid + garbage + truncated.
        "some preamble noise\nException: real.compact line 5 char 2: the actual error\ntrailing noise\n",
        "line char : \n\nException: garbage\n\n",
        "Exception: bad.compact line 1 char 1: message\r\n",
        "Exception: bad.compact line 1 char 1: message with\ttabs\tand\x0bvtab",
        // Whitespace-heavy / oddly-spaced grammar.
        "Exception:   bad.compact   line   8   char   8:   message",
    ]
}

/// Every adversarial input the battery runs, including dynamically-built
/// cases that can't be `'static` string literals.
fn all_adversarial_inputs() -> Vec<String> {
    let mut inputs: Vec<String> = static_adversarial_inputs()
        .into_iter()
        .map(str::to_string)
        .collect();

    // Embedded NUL and other C0 control bytes (all valid UTF-8 scalars, so
    // this is a valid &str, not a UTF-8 violation) — inside a basename, a
    // message, and standalone.
    inputs.push(
        "Exception: bad\0file.compact line 1 char 1: message\0with\x01control\x1bbytes\x07"
            .to_string(),
    );
    inputs.push("\0\0\0".to_string());
    inputs.push("Exception: \0 line 1 char 1: \0".to_string());

    // A very long single line with no grammar match at all.
    inputs.push("x".repeat(500_000));

    // A very long single line that *does* match, with a huge message body.
    inputs.push(format!(
        "Exception: bad.compact line 1 char 1: {}",
        "y".repeat(500_000)
    ));

    // A very long basename (still well-formed grammar-wise).
    inputs.push(format!(
        "Exception: {} line 1 char 1: message",
        "z".repeat(200_000)
    ));

    // Many lines, alternating valid and garbage.
    let mut many_lines = String::new();
    for i in 0..2_000u32 {
        if i % 2 == 0 {
            many_lines.push_str(&format!(
                "Exception: f{i}.compact line {i} char 1: err {i}\n"
            ));
        } else {
            many_lines.push_str("not a diagnostic line at all\n");
        }
    }
    inputs.push(many_lines);

    inputs
}

/// Asserts the "every non-blank line lands somewhere" invariant that falls
/// directly out of [`parse_compiler_stderr`]'s implementation: each
/// non-blank input line becomes exactly one [`RawCompilerDiagnostic`] or
/// exactly one line of [`ParsedStderr::unparsed`] — never both, never
/// neither, never a panic getting there.
fn assert_internally_consistent(input: &str, parsed: &ParsedStderr) {
    let expected_non_blank = input.lines().filter(|l| !l.trim().is_empty()).count();
    let unparsed_line_count = if parsed.unparsed.is_empty() {
        0
    } else {
        parsed.unparsed.split('\n').count()
    };
    assert_eq!(
        parsed.diagnostics.len() + unparsed_line_count,
        expected_non_blank,
        "every non-blank line must land in exactly one of diagnostics/unparsed for input {input:?} (parsed: {parsed:?})",
    );
}

#[test]
fn parse_compiler_stderr_never_panics_on_adversarial_battery() {
    let inputs = all_adversarial_inputs();
    assert!(inputs.len() >= 40, "battery shrank unexpectedly");

    for input in &inputs {
        // `parse_compiler_stderr` is documented as never panicking, so a
        // real panic here is a genuine defect, not something to swallow —
        // `catch_unwind` is used only to attribute a panic to the specific
        // offending input in the failure message, not to suppress it.
        let outcome = panic::catch_unwind(|| parse_compiler_stderr(input));
        let parsed = match outcome {
            Ok(parsed) => parsed,
            Err(_) => panic!(
                "parse_compiler_stderr panicked on adversarial input (len {}): {:?}",
                input.len(),
                &input[..input.len().min(200)]
            ),
        };
        assert_internally_consistent(input, &parsed);
    }
}

#[test]
fn empty_and_blank_inputs_yield_default_parsed_stderr() {
    assert_eq!(parse_compiler_stderr(""), ParsedStderr::default());
    assert_eq!(parse_compiler_stderr("\n\n   \n"), ParsedStderr::default());
}

#[test]
fn valid_line_survives_among_adversarial_garbage() {
    // A well-formed diagnostic embedded in a sea of malformed lines must
    // still parse correctly — the fuzz battery shouldn't be able to
    // convince the parser to drop or corrupt a line it actually recognizes.
    let stderr = concat!(
        "Exception: bad.compact line -1 char 8: negative line, garbage\n",
        "Exception: bad.compact line 99999999999999999999 char 8: overflow, garbage\n",
        "Usage: compactc.bin <flag> ...\n",
        "Exception: real.compact line 5 char 2: the actual error\n",
        "Exception: bad.compact line four char eight: non-numeric, garbage\n",
    );

    let parsed = parse_compiler_stderr(stderr);

    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].file_basename, "real.compact");
    assert_eq!(parsed.diagnostics[0].line, 5);
    assert_eq!(parsed.diagnostics[0].col, 2);
    assert_eq!(parsed.diagnostics[0].message, "the actual error");
    assert_eq!(
        parsed.unparsed.lines().count(),
        4,
        "the four garbage lines must all survive into `unparsed`"
    );
}

#[test]
fn embedded_control_bytes_do_not_panic_and_still_parse_valid_shape() {
    // NUL and other C0 control bytes are valid Unicode scalars — this is a
    // well-formed `&str`, not a UTF-8 violation — and must not upset the
    // marker-splitting logic.
    let stderr = "Exception: bad.compact line 1 char 1: message\0with\x01control\x1bbytes\x07\n";

    let parsed = parse_compiler_stderr(stderr);

    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].file_basename, "bad.compact");
    assert!(parsed.diagnostics[0].message.contains('\0'));
}

// =====================================================================
// Step 2: `locate` corpus sweep (self-skips when the corpus is absent)
// =====================================================================

/// Resolves the compactp corpus directory, mirroring
/// `crates/compact-analyzer/tests/corpus_smoke.rs::corpus_dir` exactly
/// (that helper is private to its file, so it's duplicated here rather than
/// shared — `COMPACT_CORPUS_DIR` overrides; otherwise the sibling-checkout
/// relative path, which resolves identically from this crate's manifest
/// dir since both crates sit two levels under the repo root).
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

/// Builds a battery of `(line, col)` probes for one file: fixed
/// degenerate/extreme cases plus a grid sampled across the file's actual
/// line count and each sampled line's actual (Unicode-scalar) length, so
/// every probe sits near a real boundary — in-bounds, one-past-the-end, and
/// wildly out-of-bounds.
fn battery_positions_for(text: &str) -> Vec<(u32, u32)> {
    let lines: Vec<&str> = text.lines().collect();
    let total_lines = lines.len() as u32;

    let mut positions: Vec<(u32, u32)> = vec![
        (0, 0),
        (0, 1),
        (1, 0),
        (1, 1),
        (u32::MAX, 1),
        (1, u32::MAX),
        (u32::MAX, u32::MAX),
        (total_lines.saturating_add(1), 1),
        (total_lines.saturating_add(1_000_000), 1),
    ];

    // Sample up to 24 lines spread across the file; for each, probe its
    // start, midpoint, exact content end, one-past-the-end, and far past
    // the end.
    let sample_count = total_lines.min(24);
    if let Some(step) = total_lines.checked_div(sample_count) {
        let step = step.max(1);
        let mut i = 0u32;
        while i < total_lines {
            let line_no = i + 1; // 1-based
            let line_text = lines[i as usize];
            let scalar_len = line_text.chars().count() as u32;
            for col in [
                1,
                (scalar_len / 2).max(1),
                scalar_len.max(1),
                scalar_len + 1,
                scalar_len + 500,
            ] {
                positions.push((line_no, col));
            }
            i += step;
        }
    }

    positions
}

#[test]
fn locate_never_panics_on_corpus() {
    let Some(dir) = corpus_dir() else {
        eprintln!("locate sweep SKIPPED: no COMPACT_CORPUS_DIR and no ../../../compactp checkout");
        return;
    };

    let files = analyzer_core::discover_compact_files(&[dir]);
    assert!(
        files.len() > 100,
        "expected a large corpus, got {}",
        files.len()
    );

    let mut panicked_files: Vec<String> = Vec::new();
    let mut files_swept = 0usize;
    let mut positions_checked = 0usize;

    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let len = text.len() as u32;
        let positions = battery_positions_for(&text);
        positions_checked += positions.len();

        // Each file's sweep is individually guarded: a pathological file
        // panicking must not abort the run for the remaining ~400+ files
        // (never-die), matching M3's per-file `catch_unwind` sweep. Any
        // panic is recorded, not swallowed — the test fails at the end if
        // the list is non-empty. The closure only reads through shared
        // references to plain, non-interior-mutable data (`String`,
        // `PathBuf`, `Vec<(u32, u32)>`, `u32`), so it's `UnwindSafe` with no
        // `AssertUnwindSafe` needed.
        let outcome = panic::catch_unwind(|| {
            for &(line, col) in &positions {
                if let Some(range) = locate(&text, line, col) {
                    assert!(
                        u32::from(range.end()) <= len,
                        "locate returned OOB end {range:?} for len {len} at (line={line}, col={col}) in {path:?}",
                    );
                    assert!(
                        range.start() <= range.end(),
                        "locate returned start > end {range:?} at (line={line}, col={col}) in {path:?}",
                    );
                }
            }
        });

        match outcome {
            Ok(()) => files_swept += 1,
            Err(_) => panicked_files.push(path.display().to_string()),
        }
    }

    eprintln!(
        "locate sweep RAN: {files_swept}/{} files, {positions_checked} (line, col) probes",
        files.len()
    );

    assert!(
        panicked_files.is_empty(),
        "locate panicked on {} file(s), never-die violated: {panicked_files:#?}",
        panicked_files.len()
    );
}
