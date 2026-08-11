//! Overlay filesystem that layers in-memory files over a base reader.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::{
    ByteLimit, CancellationSignal, EntryLimit, FileSystemEntryKind, FileSystemReadError,
    FileSystemReader,
};

struct OverlayEntry {
    canonical_path: PathBuf,
    base_canonicalized: bool,
    content: String,
}

/// Filesystem reader that serves in-memory editor buffers before delegating to
/// a base capability.
pub struct OverlayFileSystem<F> {
    base: F,
    overlays: Vec<OverlayEntry>,
}

impl<F: FileSystemReader> OverlayFileSystem<F> {
    /// Create an overlay filesystem serving one in-memory UTF-8 file.
    pub fn new(base: F, overlay_path: PathBuf, overlay_content: String) -> Self {
        Self::with_overlays(base, [(overlay_path, overlay_content)])
    }

    /// Create an overlay filesystem serving several in-memory files. The first
    /// overlay for one canonical identity wins.
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

    fn overlay_child_names(&self, directory: &Path) -> BTreeSet<OsString> {
        self.overlays
            .iter()
            .filter_map(|entry| {
                let relative = entry.canonical_path.strip_prefix(directory).ok()?;
                match relative.components().next() {
                    Some(Component::Normal(name)) => Some(name.to_os_string()),
                    _ => None,
                }
            })
            .collect()
    }

    fn has_overlay_descendant(&self, path: &Path) -> bool {
        self.overlays
            .iter()
            .any(|entry| entry.canonical_path.starts_with(path) && entry.canonical_path != path)
    }
}

impl<F: FileSystemReader> FileSystemReader for OverlayFileSystem<F> {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<u8>, FileSystemReadError> {
        if let Some(entry) = self.overlay(path) {
            if cancellation.is_cancelled() {
                return Err(FileSystemReadError::Cancelled);
            }
            if entry.content.len() as u64 > limit.get() {
                return Err(FileSystemReadError::ByteLimitExceeded { limit });
            }
            Ok(entry.content.as_bytes().to_vec())
        } else {
            self.base.read_bytes_bounded(path, limit, cancellation)
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        if let Some(entry) = self.overlay(path) {
            return Ok(entry.canonical_path.clone());
        }
        if self.has_overlay_descendant(path) {
            return Ok(path.to_path_buf());
        }
        self.base.canonicalize(path)
    }

    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
        if self.overlay(path).is_some() {
            Ok(FileSystemEntryKind::File)
        } else if self.has_overlay_descendant(path) {
            Ok(FileSystemEntryKind::Directory)
        } else {
            self.base.entry_kind(path)
        }
    }

    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<OsString>, FileSystemReadError> {
        if cancellation.is_cancelled() {
            return Err(FileSystemReadError::Cancelled);
        }
        let canonical = self.canonicalize(path)?;
        let mut names = match self.base.read_directory_bounded(path, limit, cancellation) {
            Ok(names) => names.into_iter().collect::<BTreeSet<_>>(),
            Err(FileSystemReadError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                BTreeSet::new()
            }
            Err(error) => return Err(error),
        };
        names.extend(self.overlay_child_names(&canonical));
        if names.len() as u64 > limit.get() {
            return Err(FileSystemReadError::EntryLimitExceeded { limit });
        }
        Ok(names.into_iter().collect())
    }

    fn is_file(&self, path: &Path) -> bool {
        self.overlay(path).is_some() || self.base.is_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.overlay(path).is_some() || self.has_overlay_descendant(path) || self.base.exists(path)
    }
}

#[cfg(test)]
mod tests {
    use crate::{InMemoryFileSystem, NeverCancel, VirtualAbsolutePath};

    use super::*;

    const TEST_LIMIT: ByteLimit = ByteLimit::new(1024);

    fn virtual_path(path: &str) -> VirtualAbsolutePath {
        VirtualAbsolutePath::new(path).unwrap()
    }

    #[test]
    fn overlay_intercepts_read() {
        let mut base = InMemoryFileSystem::new();
        base.add_file(
            virtual_path("/project/main.gcl"),
            "original content".to_string(),
        )
        .unwrap();

        let fs = OverlayFileSystem::new(
            base,
            PathBuf::from("/project/main.gcl"),
            "overlay content".to_string(),
        );

        assert_eq!(
            fs.read_to_string_bounded(Path::new("/project/main.gcl"), TEST_LIMIT, &NeverCancel,)
                .unwrap(),
            "overlay content"
        );
    }

    #[test]
    fn overlay_delegates_other_files() {
        let mut base = InMemoryFileSystem::new();
        base.add_file(
            virtual_path("/project/main.gcl"),
            "main content".to_string(),
        )
        .unwrap();
        base.add_file(
            virtual_path("/project/helper.gcl"),
            "helper content".to_string(),
        )
        .unwrap();
        let fs = OverlayFileSystem::new(
            base,
            PathBuf::from("/project/main.gcl"),
            "overlay".to_string(),
        );

        assert_eq!(
            fs.read_to_string_bounded(Path::new("/project/helper.gcl"), TEST_LIMIT, &NeverCancel,)
                .unwrap(),
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
        base.add_file(virtual_path("/project/main.gcl"), "main disk".to_string())
            .unwrap();
        base.add_file(virtual_path("/project/lib.gcl"), "lib disk".to_string())
            .unwrap();
        base.add_file(virtual_path("/project/other.gcl"), "other disk".to_string())
            .unwrap();
        let fs = OverlayFileSystem::with_overlays(
            base,
            [
                (PathBuf::from("/project/main.gcl"), "main buf".to_string()),
                (PathBuf::from("/project/lib.gcl"), "lib buf".to_string()),
            ],
        );

        assert_eq!(
            fs.read_to_string_bounded(Path::new("/project/main.gcl"), TEST_LIMIT, &NeverCancel,)
                .unwrap(),
            "main buf"
        );
        assert_eq!(
            fs.read_to_string_bounded(Path::new("/project/lib.gcl"), TEST_LIMIT, &NeverCancel,)
                .unwrap(),
            "lib buf"
        );
        assert_eq!(
            fs.read_to_string_bounded(Path::new("/project/other.gcl"), TEST_LIMIT, &NeverCancel,)
                .unwrap(),
            "other disk"
        );
    }

    #[test]
    fn canonicalize_on_overlay_path_succeeds_when_file_not_on_disk() {
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
            fs.read_to_string_bounded(Path::new("/project/unsaved.gcl"), TEST_LIMIT, &NeverCancel,)
                .unwrap(),
            "unsaved"
        );
    }
}
