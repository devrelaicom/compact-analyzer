//! Compiler-version drift check (v3c C5, spec §4.3).
//!
//! The disclosure WPP rules + native/ledger flag tables (`tables.rs`) were
//! extracted for one specific `compactc` release — [`PINNED_COMPILER_VERSION`].
//! If the developer's installed `compact` is a DIFFERENT version, the tables
//! may be stale: a flag that changed between releases is exactly the kind of
//! drift the analyzer has no way to detect on its own (a drifted flag would
//! silently become a §0 unknown rather than announcing itself). This module
//! only computes the mismatch; `compact-analyzer`'s server wires it to a
//! one-time startup advisory once the toolchain is discovered.

/// The `compactc` release the disclosure tables (`tables.rs`) were extracted
/// from (commit `0da5b045`, per `disclosure`'s module doc).
pub const PINNED_COMPILER_VERSION: &str = "0.31.1";

/// Checks `installed` (a `Toolchain::tool_version`, e.g. `"0.31.1"`) against
/// [`PINNED_COMPILER_VERSION`] at MAJOR.MINOR granularity: patch releases are
/// assumed not to change the WPP-relevant grammar or native/ledger surface,
/// so `"0.31.5"` still matches the `0.31` pin, but `"0.32.0"` does not. This
/// is a judgment call (a full-version match would also be defensible and is
/// more conservative) but MAJOR.MINOR keeps the advisory from firing on every
/// compiler patch bump while still catching the minor/major bumps that are
/// most likely to add or change a native/ledger flag.
///
/// Returns `None` when the MAJOR.MINOR components match; `Some(reason)`
/// otherwise, where `reason` names both versions and states that disclosure
/// results may be stale. A version string that doesn't parse into at least
/// two dot-separated components is fail-closed: treated as a mismatch, same
/// as the rest of the disclosure module's §0 posture.
pub fn version_mismatch(installed: &str) -> Option<String> {
    let matches = match (major_minor(installed), major_minor(PINNED_COMPILER_VERSION)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    if matches {
        return None;
    }
    Some(format!(
        "the disclosure tables were extracted for compact {PINNED_COMPILER_VERSION}; the \
         installed compiler is {installed}, so disclosure results may be stale"
    ))
}

/// Extracts the `(major, minor)` substrings from a `MAJOR.MINOR.PATCH`-ish
/// version string. `None` if fewer than two dot-separated components are
/// present.
fn major_minor(v: &str) -> Option<(&str, &str)> {
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?;
    let minor = parts.next()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_is_none() {
        assert_eq!(version_mismatch("0.31.1"), None);
    }

    #[test]
    fn same_major_minor_different_patch_is_none() {
        assert_eq!(version_mismatch("0.31.5"), None);
        assert_eq!(version_mismatch("0.31.0"), None);
    }

    #[test]
    fn different_minor_is_some_and_mentions_staleness() {
        let reason = version_mismatch("0.32.0").expect("minor bump must mismatch");
        assert!(
            reason.contains("0.31.1"),
            "must name the pinned version: {reason}"
        );
        assert!(
            reason.contains("0.32.0"),
            "must name the installed version: {reason}"
        );
        assert!(
            reason.contains("stale"),
            "must state the tables may be stale: {reason}"
        );
    }

    #[test]
    fn different_major_is_some() {
        assert!(version_mismatch("1.0.0").is_some());
    }

    #[test]
    fn unparseable_version_is_fail_closed_some() {
        assert!(version_mismatch("nightly").is_some());
        assert!(version_mismatch("0").is_some());
    }
}
