//! Overlay filesystem that layers in-memory files over a base reader.

use std::io;
use std::path::{Path, PathBuf};

use crate::FileSystemReader;

/// One overlaid file: its identity (canonical when possible) plus the
/// in-memory content served instead of the on-disk bytes.
struct OverlayEntry {
    /// The canonical form of the overlay path (or the raw form if the base
    /// reader could not canonicalize it).
    canonical_path: PathBuf,
    /// `true` if the base reader successfully canonicalized the overlay path
    /// at construction time. When `false`, the overlay file does not exist
    /// on disk yet and we match strictly on the raw path.
    base_canonicalized: bool,
    content: String,
}

/// A filesystem reader that intercepts reads to a set of overlaid paths,
/// returning in-memory content instead of delegating to the base reader.
///
/// Used by the LSP to serve unsaved editor buffers — the analyzed document
/// plus every other open document, so cross-file analysis sees what the
/// user sees rather than stale disk content.
///
/// # Path identity
///
/// Each overlay is identified by its canonical path. The constructor attempts
/// to canonicalize the incoming path using the base reader; if that succeeds,
/// every subsequent operation first canonicalizes the incoming path and
/// compares canonical forms. This makes equivalent spellings (`./main.gcl`,
/// `/tmp/../tmp/main.gcl`, symlinks) all resolve to the overlay.
///
/// If canonicalization fails — for example, when the overlay file has never
/// been saved to disk yet — the stored path is kept verbatim and matching
/// falls back to strict path equality. In that mode the reader also reports
/// the overlay path as its own canonical form, so callers that pass the raw
/// path continue to find it.
pub struct OverlayFileSystem<F> {
    base: F,
    overlays: Vec<OverlayEntry>,
}

impl<F: FileSystemReader> OverlayFileSystem<F> {
    /// Create an overlay filesystem serving a single in-memory file.
    pub fn new(base: F, overlay_path: PathBuf, overlay_content: String) -> Self {
        Self::with_overlays(base, [(overlay_path, overlay_content)])
    }

    /// Create an overlay filesystem serving several in-memory files.
    ///
    /// Paths may be any spelling that identifies the files; they do not need
    /// to be pre-canonicalized. When two overlays identify the same file, the
    /// first one wins.
    pub fn with_overlays(base: F, overlays: impl IntoIterator<Item = (PathBuf, String)>) -> Self {
        let overlays = overlays
            .into_iter()
            .map(|(path, content)| {
                let (canonical_path, base_canonicalized) = base
                    .canonicalize(&path)
                    .map_or((path, false), |canonical| (canonical, true));
                OverlayEntry {
                    canonical_path,
                    base_canonicalized,
                    content,
                }
            })
            .collect();
        Self { base, overlays }
    }

    /// The overlay entry `path` refers to, if any.
    fn overlay(&self, path: &Path) -> Option<&OverlayEntry> {
        if let Some(entry) = self
            .overlays
            .iter()
            .find(|entry| entry.canonical_path == path)
        {
            return Some(entry);
        }
        let canonical = self.base.canonicalize(path).ok()?;
        self.overlays
            .iter()
            .find(|entry| entry.base_canonicalized && entry.canonical_path == canonical)
    }
}

impl<F: FileSystemReader> FileSystemReader for OverlayFileSystem<F> {
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error> {
        self.overlay(path).map_or_else(
            || self.base.read_to_string(path),
            |entry| Ok(entry.content.clone()),
        )
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        // Short-circuit for overlays so unsaved buffers (which do not exist
        // on disk) still resolve to a stable identity.
        if let Some(entry) = self.overlay(path) {
            return Ok(entry.canonical_path.clone());
        }
        self.base.canonicalize(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        if self.overlay(path).is_some() {
            return true;
        }
        self.base.is_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        if self.overlay(path).is_some() {
            return true;
        }
        self.base.exists(path)
    }
}

#[cfg(test)]
mod tests {
    use crate::InMemoryFileSystem;

    use super::*;

    #[test]
    fn overlay_intercepts_read() {
        let mut base = InMemoryFileSystem::new();
        base.add_file(
            PathBuf::from("/project/main.gcl"),
            "original content".to_string(),
        );

        let fs = OverlayFileSystem::new(
            base,
            PathBuf::from("/project/main.gcl"),
            "overlay content".to_string(),
        );

        assert_eq!(
            fs.read_to_string(Path::new("/project/main.gcl")).unwrap(),
            "overlay content"
        );
    }

    #[test]
    fn overlay_delegates_other_files() {
        let mut base = InMemoryFileSystem::new();
        base.add_file(
            PathBuf::from("/project/main.gcl"),
            "main content".to_string(),
        );
        base.add_file(
            PathBuf::from("/project/helper.gcl"),
            "helper content".to_string(),
        );

        let fs = OverlayFileSystem::new(
            base,
            PathBuf::from("/project/main.gcl"),
            "overlay".to_string(),
        );

        assert_eq!(
            fs.read_to_string(Path::new("/project/helper.gcl")).unwrap(),
            "helper content"
        );
    }

    #[test]
    fn overlay_is_file_returns_true_for_overlay_path() {
        let base = InMemoryFileSystem::new();
        let fs = OverlayFileSystem::new(
            base,
            PathBuf::from("/project/main.gcl"),
            "content".to_string(),
        );

        assert!(fs.is_file(Path::new("/project/main.gcl")));
    }

    #[test]
    fn multiple_overlays_intercept_reads() {
        let mut base = InMemoryFileSystem::new();
        base.add_file(PathBuf::from("/project/main.gcl"), "main disk".to_string());
        base.add_file(PathBuf::from("/project/lib.gcl"), "lib disk".to_string());
        base.add_file(
            PathBuf::from("/project/other.gcl"),
            "other disk".to_string(),
        );

        let fs = OverlayFileSystem::with_overlays(
            base,
            [
                (PathBuf::from("/project/main.gcl"), "main buf".to_string()),
                (PathBuf::from("/project/lib.gcl"), "lib buf".to_string()),
            ],
        );

        assert_eq!(
            fs.read_to_string(Path::new("/project/main.gcl")).unwrap(),
            "main buf"
        );
        assert_eq!(
            fs.read_to_string(Path::new("/project/lib.gcl")).unwrap(),
            "lib buf"
        );
        assert_eq!(
            fs.read_to_string(Path::new("/project/other.gcl")).unwrap(),
            "other disk"
        );
    }

    #[test]
    fn canonicalize_on_overlay_path_succeeds_when_file_not_on_disk() {
        // B7: unsaved LSP buffer — the file does not exist via the base reader,
        // but canonicalize on the overlay path must still succeed so the
        // loader can identify the buffer.
        let base = InMemoryFileSystem::new();
        let fs = OverlayFileSystem::new(
            base,
            PathBuf::from("/project/unsaved.gcl"),
            "unsaved".to_string(),
        );

        let canonical = fs.canonicalize(Path::new("/project/unsaved.gcl")).unwrap();
        assert_eq!(canonical, PathBuf::from("/project/unsaved.gcl"));
        assert!(fs.exists(Path::new("/project/unsaved.gcl")));
        assert_eq!(
            fs.read_to_string(Path::new("/project/unsaved.gcl"))
                .unwrap(),
            "unsaved"
        );
    }
}
