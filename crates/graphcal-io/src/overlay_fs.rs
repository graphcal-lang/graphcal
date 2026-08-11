//! Overlay filesystem that layers validated in-memory files over a base reader.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::{
    ByteLimit, CancellationSignal, EntryLimit, FileSystemEntryKind, FileSystemReadError,
    FileSystemReader, VirtualAbsolutePath, VirtualPathError,
};

#[derive(Debug)]
enum OverlayIdentity {
    Existing { canonical: VirtualAbsolutePath },
    Unsaved { canonical: VirtualAbsolutePath },
}

impl OverlayIdentity {
    const fn canonical(&self) -> &VirtualAbsolutePath {
        match self {
            Self::Existing { canonical } | Self::Unsaved { canonical } => canonical,
        }
    }
}

#[derive(Debug)]
struct OverlayEntry {
    identity: OverlayIdentity,
    aliases: BTreeSet<VirtualAbsolutePath>,
    content: String,
}

impl OverlayEntry {
    const fn canonical(&self) -> &VirtualAbsolutePath {
        self.identity.canonical()
    }
}

#[derive(Debug)]
enum OverlayPath<'a> {
    File(&'a OverlayEntry),
    Directory(VirtualAbsolutePath),
}

/// Filesystem reader that serves validated in-memory editor buffers before
/// delegating to a base capability.
///
/// Construction proves every overlay is either an existing base file or an
/// unsaved file below a base-authorized existing directory. Therefore adding an
/// overlay cannot widen a rooted base filesystem's capability.
#[derive(Debug)]
pub struct OverlayFileSystem<F> {
    base: F,
    overlays: Vec<OverlayEntry>,
}

impl<F: FileSystemReader> OverlayFileSystem<F> {
    /// Create an overlay filesystem serving one in-memory UTF-8 file.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayFileSystemError`] when the path is relative, outside
    /// the base capability, or collides with a file/directory identity.
    pub fn new(
        base: F,
        overlay_path: PathBuf,
        overlay_content: String,
    ) -> Result<Self, OverlayFileSystemError> {
        Self::with_overlays(base, [(overlay_path, overlay_content)])
    }

    /// Create an overlay filesystem serving several in-memory files.
    ///
    /// Spellings that resolve to one canonical file are aliases; the first
    /// content for that identity wins. File/directory ancestor collisions are
    /// rejected.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayFileSystemError`] when any path is relative, outside
    /// the base capability, or collides with a file/directory identity.
    pub fn with_overlays(
        base: F,
        overlays: impl IntoIterator<Item = (PathBuf, String)>,
    ) -> Result<Self, OverlayFileSystemError> {
        let mut entries: Vec<OverlayEntry> = Vec::new();
        for (path, content) in overlays {
            let requested = VirtualAbsolutePath::new(path.clone()).map_err(|source| {
                OverlayFileSystemError::InvalidPath {
                    path: path.clone(),
                    source,
                }
            })?;
            let identity = resolve_overlay_identity(&base, &requested)?;
            let canonical = identity.canonical().clone();

            if let Some(existing) = entries
                .iter_mut()
                .find(|entry| entry.canonical() == &canonical)
            {
                existing.aliases.insert(requested);
                continue;
            }
            if let Some(conflicting) = entries.iter().find(|entry| {
                canonical.as_path().starts_with(entry.canonical().as_path())
                    || entry.canonical().as_path().starts_with(canonical.as_path())
            }) {
                return Err(OverlayFileSystemError::FileDirectoryCollision {
                    path: canonical,
                    conflicting: conflicting.canonical().clone(),
                });
            }

            let aliases = BTreeSet::from([requested, canonical]);
            entries.push(OverlayEntry {
                identity,
                aliases,
                content,
            });
        }
        Ok(Self {
            base,
            overlays: entries,
        })
    }

