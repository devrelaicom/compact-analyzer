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
//! **Green pending-flip model.** The native side
//! (`AnalysisHost::disclosure_diagnostics`) is the fully-landed intraprocedural
//! WPP analyzer. `FIXTURES` carries one compiler-validated fixture per WPP rule
//! from the R0 index. Each fixture tracks two verdicts: `discloses` (compactc's
//! ground truth, validated here against real `compactc`) and `native_confirms`
//! (whether the native analyzer emits a confirmed E-family leak). Every v3a task
//! flipped its own fixtures' `native_confirms` from `false` to `true` as it
//! implemented that rule, keeping the harness green at every step (rather than
//! committing a red baseline). Deferred/version-gated cases (e.g. cross-contract
//! calls) intentionally keep `native_confirms: false` — the analyzer fails closed
//! to an amber U-family advisory there, which is excluded from `native_discloses`.

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

/// True if native reports SOMETHING at this file — a confirmed E-leak (>=3100) OR any U-family
/// advisory. Used by the reject-direction parity gate: on a file compactc rejects for disclosure,
/// native must not be silent-green (§0).
fn native_reports_something(text: &str, path: &Path) -> bool {
    let mut host = AnalysisHost::new();
    let file: FileId = host.vfs_mut().file_id(path);
    host.vfs_mut().set_overlay(file, text.to_string(), 1);
    host.disclosure_diagnostics(file)
        .iter()
        .any(|d| (d.code.prefix == "E" && d.code.number >= 3100) || d.code.prefix == "U")
}

/// `compactc`'s disclosure verdict for one source file on disk: `Some(true)`
/// on a post-parse reject whose stderr carries the WPP banner, `Some(false)`
/// ONLY on a clean compile, `None` otherwise (a parse rejection, a
/// post-parse reject WITHOUT the banner, or an indeterminate outcome —
/// usage error, timeout, cancellation). The WPP pass runs over a late IR
/// (`Lwithpaths`); if compactc bails on an unrelated (e.g. type) error
/// before that IR is built, "no banner" is NOT evidence the file is
/// disclosure-clean — the pass may never have run — so that case is
/// indeterminate, not a confirmed non-disclosure.
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
        CompilerVerdict::RejectPostParse => outcome.stderr.contains(WPP_BANNER).then_some(true),
        CompilerVerdict::RejectParse | CompilerVerdict::Indeterminate => None,
    }
}

struct Fixture {
    name: &'static str,
    rule: &'static str,
    /// compactc's ground-truth WPP verdict (validated against real compactc).
    discloses: bool,
    /// Whether the CURRENT native analyzer emits a confirmed E-leak.
    /// Starts false (native is a no-op); each v3a task flips its fixtures.
    native_confirms: bool,
}

