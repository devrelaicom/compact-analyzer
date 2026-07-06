//! Core analysis engine for compact-analyzer.
//!
//! Speaks byte offsets (`text_size::TextSize`/`TextRange`) exclusively.
//! No LSP types are allowed in this crate — protocol conversion lives in
//! the `compact-analyzer` binary.

mod vfs;

pub use vfs::{FileId, Vfs};