    fn overlay_path(&self, path: &Path) -> Option<OverlayPath<'_>> {
        let path = VirtualAbsolutePath::new(path.to_path_buf()).ok()?;
        if let Some(entry) = self
            .overlays
            .iter()
            .find(|entry| entry.aliases.contains(&path))
        {
            return Some(OverlayPath::File(entry));
        }
        self.overlay_directory(&path).map(OverlayPath::Directory)
    }

    fn overlay_directory(&self, path: &VirtualAbsolutePath) -> Option<VirtualAbsolutePath> {
        self.overlays.iter().find_map(|entry| {
            entry.aliases.iter().find_map(|alias| {
                let relative = alias.as_path().strip_prefix(path.as_path()).ok()?;
                let depth = relative.components().count();
                if depth == 0 {
                    return None;
                }
                let mut canonical = entry.canonical().as_path();
                for _ in 0..depth {
                    canonical = canonical.parent()?;
                }
                VirtualAbsolutePath::new(canonical.to_path_buf()).ok()
            })
        })
    }

    fn overlay_child_names(&self, directory: &VirtualAbsolutePath) -> BTreeSet<OsString> {
        self.overlays
            .iter()
            .filter_map(|entry| {
                let relative = entry
                    .canonical()
                    .as_path()
                    .strip_prefix(directory.as_path())
                    .ok()?;
                match relative.components().next() {
                    Some(Component::Normal(name)) => Some(name.to_os_string()),
                    _ => None,
                }
            })
            .collect()
    }
}

fn resolve_overlay_identity(
    base: &impl FileSystemReader,
    requested: &VirtualAbsolutePath,
) -> Result<OverlayIdentity, OverlayFileSystemError> {
    match base.canonicalize(requested.as_path()) {
        Ok(canonical) => {
            let canonical = canonical_path(requested, canonical)?;
            let kind = base.entry_kind(canonical.as_path()).map_err(|source| {
                OverlayFileSystemError::InspectBase {
                    path: requested.clone(),
                    source,
                }
            })?;
            if kind == FileSystemEntryKind::File {
                Ok(OverlayIdentity::Existing { canonical })
            } else {
                Err(OverlayFileSystemError::TargetIsNotFile {
                    path: requested.clone(),
                    kind,
                })
            }
        }
        Err(canonicalize_error) => {
            match base.entry_kind(requested.as_path()) {
                Ok(_) => {
                    return Err(OverlayFileSystemError::InaccessibleTarget {
                        path: requested.clone(),
                        source: canonicalize_error,
                    });
                }
                Err(source) if source.kind() != io::ErrorKind::NotFound => {
                    return Err(OverlayFileSystemError::InspectBase {
                        path: requested.clone(),
                        source,
                    });
                }
                Err(_) => {}
            }
            resolve_unsaved_identity(base, requested)
        }
    }
}

fn resolve_unsaved_identity(
    base: &impl FileSystemReader,
    requested: &VirtualAbsolutePath,
) -> Result<OverlayIdentity, OverlayFileSystemError> {
    let mut candidate = requested.as_path().parent();
    while let Some(parent) = candidate {
        match base.entry_kind(parent) {
            Ok(_) => {
                let canonical_parent = base.canonicalize(parent).map_err(|source| {
                    OverlayFileSystemError::InaccessibleParent {
                        path: requested.clone(),
                        parent: parent.to_path_buf(),
                        source,
                    }
                })?;
                let canonical_parent = canonical_path(requested, canonical_parent)?;
                let kind = base
                    .entry_kind(canonical_parent.as_path())
                    .map_err(|source| OverlayFileSystemError::InspectBase {
                        path: canonical_parent.clone(),
                        source,
                    })?;
                if kind != FileSystemEntryKind::Directory {
                    return Err(OverlayFileSystemError::ParentIsNotDirectory {
                        path: requested.clone(),
                        parent: canonical_parent,
                        kind,
                    });
                }
                let relative = requested.as_path().strip_prefix(parent).map_err(|_| {
                    OverlayFileSystemError::NoAccessibleParent {
                        path: requested.clone(),
                    }
                })?;
                let canonical = VirtualAbsolutePath::new(canonical_parent.as_path().join(relative))
                    .map_err(|source| OverlayFileSystemError::InvalidCanonicalPath {
                        path: requested.clone(),
                        source,
                    })?;
                return Ok(OverlayIdentity::Unsaved { canonical });
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                candidate = parent.parent();
            }
            Err(source) => {
                return Err(OverlayFileSystemError::InspectBase {
                    path: requested.clone(),
                    source,
                });
            }
        }
    }
    Err(OverlayFileSystemError::NoAccessibleParent {
        path: requested.clone(),
    })
}

