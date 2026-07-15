//! Compiler-differential harness for native type checking (v2b.2 foundation).
//!
//! Tier 1 (this file's `rule_tagged_fixtures`): per-fixture native verdict vs a
//! verdict captured from real `compactc`, with rule attribution. Tier 2
//! (`corpus_no_false_positives`): the blocking no-false-positive gate over the
//! ~486-file corpus. Both self-skip cleanly when the toolchain / corpus is
//! absent.

use std::path::{Path, PathBuf};
use std::time::Duration;

use analyzer_core::{AnalysisHost, FileId};
use analyzer_toolchain::{CompilerVerdict, Toolchain, classify, compile_file};

/// The native checker's verdict for one source: does it report any type
/// diagnostic? (Parse diagnostics are a separate surface and are not consulted
/// here — the differential is type-only.)
fn native_rejects(text: &str, path: &Path) -> bool {
    let mut host = AnalysisHost::new();
    let file: FileId = host.vfs_mut().file_id(path);
    host.vfs_mut().set_overlay(file, text.to_string(), 1);
    !host.type_diagnostics(file).is_empty()
}

/// `compactc`'s verdict for one source file on disk, or `None` if no toolchain.
fn compiler_verdict(tc: &Toolchain, source: &Path) -> CompilerVerdict {
    let scratch = tempfile::tempdir().expect("scratch");
    let outcome = compile_file(
        tc,
        source,
        scratch.path(),
        &[],
        Duration::from_secs(30),
        None,
    );
    classify(&outcome)
}

struct Fixture {
    name: &'static str,
    /// Expected native verdict (rule-tagged expectation, captured from compactc).
    native_rejects: bool,
    /// The rule this fixture pins.
    rule: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "bool_return_ok.compact",
        native_rejects: false,
        rule: "primitive-literal-return",
    },
    Fixture {
        name: "bool_return_mismatch.compact",
        native_rejects: true,
        rule: "primitive-literal-return",
    },
];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/type")
}

#[test]
fn rule_tagged_fixtures() {
    let dir = fixtures_dir();
    let tc = Toolchain::discover(None); // None = no live cross-check, still assert native.
    for fx in FIXTURES {
        let path = dir.join(fx.name);
        let text = std::fs::read_to_string(&path).expect("fixture readable");

        // Native side: must match the captured expectation.
        let got = native_rejects(&text, &path);
        assert_eq!(
            got,
            fx.native_rejects,
            "[{}] native verdict for {} (rule {})",
            if got { "reject" } else { "accept" },
            fx.name,
            fx.rule
        );

        // Live cross-check (only when the toolchain is present): the native
        // reject direction must correspond to a compactc post-parse rejection,
        // and accept to a compactc accept. Wording/span are NOT compared.
        if let Some(tc) = &tc {
            let verdict = compiler_verdict(tc, &path);
            let expected = if fx.native_rejects {
                CompilerVerdict::RejectPostParse
            } else {
                CompilerVerdict::Accept
            };
            assert_eq!(
                verdict, expected,
                "compactc verdict for {} (rule {})",
                fx.name, fx.rule
            );
        } else {
            eprintln!(
                "type_differential: compactc absent; skipped live cross-check for {}",
                fx.name
            );
        }
    }
}
