//! Leak table + advisory emitter (spec §3.2/§3.8, R0 rows K1-K8, L1-L3).
//!
//! The interpreter (A3+) calls into this module to record sink hits
//! (`record_leak`, mirroring the source's `record-leak!`), keyed by
//! `(src, nature)`, and to emit fail-closed amber advisories (`U3100`+, spec
//! §0) for the surfaces the analyzer cannot fully decide (unknown type,
//! unhandled expression, unknown native/ledger op). Confirmed leaks render as
//! `E3100`+ diagnostics via `disclosure::leak_to_diagnostic` (source anchor
//! `analysis-passes.ss:5244`).
//!
//! A4 fills in the leak-table half A2 left as a stub: `DisclosureLeak` +
//! `record_leak` (dedup by `(src, nature)`, union witnesses on collision) +
//! `drain_leaks`. `disclosure_diagnostics_query` (`mod.rs`) now drives the
//! walk from every root into a single sink and drains it — so every item here
//! has a live non-test caller, and the A2-era file-level dead-code marker is
//! gone.

use text_size::TextRange;

use super::abs::{Witness, merge_witnesses};

/// A fail-closed "unverified" advisory (spec §0). A8 renders accumulated
/// advisories as amber U-family diagnostics; A2..A6 only record them here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advisory {
    pub src: TextRange,
    pub reason: String,
}

/// A confirmed disclosure leak recorded at a sink (spec §3.2). Keyed in the
/// leak table by `(src, nature)`; `witnesses` is the union of every witness
/// set recorded at that key (K1 ledger sink, K7 return sink, …).
/// `disclosure::leak_to_diagnostic` renders it as an `E3100` with the witness
/// origins + path trail as secondary spans (spec §3.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisclosureLeak {
    pub src: TextRange,
    /// The sink's nature string (the `what` argument of `record-leak!`) —
    /// e.g. `"ledger operation"`, `"the value returned from exported circuit f"`.
    pub nature: String,
    pub witnesses: Vec<Witness>,
}

/// Mutable state threaded through source seeding (A2) and the interpreter walk
/// (A3/A4/A6): the fail-closed advisories set (spec §0), the monotonic
/// witness-uid counter (mirrors compactc's `next-witness-uid`,
/// `analysis-passes.ss`) every fresh `Witness` needs, and the leak table
/// (K1-K8) sinks record into.
#[derive(Debug, Default)]
pub struct DisclosureSink {
    advisories: Vec<Advisory>,
    next_uid: u32,
    /// The leak table (R0 §3.2): confirmed sink hits, deduped by
    /// `(src, nature)`, witnesses unioned on collision (`record-leak!`).
    leaks: Vec<DisclosureLeak>,
}

impl DisclosureSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a fail-closed advisory (spec §0): a surface the analyzer
    /// could not fully decide (an `Unknown` declared type, an unmodeled
    /// expression, a native/unresolved callee). Never drops the uncertainty
    /// silently; A8 renders the accumulated set as amber `U31xx` diagnostics.
    pub fn emit_advisory(&mut self, src: TextRange, reason: impl Into<String>) {
        self.advisories.push(Advisory {
            src,
            reason: reason.into(),
        });
    }

    // Consumed by A8's amber advisory rendering (and this module's tests); the
    // walk only *records* advisories (`emit_advisory`) — nothing reads them back
    // out of a live query until A8, so this getter is dead in the non-test build
    // until then (mirrors `abs_equal`'s A7 deferral).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn advisories(&self) -> &[Advisory] {
        &self.advisories
    }

    /// Records a confirmed leak at a sink (spec §4.2, `record-leak!`). Deduped
    /// by `(src, nature)`: a second hit at the same key unions its witnesses
    /// into the existing entry (via `merge_witnesses`) rather than adding a
    /// duplicate row. A no-op when `witnesses` is empty (nothing leaks).
    pub fn record_leak(
        &mut self,
        src: TextRange,
        nature: impl Into<String>,
        witnesses: Vec<Witness>,
    ) {
        if witnesses.is_empty() {
            return;
        }
        let nature = nature.into();
        if let Some(existing) = self
            .leaks
            .iter_mut()
            .find(|l| l.src == src && l.nature == nature)
        {
            existing.witnesses = merge_witnesses(&existing.witnesses, &witnesses);
        } else {
            self.leaks.push(DisclosureLeak {
                src,
                nature,
                witnesses,
            });
        }
    }

    /// Drains the accumulated leak table, leaving it empty. Called once at the
    /// end of the query after every root has been walked, to render each leak
    /// as an `E3100` diagnostic.
    pub fn drain_leaks(&mut self) -> Vec<DisclosureLeak> {
        std::mem::take(&mut self.leaks)
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
    use super::super::abs::WitnessInfo;
    use super::*;

    fn w(uid: u32) -> Witness {
        Witness {
            uid,
            src: TextRange::empty(0.into()),
            info: WitnessInfo::WitnessReturn {
                function: "getW".into(),
            },
            path: Vec::new(),
        }
    }

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

    #[test]
    fn record_leak_dedups_by_src_and_nature_unioning_witnesses() {
        let mut sink = DisclosureSink::new();
        let src = TextRange::empty(5.into());
        // Empty witness set is a no-op (nothing leaks).
        sink.record_leak(src, "ledger operation", vec![]);
        assert!(sink.drain_leaks().is_empty());

        // Two hits at the same (src, nature) collapse to one row, witnesses unioned.
        sink.record_leak(src, "ledger operation", vec![w(0)]);
        sink.record_leak(src, "ledger operation", vec![w(1)]);
        // A different nature at the same src is a distinct row.
        sink.record_leak(
            src,
            "the value returned from exported circuit f",
            vec![w(0)],
        );

        let mut leaks = sink.drain_leaks();
        leaks.sort_by(|a, b| a.nature.cmp(&b.nature));
        assert_eq!(leaks.len(), 2);
        assert_eq!(leaks[0].nature, "ledger operation");
        assert_eq!(leaks[0].witnesses.len(), 2);
        assert_eq!(
            leaks[1].nature,
            "the value returned from exported circuit f"
        );
        assert_eq!(leaks[1].witnesses.len(), 1);

        // Drained: the table is now empty.
        assert!(sink.drain_leaks().is_empty());
    }
}
