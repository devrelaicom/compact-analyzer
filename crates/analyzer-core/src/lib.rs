//! Core analysis engine for compact-analyzer.
//!
//! Speaks byte offsets (`text_size::TextSize`/`TextRange`) exclusively.
//! No LSP types are allowed in this crate — protocol conversion lives in
//! the `compact-analyzer` binary.

mod analysis;
pub mod fixture;
mod item_tree;
mod ledger_adts;
mod line_index;
mod resolve;
mod source_path;
pub mod stdlib;
mod vfs;
mod workspace;

pub use analysis::{AnalysisHost, FileAnalysis};
pub use compactp_diagnostics::{Diagnostic, DiagnosticCode, LabeledSpan, Severity};
pub use compactp_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
pub use item_tree::{ItemTree, Symbol, SymbolKind};
pub use ledger_adts::LedgerMethod;
pub use line_index::{LineCol, LineIndex};
pub use resolve::{Binding, Definition, FilePosition, scope_bindings_at};
pub use source_path::{find_source_pathname, path_module_name, string_lit_text};
pub use text_size::{TextRange, TextSize};
pub use vfs::{FileId, Vfs};
pub use workspace::discover_compact_files;
