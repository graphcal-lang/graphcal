//! Overlay filesystem that layers an in-memory file over a base reader.

use std::io;
use std::path::{Path, PathBuf};

use crate::FileSystemReader;

/// A filesystem reader that intercepts reads to a single overlaid path,
/// returning in-memory content instead of delegating to the base reader.
///
/// Used by the LSP (unsaved editor buffer).
pub struct OverlayFileSystem<F> {
    base: F,
    /// Canonical path of the overlaid file.
    overlay_path: PathBuf,
    /// In-memory content for the overlaid file.
    overlay_content: String,
}

impl<F: FileSystemReader> OverlayFileSystem<F> {
    /// Create a new overlay filesystem.
    ///
    /// `overlay_path` must be a canonical path (as returned by
    /// [`FileSystemReader::canonicalize`] on the base reader).
    pub const fn new(base: F, overlay_path: PathBuf, overlay_content: String) -> Self {
        Self {
            base,
            overlay_path,
            overlay_content,
        }
    }
}

impl<F: FileSystemReader> FileSystemReader for OverlayFileSystem<F> {
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error> {
        if path == self.overlay_path {
            Ok(self.overlay_content.clone())
        } else {
            self.base.read_to_string(path)
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        self.base.canonicalize(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        if path == self.overlay_path {
            return true;
        }
        self.base.is_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        if path == self.overlay_path {
            return true;
        }
        self.base.exists(path)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

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
}
