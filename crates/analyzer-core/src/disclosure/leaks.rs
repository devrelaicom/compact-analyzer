//! Leak table + advisory emitter (spec §3.2/§3.8, R0 rows K1-K8, L1-L3).
//!
//! Future role: the interpreter (A3) will call into this module to record
//! sink hits (`record_leak`, mirroring the source's `record-leak!`), keyed by
//! `(src, nature)`, and to emit fail-closed amber advisories (`U3100`+, spec
//! §0) for the surfaces the analyzer cannot fully decide (unknown type,
//! unhandled expression, unknown native/ledger op). Confirmed leaks render as
//! `E3100`+ diagnostics via `complain` (source anchor `analysis-passes.ss:5244`).
//!
//! This task (A2) adds the `Advisory` primitive plus `DisclosureSink`, the
//! mutable context `abs_of_type`/`seed_sources` (`interp.rs`) thread through
//! seeding. The leak-table half is still a stub — Task A4 fills it in.
//!
//! `interp.rs`'s only caller is its own unit tests until A3 wires the
//! interpreter walk into `disclosure_diagnostics_query`, so nothing here is
//! reachable from a live root outside `#[cfg(test)]` either — hence the
//! file-level `#![cfg_attr(not(test), expect(dead_code))]` below (see
//! `interp.rs`'s module doc for the full rationale).
#![cfg_attr(not(test), expect(dead_code))]

use text_size::TextRange;

/// A fail-closed "unverified" advisory (spec §0). A8 renders accumulated
/// advisories as amber U-family diagnostics; A2..A6 only record them here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advisory {
    pub src: TextRange,
    pub reason: String,
}

/// Mutable state threaded through source seeding (A2) and, later, the
/// interpreter walk (A3/A4/A6): the fail-closed advisories set (spec §0)
/// plus the monotonic witness-uid counter (mirrors compactc's
/// `next-witness-uid`, `analysis-passes.ss`) every fresh `Witness` needs.
/// From A4 onward this also carries the leak table (K1-K8) — not yet
/// present, since nothing records a leak until then.
#[derive(Debug, Default)]
pub struct DisclosureSink {
    advisories: Vec<Advisory>,
    next_uid: u32,
    // Leak table: Task A4.
}

impl DisclosureSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a fail-closed advisory (spec §0): a surface the analyzer
    /// could not fully decide (here: an `Unknown` declared type — AB1).
    /// Never drops the uncertainty silently; A8 renders the accumulated set
    /// as amber `U31xx` diagnostics.
    pub fn emit_advisory(&mut self, src: TextRange, reason: impl Into<String>) {
        self.advisories.push(Advisory {
            src,
            reason: reason.into(),
        });
    }

    pub fn advisories(&self) -> &[Advisory] {
        &self.advisories
    }

    /// The next fresh witness uid, monotonically increasing across this
    /// sink's lifetime (one sink per analysis run).
    pub fn next_witness_uid(&mut self) -> u32 {
        let uid = self.next_uid;
        self.next_uid += 1;
        uid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_advisory_accumulates_and_uid_is_monotonic() {
        let mut sink = DisclosureSink::new();
        assert!(sink.advisories().is_empty());
        sink.emit_advisory(TextRange::empty(0.into()), "first".to_string());
        sink.emit_advisory(TextRange::empty(1.into()), "second".to_string());
        assert_eq!(sink.advisories().len(), 2);
        assert_eq!(sink.advisories()[0].reason, "first");
        assert_eq!(sink.advisories()[1].reason, "second");

        assert_eq!(sink.next_witness_uid(), 0);
        assert_eq!(sink.next_witness_uid(), 1);
        assert_eq!(sink.next_witness_uid(), 2);
    }
}
