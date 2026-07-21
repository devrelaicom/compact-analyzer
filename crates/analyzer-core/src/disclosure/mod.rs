//! Native witness-disclosure ("Witness Protection Program" / WPP) analysis
//! (v3 spec §3, R0 index `docs/superpowers/plans/2026-07-16-v3-wpp-rules-index.md`).
//!
//! Re-implements the compactc `track-witness-data` abstract interpreter
//! (`compactc-v0.31.1`, commit `0da5b045`) natively so the editor can surface
//! disclosure diagnostics without shelling out to the compiler. FAIL-CLOSED
//! is the program invariant (spec §0): any surface the analyzer cannot fully
//! decide (an unknown type, an unhandled expression, an unknown native or
//! ledger op) records an amber advisory (`U3100`+) rather than silently
//! reporting no leak. Confirmed leaks are `E3100`+.
//!
//! - `abs` — the `Abs` abstract-value lattice (A1).
//! - `interp` — root discovery + type-driven source seeding (A2), the
//!   interpreter walk (A3), and the ledger/return SINKS (A4).
//! - `leaks` — the `Advisory`/`DisclosureSink` fail-closed primitive (A2) plus
//!   the leak table (A4).
//! - `version` — the pinned-compiler-version drift check (v3c C5, spec §4.3):
//!   the tables above are only valid for the compactc release they were
//!   extracted from, so a mismatched installed compiler gets its own
//!   surfaced advisory (wired by `compact-analyzer`'s server, not here).
//!
//! `disclosure_diagnostics_query` (below) runs the interpreter from every root
//! (A4): it seeds sources, walks each body, drains the leak table, and renders
//! each confirmed leak as an `E3100` diagnostic. It also drains the
//! accumulated fail-closed advisories (deduped by `(src, reason)`) into amber
//! `U3100` diagnostics (A8) — so the query returns BOTH confirmed E-leaks and
//! unverified U-advisories.

mod abs;
mod interp;
mod leaks;
mod tables;
mod version;

use std::sync::Arc;

use compactp_diagnostics::{Diagnostic, DiagnosticCode};

use crate::db::{Db, FileDeps, SourceText, Workspace, item_tree};
use interp::InterpCtx;
use leaks::{DisclosureLeak, DisclosureSink, advisory_to_diagnostic};
pub use version::{PINNED_COMPILER_VERSION, version_mismatch};

/// Disclosure (WPP) diagnostics for `src`: the confirmed witness-disclosure
/// leaks (`E3100`) the intraprocedural interpreter finds, PLUS the amber
/// fail-closed advisories (`U3100`, spec §0) it couldn't fully decide.
/// Discovers the roots (exported circuits + constructor), seeds each one's
/// sources, walks its body into a shared leak table + advisory set, then
/// drains both: leaks render as `E3100` diagnostics, advisories (deduped by
/// `(src, reason)`) render as `U3100` diagnostics. Advisories NEVER replace a
/// confirmed leak — both channels are appended to the same returned set.
#[salsa::tracked(returns(clone), no_eq)]
pub fn disclosure_diagnostics_query(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
) -> Arc<[Diagnostic]> {
    let tree = item_tree(db, src);
    let mut sink = DisclosureSink::new();
    let base = InterpCtx {
        db,
        file,
        src,
        fd,
        ws,
        disclosing_fn: None,
    };
    let roots = interp::discover_roots(&base, &tree, &mut sink);
    for root in &roots {
        interp::interp_root(&base, &mut sink, root);
    }
    let mut diags: Vec<Diagnostic> = sink
        .drain_leaks()
        .into_iter()
        .map(leak_to_diagnostic)
        .collect();
    diags.extend(sink.drain_advisories().iter().map(advisory_to_diagnostic));
    Arc::from(diags)
}

