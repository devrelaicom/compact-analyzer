//! Test support: `$0` cursor markers in inline fixtures (rust-analyzer
//! convention). Not for production use.
#![doc(hidden)]

use text_size::TextSize;

/// Removes the first `$0` from `source`, returning the cleaned text and the
/// marker's byte offset. Panics if no marker is present.
pub fn extract(source: &str) -> (String, TextSize) {
    let idx = source.find("$0").expect("fixture must contain a $0 marker");
    let mut clean = String::with_capacity(source.len() - 2);
    clean.push_str(&source[..idx]);
    clean.push_str(&source[idx + 2..]);
    (clean, TextSize::new(idx as u32))
}
