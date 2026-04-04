//! Real filesystem implementation backed by `std::fs`.

use std::io;
use std::path::{Path, PathBuf};

use crate::FileSystemReader;

/// Filesystem reader that delegates to the operating system via `std::fs`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFileSystem;

impl FileSystemReader for RealFileSystem {
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
