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
        self.contents.insert(
            file,
            Content::Overlay {
                text: Arc::from(text),
                version,
            },
        );
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
        self.contents.insert(
            file,
            Content::Disk {
                text: Arc::clone(&text),
            },
        );
        Some(text)
    }
}

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
