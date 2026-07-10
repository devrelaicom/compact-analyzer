//! Editor-agnostic IDE features over analyzer-core. Speaks byte offsets
//! (`TextSize`/`TextRange`) and `FileId` — zero LSP types.

mod completion;
mod document_symbols;
mod folding_ranges;
mod goto_definition;
mod hover;
mod references;
mod rename;
mod selection_ranges;
mod semantic_tokens;
mod workspace_symbols;

pub use completion::{CompletionItem, CompletionKind, completion};
pub use document_symbols::{DocSymbol, document_symbols};
pub use folding_ranges::{FoldKind, FoldRange, folding_ranges};
pub use goto_definition::goto_definition;
pub use hover::{HoverResult, hover};
pub use references::{find_references, find_references_cancellable};
pub use rename::{RenameError, SourceEdit, rename, rename_cancellable};
pub use selection_ranges::selection_ranges;
pub use semantic_tokens::{SemToken, TokenMods, TokenType, semantic_tokens};
pub use workspace_symbols::{WorkspaceSymbolItem, workspace_symbols};

use analyzer_core::{FileId, TextRange};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavTarget {
    pub file: FileId,
    pub name_range: TextRange,
    pub full_range: TextRange,
}
