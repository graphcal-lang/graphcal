//! Filesystem abstraction for the compiler core.
//!
//! The compiler is generic over [`FileSystemReader`], enabling:
//! - Real filesystem access (via `graphcal_io::RealFileSystem`)
//! - In-memory filesystems (for tests and WASM)
//! - Overlay filesystems (for LSP unsaved editor buffers)

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// Abstraction over filesystem read operations.
///
/// The loader ([`crate::loader`]) is generic over this trait so that the
/// compiler core never calls `std::fs` directly.
pub trait FileSystemReader {
    /// Read the entire contents of a file as a UTF-8 string.
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error>;

    /// Return the canonical, absolute form of a path.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error>;

    /// Return `true` if `path` points to a regular file.
    fn is_file(&self, path: &Path) -> bool;

    /// Return `true` if `path` points to an existing filesystem entry.
    fn exists(&self, path: &Path) -> bool;
}

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
}

impl FileSystemReader for InMemoryFileSystem {
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{}", path.display())))
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        // In-memory: if the path exists, return it as-is (already "canonical").
        // Also check parent directories for directory-like queries.
        if self.files.contains_key(path) {
            return Ok(path.to_path_buf());
        }
        // Check if any file has this path as a prefix (i.e., it's a "directory").
        let is_dir = self.files.keys().any(|k| k.starts_with(path) && k != path);
        if is_dir {
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
        if self.files.contains_key(path) {
            return true;
        }
        // Check if it's a "directory" (prefix of any file path).
        self.files.keys().any(|k| k.starts_with(path) && k != path)
    }
}

/// Test-only real filesystem implementation (avoids circular dev-dependency on `graphcal-io`).
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct TestRealFs;

#[cfg(test)]
impl FileSystemReader for TestRealFs {
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error> {
        std::fs::read_to_string(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        path.canonicalize()
    }
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
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
