//! Optional `compact` CLI toolchain integration for compact-analyzer.
//!
//! Mirrors `analyzer-ide`'s place in the dependency graph: it sits beside it as a
//! feature-layer crate, depending on `analyzer-core` but not on `analyzer-ide` or
//! `lsp-types`. Its public surface speaks plain byte offsets (`text_size`) and
//! plain Rust types exclusively — LSP protocol conversion happens only in the
//! `compact-analyzer` binary.
//!
//! The `compact` CLI is a *runtime* dependency discovered on disk / `PATH` at
//! startup, never a build-time one: everything here degrades to `None` /
//! no-op when no usable toolchain is found.

mod discovery;
mod locate;
mod parse;

pub use discovery::Toolchain;
pub use locate::locate;
pub use parse::{ParsedStderr, RawCompilerDiagnostic, parse_compiler_stderr};
