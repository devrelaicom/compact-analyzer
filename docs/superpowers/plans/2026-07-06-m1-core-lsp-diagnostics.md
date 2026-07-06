# compact-analyzer M1: Core + Real-Time Syntax Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A working `compact-analyzer` LSP server that an editor can attach to today: it syncs open documents, parses them with compactp, and publishes real-time syntax diagnostics with correct UTF-16 positions — surviving any input without dying.

**Architecture:** Two crates from the spec's layout: `analyzer-core` (FileId-interned VFS with overlay-over-disk semantics, UTF-16-aware line index, content-hash-memoized parse cache over `compactp_parser`) and the `compact-analyzer` binary (synchronous `lsp-server` main loop with 150 ms debounce, per-message `catch_unwind`, byte-offset→UTF-16 conversion at the protocol boundary only). `analyzer-ide` and `analyzer-toolchain` are introduced in later milestones (M2/M4) — YAGNI for M1.

**Tech Stack:** Rust (edition 2024), compactp 0.1.0-beta.1 (crates.io), lsp-server 0.8, lsp-types 0.95.1, rowan 0.16, text-size 1, crossbeam-channel 0.5.

## Milestone map (spec → plans)

This is Plan 1 of 5 for the approved spec (`docs/superpowers/specs/2026-07-06-compact-analyzer-design.md`). Each plan ships working software:

- **M1 (this plan):** workspace + analyzer-core + LSP server publishing real-time syntax diagnostics
- **M2:** item index, import/scope resolution, stdlib stubs → goto-def, references, rename, hover, document/workspace symbols
- **M3:** completion (keywords, scope, ledger ADT methods, struct fields), semantic tokens, folding/selection ranges
- **M4:** analyzer-toolchain — compile-on-save diagnostics via `compactc --skip-zk --vscode`, formatting via `compact format`, graceful degradation
- **M5:** VS Code extension (`editors/vscode`) + cargo-dist release pipeline + Marketplace/Open VSX publishing

## Global Constraints