/// Rule-tagged fixtures pinning each WPP rule from the R0 index
/// (`docs/superpowers/plans/2026-07-16-v3-wpp-rules-index.md`), authored and
/// compiler-validated by Task R2. `discloses` is compactc's real WPP
/// verdict; `native_confirms` is `false` for every fixture (native is still
/// a no-op) until v3a's interpreter tasks flip them one by one.
const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "ledger_leak.compact",
        rule: "K1 ledger sink",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "ledger_disclosed.compact",
        rule: "K1 ledger sink + D1 disclose sanitizer",
        discloses: false,
        native_confirms: false,
    },
    Fixture {
        name: "return_witness_leak.compact",
        rule: "K7 return asymmetry (witness-return leaks)",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "return_arg_ok.compact",
        rule: "K7 return asymmetry (circuit-arg does not leak)",
        discloses: false,
        native_confirms: false,
    },
    Fixture {
        name: "implicit_flow_leak.compact",
        rule: "K2 implicit flow at ledger sink",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "implicit_flow_disclosed.compact",
        rule: "K2 implicit flow + D1 disclose sanitizer",
        discloses: false,
        native_confirms: false,
    },
    Fixture {
        name: "hash_then_leak.compact",
        rule: "N2 persistentHash conduit → K1 ledger sink (A6)",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "commit_hides_ok.compact",
        rule: "N2 persistentCommit sanitizer (both args hidden) (A6)",
        discloses: false,
        native_confirms: false,
    },
    Fixture {
        name: "container_op_leak.compact",
        rule: "L2 container-op witness arg leaks (Set.insert) (A6)",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "constructor_arg_leak.compact",
        rule: "S2 constructor-argument source",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "comingled_return.compact",
        rule: "K7 asymmetry edge case (co-mingled return)",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "struct_field_projection_ok.compact",
        rule: "P5 precise name-bound struct member projection (clean field)",
        discloses: false,
        native_confirms: false,
    },
    Fixture {
        name: "struct_field_projection_out_of_order_ok.compact",
        rule: "P5 declared-order projection (out-of-order named literal, clean field)",
        discloses: false,
        native_confirms: false,
    },
    Fixture {
        name: "map_disclose_ok.compact",
        rule: "FX2 map with disclosing lambda over a circuit-arg source (A7)",
        discloses: false,
        native_confirms: false,
    },
    Fixture {
        name: "fold_disclose_ok.compact",
        rule: "FX1 fold with disclosing lambda over a circuit-arg source (A7)",
        discloses: false,
        native_confirms: false,
    },
    Fixture {
        name: "module_reexport_ok.compact",
        rule: "AMOD module fail-closed (module-nested circuit not a root + unresolved module-imported callee → amber)",
        discloses: false,
        native_confirms: false,
    },
    Fixture {
        name: "interproc_path.compact",
        rule: "B3 cross-circuit argument path point (witness → helper arg → ledger sink)",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "module_circuit_leak.compact",
        rule: "B4 re-exported module circuit becomes a disclosing root (arg → ledger)",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "module_call_leak.compact",
        rule: "B4 top-level export calls a module-imported circuit that leaks",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "interproc_call_leak.compact",
        rule: "B2 cross-circuit call -> ledger sink",
        discloses: true,
        native_confirms: true,
    },
    Fixture {
        name: "interproc_call_ok.compact",
        rule: "B2 callee disclose() sanitizes",
        discloses: false,
        native_confirms: false,
    },
];

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

        // Native side: must match the CURRENT native-confirms expectation.
        let got = native_discloses(&text, &path);
        assert_eq!(
            got,
            fx.native_confirms,
            "[{}] native verdict for {} (rule {})",
            if got { "confirm" } else { "silent" },
            fx.name,
            fx.rule
        );

        // Live cross-check (only when the toolchain is present): compactc's
        // real WPP verdict must match the fixture's validated `discloses`.
        // Wording/span are NOT compared.
        if let Some(tc) = &tc {
            match compiler_discloses(tc, &path) {
                Some(discloses) => assert_eq!(
                    discloses, fx.discloses,
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
/// corpus file compactc ACCEPTS outright (`compiler_discloses` ==
/// `Some(false)`, i.e. the WPP pass provably ran and found nothing — see
/// that function's doc comment for why a post-parse reject is excluded
/// rather than counted as clean), the native checker must emit zero
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

    let mut accepted = 0usize;
    let mut confirmed_leak = 0usize;
    let mut indeterminate = 0usize;
    let mut false_positives: Vec<PathBuf> = Vec::new();
    let mut silent_on_reject: Vec<PathBuf> = Vec::new();

    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match compiler_discloses(&tc, &path) {
            Some(false) => {
                accepted += 1;
                if native_discloses(&text, &path) {
                    false_positives.push(path);
                }
            }
            Some(true) => {
                confirmed_leak += 1;
                if !native_reports_something(&text, &path) {
                    silent_on_reject.push(path);
                }
            }
            None => indeterminate += 1,
        }
    }

    eprintln!(
        "disclosure corpus gate: accepted={accepted} compiler_confirmed_leak={confirmed_leak} indeterminate={indeterminate} false_positives={} silent_on_reject={}",
        false_positives.len(),
        silent_on_reject.len()
    );
    assert!(
        accepted > 0,
        "disclosure corpus gate exercised no compactc-accepted files (accepted={accepted}, \
         confirmed_leak={confirmed_leak}, indeterminate={indeterminate}); \
         the toolchain or invocation may be misconfigured"
    );
    assert!(
        false_positives.is_empty(),
        "native emitted a confirmed-leak diagnostic on {} file(s) compactc accepts \
         (false positives): {:?}",
        false_positives.len(),
        false_positives
    );
    assert!(
        silent_on_reject.is_empty(),
        "native was silent (no E>=3100 leak, no U advisory) on {} file(s) compactc rejects for \
         disclosure (§0 fail-closed violation — silent green on a WPP reject): {:?}",
        silent_on_reject.len(),
        silent_on_reject
    );
}

// --- Cross-file (XF1) -------------------------------------------------------
//
// A multi-file fixture is a *directory* holding `main.compact` plus the
// module file(s) it imports via `import "./libB"`. compactc anchors a
// cross-file disclosure at the SINK in the callee file; the native analyzer
// takes the pragmatic scope and anchors at the CALL SITE in `main` — both
// REPORT the leak, and the differential compares PRESENCE (any E>=3100), not
// span, so they agree. `compiler_discloses` needs no change: it compiles
// `main.compact` in place, and the relative import resolves to its sibling.

/// Native cross-file verdict: warm the workspace by discovering+indexing every
/// `.compact` under `dir` (so `main`'s `import "./libB"` resolves to the
/// sibling and libB's `SourceText` enters `Workspace.file_srcs` — exactly how
/// the LSP server builds the workspace on init), then does `main.compact`
/// disclose a confirmed leak? Files are read from disk (the fixtures are real
/// on-disk files), so no VFS overlay is needed.
fn native_discloses_multifile(dir: &Path) -> bool {
    let mut host = AnalysisHost::new();
    host.discover_and_index(std::slice::from_ref(&dir.to_path_buf()), &|| true);
    let main = host.vfs_mut().file_id(&dir.join("main.compact"));
    host.disclosure_diagnostics(main)
        .iter()
        .any(|d| d.code.prefix == "E" && d.code.number >= 3100)
}

struct MultiFixture {
    dir: &'static str,
    /// compactc's WPP verdict on `main.compact` (validated against real compactc).
    discloses: bool,
    /// Whether the native analyzer emits a confirmed E-leak.
    native_confirms: bool,
}

/// Multi-file fixtures pinning the cross-file circuit-resolution rule (XF1)
/// against real compactc. Both were validated by compiling `main.compact` in
/// place (the `import "./libB"` resolves to the sibling `libB.compact`).
const MULTI_FIXTURES: &[MultiFixture] = &[
    MultiFixture {
        dir: "crossfile_call_leak",
        discloses: true,
        native_confirms: true,
    },
    MultiFixture {
        dir: "crossfile_call_ok",
        discloses: false,
        native_confirms: false,
    },
];

#[test]
fn crossfile_disclosure_differential() {
    let dir = fixtures_dir();
    let tc = Toolchain::discover(None);
    for fx in MULTI_FIXTURES {
        let fxdir = dir.join(fx.dir);
        let main = fxdir.join("main.compact");

        // Native side (always asserted).
        let got = native_discloses_multifile(&fxdir);
        assert_eq!(
            got, fx.native_confirms,
            "native cross-file verdict for {} (expected native_confirms={})",
            fx.dir, fx.native_confirms
        );

        // Live cross-check: compactc compiles `main.compact` in place; the
        // relative import pulls in the sibling `libB.compact`. Wording/span
        // are NOT compared (compactc anchors at the sink, native at the call
        // site) — only the accept/reject verdict.
        if let Some(tc) = &tc {
            match compiler_discloses(tc, &main) {
                Some(discloses) => assert_eq!(
                    discloses, fx.discloses,
                    "compactc cross-file verdict for {}",
                    fx.dir
                ),
                None => eprintln!(
                    "crossfile_disclosure_differential: compactc indeterminate for {}",
                    fx.dir
                ),
            }
        } else {
            eprintln!(
                "crossfile_disclosure_differential: compactc absent; skipped live cross-check for {}",
                fx.dir
            );
        }
    }
}
