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
//! each confirmed leak as an `E3100` diagnostic. Accumulated advisories are NOT
//! rendered here — that amber U-family channel is A8; the query returns only
//! confirmed E-leaks.

mod abs;
mod interp;
mod leaks;

use std::sync::Arc;

use compactp_diagnostics::{Diagnostic, DiagnosticCode};

use crate::db::{Db, FileDeps, SourceText, Workspace, item_tree};
use interp::InterpCtx;
use leaks::{DisclosureLeak, DisclosureSink};

/// Disclosure (WPP) diagnostics for `src`: the confirmed witness-disclosure
/// leaks (`E3100`) the intraprocedural interpreter finds. Discovers the roots
/// (exported circuits + constructor), seeds each one's sources, walks its body
/// into a shared leak table, then drains the table into diagnostics. Advisories
/// are recorded but not rendered (A8).
#[salsa::tracked(returns(clone), no_eq)]
pub fn disclosure_diagnostics_query(
    db: &dyn Db,
    file: crate::FileId,
    src: SourceText,
    fd: FileDeps,
    ws: Workspace,
) -> Arc<[Diagnostic]> {
    let tree = item_tree(db, src);
    let roots = interp::discover_roots(&tree);
    let mut sink = DisclosureSink::new();
    let base = InterpCtx {
        db,
        file,
        src,
        fd,
        ws,
        disclosing_fn: None,
    };
    for root in &roots {
        interp::interp_root(&base, &mut sink, root);
    }
    let diags: Vec<Diagnostic> = sink
        .drain_leaks()
        .into_iter()
        .map(leak_to_diagnostic)
        .collect();
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