/// Renders one confirmed leak as an `E3100` diagnostic (spec §3.7). The primary
/// span is the sink; each witness contributes a secondary span at its origin,
/// plus one per (deduped, see below) `PathPoint` on its witness->sink trail
/// (source anchor + intermediate exposures), so the editor can walk the
/// disclosure path. Entirely DISPLAY-ONLY: nothing here touches
/// `record_leak`/the leak table, so the confirmed `E3100` COUNT this produces
/// is always identical to the raw leak table's (§0).
///
/// Native/stdlib-collapse (v3c C4) is achieved BY CONSTRUCTION, not by any
/// code here: stdlib/builtin primitives classify as `CalleeClass::Native`
/// (`interp.rs`) and their bodies are NEVER walked -- natives are conduits
/// looked up by name in a static table (`tables::native_conduit`), not
/// interpreted ASTs -- so every `PathPoint::src` this function ever sees is
/// already a range in the file under analysis; there is no stdlib-internal
/// location to collapse. The plan's projected R0 K8 stdlib-entry-site
/// remapping only becomes necessary if a future change makes the interpreter
/// walk stdlib circuit bodies -- add that remapping here if it ever does.
fn leak_to_diagnostic(leak: DisclosureLeak) -> Diagnostic {
    // XF1 cross-file leak: `leak.src` is the CALL SITE in the analyzed file (the
    // pragmatic single-file model). Render primary-only, naming the callee — NO
    // secondary spans: the witness/path trail points into the OTHER file, which
    // has no FileId in the current single-file span model, so its ranges would be
    // out-of-range on the analyzed file. Same-file leaks (`cross_file: None`)
    // render with the full trail below, exactly as before.
    if let Some(callee) = &leak.cross_file {
        return Diagnostic::error(
            DiagnosticCode::new("E", 3100),
            format!(
                "potential witness-value disclosure via cross-file call to `{callee}`: {}",
                leak.nature
            ),
            leak.src,
        );
    }
    let mut diag = Diagnostic::error(
        DiagnosticCode::new("E", 3100),
        format!("potential witness-value disclosure: {}", leak.nature),
        leak.src,
    );
    for w in &leak.witnesses {
        diag = diag.with_secondary(w.src, Some(witness_origin_label(&w.info)));
        // Collapse CONSECUTIVE DUPLICATE path points (same src+description+
        // exposure): `merge_witnesses` (abs.rs, R0 F2) concatenates two
        // accumulated paths on a same-uid collision, which can legitimately
        // repeat the exact same point back to back in the RAW leak table --
        // e.g. a `const`-bound witness used unchanged in BOTH arms of a
        // non-constant ternary joins with itself, doubling its one-entry
        // path (pinned by
        // `interp::tests::ternary_same_witness_both_branches_yields_duplicate_path_point`).
        // Collapsing it here is purely cosmetic: `leak.src`/`leak.nature`/the
        // witness SET are untouched.
        let mut prev: Option<&abs::PathPoint> = None;
        for pp in &w.path {
            if prev == Some(pp) {
                continue;
            }
            let label = if pp.exposure.is_empty() {
                pp.description.clone()
            } else {
                format!("{} {}", pp.exposure, pp.description)
            };
            diag = diag.with_secondary(pp.src, Some(label));
            prev = Some(pp);
        }
    }
    diag
}

/// A human-readable label for a witness origin (spec §3.1), mirroring compactc's
/// "witness value potentially disclosed: <origin>" wording (R2 transcripts).
fn witness_origin_label(info: &abs::WitnessInfo) -> String {
    match info {
        abs::WitnessInfo::WitnessReturn { function } => {
            format!("the return value of witness {function}")
        }
        abs::WitnessInfo::ConstructorArg { arg } => {
            format!("the value of parameter {arg} of the constructor")
        }
        abs::WitnessInfo::CircuitArg { function, arg } => {
            format!("the value of parameter {arg} of circuit {function}")
        }
    }
}