- Binary name: `compact-analyzer`. Workspace crates are `publish = false` (the binary is the product).
- Rust edition **2024**, `rust-version = "1.90"`, workspace `resolver = "3"` (matches compactp).
- Dependencies (workspace-pinned, exact): `compactp_parser`/`compactp_syntax`/`compactp_diagnostics` = `"0.1.0-beta.1"` from crates.io; `lsp-server = "0.8"`; `lsp-types = "0.95.1"` (deliberately NOT 0.96+ — 0.95 still uses `url::Url`, which has `to_file_path`/`from_file_path`; 0.96+ switched to a `Uri` type without them); `rowan = "0.16"` and `text-size = "1"` (must match compactp's rowan for `GreenNode` type identity); `crossbeam-channel = "0.5"`; `serde_json = "1"`; `anyhow = "1"`; dev: `tempfile = "3"`.
- Local compactp source lives at `../compactp`. When an upstream parser change is needed, add an **uncommitted** `[patch.crates-io]` section pointing at `../compactp/crates/<crate>`; never commit the patch. Upstream changes land in the compactp repo, get released, then the pin here moves.
- stdout carries LSP protocol bytes ONLY. All human-facing output (logs, panics) goes to stderr (`eprintln!`).
- **Never-die contract:** every message dispatch runs under `std::panic::catch_unwind`; a panicked request gets an `InternalError` response; the loop continues.
- `analyzer-core` speaks byte offsets (`text_size::TextSize`/`TextRange`) and has zero `lsp-types` knowledge. UTF-16 conversion happens only in the binary.
- LSP `Position.character` is in UTF-16 code units (verified fixtures below encode this).
- Diagnostics debounce: **150 ms**. Diagnostics `source` field: `"compact-analyzer"`.
- compactp API facts (verified against the 0.1.0-beta.1 public-api baselines and local source): `compactp_parser::parse(&str) -> ParseResult`; `ParseResult { green: rowan::GreenNode, errors: Vec<compactp_diagnostics::Diagnostic> }` (public fields, no methods, no Clone); `Diagnostic { severity: Severity, code: DiagnosticCode, message: String, primary_span: TextRange, secondary_spans: Vec<LabeledSpan>, notes: Vec<String> }`; `Severity::{Error, Warning, Note}`; `DiagnosticCode` implements `Display` (renders e.g. `E1`); build a syntax tree with `compactp_syntax::SyntaxNode::new_root(green)`. `SyntaxNode` is `!Send` — store `GreenNode` (Send+Sync), never `SyntaxNode`.
- Verified parser fixtures (ground truth from compactp 0.1.0-beta.1, do not "fix" the tests to different values):
  - `ledger count: Field;` → 0 diagnostics
  - `ledger count Field;` → 1 error `expected COLON` (E1), zero-width span at offset 12..12
  - `@@@` → 1 error `unexpected token at top level` (E1), zero-width span at 0..0 (compactp anchors this diagnostic at 0 even when the garbage sits later in the file — known upstream quirk)
  - `/* 😀 */ ledger count Field;` → 1 error `expected COLON` at offset 23..23, which is UTF-16 column **21**
- Zero-width spans are legal LSP ranges (start == end); pass them through unchanged.
- Quality gates per task: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Commit at the end of every task (conventional-commit style, matching compactp).

---

### Task 1: Cargo workspace scaffold + CI

**Files:**
- Create: `Cargo.toml`
- Create: `crates/analyzer-core/Cargo.toml`
- Create: `crates/analyzer-core/src/lib.rs`
- Create: `crates/compact-analyzer/Cargo.toml`
- Create: `crates/compact-analyzer/src/main.rs`
- Create: `.github/workflows/ci.yml`
- Modify: `.gitignore` (ensure `/target` is ignored)

**Interfaces:**
- Consumes: nothing (first task)
- Produces: a building workspace; `compact-analyzer --version` prints `compact-analyzer 0.1.0`; workspace dep aliases used by all later tasks (`analyzer-core`, `compactp_parser`, `lsp-server`, …)

- [ ] **Step 1: Write the workspace root `Cargo.toml`**

```toml
[workspace]
resolver = "3"
members = ["crates/analyzer-core", "crates/compact-analyzer"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.90"
license = "MIT"
repository = "https://github.com/devrelaicom/compact-analyzer"

[workspace.dependencies]
# syntax layer (compactp)
compactp_parser = "0.1.0-beta.1"
compactp_syntax = "0.1.0-beta.1"
compactp_diagnostics = "0.1.0-beta.1"
rowan = "0.16"
text-size = "1"

# LSP
lsp-server = "0.8"
lsp-types = "0.95.1"

# plumbing
anyhow = "1"
crossbeam-channel = "0.5"
serde_json = "1"

# dev
tempfile = "3"

# internal
analyzer-core = { path = "crates/analyzer-core" }
```

- [ ] **Step 2: Write `crates/analyzer-core/Cargo.toml` and a placeholder `src/lib.rs`**

`crates/analyzer-core/Cargo.toml`:

```toml
[package]
name = "analyzer-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
publish = false

[dependencies]
compactp_parser.workspace = true
compactp_diagnostics.workspace = true
rowan.workspace = true
text-size.workspace = true

[dev-dependencies]
compactp_syntax.workspace = true
tempfile.workspace = true
```

`crates/analyzer-core/src/lib.rs`:

```rust
//! Core analysis engine for compact-analyzer.
//!
//! Speaks byte offsets (`text_size::TextSize`/`TextRange`) exclusively.
//! No LSP types are allowed in this crate — protocol conversion lives in
//! the `compact-analyzer` binary.
```

- [ ] **Step 3: Write `crates/compact-analyzer/Cargo.toml` and a stub `src/main.rs`**

`crates/compact-analyzer/Cargo.toml`:

```toml
[package]
name = "compact-analyzer"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
publish = false

[[bin]]
name = "compact-analyzer"
path = "src/main.rs"

[dependencies]
analyzer-core.workspace = true
anyhow.workspace = true
crossbeam-channel.workspace = true
lsp-server.workspace = true
lsp-types.workspace = true
serde_json.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

`crates/compact-analyzer/src/main.rs`:

```rust
fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--version") {
        println!("compact-analyzer {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    eprintln!(
        "compact-analyzer {}: LSP server not implemented yet",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
```

- [ ] **Step 4: Write `.github/workflows/ci.yml` and ensure `/target` is gitignored**

`.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always

jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings

  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --workspace

  msrv:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: "1.90"
      - run: cargo check --workspace --all-targets
```

Ensure a target-dir ignore exists (the initial-commit `.gitignore` already has `target`; this is a no-op guard):

```bash
grep -qxE '/?target' .gitignore || printf '/target\n' >> .gitignore
```

- [ ] **Step 5: Build and verify**

Run: `cargo build --workspace && cargo run -p compact-analyzer -- --version`
Expected: builds cleanly (crates.io fetch of compactp beta on first run), then prints `compact-analyzer 0.1.0`

Run: `cargo test --workspace && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 0 tests, all green, no warnings

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates .github .gitignore
git commit -m "feat: scaffold cargo workspace with analyzer-core and compact-analyzer crates"
```

---

### Task 2: analyzer-core — FileId interning + Vfs

**Files:**
- Create: `crates/analyzer-core/src/vfs.rs`
- Modify: `crates/analyzer-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces (used by Tasks 4 and 6):
  - `analyzer_core::FileId` — `Copy + Eq + Hash + Ord + Debug` opaque id
  - `analyzer_core::Vfs::new() -> Vfs`
  - `Vfs::file_id(&mut self, path: &Path) -> FileId` (interns; same path → same id)
  - `Vfs::path(&self, file: FileId) -> &Path`
  - `Vfs::set_overlay(&mut self, file: FileId, text: String, version: i32)`
  - `Vfs::remove_overlay(&mut self, file: FileId)` (also clears cached disk content, forcing a fresh disk read)
  - `Vfs::overlay_version(&self, file: FileId) -> Option<i32>` (None when content is from disk / absent)
  - `Vfs::read(&mut self, file: FileId) -> Option<Arc<str>>` (overlay wins; else disk, cached; None if unreadable)

- [ ] **Step 1: Write the failing tests**

Append to the new file `crates/analyzer-core/src/vfs.rs` (tests first — the types don't exist yet, so this fails to compile, which is our red):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interns_same_path_to_same_id() {
        let mut vfs = Vfs::new();
        let a = vfs.file_id(std::path::Path::new("/tmp/a.compact"));
        let b = vfs.file_id(std::path::Path::new("/tmp/b.compact"));
        let a2 = vfs.file_id(std::path::Path::new("/tmp/a.compact"));
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(vfs.path(a), std::path::Path::new("/tmp/a.compact"));
    }

    #[test]
    fn overlay_wins_over_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.compact");
        std::fs::write(&path, "on disk").unwrap();

        let mut vfs = Vfs::new();
        let file = vfs.file_id(&path);
        assert_eq!(vfs.read(file).unwrap().as_ref(), "on disk");
        assert_eq!(vfs.overlay_version(file), None);

        vfs.set_overlay(file, "in editor".to_string(), 7);
        assert_eq!(vfs.read(file).unwrap().as_ref(), "in editor");
        assert_eq!(vfs.overlay_version(file), Some(7));
    }

    #[test]
    fn remove_overlay_falls_back_to_fresh_disk_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.compact");
        std::fs::write(&path, "v1 on disk").unwrap();

        let mut vfs = Vfs::new();
        let file = vfs.file_id(&path);
        vfs.set_overlay(file, "unsaved edit".to_string(), 2);
        assert_eq!(vfs.read(file).unwrap().as_ref(), "unsaved edit");

        // disk changed while the overlay was active
        std::fs::write(&path, "v2 on disk").unwrap();
        vfs.remove_overlay(file);
        assert_eq!(vfs.read(file).unwrap().as_ref(), "v2 on disk");
        assert_eq!(vfs.overlay_version(file), None);
    }

    #[test]
    fn missing_file_reads_none() {
        let mut vfs = Vfs::new();
        let file = vfs.file_id(std::path::Path::new("/nonexistent/nope.compact"));
        assert!(vfs.read(file).is_none());
    }
}
```

And register the module in `crates/analyzer-core/src/lib.rs` (append):

```rust
mod vfs;

pub use vfs::{FileId, Vfs};
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core`
Expected: FAIL — compile errors (`cannot find type Vfs`, `FileId`)

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analyzer-core/src/vfs.rs` (above the tests module):

