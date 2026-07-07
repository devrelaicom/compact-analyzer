//! Core analysis engine for compact-analyzer.
//!
//! Speaks byte offsets (`text_size::TextSize`/`TextRange`) exclusively.
//! No LSP types are allowed in this crate — protocol conversion lives in
//! the `compact-analyzer` binary.

mod analysis;
mod item_tree;
mod line_index;
mod vfs;

pub use analysis::{AnalysisHost, FileAnalysis};
pub use compactp_diagnostics::{Diagnostic, DiagnosticCode, LabeledSpan, Severity};
pub use compactp_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
pub use item_tree::{ItemTree, Symbol, SymbolKind};
pub use line_index::{LineCol, LineIndex};
pub use text_size::{TextRange, TextSize};
pub use vfs::{FileId, Vfs};