impl crate::AnalysisHost {
    /// Disclosure (WPP) diagnostics for `file`. Thin bridge over the tracked
    /// `disclosure_diagnostics_query`, mirroring the resolution-fed bridges: it
    /// indexes the file to materialize its dependency edges and provisions the
    /// salsa inputs (`FileDeps`/`Workspace`) the interpreter's resolution needs.
    pub fn disclosure_diagnostics(&mut self, file: crate::FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        self.ensure_indexed(file);
        let fd = self.file_deps(file);
        let ws = self.workspace();
        disclosure_diagnostics_query(self.db_ref(), file, src, fd, ws).to_vec()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::CompactDatabase;

    fn empty_ws(db: &CompactDatabase) -> Workspace {
        Workspace::new(
            db,
            None,
            Arc::new(std::collections::BTreeMap::new()),
            Arc::new(std::collections::BTreeMap::new()),
        )
    }

    /// Builds a two-file workspace: `main` (FileId 0) importing `libB`
    /// (FileId 1) via `import "./libB";`, mirroring XF0's differential fixture.
    /// Returns the inputs to drive `disclosure_diagnostics_query` on `main`.
    /// `main`'s import edge always carries the callee target (so `classify_callee`
    /// resolves `stash` to `libB`'s FileId); `register_lib` controls whether
    /// `libB` is present in the workspace `file_srcs`/`file_deps` maps — set it
    /// `false` to exercise the §0 fail-closed case (callee absent from the
    /// workspace).
    fn two_file_ws(
        db: &CompactDatabase,
        main_text: &str,
        lib_text: &str,
        register_lib: bool,
    ) -> (crate::FileId, SourceText, FileDeps, Workspace) {
        use crate::db::ResolvedDep;
        let main_file = crate::FileId::from_raw_for_test(0);
        let lib_file = crate::FileId::from_raw_for_test(1);
        let main_src = SourceText::new(db, Arc::from(main_text));
        let lib_src = SourceText::new(db, Arc::from(lib_text));
        // main's resolved import edge `./libB` -> the libB file. Kept even in the
        // fail-closed case so the NAME resolves (classification succeeds) while
        // the workspace map lookup that `resolve_circuit_def` performs is what
        // fails.
        let main_fd = FileDeps::new(
            db,
            Arc::from(vec![ResolvedDep {
                raw: "./libB".to_string(),
                is_include: false,
                target: Some((lib_file, lib_src)),
            }]),
        );
        let lib_fd = FileDeps::new(db, Arc::from(Vec::new()));
        let mut deps_map = std::collections::BTreeMap::new();
        let mut srcs_map = std::collections::BTreeMap::new();
        deps_map.insert(main_file, main_fd);
        srcs_map.insert(main_file, main_src);
        if register_lib {
            deps_map.insert(lib_file, lib_fd);
            srcs_map.insert(lib_file, lib_src);
        }
        let ws = Workspace::new(db, None, Arc::new(deps_map), Arc::new(srcs_map));
        (main_file, main_src, main_fd, ws)
    }

    const LIB_STASH: &str = "module libB {\n\
                             export ledger store: Field;\n\
                             export circuit stash(v: Field): [] { store = v; }\n\
                             }\n";

    fn e_leaks(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
        diags
            .iter()
            .filter(|d| d.code.prefix == "E" && d.code.number >= 3100)
            .collect()
    }

    /// XF1 test 1 — a witness flowing through a CROSS-FILE circuit call to a
    /// ledger sink in the other file is CAUGHT, and (pragmatic call-site model)
    /// anchored at the CALL SITE in the analyzed file, not at the sink in libB.
    /// Ground truth: XF0 — compactc 0.31.1 REJECTs this exact two-file program.
    #[test]
    fn cross_file_call_leak_caught_and_anchored_at_call_site() {
        let db = CompactDatabase::default();
        let main_text = "import \"./libB\";\n\
                         witness getW(): Field;\n\
                         export circuit f(): [] { stash(getW()); }\n";
        let (file, src, fd, ws) = two_file_ws(&db, main_text, LIB_STASH, true);

        let diags = disclosure_diagnostics_query(&db, file, src, fd, ws);
        let leaks = e_leaks(&diags);
        assert_eq!(
            leaks.len(),
            1,
            "a witness crossing a cross-file circuit call to a ledger sink must confirm one leak: {diags:?}"
        );
        let leak = leaks[0];

        // Primary span is the CALL SITE in main (`stash(getW())`), NOT a range
        // inside libB.
        let span_text = &main_text
            [usize::from(leak.primary_span.start())..usize::from(leak.primary_span.end())];
        assert_eq!(
            span_text.trim(),
            "stash(getW())",
            "cross-file leak must anchor at the call site in the analyzed file"
        );
        assert!(
            leak.message.contains("cross-file call to `stash`"),
            "message must name the cross-file callee: {:?}",
            leak.message
        );
        // Primary-only: the witness/path trail would point into libB (no FileId
        // in the single-file span model), so no secondary spans are rendered.
        assert!(
            leak.secondary_spans.is_empty(),
            "cross-file leak renders primary-only: {:?}",
            leak.secondary_spans
        );
    }

    /// XF1 test 2 — `disclose()` at the cross-file call argument sanitizes the
    /// flow: no leak. Ground truth: XF0 Q4 — compactc 0.31.1 ACCEPTs it.
    #[test]
    fn cross_file_call_disclose_at_arg_sanitizes() {
        let db = CompactDatabase::default();
        let main_text = "import \"./libB\";\n\
                         witness getW(): Field;\n\
                         export circuit f(): [] { stash(disclose(getW())); }\n";
        let (file, src, fd, ws) = two_file_ws(&db, main_text, LIB_STASH, true);

        let diags = disclosure_diagnostics_query(&db, file, src, fd, ws);
        assert!(
            e_leaks(&diags).is_empty(),
            "disclose() at the cross-file call argument must sanitize the flow: {diags:?}"
        );
    }

    /// XF1 test 3 — §0 fail-closed: the callee resolves to a FileId absent from
    /// the workspace `file_srcs`, so `resolve_circuit_def` can't fetch its
    /// source. The analyzer must fail closed to an amber advisory and NEVER
    /// manufacture a false-positive E-leak nor silently report green.
    #[test]
    fn cross_file_callee_absent_from_workspace_fails_closed_amber() {
        let db = CompactDatabase::default();
        let main_text = "import \"./libB\";\n\
                         witness getW(): Field;\n\
                         export circuit f(): [] { stash(getW()); }\n";
        let (file, src, fd, ws) = two_file_ws(&db, main_text, LIB_STASH, false);

        let diags = disclosure_diagnostics_query(&db, file, src, fd, ws);
        assert!(
            e_leaks(&diags).is_empty(),
            "an unreachable callee must never manufacture a false-positive red: {diags:?}"
        );
        let advisories: Vec<_> = diags
            .iter()
            .filter(|d| d.code.prefix == "U" && d.code.number == 3100)
            .collect();
        assert!(
            !advisories.is_empty(),
            "an unreachable cross-file callee must record an amber advisory (never silent green): {diags:?}"
        );
    }

    /// End-to-end (A8): a module-nested `export circuit` is a fail-closed
    /// point (A-MOD) — excluded from the root set, recording an amber
    /// advisory rather than silently reporting no leak. The query must
    /// render that advisory as a `U3100` diagnostic, and must NOT emit any
    /// confirmed `E3100` leak for the ledger write inside it: an advisory
    /// never manufactures a false E-leak, so the differential harness's
    /// `native_discloses` (which only counts `E`-family >= 3100) stays
    /// `false` for this file.
    #[test]
    fn module_nested_circuit_renders_amber_advisory_not_a_leak() {
        let db = CompactDatabase::default();
        let text = "import CompactStandardLibrary;\n\
                     module M {\n\
                     export ledger c: Field;\n\
                     export circuit leak(x: Field): Field { c = x; return x; }\n\
                     }\n";
        let src = SourceText::new(&db, Arc::from(text));
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        let ws = empty_ws(&db);
        let file = crate::FileId::from_raw_for_test(0);

        let diags = disclosure_diagnostics_query(&db, file, src, fd, ws);

        let e_leaks: Vec<_> = diags
            .iter()
            .filter(|d| d.code.prefix == "E" && d.code.number >= 3100)
            .collect();
        assert!(
            e_leaks.is_empty(),
            "advisory must not manufacture a confirmed E-leak: {e_leaks:?}"
        );

        let advisories: Vec<_> = diags
            .iter()
            .filter(|d| d.code.prefix == "U" && d.code.number == 3100)
            .collect();
        assert_eq!(
            advisories.len(),
            1,
            "expected exactly one deduped U3100 advisory, got {diags:?}"
        );
        assert_eq!(
            advisories[0].severity,
            compactp_diagnostics::Severity::Warning
        );
        assert!(
            advisories[0]
                .message
                .contains("module-nested circuit `leak`")
        );
    }

    /// Regression pin (v3b B6, spec §8 risk #6): `disclosure_diagnostics_query`
    /// is `#[salsa::tracked]` keyed on `(file, src, fd, ws)`. Editing `src`'s
    /// text via `set_text` (the same salsa input, a new revision) must
    /// recompute rather than serve a stale memo when the edit changes a
    /// source's IDENTITY -- here, `getW` flips from a `witness` (a taint
    /// source: its return value is witness-controlled) to a plain `circuit`
    /// (clean). Ground truth validated against `compact compile` 0.5.1
    /// (compactc 0.31.1): program A REJECTs ("potential witness-value
    /// disclosure... the return value of witness getW"), program B ACCEPTs.
    #[test]
    fn witness_to_circuit_flip_clears_leak_on_recompute() {
        use salsa::Setter as _;

        let mut db = CompactDatabase::default();
        let text_a = "import CompactStandardLibrary;\n\
                       export ledger c: Uint<8>;\n\
                       witness getW(): Uint<8>;\n\
                       export circuit f(): [] { c = getW(); }\n";
        let src = SourceText::new(&db, Arc::from(text_a));
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        let ws = empty_ws(&db);
        let file = crate::FileId::from_raw_for_test(0);

        let diags_a = disclosure_diagnostics_query(&db, file, src, fd, ws);
        let leaks_a: Vec<_> = diags_a
            .iter()
            .filter(|d| d.code.prefix == "E" && d.code.number >= 3100)
            .collect();
        assert_eq!(
            leaks_a.len(),
            1,
            "witness getW() written to the ledger must confirm a leak: {diags_a:?}"
        );

        // Edit: `witness getW(): Uint<8>;` -> `circuit getW(): Uint<8> { return 0; }`.
        let text_b = "import CompactStandardLibrary;\n\
                       export ledger c: Uint<8>;\n\
                       circuit getW(): Uint<8> { return 0; }\n\
                       export circuit f(): [] { c = getW(); }\n";
        src.set_text(&mut db).to(Arc::from(text_b));

        let diags_b = disclosure_diagnostics_query(&db, file, src, fd, ws);
        let leaks_b: Vec<_> = diags_b
            .iter()
            .filter(|d| d.code.prefix == "E" && d.code.number >= 3100)
            .collect();
        assert!(
            leaks_b.is_empty(),
            "getW flipped to a plain circuit must clear the leak on recompute (not serve a stale memo): {diags_b:?}"
        );
    }

    /// Regression pin (v3b B6, spec §8 risk #6): adding an exported-circuit
    /// parameter re-seeds a fresh `CircuitArg` source on recompute, turning a
    /// previously-clean ledger write into a confirmed leak. Ground truth
    /// validated against `compact compile` 0.5.1 (compactc 0.31.1): program A
    /// (writes a constant) ACCEPTs, program B (writes the new parameter)
    /// REJECTs ("the value of parameter p of exported circuit f").
    #[test]
    fn added_circuit_param_introduces_leak_on_recompute() {
        use salsa::Setter as _;

        let mut db = CompactDatabase::default();
        let text_a = "import CompactStandardLibrary;\n\
                       export ledger store: Field;\n\
                       export circuit f(): [] { store = 0; }\n";
        let src = SourceText::new(&db, Arc::from(text_a));
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        let ws = empty_ws(&db);
        let file = crate::FileId::from_raw_for_test(0);

        let diags_a = disclosure_diagnostics_query(&db, file, src, fd, ws);
        let leaks_a: Vec<_> = diags_a
            .iter()
            .filter(|d| d.code.prefix == "E" && d.code.number >= 3100)
            .collect();
        assert!(
            leaks_a.is_empty(),
            "writing a constant to the ledger must not leak: {diags_a:?}"
        );

        // Edit: add parameter `p` and write it instead of the constant.
        let text_b = "import CompactStandardLibrary;\n\
                       export ledger store: Field;\n\
                       export circuit f(p: Field): [] { store = p; }\n";
        src.set_text(&mut db).to(Arc::from(text_b));

        let diags_b = disclosure_diagnostics_query(&db, file, src, fd, ws);
        let leaks_b: Vec<_> = diags_b
            .iter()
            .filter(|d| d.code.prefix == "E" && d.code.number >= 3100)
            .collect();
        assert_eq!(
            leaks_b.len(),
            1,
            "the new CircuitArg source `p` written to the ledger must confirm a leak on recompute: {diags_b:?}"
        );
    }

    /// Pins path-rendering fidelity + the display-only property (v3c C4) for
    /// a multi-hop leak: the same program as
    /// `crates/compact-analyzer/tests/fixtures/disclosure/hash_then_leak.compact`
    /// (compiler-validated: compactc 0.31.1 REJECTs, WPP banner naming
    /// `getW`) -- witness `getW` -> `persistentHash` N2 conduit ("a hash
    /// of") -> ledger-write K1 sink.
    ///
    /// As-merged drift from the plan (see `.superpowers/sdd/task-C4-brief.md`):
    /// the plan projected a "stdlib-collapse" of stdlib-INTERNAL path points
    /// (R0 K8) onto their stdlib entry site. That collapse target does not
    /// exist in the merged interpreter -- stdlib/builtin primitives classify
    /// as `CalleeClass::Native` (see `interp::classify_callee`) and their
    /// bodies are NEVER walked (natives are conduits looked up by name in
    /// `tables::native_conduit`, not interpreted ASTs), so a native path
    /// point's `src` is always the CALLING expression's own range in the file
    /// under analysis -- there is no stdlib-internal location to ever
    /// collapse. This test pins that property directly, plus that rendering
    /// the path (`leak_to_diagnostic`) never changes the confirmed leak
    /// COUNT (§0).
    #[test]
    fn multi_hop_path_renders_at_user_call_site_display_only() {
        let text = "import CompactStandardLibrary;\n\
                     export ledger c: Bytes<32>;\n\
                     witness getW(): Bytes<32>;\n\
                     export circuit f(): [] { c = persistentHash<Bytes<32>>(getW()); }\n";
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from(text));
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        let ws = empty_ws(&db);
        let file = crate::FileId::from_raw_for_test(0);

        // -- 1. Ground truth: drain the leak table straight off the
        // interpreter, bypassing `leak_to_diagnostic` (the rendering layer)
        // entirely -- the same walk `disclosure_diagnostics_query` runs
        // before it renders anything.
        let base = InterpCtx {
            db: &db,
            file,
            src,
            fd,
            ws,
            disclosing_fn: None,
        };
        let tree = item_tree(&db, src);
        let mut raw_sink = DisclosureSink::new();
        let roots = interp::discover_roots(&base, &tree, &mut raw_sink);
        for root in &roots {
            interp::interp_root(&base, &mut raw_sink, root);
        }
        let raw_leaks = raw_sink.drain_leaks();
        assert_eq!(
            raw_leaks.len(),
            1,
            "ground-truth leak count straight off the interpreter, pre-rendering"
        );

        // -- 2. Full pipeline through `leak_to_diagnostic`: DISPLAY-ONLY
        // (§0) means this must render the SAME count, never more or fewer.
        let diags = disclosure_diagnostics_query(&db, file, src, fd, ws);
        let e_leaks: Vec<_> = diags
            .iter()
            .filter(|d| d.code.prefix == "E" && d.code.number >= 3100)
            .collect();
        assert_eq!(
            e_leaks.len(),
            raw_leaks.len(),
            "leak_to_diagnostic must not change the leak count -- rendering is display-only: {diags:?}"
        );
        let leak = e_leaks[0];

        // -- 3. Path-rendering fidelity: one secondary span per witness
        // ORIGIN (spec §3.7's "source anchor")...
        assert!(
            leak.secondary_spans
                .iter()
                .any(|s| s.label.as_deref() == Some("the return value of witness getW")),
            "missing witness-origin secondary span: {:?}",
            leak.secondary_spans
        );
        // ...and one per PATH POINT -- here the `persistentHash` native
        // conduit's "a hash of" exposure.
        let hop = leak
            .secondary_spans
            .iter()
            .find(|s| {
                s.label
                    .as_deref()
                    .is_some_and(|l| l.starts_with("a hash of"))
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing 'a hash of' native-conduit hop: {:?}",
                    leak.secondary_spans
                )
            });
        assert_eq!(
            hop.label.as_deref(),
            Some("a hash of the argument to persistentHash")
        );