fn canonical_path(
    requested: &VirtualAbsolutePath,
    canonical: PathBuf,
) -> Result<VirtualAbsolutePath, OverlayFileSystemError> {
    VirtualAbsolutePath::new(canonical).map_err(|source| {
        OverlayFileSystemError::InvalidCanonicalPath {
            path: requested.clone(),
            source,
        }
    })
}

impl<F: FileSystemReader> FileSystemReader for OverlayFileSystem<F> {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<u8>, FileSystemReadError> {
        if let Some(OverlayPath::File(entry)) = self.overlay_path(path) {
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
        match self.overlay_path(path) {
            Some(OverlayPath::File(entry)) => Ok(entry.canonical().clone().into_path_buf()),
            Some(OverlayPath::Directory(canonical)) => Ok(canonical.into_path_buf()),
            None => self.base.canonicalize(path),
        }
    }

    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
        match self.overlay_path(path) {
            Some(OverlayPath::File(_)) => Ok(FileSystemEntryKind::File),
            Some(OverlayPath::Directory(_)) => Ok(FileSystemEntryKind::Directory),
            None => self.base.entry_kind(path),
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
        let canonical = match self.overlay_path(path) {
            Some(OverlayPath::File(entry)) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    entry.canonical().as_path().display().to_string(),
                )
                .into());
            }
            Some(OverlayPath::Directory(canonical)) => canonical,
            None => {
                return self.base.read_directory_bounded(path, limit, cancellation);
            }
        };
        let mut names =
            match self
                .base
                .read_directory_bounded(canonical.as_path(), limit, cancellation)
            {
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
        matches!(self.overlay_path(path), Some(OverlayPath::File(_))) || self.base.is_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.overlay_path(path).is_some() || self.base.exists(path)
    }
}

/// Invalid or capability-widening overlay construction.
#[derive(Debug, Error)]
pub enum OverlayFileSystemError {
    /// The requested overlay spelling was not a safe absolute virtual path.
    #[error("invalid overlay path `{}`: {source}", path.display())]
    InvalidPath {
        /// Rejected spelling.
        path: PathBuf,
        /// Path validation failure.
        #[source]
        source: VirtualPathError,
    },
    /// The base rejected metadata inspection needed to prove an identity.
    #[error("could not inspect overlay path `{}` through the base filesystem: {source}", path.as_path().display())]
    InspectBase {
        /// Overlay or canonical path being inspected.
        path: VirtualAbsolutePath,
        /// Base filesystem error.
        #[source]
        source: io::Error,
    },
    /// An existing final entry was visible but not canonicalizable through the base.
    #[error("overlay target `{}` is not accessible through the base filesystem: {source}", path.as_path().display())]
    InaccessibleTarget {
        /// Rejected overlay path.
        path: VirtualAbsolutePath,
        /// Base canonicalization error.
        #[source]
        source: io::Error,
    },
    /// An existing parent was not canonicalizable through the base capability.
    #[error("overlay path `{}` has inaccessible parent `{}`: {source}", path.as_path().display(), parent.display())]
    InaccessibleParent {
        /// Rejected overlay path.
        path: VirtualAbsolutePath,
        /// Existing parent spelling.
        parent: PathBuf,
        /// Base canonicalization error.
        #[source]
        source: io::Error,
    },
    /// No existing directory could prove that the unsaved path is in capability.
    #[error("overlay path `{}` has no parent directory accessible through the base filesystem", path.as_path().display())]
    NoAccessibleParent {
        /// Rejected overlay path.
        path: VirtualAbsolutePath,
    },
    /// Existing overlay targets must be regular files.
    #[error("overlay target `{}` is {kind:?}, not a regular file", path.as_path().display())]
    TargetIsNotFile {
        /// Rejected overlay path.
        path: VirtualAbsolutePath,
        /// Existing entry kind.
        kind: FileSystemEntryKind,
    },
    /// The nearest existing parent was not a directory.
    #[error("overlay path `{}` is below non-directory `{}` ({kind:?})", path.as_path().display(), parent.as_path().display())]
    ParentIsNotDirectory {
        /// Rejected overlay path.
        path: VirtualAbsolutePath,
        /// Blocking parent identity.
        parent: VirtualAbsolutePath,
        /// Existing entry kind.
        kind: FileSystemEntryKind,
    },
    /// The base violated its canonical absolute path contract.
    #[error("base returned an invalid canonical path for overlay `{}`: {source}", path.as_path().display())]
    InvalidCanonicalPath {
        /// Overlay path being resolved.
        path: VirtualAbsolutePath,
        /// Canonical path validation failure.
        #[source]
        source: VirtualPathError,
    },
    /// One overlay file would also have to be another overlay's directory.
    #[error("overlay file `{}` conflicts with overlay file `{}`", path.as_path().display(), conflicting.as_path().display())]
    FileDirectoryCollision {
        /// New overlay identity.
        path: VirtualAbsolutePath,
        /// Existing conflicting identity.
        conflicting: VirtualAbsolutePath,
    },
}

