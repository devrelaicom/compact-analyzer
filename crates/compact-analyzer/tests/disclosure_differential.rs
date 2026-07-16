//! Compiler-differential harness for native witness-disclosure (WPP) checking
//! (v3-R foundation).
//!
//! Mirrors `type_differential.rs`'s structure exactly. Tier 1
//! (`rule_tagged_disclosure_fixtures`): per-fixture native verdict vs a
//! verdict captured from real `compactc`, with rule attribution. Tier 2
//! (`corpus_no_false_positive_disclosures`): the blocking no-false-positive
//! gate over the corpus. Both self-skip cleanly when the toolchain / corpus
//! is absent.
//!
//! This is a **no-op baseline** (Task R1): the native side
//! (`AnalysisHost::disclosure_diagnostics`) is stubbed to always return empty
//! until Task A1 lands the interpreter, and `FIXTURES` is empty until Task R2
//! adds fixtures. The harness must still compile and pass green so v3a has a
//! stable RED/GREEN gate to work against.

use std::path::{Path, PathBuf};
use std::time::Duration;

use analyzer_core::{AnalysisHost, FileId};
use analyzer_toolchain::{CompilerVerdict, Toolchain, classify, compile_file};

/// R0-confirmed (`analysis-passes.ss:5218`, verbatim), the compiler's WPP
/// error banner. A `RejectPostParse` outcome whose stderr contains this
/// substring is a confirmed witness-disclosure rejection.
const WPP_BANNER: &str = "potential witness-value disclosure must be declared but is not:";

/// The native checker's verdict for one source: does it report a *confirmed*
/// leak? Confirmed means an E-family diagnostic numbered >= 3100 — the
/// advisory (U-family) "unverified" signal is the fail-closed surface and is
/// deliberately excluded here.
fn native_discloses(text: &str, path: &Path) -> bool {
    let mut host = AnalysisHost::new();
    let file: FileId = host.vfs_mut().file_id(path);
    host.vfs_mut().set_overlay(file, text.to_string(), 1);
    host.disclosure_diagnostics(file)
        .iter()
        .any(|d| d.code.prefix == "E" && d.code.number >= 3100)
}

/// `compactc`'s disclosure verdict for one source file on disk: `Some(true)`
/// on a post-parse reject whose stderr carries the WPP banner, `Some(false)`
/// on a clean compile or a post-parse reject for an unrelated reason, `None`
/// on a parse rejection or an indeterminate outcome (usage error, timeout,
/// cancellation) — the disclosure phase never ran, so there is no verdict to
/// pin.
fn compiler_discloses(tc: &Toolchain, source: &Path) -> Option<bool> {
    let scratch = tempfile::tempdir().expect("scratch");
    let outcome = compile_file(
        tc,
        source,
        scratch.path(),
        &[],
        Duration::from_secs(30),
        None,
    );
    match classify(&outcome) {
        CompilerVerdict::Accept => Some(false),
        CompilerVerdict::RejectPostParse => Some(outcome.stderr.contains(WPP_BANNER)),
        CompilerVerdict::RejectParse | CompilerVerdict::Indeterminate => None,
    }
}

struct Fixture {
    name: &'static str,
    /// Expected native verdict (rule-tagged expectation, captured from compactc).
    native_discloses: bool,
    /// The rule this fixture pins.
    rule: &'static str,
}

/// Empty until Task R2 lands fixtures. The native side is a deliberate
/// no-op (Task R1's whole point): the fixtures are what v3a then turns
/// RED -> GREEN.
const FIXTURES: &[Fixture] = &[];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/disclosure")
}

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

#[test]
fn rule_tagged_disclosure_fixtures() {
    let dir = fixtures_dir();
    let tc = Toolchain::discover(None); // None = no live cross-check, still assert native.
    for fx in FIXTURES {
        let path = dir.join(fx.name);
        let text = std::fs::read_to_string(&path).expect("fixture readable");

        // Native side: must match the captured expectation.
        let got = native_discloses(&text, &path);
        assert_eq!(
            got,
            fx.native_discloses,
            "[{}] native verdict for {} (rule {})",
            if got { "disclose" } else { "silent" },
            fx.name,
            fx.rule
        );

        // Live cross-check (only when the toolchain is present): the native
        // disclose/silent verdict must match compactc's WPP-banner verdict.
        // Wording/span are NOT compared.
        if let Some(tc) = &tc {
            match compiler_discloses(tc, &path) {
                Some(discloses) => assert_eq!(
                    discloses, fx.native_discloses,
                    "compactc disclosure verdict for {} (rule {})",
                    fx.name, fx.rule
                ),
                None => eprintln!(
                    "disclosure_differential: compactc verdict indeterminate for {}",
                    fx.name
                ),
            }
        } else {
            eprintln!(
                "disclosure_differential: compactc absent; skipped live cross-check for {}",
                fx.name
            );
        }
    }
}

/// Binary corpus gate (foundation direction: no false positives). For every
/// corpus file compactc's disclosure verdict is clean on (accepted outright,
/// or rejected for an unrelated reason), the native checker must emit zero
/// confirmed-leak (E-family, number >= 3100) diagnostics; advisories are
/// allowed. Skips when the toolchain or corpus is absent.
#[test]
fn corpus_no_false_positive_disclosures() {
    let Some(tc) = Toolchain::discover(None) else {
        eprintln!("disclosure corpus gate SKIPPED: compactc absent");
        return;
    };
    let Some(dir) = corpus_dir() else {
        eprintln!(
            "disclosure corpus gate SKIPPED: no COMPACT_CORPUS_DIR and no ../compactp checkout"
        );
        return;
    };
    let files = analyzer_core::discover_compact_files(&[dir]);
    assert!(
        files.len() > 100,
        "expected a large corpus, got {}",
        files.len()
    );

    let mut clean = 0usize;
    let mut confirmed_leak = 0usize;
    let mut indeterminate = 0usize;
    let mut false_positives: Vec<PathBuf> = Vec::new();

    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match compiler_discloses(&tc, &path) {
            Some(false) => {
                clean += 1;
                if native_discloses(&text, &path) {
                    false_positives.push(path);
                }
            }
            Some(true) => confirmed_leak += 1,
            None => indeterminate += 1,
        }
    }

    eprintln!(
        "disclosure corpus gate: clean={clean} compiler_confirmed_leak={confirmed_leak} indeterminate={indeterminate} false_positives={}",
        false_positives.len()
    );
    assert!(
        clean > 0,
        "disclosure corpus gate exercised no compactc-clean files (clean={clean}, \
         confirmed_leak={confirmed_leak}, indeterminate={indeterminate}); \
         the toolchain or invocation may be misconfigured"
    );
    assert!(
        false_positives.is_empty(),
        "native emitted a confirmed-leak diagnostic on {} file(s) compactc's disclosure \
         verdict is clean on (false positives): {:?}",
        false_positives.len(),
        false_positives
    );
}
