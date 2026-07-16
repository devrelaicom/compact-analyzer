//! Leak table + advisory emitter (spec §3.2/§3.8, R0 rows K1-K8, L1-L3).
//!
//! Future role: the interpreter (A3) will call into this module to record
//! sink hits (`record_leak`, mirroring the source's `record-leak!`), keyed by
//! `(src, nature)`, and to emit fail-closed amber advisories (`U3100`+, spec
//! §0) for the surfaces the analyzer cannot fully decide (unknown type,
//! unhandled expression, unknown native/ledger op). Confirmed leaks render as
//! `E3100`+ diagnostics via `complain` (source anchor `analysis-passes.ss:5244`).
//!
//! Stub for A1 — no logic yet; filled in by Task A4.