```rust
//! In-memory file store with editor-overlay-over-disk semantics.
//!
//! Open editor buffers are "overlays" that shadow the file on disk. Files
//! referenced by `include`/`import` that aren't open in the editor are read
//! from disk and cached. M1 does not watch the disk: cached disk content is
//! only refreshed when an overlay is removed (`didClose`). File watching is
//! a later-milestone concern.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Interned identity of a file path. Cheap to copy and hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(u32);

#[derive(Debug)]
enum Content {
    Overlay { text: Arc<str>, version: i32 },
    Disk { text: Arc<str> },
}

#[derive(Debug, Default)]
pub struct Vfs {
    paths: Vec<PathBuf>,
    ids: HashMap<PathBuf, FileId>,
    contents: HashMap<FileId, Content>,
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns `path`, returning a stable id. Paths are compared exactly as
    /// given (the server hands us absolute paths derived from file:// URIs).
    pub fn file_id(&mut self, path: &Path) -> FileId {
        if let Some(&id) = self.ids.get(path) {
            return id;
        }
        let id = FileId(u32::try_from(self.paths.len()).expect("more than u32::MAX files"));
        self.paths.push(path.to_path_buf());
        self.ids.insert(path.to_path_buf(), id);
        id
    }

    pub fn path(&self, file: FileId) -> &Path {
        &self.paths[file.0 as usize]
    }

    pub fn set_overlay(&mut self, file: FileId, text: String, version: i32) {
        self.contents
            .insert(file, Content::Overlay { text: Arc::from(text), version });
    }

    /// Drops the overlay (and any cached disk content) so the next `read`
    /// hits the disk fresh.
    pub fn remove_overlay(&mut self, file: FileId) {
        self.contents.remove(&file);
    }

    pub fn overlay_version(&self, file: FileId) -> Option<i32> {
        match self.contents.get(&file) {
            Some(Content::Overlay { version, .. }) => Some(*version),
            _ => None,
        }
    }

    /// Current text of the file: overlay if present, else disk (cached).
    /// `None` if the file cannot be read.
    pub fn read(&mut self, file: FileId) -> Option<Arc<str>> {
        if let Some(content) = self.contents.get(&file) {
            let (Content::Overlay { text, .. } | Content::Disk { text }) = content;
            return Some(Arc::clone(text));
        }
        let text: Arc<str> = Arc::from(std::fs::read_to_string(self.path(file)).ok()?);
        self.contents.insert(file, Content::Disk { text: Arc::clone(&text) });
        Some(text)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core`
Expected: PASS — 4 tests

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings

```bash
git add crates/analyzer-core
git commit -m "feat(core): add Vfs with FileId interning and overlay-over-disk reads"
```

---

### Task 3: analyzer-core — LineIndex (byte offsets ↔ UTF-16 line/col)

**Files:**
- Create: `crates/analyzer-core/src/line_index.rs`
- Modify: `crates/analyzer-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces (used by Tasks 4, 5):
  - `analyzer_core::LineCol { pub line: u32, pub col: u32 }` — 0-based; `col` in **UTF-16 code units**
  - `analyzer_core::LineIndex::new(text: Arc<str>) -> LineIndex`
  - `LineIndex::line_col(&self, offset: TextSize) -> LineCol` (clamps out-of-range offsets to end of text)
  - `LineIndex::offset(&self, pos: LineCol) -> Option<TextSize>` (None if line out of range; col clamps to line end)
  - Re-exports from lib.rs: `pub use text_size::{TextRange, TextSize};`

- [ ] **Step 1: Write the failing tests**

Create `crates/analyzer-core/src/line_index.rs` with the tests module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn index(text: &str) -> LineIndex {
        LineIndex::new(Arc::from(text))
    }

    #[test]
    fn ascii_single_line() {
        let li = index("ledger count Field;");
        assert_eq!(li.line_col(TextSize::new(0)), LineCol { line: 0, col: 0 });
        assert_eq!(li.line_col(TextSize::new(12)), LineCol { line: 0, col: 12 });
        assert_eq!(li.offset(LineCol { line: 0, col: 12 }), Some(TextSize::new(12)));
    }

    #[test]
    fn multiline_lf() {
        let li = index("abc\ndef\nghi");
        assert_eq!(li.line_col(TextSize::new(4)), LineCol { line: 1, col: 0 });
        assert_eq!(li.line_col(TextSize::new(7)), LineCol { line: 1, col: 3 });
        assert_eq!(li.line_col(TextSize::new(8)), LineCol { line: 2, col: 0 });
        assert_eq!(li.offset(LineCol { line: 2, col: 1 }), Some(TextSize::new(9)));
    }

    #[test]
    fn crlf_line_endings() {
        let li = index("abc\r\ndef");
        // '\r' is part of line 0; 'd' starts line 1 at byte 5
        assert_eq!(li.line_col(TextSize::new(5)), LineCol { line: 1, col: 0 });
        assert_eq!(li.offset(LineCol { line: 1, col: 0 }), Some(TextSize::new(5)));
    }

    #[test]
    fn emoji_is_two_utf16_units() {
        // "/* 😀 */ ledger count Field;" — verified fixture:
        // byte offset 23 (where the colon is expected) is UTF-16 column 21
        let li = index("/* \u{1F600} */ ledger count Field;");
        assert_eq!(li.line_col(TextSize::new(23)), LineCol { line: 0, col: 21 });
        assert_eq!(li.offset(LineCol { line: 0, col: 21 }), Some(TextSize::new(23)));
    }

    #[test]
    fn out_of_range_clamps() {
        let li = index("abc");
        // offset past the end clamps to the end
        assert_eq!(li.line_col(TextSize::new(99)), LineCol { line: 0, col: 3 });
        // col past line end clamps to line end (excluding the newline)
        let li = index("ab\ncd");
        assert_eq!(li.offset(LineCol { line: 0, col: 99 }), Some(TextSize::new(2)));
        // line past the last line is None
        assert_eq!(li.offset(LineCol { line: 9, col: 0 }), None);
    }
}
```

Register in `crates/analyzer-core/src/lib.rs` (append):