        // -- 4. Stdlib-collapse is INHERENT: the hop's `src` resolves (once
        // trimmed of surrounding trivia the untrimmed call-expr range may
        // carry) to the exact user-code call expression, never a
        // stdlib-internal location.
        let call_text = "persistentHash<Bytes<32>>(getW())";
        assert!(
            text.contains(call_text),
            "fixture must contain the call: {text:?}"
        );
        let hop_text = &text[usize::from(hop.span.start())..usize::from(hop.span.end())];
        assert_eq!(
            hop_text.trim(),
            call_text,
            "the conduit hop must be anchored at the user call site -- stdlib-collapse is \
             already inherent (natives are never walked), no K8 remapping needed"
        );
    }

    /// Pins the display-layer consecutive-duplicate-path-point collapse (v3c
    /// C4 minimal polish). Ground truth: `const y = getW(); c = 1 == 2 ? y :
    /// y;` -- a non-constant ternary (`1 == 2` interprets as `Abs::Atomic`,
    /// not `Abs::Boolean`, so `interp_ternary` takes the `combine_abs` join)
    /// joining the SAME already-pathed witness with itself. `merge_witnesses`
    /// concatenates the two identical one-entry paths, so the RAW leak table
    /// genuinely carries `["the binding of y", "the binding of y"]` back to
    /// back (pinned directly in
    /// `interp::tests::ternary_same_witness_both_branches_yields_duplicate_path_point`).
    /// `leak_to_diagnostic` must collapse that pair to a SINGLE secondary
    /// span while leaving the confirmed leak COUNT untouched -- rendering is
    /// display-only (§0).
    #[test]
    fn consecutive_duplicate_path_point_collapses_to_one_secondary_span() {
        let text = "import CompactStandardLibrary;\n\
                     export ledger c: Bytes<32>;\n\
                     witness getW(): Bytes<32>;\n\
                     export circuit f(): [] {\n\
                       const y = getW();\n\
                       c = 1 == 2 ? y : y;\n\
                     }\n";
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from(text));
        let fd = FileDeps::new(&db, Arc::from(Vec::new()));
        let ws = empty_ws(&db);
        let file = crate::FileId::from_raw_for_test(0);

        let diags = disclosure_diagnostics_query(&db, file, src, fd, ws);
        let e_leaks: Vec<_> = diags
            .iter()
            .filter(|d| d.code.prefix == "E" && d.code.number >= 3100)
            .collect();
        assert_eq!(
            e_leaks.len(),
            1,
            "the leak COUNT must be exactly one leak regardless of the duplicate path: {diags:?}"
        );
        let leak = e_leaks[0];

        let binding_hops: Vec<_> = leak
            .secondary_spans
            .iter()
            .filter(|s| s.label.as_deref() == Some("the binding of y"))
            .collect();
        assert_eq!(
            binding_hops.len(),
            1,
            "the duplicate 'the binding of y' path point must collapse to ONE secondary span: {:?}",
            leak.secondary_spans
        );
    }
}