impl OverlayFileSystemError {
    /// Overlay path responsible for this failure.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::InvalidPath { path, .. } => path,
            Self::InspectBase { path, .. }
            | Self::InaccessibleTarget { path, .. }
            | Self::InaccessibleParent { path, .. }
            | Self::NoAccessibleParent { path }
            | Self::TargetIsNotFile { path, .. }
            | Self::ParentIsNotDirectory { path, .. }
            | Self::InvalidCanonicalPath { path, .. }
            | Self::FileDirectoryCollision { path, .. } => path.as_path(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{InMemoryFileSystem, NeverCancel, RealFileSystem};

    use super::*;

    const TEST_LIMIT: ByteLimit = ByteLimit::new(1024);

    fn virtual_path(path: &str) -> VirtualAbsolutePath {
        VirtualAbsolutePath::new(path).unwrap()
    }

    fn in_memory_base() -> InMemoryFileSystem {
        let mut base = InMemoryFileSystem::new();
        base.add_file(virtual_path("/project/base.gcl"), "base".to_string())
            .unwrap();
        base
    }

    #[test]
    fn overlay_intercepts_read() {
        let mut base = in_memory_base();
        base.add_file(
            virtual_path("/project/main.gcl"),
            "original content".to_string(),
        )
        .unwrap();

        let fs = OverlayFileSystem::new(
            base,
            PathBuf::from("/project/main.gcl"),
            "overlay content".to_string(),
        )
        .unwrap();

        assert_eq!(
            fs.read_to_string_bounded(Path::new("/project/main.gcl"), TEST_LIMIT, &NeverCancel)
                .unwrap(),
            "overlay content"
        );
    }

    #[test]
    fn overlay_delegates_other_files() {
        let mut base = in_memory_base();
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
        )
        .unwrap();

        assert_eq!(
            fs.read_to_string_bounded(Path::new("/project/helper.gcl"), TEST_LIMIT, &NeverCancel)
                .unwrap(),
            "helper content"
        );
    }

    #[test]
    fn overlay_is_file_returns_true_for_unsaved_in_capability_path() {
        let fs = OverlayFileSystem::new(
            in_memory_base(),
            PathBuf::from("/project/main.gcl"),
            "content".to_string(),
        )
        .unwrap();
        assert!(fs.is_file(Path::new("/project/main.gcl")));
    }

