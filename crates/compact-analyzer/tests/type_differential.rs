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
    Fixture {
        name: "uint_in_range_ok.compact",
        native_rejects: false,
        rule: "uint-lattice",
    },
    Fixture {
        name: "uint_over_range.compact",
        native_rejects: true,
        rule: "uint-lattice",
    },
    Fixture {
        name: "uint_bitwidth_max_ok.compact",
        native_rejects: false,
        rule: "uint-lattice",
    },
    Fixture {
        name: "uint_bitwidth_over.compact",
        native_rejects: true,
        rule: "uint-lattice",
    },
    Fixture {
        name: "uint_literal_into_field_ok.compact",
        native_rejects: false,
        rule: "uint-lattice",
    },
    Fixture {
        name: "boolean_into_uint.compact",
        native_rejects: true,
        rule: "uint-lattice",
    },
    Fixture {
        name: "uint_octal_over_range.compact",
        native_rejects: true,
        rule: "uint-lattice",
    },
    Fixture {
        name: "cast_into_wider_uint_ok.compact",
        native_rejects: false,
        rule: "cast-primitives",
    },
    Fixture {
        name: "cast_into_narrower_uint.compact",
        native_rejects: true,
        rule: "cast-primitives",
    },
    Fixture {
        name: "cast_bytes_into_field_ok.compact",
        native_rejects: false,
        rule: "cast-primitives",
    },
    Fixture {
        name: "illegal_cast_bool_to_bytes.compact",
        native_rejects: true,
        rule: "cast-primitives",
    },
    Fixture {
        name: "bytes_return_ok.compact",
        native_rejects: false,
        rule: "cast-primitives",
    },
    Fixture {
        name: "bytes_return_size_mismatch.compact",
        native_rejects: true,
        rule: "cast-primitives",
    },
    Fixture {
        name: "uint_literal_into_bytes.compact",
        native_rejects: true,
        rule: "cast-primitives",
    },
    Fixture {
        name: "generic_type_ok.compact",
        native_rejects: false,
        rule: "generic-specialization",
    },
    Fixture {
        name: "generic_type_missing_args.compact",
        native_rejects: true,
        rule: "generic-specialization",
    },
    Fixture {
        name: "generic_type_wrong_count.compact",
        native_rejects: true,
        rule: "generic-specialization",
    },
    Fixture {
        name: "generic_args_on_nongeneric.compact",
        native_rejects: true,
        rule: "generic-specialization",
    },
    Fixture {
        name: "nongeneric_type_ok.compact",
        native_rejects: false,
        rule: "generic-specialization",
    },
    Fixture {
        name: "generic_type_nested_ok.compact",
        native_rejects: false,
        rule: "generic-specialization",
    },
    Fixture {
        name: "vec_covariant_ok.compact",
        native_rejects: false,
        rule: "tuple-vector-covariance",
    },
    Fixture {
        name: "vec_len_mismatch.compact",
        native_rejects: true,
        rule: "tuple-vector-covariance",
    },
    Fixture {
        name: "vec_elem_over_range.compact",
        native_rejects: true,
        rule: "tuple-vector-covariance",
    },
    Fixture {
        name: "tuple_covariant_ok.compact",
        native_rejects: false,
        rule: "tuple-vector-covariance",
    },
    Fixture {
        name: "tuple_arity_mismatch.compact",
        native_rejects: true,
        rule: "tuple-vector-covariance",
    },
    Fixture {
        name: "tuple_elem_mismatch.compact",
        native_rejects: true,
        rule: "tuple-vector-covariance",
    },
    Fixture {
        name: "vec_tuple_equiv_ok.compact",
        native_rejects: false,
        rule: "tuple-vector-covariance",
    },
    Fixture {
        name: "nested_seq_ok.compact",
        native_rejects: false,
        rule: "tuple-vector-covariance",
    },
];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/type")
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

/// Binary corpus gate (foundation direction: no false positives). For every
/// corpus file `compactc` ACCEPTS at the type phase, the native checker must
/// emit zero type diagnostics. Files compactc rejects post-parse are counted
/// and reported but not asserted against native rejection — the full
/// biconditional is the v2b release gate, reached as rules land. Skips when
/// the toolchain or corpus is absent.
#[test]
fn corpus_no_false_positives() {
    let Some(tc) = Toolchain::discover(None) else {
        eprintln!("corpus gate SKIPPED: compactc absent");
        return;
    };
    let Some(dir) = corpus_dir() else {
        eprintln!("corpus gate SKIPPED: no COMPACT_CORPUS_DIR and no ../compactp checkout");
        return;
    };
    let files = analyzer_core::discover_compact_files(&[dir]);
    assert!(
        files.len() > 100,
        "expected a large corpus, got {}",
        files.len()
    );

    let mut accepted = 0usize;
    let mut compiler_rejected = 0usize;
    let mut indeterminate = 0usize;
    let mut false_positives: Vec<PathBuf> = Vec::new();

    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match compiler_verdict(&tc, &path) {
            CompilerVerdict::Accept => {
                accepted += 1;
                if native_rejects(&text, &path) {
                    false_positives.push(path);
                }
            }
            CompilerVerdict::RejectParse | CompilerVerdict::Indeterminate => indeterminate += 1,
            CompilerVerdict::RejectPostParse => compiler_rejected += 1,
        }
    }

    eprintln!(
        "corpus gate: accepted={accepted} compiler_rejected(post-parse)={compiler_rejected} indeterminate={indeterminate} false_positives={}",
        false_positives.len()
    );
    assert!(
        accepted > 0,
        "corpus gate exercised no compactc-accepted files (accepted={accepted}, \
         compiler_rejected={compiler_rejected}, indeterminate={indeterminate}); \
         the toolchain or invocation may be misconfigured"
    );
    assert!(
        false_positives.is_empty(),
        "native emitted type diagnostics on {} file(s) compactc accepts (false positives): {:?}",
        false_positives.len(),
        false_positives
    );
}
