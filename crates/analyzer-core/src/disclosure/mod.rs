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

use std::sync::Arc;

use compactp_diagnostics::{Diagnostic, DiagnosticCode};

use crate::db::{Db, FileDeps, SourceText, Workspace, item_tree};
use interp::InterpCtx;
use leaks::{DisclosureLeak, DisclosureSink, advisory_to_diagnostic};

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
/// plus one per `PathPoint` on its witness->sink trail (source anchor +
/// intermediate exposures), so the editor can walk the disclosure path.
fn leak_to_diagnostic(leak: DisclosureLeak) -> Diagnostic {
    let mut diag = Diagnostic::error(
        DiagnosticCode::new("E", 3100),
        format!("potential witness-value disclosure: {}", leak.nature),
        leak.src,
    );
    for w in &leak.witnesses {
        diag = diag.with_secondary(w.src, Some(witness_origin_label(&w.info)));
        for pp in &w.path {
            let label = if pp.exposure.is_empty() {
                pp.description.clone()
            } else {
                format!("{} {}", pp.exposure, pp.description)
            };
            diag = diag.with_secondary(pp.src, Some(label));
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
}
