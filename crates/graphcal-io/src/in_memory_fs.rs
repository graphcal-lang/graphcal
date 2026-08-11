//! In-memory filesystem for tests and WASM environments.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::{
    ByteLimit, CancellationSignal, EntryLimit, FileSystemEntryKind, FileSystemReadError,
    FileSystemReader,
};

/// Normalized absolute path in an in-memory virtual filesystem.
///
/// Construction removes `.` components and resolves `..` components while
/// rejecting any parent traversal that would escape the virtual root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualAbsolutePath(PathBuf);

impl VirtualAbsolutePath {
    /// Validate and normalize an absolute virtual path.
    ///
    /// # Errors
    ///
    /// Returns [`VirtualPathError::NotAbsolute`] for a relative path and
    /// [`VirtualPathError::EscapesRoot`] when a `..` component crosses the
    /// virtual root.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, VirtualPathError> {
        let path = path.into();
        if !path.has_root() {
            return Err(VirtualPathError::NotAbsolute { path });
        }

        let mut normalized = PathBuf::new();
        let mut normal_components = 0usize;
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    normalized.push(component.as_os_str());
                }
                Component::CurDir => {}
                Component::Normal(name) => {
                    normalized.push(name);
                    normal_components += 1;
                }
                Component::ParentDir if normal_components > 0 => {
                    normalized.pop();
                    normal_components -= 1;
                }
                Component::ParentDir => return Err(VirtualPathError::EscapesRoot { path }),
            }
        }
        Ok(Self(normalized))
    }

    /// Borrow the normalized host representation used at filesystem boundaries.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Consume the validated path and return its normalized representation.
    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl AsRef<Path> for VirtualAbsolutePath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

/// Invalid virtual absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VirtualPathError {
    /// Virtual paths must start at a root.
    #[error("virtual filesystem path `{}` must be absolute", path.display())]
    NotAbsolute {
        /// Rejected spelling.
        path: PathBuf,
    },
    /// Parent traversal attempted to cross the virtual root.
    #[error("virtual filesystem path `{}` escapes the virtual root", path.display())]
    EscapesRoot {
        /// Rejected spelling.
        path: PathBuf,
    },
}

#[derive(Debug, Clone)]
enum FileContent {
    Utf8(String),
    Bytes(Vec<u8>),
}

impl FileContent {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Utf8(content) => content.as_bytes(),
            Self::Bytes(content) => content,
        }
    }
}

/// Failure to insert a file into an impossible virtual filesystem topology.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InMemoryFileSystemError {
    /// A file already occupies an ancestor of the requested path.
    #[error(
        "cannot add virtual file `{}` below existing file `{}`",
        path.as_path().display(),
        ancestor.as_path().display()
    )]
    FileAncestor {
        /// Requested file path.
        path: VirtualAbsolutePath,
        /// Existing file that blocks the path.
        ancestor: VirtualAbsolutePath,
    },
    /// Existing files already make the requested path a directory.
    #[error(
        "cannot add virtual file `{}` because that path is already a directory",
        path.as_path().display()
    )]
    DirectoryAlreadyExists {
        /// Requested file path.
        path: VirtualAbsolutePath,
    },
}

/// In-memory filesystem for tests and WASM environments.
///
/// Every key is a normalized [`VirtualAbsolutePath`], and each path has exactly
/// one internal content variant. Inserting a new content kind at an existing path
/// replaces the complete previous entry.
#[derive(Debug, Clone, Default)]
pub struct InMemoryFileSystem {
    files: HashMap<VirtualAbsolutePath, FileContent>,
}

impl InMemoryFileSystem {
    /// Create an empty in-memory filesystem.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a UTF-8 file at a validated path.
    ///
    /// # Errors
    ///
    /// Returns [`InMemoryFileSystemError`] if the path is already an implied
    /// directory or has an existing file as an ancestor.
    pub fn add_file(
        &mut self,
        path: VirtualAbsolutePath,
        content: String,
    ) -> Result<(), InMemoryFileSystemError> {
        self.insert(path, FileContent::Utf8(content))
    }

