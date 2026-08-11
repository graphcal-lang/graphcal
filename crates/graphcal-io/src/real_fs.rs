//! Real filesystem implementation backed by ambient `std::fs` access or a
//! held capability directory.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use cap_std::{ambient_authority, fs::Dir};

#[cfg(not(target_arch = "wasm32"))]
use crate::VirtualAbsolutePath;
use crate::{
    ByteLimit, CancellationSignal, EntryLimit, FileSystemEntryKind, FileSystemReadError,
    FileSystemReader,
};

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct RootCapability {
    requested_path: VirtualAbsolutePath,
    canonical_path: VirtualAbsolutePath,
    directory: Arc<Dir>,
}

/// Filesystem reader backed by the operating system.
///
/// An unrooted reader delegates to ambient `std::fs`. A rooted reader holds an
/// open directory capability and performs canonicalization, opens, metadata
/// queries, and directory listing relative to that handle. `cap-std` uses
/// `openat2` where available and component-by-component handle traversal on
/// other supported platforms, preventing concurrent symlink swaps from escaping
/// the held root.
#[derive(Debug, Clone, Default)]
pub struct RealFileSystem {
    #[cfg(not(target_arch = "wasm32"))]
    root: Option<RootCapability>,
}

impl RealFileSystem {
    /// Construct a sandboxed filesystem reader pinned to `project_root`.
    ///
    /// Ambient authority is used only here to open the root directory. Every
    /// later rooted operation is relative to the resulting held capability.
    ///
    /// # Errors
    ///
    /// Returns an error when `project_root` does not exist, is not a directory,
    /// or cannot be opened as a capability.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn rooted(project_root: &Path) -> Result<Self, io::Error> {
        let canonical = project_root.canonicalize()?;
        let canonical_path = VirtualAbsolutePath::new(canonical.clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let requested_path = VirtualAbsolutePath::new(project_root.to_path_buf())
            .unwrap_or_else(|_| canonical_path.clone());
        let directory = Dir::open_ambient_dir(&canonical, ambient_authority())?;
        Ok(Self {
            root: Some(RootCapability {
                requested_path,
                canonical_path,
                directory: Arc::new(directory),
            }),
        })
    }