```rust
mod line_index;

pub use line_index::{LineCol, LineIndex};
pub use text_size::{TextRange, TextSize};
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core`
Expected: FAIL — compile errors (`cannot find type LineIndex`)

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analyzer-core/src/line_index.rs`:

```rust
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
        LineCol { line: line as u32, col }
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core`
Expected: PASS — 9 tests (4 vfs + 5 line_index)

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings

```bash
git add crates/analyzer-core
git commit -m "feat(core): add LineIndex with UTF-16 aware position mapping"
```

---

### Task 4: analyzer-core — AnalysisHost (memoized parse + diagnostics)

**Files:**
- Create: `crates/analyzer-core/src/analysis.rs`
- Modify: `crates/analyzer-core/src/lib.rs`

**Interfaces:**
- Consumes: `Vfs`, `FileId` (Task 2), `LineIndex` (Task 3), `compactp_parser::parse`, `compactp_diagnostics::Diagnostic`
- Produces (used by Task 6):
  - `analyzer_core::FileAnalysis { pub green: rowan::GreenNode, pub diagnostics: Arc<Vec<Diagnostic>>, pub line_index: Arc<LineIndex> }` — `Clone` (all cheap Arc bumps)
  - `analyzer_core::AnalysisHost::new() -> AnalysisHost`
  - `AnalysisHost::vfs(&self) -> &Vfs` / `AnalysisHost::vfs_mut(&mut self) -> &mut Vfs`
  - `AnalysisHost::analyze(&mut self, file: FileId) -> Option<FileAnalysis>` (None if file unreadable; memoized on content hash)
  - Re-exports from lib.rs: `pub use compactp_diagnostics::{Diagnostic, DiagnosticCode, LabeledSpan, Severity};` (so the binary needs no direct compactp dependency)

- [ ] **Step 1: Write the failing tests**

Create `crates/analyzer-core/src/analysis.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Opens `text` as an overlay in a fresh host and returns (host, file).
    fn host_with(text: &str) -> (AnalysisHost, crate::FileId) {
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(Path::new("/test/doc.compact"));
        host.vfs_mut().set_overlay(file, text.to_string(), 1);
        (host, file)
    }

    #[test]
    fn valid_source_has_no_diagnostics() {
        let (mut host, file) = host_with("ledger count: Field;");
        let analysis = host.analyze(file).unwrap();
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn broken_source_reports_expected_colon() {
        // Verified fixture: zero-width span at offset 12
        let (mut host, file) = host_with("ledger count Field;");
        let analysis = host.analyze(file).unwrap();
        assert_eq!(analysis.diagnostics.len(), 1);
        let diag = &analysis.diagnostics[0];
        assert_eq!(diag.message, "expected COLON");
        assert_eq!(diag.primary_span.start(), crate::TextSize::new(12));
        assert_eq!(diag.primary_span.end(), crate::TextSize::new(12));
    }

    #[test]
    fn diagnostics_spans_stay_in_bounds() {
        let source = "@@@";
        let (mut host, file) = host_with(source);
        let analysis = host.analyze(file).unwrap();
        assert!(!analysis.diagnostics.is_empty());
        for diag in analysis.diagnostics.iter() {
            assert!(u32::from(diag.primary_span.end()) <= source.len() as u32);
        }
    }

    #[test]
    fn unchanged_content_hits_the_cache() {
        let (mut host, file) = host_with("ledger count: Field;");
        let first = host.analyze(file).unwrap();
        let second = host.analyze(file).unwrap();
        assert!(std::sync::Arc::ptr_eq(&first.diagnostics, &second.diagnostics));
    }

    #[test]
    fn edit_invalidates_the_cache() {
        let (mut host, file) = host_with("ledger count: Field;");
        let before = host.analyze(file).unwrap();
        assert!(before.diagnostics.is_empty());

        host.vfs_mut().set_overlay(file, "ledger count Field;".to_string(), 2);
        let after = host.analyze(file).unwrap();
        assert_eq!(after.diagnostics.len(), 1);
        assert!(!std::sync::Arc::ptr_eq(&before.diagnostics, &after.diagnostics));
    }

    #[test]
    fn tree_is_lossless() {
        let source = "/* comment */ ledger count: Field; // trailing";
        let (mut host, file) = host_with(source);
        let analysis = host.analyze(file).unwrap();
        let root = compactp_syntax::SyntaxNode::new_root(analysis.green);
        assert_eq!(root.text().to_string(), source);
    }

    #[test]
    fn unreadable_file_returns_none() {
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(Path::new("/nonexistent/nope.compact"));
        assert!(host.analyze(file).is_none());
    }
}
```

Register in `crates/analyzer-core/src/lib.rs` (append):

```rust
mod analysis;

pub use analysis::{AnalysisHost, FileAnalysis};
pub use compactp_diagnostics::{Diagnostic, DiagnosticCode, LabeledSpan, Severity};
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analyzer-core`
Expected: FAIL — compile errors (`cannot find type AnalysisHost`)

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analyzer-core/src/analysis.rs`:

```rust
//! Memoized recompute engine (spec Approach A).
//!
//! Parse results are cached per file, keyed by a content hash. An edit that
//! produces identical text is a cache hit; anything else recomputes that
//! file only. The syntax tree is stored as a `rowan::GreenNode` because
//! `SyntaxNode` is `!Send` — consumers rebuild a cursor with
//! `SyntaxNode::new_root(green)` (cheap).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use compactp_diagnostics::Diagnostic;
use rowan::GreenNode;

use crate::line_index::LineIndex;
use crate::vfs::{FileId, Vfs};

/// Everything M1 knows about one file. All fields are cheap to clone.
#[derive(Clone)]
pub struct FileAnalysis {
    pub green: GreenNode,
    pub diagnostics: Arc<Vec<Diagnostic>>,
    pub line_index: Arc<LineIndex>,
}

pub struct AnalysisHost {
    vfs: Vfs,
    cache: HashMap<FileId, (u64, FileAnalysis)>,
}

impl AnalysisHost {
    pub fn new() -> Self {
        Self { vfs: Vfs::new(), cache: HashMap::new() }
    }

    pub fn vfs(&self) -> &Vfs {
        &self.vfs
    }

    pub fn vfs_mut(&mut self) -> &mut Vfs {
        &mut self.vfs
    }

    /// Parses `file`, memoized on content. `None` if the file is unreadable.
    pub fn analyze(&mut self, file: FileId) -> Option<FileAnalysis> {
        let text = self.vfs.read(file)?;
        let hash = content_hash(&text);
        if let Some((cached_hash, analysis)) = self.cache.get(&file) {
            if *cached_hash == hash {
                return Some(analysis.clone());
            }
        }
        let result = compactp_parser::parse(&text);
        let analysis = FileAnalysis {
            green: result.green,
            diagnostics: Arc::new(result.errors),
            line_index: Arc::new(LineIndex::new(text)),
        };
        self.cache.insert(file, (hash, analysis.clone()));
        Some(analysis)
    }
}

impl Default for AnalysisHost {
    fn default() -> Self {
        Self::new()
    }
}

fn content_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analyzer-core`
Expected: PASS — 16 tests (4 vfs + 5 line_index + 7 analysis)

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings

