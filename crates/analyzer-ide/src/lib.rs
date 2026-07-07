//! Editor-agnostic IDE features over analyzer-core. Speaks byte offsets
//! (`TextSize`/`TextRange`) and `FileId` — zero LSP types.

mod goto_definition;
mod hover;
mod references;
mod rename;

pub use goto_definition::goto_definition;
pub use hover::{HoverResult, hover};
pub use references::find_references;
pub use rename::{RenameError, SourceEdit, rename};

use analyzer_core::{FileId, TextRange};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavTarget {
    pub file: FileId,
    pub name_range: TextRange,
    pub full_range: TextRange,
}
