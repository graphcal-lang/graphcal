//! In-memory filesystem for tests and WASM environments.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::{
    ByteLimit, CancellationSignal, EntryLimit, FileSystemEntryKind, FileSystemReadError,
    FileSystemReader,
};

/// In-memory filesystem for tests and WASM environments.
///
/// All paths must be absolute within the virtual tree. On bare Wasm, a path
/// rooted at `/` satisfies this invariant even though the target has no host
/// path prefix.
#[derive(Debug, Clone, Default)]
pub struct InMemoryFileSystem {
    files: HashMap<PathBuf, String>,
    binary_files: HashMap<PathBuf, Vec<u8>>,
}

fn is_virtual_absolute(path: &Path) -> bool {
    path.is_absolute() || (cfg!(target_arch = "wasm32") && path.has_root())
}

fn normalized(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.parent().is_some() {
                    normalized.pop();
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

impl InMemoryFileSystem {
    /// Create an empty in-memory filesystem.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a UTF-8 file. `path` must be absolute in the virtual tree.
    pub fn add_file(&mut self, path: PathBuf, content: String) {
        debug_assert!(
            is_virtual_absolute(&path),
            "InMemoryFileSystem requires absolute paths, got `{}`",
            path.display()
        );
        self.files.insert(path, content);
    }

    /// Insert a binary file. `path` must be absolute in the virtual tree.
    pub fn add_binary_file(&mut self, path: PathBuf, content: Vec<u8>) {
        debug_assert!(
            is_virtual_absolute(&path),
            "InMemoryFileSystem requires absolute paths, got `{}`",
            path.display()
        );
        self.binary_files.insert(path, content);
    }

    fn contains(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.binary_files.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.files
            .keys()
            .chain(self.binary_files.keys())
            .any(|key| key.starts_with(path) && key != path)
    }

    fn child_names_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<OsString>, FileSystemReadError> {
        let mut names = BTreeSet::new();
        for key in self.files.keys().chain(self.binary_files.keys()) {
            if cancellation.is_cancelled() {
                return Err(FileSystemReadError::Cancelled);
            }
            let Ok(relative) = key.strip_prefix(path) else {
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
        let bytes = self
            .binary_files
            .get(path)
            .map(Vec::as_slice)
            .or_else(|| self.files.get(path).map(String::as_bytes))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.display().to_string()))?;
        if bytes.len() as u64 > limit.get() {
            return Err(FileSystemReadError::ByteLimitExceeded { limit });
        }
        Ok(bytes.to_vec())
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        if !is_virtual_absolute(path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "InMemoryFileSystem::canonicalize requires an absolute path, got `{}`",
                    path.display()
                ),
            ));
        }
        let normalized = normalized(path);
        if self.contains(&normalized) || self.is_dir(&normalized) {
            return Ok(normalized);
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            normalized.display().to_string(),
        ))
    }

    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
        let path = normalized(path);
        if self.contains(&path) {
            Ok(FileSystemEntryKind::File)
        } else if self.is_dir(&path) {
            Ok(FileSystemEntryKind::Directory)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                path.display().to_string(),
            ))
        }
    }

    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<OsString>, FileSystemReadError> {
        let canonical = self.canonicalize(path)?;
        if !self.is_dir(&canonical) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                canonical.display().to_string(),
            )
            .into());
        }
        self.child_names_bounded(&canonical, limit, cancellation)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.contains(&normalized(path))
    }

    fn exists(&self, path: &Path) -> bool {
        let path = normalized(path);
        self.contains(&path) || self.is_dir(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NeverCancel;

    const TEST_LIMIT: ByteLimit = ByteLimit::new(1024);

    #[test]
    fn in_memory_read_existing_file() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            PathBuf::from("/project/main.gcl"),
            "param x: Dimensionless = 1.0;".to_string(),
        );
        let content = fs
            .read_to_string_bounded(Path::new("/project/main.gcl"), TEST_LIMIT, &NeverCancel)
            .unwrap();
        assert_eq!(content, "param x: Dimensionless = 1.0;");
    }

    #[test]
    fn in_memory_bounded_read_checks_before_cloning() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_binary_file(PathBuf::from("/large"), vec![0; 5]);
        let error = fs
            .read_bytes_bounded(Path::new("/large"), ByteLimit::new(4), &NeverCancel)
            .unwrap_err();
        assert!(matches!(
            error,
            FileSystemReadError::ByteLimitExceeded { .. }
        ));
    }

    #[test]
    fn in_memory_read_missing_file() {
        let fs = InMemoryFileSystem::new();
        let result = fs.read_to_string_bounded(Path::new("/missing.gcl"), TEST_LIMIT, &NeverCancel);
        assert!(result.is_err());
    }

    #[test]
    fn in_memory_canonicalize_existing() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(PathBuf::from("/project/main.gcl"), String::new());
        let canonical = fs.canonicalize(Path::new("/project/main.gcl")).unwrap();
        assert_eq!(canonical, PathBuf::from("/project/main.gcl"));
    }

    #[test]
    fn in_memory_canonicalize_directory() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(PathBuf::from("/project/sub/file.gcl"), String::new());
        let canonical = fs.canonicalize(Path::new("/project/sub")).unwrap();
        assert_eq!(canonical, PathBuf::from("/project/sub"));
    }

    #[test]
    fn in_memory_canonicalize_parent_of_root_stays_root() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(PathBuf::from("/project/main.gcl"), String::new());
        let canonical = fs.canonicalize(Path::new("/..")).unwrap();
        assert_eq!(canonical, PathBuf::from("/"));
    }

    #[test]
    fn in_memory_canonicalize_missing() {
        let fs = InMemoryFileSystem::new();
        assert!(fs.canonicalize(Path::new("/missing")).is_err());
    }

    #[test]
    fn in_memory_canonicalize_rejects_relative_path() {
        let fs = InMemoryFileSystem::new();
        let error = fs.canonicalize(Path::new("./rel.gcl")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn in_memory_is_file() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(PathBuf::from("/project/main.gcl"), String::new());
        assert!(fs.is_file(Path::new("/project/main.gcl")));
        assert!(!fs.is_file(Path::new("/project")));
    }

    #[test]
    fn in_memory_exists_and_lists_direct_children() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(PathBuf::from("/project/sub/file.gcl"), String::new());
        fs.add_file(PathBuf::from("/project/other.gcl"), String::new());
        assert!(fs.exists(Path::new("/project/sub/file.gcl")));
        assert!(fs.exists(Path::new("/project/sub")));
        assert!(!fs.exists(Path::new("/other")));
        assert_eq!(
            fs.read_directory_bounded(Path::new("/project"), EntryLimit::new(2), &NeverCancel,)
                .unwrap(),
            vec![OsString::from("other.gcl"), OsString::from("sub")]
        );
    }
}