```bash
git add crates/analyzer-core
git commit -m "feat(core): add AnalysisHost with content-hash memoized parsing"
```

---

### Task 5: compact-analyzer — protocol conversions (lsp_utils)

**Files:**
- Create: `crates/compact-analyzer/src/lsp_utils.rs`
- Modify: `crates/compact-analyzer/src/main.rs` (register module)

**Interfaces:**
- Consumes: `LineIndex`, `LineCol`, `TextRange`, `Diagnostic`, `Severity` (analyzer-core re-exports), `lsp_types`
- Produces (used by Task 6):
  - `lsp_utils::abs_path_from_uri(uri: &lsp_types::Url) -> Option<std::path::PathBuf>` (None for non-`file://` schemes)
  - `lsp_utils::range_to_lsp(li: &LineIndex, range: TextRange) -> lsp_types::Range`
  - `lsp_utils::diagnostic_to_lsp(d: &Diagnostic, li: &LineIndex, uri: &lsp_types::Url) -> lsp_types::Diagnostic`

- [ ] **Step 1: Write the failing tests**

Create `crates/compact-analyzer/src/lsp_utils.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::{Diagnostic, DiagnosticCode, LineIndex, TextRange, TextSize};
    use lsp_types::{DiagnosticSeverity, Position};
    use std::sync::Arc;

    #[test]
    fn rejects_non_file_uris() {
        let uri = lsp_types::Url::parse("untitled:Untitled-1").unwrap();
        assert!(abs_path_from_uri(&uri).is_none());
    }

    #[test]
    fn accepts_file_uris() {
        let uri = lsp_types::Url::parse("file:///tmp/x.compact").unwrap();
        assert_eq!(
            abs_path_from_uri(&uri),
            Some(std::path::PathBuf::from("/tmp/x.compact"))
        );
    }

    #[test]
    fn range_maps_to_utf16_columns() {
        // Verified fixture: byte offset 23 in this source is UTF-16 col 21
        let li = LineIndex::new(Arc::from("/* \u{1F600} */ ledger count Field;"));
        let range = range_to_lsp(&li, TextRange::new(TextSize::new(23), TextSize::new(23)));
        assert_eq!(range.start, Position::new(0, 21));
        assert_eq!(range.end, Position::new(0, 21));
    }

    #[test]
    fn diagnostic_carries_source_code_and_severity() {
        let li = LineIndex::new(Arc::from("ledger count Field;"));
        let uri = lsp_types::Url::parse("file:///tmp/x.compact").unwrap();
        let diag = Diagnostic::error(
            DiagnosticCode::new("E", 1),
            "expected COLON".to_string(),
            TextRange::new(TextSize::new(12), TextSize::new(12)),
        )
        .with_note("ledger declarations need a type".to_string());

        let lsp = diagnostic_to_lsp(&diag, &li, &uri);
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(lsp.source.as_deref(), Some("compact-analyzer"));
        assert_eq!(
            lsp.code,
            Some(lsp_types::NumberOrString::String("E1".to_string()))
        );
        assert_eq!(lsp.range.start, Position::new(0, 12));
        assert_eq!(
            lsp.message,
            "expected COLON\nnote: ledger declarations need a type"
        );
    }

    #[test]
    fn secondary_spans_become_related_information() {
        let li = LineIndex::new(Arc::from("ledger a: Field;\nledger a: Field;"));
        let uri = lsp_types::Url::parse("file:///tmp/x.compact").unwrap();
        let diag = Diagnostic::error(
            DiagnosticCode::new("E", 2),
            "duplicate name".to_string(),
            TextRange::new(TextSize::new(24), TextSize::new(25)),
        )
        .with_secondary(
            TextRange::new(TextSize::new(7), TextSize::new(8)),
            Some("first defined here".to_string()),
        );

        let lsp = diagnostic_to_lsp(&diag, &li, &uri);
        let related = lsp.related_information.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].message, "first defined here");
        assert_eq!(related[0].location.range.start, Position::new(0, 7));
    }
}
```

Register the module in `crates/compact-analyzer/src/main.rs` — add as the first line:

```rust
mod lsp_utils;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p compact-analyzer`
Expected: FAIL — compile errors (`cannot find function abs_path_from_uri`)

- [ ] **Step 3: Write the implementation**

Prepend to `crates/compact-analyzer/src/lsp_utils.rs`:

```rust
//! Conversions between analyzer-core's byte-offset world and LSP's
//! UTF-16 protocol types. This module is the ONLY place lsp_types and
//! analyzer_core types meet.

// Consumed by the server module in the next task.
#![allow(dead_code)]

use analyzer_core::{Diagnostic, LineIndex, Severity, TextRange};
use lsp_types::{
    DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Position, Range,
    Url,
};

/// Extracts an absolute filesystem path from a `file://` URI.
/// Non-file schemes (untitled:, vscode-notebook-cell:, …) return `None`;
/// the server ignores those documents.
pub(crate) fn abs_path_from_uri(uri: &Url) -> Option<std::path::PathBuf> {
    if uri.scheme() != "file" {
        return None;
    }
    uri.to_file_path().ok()
}

pub(crate) fn range_to_lsp(li: &LineIndex, range: TextRange) -> Range {
    let start = li.line_col(range.start());
    let end = li.line_col(range.end());
    Range::new(
        Position::new(start.line, start.col),
        Position::new(end.line, end.col),
    )
}

