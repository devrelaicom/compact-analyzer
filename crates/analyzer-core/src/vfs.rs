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

impl FileId {
    #[cfg(test)]
    pub(crate) fn from_raw_for_test(n: u32) -> FileId {
        FileId(n)
    }
}

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
    generation: u64,
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
        self.generation += 1;
    }

    /// Drops the overlay (and any cached disk content) so the next `read`
    /// hits the disk fresh.
    pub fn remove_overlay(&mut self, file: FileId) {
        self.contents.remove(&file);
        self.generation += 1;
    }

    pub fn overlay_version(&self, file: FileId) -> Option<i32> {
        match self.contents.get(&file) {
            Some(Content::Overlay { version, .. }) => Some(*version),
            _ => None,
        }
    }

    /// Monotonic revision, bumped by every content mutation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Drops cached *disk* content so the next `read` re-reads from disk. An
    /// active overlay is left untouched (it always wins). Called by the file
    /// watcher on an external change.
    pub fn invalidate_disk(&mut self, file: FileId) {
        if matches!(self.contents.get(&file), Some(Content::Disk { .. })) {
            self.contents.remove(&file);
        }
        self.generation += 1;
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

    #[test]
    fn generation_bumps_only_on_mutation() {
        let mut vfs = Vfs::new();
        let g0 = vfs.generation();
        let f = vfs.file_id(std::path::Path::new("/t/a.compact"));
        assert_eq!(vfs.generation(), g0); // interning is not a mutation
        vfs.set_overlay(f, "x".to_string(), 1);
        let g1 = vfs.generation();
        assert!(g1 > g0);
        vfs.remove_overlay(f);
        assert!(vfs.generation() > g1);
    }

    #[test]
    fn invalidate_disk_rereads_but_overlay_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.compact");
        std::fs::write(&path, "v1").unwrap();
        let mut vfs = Vfs::new();
        let f = vfs.file_id(&path);
        assert_eq!(vfs.read(f).unwrap().as_ref(), "v1"); // caches disk content

        std::fs::write(&path, "v2").unwrap();
        vfs.invalidate_disk(f);
        assert_eq!(vfs.read(f).unwrap().as_ref(), "v2"); // re-read after invalidate

        vfs.set_overlay(f, "edit".to_string(), 1);
        std::fs::write(&path, "v3").unwrap();
        vfs.invalidate_disk(f);
        assert_eq!(vfs.read(f).unwrap().as_ref(), "edit"); // overlay is untouched
    }

    #[test]
    #[should_panic]
    fn path_panics_on_foreign_file_id() {
        // `path` indexes by FileId; an id minted by a different Vfs is out of
        // range and panics (documented precondition — callers pass ids from
        // the same Vfs).
        let mut a = Vfs::new();
        let _ = a.file_id(std::path::Path::new("/t/only.compact")); // a has id 0
        let mut b = Vfs::new();
        let _ = b.file_id(std::path::Path::new("/t/x.compact"));
        let foreign = b.file_id(std::path::Path::new("/t/y.compact")); // id 1 in b
        let _ = a.path(foreign); // id 1 is out of range for `a`
    }
}