    /// Insert or replace a binary file at a validated path.
    ///
    /// # Errors
    ///
    /// Returns [`InMemoryFileSystemError`] if the path is already an implied
    /// directory or has an existing file as an ancestor.
    pub fn add_binary_file(
        &mut self,
        path: VirtualAbsolutePath,
        content: Vec<u8>,
    ) -> Result<(), InMemoryFileSystemError> {
        self.insert(path, FileContent::Bytes(content))
    }

    fn insert(
        &mut self,
        path: VirtualAbsolutePath,
        content: FileContent,
    ) -> Result<(), InMemoryFileSystemError> {
        if self.is_dir(&path) {
            return Err(InMemoryFileSystemError::DirectoryAlreadyExists { path });
        }
        if let Some(ancestor) = self
            .files
            .keys()
            .find(|existing| *existing != &path && path.as_path().starts_with(existing.as_path()))
            .cloned()
        {
            return Err(InMemoryFileSystemError::FileAncestor { path, ancestor });
        }
        self.files.insert(path, content);
        Ok(())
    }

    fn contains(&self, path: &VirtualAbsolutePath) -> bool {
        self.files.contains_key(path)
    }

    fn is_dir(&self, path: &VirtualAbsolutePath) -> bool {
        self.files
            .keys()
            .any(|key| key != path && key.as_path().starts_with(path.as_path()))
    }

    fn existing_path(&self, path: &Path) -> Result<VirtualAbsolutePath, io::Error> {
        let path = virtual_path(path)?;
        if self.contains(&path) || self.is_dir(&path) {
            Ok(path)
        } else {
            Err(not_found(&path))
        }
    }

    fn child_names_bounded(
        &self,
        path: &VirtualAbsolutePath,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<OsString>, FileSystemReadError> {
        let mut names = BTreeSet::new();
        for key in self.files.keys() {
            if cancellation.is_cancelled() {
                return Err(FileSystemReadError::Cancelled);
            }
            let Ok(relative) = key.as_path().strip_prefix(path.as_path()) else {
                continue;
            };
            let Some(Component::Normal(name)) = relative.components().next() else {
                continue;
            };
            names.insert(name.to_os_string());
            if names.len() as u64 > limit.get() {
                return Err(FileSystemReadError::EntryLimitExceeded { limit });
            }
        }
        Ok(names.into_iter().collect())
    }
}

impl FileSystemReader for InMemoryFileSystem {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<u8>, FileSystemReadError> {
        if cancellation.is_cancelled() {
            return Err(FileSystemReadError::Cancelled);
        }
        let path = virtual_path(path)?;
        let bytes = self
            .files
            .get(&path)
            .map(FileContent::as_bytes)
            .ok_or_else(|| not_found(&path))?;
        if bytes.len() as u64 > limit.get() {
            return Err(FileSystemReadError::ByteLimitExceeded { limit });
        }
        Ok(bytes.to_vec())
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        self.existing_path(path)
            .map(VirtualAbsolutePath::into_path_buf)
    }

    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
        let path = virtual_path(path)?;
        if self.contains(&path) {
            Ok(FileSystemEntryKind::File)
        } else if self.is_dir(&path) {
            Ok(FileSystemEntryKind::Directory)
        } else {
            Err(not_found(&path))
        }
    }

    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<OsString>, FileSystemReadError> {
        let path = self.existing_path(path)?;
        if !self.is_dir(&path) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                path.as_path().display().to_string(),
            )
            .into());
        }
        self.child_names_bounded(&path, limit, cancellation)
    }

    fn is_file(&self, path: &Path) -> bool {
        VirtualAbsolutePath::new(path.to_path_buf()).is_ok_and(|path| self.contains(&path))
    }

    fn exists(&self, path: &Path) -> bool {
        VirtualAbsolutePath::new(path.to_path_buf())
            .is_ok_and(|path| self.contains(&path) || self.is_dir(&path))
    }
}

