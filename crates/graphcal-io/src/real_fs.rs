//! Real filesystem implementation backed by `std::fs`.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::{
    ByteLimit, CancellationSignal, EntryLimit, FileSystemEntryKind, FileSystemReadError,
    FileSystemReader,
};

/// Filesystem reader that delegates to the operating system via `std::fs`.
///
/// # Sandboxing
///
/// When constructed with [`RealFileSystem::rooted`], every operation is
/// constrained to the canonical root. Canonical checks resolve parent
/// symlinks, so a lexical in-root path cannot use a linked directory to escape.
#[derive(Debug, Clone, Default)]
pub struct RealFileSystem {
    root: Option<PathBuf>,
}

impl RealFileSystem {
    /// Construct a sandboxed filesystem reader pinned to `project_root`.
    ///
    /// # Errors
    ///
    /// Returns the canonicalization error when `project_root` does not exist
    /// or cannot be resolved.
    pub fn rooted(project_root: &Path) -> Result<Self, io::Error> {
        let root = project_root.canonicalize()?;
        Ok(Self { root: Some(root) })
    }

    fn outside_root_error(canonical: &Path, root: &Path) -> io::Error {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "path {} is outside the filesystem root {}",
                canonical.display(),
                root.display()
            ),
        )
    }

    /// Check an existing target after resolving the final path component.
    fn check_access(&self, path: &Path) -> Result<PathBuf, io::Error> {
        let canonical = path.canonicalize()?;
        match &self.root {
            None => Ok(canonical),
            Some(root) if canonical.starts_with(root) => Ok(canonical),
            Some(root) => Err(Self::outside_root_error(&canonical, root)),
        }
    }

    /// Check access to an entry while preserving its final symlink identity.
    fn check_entry_access<'a>(&self, path: &'a Path) -> Result<&'a Path, io::Error> {
        let Some(root) = &self.root else {
            return Ok(path);
        };
        if path == root {
            return Ok(path);
        }
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "filesystem entry has no parent")
        })?;
        let canonical_parent = parent.canonicalize()?;
        if canonical_parent.starts_with(root) {
            Ok(path)
        } else {
            Err(Self::outside_root_error(&canonical_parent, root))
        }
    }

    fn read_file_bounded(
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<u8>, FileSystemReadError> {
        if cancellation.is_cancelled() {
            return Err(FileSystemReadError::Cancelled);
        }
        let mut file = File::open(path)?;
        let declared_len = file.metadata()?.len();
        if declared_len > limit.get() {
            return Err(FileSystemReadError::ByteLimitExceeded { limit });
        }
        let capacity = usize::try_from(declared_len).map_err(|_| {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                "file length cannot be represented on this platform",
            )
        })?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(capacity).map_err(|error| {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                format!("could not reserve bounded file buffer: {error}"),
            )
        })?;
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            if cancellation.is_cancelled() {
                return Err(FileSystemReadError::Cancelled);
            }
            let read = file.read(&mut chunk)?;
            if read == 0 {
                return Ok(bytes);
            }
            let next_len = (bytes.len() as u64).saturating_add(read as u64);
            if next_len > limit.get() {
                return Err(FileSystemReadError::ByteLimitExceeded { limit });
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
    }
}

