//! Native witness-disclosure ("Witness Protection Program" / WPP) analysis
//! (v3 spec §3, R0 index `docs/superpowers/plans/2026-07-16-v3-wpp-rules-index.md`).
//!
//! Re-implements the compactc `track-witness-data` abstract interpreter
//! (`compactc-v0.31.1`, commit `0da5b045`) natively so the editor can surface
//! disclosure diagnostics without shelling out to the compiler. FAIL-CLOSED
//! is the program invariant (spec §0): any surface the analyzer cannot fully
//! decide (an unknown type, an unhandled expression, an unknown native or
//! ledger op) must emit an amber advisory (`U3100`+) rather than silently
//! reporting no leak. Confirmed leaks are `E3100`+.
//!
//! - `abs` — the `Abs` abstract-value lattice (this task, A1).
//! - `interp` — the abstract interpreter (A2/A3; not yet created).
//! - `leaks` — the leak table + advisory emitter (A4; stub for now).
//!
//! This task (A1) wires the module and an EMPTY tracked query — no
//! disclosure detection yet.

mod abs;
mod leaks;

use std::sync::Arc;

use compactp_diagnostics::Diagnostic;

use crate::db::{Db, SourceText, item_tree};

/// Disclosure diagnostics for `src`. Empty until A2/A3 land the interpreter;
/// touches `item_tree` so salsa tracks the input this query will eventually
/// walk.
#[salsa::tracked(returns(clone), no_eq)]
pub fn disclosure_diagnostics_query(db: &dyn Db, src: SourceText) -> Arc<[Diagnostic]> {
    let _tree = item_tree(db, src); // touch the input so salsa tracks it; filled by A2+
    Arc::from(Vec::new())
}

impl crate::AnalysisHost {
    /// Disclosure (WPP) diagnostics for `file`. Thin bridge over the tracked
    /// `disclosure_diagnostics_query`, mirroring `type_diagnostics`. Empty
    /// until A2/A3 land the interpreter.
    pub fn disclosure_diagnostics(&mut self, file: crate::FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        disclosure_diagnostics_query(self.db_ref(), src).to_vec()
    }
}
