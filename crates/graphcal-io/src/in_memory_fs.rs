//! In-memory filesystem for tests and WASM environments.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::FileSystemReader;

/// In-memory filesystem for tests and WASM environments.
///
/// Paths are stored exactly as inserted — [`canonicalize`](FileSystemReader::canonicalize)
/// returns the path unchanged if the file exists.
#[derive(Debug, Clone, Default)]
pub struct InMemoryFileSystem {
    files: HashMap<PathBuf, String>,
}

impl InMemoryFileSystem {
    /// Create an empty in-memory filesystem.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a file with the given path and content.
    pub fn add_file(&mut self, path: PathBuf, content: String) {
        self.files.insert(path, content);
    }

    /// Returns `true` if `path` is a directory-like prefix of any stored file.
    fn is_dir(&self, path: &Path) -> bool {
        self.files.keys().any(|k| k.starts_with(path) && k != path)
    }
}

impl FileSystemReader for InMemoryFileSystem {
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{}", path.display())))
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        // In-memory: if the path exists (as a file or directory), return it
        // as-is (already "canonical"). Otherwise, propagate NotFound.
        if self.files.contains_key(path) || self.is_dir(path) {
            return Ok(path.to_path_buf());
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{}", path.display()),
        ))
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.is_dir(path)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;

    #[test]
    fn in_memory_read_existing_file() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            PathBuf::from("/project/main.gcl"),
            "param x: Dimensionless = 1.0;".to_string(),
        );
        let content = fs.read_to_string(Path::new("/project/main.gcl")).unwrap();
        assert_eq!(content, "param x: Dimensionless = 1.0;");
    }

    #[test]
    fn in_memory_read_missing_file() {
        let fs = InMemoryFileSystem::new();
        let result = fs.read_to_string(Path::new("/missing.gcl"));
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
    fn in_memory_canonicalize_missing() {
        let fs = InMemoryFileSystem::new();
        assert!(fs.canonicalize(Path::new("/missing")).is_err());
    }

    #[test]
    fn in_memory_is_file() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(PathBuf::from("/project/main.gcl"), String::new());
        assert!(fs.is_file(Path::new("/project/main.gcl")));
        assert!(!fs.is_file(Path::new("/project")));
    }

    #[test]
    fn in_memory_exists() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(PathBuf::from("/project/sub/file.gcl"), String::new());
        assert!(fs.exists(Path::new("/project/sub/file.gcl")));
        assert!(fs.exists(Path::new("/project/sub")));
        assert!(!fs.exists(Path::new("/other")));
    }
}
