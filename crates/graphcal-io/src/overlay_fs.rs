//! Overlay filesystem that layers an in-memory file over a base reader.

use std::io;
use std::path::{Path, PathBuf};

use crate::FileSystemReader;

/// A filesystem reader that intercepts reads to a single overlaid path,
/// returning in-memory content instead of delegating to the base reader.
///
/// Used by the LSP to serve unsaved editor buffers.
///
/// # Path identity
///
/// The overlay is identified by its canonical path. The constructor attempts
/// to canonicalize the incoming `overlay_path` using the base reader; if that
/// succeeds, every subsequent operation first canonicalizes the incoming path
/// and compares canonical forms. This makes equivalent spellings
/// (`./main.gcl`, `/tmp/../tmp/main.gcl`, symlinks) all resolve to the overlay.
///
/// If canonicalization fails — for example, when the overlay file has never
/// been saved to disk yet — the stored path is kept verbatim and matching
/// falls back to strict path equality. In that mode the constructor also
/// reports the overlay path as its own canonical form, so callers that pass
/// the raw path continue to find it.
pub struct OverlayFileSystem<F> {
    base: F,
    /// The canonical form of the overlay path (or the raw form if the base
    /// reader could not canonicalize it).
    canonical_overlay_path: PathBuf,
    /// `true` if the base reader successfully canonicalized the overlay path
    /// at construction time. When `false`, the overlay file does not exist
    /// on disk yet and we match strictly on the raw path.
    base_canonicalized: bool,
    overlay_content: String,
}

impl<F: FileSystemReader> OverlayFileSystem<F> {
    /// Create a new overlay filesystem.
    ///
    /// `overlay_path` may be any path that identifies the file being edited;
    /// it does not need to be pre-canonicalized. If the file exists on disk,
    /// the overlay is keyed on the canonical path so equivalent spellings
    /// still hit the overlay. If the file does not exist yet (unsaved LSP
    /// buffer), the raw path is used.
    pub fn new(base: F, overlay_path: PathBuf, overlay_content: String) -> Self {
        let (canonical_overlay_path, base_canonicalized) = base
            .canonicalize(&overlay_path)
            .map_or((overlay_path, false), |canonical| (canonical, true));
        Self {
            base,
            canonical_overlay_path,
            base_canonicalized,
            overlay_content,
        }
    }

    /// Return `true` if `path` refers to the overlaid file.
    fn is_overlay(&self, path: &Path) -> bool {
        if path == self.canonical_overlay_path {
            return true;
        }
        if self.base_canonicalized
            && let Ok(canonical) = self.base.canonicalize(path)
        {
            return canonical == self.canonical_overlay_path;
        }
        false
    }
}

impl<F: FileSystemReader> FileSystemReader for OverlayFileSystem<F> {
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error> {
        if self.is_overlay(path) {
            Ok(self.overlay_content.clone())
        } else {
            self.base.read_to_string(path)
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        // Short-circuit for the overlay so unsaved buffers (which do not exist
        // on disk) still resolve to a stable identity.
        if self.is_overlay(path) {
            return Ok(self.canonical_overlay_path.clone());
        }
        self.base.canonicalize(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        if self.is_overlay(path) {
            return true;
        }
        self.base.is_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        if self.is_overlay(path) {
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