    /// Rooted ambient filesystems are unavailable in bare Wasm environments.
    ///
    /// # Errors
    ///
    /// Always returns [`io::ErrorKind::Unsupported`].
    #[cfg(target_arch = "wasm32")]
    pub fn rooted(_project_root: &Path) -> Result<Self, io::Error> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "rooted ambient filesystems are unavailable on wasm32",
        ))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn outside_root_error(path: &Path, root: &Path) -> io::Error {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "path {} is outside the filesystem root {}",
                path.display(),
                root.display()
            ),
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn rooted_relative_path(root: &RootCapability, path: &Path) -> Result<PathBuf, io::Error> {
        if let Ok(normalized) = VirtualAbsolutePath::new(path.to_path_buf()) {
            for root_path in [&root.canonical_path, &root.requested_path] {
                if let Ok(relative) = normalized.as_path().strip_prefix(root_path.as_path()) {
                    return Ok(cap_relative_path(relative));
                }
            }
        }

        // This fallback maps an existing non-canonical ambient spelling (for
        // example macOS `/var` versus `/private/var`) into the held root. The
        // resulting operation still uses only the relative capability path, so
        // a race here can change which in-root entry is selected but cannot
        // grant access outside the root.
        let canonical = path.canonicalize()?;
        canonical
            .strip_prefix(root.canonical_path.as_path())
            .map(cap_relative_path)
            .map_err(|_| Self::outside_root_error(&canonical, root.canonical_path.as_path()))
    }

    fn read_file_bounded(
        mut file: impl Read,
        declared_len: u64,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<u8>, FileSystemReadError> {
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

    fn read_bytes_bounded_with_hook(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
        before_open: impl FnOnce(),
    ) -> Result<Vec<u8>, FileSystemReadError> {
        if cancellation.is_cancelled() {
            return Err(FileSystemReadError::Cancelled);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            let relative = Self::rooted_relative_path(root, path)?;
            before_open();
            let file = root
                .directory
                .open(relative)
                .map_err(normalize_rooted_error)?;
            let declared_len = file.metadata()?.len();
            return Self::read_file_bounded(file, declared_len, limit, cancellation);
        }

        before_open();
        let file = File::open(path)?;
        let declared_len = file.metadata()?.len();
        Self::read_file_bounded(file, declared_len, limit, cancellation)
    }

    fn read_directory_bounded_with_hook(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
        before_open: impl FnOnce(),
    ) -> Result<Vec<OsString>, FileSystemReadError> {
        if cancellation.is_cancelled() {
            return Err(FileSystemReadError::Cancelled);
        }
        let mut names = Vec::new();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            let relative = Self::rooted_relative_path(root, path)?;
            before_open();
            for entry in root
                .directory
                .read_dir(relative)
                .map_err(normalize_rooted_error)?
            {
                if cancellation.is_cancelled() {
                    return Err(FileSystemReadError::Cancelled);
                }
                if names.len() as u64 >= limit.get() {
                    return Err(FileSystemReadError::EntryLimitExceeded { limit });
                }
                names.push(entry.map_err(normalize_rooted_error)?.file_name());
            }
            return Ok(names);
        }

        before_open();
        for entry in std::fs::read_dir(path)? {
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
}

const fn classify_entry_kind(
    is_file: bool,
    is_directory: bool,
    is_symlink: bool,
) -> FileSystemEntryKind {
    if is_file {
        FileSystemEntryKind::File
    } else if is_directory {
        FileSystemEntryKind::Directory
    } else if is_symlink {
        FileSystemEntryKind::Symlink
    } else {
        FileSystemEntryKind::Other
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn normalize_rooted_error(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::PermissionDenied {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("rooted filesystem rejected path traversal: {error}"),
        )
    } else {
        error
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn cap_relative_path(path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path.to_path_buf()
    }
}

impl FileSystemReader for RealFileSystem {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<u8>, FileSystemReadError> {
        self.read_bytes_bounded_with_hook(path, limit, cancellation, || {})
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            let relative = Self::rooted_relative_path(root, path)?;
            return root
                .directory
                .canonicalize(relative)
                .map_err(normalize_rooted_error)
                .map(|relative| root.canonical_path.as_path().join(relative));
        }
        path.canonicalize()
    }

    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            let relative = Self::rooted_relative_path(root, path)?;
            let file_type = root
                .directory
                .symlink_metadata(relative)
                .map_err(normalize_rooted_error)?
                .file_type();
            return Ok(classify_entry_kind(
                file_type.is_file(),
                file_type.is_dir(),
                file_type.is_symlink(),
            ));
        }

        let file_type = std::fs::symlink_metadata(path)?.file_type();
        Ok(classify_entry_kind(
            file_type.is_file(),
            file_type.is_dir(),
            file_type.is_symlink(),
        ))
    }

    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<OsString>, FileSystemReadError> {
        self.read_directory_bounded_with_hook(path, limit, cancellation, || {})
    }

    fn is_file(&self, path: &Path) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            return Self::rooted_relative_path(root, path)
                .is_ok_and(|relative| root.directory.is_file(relative));
        }
        path.is_file()
    }

    fn exists(&self, path: &Path) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.root {
            return Self::rooted_relative_path(root, path)
                .is_ok_and(|relative| root.directory.try_exists(relative).unwrap_or(false));
        }
        path.exists()
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
        assert_eq!(fs_reader.canonicalize(&file).unwrap(), file);
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

    #[cfg(unix)]
    #[test]
    fn rooted_read_rejects_final_file_swap_between_mapping_and_open() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("secret.gcl");
        fs::create_dir_all(&project).unwrap();
        fs::write(&external, "outside secret").unwrap();
        let victim = project.join("victim.gcl");
        fs::write(&victim, "inside").unwrap();
        let reader = RealFileSystem::rooted(&project).unwrap();

        let error = reader
            .read_bytes_bounded_with_hook(&victim, TEST_LIMIT, &NeverCancel, || {
                fs::remove_file(&victim).unwrap();
                symlink(&external, &victim).unwrap();
            })
            .unwrap_err();

        assert!(matches!(error, FileSystemReadError::Io(_)));
    }

    #[cfg(unix)]
    #[test]
    fn rooted_read_rejects_parent_swap_between_mapping_and_open() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        let directory = project.join("directory");
        fs::create_dir_all(&directory).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(directory.join("model.gcl"), "inside").unwrap();
        fs::write(external.join("model.gcl"), "outside secret").unwrap();
        let reader = RealFileSystem::rooted(&project).unwrap();
        let requested = directory.join("model.gcl");
        let parked = project.join("parked");

        let error = reader
            .read_bytes_bounded_with_hook(&requested, TEST_LIMIT, &NeverCancel, || {
                fs::rename(&directory, &parked).unwrap();
                symlink(&external, &directory).unwrap();
            })
            .unwrap_err();

        assert!(matches!(error, FileSystemReadError::Io(_)));
    }

    #[cfg(unix)]
    #[test]
    fn rooted_directory_listing_rejects_parent_swap_before_open() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        let directory = project.join("listing");
        fs::create_dir_all(&directory).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(directory.join("inside.gcl"), "inside").unwrap();
        fs::write(external.join("secret.gcl"), "outside secret").unwrap();
        let reader = RealFileSystem::rooted(&project).unwrap();
        let parked = project.join("parked");

        let error = reader
            .read_directory_bounded_with_hook(&directory, EntryLimit::new(10), &NeverCancel, || {
                fs::rename(&directory, &parked).unwrap();
                symlink(&external, &directory).unwrap();
            })
            .unwrap_err();

        assert!(matches!(error, FileSystemReadError::Io(_)));
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
