//! Editor-agnostic IDE features over analyzer-core. Speaks byte offsets
//! (`TextSize`/`TextRange`) and `FileId` — zero LSP types.

mod goto_definition;

pub use goto_definition::goto_definition;

use analyzer_core::{FileId, TextRange};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavTarget {
    pub file: FileId,
    pub name_range: TextRange,
    pub full_range: TextRange,
}
