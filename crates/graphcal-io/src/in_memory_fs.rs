//! In-memory filesystem for tests and WASM environments.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::FileSystemReader;

/// In-memory filesystem for tests and WASM environments.
///
/// All paths must be absolute. [`canonicalize`](FileSystemReader::canonicalize)
/// returns the stored path unchanged when it exists; relative inputs are
/// rejected with `ErrorKind::InvalidInput` because an in-memory filesystem
/// has no current-directory context against which to resolve them. This
/// matches the absolute-path guarantee that `std::fs::canonicalize` upholds
/// on the real filesystem, so test and production behavior stay aligned.
#[derive(Debug, Clone, Default)]
pub struct InMemoryFileSystem {
    files: HashMap<PathBuf, String>,
    binary_files: HashMap<PathBuf, Vec<u8>>,
}

impl InMemoryFileSystem {
    /// Create an empty in-memory filesystem.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a file with the given path and content. `path` must be absolute
    /// — see the type-level invariant.
    pub fn add_file(&mut self, path: PathBuf, content: String) {
        debug_assert!(
            path.is_absolute(),
            "InMemoryFileSystem requires absolute paths, got `{}`",
            path.display()
        );
        self.files.insert(path, content);
    }

    /// Insert a binary file (e.g. a `.wasm` plugin module) with the given
    /// path and content. `path` must be absolute — see the type-level
    /// invariant.
    pub fn add_binary_file(&mut self, path: PathBuf, content: Vec<u8>) {
        debug_assert!(
            path.is_absolute(),
            "InMemoryFileSystem requires absolute paths, got `{}`",
            path.display()
        );
        self.binary_files.insert(path, content);
    }

    fn contains(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.binary_files.contains_key(path)
    }

    /// Returns `true` if `path` is a directory-like prefix of any stored file.
    fn is_dir(&self, path: &Path) -> bool {
        self.files
            .keys()
            .chain(self.binary_files.keys())
            .any(|k| k.starts_with(path) && k != path)
    }
}

impl FileSystemReader for InMemoryFileSystem {
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error> {
        match self.files.get(path) {
            Some(content) => Ok(content.clone()),
            None if self.binary_files.contains_key(path) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is a binary file, not UTF-8 text", path.display()),
            )),
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{}", path.display()),
            )),
        }
    }

    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, io::Error> {
        self.binary_files
            .get(path)
            .cloned()
            .or_else(|| self.files.get(path).map(|text| text.clone().into_bytes()))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{}", path.display())))
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "InMemoryFileSystem::canonicalize requires an absolute path, got `{}`",
                    path.display()
                ),
            ));
        }
        // Lexically normalize `.`/`..` components so behavior matches
        // `std::fs::canonicalize` (which resolves them on disk). Sound here
        // because the in-memory tree has no symlinks.
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    if normalized.parent().is_some() {
                        normalized.pop();
                    }
                }
                other => normalized.push(other),
            }
        }
        if self.contains(&normalized) || self.is_dir(&normalized) {
            return Ok(normalized);
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{}", normalized.display()),
        ))
    }

    fn is_file(&self, path: &Path) -> bool {
        self.contains(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.contains(path) || self.is_dir(path)
    }
}

#[cfg(test)]
mod tests {
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
        // The real `std::fs::canonicalize` resolves relative paths against the
        // process CWD and always returns an absolute result. The in-memory FS
        // has no CWD, so accepting a relative input would silently produce a
        // non-canonical answer and let mock/real divergence slip through.
        let fs = InMemoryFileSystem::new();
        let err = fs.canonicalize(Path::new("./rel.gcl")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
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
