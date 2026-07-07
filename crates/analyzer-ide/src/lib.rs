//! Editor-agnostic IDE features over analyzer-core. Speaks byte offsets
//! (`TextSize`/`TextRange`) and `FileId` — zero LSP types.

mod document_symbols;
mod goto_definition;
mod hover;
mod references;
mod rename;
mod workspace_symbols;

pub use document_symbols::{DocSymbol, document_symbols};
pub use goto_definition::goto_definition;
pub use hover::{HoverResult, hover};
pub use references::{find_references, find_references_cancellable};
pub use rename::{RenameError, SourceEdit, rename, rename_cancellable};
pub use workspace_symbols::{WorkspaceSymbolItem, workspace_symbols};

use analyzer_core::{FileId, TextRange};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavTarget {
    pub file: FileId,
    pub name_range: TextRange,
    pub full_range: TextRange,
}