impl FileSystemReader for RealFileSystem {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<u8>, FileSystemReadError> {
        let readable = if self.root.is_some() {
            self.check_access(path)?
        } else {
            path.to_path_buf()
        };
        Self::read_file_bounded(&readable, limit, cancellation)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        self.check_access(path)
    }

    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
        let path = self.check_entry_access(path)?;
        let file_type = std::fs::symlink_metadata(path)?.file_type();
        Ok(if file_type.is_file() {
            FileSystemEntryKind::File
        } else if file_type.is_dir() {
            FileSystemEntryKind::Directory
        } else if file_type.is_symlink() {
            FileSystemEntryKind::Symlink
        } else {
            FileSystemEntryKind::Other
        })
    }

    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<OsString>, FileSystemReadError> {
        let directory = if self.root.is_some() {
            self.check_access(path)?
        } else {
            path.to_path_buf()
        };
        let mut names = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            if cancellation.is_cancelled() {
                return Err(FileSystemReadError::Cancelled);
            }
            if names.len() as u64 >= limit.get() {
                return Err(FileSystemReadError::EntryLimitExceeded { limit });
            }
            names.push(entry?.file_name());
        }
        Ok(names)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.check_access(path).is_ok_and(|path| path.is_file())
    }

    fn exists(&self, path: &Path) -> bool {
        self.check_access(path).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NeverCancel;
    use std::fs;

    const TEST_LIMIT: ByteLimit = ByteLimit::new(1024);

    #[test]
    fn unrooted_reads_any_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.gcl");
        fs::write(&file, "param x: Dimensionless = 1.0;").unwrap();

        let fs_reader = RealFileSystem::default();
        let content = fs_reader
            .read_to_string_bounded(&file, TEST_LIMIT, &NeverCancel)
            .unwrap();
        assert!(content.contains("param x"));
    }

    #[test]
    fn rooted_allows_paths_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("a.gcl");
        fs::write(&file, "hello").unwrap();

        let fs_reader = RealFileSystem::rooted(&root).unwrap();
        assert_eq!(
            fs_reader
                .read_to_string_bounded(&file, TEST_LIMIT, &NeverCancel)
                .unwrap(),
            "hello"
        );
        assert!(fs_reader.is_file(&file));
        assert!(fs_reader.exists(&file));
    }

    #[test]
    fn bounded_read_rejects_sparse_file_before_allocating_declared_length() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("large.bin");
        let handle = File::create(&file).unwrap();
        handle.set_len(1025).unwrap();

        let error = RealFileSystem::default()
            .read_bytes_bounded(&file, TEST_LIMIT, &NeverCancel)
            .unwrap_err();
        assert!(matches!(
            error,
            FileSystemReadError::ByteLimitExceeded { .. }
        ));
    }

    #[test]
    fn rooted_rejects_paths_outside_root() {
        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&external).unwrap();

        let secret = external.join("secret.gcl");
        fs::write(&secret, "secret content").unwrap();

        let fs_reader = RealFileSystem::rooted(&project).unwrap();
        let error = fs_reader
            .read_to_string_bounded(&secret, TEST_LIMIT, &NeverCancel)
            .unwrap_err();
        assert_eq!(error.io_kind(), Some(io::ErrorKind::NotFound));
        assert!(!fs_reader.is_file(&secret));
        assert!(!fs_reader.exists(&secret));
    }

    #[cfg(unix)]
    #[test]
    fn rooted_rejects_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&external).unwrap();

        let target = external.join("escaped.gcl");
        fs::write(&target, "escaped").unwrap();

        let link = project.join("link.gcl");
        symlink(&target, &link).unwrap();

        let fs_reader = RealFileSystem::rooted(&project).unwrap();
        let error = fs_reader
            .read_to_string_bounded(&link, TEST_LIMIT, &NeverCancel)
            .unwrap_err();
        assert_eq!(error.io_kind(), Some(io::ErrorKind::NotFound));
        assert_eq!(
            fs_reader.entry_kind(&link).unwrap(),
            FileSystemEntryKind::Symlink
        );
        assert!(!fs_reader.exists(&link));
    }

    #[test]
    fn unrooted_path_outside_any_root_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("loose.gcl");
        fs::write(&file, "loose").unwrap();

        let fs_reader = RealFileSystem::default();
        assert!(fs_reader.exists(&file));
        assert!(fs_reader.is_file(&file));
        assert_eq!(
            fs_reader
                .read_to_string_bounded(&file, TEST_LIMIT, &NeverCancel)
                .unwrap(),
            "loose"
        );
    }
}
