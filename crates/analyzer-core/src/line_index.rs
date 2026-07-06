//! Byte-offset ↔ line/column mapping.
//!
//! Columns are UTF-16 code units, because that is what LSP positions use by
//! default. Lines are split on '\n'; a preceding '\r' belongs to the line it
//! terminates. Conversion scans within a single line per call — Compact
//! source lines are short, so this is simpler and fast enough versus
//! precomputing per-line UTF-16 tables.

use std::sync::Arc;

use text_size::TextSize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineCol {
    /// 0-based line number.
    pub line: u32,
    /// 0-based column in UTF-16 code units.
    pub col: u32,
}

#[derive(Debug)]
pub struct LineIndex {
    text: Arc<str>,
    /// Byte offset of the start of each line. `line_starts[0] == 0`.
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: Arc<str>) -> Self {
        let mut line_starts = vec![0u32];
        for (i, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        Self { text, line_starts }
    }

    /// Converts a byte offset into 0-based line + UTF-16 column, clamping
    /// out-of-range offsets to the end of the text.
    pub fn line_col(&self, offset: TextSize) -> LineCol {
        let offset = u32::from(offset).min(self.text.len() as u32);
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let line_start = self.line_starts[line];
        let col = self.text[line_start as usize..offset as usize]
            .chars()
            .map(|c| c.len_utf16() as u32)
            .sum();
        LineCol {
            line: line as u32,
            col,
        }
    }

    /// Converts a 0-based line + UTF-16 column into a byte offset. Columns
    /// past the end of the line clamp to the line end (before the line
    /// terminator). Returns `None` if the line does not exist.
    pub fn offset(&self, pos: LineCol) -> Option<TextSize> {
        let line_start = *self.line_starts.get(pos.line as usize)?;
        let line_end = self
            .line_starts
            .get(pos.line as usize + 1)
            .copied()
            .unwrap_or(self.text.len() as u32);
        let line_text = &self.text[line_start as usize..line_end as usize];

        let mut utf16_col = 0u32;
        for (byte_in_line, c) in line_text.char_indices() {
            if utf16_col >= pos.col {
                return Some(TextSize::new(line_start + byte_in_line as u32));
            }
            utf16_col += c.len_utf16() as u32;
        }
        // Column is at or past the line end: clamp to the end of the line's
        // content, excluding any trailing line terminator.
        let content = line_text
            .strip_suffix('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s))
            .unwrap_or(line_text);
        Some(TextSize::new(line_start + content.len() as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use text_size::TextSize;

    fn index(text: &str) -> LineIndex {
        LineIndex::new(Arc::from(text))
    }

    #[test]
    fn ascii_single_line() {
        let li = index("ledger count Field;");
        assert_eq!(li.line_col(TextSize::new(0)), LineCol { line: 0, col: 0 });
        assert_eq!(li.line_col(TextSize::new(12)), LineCol { line: 0, col: 12 });
        assert_eq!(
            li.offset(LineCol { line: 0, col: 12 }),
            Some(TextSize::new(12))
        );
    }

    #[test]
    fn multiline_lf() {
        let li = index("abc\ndef\nghi");
        assert_eq!(li.line_col(TextSize::new(4)), LineCol { line: 1, col: 0 });
        assert_eq!(li.line_col(TextSize::new(7)), LineCol { line: 1, col: 3 });
        assert_eq!(li.line_col(TextSize::new(8)), LineCol { line: 2, col: 0 });
        assert_eq!(
            li.offset(LineCol { line: 2, col: 1 }),
            Some(TextSize::new(9))
        );
    }

    #[test]
    fn crlf_line_endings() {
        let li = index("abc\r\ndef");
        // '\r' is part of line 0; 'd' starts line 1 at byte 5
        assert_eq!(li.line_col(TextSize::new(5)), LineCol { line: 1, col: 0 });
        assert_eq!(
            li.offset(LineCol { line: 1, col: 0 }),
            Some(TextSize::new(5))
        );
    }

    #[test]
    fn emoji_is_two_utf16_units() {
        // "/* 😀 */ ledger count Field;" — verified fixture:
        // byte offset 23 (where the colon is expected) is UTF-16 column 21
        let li = index("/* \u{1F600} */ ledger count Field;");
        assert_eq!(li.line_col(TextSize::new(23)), LineCol { line: 0, col: 21 });
        assert_eq!(
            li.offset(LineCol { line: 0, col: 21 }),
            Some(TextSize::new(23))
        );
    }

    #[test]
    fn out_of_range_clamps() {
        let li = index("abc");
        // offset past the end clamps to the end
        assert_eq!(li.line_col(TextSize::new(99)), LineCol { line: 0, col: 3 });
        // col past line end clamps to line end (excluding the newline)
        let li = index("ab\ncd");
        assert_eq!(
            li.offset(LineCol { line: 0, col: 99 }),
            Some(TextSize::new(2))
        );
        // line past the last line is None
        assert_eq!(li.offset(LineCol { line: 9, col: 0 }), None);
    }
}