fn virtual_path(path: &Path) -> Result<VirtualAbsolutePath, io::Error> {
    VirtualAbsolutePath::new(path.to_path_buf())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn not_found(path: &VirtualAbsolutePath) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        path.as_path().display().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NeverCancel, RealFileSystem};

    const TEST_LIMIT: ByteLimit = ByteLimit::new(1024);

    fn path(value: impl Into<PathBuf>) -> VirtualAbsolutePath {
        VirtualAbsolutePath::new(value).unwrap()
    }

    #[test]
    fn in_memory_read_existing_file() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            path("/project/main.gcl"),
            "param x: Dimensionless = 1.0;".to_string(),
        )
        .unwrap();
        let content = fs
            .read_to_string_bounded(Path::new("/project/main.gcl"), TEST_LIMIT, &NeverCancel)
            .unwrap();
        assert_eq!(content, "param x: Dimensionless = 1.0;");
    }

    #[test]
    fn in_memory_bounded_read_checks_before_cloning() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_binary_file(path("/large"), vec![0; 5]).unwrap();
        let error = fs
            .read_bytes_bounded(Path::new("/large"), ByteLimit::new(4), &NeverCancel)
            .unwrap_err();
        assert!(matches!(
            error,
            FileSystemReadError::ByteLimitExceeded { .. }
        ));
    }

    #[test]
    fn text_and_binary_inserts_replace_the_entire_prior_entry() {
        let mut fs = InMemoryFileSystem::new();
        let file = path("/value");
        fs.add_binary_file(file.clone(), b"old".to_vec()).unwrap();
        fs.add_file(file.clone(), "new".to_string()).unwrap();
        assert_eq!(
            fs.read_bytes_bounded(file.as_path(), TEST_LIMIT, &NeverCancel)
                .unwrap(),
            b"new"
        );

        fs.add_binary_file(file.clone(), vec![0xff]).unwrap();
        assert_eq!(
            fs.read_bytes_bounded(file.as_path(), TEST_LIMIT, &NeverCancel)
                .unwrap(),
            vec![0xff]
        );
        assert_eq!(
            fs.read_to_string_bounded(file.as_path(), TEST_LIMIT, &NeverCancel)
                .unwrap_err()
                .io_kind(),
            Some(io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn normalized_insertions_are_found_through_equivalent_spellings() {
        let mut fs = InMemoryFileSystem::new();
        let spelling = path("/project/sub/../model.gcl");
        assert_eq!(spelling.as_path(), Path::new("/project/model.gcl"));
        fs.add_file(spelling, "model".to_string()).unwrap();

        assert_eq!(
            fs.read_to_string_bounded(Path::new("/project/./model.gcl"), TEST_LIMIT, &NeverCancel)
                .unwrap(),
            "model"
        );
        assert_eq!(
            fs.canonicalize(Path::new("/project/sub/../model.gcl"))
                .unwrap(),
            PathBuf::from("/project/model.gcl")
        );
    }

    #[test]
    fn virtual_paths_reject_relative_and_root_escaping_spellings_in_release_builds() {
        assert!(matches!(
            VirtualAbsolutePath::new("relative/model.gcl"),
            Err(VirtualPathError::NotAbsolute { .. })
        ));
        assert!(matches!(
            VirtualAbsolutePath::new("/../model.gcl"),
            Err(VirtualPathError::EscapesRoot { .. })
        ));

        let fs = InMemoryFileSystem::new();
        let error = fs
            .read_bytes_bounded(Path::new("relative/model.gcl"), TEST_LIMIT, &NeverCancel)
            .unwrap_err();
        assert_eq!(error.io_kind(), Some(io::ErrorKind::InvalidInput));
        assert!(!fs.exists(Path::new("/../model.gcl")));
    }

    #[test]
    fn in_memory_read_missing_file() {
        let fs = InMemoryFileSystem::new();
        let result = fs.read_to_string_bounded(Path::new("/missing.gcl"), TEST_LIMIT, &NeverCancel);
        assert_eq!(result.unwrap_err().io_kind(), Some(io::ErrorKind::NotFound));
    }

    #[test]
    fn in_memory_canonicalize_existing() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(path("/project/main.gcl"), String::new())
            .unwrap();
        let canonical = fs.canonicalize(Path::new("/project/main.gcl")).unwrap();
        assert_eq!(canonical, PathBuf::from("/project/main.gcl"));
    }

    #[test]
    fn in_memory_canonicalize_directory() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(path("/project/sub/file.gcl"), String::new())
            .unwrap();
        let canonical = fs.canonicalize(Path::new("/project/sub")).unwrap();
        assert_eq!(canonical, PathBuf::from("/project/sub"));
    }

    #[test]
    fn in_memory_canonicalize_rejects_parent_of_root() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(path("/project/main.gcl"), String::new())
            .unwrap();
        let error = fs.canonicalize(Path::new("/..")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn in_memory_canonicalize_missing() {
        let fs = InMemoryFileSystem::new();
        assert_eq!(
            fs.canonicalize(Path::new("/missing")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn in_memory_canonicalize_rejects_relative_path() {
        let fs = InMemoryFileSystem::new();
        let error = fs.canonicalize(Path::new("./rel.gcl")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn canonicalization_matches_real_filesystem_dot_and_parent_semantics() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("sub")).unwrap();
        let real = RealFileSystem::default();
        let canonical_directory = real.canonicalize(directory.path()).unwrap();
        let real_file = canonical_directory.join("model.gcl");
        std::fs::write(&real_file, b"model").unwrap();
        let alternate = canonical_directory.join("sub/.././model.gcl");
        let real_canonical = real.canonicalize(&alternate).unwrap();
        let mut memory = InMemoryFileSystem::new();
        memory
            .add_file(path(real_canonical.clone()), "model".to_string())
            .unwrap();

        assert_eq!(memory.canonicalize(&alternate).unwrap(), real_canonical);
    }

    #[test]
    fn in_memory_is_file() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(path("/project/main.gcl"), String::new())
            .unwrap();
        assert!(fs.is_file(Path::new("/project/main.gcl")));
        assert!(!fs.is_file(Path::new("/project")));
        assert!(!fs.is_file(Path::new("relative")));
    }

    #[test]
    fn in_memory_exists_and_lists_direct_children() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(path("/project/sub/file.gcl"), String::new())
            .unwrap();
        fs.add_file(path("/project/other.gcl"), String::new())
            .unwrap();
        assert!(fs.exists(Path::new("/project/sub/file.gcl")));
        assert!(fs.exists(Path::new("/project/sub")));
        assert!(!fs.exists(Path::new("/other")));
        assert_eq!(
            fs.read_directory_bounded(Path::new("/project"), EntryLimit::new(2), &NeverCancel,)
                .unwrap(),
            vec![OsString::from("other.gcl"), OsString::from("sub")]
        );
    }

    #[test]
    fn insertion_rejects_file_directory_topology_conflicts() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(path("/project/file/child.gcl"), String::new())
            .unwrap();
        assert!(matches!(
            fs.add_file(path("/project/file"), String::new()),
            Err(InMemoryFileSystemError::DirectoryAlreadyExists { .. })
        ));

        fs.add_file(path("/project/other.gcl"), String::new())
            .unwrap();
        assert!(matches!(
            fs.add_file(path("/project/other.gcl/child"), String::new()),
            Err(InMemoryFileSystemError::FileAncestor { .. })
        ));
    }
}