    #[test]
    fn duplicate_normalized_spellings_keep_the_first_content() {
        let fs = OverlayFileSystem::with_overlays(
            in_memory_base(),
            [
                (
                    PathBuf::from("/project/sub/../main.gcl"),
                    "first".to_string(),
                ),
                (PathBuf::from("/project/main.gcl"), "second".to_string()),
            ],
        )
        .unwrap();

        assert_eq!(
            fs.read_to_string_bounded(Path::new("/project/main.gcl"), TEST_LIMIT, &NeverCancel)
                .unwrap(),
            "first"
        );
    }

    #[test]
    fn canonicalize_on_unsaved_overlay_uses_the_authorized_parent() {
        let fs = OverlayFileSystem::new(
            in_memory_base(),
            PathBuf::from("/project/sub/../unsaved.gcl"),
            "unsaved".to_string(),
        )
        .unwrap();

        let canonical = fs.canonicalize(Path::new("/project/unsaved.gcl")).unwrap();
        assert_eq!(canonical, PathBuf::from("/project/unsaved.gcl"));
        assert!(fs.exists(Path::new("/project/unsaved.gcl")));
    }

    #[test]
    fn overlay_file_and_directory_collisions_are_rejected() {
        let mut base = in_memory_base();
        base.add_file(virtual_path("/project/file.gcl"), "base".to_string())
            .unwrap();
        let beneath_base_file = OverlayFileSystem::new(
            base,
            PathBuf::from("/project/file.gcl/child.gcl"),
            "child".to_string(),
        )
        .unwrap_err();
        assert!(matches!(
            beneath_base_file,
            OverlayFileSystemError::ParentIsNotDirectory { .. }
        ));

        let overlay_collision = OverlayFileSystem::with_overlays(
            in_memory_base(),
            [
                (PathBuf::from("/project/new.gcl"), "file".to_string()),
                (
                    PathBuf::from("/project/new.gcl/child.gcl"),
                    "child".to_string(),
                ),
            ],
        )
        .unwrap_err();
        assert!(matches!(
            overlay_collision,
            OverlayFileSystemError::FileDirectoryCollision { .. }
        ));
    }

    #[test]
    fn relative_overlay_paths_are_rejected() {
        let error = OverlayFileSystem::new(
            in_memory_base(),
            PathBuf::from("relative.gcl"),
            "content".to_string(),
        )
        .unwrap_err();
        assert!(matches!(error, OverlayFileSystemError::InvalidPath { .. }));
    }

    #[test]
    fn rooted_base_rejects_existing_and_unsaved_outside_overlays() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join("secret.gcl");
        std::fs::write(&outside_file, "secret").unwrap();
        let base = RealFileSystem::rooted(&project).unwrap();

        assert!(OverlayFileSystem::new(base.clone(), outside_file, "overlay".to_string()).is_err());
        assert!(
            OverlayFileSystem::new(
                base.clone(),
                outside.join("unsaved.gcl"),
                "overlay".to_string(),
            )
            .is_err()
        );
        assert!(
            OverlayFileSystem::new(
                base,
                project.join("../outside/parent.gcl"),
                "overlay".to_string(),
            )
            .is_err()
        );
    }

    #[test]
    fn rooted_base_accepts_unsaved_in_root_overlay_without_widening() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let canonical_project = project.canonicalize().unwrap();
        let base = RealFileSystem::rooted(&canonical_project).unwrap();
        let unsaved = canonical_project.join("nested/unsaved.gcl");

        let fs = OverlayFileSystem::new(base, unsaved.clone(), "overlay".to_string()).unwrap();

        assert_eq!(fs.canonicalize(&unsaved).unwrap(), unsaved);
        assert_eq!(
            fs.read_to_string_bounded(&unsaved, TEST_LIMIT, &NeverCancel)
                .unwrap(),
            "overlay"
        );
        assert_eq!(
            fs.read_directory_bounded(
                &canonical_project.join("nested"),
                EntryLimit::new(1),
                &NeverCancel,
            )
            .unwrap(),
            vec![OsString::from("unsaved.gcl")]
        );
    }
}