pub(crate) fn diagnostic_to_lsp(d: &Diagnostic, li: &LineIndex, uri: &Url) -> lsp_types::Diagnostic {
    let severity = match d.severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Note => DiagnosticSeverity::INFORMATION,
    };

    let mut message = d.message.clone();
    for note in &d.notes {
        message.push_str("\nnote: ");
        message.push_str(note);
    }

    let related_information = if d.secondary_spans.is_empty() {
        None
    } else {
        Some(
            d.secondary_spans
                .iter()
                .map(|s| DiagnosticRelatedInformation {
                    location: Location { uri: uri.clone(), range: range_to_lsp(li, s.span) },
                    message: s
                        .label
                        .clone()
                        .unwrap_or_else(|| "related location".to_string()),
                })
                .collect(),
        )
    };

    lsp_types::Diagnostic {
        range: range_to_lsp(li, d.primary_span),
        severity: Some(severity),
        code: Some(NumberOrString::String(d.code.to_string())),
        source: Some("compact-analyzer".to_string()),
        message,
        related_information,
        ..Default::default()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p compact-analyzer`
Expected: PASS — 5 tests

Note: `accepts_file_uris` uses a Unix-style path; on Windows `to_file_path` yields `\tmp\x.compact` from that URI. If the Windows CI job fails on this assertion, gate that one test with `#[cfg(unix)]` and add a `#[cfg(windows)]` twin using `file:///C:/tmp/x.compact` → `C:\tmp\x.compact`.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings

```bash
git add crates/compact-analyzer
git commit -m "feat(server): add byte-offset to UTF-16 LSP protocol conversions"
```

---

### Task 6: compact-analyzer — LSP server loop + black-box integration tests

**Files:**
- Create: `crates/compact-analyzer/src/server.rs`
- Create: `crates/compact-analyzer/tests/lsp_integration.rs`
- Modify: `crates/compact-analyzer/src/main.rs`
- Modify: `crates/compact-analyzer/src/lsp_utils.rs` (remove the temporary `#![allow(dead_code)]`)

**Interfaces:**
- Consumes: `AnalysisHost`, `FileId`, `Vfs` methods (Tasks 2/4), `lsp_utils` (Task 5), `lsp_server::{Connection, Message, Notification, Request, Response}`
- Produces: `server::run() -> anyhow::Result<()>` called from `main`; the shipped `compact-analyzer` binary behavior — initialize handshake with `serverInfo.name == "compact-analyzer"`, FULL-sync document tracking, debounced publishDiagnostics, diagnostics cleared on close, never-die request handling

- [ ] **Step 1: Write the failing black-box tests (client harness + scenarios)**

Create `crates/compact-analyzer/tests/lsp_integration.rs`:

```rust
//! Black-box LSP tests: spawn the real binary, speak the protocol over
//! stdio, assert observable behavior. No internal APIs.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next_id: i64,
}

impl Client {
    fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_compact-analyzer"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to spawn compact-analyzer");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, incoming) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(msg) = read_message(&mut reader) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        Client { child, stdin, incoming, next_id: 0 }
    }

    fn initialize(&mut self) {
        let response = self.request("initialize", json!({"capabilities": {}}));
        assert!(response.get("result").is_some(), "initialize failed: {response}");
        self.notify("initialized", json!({}));
    }

    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let msg = self.recv();
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                return msg;
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn recv(&mut self) -> Value {
        self.incoming
            .recv_timeout(Duration::from_secs(10))
            .expect("timed out waiting for a server message")
    }

    /// Skips unrelated messages until a notification with `method` arrives,
    /// returning its params.
    fn wait_for_notification(&mut self, method: &str) -> Value {
        loop {
            let msg = self.recv();
            if msg.get("method").and_then(Value::as_str) == Some(method) {
                return msg["params"].clone();
            }
        }
    }

    fn shutdown(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        let status = self.child.wait().expect("server did not exit");
        assert!(status.success(), "server exited with failure: {status:?}");
    }
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length: ") {
            content_length = rest.parse().ok();
        }
    }
    let mut buf = vec![0u8; content_length?];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

/// Creates a temp dir + file URI for a document (file need not exist on
/// disk — content arrives via didOpen).
fn temp_doc() -> (tempfile::TempDir, lsp_types::Url) {
    let dir = tempfile::tempdir().unwrap();
    let uri = lsp_types::Url::from_file_path(dir.path().join("doc.compact")).unwrap();
    (dir, uri)
}

fn did_open(client: &mut Client, uri: &lsp_types::Url, version: i64, text: &str) {
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": uri, "languageId": "compact", "version": version, "text": text,
        }}),
    );
}

#[test]
fn initialize_reports_server_info() {
    let mut client = Client::start();
    let response = client.request("initialize", json!({"capabilities": {}}));
    assert_eq!(response["result"]["serverInfo"]["name"], "compact-analyzer");
    client.notify("initialized", json!({}));
    client.shutdown();
}

#[test]
fn publishes_diagnostics_then_clears_after_fix() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    // Verified fixture: missing colon → "expected COLON" at offset 12
    did_open(&mut client, &uri, 1, "ledger count Field;");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert_eq!(params["uri"], json!(uri));
    assert_eq!(params["version"], 1);
    let diags = params["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["message"], "expected COLON");
    assert_eq!(diags[0]["source"], "compact-analyzer");
    assert_eq!(diags[0]["severity"], 1); // Error
    assert_eq!(diags[0]["code"], "E1");
    assert_eq!(diags[0]["range"]["start"], json!({"line": 0, "character": 12}));

    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"text": "ledger count: Field;"}],
        }),
    );
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert_eq!(params["version"], 2);
    assert!(params["diagnostics"].as_array().unwrap().is_empty());

    client.shutdown();
}

#[test]
fn positions_are_utf16_code_units() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    // Verified fixture: error at byte offset 23 == UTF-16 column 21
    did_open(&mut client, &uri, 1, "/* \u{1F600} */ ledger count Field;");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["range"]["start"], json!({"line": 0, "character": 21}));

    client.shutdown();
}

#[test]
fn clears_diagnostics_on_close() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    did_open(&mut client, &uri, 1, "@@@");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert!(!params["diagnostics"].as_array().unwrap().is_empty());

    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": uri}}),
    );
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert!(params["diagnostics"].as_array().unwrap().is_empty());

    client.shutdown();
}

#[test]
fn unknown_requests_get_an_error_not_a_crash() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    let response = client.request("textDocument/hover", json!({
        "textDocument": {"uri": uri},
        "position": {"line": 0, "character": 0},
    }));
    assert!(response.get("error").is_some(), "expected error response: {response}");

    // Server is still alive and functional afterwards
    did_open(&mut client, &uri, 1, "ledger count: Field;");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert!(params["diagnostics"].as_array().unwrap().is_empty());

    client.shutdown();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p compact-analyzer --test lsp_integration`
Expected: FAIL — the stub binary exits immediately, so tests fail with "timed out waiting for a server message" (or a broken-pipe panic on send). Both are the correct red.

- [ ] **Step 3: Write the server implementation**

Create `crates/compact-analyzer/src/server.rs`:

```rust
//! The LSP server: synchronous main loop over stdio.
//!
//! Never-die contract: every message dispatch runs under catch_unwind; a
//! panicked request gets an InternalError response and the loop continues.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use analyzer_core::{AnalysisHost, FileId};
use crossbeam_channel::RecvTimeoutError;
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, Url,
};

use crate::lsp_utils;

const DEBOUNCE: Duration = Duration::from_millis(150);

// JSON-RPC error codes (avoid depending on lsp-server exporting them).
const METHOD_NOT_FOUND: i32 = -32601;
const INTERNAL_ERROR: i32 = -32603;

pub(crate) fn run() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();

    let (initialize_id, _initialize_params) = connection.initialize_start()?;
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        ..Default::default()
    };
    let initialize_result = serde_json::json!({
        "capabilities": capabilities,
        "serverInfo": {
            "name": "compact-analyzer",
            "version": env!("CARGO_PKG_VERSION"),
        },
    });
    connection.initialize_finish(initialize_id, initialize_result)?;
    eprintln!("compact-analyzer: initialized");

    let mut state = GlobalState::new(connection.sender.clone());

    loop {
        // When diagnostics are pending, wait only until the debounce
        // deadline; otherwise block until the next message.
        let msg = if let Some(deadline) = state.debounce_deadline {
            let now = Instant::now();
            if deadline <= now {
                state.flush_pending_diagnostics();
                continue;
            }
            match connection.receiver.recv_timeout(deadline - now) {
                Ok(msg) => msg,
                Err(RecvTimeoutError::Timeout) => {
                    state.flush_pending_diagnostics();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match connection.receiver.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            }
        };

        if let Message::Request(req) = &msg {
            if connection.handle_shutdown(req)? {
                break;
            }
        }
        state.dispatch(msg);
    }

    io_threads.join()?;
    eprintln!("compact-analyzer: shut down");
    Ok(())
}

struct GlobalState {
    sender: crossbeam_channel::Sender<Message>,
    host: AnalysisHost,
    /// Documents currently open in the editor, by URI.
    open_files: HashMap<Url, FileId>,
    /// Files with not-yet-published diagnostics.
    pending_diagnostics: HashSet<FileId>,
    /// When set, diagnostics are published once this instant passes.
    debounce_deadline: Option<Instant>,
}

impl GlobalState {
    fn new(sender: crossbeam_channel::Sender<Message>) -> Self {
        Self {
            sender,
            host: AnalysisHost::new(),
            open_files: HashMap::new(),
            pending_diagnostics: HashSet::new(),
            debounce_deadline: None,
        }
    }

    /// Never-die wrapper: panics are logged, requests still get a response.
    fn dispatch(&mut self, msg: Message) {
        let request_id = match &msg {
            Message::Request(req) => Some(req.id.clone()),
            _ => None,
        };
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle_message(msg)));
        if let Err(panic) = result {
            eprintln!(
                "compact-analyzer: panic while handling message: {}",
                panic_message(panic.as_ref())
            );
            if let Some(id) = request_id {
                self.respond(Response::new_err(
                    id,
                    INTERNAL_ERROR,
                    "internal error (panic); see server logs".to_string(),
                ));
            }
        }
    }

    fn handle_message(&mut self, msg: Message) {
        match msg {
            Message::Request(req) => self.handle_request(req),
            Message::Notification(not) => self.handle_notification(not),
            Message::Response(_) => {} // we never send server-to-client requests in M1
        }
    }

    fn handle_request(&mut self, req: Request) {
        // shutdown is intercepted in the main loop; nothing else is
        // supported in M1.
        self.respond(Response::new_err(
            req.id,
            METHOD_NOT_FOUND,
            format!("method not supported: {}", req.method),
        ));
    }

    fn handle_notification(&mut self, not: Notification) {
        match not.method.as_str() {
            "textDocument/didOpen" => {
                if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(not.params)
                {
                    self.did_open(params);
                }
            }
            "textDocument/didChange" => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeTextDocumentParams>(not.params)
                {
                    self.did_change(params);
                }
            }
            "textDocument/didClose" => {
                if let Ok(params) =
                    serde_json::from_value::<DidCloseTextDocumentParams>(not.params)
                {
                    self.did_close(params);
                }
            }
            // Recognized but irrelevant in M1.
            "textDocument/didSave" | "initialized" | "$/setTrace" | "$/cancelRequest" => {}
            other => eprintln!("compact-analyzer: ignoring notification {other}"),
        }
    }

    fn did_open(&mut self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some(path) = lsp_utils::abs_path_from_uri(&uri) else {
            eprintln!("compact-analyzer: ignoring non-file document {uri}");
            return;
        };
        let file = self.host.vfs_mut().file_id(&path);
        self.host
            .vfs_mut()
            .set_overlay(file, params.text_document.text, params.text_document.version);
        self.open_files.insert(uri, file);
        self.schedule_diagnostics(file);
    }

    fn did_change(&mut self, params: DidChangeTextDocumentParams) {
        let Some(&file) = self.open_files.get(&params.text_document.uri) else {
            return;
        };
        // FULL sync: the last change contains the entire new document text.
        let Some(change) = params.content_changes.into_iter().next_back() else {
            return;
        };
        self.host
            .vfs_mut()
            .set_overlay(file, change.text, params.text_document.version);
        self.schedule_diagnostics(file);
    }

    fn did_close(&mut self, params: DidCloseTextDocumentParams) {
        let Some(file) = self.open_files.remove(&params.text_document.uri) else {
            return;
        };
        self.host.vfs_mut().remove_overlay(file);
        self.pending_diagnostics.remove(&file);
        // Clear this document's diagnostics in the editor.
        self.publish(PublishDiagnosticsParams {
            uri: params.text_document.uri,
            diagnostics: vec![],
            version: None,
        });
    }

    fn schedule_diagnostics(&mut self, file: FileId) {
        self.pending_diagnostics.insert(file);
        self.debounce_deadline = Some(Instant::now() + DEBOUNCE);
    }

    fn flush_pending_diagnostics(&mut self) {
        self.debounce_deadline = None;
        let files: Vec<FileId> = self.pending_diagnostics.drain().collect();
        for file in files {
            let Some(uri) = self
                .open_files
                .iter()
                .find(|(_, &f)| f == file)
                .map(|(uri, _)| uri.clone())
            else {
                continue; // closed before the debounce fired
            };
            let Some(analysis) = self.host.analyze(file) else {
                continue;
            };
            let version = self.host.vfs().overlay_version(file);
            let diagnostics = analysis
                .diagnostics
                .iter()
                .map(|d| lsp_utils::diagnostic_to_lsp(d, &analysis.line_index, &uri))
                .collect();
            self.publish(PublishDiagnosticsParams { uri, diagnostics, version });
        }
    }

    fn publish(&self, params: PublishDiagnosticsParams) {
        self.send_notification::<lsp_types::notification::PublishDiagnostics>(params);
    }

    fn send_notification<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        let not = Notification::new(N::METHOD.to_string(), params);
        let _ = self.sender.send(Message::Notification(not));
    }

    fn respond(&self, response: Response) {
        let _ = self.sender.send(Message::Response(response));
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}
```

Replace `crates/compact-analyzer/src/main.rs` in full:

```rust
mod lsp_utils;
mod server;

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--version") {
        println!("compact-analyzer {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    eprintln!(
        "compact-analyzer {}: starting LSP server on stdio",
        env!("CARGO_PKG_VERSION")
    );
    server::run()
}
```

Remove the temporary `#![allow(dead_code)]` line (and its comment) from `crates/compact-analyzer/src/lsp_utils.rs` — the server now uses every function.

- [ ] **Step 4: Run all tests to verify they pass**

Run: `cargo test -p compact-analyzer`
Expected: PASS — 5 unit tests + 5 integration tests

Run: `cargo test --workspace`
Expected: PASS — all crates green

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings

```bash
git add crates/compact-analyzer
git commit -m "feat(server): LSP main loop with debounced diagnostics and never-die dispatch"
```

---

### Task 7: License, README, and editor smoke instructions

**Files:**
- Create: `LICENSE`
- Modify: `README.md`

**Interfaces:**
- Consumes: the working binary from Task 6
- Produces: a repo a stranger can clone, build, and attach to Neovim in five minutes

- [ ] **Step 1: Add the MIT license**

Create `LICENSE` with the standard MIT text (copyright line: `Copyright (c) 2026 Aaron Bassett`), matching compactp's licensing. (Assumption flagged during planning: MIT to match compactp. If a different license is wanted, swap this file — nothing else in M1 depends on it.)

```text
MIT License

Copyright (c) 2026 Aaron Bassett

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- [ ] **Step 2: Replace `README.md`**

```markdown
# compact-analyzer

A [rust-analyzer](https://rust-analyzer.github.io/)-style language server for
the [Compact](https://docs.midnight.network/compact) smart contract language
(Midnight Network).

Built on [compactp](https://github.com/devrelaicom/compactp), a lossless,
error-tolerant parser frontend for Compact.

## Status: M1 — real-time syntax diagnostics

The server currently provides:

- Live syntax error diagnostics as you type (no compiler round-trip)
- Correct UTF-16 positions, resilient to any input (the server never dies
  on malformed code)

Name resolution, navigation, completion, compiler-integrated diagnostics,
and a VS Code extension are on the roadmap — see
`docs/superpowers/specs/2026-07-06-compact-analyzer-design.md`.

## Build

Requires Rust 1.90+.

```sh
cargo build --release
./target/release/compact-analyzer --version
```

The server speaks LSP over stdio.

## Try it in Neovim (0.10+)

```lua
vim.filetype.add({ extension = { compact = "compact" } })
vim.api.nvim_create_autocmd("FileType", {
  pattern = "compact",
  callback = function()
    vim.lsp.start({
      name = "compact-analyzer",
      cmd = { "/path/to/compact-analyzer" },
    })
  end,
})
```

Open a `.compact` file and introduce a syntax error — diagnostics appear as
you type.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

To develop against a local compactp checkout, add (but never commit):

```toml
# Cargo.toml (workspace root)
[patch.crates-io]
compactp_parser = { path = "../compactp/crates/compactp_parser" }
compactp_syntax = { path = "../compactp/crates/compactp_syntax" }
compactp_diagnostics = { path = "../compactp/crates/compactp_diagnostics" }
```

## License

MIT
```

- [ ] **Step 3: Full verification suite**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: everything green

Run: `cargo run -p compact-analyzer -- --version`
Expected: `compact-analyzer 0.1.0`

- [ ] **Step 4: Commit**

```bash
git add LICENSE README.md
git commit -m "docs: add license, README with build and Neovim attach instructions"
```

---

## Self-review notes (completed during planning)

- **Spec coverage (M1 slice):** VFS overlay (§3.2) → Task 2; UTF-16 line index (§3.2, binary-only conversion §3.4-layering) → Tasks 3/5; memoized parse cache (§2 Approach A) → Task 4; lsp-server stack, debounce, never-die catch_unwind, stale-version fields (§3.2/§5) → Task 6; black-box LSP tests incl. multibyte fixtures (§8) → Task 6. Deliberately deferred per milestone map: item index/name resolution (M2), all navigation/completion features (M2/M3), toolchain (M4), extension/distribution (M5), corpus smoke (M2, when the corpus-consuming features exist), file watching (later milestone, documented in Task 2's module docs).
- **Stale-result protection (spec §5):** M1's only push channel is publishDiagnostics, which carries the document version (Task 6); request-level version tagging becomes relevant with M2's request handlers.
- **Fixtures:** all four parser fixtures were verified against compactp 0.1.0-beta.1 via its CLI during planning — including the surprise that `@@@`-style top-level garbage anchors at offset 0 (documented in Global Constraints so nobody "fixes" a correct test).
- **Type consistency:** `FileId`/`Vfs`/`LineIndex`/`AnalysisHost`/`FileAnalysis` signatures in Task 2-4 Interfaces blocks match their uses in Tasks 5-6 (checked field-by-field).
