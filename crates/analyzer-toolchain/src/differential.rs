//! Classifies a `compact compile --skip-zk --vscode` outcome into a
//! type-differential verdict.
//!
//! Verified against `compact 0.5.1` / compiler `0.31.1` (2026-07-15): the
//! compiler has no stop-after-type-check flag, so phase isolation is done by
//! classifying the rejection. A parse rejection's single-line `--vscode`
//! `Exception:` message begins with the literal `parse error:`; a post-parse
//! rejection (type mismatch, unbound identifier, ...) does not. The native
//! analyzer has its own parser, so parse disagreements are excluded from the
//! type gate.

use crate::compile::{CompileOutcome, CompileStatus};
use crate::parse::parse_compiler_stderr;

/// A `compactc` verdict, reduced to what the type differential needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompilerVerdict {
    /// Exit `0`: accepted through the whole pipeline.
    Accept,
    /// Rejected at the parse phase (`Exception` message begins `parse
    /// error:`). Excluded from the type gate.
    RejectParse,
    /// Rejected after parsing (type/semantic error): the type-phase reject
    /// bucket.
    RejectPostParse,
    /// No usable type-phase verdict (usage error, timeout, cancellation,
    /// spawn failure). Excluded.
    Indeterminate,
}

/// Classify one [`compile_file`](crate::compile_file) outcome.
pub fn classify(outcome: &CompileOutcome) -> CompilerVerdict {
    match outcome.status {
        CompileStatus::Ok => CompilerVerdict::Accept,
        CompileStatus::CompileError => {
            let parsed = parse_compiler_stderr(&outcome.stderr);
            let is_parse = parsed
                .diagnostics
                .iter()
                .any(|d| d.message.starts_with("parse error:"));
            if is_parse {
                CompilerVerdict::RejectParse
            } else {
                CompilerVerdict::RejectPostParse
            }
        }
        CompileStatus::InvocationError | CompileStatus::TimedOut | CompileStatus::Cancelled => {
            CompilerVerdict::Indeterminate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CompileOutcome, CompileStatus};

    fn outcome(status: CompileStatus, stderr: &str) -> CompileOutcome {
        CompileOutcome {
            status,
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn accept_on_exit_zero() {
        assert_eq!(
            classify(&outcome(CompileStatus::Ok, "")),
            CompilerVerdict::Accept
        );
    }

    #[test]
    fn parse_error_is_reject_parse() {
        let e = "Exception: f.compact line 3 char 20: parse error: found \":\" looking for a typed pattern or \")\"";
        assert_eq!(
            classify(&outcome(CompileStatus::CompileError, e)),
            CompilerVerdict::RejectParse
        );
    }

    #[test]
    fn type_error_is_reject_post_parse() {
        let e = "Exception: f.compact line 2 char 3: mismatch between actual return type Boolean and declared return type Field of circuit foo";
        assert_eq!(
            classify(&outcome(CompileStatus::CompileError, e)),
            CompilerVerdict::RejectPostParse
        );
    }

    #[test]
    fn unbound_identifier_is_reject_post_parse() {
        let e = "Exception: f.compact line 3 char 10: unbound identifier bar";
        assert_eq!(
            classify(&outcome(CompileStatus::CompileError, e)),
            CompilerVerdict::RejectPostParse
        );
    }

    #[test]
    fn usage_and_timeout_are_indeterminate() {
        assert_eq!(
            classify(&outcome(CompileStatus::InvocationError, "Usage: ...")),
            CompilerVerdict::Indeterminate
        );
        assert_eq!(
            classify(&outcome(CompileStatus::TimedOut, "")),
            CompilerVerdict::Indeterminate
        );
    }
}
